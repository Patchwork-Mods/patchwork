//! The `updates` task: now and then, fetches the versions of every mod in
//! every profile whose source lists them, so the app has update marks ready
//! when it opens, and keeps a list of what can be updated
//! ([`found`]). Nothing is ever updated or deployed: that's still up to
//! whoever's at the computer. New versions of mods the user follows (see
//! [`patchwork_core::follow`]) get a desktop notification.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use jiff::{SignedDuration, Timestamp};
use patchwork_core::follow::Follows;
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
    /// Left out by versions from before mods could be followed.
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub id: String,
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

    fn notifies(&self) -> Option<&'static str> {
        Some("Notify me about mods I follow")
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
            let follows = Follows::load(cx.paths()).unwrap_or_else(|e| {
                cx.record_error(format!("couldn't read the mods you follow: {e:#}"));
                Follows::default()
            });
            if let Some((summary, body)) = notification(&new, &follows) {
                cx.notify(&summary, &body).await;
            }
            Ok(!new.is_empty())
        })
    }
}

/// How many mods a notification names before it says "and n more".
const NAMED: usize = 3;

/// What to tell the user about `new` updates to mods they follow, if any
/// are: one notification for all of them, naming each mod once however
/// many profiles have it.
fn notification(new: &[&Update], follows: &Follows) -> Option<(String, String)> {
    let mut seen = HashSet::new();
    let mut mods: Vec<(&Update, Vec<&str>)> = Vec::new();
    for u in new.iter().filter(|u| follows.follows(&u.source, &u.id)) {
        if seen.insert((&u.source, &u.id, &u.newer)) {
            mods.push((u, vec![&u.label]));
        } else if let Some((_, labels)) = mods
            .iter_mut()
            .find(|(m, _)| (&m.source, &m.id, &m.newer) == (&u.source, &u.id, &u.newer))
        {
            labels.push(&u.label);
        }
    }
    match mods.as_slice() {
        [] => None,
        [(u, labels)] => Some((
            format!("{} {} is out", u.name, u.newer),
            format!("It can be updated in {}.", list(labels)),
        )),
        _ => {
            let names: Vec<&str> = mods.iter().map(|(u, _)| u.name.as_str()).collect();
            Some((
                format!("{} mods you follow have updates", mods.len()),
                format!("{}.", list(&names)),
            ))
        }
    }
}

/// "a", "a and b", "a, b and c", or "a, b, c and 2 more".
fn list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        _ if items.len() > NAMED => format!(
            "{} and {} more",
            items[..NAMED].join(", "),
            items.len() - NAMED
        ),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
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
                    source: m.source.clone(),
                    id: m.id.clone(),
                    current,
                    newer,
                });
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(label: &str, source: &str, id: &str, newer: &str) -> Update {
        Update {
            profile: uuid::Uuid::nil(),
            label: label.into(),
            game: "ror2".into(),
            name: id.into(),
            source: source.into(),
            id: id.into(),
            current: Some("1.0.0".into()),
            newer: newer.into(),
        }
    }

    #[test]
    fn only_followed_mods_are_notified_once_each() {
        let mut follows = Follows::default();
        follows.mods.insert("ts:A".into());
        follows.mods.insert("ts:B".into());
        let a1 = update("Main", "ts", "A", "2.0.0");
        let a2 = update("Testing", "ts", "A", "2.0.0");
        let c = update("Main", "ts", "C", "3.0.0");
        assert_eq!(notification(&[&c], &follows), None);
        assert_eq!(
            notification(&[&a1, &c, &a2], &follows),
            Some((
                "A 2.0.0 is out".into(),
                "It can be updated in Main and Testing.".into()
            ))
        );
        let b = update("Main", "ts", "B", "1.1.0");
        assert_eq!(
            notification(&[&a1, &b, &a2], &follows),
            Some(("2 mods you follow have updates".into(), "A and B.".into()))
        );
    }

    #[test]
    fn lists_name_a_few_then_count() {
        assert_eq!(list(&["a"]), "a");
        assert_eq!(list(&["a", "b", "c"]), "a, b and c");
        assert_eq!(list(&["a", "b", "c", "d", "e"]), "a, b, c and 2 more");
    }
}
