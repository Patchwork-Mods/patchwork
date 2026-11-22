//! The mods the user follows, so the daemon tells them when a new version
//! is out, kept in `<config>/follows.toml`:
//!
//! ```toml
//! mods = ["thunderstore:RiskofThunder-R2API", "nexus:skyrimspecialedition/266"]
//! ```
//!
//! A mod is followed by `source:id` (see [`crate::lock::mod_key`]), so it's
//! followed in every profile that has it. Computers linked to each other
//! keep the same list (patchwork-sync's `follows`), so each of them
//! announces a new version.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lock::mod_key;
use crate::paths::Paths;
use crate::{Result, fsutil};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Follows {
    /// `source:id` of each followed mod.
    #[serde(default)]
    pub mods: BTreeSet<String>,
}

impl Follows {
    /// What's in `follows.toml`, or nothing if there isn't one.
    pub fn load(paths: &Paths) -> Result<Self> {
        Ok(fsutil::read_toml(&paths.follows())?.unwrap_or_default())
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        fsutil::write_toml(&paths.follows(), self)
    }

    pub fn follows(&self, source: &str, id: &str) -> bool {
        self.mods.contains(&mod_key(source, id))
    }
}

/// Follows the mod `source:id`, or with `follow` false stops following it.
/// Returns whether that changed anything.
pub fn set_followed(paths: &Paths, source: &str, id: &str, follow: bool) -> Result<bool> {
    let mut follows = Follows::load(paths)?;
    let key = mod_key(source, id);
    let changed = if follow {
        follows.mods.insert(key)
    } else {
        follows.mods.remove(&key)
    };
    if changed {
        follows.save(paths)?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(tmp.path());
        assert!(Follows::load(&paths).unwrap().mods.is_empty());
        assert!(set_followed(&paths, "thunderstore", "a-B", true).unwrap());
        assert!(!set_followed(&paths, "thunderstore", "a-B", true).unwrap());
        assert!(set_followed(&paths, "nexus", "skyrim/1", true).unwrap());
        let follows = Follows::load(&paths).unwrap();
        assert!(follows.follows("thunderstore", "a-B"));
        assert!(follows.follows("nexus", "skyrim/1"));
        assert!(!follows.follows("nexus", "a-B"));
        assert!(set_followed(&paths, "thunderstore", "a-B", false).unwrap());
        assert!(
            !Follows::load(&paths)
                .unwrap()
                .follows("thunderstore", "a-B")
        );
    }
}
