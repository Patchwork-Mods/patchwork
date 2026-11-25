//! Reading files out of Larian's `.pak` packages (LSPK, version 18: what
//! Baldur's Gate 3 and its mods use), as LSLib does.
//!
//! A package starts with `LSPK` and a header that says where its file list
//! is. The list is a count, a compressed size, and that many LZ4-compressed
//! (block format) entries of 272 bytes: a 256-byte name, where its data is,
//! how it's compressed, and its size on disk and once uncompressed.
//! Only the list and the few files asked for are read, never the whole
//! package.
//!
//! A big package can be split in parts: `Big.pak` with the header and the
//! first part's data, then `Big_1.pak`, `Big_2.pak` and so on with only
//! data. Each entry says which part its data is in, and where in that
//! part's file.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// A file in a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its path in the package, with `/` between folders.
    pub name: String,
    offset: u64,
    part: u8,
    flags: u8,
    size_on_disk: u32,
    uncompressed: u32,
}

/// An open package.
#[derive(Debug)]
pub struct Pak {
    file: File,
    len: u64,
    pub entries: Vec<Entry>,
}

const SIGNATURE: &[u8; 4] = b"LSPK";
const VERSION: u32 = 18;
/// Everything after the signature: version, file list offset and size,
/// flags, priority, MD5 and the number of parts.
const HEADER: usize = 4 + 8 + 4 + 1 + 1 + 16 + 2;
const ENTRY: usize = 256 + 4 + 2 + 1 + 1 + 4 + 4;
/// All files compressed into one LZ4 frame, which mods never are.
const SOLID: u8 = 0x04;
/// More files than any package has, so a broken header can't ask for
/// gigabytes.
const MAX_FILES: usize = 1 << 20;
/// No file read from a package (`meta.lsx`) comes close to this.
const MAX_FILE: u32 = 64 << 20;

const NONE: u8 = 0;
const ZLIB: u8 = 1;
const LZ4: u8 = 2;
const ZSTD: u8 = 3;

impl Pak {
    pub fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        let mut head = [0u8; 4 + HEADER];
        let not_pak = || "it isn't a Larian package (it doesn't start with LSPK)".to_owned();
        file.read_exact(&mut head[..4]).map_err(|_| not_pak())?;
        if &head[..4] != SIGNATURE {
            return Err(not_pak());
        }
        file.read_exact(&mut head[4..])
            .map_err(|_| "it's too short to be a package".to_owned())?;
        let version = u32_at(&head, 4);
        if version != VERSION {
            return Err(format!(
                "it's a version {version} package, and only version {VERSION} (Baldur's Gate 3's) can be read"
            ));
        }
        let list_offset = u64::from_le_bytes(head[8..16].try_into().unwrap());
        let flags = head[20];
        if flags & SOLID != 0 {
            return Err("it's a solid package, which can't be read yet".to_owned());
        }

        file.seek(SeekFrom::Start(list_offset))
            .map_err(|e| e.to_string())?;
        let mut counts = [0u8; 8];
        file.read_exact(&mut counts)
            .map_err(|_| "its file list is cut short".to_owned())?;
        let count = u32_at(&counts, 0) as usize;
        let compressed = u32_at(&counts, 4) as u64;
        if count > MAX_FILES || list_offset + 8 + compressed > len {
            return Err("its file list is broken".to_owned());
        }
        let mut packed = vec![0u8; compressed as usize];
        file.read_exact(&mut packed)
            .map_err(|_| "its file list is cut short".to_owned())?;
        let list = lz4_flex::block::decompress(&packed, count * ENTRY)
            .map_err(|e| format!("its file list is broken: {e}"))?;
        if list.len() != count * ENTRY {
            return Err("its file list is broken".to_owned());
        }
        let entries = list
            .as_chunks::<ENTRY>()
            .0
            .iter()
            .map(|e| entry(e))
            .collect();
        Ok(Self { file, len, entries })
    }

    /// The contents of `entry`, one of this package's. `part` finds a later
    /// part's file from its name (`Big_1.pak` for part 1 of `Big.pak`,
    /// named `file`), for an entry whose data is in one.
    pub fn read(
        &mut self,
        entry: &Entry,
        file: &str,
        part: &dyn Fn(&str) -> Option<PathBuf>,
    ) -> Result<Vec<u8>, String> {
        if entry.size_on_disk > MAX_FILE || entry.uncompressed > MAX_FILE {
            return Err(format!("{} is too big", entry.name));
        }
        let mut other;
        let (data_file, len) = if entry.part == 0 {
            (&mut self.file, self.len)
        } else {
            let name = part_name(file, entry.part);
            let missing = || format!("{} is in {name}, which isn't there", entry.name);
            let path = part(&name).ok_or_else(missing)?;
            other = File::open(path).map_err(|_| missing())?;
            let len = other.metadata().map_err(|e| e.to_string())?.len();
            (&mut other, len)
        };
        if entry.offset + entry.size_on_disk as u64 > len {
            return Err(format!("{} is cut short", entry.name));
        }
        data_file
            .seek(SeekFrom::Start(entry.offset))
            .map_err(|e| e.to_string())?;
        let mut data = vec![0u8; entry.size_on_disk as usize];
        data_file
            .read_exact(&mut data)
            .map_err(|_| format!("{} is cut short", entry.name))?;
        let size = entry.uncompressed as usize;
        let bad = |e: String| format!("couldn't uncompress {}: {e}", entry.name);
        match entry.flags & 0x0f {
            NONE => Ok(data),
            ZLIB => {
                let mut out = Vec::with_capacity(size);
                flate2::read::ZlibDecoder::new(&data[..])
                    .take(MAX_FILE as u64)
                    .read_to_end(&mut out)
                    .map_err(|e| bad(e.to_string()))?;
                Ok(out)
            }
            LZ4 => lz4_flex::block::decompress(&data, size).map_err(|e| bad(e.to_string())),
            ZSTD => {
                let mut out = Vec::with_capacity(size);
                ruzstd::decoding::StreamingDecoder::new(&data[..])
                    .map_err(|e| bad(e.to_string()))?
                    .take(MAX_FILE as u64)
                    .read_to_end(&mut out)
                    .map_err(|e| bad(e.to_string()))?;
                Ok(out)
            }
            other => Err(bad(format!("unknown compression method {other}"))),
        }
    }
}

/// The file name of part `n` of the package named `file`: `Big_1.pak` for
/// part 1 of `Big.pak`.
pub fn part_name(file: &str, n: u8) -> String {
    let stem = file
        .len()
        .checked_sub(".pak".len())
        .filter(|&at| file.is_char_boundary(at) && file[at..].eq_ignore_ascii_case(".pak"))
        .map_or(file, |at| &file[..at]);
    format!("{stem}_{n}.pak")
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn entry(b: &[u8]) -> Entry {
    let name = &b[..256];
    let name = &name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())];
    let low = u32_at(b, 256) as u64;
    let high = u16::from_le_bytes([b[260], b[261]]) as u64;
    Entry {
        name: String::from_utf8_lossy(name).replace('\\', "/"),
        offset: low | (high << 32),
        part: b[262],
        flags: b[263],
        size_on_disk: u32_at(b, 264),
        uncompressed: u32_at(b, 268),
    }
}

/// Writes packages, for tests.
#[cfg(test)]
pub(crate) mod write {
    use std::io::Write;

    use super::*;

    /// How a file is stored.
    #[derive(Clone, Copy)]
    pub enum How {
        Stored,
        Zlib,
        Lz4,
        Zstd,
    }

    /// A version 18 package of `files`.
    pub fn pak(files: &[(&str, &[u8], How)]) -> Vec<u8> {
        let files: Vec<_> = files.iter().map(|&(n, c, h)| (n, c, h, 0)).collect();
        split(&files).swap_remove(0)
    }

    /// A version 18 package of `files`, each in the part it says: the
    /// package's own file, then each later part's.
    pub fn split(files: &[(&str, &[u8], How, u8)]) -> Vec<Vec<u8>> {
        let mut out = SIGNATURE.to_vec();
        out.resize(4 + HEADER, 0);
        let parts = files.iter().map(|f| f.3).max().unwrap_or(0);
        let mut later = vec![Vec::new(); parts as usize];
        let mut list = Vec::new();
        for &(name, contents, how, part) in files {
            let into = match part {
                0 => &mut out,
                n => &mut later[n as usize - 1],
            };
            let offset = into.len() as u64;
            let (flags, data) = match how {
                How::Stored => (NONE, contents.to_vec()),
                How::Zlib => {
                    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
                    z.write_all(contents).unwrap();
                    (ZLIB | 0x20, z.finish().unwrap())
                }
                How::Lz4 => (LZ4 | 0x20, lz4_flex::block::compress(contents)),
                How::Zstd => (
                    ZSTD | 0x20,
                    ruzstd::encoding::compress_to_vec(
                        contents,
                        ruzstd::encoding::CompressionLevel::Fastest,
                    ),
                ),
            };
            into.extend_from_slice(&data);
            let mut e = [0u8; ENTRY];
            e[..name.len()].copy_from_slice(name.as_bytes());
            e[256..260].copy_from_slice(&(offset as u32).to_le_bytes());
            e[260..262].copy_from_slice(&((offset >> 32) as u16).to_le_bytes());
            e[262] = part;
            e[263] = flags;
            e[264..268].copy_from_slice(&(data.len() as u32).to_le_bytes());
            let uncompressed = if flags == NONE { 0 } else { contents.len() };
            e[268..272].copy_from_slice(&(uncompressed as u32).to_le_bytes());
            list.extend_from_slice(&e);
        }
        let list_offset = out.len() as u64;
        let packed = lz4_flex::block::compress(&list);
        out.extend_from_slice(&(files.len() as u32).to_le_bytes());
        out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        out.extend_from_slice(&packed);
        out[4..8].copy_from_slice(&VERSION.to_le_bytes());
        out[8..16].copy_from_slice(&list_offset.to_le_bytes());
        out[16..20].copy_from_slice(&((8 + packed.len()) as u32).to_le_bytes());
        out[38..40].copy_from_slice(&(u16::from(parts) + 1).to_le_bytes());
        let mut all = vec![out];
        all.extend(later);
        all
    }
}
