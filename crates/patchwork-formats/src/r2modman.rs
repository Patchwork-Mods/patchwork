//! r2modman's profile format, which Thunderstore Mod Manager and Gale also
//! understand: a zip (`.r2z`) holding `export.r2x`, a YAML list of
//! Thunderstore packages, alongside the profile's config files.
//!
//! A Thunderstore profile code is the same zip, base64-encoded and uploaded
//! to Thunderstore, which hands back a code to share.
use std::collections::{HashMap, HashSet};
use std::io::{Cursor, Write};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use patchwork_core::Result;
use patchwork_core::game::GameDef;
use patchwork_core::lock::{Lockfile, mod_key};
use patchwork_core::profile::ProfileMod;
use patchwork_core::share::{FormatContext, ProfileFormat, SharedProfile};
use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

use crate::{read_zip_entry, zip_contains};

const SOURCE: &str = "thunderstore";
const MANIFEST: &str = "export.r2x";
const CODE_HEADER: &str = "#r2modman";
const API: &str = "https://thunderstore.io/api/experimental/legacyprofile";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    profile_name: String,
    #[serde(default)]
    mods: Vec<ManifestMod>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestMod {
    /// `Namespace-Name`, which is also patchwork's Thunderstore id.
    name: String,
    version: ManifestVersion,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Serialize, Deserialize)]
struct ManifestVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl ManifestVersion {
    fn parse(version: &str) -> Option<Self> {
        let mut parts = version.split('.').map(|p| p.parse().ok());
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) => Some(Self {
                major,
                minor,
                patch,
            }),
            _ => None,
        }
    }
}

fn on_thunderstore(game: &GameDef) -> bool {
    game.sources.contains_key(SOURCE)
}

/// An r2modman profile file (`.r2z`).
pub struct R2modman;

#[async_trait]
impl ProfileFormat for R2modman {
    fn id(&self) -> &str {
        "r2modman"
    }

    fn description(&self) -> &str {
        "an r2modman profile file (.r2z), for Thunderstore games"
    }

    fn extension(&self) -> Option<&str> {
        Some("r2z")
    }

    fn supports(&self, game: &GameDef) -> bool {
        on_thunderstore(game)
    }

    fn needs_lock(&self) -> bool {
        true
    }

    fn detect(&self, input: &[u8]) -> bool {
        zip_contains(input, MANIFEST)
    }

    async fn export(&self, ctx: &FormatContext<'_>, shared: &SharedProfile) -> Result<Vec<u8>> {
        to_r2z(self.id(), ctx, shared)
    }

    async fn import(&self, ctx: &FormatContext<'_>, input: &[u8]) -> Result<SharedProfile> {
        from_r2z(self.id(), ctx, input)
    }
}

/// A Thunderstore profile code, as made by r2modman's "export as code".
/// Exporting uploads the profile to Thunderstore.
pub struct ThunderstoreCode {
    client: reqwest::Client,
}

impl Default for ThunderstoreCode {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(concat!("patchwork/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client should build");
        Self { client }
    }
}

#[derive(Deserialize)]
struct Created {
    key: String,
}

#[async_trait]
impl ProfileFormat for ThunderstoreCode {
    fn id(&self) -> &str {
        "thunderstore-code"
    }

    fn description(&self) -> &str {
        "a Thunderstore profile code (uploads the profile to Thunderstore)"
    }

    fn extension(&self) -> Option<&str> {
        None
    }

    fn supports(&self, game: &GameDef) -> bool {
        on_thunderstore(game)
    }

    fn needs_lock(&self) -> bool {
        true
    }

    fn detect(&self, input: &[u8]) -> bool {
        std::str::from_utf8(input).is_ok_and(|text| is_code(text.trim()))
    }

    async fn export(&self, ctx: &FormatContext<'_>, shared: &SharedProfile) -> Result<Vec<u8>> {
        let r2z = to_r2z(self.id(), ctx, shared)?;
        let url = format!("{API}/create/");
        let reqwest_error = |result| patchwork_core::Error::Reqwest {
            url: url.clone(),
            result,
        };
        let created: Created = self
            .client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(encode_code_payload(&r2z))
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(reqwest_error)?
            .json()
            .await
            .map_err(reqwest_error)?;
        Ok(format!("{}\n", created.key).into_bytes())
    }

    async fn import(&self, ctx: &FormatContext<'_>, input: &[u8]) -> Result<SharedProfile> {
        let code = std::str::from_utf8(input).unwrap_or_default().trim();
        if !is_code(code) {
            return Err(crate::error(
                self.id(),
                format!("`{code}` isn't a profile code"),
            ));
        }
        let url = format!("{API}/get/{code}/");
        let reqwest_error = |result| patchwork_core::Error::Reqwest {
            url: url.clone(),
            result,
        };
        let payload = self
            .client
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(reqwest_error)?
            .text()
            .await
            .map_err(reqwest_error)?;
        let r2z = decode_code_payload(&payload).map_err(|e| crate::error(self.id(), e))?;
        from_r2z(self.id(), ctx, &r2z)
    }
}

/// Codes are UUIDs, but anything code-shaped is passed along to Thunderstore
/// to decide.
fn is_code(text: &str) -> bool {
    (8..=64).contains(&text.len()) && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn encode_code_payload(r2z: &[u8]) -> String {
    format!("{CODE_HEADER}\n{}", BASE64.encode(r2z))
}

fn decode_code_payload(payload: &str) -> Result<Vec<u8>, String> {
    let data = payload
        .trim()
        .strip_prefix(CODE_HEADER)
        .ok_or("Thunderstore didn't send back an r2modman profile")?;
    let data: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    BASE64
        .decode(data)
        .map_err(|e| format!("bad profile data from Thunderstore: {e}"))
}

fn to_r2z(format_id: &str, ctx: &FormatContext<'_>, shared: &SharedProfile) -> Result<Vec<u8>> {
    let error = |message: String| crate::error(format_id, message);
    let lock = shared
        .lock
        .as_ref()
        .ok_or_else(|| error("the profile needs locking first".to_owned()))?;
    let enabled = enabled_keys(&shared.mods, lock);
    let mut mods = Vec::new();
    for locked in &lock.mods {
        if locked.source != SOURCE {
            ctx.warn(format!(
                "{} isn't from Thunderstore, so r2modman can't install it; skipped",
                locked.name
            ));
            continue;
        }
        let Some(version) = locked.version.as_deref().and_then(ManifestVersion::parse) else {
            ctx.warn(format!("{} has no usable version; skipped", locked.name));
            continue;
        };
        mods.push(ManifestMod {
            name: locked.id.clone(),
            version,
            enabled: enabled.contains(&locked.key()),
        });
    }
    let manifest = Manifest {
        profile_name: shared.name.clone().unwrap_or_default(),
        mods,
    };
    let yaml = serde_norway::to_string(&manifest).map_err(|e| error(e.to_string()))?;

    let zip_error = |e: zip::result::ZipError| error(e.to_string());
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(MANIFEST, SimpleFileOptions::default())
        .map_err(zip_error)?;
    zip.write_all(yaml.as_bytes())
        .map_err(|e| error(e.to_string()))?;
    Ok(zip.finish().map_err(zip_error)?.into_inner())
}

/// r2modman lists dependencies as mods in their own right, so a dependency
/// is enabled if anything enabled needs it.
fn enabled_keys(mods: &[ProfileMod], lock: &Lockfile) -> HashSet<String> {
    let deps: HashMap<String, &[String]> = lock
        .mods
        .iter()
        .map(|m| (m.key(), m.dependencies.as_slice()))
        .collect();
    let mut enabled = HashSet::new();
    let mut stack: Vec<String> = mods
        .iter()
        .filter(|m| m.enabled)
        .map(|m| mod_key(&m.source, &m.id))
        .collect();
    while let Some(key) = stack.pop() {
        if let Some(needs) = deps.get(&key) {
            stack.extend(needs.iter().cloned());
        }
        enabled.insert(key);
    }
    enabled
}

fn from_r2z(format_id: &str, ctx: &FormatContext<'_>, input: &[u8]) -> Result<SharedProfile> {
    let error = |message: String| crate::error(format_id, message);
    let mut zip = zip::ZipArchive::new(Cursor::new(input)).map_err(|e| error(e.to_string()))?;
    let text = read_zip_entry(&mut zip, MANIFEST)
        .map_err(error)?
        .ok_or_else(|| error(format!("not an r2modman profile (no {MANIFEST})")))?;
    let manifest: Manifest =
        serde_norway::from_str(&text).map_err(|e| error(format!("{MANIFEST}: {e}")))?;

    let configs = zip
        .file_names()
        .filter(|name| *name != MANIFEST && !name.ends_with('/'))
        .count();
    if configs > 0 {
        ctx.warn(format!(
            "skipped {configs} config file{}; patchwork doesn't manage configs yet",
            if configs == 1 { "" } else { "s" }
        ));
    }

    let mods = manifest
        .mods
        .into_iter()
        .map(|m| {
            let ManifestVersion {
                major,
                minor,
                patch,
            } = m.version;
            ProfileMod {
                name: m.name.clone(),
                source: SOURCE.to_owned(),
                id: m.name,
                version: Some(format!("{major}.{minor}.{patch}")),
                enabled: m.enabled,
                priority: 0,
                game_versions: None,
                ext: Default::default(),
                replaces: Vec::new(),
                ignores: Vec::new(),
            }
        })
        .collect();
    Ok(SharedProfile {
        name: Some(manifest.profile_name),
        mods,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests;
