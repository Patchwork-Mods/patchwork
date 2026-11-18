//! The `updates` task: now and then, fetches the versions of every mod in
//! every profile whose source lists them, so the app has update marks ready
//! when it opens, and keeps a list of what can be updated
//! ([`found`]). Nothing is ever updated or deployed: that's still up to
//! whoever's at the computer.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use jiff::{SignedDuration, Timestamp};
use patchwork_core::paths::Paths;
use patchwork_core::{KnownVersions, Patchwork};
use serde::{Deserialize, Serialize};

use crate::task::{Cx, Every, Job, Schedule};

/// How often it runs unless `daemon.toml` says.
pub const EVERY: SignedDuration = SignedDuration::from_hours(6);

pub struct Updates;

/// A mod that can move to a newer version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Update {
    pub profile: uuid::Uuid,
    /// The profile's label, as of the check.
    pub label: String,
    pub game: String,
    /// The mod's name in the profile.
    pub name: String,
    /// What it's pinned to or locked at.
    pub current: Option<String>,
    pub newer: String,
}

/// What the last check found.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub checked: Option<Timestamp>,
    pub updates: Vec<Update>,
    /// Mods whose versions couldn't be fetched, with why.
    #[serde(default)]
    pub failed: Vec<String>,
}

fn found_file(paths: &Paths) -> PathBuf {
    paths.daemon().join("tasks/updates/found.json")
}

/// What the last check found, if there's been one.
pub fn found(paths: &Paths) -> Found {
    // only a record, so one that can't be read is as good as none
    fs::read(found_file(paths))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_found(paths: &Paths, found: &Found) -> Result<()> {
    let path = found_file(paths);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    fs::write(&path, serde_json::to_vec(found)?).with_context(|| path.display().to_string())
}

impl Job for Updates {
    fn id(&self) -> &'static str {
        "updates"
    }

    fn description(&self) -> &'static str {
        "Automatic mod update checks"
    }

    fn default_enabled(&self) -> bool {
        true
    }

    fn schedule(&self) -> Box<dyn Schedule> {
        Box::new(Every(EVERY))
    }

    fn run<'a>(&'a self, pw: &'a Patchwork, cx: &'a Cx) -> BoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let before = found(cx.paths());
            let found = check(pw).await?;
            let new: Vec<_> = found
                .updates
                .iter()
                .filter(|u| !before.updates.contains(u))
                .collect();
            match new.len() {
                0 => {}
                1 => cx.record(format!(
                    "{}: {} can be updated to {}",
                    new[0].label, new[0].name, new[0].newer
                )),
                n => cx.record(format!("{n} mods can be updated")),
            }
            if !found.failed.is_empty() {
                cx.record_error(format!(
                    "couldn't check {} mod(s), such as {}",
                    found.failed.len(),
                    found.failed[0]
                ));
            }
            save_found(cx.paths(), &found)?;
            Ok(!new.is_empty())
        })
    }
}

/// Fetches the versions of each profile's mods, one at a time to go easy
/// on sources' limits, skipping any fetched lately (by the app, say). If
/// the daemon's paused partway, what's been fetched is kept, and the check
/// starts again once it picks up.
async fn check(pw: &Patchwork) -> Result<Found> {
    let since = Timestamp::now() - KnownVersions::TTL;
    let mut found = Found {
        checked: Some(Timestamp::now()),
        ..Found::default()
    };
    let mut fetched = HashSet::new();
    for (profile, loaded) in pw.profiles()? {
        let label = loaded.label();
        let Ok(lookup) = pw.lookup(&loaded) else {
            continue;
        };
        let lock = pw.load_lock(&profile).ok().flatten();
        for m in &loaded.mods {
            if !pw.lists_versions(&m.source).unwrap_or(false) {
                continue;
            }
            let key = (lookup.clone(), m.source.clone(), m.id.clone());
            if fetched.insert(key)
                && let Err(e) = pw.versions_since(&lookup, &m.source, &m.id, since).await
            {
                found.failed.push(format!("{} in {label}: {e}", m.name));
                continue;
            }
            if let Some(newer) = pw.known_update(&lookup, m, lock.as_ref()) {
                let current = m.version.clone().or_else(|| {
                    let key = patchwork_core::lock::mod_key(&m.source, &m.id);
                    lock.as_ref()?
                        .mods
                        .iter()
                        .find(|l| l.has_key(&key))?
                        .version
                        .clone()
                });
                found.updates.push(Update {
                    profile: profile.uuid,
                    label: label.clone(),
                    game: loaded.game.clone(),
                    name: m.name.clone(),
                    current,
                    newer,
                });
            }
        }
    }
    Ok(found)
}
