//! What the daemon does, a [`Task`] at a time, and when.
//!
//! A task says when it next needs patchwork open ([`Task::wanted`]), which
//! it works out without opening it. While any task wants it now, the daemon
//! opens patchwork and runs each one that does, all at once, and closes it
//! again once none of them do. A task like syncing keeps going until it's
//! told to stop; a periodic one ([`Periodic`]) runs a [`Job`] and is done,
//! and its [`Schedule`] says when it's next due.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use jiff::{SignedDuration, Timestamp};
use patchwork_core::Patchwork;
use patchwork_core::paths::Paths;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::settings::TaskSettings;

/// Something the daemon does in the background.
pub trait Task: Send + Sync {
    /// What it's called in `daemon.toml`, status and the log, such as
    /// `"sync"`.
    fn id(&self) -> &'static str;

    /// Names what it does, as a setting, for listing tasks, such as
    /// "Automatic mod update checks".
    fn description(&self) -> &'static str;

    /// Whether it's on when `daemon.toml` doesn't say.
    fn default_enabled(&self) -> bool;

    /// When it next needs patchwork open, without opening it. Asked again
    /// whenever the daemon's deciding what to do, and at least every
    /// minute while it's idle.
    fn wanted(&self, cx: &Cx) -> Wanted;

    /// Does its work with patchwork open. Should return once there's
    /// nothing left to do for now, or as soon as [`Cx::stopping`] does; an
    /// error has it tried again after a while.
    fn run<'a>(&'a self, pw: &'a Patchwork, cx: &'a Cx) -> BoxFuture<'a, Result<()>>;
}

/// When a [`Task`] next needs patchwork open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    Now,
    At(Timestamp),
    /// Not until something changes, such as a profile being synced.
    Never,
}

impl Wanted {
    pub fn due(self, now: Timestamp) -> bool {
        match self {
            Wanted::Now => true,
            Wanted::At(at) => at <= now,
            Wanted::Never => false,
        }
    }

    pub fn at(self) -> Option<Timestamp> {
        match self {
            Wanted::Now => Some(Timestamp::now()),
            Wanted::At(at) => Some(at),
            Wanted::Never => None,
        }
    }
}

/// What a [`Task`] gets to work with.
pub struct Cx {
    pub(crate) paths: Paths,
    pub(crate) settings: TaskSettings,
    pub(crate) id: &'static str,
    pub(crate) stopping: watch::Receiver<bool>,
    pub(crate) record: Box<dyn Fn(String, bool) + Send + Sync>,
}

impl Cx {
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// Its own part of `daemon.toml`.
    pub fn settings(&self) -> &TaskSettings {
        &self.settings
    }

    /// Where it can keep files of its own.
    pub fn dir(&self) -> PathBuf {
        self.paths.daemon().join("tasks").join(self.id)
    }

    /// Notes something it did in the daemon's log and status.
    pub fn record(&self, message: impl Into<String>) {
        (self.record)(message.into(), false)
    }

    /// Notes something that went wrong in the daemon's log and status.
    pub fn record_error(&self, message: impl Into<String>) {
        (self.record)(message.into(), true)
    }

    /// Resolves once the task should stop: the daemon's been paused or
    /// told to stop.
    pub async fn stopping(&self) {
        let mut stopping = self.stopping.clone();
        let _ = stopping.wait_for(|s| *s).await;
    }

    pub fn is_stopping(&self) -> bool {
        *self.stopping.borrow()
    }
}

/// One run of a [`Job`], as kept for its [`Schedule`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub started: Timestamp,
    pub finished: Timestamp,
    pub ok: bool,
    /// Whether it found anything new, such as updates. A smarter schedule
    /// can run a job more often while it keeps finding things.
    pub found: bool,
}

/// When a [`Periodic`] task's job is next due.
pub trait Schedule: Send + Sync {
    /// Going by its runs so far, oldest first. A time that's passed means
    /// it's due.
    fn next(&self, runs: &[Run]) -> Timestamp;
}

/// The same length of time after each run, or a shorter one (up to
/// [`Every::RETRY`]) after one that failed.
#[derive(Debug, Clone, Copy)]
pub struct Every(pub SignedDuration);

impl Every {
    /// The longest it waits to try again after a failed run.
    pub const RETRY: SignedDuration = SignedDuration::from_mins(15);
}

impl Schedule for Every {
    fn next(&self, runs: &[Run]) -> Timestamp {
        let Some(last) = runs.last() else {
            // never run, so it's due
            return Timestamp::UNIX_EPOCH;
        };
        let wait = if last.ok {
            self.0
        } else {
            self.0.min(Self::RETRY)
        };
        last.finished.checked_add(wait).unwrap_or(Timestamp::MAX)
    }
}

/// Something a [`Periodic`] task runs now and then.
pub trait Job: Send + Sync {
    fn id(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn default_enabled(&self) -> bool;
    /// How often it runs unless `daemon.toml` says (see
    /// [`TaskSettings::every`]).
    fn schedule(&self) -> Box<dyn Schedule>;
    /// Runs it once. True if it found anything new (see [`Run::found`]).
    fn run<'a>(&'a self, pw: &'a Patchwork, cx: &'a Cx) -> BoxFuture<'a, Result<bool>>;
}

/// A [`Job`] run on its [`Schedule`], with its runs remembered across
/// restarts in its [`Cx::dir`].
pub struct Periodic<J>(pub J);

impl<J> Periodic<J> {
    /// How many runs are remembered for the schedule.
    pub const RUNS_KEPT: usize = 20;
}

impl<J: Job> Periodic<J> {
    fn schedule(&self, cx: &Cx) -> Box<dyn Schedule> {
        match cx.settings.every() {
            Ok(Some(every)) => Box::new(Every(every)),
            Ok(None) => self.0.schedule(),
            Err(e) => {
                cx.record_error(format!("{e:#}; going by its own schedule"));
                self.0.schedule()
            }
        }
    }
}

fn runs_file(cx: &Cx) -> PathBuf {
    cx.dir().join("runs.json")
}

/// The runs remembered for a [`Periodic`] task, oldest first.
pub fn runs(cx: &Cx) -> Vec<Run> {
    // only a record, so one that can't be read is as good as none
    fs::read(runs_file(cx))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_run(cx: &Cx, run: Run, kept: usize) -> Result<()> {
    let mut all = runs(cx);
    all.push(run);
    if all.len() > kept {
        all.drain(..all.len() - kept);
    }
    let path = runs_file(cx);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    fs::write(&path, serde_json::to_vec(&all)?).with_context(|| path.display().to_string())
}

impl<J: Job> Task for Periodic<J> {
    fn id(&self) -> &'static str {
        self.0.id()
    }

    fn description(&self) -> &'static str {
        self.0.description()
    }

    fn default_enabled(&self) -> bool {
        self.0.default_enabled()
    }

    fn wanted(&self, cx: &Cx) -> Wanted {
        Wanted::At(self.schedule(cx).next(&runs(cx)))
    }

    fn run<'a>(&'a self, pw: &'a Patchwork, cx: &'a Cx) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let started = Timestamp::now();
            let result = tokio::select! {
                result = self.0.run(pw, cx) => result,
                // tried again from the start once the daemon picks up again
                _ = cx.stopping() => return Ok(()),
            };
            let run = Run {
                started,
                finished: Timestamp::now(),
                ok: result.is_ok(),
                found: *result.as_ref().unwrap_or(&false),
            };
            save_run(cx, run, Self::RUNS_KEPT)?;
            result.map(|_| ())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(finished: Timestamp, ok: bool) -> Run {
        Run {
            started: finished,
            finished,
            ok,
            found: false,
        }
    }

    #[test]
    fn every_waits_after_each_run_and_less_after_a_failure() {
        let every = Every(SignedDuration::from_hours(6));
        assert!(every.next(&[]) <= Timestamp::now());
        let t = Timestamp::from_second(1_000_000).unwrap();
        assert_eq!(
            every.next(&[run(t, true)]),
            t + SignedDuration::from_hours(6)
        );
        assert_eq!(every.next(&[run(t, false)]), t + Every::RETRY);
        // shorter than a retry anyway
        let often = Every(SignedDuration::from_mins(5));
        assert_eq!(
            often.next(&[run(t, false)]),
            t + SignedDuration::from_mins(5)
        );
    }
}
