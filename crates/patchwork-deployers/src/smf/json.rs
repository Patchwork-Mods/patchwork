//! SMF's JSON content types, which edit the game's JSON-based resources
//! rather than replacing them: the unlockables and repository (merged into),
//! contracts (added), blobs (added) and RFC 6902 patches of any JSON, REPO
//! or ORES resource.

use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;
use serde_json::{Map, Value};

use super::ores::Ores;

pub const UNLOCKABLES: u64 = 0x0057_C2C3_9411_15CA;
pub const REPOSITORY: u64 = 0x0020_4D1A_FD76_AB13;
pub const CONTRACTS: u64 = 0x002B_0702_0D21_D727;
pub const BLOBS: u64 = 0x0085_8D45_F5F9_E3CA;
/// The Destinations menu's list of missions.
pub const DESTINATIONS: u64 = 0x004F_4B73_8474_CEAD;
/// How resources SMF adds to a lookup table are referenced from it.
pub const LOOKUP_REFERENCE: u8 = 0x9F;

/// Merges `src` into `target` like SMF (lodash's `merge`, but replacing
/// arrays): objects are merged key by key, and anything else replaces.
pub fn deep_merge(target: &mut Value, src: Value) {
    match (target, src) {
        (Value::Object(target), Value::Object(src)) => {
            for (key, value) in src {
                match target.get_mut(&key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (target, src) => *target = src,
    }
}

pub fn parse(data: &[u8]) -> Result<Value, String> {
    let text = std::str::from_utf8(data).map_err(|e| e.to_string())?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())
}

pub fn to_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

/// An array of objects as an object keyed by each one's `key` field.
fn keyed(value: Value, key: &str) -> Result<Map<String, Value>, String> {
    let Value::Array(items) = value else {
        return Err("expected an array".to_owned());
    };
    let mut map = Map::new();
    for item in items {
        let id = match item.get(key) {
            Some(Value::String(id)) => id.clone(),
            Some(other) => other.to_string(),
            None => return Err(format!("an entry has no `{key}`")),
        };
        map.insert(id, item);
    }
    Ok(map)
}

/// Back to an array, giving entries added by key their `key` field.
fn unkeyed(map: Map<String, Value>, key: &str) -> Value {
    Value::Array(
        map.into_iter()
            .map(|(id, mut item)| {
                if let Value::Object(fields) = &mut item {
                    let missing = match fields.get(key) {
                        None | Some(Value::Null) => true,
                        Some(Value::String(s)) => s.is_empty(),
                        Some(Value::Bool(b)) => !b,
                        _ => false,
                    };
                    if missing {
                        fields.insert(key.to_owned(), Value::String(id));
                    }
                }
                item
            })
            .collect(),
    )
}

/// Merges an `unlockables.json` into the unlockables ORES.
pub fn merge_unlockables(ores: &[u8], patch: Value) -> Result<Vec<u8>, String> {
    let Ores::Json(text) = Ores::parse(ores)? else {
        return Err("the unlockables ORES isn't JSON".to_owned());
    };
    let mut items = Value::Object(keyed(parse(text.as_bytes())?, "Id")?);
    deep_merge(&mut items, patch);
    let Value::Object(items) = items else {
        unreachable!()
    };
    let text = serde_json::to_string(&unkeyed(items, "Id")).unwrap();
    Ok(Ores::Json(text).serialize())
}

/// Merges a `repository.json` into the repository, returning it and what
/// the items it edited refer to, which the repository has to reference.
pub fn merge_repository(
    repo: &[u8],
    patch: Value,
) -> Result<(Vec<u8>, Vec<RuntimeResourceID>), String> {
    let edited: Vec<String> = match &patch {
        Value::Object(items) => items.keys().cloned().collect(),
        _ => return Err("expected an object of repository items".to_owned()),
    };
    let mut items = Value::Object(keyed(parse(repo)?, "ID_")?);
    deep_merge(&mut items, patch);
    let Value::Object(items) = items else {
        unreachable!()
    };

    let mut references = Vec::new();
    for id in &edited {
        let Some(item) = items.get(id) else {
            continue;
        };
        match item.get("Runtime") {
            Some(Value::Number(n)) => {
                if let Some(n) = n.as_u64() {
                    references.push(RuntimeResourceID::from(n));
                }
            }
            Some(Value::String(s)) => {
                if let Ok(n) = s.trim().parse::<u64>() {
                    references.push(RuntimeResourceID::from(n));
                }
            }
            _ => {}
        }
        if let Some(Value::String(image)) = item.get("Image") {
            references.push(cloud_resource(image, "gfx"));
        }
    }
    Ok((to_bytes(&unkeyed(items, "ID_")), references))
}

/// A file in the game's online resources, like a repository item's image.
fn cloud_resource(path: &str, kind: &str) -> RuntimeResourceID {
    RuntimeResourceID::from_raw_string(
        &format!("[assembly:/_pro/online/default/cloudstorage/resources/{path}].pc_{kind}")
            .to_lowercase(),
    )
}

/// The runtime ID and resource type of a blob at `path` in a blobs folder.
pub fn blob(path: &str) -> (RuntimeResourceID, String) {
    let extension = path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    let (kind, resource_type) = if extension.starts_with("jp") || extension == "png" {
        ("gfx".to_owned(), "GFXI".to_owned())
    } else if extension == "json" {
        ("json".to_owned(), "JSON".to_owned())
    } else {
        (extension.clone(), extension.to_ascii_uppercase())
    };
    (cloud_resource(path, &kind), resource_type)
}

/// The runtime ID a contract added by a mod is stored under.
pub fn contract_hash(id: &str) -> RuntimeResourceID {
    RuntimeResourceID::from_raw_string(&format!("smfContract{id}").to_lowercase())
}

/// Where a contract's `SMF.destinations` asks for it to be listed.
#[derive(Debug, Clone, PartialEq)]
pub struct Destination {
    pub id: String,
    pub before: Option<String>,
    pub after: Option<String>,
    pub context: Option<String>,
}

impl Destination {
    pub fn of_contract(contract: &Value, id: &str) -> Option<Self> {
        let destinations = contract.get("SMF")?.get("destinations")?;
        if destinations.get("addToDestinations") != Some(&Value::Bool(true)) {
            return None;
        }
        let text = |key: &str| destinations.get(key)?.as_str().map(str::to_owned);
        Some(Self {
            id: id.to_owned(),
            before: text("placeBefore"),
            after: text("placeAfter"),
            context: text("narrativeContext"),
        })
    }
}

/// Lists contracts in the Destinations menu, as SMF does.
pub fn add_destinations(menu: &mut Value, destinations: &[Destination]) -> Result<(), String> {
    let children = menu
        .pointer_mut("/Root/Children")
        .and_then(Value::as_array_mut)
        .ok_or("the destinations menu has no Root.Children")?;
    for d in destinations {
        let entry = serde_json::json!({
            "Id": d.id,
            "_comment": "Automatically added by SMF.",
            "NarrativeContext": d.context.as_deref().unwrap_or("Mission"),
            "Meta": { "Ui": { "Row": 3, "Col": 5 } },
        });
        let position = |id: &String| {
            children
                .iter()
                .position(|c| c.get("Id") == Some(&Value::String(id.clone())))
        };
        let at = match (&d.before, &d.after) {
            (Some(before), _) => position(before),
            (None, Some(after)) => position(after).map(|i| i + 1),
            (None, None) => None,
        };
        match at {
            Some(at) => children.insert(at, entry),
            None => children.push(entry),
        }
    }
    Ok(())
}

/// Applies a `JSON.patch.json`'s RFC 6902 patch to a resource of
/// `resource_type`. Like SMF, the unlockables and repository are patched as
/// objects keyed by id; a hash ORES is patched as an object of hex ids.
pub fn apply_patch(resource_type: &str, data: &[u8], patch: Value) -> Result<Vec<u8>, String> {
    let patch: json_patch::Patch = serde_json::from_value(patch).map_err(|e| e.to_string())?;
    let apply = |value: &mut Value| json_patch::patch(value, &patch).map_err(|e| e.to_string());
    match resource_type {
        "ORES" => match Ores::parse(data)? {
            Ores::Json(text) => {
                let mut value = parse(text.as_bytes())?;
                let keyed_by_id = value.is_array();
                if keyed_by_id {
                    value = Value::Object(keyed(value, "Id")?);
                }
                apply(&mut value)?;
                if let (true, Value::Object(map)) = (keyed_by_id, &value) {
                    value = Value::Array(map.values().cloned().collect());
                }
                Ok(Ores::Json(serde_json::to_string(&value).unwrap()).serialize())
            }
            Ores::Hashes(entries) => {
                let mut value = Value::Object(
                    entries
                        .into_iter()
                        .map(|(hash, path)| (format!("{hash:016X}"), Value::String(path)))
                        .collect(),
                );
                apply(&mut value)?;
                let Value::Object(map) = value else {
                    return Err("the patched ORES isn't an object".to_owned());
                };
                let entries = map
                    .into_iter()
                    .map(|(hash, path)| {
                        let hash = u64::from_str_radix(&hash, 16)
                            .map_err(|_| format!("`{hash}` isn't a runtime ID"))?;
                        let path = path.as_str().ok_or("ORES values must be strings")?;
                        Ok((hash, path.to_owned()))
                    })
                    .collect::<Result<_, String>>()?;
                Ok(Ores::Hashes(entries).serialize())
            }
        },
        "REPO" => {
            let mut value = Value::Object(keyed(parse(data)?, "ID_")?);
            apply(&mut value)?;
            let Value::Object(map) = value else {
                return Err("the patched repository isn't an object".to_owned());
            };
            Ok(to_bytes(&Value::Array(
                map.into_iter().map(|(_, v)| v).collect(),
            )))
        }
        _ => {
            let mut value = parse(data)?;
            apply(&mut value)?;
            Ok(to_bytes(&value))
        }
    }
}
