//! FTL's resource archive, `ftl.dat` (FTL 1.6 and later).
//!
//! Everything is big-endian:
//!
//! ```text
//! header   "PKG\n", u16 header size (16), u16 entry size (20),
//!          u32 entry count, u32 size of the path region
//! entries  u32 path hash, u32 path offset (top byte: flags),
//!          u32 data offset, u32 data size, u32 unpacked size
//! paths    NUL-terminated, at the offsets the entries give
//! data     each file's bytes
//! ```
//!
//! Entries are sorted by [`hash`], which is how the game looks paths up,
//! case-insensitively. The game's own archive keeps every file
//! uncompressed and starts each at a multiple of 64 bytes, and so does
//! [`write`].

use std::fs::File;
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"PKG\n";
const HEADER_SIZE: u16 = 16;
const ENTRY_SIZE: u16 = 20;
const ALIGN: u64 = 64;
/// The flag (in a path offset's top byte) for a compressed file.
const DEFLATED: u32 = 0x0100_0000;

/// The hash the game finds files by: each byte of the lowercased path
/// XORed into the hash rotated left by 27.
pub fn hash(path: &str) -> u32 {
    path.bytes().fold(0, |h: u32, b| {
        h.rotate_left(27) ^ u32::from(b.to_ascii_lowercase())
    })
}

/// A file in an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub offset: u64,
    pub size: u32,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn be_u16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// The files in the archive `r` holds, in the order they're listed.
pub fn read_index(r: &mut impl Read) -> io::Result<Vec<Entry>> {
    let mut header = [0; HEADER_SIZE as usize];
    r.read_exact(&mut header)?;
    if &header[..4] != MAGIC {
        return Err(invalid("not an FTL 1.6 resource archive (no PKG header)"));
    }
    let (header_size, entry_size) = (be_u16(&header[4..]), be_u16(&header[6..]));
    if header_size != HEADER_SIZE || entry_size != ENTRY_SIZE {
        return Err(invalid(format!(
            "unexpected archive layout (header {header_size}, entries {entry_size})"
        )));
    }
    let count = be_u32(&header[8..]) as usize;
    let paths_size = be_u32(&header[12..]) as usize;
    let mut entries = vec![0; count * ENTRY_SIZE as usize];
    r.read_exact(&mut entries)?;
    let mut paths = vec![0; paths_size];
    r.read_exact(&mut paths)?;
    entries
        .as_chunks::<{ ENTRY_SIZE as usize }>()
        .0
        .iter()
        .map(|e| {
            let path_field = be_u32(&e[4..]);
            if path_field & DEFLATED != 0 {
                return Err(invalid("compressed files in ftl.dat aren't supported"));
            }
            let start = (path_field & 0x00ff_ffff) as usize;
            let path = paths
                .get(start..)
                .and_then(|p| p.iter().position(|&b| b == 0).map(|end| &p[..end]))
                .ok_or_else(|| invalid("a file's path is outside the archive's paths"))?;
            let path = String::from_utf8(path.to_vec())
                .map_err(|_| invalid("a file's path isn't UTF-8"))?;
            Ok(Entry {
                path,
                offset: u64::from(be_u32(&e[8..])),
                size: be_u32(&e[12..]),
            })
        })
        .collect()
}

/// Where a file to [`write`] comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Data {
    /// A range of the archive passed to [`write`] as `base`.
    Base {
        offset: u64,
        size: u32,
    },
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// Writes an archive of `files` to `out`. `base` is the archive that
/// [`Data::Base`] ranges are in. `files` must not have two paths that
/// differ only in case, which the game couldn't tell apart.
pub fn write(out: &Path, base: Option<&Path>, files: &[(String, Data)]) -> io::Result<()> {
    let mut order: Vec<usize> = (0..files.len()).collect();
    order.sort_by_cached_key(|&i| (hash(&files[i].0), files[i].0.clone()));

    let mut paths = Vec::new();
    let mut path_offsets = vec![0u32; files.len()];
    for (i, (path, _)) in files.iter().enumerate() {
        path_offsets[i] = u32::try_from(paths.len())
            .ok()
            .filter(|&o| o & !0x00ff_ffff == 0)
            .ok_or_else(|| invalid("too many files for one archive"))?;
        paths.extend_from_slice(path.as_bytes());
        paths.push(0);
    }

    let index_end =
        u64::from(HEADER_SIZE) + u64::from(ENTRY_SIZE) * files.len() as u64 + paths.len() as u64;
    let mut sizes = Vec::with_capacity(files.len());
    for (path, data) in files {
        let size = match data {
            Data::Base { size, .. } => u64::from(*size),
            Data::File(p) => p.metadata()?.len(),
            Data::Bytes(b) => b.len() as u64,
        };
        sizes.push(u32::try_from(size).map_err(|_| invalid(format!("{path} is over 4 GiB")))?);
    }
    let mut offsets = Vec::with_capacity(files.len());
    let mut at = index_end.next_multiple_of(ALIGN);
    for size in &sizes {
        offsets.push(u32::try_from(at).map_err(|_| invalid("the archive would be over 4 GiB"))?);
        at = (at + u64::from(*size)).next_multiple_of(ALIGN);
    }

    let mut w = BufWriter::with_capacity(1 << 20, File::create(out)?);
    w.write_all(MAGIC)?;
    w.write_all(&HEADER_SIZE.to_be_bytes())?;
    w.write_all(&ENTRY_SIZE.to_be_bytes())?;
    let count = u32::try_from(files.len()).map_err(|_| invalid("too many files"))?;
    w.write_all(&count.to_be_bytes())?;
    let paths_len = u32::try_from(paths.len()).map_err(|_| invalid("paths too long"))?;
    w.write_all(&paths_len.to_be_bytes())?;
    for &i in &order {
        for field in [
            hash(&files[i].0),
            path_offsets[i],
            offsets[i],
            sizes[i],
            sizes[i],
        ] {
            w.write_all(&field.to_be_bytes())?;
        }
    }
    w.write_all(&paths)?;

    let mut base = base.map(File::open).transpose()?;
    let mut pos = index_end;
    for (i, (path, data)) in files.iter().enumerate() {
        let start = u64::from(offsets[i]);
        io::copy(&mut io::repeat(0).take(start - pos), &mut w)?;
        let written = match data {
            Data::Base { offset, size } => {
                let base = base.as_mut().ok_or_else(|| {
                    invalid(format!("{path} is from a base archive, but none was given"))
                })?;
                base.seek(SeekFrom::Start(*offset))?;
                io::copy(&mut base.take(u64::from(*size)), &mut w)?
            }
            Data::File(p) => io::copy(&mut File::open(p)?.take(u64::from(sizes[i])), &mut w)?,
            Data::Bytes(b) => {
                w.write_all(b)?;
                b.len() as u64
            }
        };
        if written != u64::from(sizes[i]) {
            return Err(invalid(format!("{path} changed size while being written")));
        }
        pos = start + written;
    }
    w.into_inner().map_err(|e| e.into_error())?.sync_all()
}

/// Reads one file's bytes out of an archive.
pub fn read_entry(archive: &mut (impl Read + Seek), offset: u64, size: u32) -> io::Result<Vec<u8>> {
    archive.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0; size as usize];
    archive.read_exact(&mut buf)?;
    Ok(buf)
}
