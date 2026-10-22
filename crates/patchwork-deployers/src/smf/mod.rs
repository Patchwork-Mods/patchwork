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
//! [`texture`]).
//!
//! Not supported yet: SMF's other special file types (materials, sounds,
//! language files and deltas), entities made for QuickEntity versions
//! before 3.1, localisation, scripts and choosing options (those enabled by
//! default are used). Mods that need them fail to deploy rather than half
//! working.

mod entity;
mod game;
mod json;
mod manifest;
mod meta;
mod ores;
mod resource;
mod text;
mod texture;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::UNIX_EPOCH;

use patchwork_core::deploy::{
    BuildContext, DeployContext, Deployer, DesiredState, GameFiles, InstalledMod, ModFile,
    Placement, Src,
};
use patchwork_core::error::IoContext;
use patchwork_core::event::Event;
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
const UNSUPPORTED_TYPES: &[&str] = &[
    "material.json",
    "clng.json",
    "ditl.json",
    "dlge.json",
    "locr.json",
    "rtlv.json",
    "sfx.wem",
    "delta",
];

/// The partition SMF puts edits to the game's lookup tables in.
const BASE: &str = "chunk0";

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
    checks: Vec<(String, Vec<String>, Vec<String>)>,
    /// The current mod's entity patches, applied once its content's staged.
    entity_patches: Vec<EntityPatch>,
}

struct EntityPatch {
    partition: String,
    factory: u64,
    blueprint: u64,
    file: RelPath,
    patch: Patch,
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
            checks: Vec::new(),
            entity_patches: Vec::new(),
        }
    }

    fn run(mut self, mods: &[InstalledMod]) -> Result<DesiredState, String> {
        for m in mods {
            self.ctx.reporter.report(Event::Building {
                step: m.name.clone(),
            });
            self.stage_mod(m)?;
        }
        self.check_requirements();
        if !self.destinations.is_empty()
            && self.errors.is_empty()
            && let Err(e) = self.stage_destinations()
        {
            self.errors.push(format!(
                "couldn't add contracts to the Destinations menu: {e}"
            ));
        }
        if !self.errors.is_empty() {
            return Err(self.errors.join("\n"));
        }
        if self.staged.is_empty() && self.package_definition.is_empty() && self.thumbs.is_empty() {
            return Ok(DesiredState {
                placements: Vec::new(),
                warnings: self.warnings,
            });
        }

        let mut placements = Vec::new();
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
        let mut manifests: Vec<&ModFile> = m
            .files
            .iter()
            .filter(|f| f.rel.file_name().eq_ignore_ascii_case("manifest.json"))
            .collect();
        manifests.sort_by(|a, b| a.rel.cmp(&b.rel));
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

        let (active, option_warnings) = manifest.active();
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
            if !data.peacock_plugins.is_empty() {
                self.warnings.push(format!(
                    "{label} has Peacock plugins, which patchwork doesn't install yet"
                ));
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
                None => name[stem.len() + 1..].to_owned(),
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
    /// anything already staged there.
    fn stage_dependency(&mut self, label: &str, dependency: &Dependency) -> Result<(), String> {
        let Some(rrid) = meta::parse_hex_hash(dependency.runtime_id()) else {
            self.errors.push(format!(
                "{label}: dependency `{}` isn't a runtime ID",
                dependency.runtime_id()
            ));
            return Ok(());
        };
        let found: Vec<_> = self
            .game()?
            .dependencies(rrid, dependency.includes_chunk1())
            .into_iter()
            .map(|(archive, info)| (archive.path.clone(), info.clone()))
            .collect();
        if found.is_empty() {
            self.warnings.push(format!(
                "{label}: the game has nothing to copy for dependency {}",
                dependency.runtime_id()
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
        self.stage_built(partition, hash, current.with_data(data));
        Ok(())
    }

    fn stage_entity(&mut self, file: &ModFile, partition: &str) -> Result<(), String> {
        let data = fs::read(&file.src).map_err(|e| e.to_string())?;
        let entity = entity::parse_entity(&data)?;
        let resources = entity::generate(&entity)?;
        self.stage_entity_resources(partition, resources);
        Ok(())
    }

    fn stage_entity_resources(&mut self, partition: &str, resources: entity::Resources) {
        let entity::Resources { factory, blueprint } = resources;
        self.stage_built(partition, factory.0, factory.1);
        self.stage_built(partition, blueprint.0, blueprint.1);
    }

    fn queue_entity_patch(&mut self, file: &ModFile, partition: &str) -> Result<(), String> {
        let data = fs::read(&file.src).map_err(|e| e.to_string())?;
        let patch = entity::parse_patch(&data)?;
        self.entity_patches.push(EntityPatch {
            partition: partition.to_owned(),
            factory: u64::from(meta::parse_hash(&patch.factory_hash)),
            blueprint: u64::from(meta::parse_hash(&patch.blueprint_hash)),
            file: file.rel.clone(),
            patch,
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
            self.ctx.reporter.report(Event::Building {
                step: format!("entity {factory_hash:016X} ({}/{total})", i + 1),
            });
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
            match patch_entity(group, factory, blueprint) {
                Ok(resources) => self.stage_entity_resources(&partition, resources),
                Err(e) => self.errors.push(format!("{label}: {e}")),
            }
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
        self.ctx.reporter.report(Event::Building {
            step: format!("texture {text:016X}"),
        });
        let texture::Resources { text, texd } = texture::build(&tga, &meta, text, texd)?;
        self.stage_built(partition, text.0, text.1);
        if let Some((hash, loaded)) = texd {
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
        dst: rel.clone(),
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
