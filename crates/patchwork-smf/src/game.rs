//! The game's own archives, for finding resources that mods' `dependencies`
//! ask to have copied.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::str::FromStr;

use patchwork_core::deploy::GameFiles;
use patchwork_core::relpath::RelPath;
use rpkg_rs::resource::pdefs::{PartitionId, PartitionType};
use rpkg_rs::resource::resource_info::ResourceInfo;
use rpkg_rs::resource::resource_package::ResourcePackage;
use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;

use super::RUNTIME;

/// An archive's name split into the partition it belongs to and its patch
/// number (`None` for the partition's base archive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveName {
    pub partition: PartitionId,
    pub patch: Option<usize>,
}

impl ArchiveName {
    pub fn parse(file_name: &str) -> Option<Self> {
        let stem = file_name
            .strip_suffix(".rpkg")
            .or_else(|| file_name.strip_suffix(".RPKG"))?;
        let partition = PartitionId::from_str(stem).ok()?;
        let patch = match stem.rsplit_once("patch") {
            Some((_, n)) => Some(n.parse().ok()?),
            None => None,
        };
        Some(Self { partition, patch })
    }

    /// Plain `chunkN` partitions in order, then everything else. Within a
    /// partition, later patches first, since they replace earlier ones.
    fn preference(&self) -> (bool, usize, std::cmp::Reverse<Option<usize>>) {
        let standard = matches!(self.partition.part_type, PartitionType::Standard);
        (
            !standard,
            self.partition.index,
            std::cmp::Reverse(self.patch),
        )
    }

    fn is_chunk(&self, index: usize) -> bool {
        matches!(self.partition.part_type, PartitionType::Standard) && self.partition.index == index
    }
}

/// The partitions that have a base archive in the game, by name.
pub fn partitions(game: &GameFiles<'_>) -> patchwork_core::Result<HashSet<String>> {
    let runtime = RelPath::new(RUNTIME).unwrap();
    Ok(game
        .list(Some(&runtime))?
        .iter()
        .filter_map(|rel| ArchiveName::parse(rel.file_name()))
        .filter(|name| name.patch.is_none())
        .map(|name| name.partition.to_string())
        .collect())
}

pub struct Archive {
    pub name: ArchiveName,
    pub path: PathBuf,
    pub package: ResourcePackage,
}

/// Every archive the game has, with their resource tables read.
pub struct GameIndex {
    archives: Vec<Archive>,
    /// For each resource, the archives that have it, most preferred first.
    by_resource: HashMap<RuntimeResourceID, Vec<usize>>,
}

impl GameIndex {
    pub fn load(game: &GameFiles<'_>) -> Result<Self, String> {
        let runtime = RelPath::new(RUNTIME).unwrap();
        let mut archives = Vec::new();
        for rel in game.list(Some(&runtime)).map_err(|e| e.to_string())? {
            let Some(name) = ArchiveName::parse(rel.file_name()) else {
                continue;
            };
            let Some(path) = game.original(&rel) else {
                continue;
            };
            let package = ResourcePackage::from_file(&path)
                .map_err(|e| format!("couldn't read {rel}: {e}"))?;
            archives.push(Archive {
                name,
                path,
                package,
            });
        }
        archives.sort_by_key(|a| a.name.preference());
        let mut by_resource: HashMap<_, Vec<usize>> = HashMap::new();
        for (i, archive) in archives.iter().enumerate() {
            for rrid in archive.package.resources().keys() {
                by_resource.entry(*rrid).or_default().push(i);
            }
        }
        Ok(Self {
            archives,
            by_resource,
        })
    }

    /// The version of a resource the game loads.
    pub fn latest(&self, rrid: RuntimeResourceID) -> Option<(&Archive, &ResourceInfo)> {
        let archive = &self.archives[*self.by_resource.get(&rrid)?.first()?];
        Some((archive, &archive.package.resources()[&rrid]))
    }

    /// `root` and everything it references, directly or not, that's in an
    /// archive outside chunk0 (and chunk1, unless `from_chunk1`), as RPKG
    /// Tool's `-extract_non_base_hash_depends_from` finds them. Each comes
    /// from the most preferred archive that has it.
    pub fn dependencies(
        &self,
        root: RuntimeResourceID,
        from_chunk1: bool,
    ) -> Vec<(&Archive, &ResourceInfo)> {
        let mut seen = HashSet::from([root]);
        let mut queue = vec![root];
        let mut found = Vec::new();
        while let Some(rrid) = queue.pop() {
            let Some(holders) = self.by_resource.get(&rrid) else {
                continue;
            };
            // References come from the version the game would load.
            let current = &self.archives[holders[0]];
            for (reference, _) in current.package.resources()[&rrid].references() {
                if seen.insert(*reference) {
                    queue.push(*reference);
                }
            }
            let copyable = holders
                .iter()
                .map(|&i| &self.archives[i])
                .find(|a| !a.name.is_chunk(0) && (from_chunk1 || !a.name.is_chunk(1)));
            if let Some(archive) = copyable {
                found.push((archive, &archive.package.resources()[&rrid]));
            }
        }
        found
    }
}
