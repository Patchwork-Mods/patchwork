//! ORES resources: the game's lookup tables for online content, stored in
//! Glacier's BIN1 serialization format. There are two kinds:
//!
//! - JSON ORES (like the unlockables, `0057C2C3941115CA`): a single string
//!   holding a JSON document.
//! - Hash ORES (like the contracts, `002B07020D21D727`, and blobs,
//!   `00858D45F5F9E3CA`): a list of strings, each with a runtime ID.
//!
//! BIN1 is a 16 byte header (`BIN1`, alignment, section count, then the data
//! size, big endian), the data, whose pointers are offsets from the start of
//! the data, and a relocation section listing where those pointers are.

const MAGIC: &[u8; 8] = b"BIN1\x00\x08\x01\x00";
/// Where the data starts, and what pointers in it are relative to.
const DATA: usize = 0x10;
const RELOCATIONS: &[u8; 4] = b"\xED\xA5\xEB\x12";
/// Set in a string's length when its characters are stored separately.
const STATIC_STRING: u32 = 0x4000_0000;
const LENGTH_MASK: u32 = 0x3FFF_FFFF;
/// The size of a hash ORES entry: a string (length, padding, pointer) and a
/// runtime ID (high then low half).
const ENTRY: usize = 0x18;

#[derive(Debug, Clone, PartialEq)]
pub enum Ores {
    Json(String),
    Hashes(Vec<(u64, String)>),
}

impl Ores {
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let r = Reader(bytes);
        if r.get(0, 8)? != MAGIC {
            return Err("not a BIN1 file".to_owned());
        }
        // A hash ORES's root is an array, whose first pointer is to the
        // entries just after the array (and its length); a JSON ORES's is a
        // string.
        let first = r.u32(DATA)?;
        if first & STATIC_STRING != 0 {
            let len = (first & LENGTH_MASK) as usize;
            let chars = DATA + r.u64(DATA + 8)? as usize;
            let text = r.get(chars, len)?;
            return String::from_utf8(text.to_vec())
                .map(Ores::Json)
                .map_err(|e| e.to_string());
        }
        let begin = DATA + r.u64(DATA)? as usize;
        let end = DATA + r.u64(DATA + 8)? as usize;
        let count = end.checked_sub(begin).ok_or("bad array")? / ENTRY;
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let at = begin + i * ENTRY;
            let len = (r.u32(at)? & LENGTH_MASK) as usize;
            let chars = DATA + r.u64(at + 8)? as usize;
            let text = String::from_utf8(r.get(chars, len)?.to_vec()).map_err(|e| e.to_string())?;
            let hash = (u64::from(r.u32(at + 16)?) << 32) | u64::from(r.u32(at + 20)?);
            entries.push((hash, text));
        }
        Ok(Ores::Hashes(entries))
    }

    pub fn serialize(&self) -> Vec<u8> {
        match self {
            Ores::Json(text) => serialize_json(text),
            Ores::Hashes(entries) => serialize_hashes(entries),
        }
    }
}

fn serialize_json(text: &str) -> Vec<u8> {
    let len = text.len() as u32;
    let mut data = Vec::new();
    // The root string, its characters right after it (behind their length,
    // counting the terminator).
    data.extend((len | STATIC_STRING).to_le_bytes());
    data.extend(0u32.to_le_bytes());
    data.extend(0x14u64.to_le_bytes());
    data.extend((len + 1).to_le_bytes());
    data.extend(text.as_bytes());
    data.push(0);
    finish(data, &[8])
}

fn serialize_hashes(entries: &[(u64, String)]) -> Vec<u8> {
    let array = 0x20u64;
    let strings = array as usize + entries.len() * ENTRY;
    let mut data = Vec::new();
    // The array: begin, end and capacity pointers, then its length.
    data.extend(array.to_le_bytes());
    data.extend((strings as u64).to_le_bytes());
    data.extend((strings as u64).to_le_bytes());
    data.extend(0u32.to_le_bytes());
    data.extend((entries.len() as u32).to_le_bytes());

    let mut pointers = vec![0, 8, 16];
    let mut string_data = Vec::new();
    for (i, (hash, text)) in entries.iter().enumerate() {
        let chars = strings + string_data.len() + 4;
        data.extend((text.len() as u32 | STATIC_STRING).to_le_bytes());
        data.extend(0u32.to_le_bytes());
        pointers.push(data.len());
        data.extend((chars as u64).to_le_bytes());
        data.extend(((hash >> 32) as u32).to_le_bytes());
        data.extend((*hash as u32).to_le_bytes());

        string_data.extend((text.len() as u32 + 1).to_le_bytes());
        string_data.extend(text.as_bytes());
        string_data.push(0);
        if i + 1 < entries.len() {
            string_data.resize(string_data.len().next_multiple_of(4), 0);
        }
    }
    data.extend(string_data);
    finish(data, &pointers)
}

/// Adds the header and the relocation section listing `pointers`.
fn finish(data: Vec<u8>, pointers: &[usize]) -> Vec<u8> {
    let mut out = Vec::with_capacity(DATA + data.len() + 12 + pointers.len() * 4);
    out.extend(MAGIC);
    out.extend((data.len() as u32).to_be_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(data);
    out.extend(RELOCATIONS);
    out.extend((4 + pointers.len() as u32 * 4).to_le_bytes());
    out.extend((pointers.len() as u32).to_le_bytes());
    for pointer in pointers {
        out.extend((*pointer as u32).to_le_bytes());
    }
    out
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn get(&self, at: usize, len: usize) -> Result<&[u8], String> {
        self.0
            .get(at..at + len)
            .ok_or_else(|| "ORES is truncated".to_owned())
    }

    fn u32(&self, at: usize) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.get(at, 4)?.try_into().unwrap()))
    }

    fn u64(&self, at: usize) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.get(at, 8)?.try_into().unwrap()))
    }
}
