//! Deploys mods made for Simple Mod Framework (SMF), the HITMAN World of
//! Assassination mod loader, without running SMF itself.
//!
//! The game reads its resources from `.rpkg` archives in `Runtime/`, so
//! deploying means building archives of our own: mods' resources are staged
//! per partition (later mods replacing earlier ones' resources), packed into
//! `<partition>patch300.rpkg`, and `packagedefinition.txt` is changed so the
//! game loads them. Plain RPKG mods (archives without a manifest) have their
//! resources staged the same way.
//!
//! ```toml
//! [deployer]
//! kind = "smf"
//! ```
//!
//! Besides raw resources, SMF's JSON content types are supported: edits to
//! the unlockables and repository, contracts, blobs and JSON patches. These
//! change whichever version of a resource is current: an earlier mod's if
//! one staged it, otherwise the game's. So are entities, whole or as patches
//! (see [`entity`]); like SMF, a mod's entity patches are applied once the
//! rest of its content is staged. Textures are converted from TGA (see
//! [`texture`]). Changes to the game's localised strings (`localisation`,
//! `localisationOverrides`) are applied once every mod's staged, like SMF
//! does, to the current version of each LOCR (see [`locr`]), and
//! `localisedLines` become LINE resources. Whole LOCRs can be given as
//! `.locr.json`.
//!
//! Dialogue (`.dlge.json`, `.ditl.json`,
//! `.clng.json`) is rebuilt natively (see [`dlge`]), and so are materials
//! (`.material.json`, see [`material`]).
//!
//! Peacock plugins are placed in the game's folder, listed in a deploy summary
//! Peacock can read (see [`peacock`]).
//!
//! Mods' options are picked with the `smf.options` hook (see [`options`]),
//! and conditional options are on or off by what else is deployed, as in
//! SMF.
//!
//! Not supported yet: SMF's other special file types (sounds,
//! `.rtlv.json` and deltas), entities made for QuickEntity versions
//! before 3.1, and scripts. Mods that need them fail to deploy rather than
//! half working.

mod cache;
mod condition;
mod dlge;
mod entity;
mod game;
mod json;
mod locr;
mod manifest;
mod material;
mod meta;
mod options;
mod ores;
mod peacock;
mod resource;
mod text;
mod texture;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::UNIX_EPOCH;

use patchwork_core::deploy::{
    BuildContext, DeployContext, Deployer, DesiredState, Dest, GameFiles, InstalledMod, ModFile,
    Placement, Src,
};
use patchwork_core::error::IoContext;
use patchwork_core::event::Event;
use patchwork_core::ext::{Extension, Registrar};
use patchwork_core::hash::ContentHash;
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};
use quickentity_rs::patch_structs::Patch;
use rpkg_rs::resource::pdefs::PartitionId;
use rpkg_rs::resource::resource_partition::PatchId;
use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;
use serde_json::Value;

use game::{ArchiveName, GameIndex};
use json::Destination;
use manifest::{Dependency, Manifest, PackageDefinitionEntry};
use meta::Meta;
use ores::Ores;
use resource::{Archives, Header, Loaded, Resource, read_package};

/// HITMAN World of Assassination, with mods made for Simple Mod Framework.
pub struct Smf;

impl Extension for Smf {
    fn id(&self) -> &'static str {
        "smf"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(SmfDeployer)
            .mod_options("options", options::SmfOptions);
    }
}

pub struct SmfDeployer;

const RUNTIME: &str = "Runtime";
const PACKAGE_DEFINITION: &str = "Runtime/packagedefinition.txt";
const THUMBS: &str = "Retail/thumbs.dat";
/// The patch number of the archives deploying writes; see
/// [`text::PATCH_LEVEL`].
const PATCH: usize = 300;
/// Shown as the mod deployed files come from.
const BUILT_BY: &str = "SMF mods";

/// SMF content files that are converted rather than copied, which patchwork
/// can't do yet.
const UNSUPPORTED_TYPES: &[&str] = &["rtlv.json", "sfx.wem", "delta"];

/// The partition SMF puts edits to the game's lookup tables in.
const BASE: &str = "chunk0";

/// The manifest keys string changes come from.
const LOCALISATION: &str = "localisation";
const LOCALISATION_OVERRIDES: &str = "localisationOverrides";

/// The flag a LINE resource's reference to its LOCR has.
const LINE_REFERENCE: u8 = 0x1F;

impl Deployer for SmfDeployer {
    fn kind(&self) -> &str {
        "smf"
    }

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
        Build::new(build)
            .run(mods)
            .map_err(|message| Error::Deployer {
                kind: self.kind().to_owned(),
                message,
            })
    }

    /// The package definition and thumbs file by content, and the game's
    /// archives by size and modification time.
    fn fingerprint(
        &self,
        _: &DeployContext<'_>,
        game: &GameFiles<'_>,
    ) -> Result<Option<ContentHash>> {
        let mut hasher = Vec::new();
        for file in [PACKAGE_DEFINITION, THUMBS] {
            let rel = RelPath::new(file).unwrap();
            let hash = match game.original(&rel) {
                Some(path) => ContentHash::of_file(&path)?.to_string(),
                None => String::new(),
            };
            hasher.extend_from_slice(format!("{file}={hash}\n").as_bytes());
        }
        let runtime = RelPath::new(RUNTIME).unwrap();
        for rel in game.list(Some(&runtime))? {
            if ArchiveName::parse(rel.file_name()).is_none() {
                continue;
            }
            let path = rel.under(game.root());
            let meta = fs::metadata(&path).at(&path)?;
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos());
            hasher.extend_from_slice(format!("{rel}={}:{modified}\n", meta.len()).as_bytes());
        }
        Ok(Some(ContentHash::of(&hasher)))
    }
}

struct Build<'a> {
    ctx: &'a BuildContext<'a>,
    /// Resources for each partition, by runtime ID.
    staged: BTreeMap<String, BTreeMap<u64, Resource>>,
    package_definition: Vec<PackageDefinitionEntry>,
    thumbs: Vec<String>,
    warnings: Vec<String>,
    /// Problems that stop the deploy, gathered so they can all be reported
    /// at once.
    errors: Vec<String>,
    game: Option<GameIndex>,
    archives: Archives,
    /// Contracts to list in the Destinations menu once every mod's staged.
    destinations: Vec<Destination>,
    /// Manifest ids of the mods being deployed, and what they require.
    ids: HashSet<String>,
    /// What SMF's `config.loadOrder` would be, for conditional options.
    load_order: HashSet<String>,
    checks: Vec<(String, Vec<String>, Vec<String>)>,
    /// The current mod's entity patches, applied once its content's staged.
    entity_patches: Vec<EntityPatch>,
    /// Every mod's changes to localised strings, applied once all mods are
    /// staged: `localisation` first, then `localisationOverrides`, like SMF.
    string_overrides: Vec<StringOverride>,
    server_side: peacock::ServerSide,
    /// The plugins to place, from the mods they're in.
    plugin_placements: Vec<Placement>,
}

struct StringOverride {
    label: String,
    /// The manifest key it comes from.
    feature: &'static str,
    locr: u64,
    language: usize,
    hash: u32,
    text: String,
}

struct EntityPatch {
    partition: String,
    factory: u64,
    blueprint: u64,
    file: RelPath,
    patch: Patch,
    /// The patch file's contents, for the cache key.
    source: Vec<u8>,
}

impl<'a> Build<'a> {
    fn new(ctx: &'a BuildContext<'a>) -> Self {
        Self {
            ctx,
            staged: BTreeMap::new(),
            package_definition: Vec::new(),
            thumbs: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            game: None,
            archives: Archives::default(),
            destinations: Vec::new(),
            ids: HashSet::new(),
            load_order: HashSet::new(),
            checks: Vec::new(),
            entity_patches: Vec::new(),
            string_overrides: Vec::new(),
            server_side: peacock::ServerSide::default(),
            plugin_placements: Vec::new(),
        }
    }

    fn run(mut self, mods: &[InstalledMod]) -> Result<DesiredState, String> {
        self.load_order = options::load_order(mods);
        for m in mods {
            self.ctx.reporter.report(Event::Building {
                step: m.name.clone(),
            });
            self.stage_mod(m)?;
        }
        self.apply_string_overrides()?;
        self.check_requirements();
        if !self.destinations.is_empty()
            && self.errors.is_empty()
            && let Err(e) = self.stage_destinations()
        {
            self.errors.push(format!(
                "couldn't add contracts to the Destinations menu: {e}"
            ));
        }
        // Some problems are found once per file, but are only worth saying
        // once.
        dedupe(&mut self.errors);
        dedupe(&mut self.warnings);
        if !self.errors.is_empty() {
            return Err(self.errors.join("\n"));
        }
        let mut placements = self.write_server_side()?;
        if self.staged.is_empty() && self.package_definition.is_empty() && self.thumbs.is_empty() {
            return Ok(DesiredState {
                placements,
                warnings: self.warnings,
            });
        }

        let partitions = game::partitions(&self.ctx.game).map_err(|e| e.to_string())?;
        for (partition, resources) in std::mem::take(&mut self.staged) {
            self.ctx.reporter.report(Event::Building {
                step: format!("{partition} archive"),
            });
            let patch = if partitions.contains(&partition) {
                PatchId::Patch(PATCH)
            } else {
                // A partition a mod added to the package definition.
                PatchId::Base
            };
            let id = PartitionId::from_str(&partition).map_err(|e| e.to_string())?;
            let name = id.to_filename(patch);
            let rel = format!("{RUNTIME}/{name}");
            let out = self.output(&rel)?;
            self.archives.write(id, patch, resources, &out)?;
            placements.push(placement(&rel));
        }

        placements.push(self.write_package_definition()?);
        if !self.thumbs.is_empty() {
            placements.push(self.write_thumbs()?);
        }
        Ok(DesiredState {
            placements,
            warnings: self.warnings,
        })
    }

    fn stage_mod(&mut self, m: &InstalledMod) -> Result<(), String> {
        let manifests = options::manifests(m);
        if !manifests.is_empty() {
            for file in manifests {
                self.stage_framework_mod(m, file)?;
            }
            return Ok(());
        }

        let archives: Vec<&ModFile> = m
            .files
            .iter()
            .filter(|f| has_extension(f.rel.file_name(), "rpkg"))
            .collect();
        if archives.is_empty() {
            self.warnings.push(format!(
                "`{}` has no manifest.json or .rpkg files, so there's nothing to deploy",
                m.name
            ));
        }
        for file in archives {
            let partition = archive_partition(&file.rel);
            let package = read_package(&file.src)
                .map_err(|e| format!("`{}`: couldn't read {}: {e}", m.name, file.rel))?;
            let staged = self.staged.entry(partition).or_default();
            for (rrid, info) in package.resources() {
                staged.insert(
                    u64::from(*rrid),
                    Resource::Packaged {
                        archive: file.src.clone(),
                        info: info.clone(),
                    },
                );
            }
        }
        Ok(())
    }

    fn stage_framework_mod(&mut self, m: &InstalledMod, file: &ModFile) -> Result<(), String> {
        let text = fs::read_to_string(&file.src)
            .map_err(|e| format!("`{}`: couldn't read {}: {e}", m.name, file.rel))?;
        let manifest = Manifest::parse(&text)
            .map_err(|e| format!("`{}`: {} isn't a valid SMF manifest: {e}", m.name, file.rel))?;
        let root = file.rel.parent();
        let files = ModFiles::new(m, root.as_ref());
        let label = format!("`{}` ({})", m.name, manifest.id);

        let picked = options::picked(m).unwrap_or_else(|e| {
            self.warnings
                .push(format!("{label} gets its default options, since {e}"));
            None
        });
        let (active, option_warnings) = manifest.active(picked.as_deref(), &self.load_order);
        for warning in option_warnings {
            self.warnings.push(format!("{label}: {warning}"));
        }
        let mut requirements = Vec::new();
        let mut incompatibilities = Vec::new();
        for data in &active {
            for feature in data.unsupported() {
                self.errors.push(format!(
                    "{label} uses `{feature}`, which patchwork can't deploy yet"
                ));
            }
            for plugin in &data.peacock_plugins {
                self.stage_plugin(m, &manifest.id, &label, &files, plugin);
            }
            for folder in &data.content_folders {
                self.stage_content(&label, &files, folder);
            }
            for folder in &data.blobs_folders {
                if let Err(e) = self.stage_blobs(&files, folder) {
                    self.errors
                        .push(format!("{label}: blobs in `{folder}`: {e}"));
                }
            }
            self.package_definition
                .extend(data.packagedefinition.iter().cloned());
            self.thumbs.extend(data.thumbs.iter().cloned());
            self.queue_localisation(&label, data);
            self.queue_string_overrides(&label, data);
            self.stage_lines(&label, data);
            for dependency in &data.dependencies {
                self.stage_dependency(&label, dependency)?;
            }
            requirements.extend(data.requirements.iter().map(|r| r.id().to_owned()));
            incompatibilities.extend(data.incompatibilities.iter().map(|r| r.id().to_owned()));
        }
        self.apply_entity_patches(&label)?;
        self.ids.insert(manifest.id.clone());
        self.checks.push((label, requirements, incompatibilities));
        Ok(())
    }

    /// Places a Peacock plugin, to be listed in the deploy summary.
    fn stage_plugin(
        &mut self,
        m: &InstalledMod,
        id: &str,
        label: &str,
        files: &ModFiles<'_>,
        plugin: &str,
    ) {
        let rel = plugin.replace('\\', "/");
        let Some(file) = files.get(rel.strip_prefix("./").unwrap_or(&rel)) else {
            self.errors
                .push(format!("{label}: Peacock plugin {plugin} isn't in the mod"));
            return;
        };
        let Some(dst) = peacock::plugin_path(id, plugin) else {
            self.errors.push(format!(
                "{label}: Peacock plugin {plugin} can't be placed under a path made from its id"
            ));
            return;
        };
        if self.server_side.plugins.contains(&dst) {
            return;
        }
        self.plugin_placements.push(Placement {
            dst: Dest::game(RelPath::new(&dst).unwrap()),
            src: m.src(file),
            mod_name: BUILT_BY.to_owned(),
        });
        self.server_side.plugins.push(dst);
    }

    /// Stages a content folder: `<folder>/<partition>/...` holding raw
    /// resources, each with an optional `.meta` or `.meta.json`.
    fn stage_content(&mut self, label: &str, files: &ModFiles<'_>, folder: &str) {
        for (rest, file) in files.under(folder) {
            let mut parts = rest.splitn(2, '/');
            let (Some(partition), Some(_)) = (parts.next(), parts.next()) else {
                continue;
            };
            let name = file.rel.file_name();
            let lower = name.to_ascii_lowercase();
            let Some((stem, extension)) = lower.split_once('.') else {
                continue;
            };
            if extension.ends_with("meta") || extension.ends_with("meta.json") {
                continue;
            }
            if UNSUPPORTED_TYPES.contains(&extension) {
                self.errors.push(format!(
                    "{label} has .{extension} files, which patchwork can't deploy yet"
                ));
                continue;
            }
            let partition = partition.to_ascii_lowercase();
            if PartitionId::from_str(&partition).is_err() {
                self.errors.push(format!(
                    "{label}: {} is in `{partition}`, which isn't a partition like chunk0",
                    file.rel
                ));
                continue;
            }
            let rel = format!("{}/{rest}", folder.trim_matches('/'));
            let special = match extension {
                "unlockables.json" => Some(self.stage_unlockables(file)),
                "repository.json" => Some(self.stage_repository(file)),
                "contract.json" => Some(self.stage_contract(file)),
                "json.patch.json" => Some(self.stage_json_patch(file, &partition)),
                "entity.json" => Some(self.stage_entity(file, &partition)),
                "entity.patch.json" => Some(self.queue_entity_patch(file, &partition)),
                "texture.tga" => Some(self.stage_texture(files, &rel, file, stem, &partition)),
                "dlge.json" => Some(self.stage_dialogue(file, &partition, "DLGE")),
                "ditl.json" => Some(self.stage_dialogue(file, &partition, "DITL")),
                "clng.json" => Some(self.stage_dialogue(file, &partition, "CLNG")),
                "material.json" => Some(self.stage_material(file, &partition)),
                "locr.json" => Some(self.stage_locr(file, &partition)),
                _ => None,
            };
            if let Some(result) = special {
                if let Err(e) = result {
                    self.errors.push(format!("{label}: {}: {e}", file.rel));
                }
                continue;
            }
            // Anything else that isn't `<hash>.<TYPE>` is ignored, like SMF does.
            if extension.len() != 4 {
                continue;
            }
            let Some(rrid) = meta::parse_hex_hash(stem) else {
                self.errors.push(format!(
                    "{label}: {} isn't named after a runtime ID",
                    file.rel
                ));
                continue;
            };
            let meta = match read_meta(files, &rel) {
                Ok(meta) => meta,
                Err(e) => {
                    self.errors.push(format!("{label}: {e}"));
                    continue;
                }
            };
            let resource_type = match &meta {
                Some(meta) => meta.resource_type.clone(),
                None => name[stem.len() + 1..].to_ascii_uppercase(),
            };
            self.staged.entry(partition).or_default().insert(
                u64::from(rrid),
                Resource::File {
                    path: file.src.clone(),
                    resource_type,
                    meta,
                },
            );
        }
    }

    /// Copies what a resource depends on into a partition, without replacing
    /// anything already staged there. Like SMF (which hands it to rpkg-cli
    /// as a filter), a dependency can list several resources, separated by
    /// commas.
    fn stage_dependency(&mut self, label: &str, dependency: &Dependency) -> Result<(), String> {
        for id in dependency.runtime_id().split(',').map(str::trim) {
            let Some(rrid) = meta::parse_hex_hash(id) else {
                self.errors
                    .push(format!("{label}: dependency `{id}` isn't a runtime ID"));
                continue;
            };
            let found: Vec<_> = self
                .game()?
                .dependencies(rrid, dependency.includes_chunk1())
                .into_iter()
                .map(|(archive, info)| (archive.path.clone(), info.clone()))
                .collect();
            if found.is_empty() {
                self.warnings.push(format!(
                    "{label}: the game has nothing to copy for dependency {id}"
                ));
            }
            let staged = self
                .staged
                .entry(format!("chunk{}", dependency.to_chunk()))
                .or_default();
            for (archive, info) in found {
                staged
                    .entry(u64::from(*info.rrid()))
                    .or_insert(Resource::Packaged { archive, info });
            }
        }
        Ok(())
    }

    fn game(&mut self) -> Result<&GameIndex, String> {
        if self.game.is_none() {
            self.ctx.reporter.report(Event::Building {
                step: "an index of the game's archives".to_owned(),
            });
            self.game = Some(GameIndex::load(&self.ctx.game)?);
        }
        Ok(self.game.as_ref().unwrap())
    }

    /// The current version of a resource for `partition`: what's staged
    /// there, otherwise the game's.
    fn current(&mut self, partition: &str, hash: u64) -> Result<Option<Loaded>, String> {
        if let Some(resource) = self.staged.get(partition).and_then(|s| s.get(&hash)) {
            return self.archives.load(resource).map(Some);
        }
        match self.game()?.latest(RuntimeResourceID::from(hash)) {
            Some((archive, info)) => resource::load_packaged(&archive.package, info)
                .map(Some)
                .map_err(|e| format!("{}: {e}", archive.path.display())),
            None => Ok(None),
        }
    }

    /// [`Build::current`] for a resource the game is sure to have.
    fn current_game(&mut self, partition: &str, hash: u64) -> Result<Loaded, String> {
        self.current(partition, hash)?
            .ok_or_else(|| format!("the game has no {hash:016X}"))
    }

    fn stage_built(&mut self, partition: &str, hash: u64, loaded: Loaded) {
        self.staged
            .entry(partition.to_owned())
            .or_default()
            .insert(hash, Resource::Built(loaded));
    }

    fn stage_unlockables(&mut self, file: &ModFile) -> Result<(), String> {
        let patch = read_json(file)?;
        let current = self.current_game(BASE, json::UNLOCKABLES)?;
        let data = json::merge_unlockables(&current.data, patch)?;
        self.stage_built(BASE, json::UNLOCKABLES, current.with_data(data));
        Ok(())
    }

    fn stage_repository(&mut self, file: &ModFile) -> Result<(), String> {
        let patch = read_json(file)?;
        let current = self.current_game(BASE, json::REPOSITORY)?;
        let (data, references) = json::merge_repository(&current.data, patch)?;
        let mut repository = current.with_data(data);
        for reference in references {
            repository
                .header
                .reference(reference, json::LOOKUP_REFERENCE);
        }
        self.stage_built(BASE, json::REPOSITORY, repository);
        Ok(())
    }

    /// Adds a contract (or replaces one of the game's), listing it in the
    /// contracts ORES under a hash of its id.
    fn stage_contract(&mut self, file: &ModFile) -> Result<(), String> {
        let contract = read_json(file)?;
        let id = contract
            .pointer("/Metadata/Id")
            .and_then(Value::as_str)
            .ok_or("the contract has no Metadata.Id")?
            .to_owned();
        let contracts = self.current_game(BASE, json::CONTRACTS)?;
        let Ores::Hashes(mut entries) = Ores::parse(&contracts.data)? else {
            return Err("the contracts ORES isn't a list of hashes".to_owned());
        };
        let hash = match entries.iter().find(|(_, path)| *path == id) {
            Some((hash, _)) => *hash,
            None => {
                let hash = u64::from(json::contract_hash(&id));
                entries.push((hash, id.clone()));
                let mut contracts = contracts.with_data(Ores::Hashes(entries).serialize());
                contracts
                    .header
                    .reference(RuntimeResourceID::from(hash), json::LOOKUP_REFERENCE);
                self.stage_built(BASE, json::CONTRACTS, contracts);
                hash
            }
        };
        if let Some(destination) = Destination::of_contract(&contract, &id) {
            self.destinations.push(destination);
        }
        let data = json::to_bytes(&contract);
        self.server_side.contracts.insert(id, contract);
        let loaded = match self.current(BASE, hash)? {
            Some(existing) => existing.with_data(data),
            None => Loaded {
                resource_type: "JSON".to_owned(),
                header: Header::default_for(data.len() as u32),
                data,
            },
        };
        self.stage_built(BASE, hash, loaded);
        Ok(())
    }

    fn stage_json_patch(&mut self, file: &ModFile, partition: &str) -> Result<(), String> {
        let patch = read_json(file)?;
        let target = patch
            .get("file")
            .and_then(Value::as_str)
            .ok_or("the patch has no `file`")?;
        let hash = u64::from(meta::parse_hash(target));
        let resource_type = patch.get("type").and_then(Value::as_str).unwrap_or("JSON");
        let operations = patch
            .get("patch")
            .cloned()
            .ok_or("the patch has no `patch`")?;
        let current = self
            .current(partition, hash)?
            .ok_or_else(|| format!("the game has no {target} to patch"))?;
        let data = json::apply_patch(resource_type, &current.data, operations)?;
        self.note_patched_contract(hash, &data)?;
        self.stage_built(partition, hash, current.with_data(data));
        Ok(())
    }

    /// Peacock serves contracts itself, so it has to hear about patches to
    /// them too.
    fn note_patched_contract(&mut self, hash: u64, data: &[u8]) -> Result<(), String> {
        let Some(contracts) = self.current(BASE, json::CONTRACTS)? else {
            return Ok(());
        };
        let Ores::Hashes(entries) = Ores::parse(&contracts.data)? else {
            return Ok(());
        };
        if entries.iter().any(|(entry, _)| *entry == hash) {
            let contract = json::parse(data)?;
            if let Some(id) = contract.pointer("/Metadata/Id").and_then(Value::as_str) {
                let id = id.to_owned();
                self.server_side.contracts.insert(id, contract);
            }
        }
        Ok(())
    }

    fn stage_entity(&mut self, file: &ModFile, partition: &str) -> Result<(), String> {
        let data = fs::read(&file.src).map_err(|e| e.to_string())?;
        let mut key = cache::Key::new("entity");
        key.field(&data);
        let built = self.cached(&key, &format!("entity {}", file.rel), || {
            let entity = entity::parse_entity(&data)?;
            Ok(entity::generate(&entity)?.into_vec())
        })?;
        for (hash, loaded) in built {
            self.stage_built(partition, hash, loaded);
        }
        Ok(())
    }

    /// What `make` makes, or what it made last time from the same `key`
    /// if that's in the cache. `step` is the progress shown while making.
    fn cached(
        &self,
        key: &cache::Key,
        step: &str,
        make: impl FnOnce() -> Result<Vec<(u64, Loaded)>, String>,
    ) -> Result<Vec<(u64, Loaded)>, String> {
        if let Some(built) = cache::get(self.ctx.cache, key) {
            return Ok(built);
        }
        self.ctx.reporter.report(Event::Building {
            step: step.to_owned(),
        });
        let built = make()?;
        let kept: Vec<_> = built.iter().map(|(h, l)| (*h, l)).collect();
        cache::put(self.ctx.cache, key, &kept);
        Ok(built)
    }

    fn queue_entity_patch(&mut self, file: &ModFile, partition: &str) -> Result<(), String> {
        let data = fs::read(&file.src).map_err(|e| e.to_string())?;
        let patch = entity::parse_patch(&data)?;
        let source = data;
        self.entity_patches.push(EntityPatch {
            partition: partition.to_owned(),
            factory: u64::from(meta::parse_hash(&patch.factory_hash)),
            blueprint: u64::from(meta::parse_hash(&patch.blueprint_hash)),
            file: file.rel.clone(),
            patch,
            source,
        });
        Ok(())
    }

    /// Applies the current mod's entity patches, in order, to the current
    /// version of each entity. Converting an entity is slow, so each is
    /// converted once for all its patches. Entities are done one at a time:
    /// a level's can take gigabytes of memory to convert, and QuickEntity
    /// already uses every core for each.
    fn apply_entity_patches(&mut self, label: &str) -> Result<(), String> {
        let mut groups: Vec<Vec<EntityPatch>> = Vec::new();
        for patch in std::mem::take(&mut self.entity_patches) {
            let group = groups.iter_mut().find(|g| {
                let first = &g[0];
                (&first.partition, first.factory, first.blueprint)
                    == (&patch.partition, patch.factory, patch.blueprint)
            });
            match group {
                Some(group) => group.push(patch),
                None => groups.push(vec![patch]),
            }
        }
        if groups.is_empty() {
            return Ok(());
        }

        let total = groups.len();
        for (i, group) in groups.into_iter().enumerate() {
            let first = &group[0];
            let (partition, factory_hash, blueprint_hash) =
                (first.partition.clone(), first.factory, first.blueprint);
            let factory = self.current(&partition, factory_hash)?;
            let blueprint = self.current(&partition, blueprint_hash)?;
            let (Some(factory), Some(blueprint)) = (&factory, &blueprint) else {
                let (missing, hash) = match factory {
                    None => ("TEMP", factory_hash),
                    Some(_) => ("TBLU", blueprint_hash),
                };
                self.errors.push(format!(
                    "{label}: {}: the game has no {missing} {hash:016X} to patch",
                    group[0].file
                ));
                continue;
            };
            let mut key = cache::Key::new("entity patch");
            key.resource(factory_hash, factory)
                .resource(blueprint_hash, blueprint);
            for patch in &group {
                key.field(&patch.source);
            }
            let step = format!("entity {factory_hash:016X} ({}/{total})", i + 1);
            let built = self.cached(&key, &step, || {
                patch_entity(group, factory, blueprint).map(entity::Resources::into_vec)
            });
            match built {
                Ok(built) => {
                    for (hash, loaded) in built {
                        self.stage_built(&partition, hash, loaded);
                    }
                }
                Err(e) => self.errors.push(format!("{label}: {e}")),
            }
        }
        Ok(())
    }

    fn queue_localisation(&mut self, label: &str, data: &manifest::ModData) {
        let strings = match data.localisation() {
            Ok(strings) => strings,
            Err(e) => {
                self.errors.push(format!("{label}: {e}"));
                return;
            }
        };
        for s in strings {
            let Some(language) = locr::language(&s.language) else {
                self.errors.push(format!(
                    "{label}: `localisation` has strings for `{}`, which isn't a language the game has",
                    s.language
                ));
                continue;
            };
            self.queue_string(StringOverride {
                label: label.to_owned(),
                feature: LOCALISATION,
                locr: locr::MOD_STRINGS,
                language,
                hash: locr::name_hash(&s.key),
                text: s.text,
            });
        }
    }

    /// Queues a change to a string. Like SMF, English strings are the
    /// fallback language's too.
    fn queue_string(&mut self, string: StringOverride) {
        if string.language == locr::ENGLISH {
            self.string_overrides.push(StringOverride {
                label: string.label.clone(),
                feature: string.feature,
                locr: string.locr,
                language: locr::FALLBACK,
                hash: string.hash,
                text: string.text.clone(),
            });
        }
        self.string_overrides.push(string);
    }

    /// Stages `localisedLines`: each a LINE resource naming a string. Like
    /// SMF's, they go in chunk0 and don't say what memory they need.
    fn stage_lines(&mut self, label: &str, data: &manifest::ModData) {
        let lines = match data.localised_lines() {
            Ok(lines) => lines,
            Err(e) => {
                self.errors.push(format!("{label}: {e}"));
                return;
            }
        };
        for (line, name) in lines {
            let data = locr::line(&name);
            let header = Header {
                references: vec![(RuntimeResourceID::from(locr::MOD_STRINGS), LINE_REFERENCE)],
                compressed: false,
                system_memory: u32::MAX,
                ..Header::default_for(data.len() as u32)
            };
            self.stage_built(
                BASE,
                u64::from(meta::parse_hash(&line)),
                Loaded {
                    resource_type: "LINE".to_owned(),
                    header,
                    data,
                },
            );
        }
    }

    fn queue_string_overrides(&mut self, label: &str, data: &manifest::ModData) {
        let overrides = match data.localisation_overrides() {
            Ok(overrides) => overrides,
            Err(e) => {
                self.errors.push(format!("{label}: {e}"));
                return;
            }
        };
        for o in overrides {
            let Some(language) = locr::language(&o.language) else {
                self.errors.push(format!(
                    "{label}: `localisationOverrides` has strings for `{}`, which isn't a language the game has",
                    o.language
                ));
                continue;
            };
            self.queue_string(StringOverride {
                label: label.to_owned(),
                feature: LOCALISATION_OVERRIDES,
                locr: u64::from(meta::parse_hash(&o.locr)),
                language,
                hash: locr::string_hash(&o.key),
                text: o.text,
            });
        }
    }

    /// Applies every mod's changes to localised strings, in load order, to
    /// the current version of each LOCR. Each is staged in the partition
    /// its current version is in.
    fn apply_string_overrides(&mut self) -> Result<(), String> {
        let mut strings = std::mem::take(&mut self.string_overrides);
        strings.sort_by_key(|o| o.feature == LOCALISATION_OVERRIDES);
        let mut by_locr: Vec<(u64, Vec<StringOverride>)> = Vec::new();
        for o in strings {
            match by_locr.iter_mut().find(|(hash, _)| *hash == o.locr) {
                Some((_, group)) => group.push(o),
                None => by_locr.push((o.locr, vec![o])),
            }
        }
        for (hash, overrides) in by_locr {
            let Some(partition) = self.partition_of(hash)? else {
                for (label, feature) in sources(&overrides) {
                    self.errors.push(format!(
                        "{label}: `{feature}`: the game has no LOCR {hash:016X}"
                    ));
                }
                continue;
            };
            let current = self.current_game(&partition, hash)?;
            let mut strings = match locr::Locr::parse(&current.data) {
                Ok(strings) => strings,
                Err(e) => {
                    for (label, feature) in sources(&overrides) {
                        self.errors.push(format!(
                            "{label}: `{feature}`: couldn't read LOCR {hash:016X}: {e}"
                        ));
                    }
                    continue;
                }
            };
            for o in &overrides {
                if let Err(e) = strings.set(o.language, o.hash, &o.text) {
                    self.errors.push(format!(
                        "{}: `{}`: string {:08X}: {e}",
                        o.label, o.feature, o.hash
                    ));
                }
            }
            self.stage_built(&partition, hash, current.with_data(strings.serialize()));
        }
        Ok(())
    }

    /// Which partition the current version of a resource is in: one it's
    /// been staged in, otherwise the one the game loads it from.
    fn partition_of(&mut self, hash: u64) -> Result<Option<String>, String> {
        if let Some((partition, _)) = self
            .staged
            .iter()
            .find(|(_, resources)| resources.contains_key(&hash))
        {
            return Ok(Some(partition.clone()));
        }
        Ok(self
            .game()?
            .latest(RuntimeResourceID::from(hash))
            .map(|(archive, _)| archive.name.partition.to_string()))
    }

    /// Rebuilds a dialogue resource (a DLGE, DITL or CLNG) from TonyTools
    /// JSON, keeping the header of the version it replaces if there is one.
    fn stage_dialogue(
        &mut self,
        file: &ModFile,
        partition: &str,
        resource_type: &str,
    ) -> Result<(), String> {
        let text = fs::read_to_string(&file.src).map_err(|e| e.to_string())?;
        let built = match resource_type {
            "DITL" => dlge::build_ditl(&text)?,
            "CLNG" => dlge::build_clng(&text)?,
            _ => dlge::build(&text)?,
        };
        self.stage_rebuilt(partition, resource_type, built)
    }

    /// Stages a resource rebuilt from JSON, keeping the header of the
    /// version it replaces if there is one.
    fn stage_rebuilt(
        &mut self,
        partition: &str,
        resource_type: &str,
        built: dlge::Built,
    ) -> Result<(), String> {
        let hash = u64::from(built.hash);
        let mut loaded = match self.current(partition, hash)? {
            Some(existing) => existing.with_data(built.data),
            // Like the game's own, which don't say what memory they need.
            None => Loaded {
                resource_type: resource_type.to_owned(),
                header: Header {
                    system_memory: u32::MAX,
                    ..Header::default_for(built.data.len() as u32)
                },
                data: built.data,
            },
        };
        loaded.header.references = built.references;
        self.stage_built(partition, hash, loaded);
        Ok(())
    }

    /// Builds a whole LOCR from TonyTools JSON, replacing the current
    /// version (whose first byte it keeps, as the game's differ from
    /// TonyTools').
    fn stage_locr(&mut self, file: &ModFile, partition: &str) -> Result<(), String> {
        let text = fs::read_to_string(&file.src).map_err(|e| e.to_string())?;
        let (hash, mut strings) = locr::Locr::from_json(&text)?;
        if let Some(existing) = self.current(partition, u64::from(hash))? {
            strings.keep_version(&locr::Locr::parse(&existing.data)?);
        }
        let built = dlge::Built {
            hash,
            data: strings.serialize(),
            references: Vec::new(),
        };
        self.stage_rebuilt(partition, "LOCR", built)
    }

    /// Rebuilds a material's MATI, MATT and MATB from RPKG Tool JSON. Like
    /// SMF, they get the meta RPKG Tool writes for them, which doesn't say
    /// what memory they need.
    fn stage_material(&mut self, file: &ModFile, partition: &str) -> Result<(), String> {
        let text = fs::read_to_string(&file.src).map_err(|e| e.to_string())?;
        for (built, resource_type) in material::build(&text)? {
            let header = Header {
                references: built.references,
                system_memory: u32::MAX,
                ..Header::default_for(built.data.len() as u32)
            };
            self.stage_built(
                partition,
                u64::from(built.hash),
                Loaded {
                    resource_type: resource_type.to_owned(),
                    header,
                    data: built.data,
                },
            );
        }
        Ok(())
    }

    /// Converts a TGA into a TEXT, and a TEXD if its name gives one.
    /// `rel` is its path relative to the manifest, for finding its meta.
    fn stage_texture(
        &mut self,
        files: &ModFiles<'_>,
        rel: &str,
        file: &ModFile,
        stem: &str,
        partition: &str,
    ) -> Result<(), String> {
        let (text, texd) = texture::parse_name(stem)
            .ok_or("textures have to be named <TEXT hash> or <TEXT hash>~<TEXD hash>")?;
        let meta = files
            .get(&format!("{rel}.meta"))
            .ok_or("the texture has no .meta from HMTextureTools next to it")?;
        let meta = fs::read(&meta.src).map_err(|e| format!("{}: {e}", meta.rel))?;
        let tga = fs::read(&file.src).map_err(|e| e.to_string())?;
        let mut key = cache::Key::new("texture");
        key.hash(text)
            .hash(texd.unwrap_or_default())
            .field(&meta)
            .field(&tga);
        let built = self.cached(&key, &format!("texture {text:016X}"), || {
            let texture::Resources { text, texd } = texture::build(&tga, &meta, text, texd)?;
            Ok([Some(text), texd].into_iter().flatten().collect())
        })?;
        for (hash, loaded) in built {
            self.stage_built(partition, hash, loaded);
        }
        Ok(())
    }

    /// Adds files to the game's online resources, listing each in the blobs
    /// ORES by its path in the folder.
    fn stage_blobs(&mut self, files: &ModFiles<'_>, folder: &str) -> Result<(), String> {
        let blobs: Vec<_> = files.under(folder).collect();
        if blobs.is_empty() {
            return Ok(());
        }
        let ores = self.current_game(BASE, json::BLOBS)?;
        let Ores::Hashes(mut entries) = Ores::parse(&ores.data)? else {
            return Err("the blobs ORES isn't a list of hashes".to_owned());
        };
        let mut references = Vec::new();
        for (rest, file) in blobs {
            let path = rest.to_lowercase();
            let (rrid, resource_type) = json::blob(&path);
            let hash = u64::from(rrid);
            match entries.iter_mut().find(|(h, _)| *h == hash) {
                Some(entry) => entry.1 = path,
                None => entries.push((hash, path)),
            }
            references.push(rrid);
            self.staged.entry(BASE.to_owned()).or_default().insert(
                hash,
                Resource::File {
                    path: file.src.clone(),
                    resource_type,
                    meta: None,
                },
            );
        }
        let mut ores = ores.with_data(Ores::Hashes(entries).serialize());
        for rrid in references {
            ores.header.reference(rrid, json::LOOKUP_REFERENCE);
        }
        self.stage_built(BASE, json::BLOBS, ores);
        Ok(())
    }

    fn stage_destinations(&mut self) -> Result<(), String> {
        let current = self.current_game(BASE, json::DESTINATIONS)?;
        let mut menu = json::parse(&current.data)?;
        json::add_destinations(&mut menu, &self.destinations)?;
        self.stage_built(
            BASE,
            json::DESTINATIONS,
            current.with_data(json::to_bytes(&menu)),
        );
        Ok(())
    }

    fn check_requirements(&mut self) {
        for (label, requirements, incompatibilities) in &self.checks {
            for id in requirements {
                if !self.ids.contains(id) {
                    self.warnings
                        .push(format!("{label} needs `{id}`, which isn't being deployed"));
                }
            }
            for id in incompatibilities {
                if self.ids.contains(id) {
                    self.warnings.push(format!(
                        "{label} doesn't work with `{id}`, which is being deployed"
                    ));
                }
            }
        }
    }

    /// Writes the deploy summary for Peacock, if any framework mods are being
    /// deployed, and returns it with the plugins to place.
    fn write_server_side(&mut self) -> Result<Vec<Placement>, String> {
        if self.ids.is_empty() {
            return Ok(Vec::new());
        }
        // Mods' unlockables are what Peacock serves instead of its own, so
        // it only gets them if one changed them.
        let changed = self
            .staged
            .get(BASE)
            .is_some_and(|s| s.contains_key(&json::UNLOCKABLES));
        let unlockables = if changed {
            let current = self.current_game(BASE, json::UNLOCKABLES)?;
            let Ores::Json(text) = Ores::parse(&current.data)? else {
                return Err("the unlockables ORES isn't JSON".to_owned());
            };
            Some(json::parse(text.as_bytes())?)
        } else {
            None
        };
        let root = self.ctx.game.root();
        let summary = peacock::summary(root, &self.ids, &self.server_side, unlockables);
        let out = self.output(peacock::SUMMARY)?;
        fs::write(&out, summary).map_err(|e| format!("{}: {e}", out.display()))?;
        if !self.server_side.plugins.is_empty() {
            self.warnings.push(peacock::setup_warning(root));
        }
        let mut placements = std::mem::take(&mut self.plugin_placements);
        placements.push(placement(peacock::SUMMARY));
        Ok(placements)
    }

    fn write_package_definition(&mut self) -> Result<Placement, String> {
        let text = self.read_original(PACKAGE_DEFINITION)?;
        if text::is_patched(&text) {
            self.warnings.push(format!(
                "{PACKAGE_DEFINITION} has already been changed by another mod manager; \
                 if the game misbehaves, verify its files and deploy again"
            ));
        }
        let patched = text::patch_package_definition(&text, &self.package_definition)?;
        self.write_encrypted(PACKAGE_DEFINITION, patched)
    }

    fn write_thumbs(&mut self) -> Result<Placement, String> {
        let text = self.read_original(THUMBS)?;
        let patched = text::patch_thumbs(&text, &self.thumbs)?;
        self.write_encrypted(THUMBS, patched)
    }

    fn read_original(&self, file: &str) -> Result<String, String> {
        let rel = RelPath::new(file).unwrap();
        let path = self.ctx.game.original(&rel).ok_or_else(|| {
            format!("the game has no {file}; is its folder set to the right place?")
        })?;
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        text::decrypt(&bytes).map_err(|e| format!("couldn't decrypt {file}: {e}"))
    }

    fn write_encrypted(&self, file: &str, text: String) -> Result<Placement, String> {
        let out = self.output(file)?;
        let bytes = text::encrypt(text).map_err(|e| format!("couldn't encrypt {file}: {e}"))?;
        fs::write(&out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
        Ok(placement(file))
    }

    fn output(&self, rel: &str) -> Result<PathBuf, String> {
        let path = RelPath::new(rel).unwrap().under(self.ctx.out);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        Ok(path)
    }
}

/// Places a file the build wrote to `rel` in its output at the same path in
/// the game.
fn placement(rel: &str) -> Placement {
    let rel = RelPath::new(rel).unwrap();
    Placement {
        dst: Dest::game(rel.clone()),
        src: Src::Built(rel),
        mod_name: BUILT_BY.to_owned(),
    }
}

/// A framework mod's files, relative to the folder its manifest is in.
struct ModFiles<'a> {
    files: Vec<(String, &'a ModFile)>,
    /// Lowercased relative path to file, for finding meta files.
    by_path: HashMap<String, &'a ModFile>,
}

impl<'a> ModFiles<'a> {
    fn new(m: &'a InstalledMod, root: Option<&RelPath>) -> Self {
        let prefix = root.map(|r| format!("{r}/"));
        let files: Vec<_> = m
            .files
            .iter()
            .filter_map(|f| {
                let rel = f.rel.as_str();
                let rest = match &prefix {
                    Some(p) => rel.strip_prefix(p.as_str())?,
                    None => rel,
                };
                Some((rest.to_owned(), f))
            })
            .collect();
        let by_path = files
            .iter()
            .map(|(rest, f)| (rest.to_ascii_lowercase(), *f))
            .collect();
        Self { files, by_path }
    }

    /// Files under `folder` (matched ignoring case, as on Windows), with
    /// their paths relative to it.
    fn under<'s>(&'s self, folder: &str) -> impl Iterator<Item = (&'s str, &'a ModFile)> + 's {
        let prefix = format!("{}/", folder.trim_matches('/').to_ascii_lowercase());
        self.files.iter().filter_map(move |(rest, f)| {
            let lower = rest.to_ascii_lowercase();
            lower
                .starts_with(&prefix)
                .then(|| (&rest[prefix.len()..], *f))
        })
    }

    fn get(&self, rel: &str) -> Option<&'a ModFile> {
        self.by_path.get(&rel.to_ascii_lowercase()).copied()
    }
}

/// The meta file next to a raw resource, if it has one. Like RPKG Tool, a
/// binary `.meta` wins over `.meta.json`.
/// `rel` is the resource's path relative to the manifest.
fn read_meta(files: &ModFiles<'_>, rel: &str) -> Result<Option<Meta>, String> {
    if let Some(meta) = files.get(&format!("{rel}.meta")) {
        let bytes = fs::read(&meta.src).map_err(|e| format!("{}: {e}", meta.rel))?;
        return Meta::from_binary(&bytes)
            .map(Some)
            .map_err(|e| format!("{}: {e}", meta.rel));
    }
    if let Some(meta) = files.get(&format!("{rel}.meta.json")) {
        let text = fs::read_to_string(&meta.src).map_err(|e| format!("{}: {e}", meta.rel))?;
        return Meta::from_json(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", meta.rel));
    }
    Ok(None)
}

/// Which partition a plain RPKG mod's archive goes in: the `chunkN` folder
/// it's in if any (SMF's layout for RPKG mods), otherwise its own name
/// (`chunk0patch3.rpkg`), otherwise chunk0.
fn archive_partition(rel: &RelPath) -> String {
    let mut dirs: Vec<&str> = rel.components().collect();
    dirs.pop();
    let from_dir = dirs.iter().rev().find_map(|dir| {
        let id = PartitionId::from_str(&dir.to_ascii_lowercase()).ok()?;
        (!dir.to_ascii_lowercase().contains("patch")).then(|| id.to_string())
    });
    from_dir
        .or_else(|| {
            ArchiveName::parse(&rel.file_name().to_ascii_lowercase())
                .map(|n| n.partition.to_string())
        })
        .unwrap_or_else(|| "chunk0".to_owned())
}

/// Applies a group of patches to the same entity, given its resources.
fn patch_entity(
    patches: Vec<EntityPatch>,
    factory: &Loaded,
    blueprint: &Loaded,
) -> Result<entity::Resources, String> {
    let first = &patches[0];
    let (factory_hash, blueprint_hash, first_file) =
        (first.factory, first.blueprint, first.file.clone());
    let mut entity = entity::decode((factory_hash, factory), (blueprint_hash, blueprint))
        .map_err(|e| format!("{first_file}: couldn't read entity {factory_hash:016X}: {e}"))?;
    for EntityPatch { file, patch, .. } in patches {
        entity::apply(&mut entity, patch).map_err(|e| format!("{file}: {e}"))?;
    }
    entity::generate(&entity)
        .map_err(|e| format!("{first_file}: couldn't rebuild entity {factory_hash:016X}: {e}"))
}

/// The mods and manifest keys a group of string changes come from, each
/// once.
fn sources(overrides: &[StringOverride]) -> Vec<(&str, &'static str)> {
    let mut seen = HashSet::new();
    overrides
        .iter()
        .map(|o| (o.label.as_str(), o.feature))
        .filter(|source| seen.insert(*source))
        .collect()
}

/// Removes repeats, keeping the first of each.
fn dedupe(items: &mut Vec<String>) {
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.clone()));
}

fn has_extension(name: &str, extension: &str) -> bool {
    name.rsplit_once('.')
        .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case(extension))
}

fn read_json(file: &ModFile) -> Result<Value, String> {
    let data = fs::read(&file.src).map_err(|e| e.to_string())?;
    json::parse(&data)
}

#[cfg(test)]
mod tests;
