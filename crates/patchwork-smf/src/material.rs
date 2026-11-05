//! Materials: a MATI (the material instance: its shader type, flags and a
//! tree of properties such as textures and colours), and optionally a MATT
//! and MATB, the entity type and blueprint that let entities override some
//! of those properties. Mods give all three as RPKG Tool's
//! `.material.json`, which is rebuilt here the way RPKG Tool's
//! `-json_to_material` does, quirks included.
//!
//! A MATI property is a 16-byte record: its four-letter name (reversed), a
//! value or an offset to it, a count and a type. Properties with children
//! (the instance, binders) point at their children's records, and strings
//! and float arrays are written after every record, each padded to 16
//! bytes.

use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;
use serde_json::{Map, Value};

use super::dlge::Built;
use super::meta;

/// `[modules:/zrendermaterialentity.class].pc_entitytype`, the MATT's type.
const MATT_CPPT: u64 = 0x00B4B11DA327CAD0;
/// `[modules:/zrendermaterialentity.class].pc_entityblueprint`.
const MATB_CBLU: u64 = 0x00A1595C0918E2C9;
/// The flags RPKG Tool gives references that have to be loaded first, and
/// ones that are only installed.
const FLAG_INSTALL: u8 = 0x1F;
const FLAG_LOAD: u8 = 0x5F;

const TYPE_FLOAT: u32 = 0;
const TYPE_STRING: u32 = 1;
const TYPE_INT: u32 = 2;
const TYPE_PROPERTY: u32 = 3;

/// Property names as the JSON gives them, their tags and their types.
const PROPERTIES: &[(&str, &str, u32)] = &[
    ("Alpha Reference", "AREF", TYPE_INT),
    ("Alpha Test Enabled", "ATST", TYPE_INT),
    ("Blend Enabled", "BENA", TYPE_INT),
    ("Binder", "BIND", TYPE_PROPERTY),
    ("Blend Mode", "BMOD", TYPE_STRING),
    ("Color", "COLO", TYPE_PROPERTY),
    ("Culling Mode", "CULL", TYPE_STRING),
    ("Decal Blend Diffuse", "DBDE", TYPE_INT),
    ("Decal Blend Emission", "DBEE", TYPE_INT),
    ("Decal Blend Normal", "DBNE", TYPE_INT),
    ("Decal Blend Roughness", "DBRE", TYPE_INT),
    ("Decal Blend Specular", "DBSE", TYPE_INT),
    ("Enabled", "ENAB", TYPE_INT),
    ("Fog Enabled", "FENA", TYPE_INT),
    ("Float Value", "FLTV", TYPE_PROPERTY),
    ("Instance", "INST", TYPE_PROPERTY),
    ("Name", "NAME", TYPE_STRING),
    ("Opacity", "OPAC", TYPE_FLOAT),
    ("Render State", "RSTA", TYPE_PROPERTY),
    ("Subsurface Value", "SSBW", TYPE_FLOAT),
    ("Subsurface Blue", "SSVB", TYPE_FLOAT),
    ("Subsurface Green", "SSVG", TYPE_FLOAT),
    ("Subsurface Red", "SSVR", TYPE_FLOAT),
    ("Tags", "TAGS", TYPE_STRING),
    ("Texture", "TEXT", TYPE_PROPERTY),
    ("Tiling U", "TILU", TYPE_STRING),
    ("Tiling V", "TILV", TYPE_STRING),
    ("Texture Id", "TXID", TYPE_INT),
    ("Type", "TYPE", TYPE_STRING),
    ("Value", "VALU", TYPE_FLOAT),
    ("Z Bias", "ZBIA", TYPE_INT),
    ("Z Offset", "ZOFF", TYPE_FLOAT),
];

/// Class flags by bit. Later names are older spellings.
const CLASS_FLAGS: &[(u32, &[&str])] = &[
    (0x1, &["REFLECTION2D"]),
    (0x2, &["REFRACTION2D"]),
    (0x4, &["LIGHTING"]),
    (0x8, &["EMISSIVE"]),
    (0x10, &["DISCARD"]),
    (0x20, &["LM_SKIN"]),
    (0x40, &["PRIMCLASS_STANDARD"]),
    (0x80, &["PRIMCLASS_LINKED"]),
    (0x100, &["PRIMCLASS_WEIGHTED"]),
    (0x200, &["DOFOVERRIDE"]),
    (0x400, &["USES_DEFAULT_VS"]),
    (0x800, &["USES_SPRITE_SA_VS"]),
    (0x1000, &["USES_SPRITE_AO_VS"]),
    (0x2000, &["ALPHA"]),
    (0x4000, &["USES_SIMPLE_SHADER"]),
    (0x8000, &["DISABLE_INSTANCING"]),
    (0x10000, &["LM_HAIR"]),
    (0x20000, &["SAMPLE_LIGHTING", "UNKNOWN_1"]),
    (0x40000, &["HORIZONMAPPING", "UNKNOWN_2"]),
    (0x80000, &["UNKNOWN_3"]),
    (0x100000, &["UNKNOWN_4"]),
    (0x200000, &["UNKNOWN_5"]),
];

/// Instance flags. Some are combinations of others, and are added on top of
/// them when set (as RPKG Tool does), so `ALL` overflows.
const INSTANCE_FLAGS: &[(u32, &[&str])] = &[
    (0x1, &["OPAQUE_EMISSIVE"]),
    (0x2, &["TRANS_EMISSIVE"]),
    (0x4, &["TRANSADD_EMISSIVE"]),
    (0x6, &["TRANSALL"]),
    (0x8, &["OPAQUE_LIT"]),
    (0x10, &["TRANS_LIT"]),
    (0x20, &["DECAL"]),
    (0x40, &["REFRACTIVE"]),
    (0x80, &["LM_SKIN"]),
    (0x100, &["LM_HAIR"]),
    (0x200, &["FORCE_EMISSIVE"]),
    (0x400, &["DISABLE_SHADER_LOD"]),
    (0x800, &["DISCARD"]),
    (0x1000, &["DECAL_EMISSIVE"]),
    (0x1020, &["DECAL_ALL"]),
    (0x2000, &["WATER_CLIPPING"]),
    (0x4000, &["SAMPLE_LIGHTING"]),
    (0x8000, &["EXCLUDE_GLOBAL_SHADOWS"]),
    (0xFFFFFFFF, &["ALL"]),
];

/// The rebuilt resources with their types: the MATI, then the MATT and
/// MATB if the JSON names them.
pub fn build(text: &str) -> Result<Vec<(Built, &'static str)>, String> {
    let json: Map<String, Value> =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let path = |key: &str| -> Result<Option<RuntimeResourceID>, String> {
        match json.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.is_empty() => Ok(None),
            Some(Value::String(s)) => Ok(Some(meta::parse_hash(s))),
            Some(_) => Err(format!("`{key}` has to be a resource path or hash")),
        }
    };
    let mati = path("MATI")?.ok_or("the material has no `MATI`")?;
    let entity = path("MATT")?.zip(path("MATB")?);
    let eres = path("ERES")?;
    let mate = path("MATE")?;
    let kind = json
        .get("TYPE")
        .and_then(Value::as_str)
        .ok_or("the material has no `TYPE`")?;
    let flags = json.get("Flags");
    let class_flags = encode_flags(CLASS_FLAGS, flags.and_then(|f| f.get("Class")))?;
    let instance_flags = encode_flags(INSTANCE_FLAGS, flags.and_then(|f| f.get("Instance")))?;
    let instance = json
        .get("Material")
        .and_then(|m| m.get("Instance"))
        .and_then(|i| i.get(0))
        .and_then(Value::as_object)
        .ok_or("the material has no `Material.Instance`")?;
    let binder = instance
        .get("Binder")
        .and_then(|b| b.get(0))
        .and_then(Value::as_object)
        .ok_or("the material's instance has no `Binder`")?;

    // Textures are referenced in order, and each texture property gives
    // its reference's index (or none if it has no texture).
    let mut textures = Vec::new();
    let mut texture_ids = Vec::new();
    let mut texture_names = Vec::new();
    for texture in binder
        .get("Texture")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let (Some(id), Some(name)) = (texture.get("Texture Id"), texture.get("Name")) else {
            continue;
        };
        let id = string(id, "Texture Id")?;
        if id.is_empty() {
            texture_ids.push(u32::MAX);
        } else {
            texture_ids.push(textures.len() as u32);
            textures.push(meta::parse_hash(id));
        }
        texture_names.push(string(name, "Name")?);
    }
    let mut references: Vec<_> = textures.iter().map(|&t| (t, FLAG_LOAD)).collect();
    references.extend(mate.map(|m| (m, FLAG_INSTALL)));
    let eres_index = match eres {
        Some(eres) => {
            references.push((eres, FLAG_LOAD));
            references.len() as u32 - 1
        }
        None => u32::MAX,
    };

    let mut w = Writer::default();
    let type_length = (kind.len() / 0x10 + 1) * 0x10;
    w.u32(type_length as u32 + 0x10);
    w.align();
    w.string(kind);
    w.align();
    let instance_offset = w.out.len() as u32 + 0x30;
    for v in [
        0x10,
        textures.len() as u32,
        class_flags,
        instance_flags,
        eres_index,
    ] {
        w.u32(v);
    }
    for v in [0, 0, instance_offset, 3, 0, 0, 0] {
        w.u32(v);
    }

    let start = w.out.len() as u32;
    w.record("INST", start + 0x10, instance.len() as u32, TYPE_PROPERTY);
    let mut pending = Vec::new();
    let name = string(instance.get("Name").unwrap_or(&Value::Null), "Name")?;
    let tags = string(instance.get("Tags").unwrap_or(&Value::Null), "Tags")?;
    let start = w.out.len() as u32;
    pending.push((
        w.record("NAME", 0, name.len() as u32 + 1, TYPE_STRING),
        Pending::String(name),
    ));
    pending.push((
        w.record("TAGS", 0, tags.len() as u32 + 1, TYPE_STRING),
        Pending::String(tags),
    ));
    let bound: Vec<_> = binder
        .iter()
        .flat_map(|(key, list)| children(list).into_iter().map(move |child| (key, child)))
        .collect();
    w.record("BIND", start + 0x30, bound.len() as u32, TYPE_PROPERTY);
    let mut groups = Vec::new();
    for (key, child) in bound {
        let tag = tag(key);
        let record = w.record(&tag, 0, children(child).len() as u32, property_type(&tag));
        groups.push((record, child));
    }

    let mut texture_index = 0;
    for (record, group) in groups {
        w.point(record);
        let Some(group) = group.as_object() else {
            continue;
        };
        for (key, value) in group {
            let tag = tag(key);
            match property_type(&tag) {
                TYPE_FLOAT => match value {
                    Value::Array(values) => {
                        let record = w.record(&tag, 0, values.len() as u32, TYPE_FLOAT);
                        pending.push((record, Pending::Floats(values)));
                    }
                    value => {
                        let v = float(value, key)?;
                        w.record(&tag, v.to_bits(), 1, TYPE_FLOAT);
                    }
                },
                TYPE_STRING => {
                    let s = string(value, key)?;
                    // Unlike the name and tags, without the terminator.
                    let record = w.record(&tag, 0, s.len() as u32, TYPE_STRING);
                    pending.push((record, Pending::String(s)));
                }
                TYPE_INT if key == "Texture Id" => {
                    let id = *texture_ids
                        .get(texture_index)
                        .ok_or("a texture has a `Texture Id` but no `Name`")?;
                    texture_index += 1;
                    w.record(&tag, id, 1, TYPE_INT);
                }
                TYPE_INT => {
                    let v = int(value, key)?;
                    w.record(&tag, v, 1, TYPE_INT);
                }
                _ => {}
            }
        }
    }
    for (record, value) in pending {
        w.point(record);
        match value {
            Pending::String(s) => w.string(s),
            Pending::Floats(values) => {
                for value in values {
                    w.u32(float(value, "Value")?.to_bits());
                }
            }
        }
        w.align();
    }

    let mut built = vec![(
        Built {
            hash: mati,
            data: w.out,
            references,
        },
        "MATI",
    )];
    if let Some((matt, matb)) = entity {
        let overrides = build_overrides(json.get("Overrides"), &texture_names)?;
        let mut matt_references = vec![
            (RuntimeResourceID::from(MATT_CPPT), FLAG_INSTALL),
            (matb, FLAG_INSTALL),
            (mati, FLAG_LOAD),
        ];
        matt_references.extend(overrides.textures.into_iter().map(|t| (t, FLAG_LOAD)));
        built.push((
            Built {
                hash: matt,
                data: overrides.matt,
                references: matt_references,
            },
            "MATT",
        ));
        built.push((
            Built {
                hash: matb,
                data: overrides.matb,
                references: vec![(RuntimeResourceID::from(MATB_CBLU), FLAG_INSTALL)],
            },
            "MATB",
        ));
    }
    Ok(built)
}

/// What entities can override: each is a type byte, then in the MATT its
/// value (or, for a texture, its reference's index) and in the MATB its
/// name.
struct Overrides {
    matt: Vec<u8>,
    matb: Vec<u8>,
    /// The textures the MATT references.
    textures: Vec<RuntimeResourceID>,
}

fn build_overrides(overrides: Option<&Value>, texture_names: &[&str]) -> Result<Overrides, String> {
    let mut matt = Writer::default();
    let mut matb = Writer::default();
    let mut textures = Vec::new();
    let Some(overrides) = overrides.and_then(Value::as_object) else {
        return Ok(Overrides {
            matt: matt.out,
            matb: matb.out,
            textures,
        });
    };
    let mut name = |kind: u8, name: &str| {
        matb.out.push(kind);
        matb.u32(name.len() as u32 + 1);
        matb.string(name);
    };
    for (key, value) in overrides {
        match (key.as_str(), value) {
            ("Texture", Value::Object(map)) => {
                for (slot, texture) in map {
                    let texture = string(texture, slot)?;
                    // Only textures the instance has a slot for.
                    let known = texture_names.contains(&format!("map{slot}").as_str());
                    if texture.is_empty() || !known {
                        continue;
                    }
                    matt.out.push(1);
                    // After the MATT's type, MATB and MATI.
                    matt.u32(textures.len() as u32 + 3);
                    name(1, slot);
                    textures.push(meta::parse_hash(texture));
                }
            }
            ("Color", Value::Object(map)) => {
                for (slot, color) in map {
                    let Value::Array(values) = color else {
                        continue;
                    };
                    let kind = match values.len() {
                        3 => 2,
                        4 => 3,
                        n => return Err(format!("colour `{slot}` has {n} values, not 3 or 4")),
                    };
                    matt.out.push(kind);
                    for v in values {
                        matt.u32(float(v, slot)?.to_bits());
                    }
                    name(kind, slot);
                }
            }
            ("Texture" | "Color", _) => {
                return Err(format!("`Overrides.{key}` has to be an object"));
            }
            (_, Value::Array(values)) => {
                let kind = match values.len() {
                    2 => 5,
                    3 => 6,
                    4 => 7,
                    n => return Err(format!("override `{key}` has {n} values, not 2 to 4")),
                };
                matt.out.push(kind);
                for v in values {
                    matt.u32(float(v, key)?.to_bits());
                }
                name(kind, key);
            }
            (_, value) => {
                matt.out.push(4);
                matt.u32(float(value, key)?.to_bits());
                name(4, key);
            }
        }
    }
    Ok(Overrides {
        matt: matt.out,
        matb: matb.out,
        textures,
    })
}

/// Adds up the bits of the flags set to true. A bit with two names is
/// read from the first one the JSON has.
fn encode_flags(flags: &[(u32, &[&str])], json: Option<&Value>) -> Result<u32, String> {
    let Some(json) = json.and_then(Value::as_object) else {
        return Ok(0);
    };
    let mut value = 0u32;
    for (bit, names) in flags {
        let Some(set) = names.iter().find_map(|name| json.get(*name)) else {
            continue;
        };
        if set
            .as_bool()
            .ok_or("material flags have to be true or false")?
        {
            value = value.wrapping_add(*bit);
        }
    }
    Ok(value)
}

enum Pending<'a> {
    String(&'a str),
    Floats(&'a [Value]),
}

/// A property's tag: its four letters if it has a name, else the key as
/// written.
fn tag(key: &str) -> String {
    PROPERTIES
        .iter()
        .find(|(name, _, _)| *name == key)
        .map_or(key, |(_, tag, _)| tag)
        .to_owned()
}

fn property_type(tag: &str) -> u32 {
    PROPERTIES
        .iter()
        .find(|(_, t, _)| *t == tag)
        .map_or(u32::MAX, |(_, _, kind)| *kind)
}

/// The items of an array or the values of an object, as RPKG Tool counts
/// them.
fn children(value: &Value) -> Vec<&Value> {
    match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(map) => map.values().collect(),
        Value::Null => Vec::new(),
        other => vec![other],
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .as_str()
        .ok_or_else(|| format!("`{key}` has to be a string"))
}

fn float(value: &Value, key: &str) -> Result<f32, String> {
    value
        .as_f64()
        .map(|v| v as f32)
        .ok_or_else(|| format!("`{key}` has to be a number"))
}

fn int(value: &Value, key: &str) -> Result<u32, String> {
    match (value.as_i64(), value.as_f64()) {
        (Some(v), _) => Ok(v as u32),
        (None, Some(v)) => Ok(v as u32),
        _ => Err(format!("`{key}` has to be a number")),
    }
}

#[derive(Default)]
struct Writer {
    out: Vec<u8>,
}

impl Writer {
    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    /// A string and its terminator.
    fn string(&mut self, s: &str) {
        self.out.extend_from_slice(s.as_bytes());
        self.out.push(0);
    }

    fn align(&mut self) {
        while !self.out.len().is_multiple_of(0x10) {
            self.out.push(0);
        }
    }

    /// Writes a property record, returning where its value is so it can be
    /// pointed at data written later.
    fn record(&mut self, tag: &str, value: u32, count: u32, kind: u32) -> usize {
        let mut name = [0; 4];
        for (i, b) in tag.bytes().take(4).enumerate() {
            name[3 - i] = b;
        }
        self.out.extend_from_slice(&name);
        let at = self.out.len();
        self.u32(value);
        self.u32(count);
        // RPKG Tool writes no type for a property it doesn't know.
        if kind != u32::MAX {
            self.u32(kind);
        }
        at
    }

    /// Points a record's value at what's written next.
    fn point(&mut self, record: usize) {
        let offset = self.out.len() as u32;
        self.out[record..record + 4].copy_from_slice(&offset.to_le_bytes());
    }
}
