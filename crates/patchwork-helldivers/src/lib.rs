//! Deploys HELLDIVERS 2 mods.
//!
//! The game (Stingray engine) reads its archives from `data/`, and a mod is a
//! set of patches to them: `<archive>.patch_<n>`, usually with
//! `.patch_<n>.gpu_resources` and `.patch_<n>.stream` beside it. The game
//! loads each archive's patches from 0 upwards and stops at the first number
//! that's missing, so two mods that both ship `9ba626afa44a3aa3.patch_0`
//! can't just be copied in: one would replace the other.
//!
//! So every patch is renumbered when it's deployed. For each archive, the
//! mods' patches are numbered in load order (and within a mod, by folder
//! and then by their own number), carrying on after any patches the game
//! ships itself. Later mods get higher numbers, which the game applies over
//! lower ones. A patch missing one of its three files gets an empty one, as
//! the community's mod manager does.
//!
//! ```toml
//! [deployer]
//! kind = "helldivers"
//! ```
//!
//! Files that aren't patches (readmes, previews, `manifest.json`) are left
//! out. Mods with several variants in folders of their own are deployed
//! whole, with a warning, until choosing options is supported.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;

use patchwork_core::deploy::{
    BuildContext, DeployContext, Deployer, DesiredState, Dest, GameFiles, InstalledMod, ModFile,
    Placement, Src,
};
use patchwork_core::error::IoContext;
use patchwork_core::ext::{Extension, Registrar};
use patchwork_core::hash::ContentHash;
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};

/// The `helldivers` extension: the deployer for HELLDIVERS 2.
pub struct Helldivers;

impl Extension for Helldivers {
    fn id(&self) -> &'static str {
        "helldivers"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(HelldiversDeployer);
    }
}

pub struct HelldiversDeployer;

/// Where the game's archives are, in its folder.
const DATA: &str = "data";

/// The empty file placed for a patch's missing parts, in the build's output.
const EMPTY: &str = "empty";

/// The three files a patch is made of, in the order they're placed.
const PARTS: [&str; 3] = ["", ".gpu_resources", ".stream"];

/// A patch file's name, taken apart.
#[derive(Debug, PartialEq, Eq)]
struct PatchName {
    /// The archive it patches, lowercased.
    archive: String,
    index: u32,
    /// Which of [`PARTS`] it is.
    part: usize,
}

impl PatchName {
    /// `<archive>.patch_<n>`, optionally followed by `.gpu_resources` or
    /// `.stream`, where the archive's name is 16 letters or digits. Case is
    /// ignored, since mods aren't always consistent.
    fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        let (archive, rest) = name.split_once('.')?;
        if archive.len() != 16 || !archive.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
        let rest = rest.strip_prefix("patch_")?;
        let (digits, part) = match rest.split_once('.') {
            Some((digits, suffix)) => {
                let part = PARTS[1..].iter().position(|p| &p[1..] == suffix)? + 1;
                (digits, part)
            }
            None => (rest, 0),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        Some(Self {
            archive: archive.to_owned(),
            index: digits.parse().ok()?,
            part,
        })
    }
}

fn data() -> RelPath {
    RelPath::new(DATA).unwrap()
}

/// The patches the game ships itself, as the number each archive's first
/// mod patch has to have.
fn game_patches(game: &GameFiles<'_>) -> Result<HashMap<String, u32>> {
    let mut next = HashMap::new();
    for rel in game.list(Some(&data()))? {
        if let Some(name) = PatchName::parse(rel.file_name()) {
            let n = next.entry(name.archive).or_insert(0);
            *n = (*n).max(name.index + 1);
        }
    }
    Ok(next)
}

/// One of a mod's patches: its files, by part.
type Patch<'a> = [Option<&'a ModFile>; 3];

/// A mod's patches, by the folder they're in, the archive and their own
/// number, so iterating gives the order they're deployed in.
fn patches(m: &InstalledMod) -> BTreeMap<(String, String, u32), Patch<'_>> {
    let mut patches: BTreeMap<_, Patch<'_>> = BTreeMap::new();
    for file in &m.files {
        let Some(name) = PatchName::parse(file.rel.file_name()) else {
            continue;
        };
        let folder = file.rel.parent().map(|p| p.to_string()).unwrap_or_default();
        let patch = patches
            .entry((folder, name.archive, name.index))
            .or_default();
        patch[name.part].get_or_insert(file);
    }
    patches
}

impl HelldiversDeployer {
    fn error(&self, message: String) -> Error {
        Error::Deployer {
            kind: self.kind().to_owned(),
            message,
        }
    }
}

impl Deployer for HelldiversDeployer {
    fn kind(&self) -> &str {
        "helldivers"
    }

    /// Everything's done in [`Deployer::build`], since the numbers depend
    /// on the patches the game already has.
    fn plan(&self, _: &DeployContext<'_>, _: &[InstalledMod]) -> Result<DesiredState> {
        Ok(DesiredState::default())
    }

    fn builds(&self, _: &DeployContext<'_>) -> bool {
        true
    }

    fn build(
        &self,
        _: &DeployContext<'_>,
        build: &BuildContext<'_>,
        mods: &[InstalledMod],
    ) -> Result<DesiredState> {
        let mut next = game_patches(&build.game)?;
        let mut desired = DesiredState::default();
        let mut empty = false;
        for m in mods {
            let patches = patches(m);
            if patches.is_empty() {
                desired.warnings.push(format!(
                    "{} has no patch files (`<archive>.patch_0` and the like), so nothing from it is deployed",
                    m.name
                ));
                continue;
            }
            let mut folders: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for (folder, archive, _) in patches.keys() {
                folders.entry(archive).or_default().insert(folder);
            }
            if folders.values().any(|f| f.len() > 1) {
                desired.warnings.push(format!(
                    "{} patches the same archive from more than one folder, which usually means it has variants to choose between; all of them are deployed, later folders over earlier ones",
                    m.name
                ));
            }
            for ((_, archive, _), files) in &patches {
                let n = next.entry(archive.clone()).or_insert(0);
                for (part, file) in PARTS.iter().zip(files) {
                    let dst = RelPath::new(&format!("{DATA}/{archive}.patch_{n}{part}"))
                        .map_err(|e| self.error(e))?;
                    let src = match file {
                        Some(file) => m.src(file),
                        None => {
                            empty = true;
                            Src::Built(RelPath::new(EMPTY).unwrap())
                        }
                    };
                    desired.placements.push(Placement {
                        dst: Dest::game(dst),
                        src,
                        mod_name: m.name.clone(),
                    });
                }
                *n += 1;
            }
        }
        if empty {
            let path = build.out.join(EMPTY);
            fs::write(&path, []).at(&path)?;
        }
        Ok(desired)
    }

    /// The names of the patches the game ships itself, which decide where
    /// mods' numbering starts.
    fn fingerprint(
        &self,
        _: &DeployContext<'_>,
        game: &GameFiles<'_>,
    ) -> Result<Option<ContentHash>> {
        let mut names: Vec<_> = game
            .list(Some(&data()))?
            .into_iter()
            .filter(|rel| PatchName::parse(rel.file_name()).is_some())
            .map(|rel| rel.file_name().to_owned())
            .collect();
        names.sort();
        Ok(Some(ContentHash::of(names.join("\n").as_bytes())))
    }
}

#[cfg(test)]
mod tests;
