//! What can be found out about a game install without the user saying: which
//! version of the game it is, and which build of it Steam has. Mods made for
//! one version of a game don't always work with another, so this is what
//! notices when a game has updated underneath what's deployed.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::game::{GameDef, VersionProbe};
use crate::vdf;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameEnv {
    /// From the first of the game definition's version probes that found
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Steam's id for the build that's installed, for Steam installs. It
    /// changes with every update, even ones that leave `version` alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_build: Option<String>,
}

impl GameEnv {
    /// Looks at the install of `game` in `root`. Anything that can't be
    /// read is left unknown rather than being an error, since a game that
    /// can't be identified can still have mods deployed to it.
    pub fn detect(game: &GameDef, root: &Path) -> Self {
        Self {
            version: game
                .detect
                .version
                .iter()
                .find_map(|probe| probe_version(probe, root)),
            steam_build: game
                .detect
                .steam
                .iter()
                .find_map(|app| steam_build(root, *app)),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl fmt::Display for GameEnv {
    /// `1.6.1170 (Steam build 13190634)`, or as much of it as is known.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.version, &self.steam_build) {
            (Some(v), Some(b)) => write!(f, "{v} (Steam build {b})"),
            (Some(v), None) => f.write_str(v),
            (None, Some(b)) => write!(f, "Steam build {b}"),
            (None, None) => f.write_str("unknown version"),
        }
    }
}

fn probe_version(probe: &VersionProbe, root: &Path) -> Option<String> {
    match probe {
        VersionProbe::Exe { file, parts } => {
            let numbers = exe_version(&file.under(root))?;
            let keep = parts.unwrap_or(4).clamp(1, 4);
            let kept: Vec<String> = numbers[..keep].iter().map(u16::to_string).collect();
            Some(kept.join("."))
        }
        VersionProbe::Json { file, key } => {
            let text = std::fs::read_to_string(file.under(root)).ok()?;
            let mut value: &serde_json::Value = &serde_json::from_str(&text).ok()?;
            for part in key.split('.') {
                value = value.get(part)?;
            }
            match value {
                serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_owned()),
                serde_json::Value::Number(n) => Some(n.to_string()),
                _ => None,
            }
        }
    }
}

/// The file version from a Windows executable's version resource, as its
/// four numbers. Reads only the headers and the resource section, since game
/// executables can be hundreds of megabytes.
pub fn exe_version(path: &Path) -> Option<[u16; 4]> {
    let mut file = File::open(path).ok()?;
    let read = |file: &mut File, at: u64, len: usize| -> Option<Vec<u8>> {
        let mut buf = vec![0; len];
        file.seek(SeekFrom::Start(at)).ok()?;
        file.read_exact(&mut buf).ok()?;
        Some(buf)
    };

    let dos = read(&mut file, 0, 64)?;
    if &dos[..2] != b"MZ" {
        return None;
    }
    let pe = u64::from(u32_at(&dos, 0x3c)?);
    let headers = read(&mut file, pe, 24)?;
    if &headers[..4] != b"PE\0\0" {
        return None;
    }
    let sections = usize::from(u16_at(&headers, 6)?);
    let optional_len = usize::from(u16_at(&headers, 20)?);
    let optional = read(&mut file, pe + 24, optional_len)?;
    // The data directories are where they are in PE32 and PE32+ headers.
    let (count_at, dirs_at) = match u16_at(&optional, 0)? {
        0x10b => (92, 96),
        0x20b => (108, 112),
        _ => return None,
    };
    const RESOURCE_DIR: u32 = 2;
    if u32_at(&optional, count_at)? <= RESOURCE_DIR {
        return None;
    }
    let rsrc_rva = u32_at(&optional, dirs_at + 8 * RESOURCE_DIR as usize)?;
    if rsrc_rva == 0 {
        return None;
    }

    let table = read(&mut file, pe + 24 + optional_len as u64, sections * 40)?;
    let section = table.as_chunks::<40>().0.iter().find_map(|s| {
        let va = u32_at(s, 12)?;
        let raw_len = u32_at(s, 16)?;
        let size = u32_at(s, 8)?.max(raw_len);
        (va <= rsrc_rva && rsrc_rva < va.checked_add(size)?).then_some((
            va,
            raw_len,
            u32_at(s, 20)?,
        ))
    })?;
    let (va, raw_len, raw_at) = section;
    // A resource section bigger than this is more likely a broken file.
    if raw_len > 256 << 20 {
        return None;
    }
    let data = read(&mut file, u64::from(raw_at), raw_len as usize)?;
    let base = (rsrc_rva - va) as usize;
    let at = |rva: u32| rva.checked_sub(va).map(|o| o as usize);

    // Type, then name, then language. Any name and language will do: games
    // only have the one version resource.
    const RT_VERSION: u32 = 16;
    let types = base;
    let names = base + subdir(&data, types, Some(RT_VERSION))?;
    let langs = base + subdir(&data, names, None)?;
    let entry = base + entry(&data, langs, None)?.0 as usize;
    let start = at(u32_at(&data, entry)?)?;
    let len = u32_at(&data, entry + 4)? as usize;
    let info = data.get(start..start.checked_add(len)?)?;

    // VS_FIXEDFILEINFO starts with this signature, after a header whose
    // length depends on padding, so it's found rather than worked out.
    const SIGNATURE: u32 = 0xfeef04bd;
    let fixed = (0..info.len().saturating_sub(16))
        .step_by(4)
        .find(|&i| u32_at(info, i) == Some(SIGNATURE))?;
    let ms = u32_at(info, fixed + 8)?;
    let ls = u32_at(info, fixed + 12)?;
    Some([(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16])
}

/// The offset (from the start of the resource section) that a resource
/// directory's entry points to, and whether that's another directory. `id`
/// picks the entry with that numeric id; `None` takes the first.
fn entry(data: &[u8], dir: usize, id: Option<u32>) -> Option<(u32, bool)> {
    let named = usize::from(u16_at(data, dir + 12)?);
    let ids = usize::from(u16_at(data, dir + 14)?);
    (0..named + ids).find_map(|i| {
        let at = dir + 16 + i * 8;
        let name = u32_at(data, at)?;
        let target = u32_at(data, at + 4)?;
        let matches = match id {
            Some(id) => name == id,
            None => true,
        };
        matches.then_some((target & 0x7fff_ffff, target & 0x8000_0000 != 0))
    })
}

fn subdir(data: &[u8], dir: usize, id: Option<u32>) -> Option<usize> {
    match entry(data, dir, id)? {
        (offset, true) => Some(offset as usize),
        (_, false) => None,
    }
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

/// The build id from Steam's manifest for `app`, if `root` is in a Steam
/// library (`<library>/steamapps/common/<game>`).
fn steam_build(root: &Path, app: u32) -> Option<String> {
    let steamapps = root.parent()?.parent()?;
    let manifest = steamapps.join(format!("appmanifest_{app}.acf"));
    let text = std::fs::read_to_string(manifest).ok()?;
    let build = vdf::parse(&text)?
        .get("AppState")?
        .get_str("buildid")?
        .to_owned();
    (!build.is_empty()).then_some(build)
}

#[cfg(test)]
mod tests;
