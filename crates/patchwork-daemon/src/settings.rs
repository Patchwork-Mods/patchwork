//! Which of the daemon's tasks are on, and how often the periodic ones run,
//! kept in `<config>/daemon.toml`:
//!
//! ```toml
//! enabled = true
//!
//! [tasks.sync]
//! enabled = true
//!
//! [tasks.updates]
//! enabled = true
//! every = "6h"
//! notify = true
//! ```
//!
//! `enabled = false` turns the whole daemon off: it runs none of its tasks,
//! whatever theirs say. Anything else left out is up to the task (see [`crate::Task::default_enabled`]).
//! The daemon reads this each time it picks up again after being paused,
//! so changing it and then pausing the daemon (as [`set_enabled`] does)
//! is enough for it to take effect.

use std::collections::BTreeMap;
use std::fs;

use anyhow::{Context, Result};
use jiff::SignedDuration;
use patchwork_core::paths::Paths;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonSettings {
    /// Whether the daemon does anything at all (on unless it says not).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tasks: BTreeMap<String, TaskSettings>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// How long to leave between runs of a periodic task, such as `"6h"`
    /// or `"30m"`, in place of its own schedule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<String>,
    /// Whether a task that sends desktop notifications (see
    /// [`crate::Task::notifies`]) does; on unless it says not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<bool>,
}

impl TaskSettings {
    /// See [`TaskSettings::notify`].
    pub fn notifies(&self) -> bool {
        self.notify.unwrap_or(true)
    }

    /// [`TaskSettings::every`], if it's set and makes sense.
    pub fn every(&self) -> Result<Option<SignedDuration>> {
        let Some(every) = &self.every else {
            return Ok(None);
        };
        let every: SignedDuration = every
            .parse()
            .with_context(|| format!("`every = \"{every}\"` isn't a length of time"))?;
        anyhow::ensure!(every.is_positive(), "`every` has to be more than nothing");
        Ok(Some(every))
    }
}

impl DaemonSettings {
    /// What's in `daemon.toml`, or the defaults if there isn't one.
    pub fn load(paths: &Paths) -> Result<Self> {
        let path = paths.daemon_settings();
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| path.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| path.display().to_string()),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let path = paths.daemon_settings();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
        }
        fs::write(&path, toml::to_string(self)?).with_context(|| path.display().to_string())
    }

    pub fn task(&self, id: &str) -> TaskSettings {
        self.tasks.get(id).cloned().unwrap_or_default()
    }

    /// Whether the daemon's on at all (see [`DaemonSettings::enabled`]).
    pub fn on(&self) -> bool {
        self.enabled.unwrap_or(true)
    }
}

/// Turns the whole daemon on or off, and has a running daemon pick that up.
/// Its tasks keep their own settings for when it's back on.
pub fn set_on(paths: &Paths, on: bool) -> Result<()> {
    let mut settings = DaemonSettings::load(paths)?;
    settings.enabled = Some(on);
    settings.save(paths)?;
    drop(crate::pause(paths)?);
    Ok(())
}

/// Turns task `id` on or off, and has a running daemon pick that up.
pub fn set_enabled(paths: &Paths, id: &str, enabled: bool) -> Result<()> {
    let mut settings = DaemonSettings::load(paths)?;
    settings.tasks.entry(id.to_owned()).or_default().enabled = Some(enabled);
    settings.save(paths)?;
    // it reads its settings again once it's no longer paused
    drop(crate::pause(paths)?);
    Ok(())
}

/// Turns task `id`'s desktop notifications on or off.
pub fn set_notify(paths: &Paths, id: &str, notify: bool) -> Result<()> {
    let mut settings = DaemonSettings::load(paths)?;
    settings.tasks.entry(id.to_owned()).or_default().notify = Some(notify);
    settings.save(paths)?;
    drop(crate::pause(paths)?);
    Ok(())
}

/// Sets how long task `id` leaves between runs (see
/// [`TaskSettings::every`]), or with `None` goes back to its own schedule.
pub fn set_every(paths: &Paths, id: &str, every: Option<&str>) -> Result<()> {
    let mut settings = DaemonSettings::load(paths)?;
    let task = settings.tasks.entry(id.to_owned()).or_default();
    task.every = every.map(str::to_owned);
    task.every()?;
    settings.save(paths)?;
    drop(crate::pause(paths)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_lengths_of_time_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(tmp.path());
        assert!(DaemonSettings::load(&paths).unwrap().tasks.is_empty());
        assert!(DaemonSettings::load(&paths).unwrap().on());
        set_enabled(&paths, "sync", false).unwrap();
        set_every(&paths, "updates", Some("6h")).unwrap();
        assert!(
            DaemonSettings::load(&paths)
                .unwrap()
                .task("updates")
                .notifies()
        );
        set_notify(&paths, "updates", false).unwrap();
        let settings = DaemonSettings::load(&paths).unwrap();
        assert_eq!(settings.task("sync").enabled, Some(false));
        assert_eq!(
            settings.task("updates").every().unwrap(),
            Some(SignedDuration::from_hours(6))
        );
        assert!(!settings.task("updates").notifies());
        assert!(set_every(&paths, "updates", Some("soon")).is_err());
        assert_eq!(settings.task("nothing").enabled, None);
        set_on(&paths, false).unwrap();
        let settings = DaemonSettings::load(&paths).unwrap();
        assert!(!settings.on());
        assert_eq!(settings.task("sync").enabled, Some(false));
    }
}
