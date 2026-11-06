//! LOCR resources, the game's localised strings: for each language, a list
//! of strings encrypted with XTEA, each under a CRC32 of its name. Mods'
//! `localisation` and `localisationOverrides` change strings in them.
//!
//! Strings that aren't changed are kept as they're stored, so rebuilding a
//! LOCR never has to decrypt (or re-encrypt) the game's own text.
//!
//! Mods can also give a whole LOCR as TonyTools' `.locr.json` (see
//! [`Locr::from_json`]).

use rpkg_rs::encryption::xtea::Xtea;
use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::meta;

/// The game's languages in the order a LOCR lists them, as SMF names them
/// in manifests.
const LANGUAGES: [&str; 10] = [
    "xx",
    "english",
    "french",
    "italian",
    "german",
    "spanish",
    "russian",
    "chineseSimplified",
    "chineseTraditional",
    "japanese",
];

/// The same languages as the game's own codes, which manifests use too.
const CODES: [&str; 10] = ["xx", "en", "fr", "it", "de", "es", "ru", "cn", "tc", "jp"];

/// Marks a language a LOCR has no strings for.
const NO_STRINGS: u32 = u32::MAX;

/// The LOCR mods' `localisation` strings go in, and that LINE resources use.
pub const MOD_STRINGS: u64 = 0x00F5_8178_76E6_91F1;

/// The language whose strings are also used as `xx`.
pub const ENGLISH: usize = 1;
/// The language the game falls back on.
pub const FALLBACK: usize = 0;

/// A language's index in a LOCR, by name or code.
pub fn language(name: &str) -> Option<usize> {
    LANGUAGES
        .iter()
        .position(|l| l.eq_ignore_ascii_case(name))
        .or_else(|| CODES.iter().position(|c| c.eq_ignore_ascii_case(name)))
}

/// The hash a string is stored under. Manifests give it as a decimal
/// number; anything else is taken as the string's name, hashed like the
/// game's (uppercase).
pub fn string_hash(key: &str) -> u32 {
    match key.parse::<u32>() {
        Ok(hash) if key.bytes().all(|b| b.is_ascii_digit()) => hash,
        _ => name_hash(key),
    }
}

/// The hash of a string's name, as the game does it (uppercase).
pub fn name_hash(name: &str) -> u32 {
    crc32fast::hash(name.to_ascii_uppercase().as_bytes())
}

/// The contents of a LINE resource, which names a string in [`MOD_STRINGS`].
pub fn line(name: &str) -> Vec<u8> {
    let mut data = name_hash(name).to_le_bytes().to_vec();
    data.push(0);
    data
}

/// The hash of a `.locr.json` key: 8 hex digits are a hash already (as
/// TonyTools writes strings it has no name for), anything else is a name.
fn json_key_hash(key: &str) -> u32 {
    match u32::from_str_radix(key, 16) {
        Ok(hash) if key.len() == 8 => hash,
        _ => name_hash(key),
    }
}

/// TonyTools' JSON form of a LOCR.
#[derive(Deserialize)]
struct Json {
    hash: String,
    /// The languages' order, comma-separated, if not the game's.
    langmap: Option<String>,
    /// HITMAN (2016)'s other cipher, which World of Assassination doesn't use.
    #[serde(default)]
    symmetric: bool,
    languages: Map<String, Value>,
}

/// A language's strings, by hash, still encrypted.
type Strings = Vec<(u32, Vec<u8>)>;

#[derive(Debug, Clone, PartialEq)]
pub struct Locr {
    /// The byte the LOCR starts with, kept as it was.
    version: u8,
    /// Each language's strings. `None` for a language the LOCR has no
    /// table for.
    languages: Vec<Option<Strings>>,
}

impl Locr {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let Some(&version) = data.first() else {
            return Err("the LOCR is empty".to_owned());
        };
        let mut reader = Reader { data, at: 1 };
        // The offset table runs up to the first language's strings.
        let mut offsets = Vec::new();
        let mut end = data.len();
        while reader.at + 4 <= end {
            let offset = reader.u32()?;
            if offset != NO_STRINGS {
                end = end.min(offset as usize);
            }
            offsets.push(offset);
        }
        let mut languages = Vec::new();
        for offset in offsets {
            if offset == NO_STRINGS {
                languages.push(None);
                continue;
            }
            reader.at = offset as usize;
            let count = reader.u32()?;
            let mut strings = Vec::new();
            for _ in 0..count {
                let hash = reader.u32()?;
                let len = reader.u32()? as usize;
                let text = reader.bytes(len)?.to_vec();
                reader.bytes(1)?; // null terminator
                strings.push((hash, text));
            }
            languages.push(Some(strings));
        }
        Ok(Self { version, languages })
    }

    /// Builds a LOCR from TonyTools' `.locr.json`, returning its runtime ID
    /// too. Every language gets a table, empty if the JSON has no strings
    /// for it, and it starts with 0, like TonyTools writes them.
    pub fn from_json(text: &str) -> Result<(RuntimeResourceID, Self), String> {
        let json: Json =
            serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
        if json.symmetric {
            return Err("it uses HITMAN (2016)'s `symmetric` cipher".to_owned());
        }
        let order: Vec<String> = match &json.langmap {
            Some(map) => map.split(',').map(|l| l.trim().to_owned()).collect(),
            None => CODES.iter().map(|&c| c.to_owned()).collect(),
        };
        let mut locr = Self {
            version: 0,
            languages: vec![Some(Vec::new()); order.len()],
        };
        for (language, strings) in &json.languages {
            let index = order
                .iter()
                .position(|l| l.eq_ignore_ascii_case(language))
                .ok_or_else(|| format!("`{language}` isn't one of its languages"))?;
            let strings = strings
                .as_object()
                .ok_or_else(|| format!("`{language}` isn't an object of strings"))?;
            for (key, text) in strings {
                let text = text
                    .as_str()
                    .ok_or_else(|| format!("`{language}`.`{key}` isn't a string"))?;
                locr.set(index, json_key_hash(key), text)?;
            }
        }
        Ok((meta::parse_hash(&json.hash), locr))
    }

    /// Starts with the same byte as `other`.
    pub fn keep_version(&mut self, other: &Locr) {
        self.version = other.version;
    }

    /// Sets a string, replacing it if the language has it already.
    pub fn set(&mut self, language: usize, hash: u32, text: &str) -> Result<(), String> {
        let encrypted = Xtea::encrypt_string(text.to_owned(), &Xtea::WOA_L10N_KEY)
            .map_err(|e| e.to_string())?;
        if self.languages.len() <= language {
            self.languages.resize(language + 1, None);
        }
        let strings = self.languages[language].get_or_insert_with(Vec::new);
        match strings.iter_mut().find(|(h, _)| *h == hash) {
            Some(existing) => existing.1 = encrypted,
            None => strings.push((hash, encrypted)),
        }
        Ok(())
    }

    /// A string's text, if the language has it.
    #[cfg(test)]
    pub fn get(&self, language: usize, hash: u32) -> Option<String> {
        let strings = self.languages.get(language)?.as_ref()?;
        let (_, text) = strings.iter().find(|(h, _)| *h == hash)?;
        let text = Xtea::decrypt_string(text, &Xtea::WOA_L10N_KEY).ok()?;
        Some(text.trim_end_matches('\0').to_owned())
    }

    pub fn serialize(&self) -> Vec<u8> {
        let mut out = vec![self.version];
        let table = out.len();
        out.resize(table + self.languages.len() * 4, 0);
        for (i, strings) in self.languages.iter().enumerate() {
            let offset = match strings {
                Some(_) => out.len() as u32,
                None => NO_STRINGS,
            };
            out[table + i * 4..table + i * 4 + 4].copy_from_slice(&offset.to_le_bytes());
            let Some(strings) = strings else {
                continue;
            };
            out.extend_from_slice(&(strings.len() as u32).to_le_bytes());
            for (hash, text) in strings {
                out.extend_from_slice(&hash.to_le_bytes());
                out.extend_from_slice(&(text.len() as u32).to_le_bytes());
                out.extend_from_slice(text);
                out.push(0);
            }
        }
        out
    }
}

struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, len: usize) -> Result<&'a [u8], String> {
        let bytes = self
            .data
            .get(self.at..self.at + len)
            .ok_or("the LOCR is truncated")?;
        self.at += len;
        Ok(bytes)
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
}
