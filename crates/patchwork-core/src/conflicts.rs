//! Files that more than one of a profile's mods provide. Only one copy of
//! each can be deployed, the one from whichever mod comes last (see
//! [`deploy::plan`](crate::deploy::plan)), which is what makes load order
//! matter. This works out which mod's copy wins each file, so a frontend can
//! show what moving a mod would change.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use crate::deploy::{Dest, InstalledMod, Placement, RootId, Src};
use crate::hash::ContentHash;
use crate::relpath::RelPath;

/// A file that more than one mod provides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileConflict {
    /// Spelled the way the first mod to provide it spells it.
    pub dst: Dest,
    /// The mods that provide it, in load order: the last one's copy is the
    /// one deployed.
    pub mods: Vec<String>,
    /// Whether every mod's copy is the same, so it doesn't matter which wins.
    pub identical: bool,
}

impl FileConflict {
    /// The mod whose copy is deployed.
    pub fn winner(&self) -> &str {
        self.mods.last().expect("a conflict has at least two mods")
    }
}

/// Every file that more than one of a profile's mods provide.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Conflicts {
    /// Every mod that was looked at, in load order.
    pub mods: Vec<String>,
    /// Sorted by where they go.
    pub files: Vec<FileConflict>,
}

/// Some of a mod's files that clash with another mod's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModFiles {
    #[serde(rename = "mod")]
    pub mod_name: String,
    pub files: Vec<Dest>,
}

/// How one mod's files clash with the others'. Files that are the same in
/// every mod that has them are only counted, since nothing's lost whichever
/// copy wins.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModConflicts {
    /// Mods earlier in the load order whose copies of files this mod's
    /// replace.
    pub overrides: Vec<ModFiles>,
    /// Mods later in the load order whose copies replace this mod's.
    pub overridden_by: Vec<ModFiles>,
    /// Files it shares with other mods that are the same in all of them.
    pub identical: usize,
}

impl ModConflicts {
    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty() && self.overridden_by.is_empty() && self.identical == 0
    }

    /// Whether any of its files differ from another mod's copy, which is
    /// when load order matters.
    pub fn matters(&self) -> bool {
        !self.overrides.is_empty() || !self.overridden_by.is_empty()
    }

    /// Whether none of its files that differ from another mod's are
    /// deployed: it's overridden in everything it shares and overrides
    /// nothing.
    pub fn lost(&self) -> bool {
        self.overrides.is_empty() && !self.overridden_by.is_empty()
    }
}

/// Finds the files that more than one of `mods` (in load order) place, from a deployer's
/// `placements` for them. Placements are taken in order, later ones winning,
/// as deploying does. Paths that differ only in case are the same file,
/// since deploying makes them so. Built files are left out: they're made
/// from the mods rather than being any one mod's.
pub fn find(mods: &[InstalledMod], placements: &[Placement]) -> Conflicts {
    let sources: HashMap<(ContentHash, &RelPath), &Path> = mods
        .iter()
        .flat_map(|m| m.files.iter().map(|f| ((m.hash, &f.rel), f.src.as_path())))
        .collect();
    // Each file's copies in order, by its root and lowercased path.
    let mut index: HashMap<(&RootId, String), usize> = HashMap::new();
    let mut groups: Vec<(&Dest, Vec<FileCopy<'_>>)> = Vec::new();
    for p in placements {
        let Src::Mod { hash, rel } = &p.src else {
            continue;
        };
        let key = (&p.dst.root, p.dst.path.as_str().to_lowercase());
        let i = *index.entry(key).or_insert_with(|| {
            groups.push((&p.dst, Vec::new()));
            groups.len() - 1
        });
        let copies = &mut groups[i].1;
        // A mod that places a file twice only deploys its last copy.
        copies.retain(|(name, _)| *name != p.mod_name);
        copies.push((&p.mod_name, sources.get(&(*hash, rel)).copied()));
    }
    let mut files: Vec<FileConflict> = groups
        .into_iter()
        .filter(|(_, copies)| copies.len() > 1)
        .map(|(dst, copies)| FileConflict {
            dst: dst.clone(),
            identical: same_contents(copies.iter().map(|(_, src)| *src)),
            mods: copies
                .into_iter()
                .map(|(name, _)| name.to_owned())
                .collect(),
        })
        .collect();
    files.sort_by(|a, b| a.dst.cmp(&b.dst));
    Conflicts {
        mods: mods.iter().map(|m| m.name.clone()).collect(),
        files,
    }
}

/// One mod's copy of a file: the mod, and where the copy is if it's known.
type FileCopy<'a> = (&'a str, Option<&'a Path>);

/// Whether every one of `paths` is there and has the same contents.
fn same_contents<'a>(paths: impl Iterator<Item = Option<&'a Path>>) -> bool {
    let Some(paths) = paths.collect::<Option<Vec<_>>>() else {
        return false;
    };
    // Sizes first, so most files that differ aren't read at all.
    let sizes: Option<Vec<u64>> = paths
        .iter()
        .map(|p| p.metadata().ok().map(|m| m.len()))
        .collect();
    let Some(sizes) = sizes else {
        return false;
    };
    if sizes.iter().any(|s| *s != sizes[0]) {
        return false;
    }
    let mut hashes = paths.iter().map(|p| ContentHash::of_file(p).ok());
    let first = hashes.next().flatten();
    first.is_some() && hashes.all(|h| h == first)
}

impl Conflicts {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// How `name`'s files clash with the other mods', with the mods in the
    /// order they're first met; [`Conflicts::by_mod`] puts them in load
    /// order.
    pub fn of(&self, name: &str) -> ModConflicts {
        let mut overrides: Vec<ModFiles> = Vec::new();
        let mut overridden_by: Vec<ModFiles> = Vec::new();
        let mut identical = 0;
        for file in &self.files {
            let Some(at) = file.mods.iter().position(|m| m == name) else {
                continue;
            };
            if file.identical {
                identical += 1;
                continue;
            }
            for (i, other) in file.mods.iter().enumerate() {
                if i == at {
                    continue;
                }
                let list = if i < at {
                    &mut overrides
                } else {
                    &mut overridden_by
                };
                match list.iter_mut().find(|m| m.mod_name == *other) {
                    Some(m) => m.files.push(file.dst.clone()),
                    None => list.push(ModFiles {
                        mod_name: other.clone(),
                        files: vec![file.dst.clone()],
                    }),
                }
            }
        }
        ModConflicts {
            overrides,
            overridden_by,
            identical,
        }
    }

    /// [`Conflicts::of`] every mod that has any, in load order, with the
    /// mods each lists in load order too.
    pub fn by_mod(&self) -> Vec<(&str, ModConflicts)> {
        let rank: HashMap<&str, usize> = self
            .mods
            .iter()
            .enumerate()
            .map(|(i, m)| (m.as_str(), i))
            .collect();
        let sort = |list: &mut Vec<ModFiles>| {
            list.sort_by_key(|m| rank.get(m.mod_name.as_str()).copied().unwrap_or(usize::MAX))
        };
        self.mods
            .iter()
            .filter_map(|name| {
                let mut of = self.of(name);
                sort(&mut of.overrides);
                sort(&mut of.overridden_by);
                (!of.is_empty()).then_some((name.as_str(), of))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::deploy::ModFile;

    /// A mod named `name` whose files are `(path, contents)`, written under
    /// `dir`, and placements of each straight into the game folder.
    fn mod_with(dir: &Path, name: &str, files: &[(&str, &str)]) -> (InstalledMod, Vec<Placement>) {
        let hash = ContentHash::of(name.as_bytes());
        let files: Vec<ModFile> = files
            .iter()
            .map(|(rel, contents)| {
                let src: PathBuf = dir.join(name).join(rel);
                fs::create_dir_all(src.parent().unwrap()).unwrap();
                fs::write(&src, contents).unwrap();
                ModFile {
                    rel: RelPath::new(rel).unwrap(),
                    src,
                }
            })
            .collect();
        let placements = files
            .iter()
            .map(|f| Placement {
                dst: Dest::game(f.rel.clone()),
                src: Src::Mod {
                    hash,
                    rel: f.rel.clone(),
                },
                mod_name: name.to_owned(),
            })
            .collect();
        let m = InstalledMod {
            name: name.to_owned(),
            source: "local".to_owned(),
            id: name.to_owned(),
            version: None,
            hash,
            files,
            kind: None,
        };
        (m, placements)
    }

    fn find_in(mods: &[(InstalledMod, Vec<Placement>)]) -> Conflicts {
        let installed: Vec<InstalledMod> = mods.iter().map(|(m, _)| m.clone()).collect();
        let placements: Vec<Placement> = mods.iter().flat_map(|(_, p)| p.clone()).collect();
        find(&installed, &placements)
    }

    #[test]
    fn later_mods_win_and_case_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let mods = [
            mod_with(
                tmp.path(),
                "a",
                &[("Data/x.txt", "a"), ("Data/only-a.txt", "a")],
            ),
            mod_with(
                tmp.path(),
                "b",
                &[("data/X.txt", "b"), ("Data/same.txt", "same")],
            ),
            mod_with(
                tmp.path(),
                "c",
                &[("Data/x.txt", "c"), ("Data/same.txt", "same")],
            ),
        ];
        let conflicts = find_in(&mods);
        assert_eq!(conflicts.files.len(), 2);

        let same = &conflicts.files[0];
        assert_eq!(same.dst.to_string(), "Data/same.txt");
        assert!(same.identical);

        let x = &conflicts.files[1];
        assert_eq!(x.dst.to_string(), "Data/x.txt");
        assert_eq!(x.mods, ["a", "b", "c"]);
        assert_eq!(x.winner(), "c");
        assert!(!x.identical);

        let by_mod = conflicts.by_mod();
        let a = &by_mod[0].1;
        assert!(a.overrides.is_empty() && a.lost());
        let names = |list: &[ModFiles]| list.iter().map(|m| m.mod_name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&a.overridden_by), ["b", "c"]);

        let b = &by_mod[1].1;
        assert_eq!(names(&b.overrides), ["a"]);
        assert_eq!(names(&b.overridden_by), ["c"]);
        assert_eq!(b.identical, 1);

        let c = &by_mod[2].1;
        assert_eq!(names(&c.overrides), ["a", "b"]);
        assert!(c.overridden_by.is_empty() && !c.lost());
        assert_eq!(c.identical, 1);
    }

    #[test]
    fn a_mod_placing_a_file_twice_isnt_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        let (m, mut placements) = mod_with(tmp.path(), "a", &[("x.txt", "a")]);
        placements.push(placements[0].clone());
        assert!(find(&[m], &placements).is_empty());
    }

    #[test]
    fn built_files_are_left_out() {
        let tmp = tempfile::tempdir().unwrap();
        let (m, mut placements) = mod_with(tmp.path(), "a", &[("plugins.txt", "a")]);
        placements.push(Placement {
            dst: Dest::game(RelPath::new("plugins.txt").unwrap()),
            src: Src::Built(RelPath::new("plugins.txt").unwrap()),
            mod_name: "bethesda".to_owned(),
        });
        assert!(find(&[m], &placements).is_empty());
    }
}
