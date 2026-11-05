//! DLGE resources, the game's dialogue: a tree of containers (random
//! picks, switches and sequences) whose leaves are lines, each with a
//! sound and face animation per language and an XTEA-encrypted subtitle.
//! Mods give them as TonyTools' `.dlge.json`, which is rebuilt here the way
//! TonyTools does.
//!
//! Soundtags, switches and lines are named in the JSON; the game stores the
//! CRC32 of each name (TonyTools' hash list only maps names back), so no
//! hash list is needed to rebuild.
//!
//! Each DLGE references a DITL, which maps soundtags to the resources that
//! play them, and a CLNG, which says which languages have their own voice
//! lines. Mods can give those as `.ditl.json` and `.clng.json`.

use rpkg_rs::encryption::xtea::Xtea;
use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::meta;

/// The languages a DLGE has slots for, unless the JSON gives a `langmap`.
const LANGUAGES: [&str; 10] = ["xx", "en", "fr", "it", "de", "es", "ru", "cn", "tc", "jp"];
/// The language `defaultWav` and `defaultFfx` are for.
const DEFAULT_LANGUAGE: &str = "en";
/// The flag on references to the DITL and CLNG.
const LOOKUP_FLAG: u8 = 0x1F;

#[derive(Deserialize)]
struct Json {
    hash: String,
    #[serde(rename = "DITL")]
    ditl: String,
    #[serde(rename = "CLNG")]
    clng: String,
    langmap: Option<String>,
    #[serde(rename = "rootContainer")]
    root: Container,
}

#[derive(Deserialize, Clone)]
#[serde(tag = "type")]
enum Container {
    WavFile(WavFile),
    Random {
        cases: Option<Vec<String>>,
        containers: Vec<Container>,
    },
    Switch {
        #[serde(rename = "switchKey")]
        switch_key: String,
        default: String,
        containers: Vec<Container>,
    },
    Sequence {
        containers: Vec<Container>,
    },
    Null,
}

#[derive(Deserialize, Clone)]
struct WavFile {
    #[serde(rename = "wavName")]
    wav_name: String,
    cases: Option<Vec<String>>,
    weight: Option<Value>,
    soundtag: String,
    #[serde(rename = "defaultWav")]
    default_wav: Option<String>,
    #[serde(rename = "defaultFfx")]
    default_ffx: Option<String>,
    #[serde(default)]
    languages: Map<String, Value>,
}

impl Container {
    fn kind(&self) -> u16 {
        match self {
            Container::WavFile(_) => 1,
            Container::Random { .. } => 2,
            Container::Switch { .. } => 3,
            Container::Sequence { .. } => 4,
            Container::Null => 0x15,
        }
    }
}

/// A rebuilt resource: its runtime ID, data and references in order.
pub struct Built {
    pub hash: RuntimeResourceID,
    pub data: Vec<u8>,
    pub references: Vec<(RuntimeResourceID, u8)>,
}

pub fn build(text: &str) -> Result<Built, String> {
    let json: Json = parse(text)?;
    let languages: Vec<String> = match &json.langmap {
        Some(map) => map.split(',').map(str::to_owned).collect(),
        None => LANGUAGES.iter().map(|&l| l.to_owned()).collect(),
    };
    let mut writer = Writer {
        languages,
        ..Writer::default()
    };
    writer.depend(&json.ditl, LOOKUP_FLAG);
    writer.u32(0);
    writer.depend(&json.clng, LOOKUP_FLAG);
    writer.u32(1);
    if matches!(json.root, Container::Null) {
        return Err("the dialogue's root container is empty".to_owned());
    }
    writer.container(&json.root)?;
    let index = match json.root {
        Container::WavFile(_) => writer.indices.wav,
        _ => writer.indices.global,
    };
    writer.u16((json.root.kind() << 12) | (index as u16 & 0xFFF));
    Ok(writer.finish(&json.hash))
}

/// Rebuilds a DITL: each soundtag's hash, with a reference to what it
/// plays.
pub fn build_ditl(text: &str) -> Result<Built, String> {
    #[derive(Deserialize)]
    struct Ditl {
        hash: String,
        soundtags: Map<String, Value>,
    }
    let json: Ditl = parse(text)?;
    let mut writer = Writer::default();
    writer.u32(json.soundtags.len() as u32);
    for (tag, target) in &json.soundtags {
        let target = target
            .as_str()
            .ok_or_else(|| format!("soundtag `{tag}` isn't given a resource"))?;
        let index = writer.depend(target, LOOKUP_FLAG);
        writer.u32(index);
        writer.u32(name_hash(tag));
    }
    Ok(writer.finish(&json.hash))
}

/// Rebuilds a CLNG: a byte per language, set if it has its own voice lines.
pub fn build_clng(text: &str) -> Result<Built, String> {
    #[derive(Deserialize)]
    struct Clng {
        hash: String,
        languages: Map<String, Value>,
    }
    let json: Clng = parse(text)?;
    let mut writer = Writer::default();
    for (language, value) in &json.languages {
        let value = value
            .as_bool()
            .ok_or_else(|| format!("language `{language}` has to be true or false"))?;
        writer.out.push(u8::from(value));
    }
    Ok(writer.finish(&json.hash))
}

fn parse<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())
}

/// The last index of each kind of container written, -1 before the first.
struct Indices {
    global: i32,
    wav: i32,
    random: i32,
    switch: i32,
    sequence: i32,
}

impl Default for Indices {
    fn default() -> Self {
        Self {
            global: -1,
            wav: -1,
            random: -1,
            switch: -1,
            sequence: -1,
        }
    }
}

#[derive(Default)]
struct Writer {
    out: Vec<u8>,
    languages: Vec<String>,
    /// Referenced resources by path or hash, with their flags.
    depends: Vec<(String, u8)>,
    indices: Indices,
}

/// A name's hash: 8 hex digits are a hash already (as TonyTools writes ones
/// it has no name for), anything else is hashed.
fn name_hash(name: &str) -> u32 {
    match u32::from_str_radix(name, 16) {
        Ok(hash) if name.len() == 8 => hash,
        _ => crc32fast::hash(name.as_bytes()),
    }
}

fn subtitle(text: &str) -> Result<Vec<u8>, String> {
    Xtea::encrypt_string(text.to_owned(), &Xtea::WOA_L10N_KEY).map_err(|e| e.to_string())
}

impl Writer {
    fn finish(self, hash: &str) -> Built {
        Built {
            hash: meta::parse_hash(hash),
            data: self.out,
            references: self
                .depends
                .into_iter()
                .map(|(path, flag)| (meta::parse_hash(&path), flag))
                .collect(),
        }
    }

    fn u16(&mut self, v: u16) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    fn bytes(&mut self, bytes: &[u8]) {
        self.u32(bytes.len() as u32);
        self.out.extend_from_slice(bytes);
    }

    /// The index of a reference, adding it if it's new.
    fn depend(&mut self, path: &str, flag: u8) -> u32 {
        match self.depends.iter().position(|(p, _)| p == path) {
            Some(i) => i as u32,
            None => {
                self.depends.push((path.to_owned(), flag));
                (self.depends.len() - 1) as u32
            }
        }
    }

    /// Writes a container's header and its (type, index, hashes) entries.
    fn header(&mut self, kind: u8, group: u32, default: u32, entries: &[(u16, Vec<u32>)]) {
        self.out.push(kind);
        self.u32(group);
        self.u32(default);
        self.u32(entries.len() as u32);
        for (type_index, hashes) in entries {
            self.u16(*type_index);
            self.u32(hashes.len() as u32);
            for hash in hashes {
                self.u32(*hash);
            }
        }
    }

    fn entry(child: &Container, index: i32) -> u16 {
        (child.kind() << 12) | (index as u16 & 0xFFF)
    }

    fn container(&mut self, container: &Container) -> Result<(), String> {
        match container {
            Container::WavFile(wav) => {
                self.wav(wav)?;
                self.indices.wav += 1;
            }
            Container::Random { containers, .. } => {
                let mut entries = Vec::new();
                for child in containers {
                    let Container::WavFile(wav) = child else {
                        return Err("a Random container can only hold WavFiles".to_owned());
                    };
                    let weight = match &wav.weight {
                        Some(Value::String(hex)) => u32::from_str_radix(hex, 16)
                            .map_err(|_| format!("`{hex}` isn't a weight"))?,
                        Some(Value::Number(n)) => {
                            (n.as_f64().unwrap_or(0.0) * f64::from(0xFF_FFFF)).round() as u32
                        }
                        _ => return Err(format!("{} has no weight", wav.wav_name)),
                    };
                    self.container(child)?;
                    entries.push((Self::entry(child, self.indices.wav), vec![weight]));
                }
                self.header(2, 0, 0, &entries);
                self.indices.global += 1;
                self.indices.random += 1;
            }
            Container::Switch {
                switch_key,
                default,
                containers,
            } => {
                if self.indices.switch != -1 {
                    return Err("a dialogue can only have one Switch".to_owned());
                }
                let mut entries = Vec::new();
                for child in containers {
                    let cases = match child {
                        Container::WavFile(w) => &w.cases,
                        Container::Random { cases, .. } => cases,
                        _ => return Err("a Switch can only hold WavFiles and Randoms".to_owned()),
                    };
                    let cases = cases.clone().ok_or("a Switch's containers need `cases`")?;
                    self.container(child)?;
                    let index = match child {
                        Container::WavFile(_) => self.indices.wav,
                        _ => self.indices.random,
                    };
                    let hashes = cases.iter().map(|c| name_hash(c)).collect();
                    entries.push((Self::entry(child, index), hashes));
                    self.indices.switch += 1;
                }
                self.header(3, name_hash(switch_key), name_hash(default), &entries);
                self.indices.global += 1;
                self.indices.switch += 1;
            }
            Container::Sequence { containers } => {
                if self.indices.sequence != -1 {
                    return Err("a dialogue can only have one Sequence".to_owned());
                }
                let mut entries = Vec::new();
                for child in containers {
                    self.container(child)?;
                    let index = match child {
                        Container::WavFile(_) => self.indices.wav,
                        Container::Random { .. } | Container::Switch { .. } => self.indices.global,
                        _ => return Err("a Sequence can't hold another Sequence".to_owned()),
                    };
                    entries.push((Self::entry(child, index), Vec::new()));
                    self.indices.sequence += 1;
                }
                self.header(4, 0, 0, &entries);
                self.indices.global += 1;
                self.indices.sequence += 1;
            }
            Container::Null => return Err("a container is empty".to_owned()),
        }
        Ok(())
    }

    /// A line: its soundtag and name, then per language its sound and face
    /// animation (as reference indices) and subtitle.
    fn wav(&mut self, wav: &WavFile) -> Result<(), String> {
        self.out.push(1);
        self.u32(crc32fast::hash(wav.soundtag.as_bytes()));
        self.u32(name_hash(&wav.wav_name));
        self.u32(0);
        for (i, language) in self.languages.clone().iter().enumerate() {
            let flag = 0x80 + i as u8;
            let value = wav.languages.get(language);
            if let (true, Some(sound), Some(ffx)) = (
                language == DEFAULT_LANGUAGE,
                &wav.default_wav,
                &wav.default_ffx,
            ) {
                let (sound, ffx) = (self.depend(sound, flag), self.depend(ffx, flag));
                self.u32(sound);
                self.u32(ffx);
                match value.and_then(Value::as_str) {
                    Some(text) => self.bytes(&subtitle(text)?),
                    None => self.u32(0),
                }
                continue;
            }
            match value {
                Some(Value::Object(o)) => {
                    let path = |k: &str| {
                        o.get(k)
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                            .ok_or(format!("{}'s `{language}` has no `{k}`", wav.wav_name))
                    };
                    let (sound, ffx) = (path("wav")?, path("ffx")?);
                    let (sound, ffx) = (self.depend(&sound, flag), self.depend(&ffx, flag));
                    self.u32(sound);
                    self.u32(ffx);
                    match o.get("subtitle").and_then(Value::as_str) {
                        Some(text) => self.bytes(&subtitle(text)?),
                        None => self.u32(0),
                    }
                }
                Some(Value::String(text)) => {
                    self.out.extend_from_slice(&u64::MAX.to_le_bytes());
                    self.bytes(&subtitle(text)?);
                }
                _ => {
                    self.out.extend_from_slice(&u64::MAX.to_le_bytes());
                    self.u32(0);
                }
            }
        }
        Ok(())
    }
}
