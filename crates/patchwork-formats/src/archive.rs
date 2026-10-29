use std::fs::{self, File};
use std::io::{self, Cursor, Write};
use std::path::Path;

use async_trait::async_trait;
use patchwork_core::Result;
use patchwork_core::hash::ContentHash;
use patchwork_core::lock::Lockfile;
use patchwork_core::profile::{PROFILE_SCHEMA, Profile};
use patchwork_core::relpath::RelPath;
use patchwork_core::share::{Bundled, FormatContext, ProfileFormat, SharedProfile};
use uuid::Uuid;
use zip::write::SimpleFileOptions;

use crate::{read_zip_entry, zip_contains};

const ID: &str = "patchwork";
const PROFILE: &str = "profile.toml";
const LOCK: &str = "profile.lock";
const FILES: &str = "files/";

fn error(message: impl Into<String>) -> patchwork_core::Error {
    crate::error(ID, message)
}

/// patchwork's own format: a zip holding the profile, its lockfile and the
/// files of any local mods, so the exact setup can be reproduced elsewhere.
///
/// - `profile.toml`, `profile.lock`: as they are on disk
/// - `files/<hash>/<name>`: each local mod, keyed by its locked hash
pub struct Archive;

#[async_trait]
impl ProfileFormat for Archive {
    fn id(&self) -> &str {
        ID
    }

    fn description(&self) -> &str {
        "a zip with the profile, its lockfile and any local mods"
    }

    fn extension(&self) -> Option<&str> {
        Some("zip")
    }

    fn needs_lock(&self) -> bool {
        true
    }

    fn detect(&self, input: &[u8]) -> bool {
        zip_contains(input, PROFILE)
    }

    async fn export(&self, _ctx: &FormatContext<'_>, shared: &SharedProfile) -> Result<Vec<u8>> {
        let profile = Profile {
            schema: PROFILE_SCHEMA,
            id: shared.id.unwrap_or_else(Uuid::new_v4),
            game: shared.game.clone().unwrap_or_default(),
            name: shared.name.clone().unwrap_or_default(),
            hidden_tags: shared.hidden_tags.clone().unwrap_or_default(),
            env: Default::default(),
            mods: shared.mods.clone(),
            ext: shared.ext.clone().unwrap_or_default(),
        };
        let zip_error = |e: zip::result::ZipError| error(e.to_string());
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default().large_file(true);

        let text = toml::to_string_pretty(&profile).map_err(|e| error(e.to_string()))?;
        zip.start_file(PROFILE, options).map_err(zip_error)?;
        zip.write_all(text.as_bytes())
            .map_err(|e| error(e.to_string()))?;
        if let Some(lock) = &shared.lock {
            let text = toml::to_string_pretty(lock).map_err(|e| error(e.to_string()))?;
            zip.start_file(LOCK, options).map_err(zip_error)?;
            zip.write_all(text.as_bytes())
                .map_err(|e| error(e.to_string()))?;
        }

        for (hash, bundled) in &shared.bundled {
            let prefix = format!("{FILES}{hash}/{}", bundled.name);
            if bundled.path.is_dir() {
                for entry in walkdir::WalkDir::new(&bundled.path).sort_by_file_name() {
                    let entry = entry.map_err(|e| error(e.to_string()))?;
                    if !entry.file_type().is_file() {
                        continue;
                    }
                    let rel = entry
                        .path()
                        .strip_prefix(&bundled.path)
                        .expect("walkdir yields paths under its root")
                        .to_string_lossy()
                        .replace('\\', "/");
                    zip.start_file(format!("{prefix}/{rel}"), options)
                        .map_err(zip_error)?;
                    copy_into(entry.path(), &mut zip)?;
                }
            } else {
                zip.start_file(prefix, options).map_err(zip_error)?;
                copy_into(&bundled.path, &mut zip)?;
            }
        }
        Ok(zip.finish().map_err(zip_error)?.into_inner())
    }

    async fn import(&self, ctx: &FormatContext<'_>, input: &[u8]) -> Result<SharedProfile> {
        let mut zip = zip::ZipArchive::new(Cursor::new(input)).map_err(|e| error(e.to_string()))?;
        let profile: Profile = toml::from_str(
            &read_zip_entry(&mut zip, PROFILE)
                .map_err(error)?
                .ok_or_else(|| error(format!("not a patchwork profile (no {PROFILE})")))?,
        )
        .map_err(|e| error(format!("{PROFILE}: {e}")))?;
        if profile.schema > PROFILE_SCHEMA {
            return Err(error(format!(
                "the profile uses schema {}, but this build only supports up to {PROFILE_SCHEMA}; update patchwork",
                profile.schema
            )));
        }
        let lock: Option<Lockfile> = read_zip_entry(&mut zip, LOCK)
            .map_err(error)?
            .map(|text| toml::from_str(&text))
            .transpose()
            .map_err(|e| error(format!("{LOCK}: {e}")))?;

        let mut shared = SharedProfile {
            game: Some(profile.game),
            name: Some(profile.name),
            id: Some(profile.id),
            mods: profile.mods,
            hidden_tags: Some(profile.hidden_tags),
            ext: Some(profile.ext),
            lock,
            ..Default::default()
        };
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(|e| error(e.to_string()))?;
            let Some(rest) = entry.name().strip_prefix(FILES) else {
                continue;
            };
            if entry.is_dir() {
                continue;
            }
            let rel = RelPath::new(rest).map_err(|e| error(format!("bad entry: {e}")))?;
            let mut parts = rel.as_str().splitn(3, '/');
            let (Some(hash), Some(name)) = (parts.next(), parts.next()) else {
                return Err(error(format!("bad entry: {}", entry.name())));
            };
            let hash: ContentHash = hash
                .parse()
                .map_err(|_| error(format!("bad entry: {}", entry.name())))?;
            shared.bundled.entry(hash).or_insert_with(|| Bundled {
                path: ctx.scratch.join(hash.to_string()).join(name),
                name: name.to_owned(),
            });

            let target = rel.under(ctx.scratch);
            if let Some(dir) = target.parent() {
                fs::create_dir_all(dir).map_err(|e| error(format!("{}: {e}", dir.display())))?;
            }
            let mut out =
                File::create(&target).map_err(|e| error(format!("{}: {e}", target.display())))?;
            io::copy(&mut entry, &mut out)
                .map_err(|e| error(format!("{}: {e}", target.display())))?;
        }
        Ok(shared)
    }
}

fn copy_into(path: &Path, out: &mut impl Write) -> Result<()> {
    let mut file = File::open(path).map_err(|e| error(format!("{}: {e}", path.display())))?;
    io::copy(&mut file, out).map_err(|e| error(format!("{}: {e}", path.display())))?;
    Ok(())
}
