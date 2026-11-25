//! Shared sets (see [`patchwork_core::set`]), kept in step between the
//! person's own computers through the devices document (see
//! [`Node::link_devices`]), so a folder linked in several profiles is
//! linked on all of them. Friends never get the sets: a synced profile's
//! document has each set it includes as an ordinary folder.
//!
//! Each set is one entry, `set/<set id>`, holding the [`Set`] as JSON (or
//! [`REMOVED`] once it's gone); the newest wins. What each was when last
//! in step is kept in `<data>/sync/sets/<set id>`, so that a change made
//! here can be told from one that arrived, as for personal layers (see
//! [`personal`](crate::personal)).
//!
//! Which profiles include a set isn't in its entry: each computer links a
//! profile's folder to the set with its id when the folder's as the set
//! has it (see [`Patchwork::apply_set`]), so it works out the same
//! whichever of the set and the profile arrives first.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use iroh_docs::api::Doc;
use patchwork_core::set::Set;
use patchwork_core::sync::{Entries, REMOVED};
use patchwork_core::{Error, Patchwork};
use uuid::Uuid;

use crate::{Node, Outcome};

/// Starts the devices document's key for each shared set.
pub(crate) const SET: &str = "set/";

fn base_dir(dir: &Path) -> PathBuf {
    dir.join("sets")
}

fn base_file(dir: &Path, set: Uuid) -> PathBuf {
    base_dir(dir).join(set.to_string())
}

/// The set as of the last time it was in step: `None` if it never has
/// been, else its entry's value ([`REMOVED`] for gone).
fn base(dir: &Path, set: Uuid) -> Result<Option<Vec<u8>>> {
    let path = base_file(dir, set);
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).context(path.display().to_string()),
    }
}

fn save_base(dir: &Path, set: Uuid, value: &[u8]) -> Result<()> {
    let path = base_file(dir, set);
    fs::create_dir_all(base_dir(dir)).with_context(|| base_dir(dir).display().to_string())?;
    fs::write(&path, value).with_context(|| path.display().to_string())
}

/// Forgets what every set was when last in step, for when this computer
/// stops being linked to others.
pub(crate) fn forget(dir: &Path) -> Result<()> {
    match fs::remove_dir_all(base_dir(dir)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).context(base_dir(dir).display().to_string())
        }
        _ => Ok(()),
    }
}

/// Every set here, as its entry's value, by id.
fn local(pw: &Patchwork) -> Result<Vec<(Uuid, Vec<u8>)>> {
    Ok(pw
        .all_sets()?
        .into_iter()
        .map(|s| (s.id, s.to_bytes()))
        .collect())
}

fn set(value: &[u8]) -> Result<Option<Set>> {
    if value == REMOVED {
        return Ok(None);
    }
    Ok(Some(Set::from_bytes(value).context(
        "a shared folder from another computer can't be read",
    )?))
}

/// What a set should be, given what it is here (`local`), what it was when
/// last in step (`base`) and what the document has (`doc`): the value to
/// keep, and whether the document needs it.
fn settle(local: &[u8], base: Option<&[u8]>, doc: Option<&[u8]>) -> (Vec<u8>, bool) {
    match (base, doc) {
        // changed here since: that's the newest, unless it's the same
        (Some(base), doc) if base != local => (local.to_vec(), doc != Some(local)),
        (_, Some(doc)) => (doc.to_vec(), false),
        // nothing there yet
        (_, None) => (local.to_vec(), local != REMOVED),
    }
}

impl Node {
    /// Puts sets changed here since they were last in step in the devices
    /// document `doc`, and takes whatever's newer in it. Returns what
    /// didn't work but didn't stop the rest, for each profile that includes
    /// the set it was about.
    pub(crate) async fn sync_sets(&self, pw: &Patchwork, doc: &Doc) -> Result<Vec<Outcome>> {
        let entries = self.read_entries(doc).await?;
        let here = local(pw)?;
        let ids: BTreeSet<Uuid> = here
            .iter()
            .map(|(id, _)| *id)
            .chain(in_doc(&entries))
            .collect();
        let mut warnings = Vec::new();
        let mut publish = Vec::new();
        for id in ids {
            let key = format!("{SET}{id}");
            let local = here
                .iter()
                .find(|(have, _)| *have == id)
                .map_or(REMOVED, |(_, value)| value.as_slice());
            let base = base(&self.dir, id)?;
            let (value, out) = settle(local, base.as_deref(), entries.get(&key).map(Vec::as_slice));
            if out {
                publish.push((key, value.clone()));
            }
            match pw.apply_set(id, set(&value)?.as_ref()) {
                Ok(_) => save_base(&self.dir, id, &value)?,
                // left as it is until this computer's updated
                Err(e @ Error::Schema { .. }) => {
                    for profile in pw.profiles_including(id)? {
                        let mut outcome = Outcome::nothing(profile);
                        outcome.warnings.push(e.to_string());
                        warnings.push(outcome);
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        self.write(doc, publish).await?;
        Ok(warnings)
    }

    /// Whether any set has changed here since it was last in step.
    pub(crate) fn sets_changed(&self, pw: &Patchwork) -> bool {
        // one that can't be read is put right once it can
        let Ok(here) = local(pw) else {
            return false;
        };
        let changed = here.iter().any(|(id, value)| {
            base(&self.dir, *id).is_ok_and(|base| base.as_deref() != Some(value.as_slice()))
        });
        let gone = fs::read_dir(base_dir(&self.dir)).is_ok_and(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                let Ok(id) = e.file_name().to_string_lossy().parse::<Uuid>() else {
                    return false;
                };
                !here.iter().any(|(have, _)| *have == id)
                    && base(&self.dir, id).is_ok_and(|b| b.as_deref() != Some(REMOVED))
            })
        });
        changed || gone
    }
}

/// The sets `entries` (the devices document's) have, by id.
fn in_doc(entries: &Entries) -> impl Iterator<Item = Uuid> + '_ {
    entries
        .keys()
        .filter_map(|k| k.strip_prefix(SET)?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_here_goes_out() {
        assert_eq!(settle(b"b", Some(b"a"), Some(b"c")), (b"b".to_vec(), true));
        assert_eq!(
            settle(REMOVED, Some(b"a"), Some(b"a")),
            (REMOVED.to_vec(), true)
        );
        // unless it's there already
        assert_eq!(settle(b"b", Some(b"a"), Some(b"b")), (b"b".to_vec(), false));
    }

    #[test]
    fn otherwise_the_document_wins() {
        assert_eq!(
            settle(b"a", Some(b"a"), Some(REMOVED)),
            (REMOVED.to_vec(), false)
        );
        assert_eq!(settle(b"a", None, Some(b"b")), (b"b".to_vec(), false));
        assert_eq!(settle(b"a", Some(b"a"), None), (b"a".to_vec(), true));
        assert_eq!(settle(REMOVED, None, None), (REMOVED.to_vec(), false));
    }
}
