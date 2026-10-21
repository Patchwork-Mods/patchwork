//! Staged resources: where each one's data comes from, reading it back so
//! a later mod can change it, and packing them into an archive.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use rpkg_rs::resource::package_builder::{PackageBuilder, PackageResourceBuilder};
use rpkg_rs::resource::pdefs::PartitionId;
use rpkg_rs::resource::resource_info::ResourceInfo;
use rpkg_rs::resource::resource_package::{
    PackageVersion, ResourcePackage, ResourceReferenceFlags, ResourceReferenceFlagsStandard,
};
use rpkg_rs::resource::resource_partition::PatchId;
use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;

use super::meta::Meta;

/// LZ4 HC level for resources that are compressed here rather than copied.
/// Low, since building happens on every deploy and a better ratio barely
/// matters.
pub const COMPRESSION_LEVEL: i32 = 4;

/// Where a staged resource's data comes from.
pub enum Resource {
    /// A raw resource from a mod, like `00123456789ABCDE.TEMP`.
    File {
        path: PathBuf,
        resource_type: String,
        meta: Option<Meta>,
    },
    /// A resource in an archive: from a plain RPKG mod, or the game's own.
    Packaged {
        archive: PathBuf,
        info: ResourceInfo,
    },
    /// A resource made while deploying, such as a patched JSON file.
    Built(Loaded),
}

/// What a resource's entry in an archive says besides its data.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// Referenced resources and their flags, in order.
    pub references: Vec<(RuntimeResourceID, u8)>,
    pub compressed: bool,
    pub scrambled: bool,
    pub system_memory: u32,
    pub video_memory: u32,
}

impl Header {
    /// What RPKG Tool uses for a raw resource without a meta file.
    pub fn default_for(size: u32) -> Self {
        Self {
            references: Vec::new(),
            compressed: true,
            scrambled: true,
            system_memory: size,
            video_memory: u32::MAX,
        }
    }

    fn of(info: &ResourceInfo) -> Self {
        Self {
            references: info
                .references()
                .iter()
                .map(|(rrid, flags)| (*rrid, flags.to_standard().into_bits()))
                .collect(),
            compressed: info.is_compressed(),
            scrambled: info.is_scrambled(),
            system_memory: info.system_memory_requirement(),
            video_memory: info.video_memory_requirement(),
        }
    }

    /// Adds a reference unless it's there already.
    pub fn reference(&mut self, rrid: RuntimeResourceID, flag: u8) {
        if !self.references.iter().any(|(r, _)| *r == rrid) {
            self.references.push((rrid, flag));
        }
    }
}

/// A resource read into memory.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub resource_type: String,
    pub data: Vec<u8>,
    pub header: Header,
}

impl Loaded {
    /// The same resource with different data, its memory needs scaled to
    /// match.
    pub fn with_data(self, data: Vec<u8>) -> Self {
        let system_memory = scale_memory(
            self.header.system_memory,
            self.data.len() as u32,
            data.len() as u32,
        );
        Self {
            data,
            header: Header {
                system_memory,
                ..self.header
            },
            ..self
        }
    }
}

/// Scales the memory a resource of `original` bytes needed to one of `size`
/// bytes, the way RPKG Tool does.
pub fn scale_memory(memory: u32, original: u32, size: u32) -> u32 {
    if memory == u32::MAX || original == 0 {
        return memory;
    }
    let ratio = f64::from(memory) / f64::from(original);
    let scaled = (f64::from(size) * ratio) as u32;
    let rounding = memory.wrapping_sub((f64::from(original) * ratio) as u32);
    scaled.wrapping_add(rounding)
}

/// Archives opened for reading, kept so each is only parsed once.
#[derive(Default)]
pub struct Archives {
    open: HashMap<PathBuf, ResourcePackage>,
}

impl Archives {
    fn get(&mut self, path: &Path) -> Result<&ResourcePackage, String> {
        if !self.open.contains_key(path) {
            let package = read_package(path).map_err(|e| format!("{}: {e}", path.display()))?;
            self.open.insert(path.to_owned(), package);
        }
        Ok(&self.open[path])
    }

    /// Reads a staged resource back.
    pub fn load(&mut self, resource: &Resource) -> Result<Loaded, String> {
        match resource {
            Resource::File {
                path,
                resource_type,
                meta,
            } => {
                let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
                let header = match meta {
                    Some(meta) => meta.header(data.len() as u32),
                    None => Header::default_for(data.len() as u32),
                };
                Ok(Loaded {
                    resource_type: resource_type.clone(),
                    data,
                    header,
                })
            }
            Resource::Packaged { archive, info } => {
                let package = self.get(archive)?;
                load_packaged(package, info).map_err(|e| format!("{}: {e}", archive.display()))
            }
            Resource::Built(loaded) => Ok(loaded.clone()),
        }
    }

    /// Packs `resources` into a new archive at `out`.
    pub fn write(
        &mut self,
        partition: PartitionId,
        patch: PatchId,
        resources: BTreeMap<u64, Resource>,
        out: &Path,
    ) -> Result<(), String> {
        let mut builder = PackageBuilder::new_with_patch_id(partition, patch);
        for (hash, resource) in resources {
            let rrid = RuntimeResourceID::from(hash);
            let resource = match resource {
                Resource::File {
                    path,
                    resource_type,
                    meta,
                } => file_resource(rrid, &path, &resource_type, meta.as_ref())?,
                Resource::Packaged { archive, info } => self.copy(&archive, info)?,
                Resource::Built(loaded) => built_resource(rrid, loaded)?,
            };
            builder.with_resource(resource);
        }
        builder
            .build_to_file(PackageVersion::RPKGv2, out)
            .map_err(|e| format!("couldn't write {}: {e}", out.display()))
    }

    fn copy(
        &mut self,
        archive: &Path,
        info: ResourceInfo,
    ) -> Result<PackageResourceBuilder, String> {
        let err = |e: &dyn std::fmt::Display| format!("{}: {e}", archive.display());
        if info.is_compressed() {
            // Copied as it's stored, without decompressing and compressing
            // it again.
            let size = info.compressed_size().unwrap_or(info.size());
            let mut data = vec![0; size as usize];
            let mut file = fs::File::open(archive).map_err(|e| err(&e))?;
            file.seek(SeekFrom::Start(info.data_offset()))
                .map_err(|e| err(&e))?;
            file.read_exact(&mut data).map_err(|e| err(&e))?;
            return PackageResourceBuilder::from_resource_info(info, data, true)
                .map_err(|e| err(&e));
        }
        // rpkg-rs would take stored data that isn't compressed to be, so it's
        // read out (descrambled) and stored again instead.
        let data = self
            .get(archive)?
            .read_resource(info.rrid())
            .map_err(|e| err(&e))?;
        PackageResourceBuilder::from_resource_info(info, data, false).map_err(|e| err(&e))
    }
}

/// Reads a resource out of an archive that's already open.
pub fn load_packaged(package: &ResourcePackage, info: &ResourceInfo) -> Result<Loaded, String> {
    let data = package
        .read_resource(info.rrid())
        .map_err(|e| e.to_string())?;
    Ok(Loaded {
        resource_type: info.data_type(),
        data,
        header: Header::of(info),
    })
}

/// Reads an archive's resource table. rpkg-rs goes by the file name to tell
/// patch archives (which have a list of removed resources) from base ones,
/// but RPKG mods are often renamed, so the other layout is tried too.
pub fn read_package(path: &Path) -> Result<ResourcePackage, String> {
    match ResourcePackage::from_file(path) {
        Ok(package) => Ok(package),
        Err(e) => {
            let named_patch = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains("patch"));
            let data = fs::read(path).map_err(|e| e.to_string())?;
            ResourcePackage::from_memory(data, !named_patch).map_err(|_| e.to_string())
        }
    }
}

fn with_header(resource: &mut PackageResourceBuilder, header: &Header) {
    resource.with_memory_requirements(header.system_memory, header.video_memory);
    for (reference, flag) in &header.references {
        resource.with_reference(
            *reference,
            ResourceReferenceFlags::Standard(ResourceReferenceFlagsStandard::from_bits(*flag)),
        );
    }
}

fn file_resource(
    rrid: RuntimeResourceID,
    path: &Path,
    resource_type: &str,
    meta: Option<&Meta>,
) -> Result<PackageResourceBuilder, String> {
    // Without a meta file RPKG Tool compresses and scrambles everything.
    let (compressed, scrambled) = meta.map_or((true, true), |m| (m.compressed, m.scrambled));
    let mut resource = PackageResourceBuilder::from_file(
        rrid,
        resource_type,
        path,
        compressed.then_some(COMPRESSION_LEVEL),
        scrambled,
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(meta) = meta {
        let size = fs::metadata(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .len();
        let size = u32::try_from(size).map_err(|_| format!("{} is too large", path.display()))?;
        with_header(&mut resource, &meta.header(size));
    }
    Ok(resource)
}

fn built_resource(
    rrid: RuntimeResourceID,
    loaded: Loaded,
) -> Result<PackageResourceBuilder, String> {
    let Loaded {
        resource_type,
        data,
        header,
    } = loaded;
    let mut resource = PackageResourceBuilder::from_memory(
        rrid,
        &resource_type,
        data,
        header.compressed.then_some(COMPRESSION_LEVEL),
        header.scrambled,
    )
    .map_err(|e| format!("{rrid}: {e}"))?;
    with_header(&mut resource, &header);
    Ok(resource)
}
