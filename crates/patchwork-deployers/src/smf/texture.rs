//! Textures, which mods give as a TGA (`.texture.tga`) for SMF to convert
//! with HMTextureTools into the game's TEXT resource and, for larger
//! textures, a TEXD holding the higher-resolution mips. glacier-texture does
//! the converting here.
//!
//! The file is named after the resources to make: `<TEXT>.texture.tga` for
//! a TEXT on its own, the TGA used as its only mip, or
//! `<TEXT>~<TEXD>.texture.tga` for both, with a full set of mips split
//! between them. Next to it, a `.texture.tga.meta` from HMTextureTools says
//! how to encode it.

use std::io::Cursor;

use glacier_texture::GlacierGame;
use glacier_texture::enums::{RenderFormat, RenderFormatMapping, TextureType, WoaRenderFormat};
use glacier_texture::pack::{TextureMapBuilder, TextureMapParameters};
use glacier_texture::texture_map::TextureMap;
use rpkg_rs::resource::runtime_resource_id::RuntimeResourceID;
use rpkg_rs::{GlacierResource, WoaVersion};

use super::json;
use super::meta;
use super::resource::{Header, Loaded};

/// A texture's resources: the TEXT, and the TEXD if it has one.
pub struct Resources {
    pub text: (u64, Loaded),
    pub texd: Option<(u64, Loaded)>,
}

/// The TEXT and TEXD hashes a texture's file name gives, from its stem
/// (everything before `.texture.tga`).
pub fn parse_name(stem: &str) -> Option<(u64, Option<u64>)> {
    let hash = |s: &str| meta::parse_hex_hash(s).map(u64::from);
    match stem.split_once('~') {
        Some((text, texd)) => Some((hash(text)?, Some(hash(texd)?))),
        None => Some((hash(stem)?, None)),
    }
}

/// HMTextureTools' meta for Hitman 3: its `H3::Meta` struct as it lies in
/// memory, padding included.
struct TextureMeta {
    texture_type: TextureType,
    flags: u32,
    format: RenderFormat,
    /// Whether each mip is LZ4 compressed.
    compressed: bool,
}

const META_LEN: usize = 16;
// Texture flags glacier-texture lets be set, as HMTextureTools copies them
// into the TEXT from the meta. LZ4 compression goes by `compressed` instead.
const DEFERRED: u32 = 0x2;
const UNKNOWN1: u32 = 0x8;

impl TextureMeta {
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() != META_LEN {
            return Err(format!(
                "the texture's meta is {} bytes rather than {META_LEN}; is it from \
                 HMTextureTools for Hitman 3?",
                bytes.len()
            ));
        }
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
        let texture_type = match u16_at(0) {
            0 => TextureType::Colour,
            1 => TextureType::Normal,
            2 => TextureType::Height,
            3 => TextureType::CompoundNormal,
            4 => TextureType::Billboard,
            other => return Err(format!("the texture's meta has an unknown type {other}")),
        };
        let format = u16_at(8);
        Ok(Self {
            texture_type,
            flags: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            format: WoaRenderFormat::try_from_u16(format)
                .ok_or_else(|| format!("the texture's meta has an unknown format {format:#X}"))?,
            compressed: bytes[10] != 0,
        })
    }

    fn params(&self) -> TextureMapParameters {
        let mut params = TextureMapParameters::new(self.format);
        params.set_texture_type(self.texture_type);
        let mut flags = params.flags();
        flags.set_lz4_compression(self.compressed);
        flags.set_deferred(self.flags & DEFERRED != 0);
        flags.set_unknown1(self.flags & UNKNOWN1 != 0);
        params.set_flags(flags);
        params
    }
}

/// Converts a TGA into a texture's resources, as HMTextureTools would.
pub fn build(tga: &[u8], meta: &[u8], text: u64, texd: Option<u64>) -> Result<Resources, String> {
    let meta = TextureMeta::parse(meta)?;
    let texture = TextureMapBuilder::from_tga(Cursor::new(tga))
        .map_err(|e| format!("couldn't read the TGA: {e}"))?
        .with_params(meta.params())
        .with_mipblock1(texd.is_some())
        .build(GlacierGame::HM3)
        .map_err(|e| format!("couldn't convert the texture: {e}"))?;

    let data = <TextureMap as GlacierResource>::serialize(&texture, WoaVersion::HM3)
        .map_err(|e| format!("couldn't write the TEXT: {e}"))?;
    let video_memory = video_memory(&data)?;
    let mut header = Header {
        references: Vec::new(),
        compressed: false,
        scrambled: true,
        system_memory: u32::MAX,
        video_memory,
    };
    let texd = match texd {
        Some(hash) => {
            let mipblock = texture
                .mipblock1()
                .ok_or("glacier-texture made no TEXD for the texture")?;
            let data = mipblock
                .serialize(WoaVersion::HM3)
                .map_err(|e| format!("couldn't write the TEXD: {e}"))?;
            header
                .references
                .push((RuntimeResourceID::from(hash), json::LOOKUP_REFERENCE));
            let texd = Loaded {
                resource_type: "TEXD".to_owned(),
                data,
                header: Header {
                    references: Vec::new(),
                    compressed: false,
                    scrambled: false,
                    system_memory: u32::MAX,
                    video_memory,
                },
            };
            Some((hash, texd))
        }
        None => None,
    };
    let loaded = Loaded {
        resource_type: "TEXT".to_owned(),
        data,
        header,
    };
    Ok(Resources {
        text: (text, loaded),
        texd,
    })
}

/// The video memory the game gives a texture, going by its TEXT: the size
/// of the mips only its TEXD holds, uncompressed, or 0 if it has none. Its
/// TEXD gets the same. (glacier-texture's own figure is something else.)
fn video_memory(text: &[u8]) -> Result<u32, String> {
    const MIP_LEVELS: usize = 0x12;
    const MIP_SIZES: usize = 0x18;
    const TEXT_MIP_LEVELS: usize = 0x93;
    let byte = |at: usize| {
        text.get(at)
            .copied()
            .map(usize::from)
            .ok_or("the TEXT glacier-texture wrote is truncated")
    };
    let texd_only = byte(MIP_LEVELS)?.saturating_sub(byte(TEXT_MIP_LEVELS)?);
    if texd_only == 0 {
        return Ok(0);
    }
    let at = MIP_SIZES + (texd_only - 1) * 4;
    let size = text
        .get(at..at + 4)
        .ok_or("the TEXT glacier-texture wrote is truncated")?;
    // Mip sizes are cumulative.
    Ok(u32::from_le_bytes(size.try_into().unwrap()))
}
