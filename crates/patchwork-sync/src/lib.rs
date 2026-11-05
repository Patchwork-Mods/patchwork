//! Keeping profiles in step between machines, peer to peer, with iroh.
//!
//! Each synced profile is an iroh-docs document holding the entries of
//! [`patchwork_core::sync`]; iroh-docs keeps the newest value of each key,
//! whoever wrote it. The core knows nothing about networking: this crate
//! only goes through those entries, [`Patchwork::apply_synced`] and
//! friends, and [`Patchwork::watch_profiles`].
//!
//! There are three ways to share:
//!
//! - Between one person's own computers ([`Node::link_devices`]): every
//!   profile on any of them, including ones made later, is kept in step on
//!   all of them. A devices document lists each profile's document, as
//!   `profile/<uuid>`, with a ticket to it.
//! - With friends ([`Node::share`], writable): everyone can change the
//!   profile, and every change goes to everyone. Whoever can write to a
//!   document puts themselves in it as `peer/<endpoint id>`, so everyone
//!   knows everyone else, and gets changes from whoever's online, not just
//!   from whoever made them.
//! - Read only ([`Node::share`], not writable): others follow the profile.
//!   They can add mods of their own, which stay on their computer (see
//!   [`Patchwork::unshared_mods`]). Only those with write access can change
//!   what's in the document: iroh-docs refuses entries not signed with its
//!   key, and every file is checked against its hash, so followers can pass
//!   the profile on to each other but can't slip anything into it.
//!
//! Mods aren't sent between machines, only what they are; each machine
//! downloads them from their sources itself. Local mods, which have no
//! source, are the exception: their files go in the document too, under
//! `file/` (see [`files`]), and machines that don't have them get them from
//! one that does.
//!
//! Synced changes are saved and downloaded straight away, but never
//! deployed: that's always up to whoever's at the machine.
//!
//! Write access can't be taken back from a document, so removing a friend
//! moves everyone else to a new one (see [`moves`]).

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::stream::FuturesUnordered;
use futures_util::{Stream, StreamExt};
use iroh::endpoint::presets::{self, Preset};
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, PublicKey, SecretKey};
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
use patchwork_core::sync::{Entries, FILE, MOD, MOVED, PEER, REMOVED};
use patchwork_core::{Error, Patchwork, SyncApplied};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

pub mod daemon;
pub mod files;
pub mod moves;

pub use moves::Removed;

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

/// Marks the devices document (see the [crate docs](crate)), so that
/// joining can tell it from a profile's.
const DEVICES: &str = "devices";

/// In the devices document, a ticket to a profile's document, or
/// [`REMOVED`] once it's no longer synced.
const DEVICE_PROFILE: &str = "profile/";

/// A profile this machine syncs, as kept in `<data>/sync/profiles.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncedProfile {
    pub profile: Uuid,
    /// The iroh-docs document it's synced through.
    pub doc: String,
    /// Whether this machine can change it, or only follows it.
    pub writable: bool,
    /// Whether it's been on this machine, so that if it's gone, it was
    /// removed, rather than not having arrived yet.
    #[serde(default)]
    pub arrived: bool,
    /// Whether its document has moved somewhere this machine hasn't been
    /// told about yet (see [`moves`]), so changes from it aren't taken.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub moved: bool,
    /// The document it was synced through before it last moved, so word
    /// of that move is known to be old news.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<String>,
    /// The machines, by endpoint id, still to be told it moved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub to_tell: Vec<String>,
}

impl SyncedProfile {
    fn new(profile: Uuid, doc: &Doc, writable: bool, arrived: bool) -> Self {
        Self {
            profile,
            doc: doc.id().to_string(),
            writable,
            arrived,
            moved: false,
            moved_from: None,
            to_tell: Vec::new(),
        }
    }

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
    /// The devices document, if this machine's linked to others.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    devices: Option<String>,
    #[serde(default, rename = "profile")]
    profiles: Vec<SyncedProfile>,
}

/// What [`Node::join`] joined.
#[derive(Debug)]
pub enum Joined {
    /// A profile, shared with [`Node::share`].
    Profile(Outcome),
    /// The user's other computers, with this many profiles between them.
    Devices { profiles: usize },
}

/// What a ticket says about where to find this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Addresses {
    /// Its relay and its IP addresses, so others can connect straight to
    /// it. For tickets handed to people the user knows.
    #[default]
    Included,
    /// Only its relay, to start from, which keeps its IP addresses out of a
    /// ticket that's posted publicly. Once others connect they may still
    /// learn them, since iroh trades them to connect directly.
    Hidden,
}

impl Addresses {
    fn options(self) -> AddrInfoOptions {
        match self {
            Addresses::Included => AddrInfoOptions::RelayAndAddresses,
            Addresses::Hidden => AddrInfoOptions::Relay,
        }
    }
}

/// A document [`Node::run`] hears from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Watched {
    Devices,
    Profile(Uuid),
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
    /// Whether the profile's document has moved (see [`moves`]) without
    /// this machine being told where, so nothing was synced. Someone
    /// syncing it hands the new one over when they're next both online,
    /// unless this machine was removed, or only follows the profile, in
    /// which case it needs a new ticket.
    pub moved: bool,
}

impl Outcome {
    fn nothing(profile: ProfileRef) -> Self {
        Self {
            profile,
            applied: None,
            received_files: 0,
            sent_files: 0,
            warnings: Vec::new(),
            moved: false,
        }
    }
}

/// A `peer/` entry: a machine syncing a document.
struct Peer {
    id: PublicKey,
    /// Where it was last, if that's arrived.
    addr: Option<EndpointAddr>,
    /// Its id was reset (see [`Node::reset_id`]), so it's not there.
    gone: bool,
}

impl Peer {
    fn addr(&self) -> EndpointAddr {
        self.addr
            .clone()
            .unwrap_or_else(|| EndpointAddr::new(self.id))
    }
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
    /// Something happened in one of the documents.
    doc_events: mpsc::UnboundedSender<Watched>,
    doc_events_rx: Mutex<Option<mpsc::UnboundedReceiver<Watched>>>,
    /// Documents whose events are being passed on, by what passes them on.
    watching: Mutex<HashMap<Watched, tokio::task::AbortHandle>>,
    /// The handle to each document opened so far. iroh-docs counts every
    /// handle it gives out, and won't drop a document until they're all
    /// closed, so each is only opened once.
    open_docs: Mutex<HashMap<NamespaceId, Doc>>,
    /// Word of moves from other machines, for [`Node::run`] or
    /// [`Node::sync_once`] to take.
    moves_rx: Mutex<Option<mpsc::UnboundedReceiver<moves::Incoming>>>,
    /// The machines being told of a move right now, by profile.
    telling: Mutex<HashSet<(Uuid, PublicKey)>>,
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
        let (moves_tx, moves_rx) = mpsc::unbounded_channel();
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, BlobsProtocol::new(&blobs, None))
            .accept(iroh_gossip::ALPN, gossip)
            .accept(iroh_docs::ALPN, docs.clone())
            .accept(moves::ALPN, moves::Handler::new(moves_tx))
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
            open_docs: Mutex::default(),
            moves_rx: Mutex::new(Some(moves_rx)),
            telling: Mutex::default(),
        })
    }

    pub async fn shutdown(self) -> Result<()> {
        self.router.shutdown().await?;
        // the blob store can stop without answering, which is just as
        // stopped
        let _ = self.blobs.shutdown().await;
        Ok(())
    }

    /// The id other machines know this one by. It's the same in every
    /// document, until [`Node::reset_id`].
    pub fn id(&self) -> PublicKey {
        self.router.endpoint().id()
    }

    /// Gives this machine a new id, which it's known by from when it's
    /// next started. Every document it could announce itself in is told
    /// the old id is gone, so others stop trying to reach it there; that
    /// goes out the next time it syncs. Profiles stay synced as they are,
    /// since what lets a machine into a document is its ticket, not its
    /// id. Tickets handed out before still work, but as they point at the
    /// old id, joining with one finds this machine only through others
    /// already syncing.
    pub async fn reset_id(self) -> Result<PublicKey> {
        let old = self.id();
        let key = format!("{PEER}{old}");
        let mut docs = Vec::new();
        if let Some(doc) = self.devices_doc().await? {
            docs.push(doc);
        }
        for synced in self.synced()? {
            if synced.writable
                && let Some(doc) = self.doc(synced.namespace()?).await?
            {
                docs.push(doc);
            }
        }
        let mut written = Ok(());
        for doc in &docs {
            written = self.write(doc, vec![(key.clone(), REMOVED.to_vec())]).await;
            if written.is_err() {
                break;
            }
        }
        let dir = self.dir.clone();
        self.shutdown().await?;
        // a machine that's still `old` writes itself back the next time it
        // syncs, so a failure part way leaves nothing to undo
        written?;
        new_secret_key(&dir.join("secret.key"))
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

    /// Whether every profile here is kept in step with the user's other
    /// computers; see [`Node::link_devices`].
    pub fn devices_linked(&self) -> Result<bool> {
        devices_linked(&self.dir)
    }

    fn save_list(&self, list: &SyncedProfiles) -> Result<()> {
        let text = toml::to_string(list)?;
        let path = self.profiles_file();
        fs::write(&path, text).with_context(|| path.display().to_string())
    }

    fn remember(&self, synced: SyncedProfile) -> Result<()> {
        let mut list = read_list(&self.dir)?;
        list.profiles.retain(|s| s.profile != synced.profile);
        list.profiles.push(synced);
        self.save_list(&list)
    }

    fn profiles_file(&self) -> PathBuf {
        profiles_file(&self.dir)
    }

    /// A ticket another machine can join `profile` with, starting to sync
    /// it if it isn't yet. With `writable`, whoever has the ticket can
    /// change the profile too; otherwise they can only follow it. Write
    /// access can't be taken back once it's given, short of moving
    /// everyone else to a new document ([`Node::remove_peers`]).
    pub async fn share(
        &self,
        pw: &Patchwork,
        profile: &ProfileRef,
        writable: bool,
        addresses: Addresses,
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
                let doc = self.keep(self.docs.create().await?).await?;
                let entries = pw.start_sync(profile)?;
                self.write(&doc, entries.into_iter().collect()).await?;
                self.remember(SyncedProfile::new(profile.uuid, &doc, true, true))?;
                self.announce(&doc).await?;
                files::publish(self, pw, &doc, profile).await?;
                doc
            }
        };
        self.watch(Watched::Profile(profile.uuid), &doc).await?;
        self.start(&doc).await?;
        let mode = if writable {
            ShareMode::Write
        } else {
            ShareMode::Read
        };
        ticket(&doc, mode, addresses).await
    }

    /// Joins whatever a ticket from [`Node::share`] or
    /// [`Node::link_devices`] is for, waiting up to `timeout` for it to
    /// arrive. A profile is saved here and its mods downloaded, but nothing
    /// is deployed. Linked computers' profiles arrive in the background,
    /// while [`Node::run`] runs.
    pub async fn join(&self, pw: &Patchwork, ticket: &str, timeout: Duration) -> Result<Joined> {
        let ticket: DocTicket = ticket.trim().parse().context("that isn't a sync ticket")?;
        let writable = matches!(ticket.capability.kind(), CapabilityKind::Write);
        let (doc, events) = self.docs.import_and_subscribe(ticket).await?;
        let doc = self.keep(doc).await?;
        wait_for_sync(events, timeout).await;
        let entries = self.read_entries(&doc).await?;
        if entries.is_empty() {
            bail!(
                "nothing arrived from the other computer within {}s; \
                 check that it's still sharing and try again",
                timeout.as_secs()
            );
        }
        if entries.contains_key(DEVICES) {
            return self.join_devices(pw, doc, &entries).await;
        }
        let applied = pw.apply_synced(&entries, writable)?;
        let synced = SyncedProfile::new(applied.profile.uuid, &doc, writable, true);
        self.remember(synced.clone())?;
        self.watch(Watched::Profile(synced.profile), &doc).await?;
        let outcome = self
            .finish_step(pw, &doc, &synced, Some(applied), Vec::new())
            .await?;
        // the user's other computers get it too
        if let Some(devices) = self.devices_doc().await? {
            self.sync_devices(pw, &devices).await?;
        }
        Ok(Joined::Profile(outcome))
    }

    async fn join_devices(&self, pw: &Patchwork, doc: Doc, entries: &Entries) -> Result<Joined> {
        let mut list = read_list(&self.dir)?;
        let id = doc.id().to_string();
        if list.devices.as_ref().is_some_and(|d| *d != id) {
            bail!("this computer is already linked to others; unlink it first");
        }
        list.devices = Some(id);
        self.save_list(&list)?;
        self.announce(&doc).await?;
        self.watch(Watched::Devices, &doc).await?;
        self.sync_devices(pw, &doc).await?;
        let profiles = entries
            .iter()
            .filter(|(k, v)| k.starts_with(DEVICE_PROFILE) && v.as_slice() != REMOVED)
            .count();
        Ok(Joined::Devices { profiles })
    }

    /// Stops syncing `profile` here. It stays on this machine as it is, and
    /// on any others as they are, but linked computers no longer pick it up.
    pub async fn leave(&self, pw: &Patchwork, profile: &ProfileRef) -> Result<()> {
        let Some(synced) = self.synced_profile(profile)? else {
            bail!("that profile isn't synced");
        };
        self.drop_doc(Watched::Profile(profile.uuid), synced.namespace()?)
            .await?;
        pw.stop_sync(profile)?;
        let mut list = read_list(&self.dir)?;
        list.profiles.retain(|s| s.profile != profile.uuid);
        self.save_list(&list)?;
        if let Some(devices) = self.devices_doc().await? {
            let key = format!("{DEVICE_PROFILE}{}", profile.uuid);
            self.write(&devices, vec![(key, REMOVED.to_vec())]).await?;
        }
        Ok(())
    }

    /// A ticket to link another of the user's own computers to this one.
    /// From then on, every profile on any of them (including ones made
    /// later, and ones shared with friends or followed) is kept in step on
    /// all of them. Whoever has the ticket can change all of them, so it's
    /// only for the user's own computers.
    pub async fn link_devices(&self, pw: &Patchwork) -> Result<String> {
        let doc = match self.devices_doc().await? {
            Some(doc) => doc,
            None => {
                let doc = self.keep(self.docs.create().await?).await?;
                self.write(&doc, vec![(DEVICES.to_owned(), b"{}".to_vec())])
                    .await?;
                let mut list = read_list(&self.dir)?;
                list.devices = Some(doc.id().to_string());
                self.save_list(&list)?;
                doc
            }
        };
        self.announce(&doc).await?;
        self.watch(Watched::Devices, &doc).await?;
        self.start(&doc).await?;
        self.sync_devices(pw, &doc).await?;
        ticket(&doc, ShareMode::Write, Addresses::Included).await
    }

    /// Stops keeping every profile in step with the user's other computers.
    /// Profiles synced so far stay synced; stop each with [`Node::leave`].
    pub async fn unlink_devices(&self) -> Result<()> {
        let mut list = read_list(&self.dir)?;
        let Some(id) = list.devices.take() else {
            bail!("this computer isn't linked to any others");
        };
        let id: NamespaceId = id
            .parse()
            .map_err(|e| anyhow!("bad document id `{id}`: {e}"))?;
        self.drop_doc(Watched::Devices, id).await?;
        self.save_list(&list)
    }

    async fn devices_doc(&self) -> Result<Option<Doc>> {
        let Some(id) = read_list(&self.dir)?.devices else {
            return Ok(None);
        };
        let id: NamespaceId = id
            .parse()
            .map_err(|e| anyhow!("bad document id `{id}`: {e}"))?;
        self.doc(id).await
    }

    /// The handle to document `id`, if it's here.
    async fn doc(&self, id: NamespaceId) -> Result<Option<Doc>> {
        if let Some(doc) = self.open_docs.lock().unwrap().get(&id) {
            return Ok(Some(doc.clone()));
        }
        match self.docs.open(id).await? {
            Some(doc) => Ok(Some(self.keep(doc).await?)),
            None => Ok(None),
        }
    }

    /// Keeps a document's newly opened handle as the one to use, unless
    /// there's one already, in which case it's closed.
    async fn keep(&self, doc: Doc) -> Result<Doc> {
        let kept = match self.open_docs.lock().unwrap().entry(doc.id()) {
            Entry::Occupied(e) => Some(e.get().clone()),
            Entry::Vacant(e) => {
                e.insert(doc.clone());
                None
            }
        };
        match kept {
            Some(kept) => {
                doc.close().await?;
                Ok(kept)
            }
            None => Ok(doc),
        }
    }

    /// Stops syncing document `id`, and removes it from this machine.
    async fn drop_doc(&self, watched: Watched, id: NamespaceId) -> Result<()> {
        if let Some(watcher) = self.watching.lock().unwrap().remove(&watched) {
            watcher.abort();
        }
        self.forget_doc(id).await
    }

    /// Removes document `id` from this machine, leaving whatever watches it.
    async fn forget_doc(&self, id: NamespaceId) -> Result<()> {
        if let Some(doc) = self.doc(id).await? {
            self.open_docs.lock().unwrap().remove(&id);
            doc.leave().await?;
            doc.close().await?;
        }
        self.docs.drop_doc(id).await?;
        Ok(())
    }

    /// Starts syncing the profiles the user's other computers have put in
    /// the devices document, which arrive as [`Node::run`] hears from their
    /// documents, and puts in this computer's.
    async fn sync_devices(&self, pw: &Patchwork, doc: &Doc) -> Result<()> {
        let entries = self.read_entries(doc).await?;
        let synced = self.synced()?;
        for (key, value) in &entries {
            let Some(uuid) = key.strip_prefix(DEVICE_PROFILE) else {
                continue;
            };
            let Ok(uuid) = uuid.parse::<Uuid>() else {
                continue;
            };
            if value.as_slice() == REMOVED || synced.iter().any(|s| s.profile == uuid) {
                continue;
            }
            let ticket: String = serde_json::from_slice(value)
                .with_context(|| format!("the ticket for profile {uuid} can't be read"))?;
            let ticket: DocTicket = ticket.parse().context("that isn't a sync ticket")?;
            let writable = matches!(ticket.capability.kind(), CapabilityKind::Write);
            let profile_doc = self.keep(self.docs.import(ticket).await?).await?;
            self.remember(SyncedProfile::new(uuid, &profile_doc, writable, false))?;
            self.watch(Watched::Profile(uuid), &profile_doc).await?;
            self.start(&profile_doc).await?;
        }
        for (profile, _) in pw.profiles()? {
            let key = format!("{DEVICE_PROFILE}{}", profile.uuid);
            // removed ones stay removed
            if entries.contains_key(&key) {
                continue;
            }
            let writable = self.synced_profile(&profile)?.is_none_or(|s| s.writable);
            let ticket = self
                .share(pw, &profile, writable, Addresses::Included)
                .await?;
            self.write(doc, vec![(key, serde_json::to_vec(&ticket)?)])
                .await?;
        }
        Ok(())
    }

    /// Whether `synced` was here and has since been removed, so should stop
    /// being synced.
    fn removed(&self, pw: &Patchwork, synced: &SyncedProfile) -> bool {
        synced.arrived && pw.load_profile(&synced.reference()).is_err()
    }

    /// Syncs every synced profile once with whichever other machines can be
    /// reached within `timeout`, then stops.
    pub async fn sync_once(&self, pw: &Patchwork, timeout: Duration) -> Result<Vec<Outcome>> {
        let mut outcomes = self.take_moves(pw).await;
        if let Some(doc) = self.devices_doc().await? {
            let events = doc.subscribe().await?;
            // in case this machine's id has changed since it was linked
            self.announce(&doc).await?;
            self.start(&doc).await?;
            wait_for_sync(events, timeout).await;
            self.sync_devices(pw, &doc).await?;
            doc.leave().await?;
        }
        for synced in self.synced()? {
            if self.removed(pw, &synced) {
                self.leave(pw, &synced.reference()).await?;
                continue;
            }
            if synced.moved {
                outcomes.push(Outcome {
                    moved: true,
                    ..Outcome::nothing(synced.reference())
                });
                continue;
            }
            let doc = self.open(&synced).await?;
            let events = doc.subscribe().await?;
            // what's changed here goes out first
            let mut warnings = self.publish_local(pw, &doc, &synced).await?;
            self.start(&doc).await?;
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
        // others were told while this was syncing
        outcomes.append(&mut self.take_moves(pw).await);
        for synced in self.synced()? {
            if !synced.to_tell.is_empty() {
                self.tell_now(&synced).await?;
            }
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
        let mut moves_rx = self
            .moves_rx
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow!("the node is already running"))?;
        let mut changes = pw.watch_profiles();
        let mut hand_edits = tokio::time::interval(HAND_EDITS_EVERY);
        let mut tell_moves = tokio::time::interval(moves::TELL_EVERY);
        let mut telling = FuturesUnordered::new();
        if let Some(doc) = self.devices_doc().await? {
            self.announce(&doc).await?;
            self.watch(Watched::Devices, &doc).await?;
            self.start(&doc).await?;
            if let Err(e) = self.sync_devices(pw, &doc).await {
                on_sync(Err(e));
            }
        }
        for synced in self.synced()? {
            if self.removed(pw, &synced) {
                if let Err(e) = self.leave(pw, &synced.reference()).await {
                    on_sync(Err(e));
                }
                continue;
            }
            if synced.moved {
                continue;
            }
            let doc = self.open(&synced).await?;
            self.watch(Watched::Profile(synced.profile), &doc).await?;
            self.start(&doc).await?;
            on_sync(self.sync_profile(pw, &synced).await);
        }
        loop {
            let watched = tokio::select! {
                Some(watched) = doc_events.recv() => watched,
                Some(incoming) = moves_rx.recv() => {
                    if let Some(outcome) = self.take_move(pw, incoming).await {
                        on_sync(outcome);
                    }
                    continue;
                }
                _ = tell_moves.tick() => {
                    match self.synced() {
                        Ok(synced) => {
                            for synced in synced.iter().filter(|s| !s.to_tell.is_empty()) {
                                match self.notices(synced).await {
                                    Ok(notices) => telling.extend(notices),
                                    Err(e) => on_sync(Err(e)),
                                }
                            }
                        }
                        Err(e) => on_sync(Err(e)),
                    }
                    continue;
                }
                Some(told) = telling.next(), if !telling.is_empty() => {
                    if let Err(e) = self.told(told) {
                        on_sync(Err(e));
                    }
                    continue;
                }
                change = changes.recv() => match change {
                    Ok(change) if !change.synced => Watched::Profile(change.profile.uuid),
                    Ok(_) => continue,
                    // too many at once; the next check catches up
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                },
                _ = hand_edits.tick() => {
                    for synced in self.synced()? {
                        let changed = synced.writable
                            && !synced.moved
                            && !self.removed(pw, &synced)
                            && !pw.unsynced_changes(&synced.reference()).unwrap_or_default().is_empty();
                        if changed {
                            on_sync(self.sync_profile(pw, &synced).await);
                        }
                    }
                    continue;
                }
            };
            // a burst of events is usually one change arriving, so each
            // document in it is synced once
            let mut due = BTreeSet::from([watched]);
            while let Ok(more) = doc_events.try_recv() {
                due.insert(more);
            }
            for watched in due {
                if let Err(e) = self.sync_watched(pw, watched, &on_sync).await {
                    on_sync(Err(e));
                }
            }
        }
    }

    async fn sync_watched(
        &self,
        pw: &Patchwork,
        watched: Watched,
        on_sync: &impl Fn(Result<Outcome>),
    ) -> Result<()> {
        let uuid = match watched {
            Watched::Devices => {
                if let Some(doc) = self.devices_doc().await? {
                    self.sync_devices(pw, &doc).await?;
                }
                return Ok(());
            }
            Watched::Profile(uuid) => uuid,
        };
        let profile = ProfileRef { uuid };
        match self.synced_profile(&profile)? {
            Some(synced) if self.removed(pw, &synced) => self.leave(pw, &profile).await?,
            Some(synced) if synced.moved => {}
            Some(synced) => on_sync(self.sync_profile(pw, &synced).await),
            // a new profile, which linked computers get
            None => {
                if let Some(doc) = self.devices_doc().await?
                    && pw.load_profile(&profile).is_ok()
                {
                    self.sync_devices(pw, &doc).await?;
                }
            }
        }
        Ok(())
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
        if !synced.writable || synced.moved || pw.load_profile(&profile).is_err() {
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
        if synced.moved || self.has_moved(doc).await? {
            if !synced.moved {
                self.remember(SyncedProfile {
                    moved: true,
                    ..synced.clone()
                })?;
            }
            return Ok(Outcome {
                moved: true,
                ..Outcome::nothing(synced.reference())
            });
        }
        let entries = self.read_entries(doc).await?;
        if entries.is_empty() {
            return Ok(Outcome::nothing(synced.reference()));
        }
        let applied = pw.apply_synced(&entries, synced.writable)?;
        if !synced.arrived {
            self.remember(SyncedProfile {
                arrived: true,
                ..synced.clone()
            })?;
        }
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
        if synced.writable {
            self.announce(doc).await?;
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
            moved: false,
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
            if key.starts_with(FILE) || key.starts_with(PEER) || key == MOVED {
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
        self.doc(synced.namespace()?)
            .await?
            .ok_or_else(|| anyhow!("the document for profile {} is gone", synced.profile))
    }

    /// Starts syncing `doc` with whoever's online of those known to sync
    /// it: those it was synced with before, and those in its `peer/` keys.
    async fn start(&self, doc: &Doc) -> Result<()> {
        let me = self.id();
        let peers = self
            .peers_in(doc)
            .await?
            .into_iter()
            .filter(|p| p.id != me && !p.gone)
            .map(|p| p.addr())
            .collect();
        doc.start_sync(peers).await?;
        Ok(())
    }

    /// The machines in `doc`'s `peer/` keys, as last written.
    async fn peers_in(&self, doc: &Doc) -> Result<Vec<Peer>> {
        let mut peers = Vec::new();
        let query = Query::single_latest_per_key().key_prefix(PEER).build();
        let stream = doc.get_many(query).await?;
        let mut stream = std::pin::pin!(stream);
        while let Some(entry) = stream.next().await {
            let entry = entry?;
            let Some(id) = entry_key(entry.key())
                .and_then(|k| k.strip_prefix(PEER))
                .and_then(|id| id.parse::<PublicKey>().ok())
            else {
                continue;
            };
            // otherwise it's looked up
            let (addr, gone) = match self.blobs.get_bytes(entry.content_hash()).await {
                Ok(value) if *value == *REMOVED => (None, true),
                Ok(value) => (
                    serde_json::from_slice::<EndpointAddr>(&value)
                        .ok()
                        .filter(|a| a.id == id),
                    false,
                ),
                Err(_) => (None, false),
            };
            peers.push(Peer { id, addr, gone });
        }
        Ok(peers)
    }

    /// Whether `doc` has been left for another (see [`moves`]).
    async fn has_moved(&self, doc: &Doc) -> Result<bool> {
        Ok(doc
            .get_one(Query::key_exact(doc_key(MOVED)))
            .await?
            .is_some())
    }

    /// Puts this computer in `doc`'s `peer/` keys, with where it can be
    /// reached, so that others syncing it can find it. It's only written
    /// again when that changes.
    async fn announce(&self, doc: &Doc) -> Result<()> {
        let addr = self.router.endpoint().addr();
        let key = format!("{PEER}{}", addr.id);
        let value = serde_json::to_vec(&addr)?;
        if let Some(entry) = doc.get_one(Query::key_exact(doc_key(&key))).await?
            && self
                .blobs
                .get_bytes(entry.content_hash())
                .await
                .is_ok_and(|v| *v == value)
        {
            return Ok(());
        }
        self.write(doc, vec![(key, value)]).await
    }

    /// Passes on `doc`'s events for [`Node::run`], once.
    async fn watch(&self, watched: Watched, doc: &Doc) -> Result<()> {
        if self.watching.lock().unwrap().contains_key(&watched) {
            return Ok(());
        }
        let mut events = doc.subscribe().await?;
        let tx = self.doc_events.clone();
        let watcher = tokio::spawn(async move {
            while let Some(event) = events.next().await {
                let worth_a_look = matches!(
                    event,
                    Ok(LiveEvent::InsertRemote { .. }
                        | LiveEvent::ContentReady { .. }
                        | LiveEvent::PendingContentReady)
                );
                if worth_a_look && tx.send(watched).is_err() {
                    return;
                }
            }
        });
        let previous = self
            .watching
            .lock()
            .unwrap()
            .insert(watched, watcher.abort_handle());
        // watched twice at once
        if let Some(previous) = previous {
            previous.abort();
        }
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
    Ok(read_list(dir)?.profiles)
}

/// Whether the computer whose node keeps `dir` is linked to the user's
/// others (see [`Node::link_devices`]), without starting a node.
pub fn devices_linked(dir: &Path) -> Result<bool> {
    Ok(read_list(dir)?.devices.is_some())
}

fn read_list(dir: &Path) -> Result<SyncedProfiles> {
    let path = profiles_file(dir);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SyncedProfiles::default()),
        Err(e) => return Err(e).context(path.display().to_string()),
    };
    toml::from_str(&text).with_context(|| path.display().to_string())
}

/// A ticket to `doc`, as text.
async fn ticket(doc: &Doc, mode: ShareMode, addresses: Addresses) -> Result<String> {
    let ticket = doc.share(mode, addresses.options()).await?;
    Ok(iroh_tickets::Ticket::encode_string(&ticket))
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

/// Replaces the key at `path` with a new one, returning the id it gives.
/// It's written beside the old one first, so a failure leaves the old one.
fn new_secret_key(path: &Path) -> Result<PublicKey> {
    let key = SecretKey::generate();
    let staged = path.with_extension("key.new");
    match fs::remove_file(&staged) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            return Err(e).context(staged.display().to_string());
        }
        _ => {}
    }
    write_private(&staged, &key.to_bytes())?;
    fs::rename(&staged, path).with_context(|| path.display().to_string())?;
    Ok(key.public())
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
