//! Deploys Deep Rock Galactic mods the way mint (the community's mod
//! integration tool) does, so they work without the game's own mod.io
//! integration.
//!
//! A mod is an Unreal `.pak` (mod.io's zips have one inside). Deploying
//! builds one `FSD/Content/Paks/mods_P.pak`, which the game loads after its
//! own pak, with:
//!
//! - every mod's files, later mods' over earlier ones' at the same path.
//!   Paths are matched without case, and given the case the game's own
//!   files have. Mods' `AssetRegistry.bin` and shader bytecode are left
//!   out, as mint does.
//! - the game's `AssetRegistry.bin` with every mod's assets added, which is
//!   how mods' `InitSpacerig` and `InitCave` blueprints are found.
//! - a few of the game's blueprints, changed to start mods (see
//!   [`integrate`]), and mint's own blueprints that do it, from
//!   `assets/integration`.
//!
//! ```toml
//! [deployer]
//! kind = "drg"
//! ```
//!
//! Not done (mint does these with a DLL it puts next to the game's exe):
//! turning off the game's own mod.io integration, so mods subscribed to in
//! game also load; showing the mods in game and in the server list; and
//! saving mods' save games on the Microsoft Store version.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Cursor, Read, Seek, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

use include_dir::{Dir as IncludedDir, DirEntry, include_dir};
use patchwork_core::deploy::{
    BuildContext, DeployContext, Deployer, DesiredState, Dest, GameFiles, InstalledMod, Placement,
    Src,
};
use patchwork_core::error::IoContext;
use patchwork_core::event::Event;
use patchwork_core::ext::{Extension, Registrar};
use patchwork_core::hash::ContentHash;
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};
use uasset_utils::asset_registry::{AssetRegistry, Readable as _, Writable as _};
use unreal_asset::engine_version::EngineVersion;
use unreal_asset::{Asset, AssetBuilder};

mod integrate;

#[cfg(test)]
mod tests;

/// The `drg` extension: the deployer for Deep Rock Galactic.
pub struct Drg;

impl Extension for Drg {
    fn id(&self) -> &'static str {
        "drg"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(DrgDeployer);
    }
}

pub struct DrgDeployer;

/// Where the game's paks are.
const PAKS: &str = "FSD/Content/Paks";

/// The game's own pak: Steam's, then the Microsoft Store's.
const GAME_PAKS: [&str; 2] = ["FSD-WindowsNoEditor.pak", "FSD-WinGDK.pak"];

/// The pak that's built, in the build's output and in [`PAKS`]. It's the
/// name mint uses, so deploying replaces a pak mint made rather than
/// loading both.
const BUILT: &str = "mods_P.pak";

/// Shown as the mod the pak comes from.
const BUILT_BY: &str = "DRG mods";

/// Where every path in the paks is relative to.
const MOUNT: &str = "../../../";

const ASSET_REGISTRY: &str = "FSD/AssetRegistry.bin";

/// The game's player controller, which [`integrate::hook_pcb`] makes
/// spawn mods.
const PLAYER_CONTROLLER: &str = "FSD/Content/Game/BP_PlayerControllerBase";

/// The game's blueprints [`integrate::patch`] changes.
const PATCHED: [&str; 6] = [
    "FSD/Content/Game/BP_GameInstance",
    "FSD/Content/Game/SpaceRig/BP_PlayerController_SpaceRig",
    "FSD/Content/Game/StartMenu/Bp_StartMenu_PlayerController",
    "FSD/Content/UI/Menu_DeepDives/ITM_DeepDives_Join",
    "FSD/Content/UI/Menu_ServerList/_MENU_ServerList",
    "FSD/Content/UI/Menu_ServerList/WND_JoiningModded",
];
const ESCAPE_MENU: &str = "FSD/Content/UI/Menu_EscapeMenu/MENU_EscapeMenu";
const MODDING_TAB: &str = "FSD/Content/UI/Menu_EscapeMenu/Modding/MENU_Modding";
const SERVER_LIST_ENTRY: &str = "FSD/Content/UI/Menu_ServerList/ITM_ServerList_Entry";

/// mint's blueprints, packed into every build.
static INTEGRATION: IncludedDir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/integration");

impl DrgDeployer {
    fn error(&self, message: String) -> Error {
        Error::Deployer {
            kind: self.kind().to_owned(),
            message,
        }
    }
}

/// The game's own pak: where it is in the game's folder, and the file.
fn locate(game: &GameFiles<'_>) -> Option<(RelPath, std::path::PathBuf)> {
    GAME_PAKS.iter().find_map(|name| {
        let rel = RelPath::new(&format!("{PAKS}/{name}")).unwrap();
        let path = game.original(&rel)?;
        Some((rel, path))
    })
}

fn stamp(path: &Path) -> Result<String> {
    let meta = fs::metadata(path).at(path)?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    Ok(format!("{} {modified}", meta.len()))
}

impl Deployer for DrgDeployer {
    fn kind(&self) -> &str {
        "drg"
    }

    /// Everything's in [`Deployer::build`], since every mod ends up in the
    /// one pak.
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
        let mut desired = DesiredState::default();
        let mut paks = Vec::new();
        for m in mods {
            let mut found: Vec<&Path> = m
                .files
                .iter()
                .filter(|f| {
                    Path::new(f.rel.file_name())
                        .extension()
                        .is_some_and(|e| e.eq_ignore_ascii_case("pak"))
                })
                .map(|f| f.src.as_path())
                .collect();
            if found.is_empty() {
                desired.warnings.push(format!(
                    "{} has no .pak, so none of it was deployed",
                    m.name
                ));
            }
            found.sort();
            paks.extend(found.into_iter().map(|p| (m.name.as_str(), p)));
        }
        if paks.is_empty() {
            return Ok(desired);
        }
        let (_, base) = locate(&build.game).ok_or_else(|| {
            self.error(format!(
                "the game's pak wasn't found (looked for {} in {PAKS})",
                GAME_PAKS.join(" and ")
            ))
        })?;
        let out = build.out.join(BUILT);
        let file = File::create(&out).at(&out)?;
        let warnings = integrate_mods(&base, &paks, BufWriter::new(file), &|step| {
            build.reporter.report(Event::Building {
                step: step.to_owned(),
            })
        })
        .map_err(|e| self.error(e))?;
        desired.warnings.extend(warnings);
        desired.placements.push(Placement {
            dst: Dest::game(RelPath::new(&format!("{PAKS}/{BUILT}")).unwrap()),
            src: Src::Built(RelPath::new(BUILT).unwrap()),
            mod_name: BUILT_BY.to_owned(),
        });
        Ok(desired)
    }

    /// The size and modification time of the game's pak, whose blueprints
    /// and asset registry are built on.
    fn fingerprint(
        &self,
        _: &DeployContext<'_>,
        game: &GameFiles<'_>,
    ) -> Result<Option<ContentHash>> {
        let Some((rel, path)) = locate(game) else {
            return Ok(None);
        };
        let key = format!("{rel} {}", stamp(&path)?);
        Ok(Some(ContentHash::of(key.as_bytes())))
    }
}

/// A blueprint's two files.
#[derive(Debug, Default)]
struct RawAsset {
    uasset: Option<Vec<u8>>,
    uexp: Option<Vec<u8>>,
}

impl RawAsset {
    fn parse(&self, path: &str) -> std::result::Result<Asset<Cursor<&Vec<u8>>>, String> {
        let (Some(uasset), Some(uexp)) = (&self.uasset, &self.uexp) else {
            return Err(format!(
                "{path} isn't in the game's pak; this version of the game may not be supported"
            ));
        };
        AssetBuilder::new(Cursor::new(uasset), EngineVersion::VER_UE4_27)
            .bulk(Cursor::new(uexp))
            .build()
            .map_err(|e| format!("couldn't read {path}: {e}"))
    }
}

/// Packs `paks` (by mod name, in load order) into a pak written to `out`,
/// with the game's pak at `base` as described in the crate's docs. Returns
/// warnings.
fn integrate_mods<W: Write + Seek>(
    base: &Path,
    paks: &[(&str, &Path)],
    out: W,
    report: &dyn Fn(&str),
) -> std::result::Result<Vec<String>, String> {
    let mut warnings = Vec::new();
    report("reading the game's pak");
    let mut base_reader = BufReader::new(
        File::open(base).map_err(|e| format!("couldn't open {}: {e}", base.display()))?,
    );
    let game_pak = repak::PakBuilder::new()
        .reader(&mut base_reader)
        .map_err(|e| format!("couldn't read {}: {e}", base.display()))?;
    let registry_bytes = game_pak
        .get(ASSET_REGISTRY, &mut base_reader)
        .map_err(|e| format!("couldn't read {ASSET_REGISTRY} from the game's pak: {e}"))?;
    let mut registry = AssetRegistry::read(&mut Cursor::new(registry_bytes))
        .map_err(|e| format!("couldn't read the game's {ASSET_REGISTRY}: {e}"))?;

    let mut patched: BTreeMap<&str, RawAsset> = [PLAYER_CONTROLLER]
        .into_iter()
        .chain(PATCHED)
        .chain([ESCAPE_MENU, MODDING_TAB, SERVER_LIST_ENTRY])
        .map(|path| (path, RawAsset::default()))
        .collect();
    let game_file = |path: &str, reader: &mut BufReader<File>| match game_pak.get(path, reader) {
        Ok(data) => Ok(Some(data)),
        Err(repak::Error::MissingEntry(_)) => Ok(None),
        Err(e) => Err(format!("couldn't read {path} from the game's pak: {e}")),
    };

    let mut writer = PakWriter::new(out, &game_pak.files());
    // Paths already written, lowercased: later mods are read first, so
    // the first copy of a file is the one that's kept.
    let mut written = HashSet::new();
    // Which of `patched` a mod has replaced.
    let mut replaced = HashSet::new();

    for (mod_name, path) in paks.iter().rev() {
        report(&format!("adding {mod_name}"));
        let context = |e: &dyn std::fmt::Display| format!("{mod_name}: {}: {e}", path.display());
        let mut reader = BufReader::new(File::open(path).map_err(|e| context(&e))?);
        let pak = repak::PakBuilder::new()
            .reader(&mut reader)
            .map_err(|e| context(&e))?;
        let mount = pak.mount_point().to_owned();
        let mut files = HashMap::new();
        for in_pak in pak.files() {
            let full = join_mount(&mount, &in_pak);
            let normalized = full.strip_prefix(MOUNT).ok_or_else(|| {
                format!("{mod_name}: {in_pak} is mounted at {mount}, outside the game's folder")
            })?;
            // In the game's case, so that the game's blueprints are found
            // whatever case a mod has them in.
            files.insert(writer.game_case(normalized), in_pak);
        }

        for (normalized, in_pak) in &files {
            let Some(stem) = normalized
                .strip_suffix(".uasset")
                .or_else(|| normalized.strip_suffix(".umap"))
            else {
                continue;
            };
            let Some(uexp) = files.get(&format!("{stem}.uexp")) else {
                continue;
            };
            let read =
                |p: &str, reader: &mut BufReader<File>| pak.get(p, reader).map_err(|e| context(&e));
            let uasset = read(in_pak, &mut reader)?;
            let uexp = read(uexp, &mut reader)?;
            let asset = AssetBuilder::new(Cursor::new(uasset), EngineVersion::VER_UE4_27)
                .bulk(Cursor::new(uexp))
                .skip_data(true)
                .build();
            match asset {
                Ok(asset) => registry
                    .populate(stem, &asset)
                    .map_err(|e| format!("{mod_name}: couldn't register {normalized}: {e}"))?,
                Err(e) => warnings.push(format!(
                    "{mod_name}: {normalized} couldn't be read, so it's missing from the asset \
                     registry: {e}"
                )),
            }
        }

        for (normalized, in_pak) in files {
            let name = normalized.rsplit('/').next().unwrap_or(&normalized);
            if name == "AssetRegistry.bin" || name.ends_with(".ushaderbytecode") {
                continue;
            }
            let lower = normalized.to_ascii_lowercase();
            let blueprint = normalized
                .strip_suffix(".uasset")
                .map(|p| (p, true))
                .or_else(|| normalized.strip_suffix(".uexp").map(|p| (p, false)))
                .and_then(|(p, uasset)| Some((*patched.get_key_value(p)?.0, uasset)));
            if let Some((blueprint, uasset)) = blueprint {
                if replaced.insert(lower) {
                    let data = pak.get(&in_pak, &mut reader).map_err(|e| context(&e))?;
                    let raw = patched.get_mut(blueprint).unwrap();
                    match uasset {
                        true => raw.uasset = Some(data),
                        false => raw.uexp = Some(data),
                    }
                }
                continue;
            }
            if !written.insert(lower) {
                continue;
            }
            let data = pak.get(&in_pak, &mut reader).map_err(|e| context(&e))?;
            writer.write_file(&data, &normalized)?;
        }
    }

    report("changing the game's blueprints");
    for (path, raw) in &mut patched {
        if raw.uasset.is_none() {
            raw.uasset = game_file(&format!("{path}.uasset"), &mut base_reader)?;
        }
        if raw.uexp.is_none() {
            raw.uexp = game_file(&format!("{path}.uexp"), &mut base_reader)?;
        }
    }
    for (path, raw) in &patched {
        let mut asset = raw.parse(path)?;
        let changed = match *path {
            PLAYER_CONTROLLER => integrate::hook_pcb(&mut asset),
            ESCAPE_MENU => integrate::patch_modding_tab(&mut asset),
            MODDING_TAB => integrate::patch_modding_tab_item(&mut asset),
            SERVER_LIST_ENTRY => integrate::patch_server_list_entry(&mut asset),
            _ => integrate::patch(&mut asset),
        };
        changed.map_err(|e| format!("couldn't change the game's {path}: {e}"))?;
        writer.write_asset(asset, path)?;
    }

    let mut integration = Vec::new();
    included_files(&INTEGRATION, &mut integration);
    for (path, data) in integration {
        writer.write_file(data, &path)?;
    }

    report("writing the asset registry");
    let mut bytes = Vec::new();
    registry
        .write(&mut bytes)
        .map_err(|e| format!("couldn't write the asset registry: {e}"))?;
    writer.write_file(&bytes, ASSET_REGISTRY)?;
    writer.finish()?;
    Ok(warnings)
}

/// A file's path in a pak mounted at `mount`.
fn join_mount(mount: &str, path: &str) -> String {
    if mount.is_empty() || mount.ends_with('/') {
        format!("{mount}{path}")
    } else {
        format!("{mount}/{path}")
    }
}

/// Every file under `dir`, by its path there.
fn included_files<'a>(dir: &'a IncludedDir<'a>, files: &mut Vec<(String, &'a [u8])>) {
    for entry in dir.entries() {
        match entry {
            DirEntry::Dir(dir) => included_files(dir, files),
            DirEntry::File(file) => files.push((
                file.path().to_string_lossy().replace('\\', "/"),
                file.contents(),
            )),
        }
    }
}

/// A folder in the game's pak, for giving mods' paths the game's case.
#[derive(Debug, Default)]
struct Folder {
    name: String,
    children: HashMap<String, Folder>,
}

/// Writes the built pak: zlib compressed, version 11, as mint does.
struct PakWriter<W: Write + Seek> {
    pak: repak::PakWriter<W>,
    game: HashMap<String, Folder>,
}

impl<W: Write + Seek> PakWriter<W> {
    fn new(out: W, game_files: &[String]) -> Self {
        let mut game: HashMap<String, Folder> = HashMap::new();
        for path in game_files {
            let mut children = &mut game;
            for part in path.split('/').filter(|p| !p.is_empty()) {
                children = &mut children
                    .entry(part.to_ascii_lowercase())
                    .or_insert_with(|| Folder {
                        name: part.to_owned(),
                        children: HashMap::new(),
                    })
                    .children;
            }
        }
        PakWriter {
            pak: repak::PakBuilder::new()
                .compression([repak::Compression::Zlib])
                .writer(out, repak::Version::V11, MOUNT.to_owned(), None),
            game,
        }
    }

    /// `path`, with the case of the game's own file or folders where it
    /// has them, since the game looks paths up with case.
    fn game_case(&self, path: &str) -> String {
        let mut children = Some(&self.game);
        let mut parts = Vec::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            match children.and_then(|c| c.get(&part.to_ascii_lowercase())) {
                Some(folder) => {
                    parts.push(folder.name.as_str());
                    children = Some(&folder.children);
                }
                None => {
                    parts.push(part);
                    children = None;
                }
            }
        }
        parts.join("/")
    }

    fn write_file(&mut self, data: &[u8], path: &str) -> std::result::Result<(), String> {
        let path = self.game_case(path);
        self.pak
            .write_file(&path, data)
            .map_err(|e| format!("couldn't write {path} to {BUILT}: {e}"))
    }

    fn write_asset<C: Read + Seek>(
        &mut self,
        asset: Asset<C>,
        path: &str,
    ) -> std::result::Result<(), String> {
        let mut uasset = Cursor::new(Vec::new());
        let mut uexp = Cursor::new(Vec::new());
        asset
            .write_data(&mut uasset, Some(&mut uexp))
            .map_err(|e| format!("couldn't write the changed {path}: {e}"))?;
        self.write_file(uasset.get_ref(), &format!("{path}.uasset"))?;
        self.write_file(uexp.get_ref(), &format!("{path}.uexp"))
    }

    fn finish(self) -> std::result::Result<(), String> {
        self.pak
            .write_index()
            .map(drop)
            .map_err(|e| format!("couldn't write {BUILT}: {e}"))
    }
}
