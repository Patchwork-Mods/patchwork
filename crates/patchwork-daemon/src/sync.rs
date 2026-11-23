//! The `sync` task: keeps synced profiles in step (see [`patchwork_sync`])
//! while neither the app nor the CLI is running, and keeps a note of what
//! it changed for the app to show the next time it opens
//! ([`take_missed`]).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use futures_util::future::BoxFuture;
use jiff::Timestamp;
use patchwork_core::Patchwork;
use patchwork_core::issue::Severity;
use patchwork_core::paths::Paths;
use patchwork_core::profile::ProfileRef;
use patchwork_sync::{Node, Outcome, devices_linked, synced_profiles};
use serde::{Deserialize, Serialize};

use crate::task::{Cx, Task, Wanted};

/// How many syncs [`take_missed`] can return; older ones are forgotten.
const MISSED_KEPT: usize = 200;

pub struct Sync;

impl Task for Sync {
    fn id(&self) -> &'static str {
        "sync"
    }

    fn description(&self) -> &'static str {
        "Automatic profile sync with other computers"
    }

    fn default_enabled(&self) -> bool {
        true
    }

    fn wanted(&self, cx: &Cx) -> Wanted {
        let dir = cx.paths().sync();
        let anything = synced_profiles(&dir).is_ok_and(|s| !s.is_empty())
            || devices_linked(&dir).unwrap_or(false);
        if anything { Wanted::Now } else { Wanted::Never }
    }

    fn run<'a>(&'a self, pw: &'a Patchwork, cx: &'a Cx) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let dir = cx.paths().sync();
            let node = Node::spawn(&dir).await.context("couldn't start syncing")?;
            let missed = missed_file(&dir);
            let (synced, mut to_check) = tokio::sync::mpsc::unbounded_channel();
            let on_sync = |outcome: Result<Outcome>| {
                if let Ok(o) = &outcome {
                    let _ = synced.send(o.profile.clone());
                }
                record_outcome(cx, &missed, pw, outcome);
            };
            let notify = async {
                let mut told = BTreeSet::new();
                while let Some(profile) = to_check.recv().await {
                    notify_blockers(pw, cx, &profile, &mut told).await;
                }
            };
            let result = tokio::select! {
                result = node.run(pw, on_sync) => result.context("syncing stopped"),
                () = notify => Ok(()),
                _ = cx.stopping() => Ok(()),
            };
            if let Err(e) = node.shutdown().await {
                cx.record_error(format!("couldn't stop syncing cleanly: {e:#}"));
            }
            result
        })
    }
}

/// Tells the user about what a sync left in the way of using `profile`:
/// its issues that stop it deploying and have a fix, like a mod whose file
/// changed. They stay among the profile's issues for when the app opens,
/// with the fix. `told` is what's been told already, so each is told once.
async fn notify_blockers(
    pw: &Patchwork,
    cx: &Cx,
    profile: &ProfileRef,
    told: &mut BTreeSet<(ProfileRef, String)>,
) {
    let Ok(issues) = pw.issues(profile) else {
        return;
    };
    let label = pw
        .load_profile(profile)
        .map(|p| p.label())
        .unwrap_or_else(|_| profile.to_string());
    for issue in issues {
        if issue.severity != Severity::Error || issue.fix.is_none() {
            continue;
        }
        if told.insert((profile.clone(), issue.message.clone())) {
            let body = format!("{}. Open Patchwork to fix it.", issue.message);
            cx.notify(&format!("{label} needs attention"), &body).await;
        }
    }
}

/// Records what a sync did, as `patchwork sync run` would print it.
fn record_outcome(cx: &Cx, missed_path: &Path, pw: &Patchwork, outcome: Result<Outcome>) {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            cx.record_error(format!("sync failed: {e:#}"));
            return;
        }
    };
    let label = pw
        .load_profile(&outcome.profile)
        .map(|p| p.label())
        .unwrap_or_else(|_| outcome.profile.to_string());
    match &outcome.applied {
        _ if outcome.moved => cx.record_error(format!(
            "{label}: moved to a new document this computer hasn't been told about"
        )),
        Some(applied) if applied.created => cx.record(format!("{label}: added")),
        Some(applied) if applied.changed => cx.record(format!("{label}: updated")),
        _ => {}
    }
    let missed = Missed {
        at: Timestamp::now(),
        profile: outcome.profile.uuid,
        created: outcome.applied.as_ref().is_some_and(|a| a.created),
        changed: outcome.applied.as_ref().is_some_and(|a| a.changed),
        received_files: outcome.received_files,
        warnings: outcome.warnings.clone(),
    };
    if (missed.created || missed.changed || !missed.warnings.is_empty())
        && let Err(e) = save_missed(missed_path, missed)
    {
        cx.record_error(format!("couldn't save what changed for the app: {e:#}"));
    }
    if outcome.received_files > 0 {
        cx.record(format!(
            "{label}: got {} local mod file(s) from other computers",
            outcome.received_files
        ));
    }
    for warning in outcome.warnings {
        cx.record_error(format!("{label}: {warning}"));
    }
}

fn missed_file(dir: &Path) -> PathBuf {
    dir.join("missed.json")
}

/// A sync the daemon did that changed a profile, or went wrong, which the
/// app shows the next time it opens (see [`take_missed`]).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Missed {
    pub at: Timestamp,
    pub profile: uuid::Uuid,
    /// It arrived on this computer.
    pub created: bool,
    pub changed: bool,
    /// How many local mods' files came from other computers.
    pub received_files: usize,
    pub warnings: Vec<String>,
}

fn read_missed(path: &Path) -> Result<Vec<Missed>> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e).with_context(|| path.display().to_string()),
    }
}

fn save_missed(path: &Path, missed: Missed) -> Result<()> {
    // one that can't be read is started afresh, rather than never saved to
    let mut all = read_missed(path).unwrap_or_default();
    all.push(missed);
    // nobody's looked in a long while, so the oldest can go
    if all.len() > MISSED_KEPT {
        all.drain(..all.len() - MISSED_KEPT);
    }
    fs::write(path, serde_json::to_vec(&all)?).with_context(|| path.display().to_string())
}

/// What the daemon synced since this was last called, oldest first, which
/// is then forgotten. Call with patchwork open, so the daemon's paused and
/// can't be adding to it.
pub fn take_missed(paths: &Paths) -> Result<Vec<Missed>> {
    let path = missed_file(&paths.sync());
    let missed = read_missed(&path)?;
    if !missed.is_empty() {
        fs::remove_file(&path).with_context(|| path.display().to_string())?;
    }
    Ok(missed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missed(n: u128) -> Missed {
        Missed {
            at: Timestamp::now(),
            profile: uuid::Uuid::from_u128(n),
            created: false,
            changed: true,
            received_files: 0,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn missed_syncs_are_kept_until_taken_and_only_so_many() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(tmp.path());
        let dir = paths.sync();
        fs::create_dir_all(&dir).unwrap();
        assert!(take_missed(&paths).unwrap().is_empty());
        for n in 0..MISSED_KEPT as u128 + 5 {
            save_missed(&missed_file(&dir), missed(n)).unwrap();
        }
        let taken = take_missed(&paths).unwrap();
        assert_eq!(taken.len(), MISSED_KEPT);
        // the oldest went
        assert_eq!(taken[0].profile, uuid::Uuid::from_u128(5));
        assert!(take_missed(&paths).unwrap().is_empty());
    }
}
