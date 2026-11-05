//! Entities, which the game stores as a pair of resources: a TEMP (the
//! factory: sub-entities' properties) and a TBLU (the blueprint: their names,
//! hierarchy and connections). Mods give them as QuickEntity JSON, either a
//! whole entity (`.entity.json`) or a patch to one of the game's
//! (`.entity.patch.json`).
//!
//! Like SMF, QuickEntity does the converting and patching. On the way
//! between it and the game's binary form, each resource goes through the
//! JSON form ResourceLib uses, which both glacier-bin1's and QuickEntity's
//! types read and write.

use glacier_bin1::game::h3::{STemplateEntityBlueprint, STemplateEntityFactory};
use hitman_commons::resourcelib::{EntityBlueprint, EntityFactory};
use hitman_commons::rpkg_tool::RpkgResourceMeta;
use quickentity_rs::patch_structs::Patch;
use quickentity_rs::qn_structs::Entity;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::json;
use super::meta::Meta;
use super::resource::{Header, Loaded};

/// The QuickEntity version entities have to be in; SMF upgrades older ones
/// with older versions of QuickEntity, which patchwork doesn't have.
const ENTITY_VERSION: f64 = 3.1;
/// Likewise for patches.
const PATCH_VERSION: u64 = 6;

pub fn parse_entity(data: &[u8]) -> Result<Entity, String> {
    let value = json::parse(data)?;
    match value.get("quickEntityVersion").and_then(Value::as_f64) {
        Some(version) if version == ENTITY_VERSION => {}
        Some(version) => {
            return Err(format!(
                "it's for QuickEntity {version}, and patchwork only supports {ENTITY_VERSION}"
            ));
        }
        None => return Err("it has no quickEntityVersion".to_owned()),
    }
    from_value(value)
}

pub fn parse_patch(data: &[u8]) -> Result<Patch, String> {
    let value = json::parse(data)?;
    match value.get("patchVersion").and_then(Value::as_u64) {
        Some(PATCH_VERSION) => {}
        Some(version) => {
            return Err(format!(
                "it's patch version {version}, and patchwork only supports {PATCH_VERSION}"
            ));
        }
        None => return Err("it has no patchVersion".to_owned()),
    }
    from_value(value)
}

/// An entity's two resources, by runtime ID.
pub struct Resources {
    pub factory: (u64, Loaded),
    pub blueprint: (u64, Loaded),
}

impl Resources {
    pub fn into_vec(self) -> Vec<(u64, Loaded)> {
        vec![self.factory, self.blueprint]
    }
}

/// Builds the game's resources for an entity.
pub fn generate(entity: &Entity) -> Result<Resources, String> {
    let (factory, factory_meta, blueprint, blueprint_meta) =
        quickentity_rs::convert_to_rt(entity).map_err(|e| format!("{e:#}"))?;
    let factory: STemplateEntityFactory = convert(&factory)?;
    let blueprint: STemplateEntityBlueprint = convert(&blueprint)?;
    let factory =
        glacier_bin1::serialize(&factory).map_err(|e| format!("couldn't write the TEMP: {e}"))?;
    let blueprint =
        glacier_bin1::serialize(&blueprint).map_err(|e| format!("couldn't write the TBLU: {e}"))?;
    Ok(Resources {
        factory: loaded(&factory_meta, factory)?,
        blueprint: loaded(&blueprint_meta, blueprint)?,
    })
}

/// Reads an entity from the game's resources for it.
pub fn decode(factory: (u64, &Loaded), blueprint: (u64, &Loaded)) -> Result<Entity, String> {
    let factory_meta = rpkg_meta(factory.0, factory.1)?;
    let blueprint_meta = rpkg_meta(blueprint.0, blueprint.1)?;
    let factory: STemplateEntityFactory = glacier_bin1::deserialize(&factory.1.data)
        .map_err(|e| format!("couldn't read the TEMP: {e}"))?;
    let blueprint: STemplateEntityBlueprint = glacier_bin1::deserialize(&blueprint.1.data)
        .map_err(|e| format!("couldn't read the TBLU: {e}"))?;
    let factory: EntityFactory = convert(&factory)?;
    let blueprint: EntityBlueprint = convert(&blueprint)?;
    // Lossless, as SMF converts entities to patch them, so nothing a patch
    // doesn't touch changes.
    quickentity_rs::convert_to_qn(&factory, &factory_meta, &blueprint, &blueprint_meta, true)
        .map_err(|e| format!("{e:#}"))
}

/// Applies a patch the way SMF does: permissively, so operations on things
/// that aren't there are skipped rather than failing.
pub fn apply(entity: &mut Entity, patch: Patch) -> Result<(), String> {
    quickentity_rs::apply_patch(entity, patch, true).map_err(|e| format!("{e:#}"))
}

/// Converts between types that share a JSON form.
fn convert<T: Serialize, U: DeserializeOwned>(value: &T) -> Result<U, String> {
    let value = serde_json::to_value(value).map_err(|e| e.to_string())?;
    from_value(value)
}

fn from_value<T: DeserializeOwned>(value: Value) -> Result<T, String> {
    serde_path_to_error::deserialize(value).map_err(|e| match e.path().to_string().as_str() {
        "." => e.inner().to_string(),
        path => format!("{path}: {}", e.inner()),
    })
}

/// A resource with the header QuickEntity's meta for it gives, which is
/// what SMF packs.
fn loaded(meta: &RpkgResourceMeta, data: Vec<u8>) -> Result<(u64, Loaded), String> {
    let hash = meta.hash_value.as_u64();
    let text = serde_json::to_string(meta).map_err(|e| e.to_string())?;
    let meta = Meta::from_json(&text)?;
    let header = meta.header(data.len() as u32);
    Ok((
        hash,
        Loaded {
            resource_type: meta.resource_type,
            data,
            header,
        },
    ))
}

/// What QuickEntity needs to know of a resource's header: its references,
/// since the entity refers to them by index.
fn rpkg_meta(hash: u64, loaded: &Loaded) -> Result<RpkgResourceMeta, String> {
    let Header { references, .. } = &loaded.header;
    let references: Vec<_> = references
        .iter()
        .map(|(rrid, flag)| {
            json!({ "hash": format!("{:016X}", u64::from(*rrid)), "flag": format!("{flag:02X}") })
        })
        .collect();
    from_value(json!({
        "hash_value": format!("{hash:016X}"),
        "hash_offset": 0,
        "hash_size": 0,
        "hash_resource_type": loaded.resource_type,
        "hash_reference_table_size": 0,
        "hash_reference_table_dummy": 0,
        "hash_size_final": 0,
        "hash_size_in_memory": 0,
        "hash_size_in_video_memory": 0,
        "hash_reference_data": references,
    }))
}
