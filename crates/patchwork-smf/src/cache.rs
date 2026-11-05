//! Resources converted while building, kept in the deployer's cache so the
//! next build needn't convert them again.
//!
//! Patching entities and converting textures is most of the time a build
//! takes, and a whole build is only reused when nothing at all has changed
//! (and not after undeploying), so each conversion is kept on its own, by
//! everything it's made from. Anything wrong with the cache just means
//! converting again.

use std::fs;
use std::path::{Path, PathBuf};

use patchwork_core::hash::ContentHash;
use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;

use super::resource::{Header, Loaded};

/// Bumped whenever what's kept, or how it's made, changes, so earlier
/// conversions aren't used.
const VERSION: u32 = 1;
const MAGIC: &[u8; 4] = b"PWRC";

/// What a conversion is made from, hashed a field at a time so large
/// resources needn't be copied.
pub struct Key(Vec<u8>);

impl Key {
    /// A key for a kind of conversion, such as `entity`.
    pub fn new(kind: &str) -> Self {
        let mut key = Self(Vec::new());
        key.field(env!("CARGO_PKG_VERSION").as_bytes());
        key.field(&VERSION.to_le_bytes());
        key.field(kind.as_bytes());
        key
    }

    pub fn field(&mut self, bytes: &[u8]) -> &mut Self {
        self.0
            .extend_from_slice(ContentHash::of(bytes).to_string().as_bytes());
        self
    }

    pub fn hash(&mut self, hash: u64) -> &mut Self {
        self.field(&hash.to_le_bytes())
    }

    pub fn resource(&mut self, hash: u64, loaded: &Loaded) -> &mut Self {
        self.hash(hash)
            .field(loaded.resource_type.as_bytes())
            .field(&header(&loaded.header))
            .field(&loaded.data)
    }

    fn path(&self, dir: &Path) -> PathBuf {
        dir.join("resources")
            .join(format!("{}.bin", ContentHash::of(&self.0)))
    }
}

/// The resources kept in `dir` for `key`, if they're there and whole.
pub fn get(dir: &Path, key: &Key) -> Option<Vec<(u64, Loaded)>> {
    let bytes = fs::read(key.path(dir)).ok()?;
    let mut r = Reader(bytes.strip_prefix(MAGIC)?);
    if r.u32()? != VERSION {
        return None;
    }
    let count = r.u32()?;
    let mut resources = Vec::new();
    for _ in 0..count {
        let hash = r.u64()?;
        let resource_type = String::from_utf8(r.bytes()?.to_vec()).ok()?;
        let data = r.bytes()?.to_vec();
        let references = (0..r.u32()?)
            .map(|_| Some((RuntimeResourceID::from(r.u64()?), r.u8()?)))
            .collect::<Option<_>>()?;
        let header = Header {
            references,
            compressed: r.u8()? != 0,
            scrambled: r.u8()? != 0,
            system_memory: r.u32()?,
            video_memory: r.u32()?,
        };
        resources.push((
            hash,
            Loaded {
                resource_type,
                data,
                header,
            },
        ));
    }
    r.0.is_empty().then_some(resources)
}

/// Keeps `resources` in `dir` for `key`, written whole or not at all. Best
/// effort: if it can't be, it's only converted again next time.
pub fn put(dir: &Path, key: &Key, resources: &[(u64, &Loaded)]) {
    let path = key.path(dir);
    let Some(parent) = path.parent() else {
        return;
    };
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(resources.len() as u32).to_le_bytes());
    for (hash, loaded) in resources {
        out.extend_from_slice(&hash.to_le_bytes());
        put_bytes(&mut out, loaded.resource_type.as_bytes());
        put_bytes(&mut out, &loaded.data);
        out.extend_from_slice(&header(&loaded.header));
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let written = fs::create_dir_all(parent)
        .and_then(|()| fs::write(&tmp, &out))
        .and_then(|()| fs::rename(&tmp, &path));
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
}

/// A resource's header, as kept.
fn header(header: &Header) -> Vec<u8> {
    let Header {
        references,
        compressed,
        scrambled,
        system_memory,
        video_memory,
    } = header;
    let mut out = Vec::new();
    out.extend_from_slice(&(references.len() as u32).to_le_bytes());
    for (rrid, flag) in references {
        out.extend_from_slice(&u64::from(*rrid).to_le_bytes());
        out.push(*flag);
    }
    out.push(u8::from(*compressed));
    out.push(u8::from(*scrambled));
    out.extend_from_slice(&system_memory.to_le_bytes());
    out.extend_from_slice(&video_memory.to_le_bytes());
    out
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let (bytes, rest) = self.0.split_first_chunk::<N>()?;
        self.0 = rest;
        Some(*bytes)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take::<1>().map(|[b]| b)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_le_bytes)
    }

    fn u64(&mut self) -> Option<u64> {
        self.take().map(u64::from_le_bytes)
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = usize::try_from(self.u64()?).ok()?;
        let (bytes, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(bytes)
    }
}
