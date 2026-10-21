//! RPKG Tool's resource metadata: the `.meta` (binary) or `.meta.json` file
//! that can sit next to a raw resource in a mod, giving its references and
//! how it's stored.

use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;
use serde::Deserialize;

use super::resource::Header;

#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub resource_type: String,
    pub compressed: bool,
    pub scrambled: bool,
    /// Referenced resources and their flags, in order.
    pub references: Vec<(RuntimeResourceID, u8)>,
    size_final: u32,
    size_in_memory: u32,
    pub size_in_video_memory: u32,
}

const HEADER_LEN: usize = 44;
const SCRAMBLED: u32 = 0x8000_0000;
const COMPRESSED_SIZE: u32 = 0x3FFF_FFFF;

impl Meta {
    pub fn from_binary(bytes: &[u8]) -> Result<Self, String> {
        let u32_at = |at: usize| -> Result<u32, String> {
            let b = bytes.get(at..at + 4).ok_or("meta file is truncated")?;
            Ok(u32::from_le_bytes(b.try_into().unwrap()))
        };
        if bytes.len() < HEADER_LEN {
            return Err("meta file is truncated".to_owned());
        }
        let size = u32_at(16)?;
        let resource_type = String::from_utf8_lossy(&bytes[20..24]).into_owned();
        let table_size = u32_at(24)?;
        let mut references = Vec::new();
        if table_size != 0 {
            let count = (u32_at(HEADER_LEN)? & COMPRESSED_SIZE) as usize;
            let flags = HEADER_LEN + 4;
            let hashes = flags + count;
            let end = hashes + count * 8;
            if bytes.len() < end {
                return Err("meta file's reference table is truncated".to_owned());
            }
            for i in 0..count {
                let at = hashes + i * 8;
                let hash = u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
                references.push((RuntimeResourceID::from(hash), bytes[flags + i]));
            }
        }
        Ok(Self {
            resource_type,
            compressed: size & COMPRESSED_SIZE != 0,
            scrambled: size & SCRAMBLED != 0,
            references,
            size_final: u32_at(32)?,
            size_in_memory: u32_at(36)?,
            size_in_video_memory: u32_at(40)?,
        })
    }

    pub fn from_json(text: &str) -> Result<Self, String> {
        #[derive(Deserialize)]
        struct Json {
            hash_size: u32,
            hash_resource_type: String,
            #[serde(default)]
            hash_reference_data: Vec<JsonReference>,
            hash_size_final: u32,
            hash_size_in_memory: u32,
            hash_size_in_video_memory: u32,
        }
        #[derive(Deserialize)]
        struct JsonReference {
            hash: String,
            flag: String,
        }

        let json: Json = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let references = json
            .hash_reference_data
            .iter()
            .map(|r| {
                let flag = u8::from_str_radix(&r.flag, 16)
                    .map_err(|_| format!("`{}` isn't a reference flag", r.flag))?;
                Ok((parse_hash(&r.hash), flag))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            resource_type: json.hash_resource_type,
            compressed: json.hash_size & COMPRESSED_SIZE != 0,
            scrambled: json.hash_size & SCRAMBLED != 0,
            references,
            size_final: json.hash_size_final,
            size_in_memory: json.hash_size_in_memory,
            size_in_video_memory: json.hash_size_in_video_memory,
        })
    }

    /// The memory a resource of `size` bytes needs, scaled from what the
    /// meta says the original needed.
    pub fn system_memory(&self, size: u32) -> u32 {
        super::resource::scale_memory(self.size_in_memory, self.size_final, size)
    }

    /// The archive entry header for the resource, with `size` bytes of data.
    pub fn header(&self, size: u32) -> Header {
        Header {
            references: self.references.clone(),
            compressed: self.compressed,
            scrambled: self.scrambled,
            system_memory: self.system_memory(size),
            video_memory: self.size_in_video_memory,
        }
    }
}

/// A runtime ID written as 16 hex digits, or as the resource path it's the
/// hash of.
pub fn parse_hash(s: &str) -> RuntimeResourceID {
    match u64::from_str_radix(s, 16) {
        Ok(hash) if s.len() == 16 => RuntimeResourceID::from(hash),
        _ => RuntimeResourceID::from_raw_string(&s.to_lowercase()),
    }
}

/// Parses a runtime ID that has to be written as hex, e.g. in a file name.
pub fn parse_hex_hash(s: &str) -> Option<RuntimeResourceID> {
    (s.len() == 16)
        .then(|| u64::from_str_radix(s, 16).ok())
        .flatten()
        .map(RuntimeResourceID::from)
}
