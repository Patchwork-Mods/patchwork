//! Keeping profiles in step between machines, peer to peer, with iroh.
//!
//! Each synced profile is an iroh-docs document holding the entries of
//! [`patchwork_core::sync`]; iroh-docs keeps the newest value of each key,
//! whoever wrote it. The core knows nothing about networking: this crate
//! only goes through those entries, [`Patchwork::apply_synced`] and
//! friends, and [`Patchwork::watch_profiles`].
//!
//! Mods aren't sent between machines, only what they are; each machine
//! downloads them from their sources itself. Local mods, which have no
//! source, are the exception: their files go in the document too, under
//! `file/` (see [`files`]), and machines that don't have them get them from
//! one that does.
//!
//! Synced changes are saved and downloaded straight away, but never
//! deployed: that's always up to whoever's at the machine.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::{Stream, StreamExt};
use iroh::endpoint::presets::{self, Preset};
use iroh::protocol::Router;
use iroh::{Endpoint, PublicKey, SecretKey};
use iroh_blobs::api::downloader::Downloader;
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{BlobsProtocol, Hash};
use iroh_docs::api::Doc;
use iroh_docs::api::protocol::{AddrInfoOptions, ShareMode};
use iroh_docs::engine::LiveEvent;
use iroh_docs::protocol::Docs;
use iroh_docs::store::Query;
use iroh_docs::{AuthorId, CapabilityKind, DocTicket, NamespaceId};
use patchwork_core::profile::ProfileRef;
use patchwork_core::sync::{Entries, FILE, MOD};
use patchwork_core::{Error, Patchwork, SyncApplied};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

pub mod files;

/// How long a one-off sync waits to hear from other machines.
pub const SYNC_TIMEOUT: Duration = Duration::from_secs(20);

/// How long a one-off sync stays around after handing out local mods'
/// files, so machines online at the time can fetch them.
const SENT_FILES_GRACE: Duration = Duration::from_secs(5);

/// How long fetching one file from other machines can take before it's
/// left for the next sync.
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// How often a running [`Node`] checks synced profiles for changes made by
/// hand, which [`Patchwork::watch_profiles`] can't see.
const HAND_EDITS_EVERY: Duration = Duration::from_secs(2);

/// A profile this machine syncs, as kept in `<data>/sync/profiles.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncedProfile {
    pub profile: Uuid,
    /// The iroh-docs document it's synced through.
    pub doc: String,
    /// Whether this machine can change it, or only follows it.
    pub writable: bool,
}

impl SyncedProfile {
    pub fn reference(&self) -> ProfileRef {
        ProfileRef { uuid: self.profile }
    }

    fn namespace(&self) -> Result<NamespaceId> {
        self.doc
            .parse()
            .map_err(|e| anyhow!("bad document id `{}`: {e}", self.doc))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SyncedProfiles {
    #[serde(default, rename = "profile")]
    profiles: Vec<SyncedProfile>,
}

/// What syncing a profile once did.
#[derive(Debug)]
pub struct Outcome {
    pub profile: ProfileRef,
    /// `None` if nothing's arrived from the document yet.
    pub applied: Option<SyncApplied>,
    /// How many local mods' files came from other machines.
    pub received_files: usize,
    /// How many files of local mods this machine has were handed out.
    pub sent_files: usize,
    /// Things that didn't work but didn't stop the sync, like a mod that
    /// couldn't be locked or downloaded.
    pub warnings: Vec<String>,
}

/// This machine's iroh node: its endpoint, and the documents and blobs it
/// syncs. Everything it keeps is under [`patchwork_core::paths::Paths::sync`].
pub struct Node {
    router: Router,
    blobs: FsStore,
    downloader: Downloader,
    docs: Docs,
    author: AuthorId,
    dir: PathBuf,
    /// Something happened in one of the documents, by profile.
    doc_events: mpsc::UnboundedSender<Uuid>,
    doc_events_rx: Mutex<Option<mpsc::UnboundedReceiver<Uuid>>>,
    /// Documents whose events are already being passed on.
    watching: Mutex<HashMap<Uuid, ()>>,
}

impl Node {
    /// Starts the node, with the key this machine always uses, made the
    /// first time. Other machines are found through iroh's relays and
    /// address lookup.
    pub async fn spawn(dir: &Path) -> Result<Self> {
        Self::spawn_with(dir, presets::N0).await
    }

    /// [`Node::spawn`], with the endpoint set up by `preset`. With
    /// [`presets::Minimal`], other machines can only be reached at the
    /// addresses in a ticket, which is how the tests run offline.
    pub async fn spawn_with(dir: &Path, preset: impl Preset) -> Result<Self> {
        fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
        let key = secret_key(&dir.join("secret.key"))?;
        let endpoint = Endpoint::builder(preset)
            .secret_key(key)
            .bind()
            .await
            .context("starting the network endpoint")?;
        let blobs = FsStore::load(dir.join("blobs"))
            .await
            .context("opening the blob store")?;
        let gossip = iroh_gossip::Gossip::builder().spawn(endpoint.clone());
        let docs_dir = dir.join("docs");
        fs::create_dir_all(&docs_dir).with_context(|| docs_dir.display().to_string())?;
        let docs = Docs::persistent(docs_dir)
            .spawn(endpoint.clone(), (*blobs).clone(), gossip.clone())
            .await
            .context("opening the document store")?;
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&blobs, None))
            .accept(iroh_gossip::ALPN, gossip)
            .accept(iroh_docs::ALPN, docs.clone())
            .spawn();
        let author = docs.author_default().await?;
        let downloader = blobs.downloader(router.endpoint());
        let (doc_events, rx) = mpsc::unbounded_channel();
        Ok(Self {
            router,
            blobs,
            downloader,
            docs,
            author,
            dir: dir.to_owned(),
            doc_events,
            doc_events_rx: Mutex::new(Some(rx)),
            watching: Mutex::default(),
        })
    }

    pub async fn shutdown(self) -> Result<()> {
        self.router.shutdown().await?;
        // the blob store can stop without answering, which is just as
        // stopped
        let _ = self.blobs.shutdown().await;
        Ok(())
    }

    /// The profiles this machine syncs.
    pub fn synced(&self) -> Result<Vec<SyncedProfile>> {
        synced_profiles(&self.dir)
    }

    pub fn synced_profile(&self, profile: &ProfileRef) -> Result<Option<SyncedProfile>> {
        Ok(self
            .synced()?
            .into_iter()
            .find(|s| s.profile == profile.uuid))
    }

    fn save_synced(&self, profiles: Vec<SyncedProfile>) -> Result<()> {
        let text = toml::to_string(&SyncedProfiles { profiles })?;
        let path = self.profiles_file();
        fs::write(&path, text).with_context(|| path.display().to_string())
    }

    fn remember(&self, synced: SyncedProfile) -> Result<()> {
        let mut all = self.synced()?;
        all.retain(|s| s.profile != synced.profile);
        all.push(synced);
        self.save_synced(all)
    }

    fn profiles_file(&self) -> PathBuf {
        profiles_file(&self.dir)
    }

    /// A ticket another machine can join `profile` with, starting to sync
    /// it if it isn't yet. With `writable`, whoever has the ticket can
    /// change the profile too; otherwise they can only follow it. Write
    /// access can't be taken back once it's given, short of moving
    /// everyone else to a new document.
    pub async fn share(
        &self,
        pw: &Patchwork,
        profile: &ProfileRef,
        writable: bool,
    ) -> Result<String> {
        let doc = match self.synced_profile(profile)? {
            Some(synced) => {
                if writable && !synced.writable {
                    bail!(
                        "this machine only follows the profile, so it can't let others change it"
                    );
                }
                self.open(&synced).await?
            }
            None => {
                let doc = self.docs.create().await?;
                let entries = pw.start_sync(profile)?;
                self.write(&doc, entries.into_iter().collect()).await?;
                self.remember(SyncedProfile {
                    profile: profile.uuid,
                    doc: doc.id().to_string(),
                    writable: true,
                })?;
                files::publish(self, pw, &doc, profile).await?;
                doc
            }
        };
        self.watch(profile.uuid, &doc).await?;
        doc.start_sync(Vec::new()).await?;
        let mode = if writable {
            ShareMode::Write
        } else {
            ShareMode::Read
        };
        let ticket = doc.share(mode, AddrInfoOptions::RelayAndAddresses).await?;
        Ok(iroh_tickets::Ticket::encode_string(&ticket))
    }

    /// Joins a profile shared with [`Node::share`], waiting up to `timeout`
    /// for it to arrive, and saves it here. Its mods are downloaded, but
    /// nothing is deployed.
    pub async fn join(&self, pw: &Patchwork, ticket: &str, timeout: Duration) -> Result<Outcome> {
        let ticket: DocTicket = ticket.trim().parse().context("that isn't a sync ticket")?;
        let writable = matches!(ticket.capability.kind(), CapabilityKind::Write);
        let (doc, events) = self.docs.import_and_subscribe(ticket).await?;
        wait_for_sync(events, timeout).await;
        let entries = self.read_entries(&doc).await?;
        if entries.is_empty() {
            bail!(
                "nothing arrived from the other machine within {}s; \
                 check that it's still sharing and try again",
                timeout.as_secs()
            );
        }
        let applied = pw.apply_synced(&entries, writable)?;
        let synced = SyncedProfile {
            profile: applied.profile.uuid,
            doc: doc.id().to_string(),
            writable,
        };
        self.remember(synced.clone())?;
        self.watch(synced.profile, &doc).await?;
        self.finish_step(pw, &doc, &synced, Some(applied), Vec::new())
            .await
    }

    /// Stops syncing `profile` here. It stays on this machine as it is.
    pub async fn leave(&self, pw: &Patchwork, profile: &ProfileRef) -> Result<()> {
        let Some(synced) = self.synced_profile(profile)? else {
            bail!("that profile isn't synced");
        };
        let id = synced.namespace()?;
        if let Some(doc) = self.docs.open(id).await? {
            doc.leave().await?;
            doc.close().await?;
        }
        self.docs.drop_doc(id).await?;
        pw.stop_sync(profile)?;
        let mut all = self.synced()?;
        all.retain(|s| s.profile != profile.uuid);
        self.save_synced(all)
    }

    /// Syncs every synced profile once with whichever other machines can be
    /// reached within `timeout`, then stops.
    pub async fn sync_once(&self, pw: &Patchwork, timeout: Duration) -> Result<Vec<Outcome>> {
        let mut outcomes = Vec::new();
        for synced in self.synced()? {
            let doc = self.open(&synced).await?;
            let events = doc.subscribe().await?;
            // what's changed here goes out first
            let mut warnings = self.publish_local(pw, &doc, &synced).await?;
            doc.start_sync(Vec::new()).await?;
            wait_for_sync(events, timeout).await;
            let mut outcome = self.step(pw, &doc, &synced).await?;
            warnings.append(&mut outcome.warnings);
            outcome.warnings = warnings;
            if outcome.sent_files > 0 {
                tokio::time::sleep(SENT_FILES_GRACE).await;
            }
            doc.leave().await?;
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }

    /// Keeps every synced profile in step until it's stopped, telling
    /// `on_sync` what each sync did. Profiles shared or joined while it
    /// runs are picked up.
    pub async fn run(&self, pw: &Patchwork, on_sync: impl Fn(Result<Outcome>)) -> Result<()> {
        let mut doc_events = self
            .doc_events_rx
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow!("the node is already running"))?;
        let mut changes = pw.watch_profiles();
        let mut hand_edits = tokio::time::interval(HAND_EDITS_EVERY);
        for synced in self.synced()? {
            let doc = self.open(&synced).await?;
            self.watch(synced.profile, &doc).await?;
            doc.start_sync(Vec::new()).await?;
            on_sync(self.sync_profile(pw, &synced).await);
        }
        loop {
            let profile = tokio::select! {
                Some(profile) = doc_events.recv() => profile,
                change = changes.recv() => match change {
                    Ok(change) if !change.synced => change.profile.uuid,
                    Ok(_) => continue,
                    // too many at once; the next check catches up
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                },
                _ = hand_edits.tick() => {
                    for synced in self.synced()? {
                        let changed = synced.writable
                            && !pw.unsynced_changes(&synced.reference()).unwrap_or_default().is_empty();
                        if changed {
                            on_sync(self.sync_profile(pw, &synced).await);
                        }
                    }
                    continue;
                }
            };
            // a burst of events is usually one change arriving, so each
            // profile in it is synced once
            let mut due = BTreeSet::from([profile]);
            while let Ok(more) = doc_events.try_recv() {
                due.insert(more);
            }
            for uuid in due {
                if let Some(synced) = self.synced_profile(&ProfileRef { uuid })? {
                    on_sync(self.sync_profile(pw, &synced).await);
                }
            }
        }
    }

    async fn sync_profile(&self, pw: &Patchwork, synced: &SyncedProfile) -> Result<Outcome> {
        let doc = self.open(synced).await?;
        let mut warnings = self.publish_local(pw, &doc, synced).await?;
        let mut outcome = self.step(pw, &doc, synced).await?;
        warnings.append(&mut outcome.warnings);
        outcome.warnings = warnings;
        Ok(outcome)
    }

    /// Publishes what's changed here since the last sync. Whoever changes
    /// the profile relocks it, so a change to its mods is locked first and
    /// the lockfile goes out with it.
    async fn publish_local(
        &self,
        pw: &Patchwork,
        doc: &Doc,
        synced: &SyncedProfile,
    ) -> Result<Vec<String>> {
        let profile = synced.reference();
        let mut warnings = Vec::new();
        if !synced.writable || pw.load_profile(&profile).is_err() {
            return Ok(warnings);
        }
        let mut changes = pw.unsynced_changes(&profile)?;
        if changes.iter().any(|(key, _)| key.starts_with(MOD)) {
            match pw.lock(&profile, false).await {
                Ok(_) => changes = pw.unsynced_changes(&profile)?,
                // the change goes out anyway, to be locked wherever it's
                // next locked
                Err(e) => warnings.push(format!("couldn't lock the profile: {e}")),
            }
        }
        self.write(doc, changes).await?;
        Ok(warnings)
    }

    /// Takes what the document has now, publishes anything of this
    /// machine's it hasn't got yet, and fetches what the profile needs.
    async fn step(&self, pw: &Patchwork, doc: &Doc, synced: &SyncedProfile) -> Result<Outcome> {
        let entries = self.read_entries(doc).await?;
        if entries.is_empty() {
            return Ok(Outcome {
                profile: synced.reference(),
                applied: None,
                received_files: 0,
                sent_files: 0,
                warnings: Vec::new(),
            });
        }
        let applied = pw.apply_synced(&entries, synced.writable)?;
        self.finish_step(pw, doc, synced, Some(applied), Vec::new())
            .await
    }

    async fn finish_step(
        &self,
        pw: &Patchwork,
        doc: &Doc,
        synced: &SyncedProfile,
        mut applied: Option<SyncApplied>,
        mut warnings: Vec<String>,
    ) -> Result<Outcome> {
        let profile = synced.reference();
        if let Some(applied) = &mut applied
            && synced.writable
        {
            self.write(doc, std::mem::take(&mut applied.unpublished))
                .await?;
        }
        let sent_files = if synced.writable {
            files::publish(self, pw, doc, &profile).await?
        } else {
            0
        };
        let received_files = files::receive(self, pw, doc, &profile, &mut warnings).await?;
        match pw.download_locked(&profile).await {
            Ok(()) => {}
            Err(Error::NeedsDownload(pending)) => {
                for p in pending {
                    warnings.push(format!(
                        "`{}` has to be downloaded by hand: {}",
                        p.name, p.page
                    ));
                }
            }
            Err(e) => warnings.push(e.to_string()),
        }
        Ok(Outcome {
            profile,
            applied,
            received_files,
            sent_files,
            warnings,
        })
    }

    async fn write(&self, doc: &Doc, changes: Vec<(String, Vec<u8>)>) -> Result<()> {
        for (key, value) in changes {
            doc.set_bytes(self.author, doc_key(&key), value).await?;
        }
        Ok(())
    }

    /// The newest value of every key in `doc` but the sync layer's own,
    /// leaving out any whose content hasn't arrived yet.
    async fn read_entries(&self, doc: &Doc) -> Result<Entries> {
        let mut entries = Entries::new();
        let stream = doc.get_many(Query::single_latest_per_key().build()).await?;
        let mut stream = std::pin::pin!(stream);
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            let Some(key) = entry_key(entry.key()) else {
                continue;
            };
            if key.starts_with(FILE) {
                continue;
            }
            match self.blobs.get_bytes(entry.content_hash()).await {
                Ok(value) => {
                    entries.insert(key.to_owned(), value.to_vec());
                }
                // it'll be taken once it's here
                Err(_) => continue,
            }
        }
        Ok(entries)
    }

    async fn open(&self, synced: &SyncedProfile) -> Result<Doc> {
        self.docs
            .open(synced.namespace()?)
            .await?
            .ok_or_else(|| anyhow!("the document for profile {} is gone", synced.profile))
    }

    /// Passes on `doc`'s events for [`Node::run`], once.
    async fn watch(&self, profile: Uuid, doc: &Doc) -> Result<()> {
        if self.watching.lock().unwrap().insert(profile, ()).is_some() {
            return Ok(());
        }
        let mut events = doc.subscribe().await?;
        let tx = self.doc_events.clone();
        tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let worth_a_look = matches!(
                    event,
                    Ok(LiveEvent::InsertRemote { .. }
                        | LiveEvent::ContentReady { .. }
                        | LiveEvent::PendingContentReady)
                );
                if worth_a_look && tx.send(profile).is_err() {
                    return;
                }
            }
        });
        Ok(())
    }

    /// Downloads blob `hash` from whichever machine syncing `doc` has it.
    pub(crate) async fn fetch(&self, doc: &Doc, hash: Hash) -> Result<()> {
        let peers: Vec<PublicKey> = doc
            .get_sync_peers()
            .await?
            .unwrap_or_default()
            .iter()
            .filter_map(|p| PublicKey::from_bytes(p).ok())
            .collect();
        if peers.is_empty() {
            bail!("no other machine is known to have it");
        }
        tokio::time::timeout(FETCH_TIMEOUT, self.downloader.download(hash, peers))
            .await
            .context("timed out")??;
        Ok(())
    }

    pub(crate) fn blobs(&self) -> &FsStore {
        &self.blobs
    }

    pub(crate) fn author(&self) -> AuthorId {
        self.author
    }

    pub(crate) fn scratch(&self) -> PathBuf {
        self.dir.join("incoming")
    }
}

/// The profiles synced from `dir` (see [`Node::spawn`]), without starting
/// a node.
pub fn synced_profiles(dir: &Path) -> Result<Vec<SyncedProfile>> {
    let path = profiles_file(dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).context(path.display().to_string()),
    };
    let list: SyncedProfiles = toml::from_str(&text).with_context(|| path.display().to_string())?;
    Ok(list.profiles)
}

fn profiles_file(dir: &Path) -> PathBuf {
    dir.join("profiles.toml")
}

/// A key as it's written to a document. iroh-docs takes a new entry to
/// replace its author's older entries whose keys start with its key (that's
/// how it deletes), so no key may start with another, which `mod/url:x` and
/// `mod/url:x.zip`, say, would. Ending every key with a NUL, which no key
/// has otherwise, rules that out.
pub(crate) fn doc_key(key: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(key.len() + 1);
    bytes.extend_from_slice(key.as_bytes());
    bytes.push(0);
    bytes
}

/// The key of a document's entry, or `None` if it isn't one of ours.
pub(crate) fn entry_key(bytes: &[u8]) -> Option<&str> {
    let key = std::str::from_utf8(bytes.strip_suffix(&[0])?).ok()?;
    (!key.contains('\0')).then_some(key)
}

/// Waits until a sync with another machine has finished and what it
/// brought has downloaded, or until `timeout`.
async fn wait_for_sync(events: impl Stream<Item = Result<LiveEvent>>, timeout: Duration) {
    let mut events = std::pin::pin!(events);
    let wait = async {
        let mut synced = false;
        while let Some(event) = events.next().await {
            match event {
                Ok(LiveEvent::SyncFinished(e)) if e.result.is_ok() => synced = true,
                Ok(LiveEvent::PendingContentReady) if synced => return,
                _ => {}
            }
        }
    };
    let _ = tokio::time::timeout(timeout, wait).await;
}

/// This machine's key, which other machines know it by.
fn secret_key(path: &Path) -> Result<SecretKey> {
    match fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow!("{} isn't a key", path.display()))?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            write_private(path, &key.to_bytes())?;
            Ok(key)
        }
        Err(e) => Err(e).context(path.display().to_string()),
    }
}

/// Writes a file only this user can read, where that's up to the file.
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| path.display().to_string())?;
        file.write_all(contents)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, contents).with_context(|| path.display().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_document_key_starts_with_another() {
        let short = doc_key("mod/url:https://example.com/a.zip");
        let long = doc_key("mod/url:https://example.com/a.zip.old");
        assert!(!long.starts_with(&short));
        assert_eq!(
            entry_key(&long),
            Some("mod/url:https://example.com/a.zip.old")
        );
        // keys someone else wrote, without the ending, aren't ours
        assert_eq!(entry_key(b"mod/x"), None);
    }
}
