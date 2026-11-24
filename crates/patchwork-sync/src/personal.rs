//! Synced profiles' personal layers (see [`patchwork_core::personal`]),
//! kept in step between the person's own computers through the devices
//! document (see [`Node::link_devices`]), so their personal mods follow
//! them but never go to anyone the profile's shared with.
//!
//! Each profile's layer, with what only it needs locked, is one entry,
//! `personal/<profile id>`, holding a [`Layer`] (or [`REMOVED`] once it has
//! no mods); the newest wins. Its local mods' files go in the devices
//! document too, the way a profile's go in its own (see [`files`]).
//!
//! What each layer was when last in step is kept in
//! `<data>/sync/personal/<profile id>`, so that a change made here can be
//! told from one that arrived, as with [`follows`](crate::follows). The
//! first time a computer's in step for a profile, it may have personal
//! mods of its own already, from before it was linked: those are added
//! to what's in the document ([`Layer::merge`]) rather than lost.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use iroh_docs::api::Doc;
use patchwork_core::personal::Layer;
use patchwork_core::profile::ProfileRef;
use patchwork_core::sync::{Entries, REMOVED};
use patchwork_core::{Error, Patchwork, SyncApplied};
use uuid::Uuid;

use crate::{Node, Outcome, files};

/// Starts the devices document's key for each profile's personal layer.
pub(crate) const PERSONAL: &str = "personal/";

fn base_dir(dir: &Path) -> PathBuf {
    dir.join("personal")
}

fn base_file(dir: &Path, profile: Uuid) -> PathBuf {
    base_dir(dir).join(profile.to_string())
}

/// The profile's layer as of the last time it was in step: `None` if it
/// never has been, else its entry's value ([`REMOVED`] for none).
fn base(dir: &Path, profile: Uuid) -> Result<Option<Vec<u8>>> {
    let path = base_file(dir, profile);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context(path.display().to_string()),
    }
}

fn save_base(dir: &Path, profile: Uuid, value: &[u8]) -> Result<()> {
    let path = base_file(dir, profile);
    fs::create_dir_all(base_dir(dir)).with_context(|| base_dir(dir).display().to_string())?;
    fs::write(&path, value).with_context(|| path.display().to_string())
}

/// Forgets what every layer was when last in step, for when this computer
/// stops being linked to others.
pub(crate) fn forget(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(base_dir(dir)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).context(base_dir(dir).display().to_string())
        }
        _ => Ok(()),
    }
}

/// The profile's layer here, as its entry's value.
fn local(pw: &Patchwork, profile: &ProfileRef) -> Result<Vec<u8>> {
    Ok(pw
        .personal_layer(profile)?
        .map_or_else(|| REMOVED.to_vec(), |l| l.to_bytes()))
}

fn layer(value: &[u8]) -> Result<Option<Layer>> {
    if value == REMOVED {
        return Ok(None);
    }
    Ok(Some(Layer::from_bytes(value).context(
        "personal mods from another computer can't be read",
    )?))
}

/// What a profile's layer should be, given what it is here (`local`), what
/// it was when last in step (`base`) and what the document has (`doc`):
/// the value to keep, and whether the document needs it.
fn settle(local: &[u8], base: Option<&[u8]>, doc: Option<&[u8]>) -> Result<(Vec<u8>, bool)> {
    match (base, doc) {
        // changed here since: that's the newest
        (Some(base), _) if base != local => Ok((local.to_vec(), true)),
        (Some(_), Some(doc)) => Ok((doc.to_vec(), false)),
        // nothing new either way, but the document should have it
        (Some(_), None) => Ok((local.to_vec(), local != REMOVED)),
        (None, None) => Ok((local.to_vec(), local != REMOVED)),
        // never in step: keep what both have
        (None, Some(doc)) => match (layer(doc)?, layer(local)?) {
            (Some(theirs), Some(ours)) => {
                let merged = Layer::merge(theirs, ours).to_bytes();
                let publish = merged != doc;
                Ok((merged, publish))
            }
            (None, Some(_)) => Ok((local.to_vec(), true)),
            (_, None) => Ok((doc.to_vec(), false)),
        },
    }
}

impl Node {
    /// Puts personal layers changed here since they were last in step in
    /// the devices document `doc`, takes whatever's newer in it, and
    /// passes local mods' files both ways. Profiles that haven't arrived
    /// here yet are left until they have. Returns what it did for each
    /// profile it did anything for.
    pub(crate) async fn sync_personal(&self, pw: &Patchwork, doc: &Doc) -> Result<Vec<Outcome>> {
        let entries = self.read_entries(doc).await?;
        let mut outcomes = Vec::new();
        for (profile, _) in pw.profiles()? {
            if let Some(outcome) = self.personal_step(pw, doc, &entries, &profile).await? {
                outcomes.push(outcome);
            }
        }
        Ok(outcomes)
    }

    /// [`Node::sync_personal`] for one profile, such as one that's just
    /// arrived.
    pub(crate) async fn sync_personal_of(
        &self,
        pw: &Patchwork,
        doc: &Doc,
        profile: &ProfileRef,
    ) -> Result<Option<Outcome>> {
        let entries = self.read_entries(doc).await?;
        self.personal_step(pw, doc, &entries, profile).await
    }

    async fn personal_step(
        &self,
        pw: &Patchwork,
        doc: &Doc,
        entries: &Entries,
        profile: &ProfileRef,
    ) -> Result<Option<Outcome>> {
        let key = format!("{PERSONAL}{}", profile.uuid);
        let base = base(&self.dir, profile.uuid)?;
        let local = local(pw, profile)?;
        let there = entries.get(&key).map(Vec::as_slice);
        if base.is_none() && there.is_none() && local == REMOVED {
            return Ok(None);
        }
        let mut outcome = Outcome::nothing(profile.clone());
        let (value, publish) = settle(&local, base.as_deref(), there)?;
        if publish {
            self.write(doc, vec![(key, value.clone())]).await?;
        }
        let changed = match pw.apply_personal(profile, layer(&value)?.as_ref()) {
            Ok(changed) => changed,
            Err(e @ Error::Schema { .. }) => {
                // left as it is until this computer's updated
                outcome.warnings.push(e.to_string());
                return Ok(Some(outcome));
            }
            Err(e) => return Err(e.into()),
        };
        save_base(&self.dir, profile.uuid, &value)?;
        if changed {
            outcome.applied = Some(SyncApplied {
                profile: profile.clone(),
                created: false,
                changed: true,
                unpublished: Vec::new(),
            });
        }
        if let Some(layer) = layer(&value)? {
            outcome.sent_files = files::publish(self, pw, doc, &layer.locked).await?;
            outcome.received_files =
                files::receive(self, pw, doc, profile, &layer.locked, &mut outcome.warnings)
                    .await?;
        }
        if changed || outcome.received_files > 0 {
            match pw.download_locked(profile).await {
                Ok(()) => {}
                Err(Error::NeedsDownload(pending)) => {
                    for p in pending {
                        outcome.warnings.push(format!(
                            "`{}` has to be downloaded by hand: {}",
                            p.name, p.page
                        ));
                    }
                }
                Err(e) => outcome.warnings.push(e.to_string()),
            }
        }
        let did_something = outcome.applied.is_some()
            || outcome.received_files > 0
            || outcome.sent_files > 0
            || !outcome.warnings.is_empty();
        Ok(did_something.then_some(outcome))
    }

    /// Whether any synced profile's personal layer (only they have one)
    /// has changed here since it was last in step.
    pub(crate) fn personal_changed(&self, pw: &Patchwork) -> bool {
        // one that can't be read is put right once it can
        let Ok(synced) = self.synced() else {
            return false;
        };
        synced.iter().any(
            |s| match (local(pw, &s.reference()), base(&self.dir, s.profile)) {
                (Ok(local), Ok(Some(base))) => local != base,
                (Ok(local), Ok(None)) => local != REMOVED,
                _ => false,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use patchwork_core::personal::Personal;

    use super::*;

    fn value(mods: &[&str]) -> Vec<u8> {
        let mods = mods
            .iter()
            .map(|id| {
                toml::from_str(&format!("name = \"{id}\"\nsource = \"s\"\nid = \"{id}\"")).unwrap()
            })
            .collect();
        Layer {
            personal: Personal {
                mods,
                ..Personal::new("g")
            },
            locked: Vec::new(),
        }
        .to_bytes()
    }

    #[test]
    fn a_change_here_goes_out() {
        let (kept, publish) =
            settle(&value(&["b"]), Some(&value(&["a"])), Some(&value(&["c"]))).unwrap();
        assert_eq!(kept, value(&["b"]));
        assert!(publish);
        let (kept, publish) = settle(REMOVED, Some(&value(&["a"])), Some(&value(&["a"]))).unwrap();
        assert_eq!(kept, REMOVED);
        assert!(publish);
    }

    #[test]
    fn otherwise_the_document_wins() {
        let (kept, publish) = settle(&value(&["a"]), Some(&value(&["a"])), Some(REMOVED)).unwrap();
        assert_eq!(kept, REMOVED);
        assert!(!publish);
        let (kept, publish) = settle(&value(&["a"]), Some(&value(&["a"])), None).unwrap();
        assert_eq!(kept, value(&["a"]));
        assert!(publish);
    }

    #[test]
    fn the_first_time_both_are_kept() {
        let (kept, publish) = settle(&value(&["b"]), None, Some(&value(&["a"]))).unwrap();
        assert_eq!(kept, value(&["a", "b"]));
        assert!(publish);
        let (kept, publish) = settle(REMOVED, None, Some(&value(&["a"]))).unwrap();
        assert_eq!(kept, value(&["a"]));
        assert!(!publish);
        let (kept, publish) = settle(&value(&["b"]), None, Some(REMOVED)).unwrap();
        assert_eq!(kept, value(&["b"]));
        assert!(publish);
    }
}
