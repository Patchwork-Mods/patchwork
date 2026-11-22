//! The mods the user follows (see [`patchwork_core::follow`]), kept in step
//! between their own computers through the devices document (see
//! [`Node::link_devices`]), as `follow/<source:id>` set to [`FOLLOWED`] or
//! [`UNFOLLOWED`]. The newest of each wins, so following a mod on one
//! computer follows it on all of them, and so does stopping. Friends a
//! profile's shared with never see the devices document, so they don't get
//! these.
//!
//! What the follows were when last in step is kept in
//! `<data>/sync/follows.toml`, so that a change made here can be told from
//! one that arrived: a mod followed since then is put in the document as
//! followed, one dropped since as unfollowed, and everything else goes by
//! the document.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use iroh_docs::api::Doc;
use patchwork_core::Patchwork;
use patchwork_core::follow::Follows;

use crate::Node;

/// Starts the devices document's key for each followed mod.
pub(crate) const FOLLOW: &str = "follow/";
const FOLLOWED: &[u8] = b"1";
const UNFOLLOWED: &[u8] = b"0";

fn base_file(dir: &Path) -> PathBuf {
    dir.join("follows.toml")
}

/// The follows as of the last time they were in step, or nothing if they
/// never have been.
fn base(dir: &Path) -> Result<BTreeSet<String>> {
    let path = base_file(dir);
    match fs::read_to_string(&path) {
        Ok(text) => {
            let follows: Follows =
                toml::from_str(&text).with_context(|| path.display().to_string())?;
            Ok(follows.mods)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(e).context(path.display().to_string()),
    }
}

fn save_base(dir: &Path, mods: &BTreeSet<String>) -> Result<()> {
    let path = base_file(dir);
    let follows = Follows { mods: mods.clone() };
    fs::write(&path, toml::to_string(&follows)?).with_context(|| path.display().to_string())
}

/// Forgets what the follows were when last in step, for when this computer
/// stops being linked to others.
pub(crate) fn forget(dir: &Path) -> Result<()> {
    match fs::remove_file(base_file(dir)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).context(base_file(dir).display().to_string())
        }
        _ => Ok(()),
    }
}

/// What `local` becomes given the devices document's `entries`.
fn merged<'a>(
    local: &BTreeSet<String>,
    entries: impl IntoIterator<Item = (&'a String, &'a Vec<u8>)>,
) -> BTreeSet<String> {
    let mut mods = local.clone();
    for (key, value) in entries {
        let Some(key) = key.strip_prefix(FOLLOW) else {
            continue;
        };
        match value.as_slice() {
            FOLLOWED => {
                mods.insert(key.to_owned());
            }
            UNFOLLOWED => {
                mods.remove(key);
            }
            _ => {}
        }
    }
    mods
}

impl Node {
    /// Puts follows changed here since they were last in step in the
    /// devices document `doc`, then takes whatever's in it.
    pub(crate) async fn sync_follows(&self, pw: &Patchwork, doc: &Doc) -> Result<()> {
        let local = Follows::load(pw.paths())?.mods;
        let base = base(&self.dir)?;
        let changes = local
            .difference(&base)
            .map(|k| (format!("{FOLLOW}{k}"), FOLLOWED.to_vec()))
            .chain(
                base.difference(&local)
                    .map(|k| (format!("{FOLLOW}{k}"), UNFOLLOWED.to_vec())),
            )
            .collect();
        self.write(doc, changes).await?;
        let entries = self.read_entries(doc).await?;
        let mods = merged(&local, &entries);
        if mods != local {
            Follows { mods: mods.clone() }.save(pw.paths())?;
        }
        save_base(&self.dir, &mods)
    }

    /// Whether follows have changed here since they were last in step.
    pub(crate) fn follows_changed(&self, pw: &Patchwork) -> bool {
        // one that can't be read is put right once it can
        match (Follows::load(pw.paths()), base(&self.dir)) {
            (Ok(local), Ok(base)) => local.mods != base,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn the_document_wins_for_whatever_it_has() {
        let local = BTreeSet::from(["ts:a".to_owned(), "ts:b".to_owned()]);
        let entries = BTreeMap::from([
            ("follow/ts:b".to_owned(), UNFOLLOWED.to_vec()),
            ("follow/ts:c".to_owned(), FOLLOWED.to_vec()),
            ("profile/x".to_owned(), FOLLOWED.to_vec()),
        ]);
        assert_eq!(
            merged(&local, &entries),
            BTreeSet::from(["ts:a".to_owned(), "ts:c".to_owned()])
        );
    }
}
