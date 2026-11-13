//! Keeping the open profile's mods' versions and summaries fresh. The
//! profile comes with what was last fetched for each, so the UI is complete
//! straight away; whatever's stale (see `KnownVersions::is_stale` and
//! `KnownSummary::is_stale`) is then fetched in the background, a few at a
//! time, and sent to the window as `patchwork://details` as each comes in.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use patchwork_core::lock::mod_key;
use patchwork_core::profile::ProfileRef;
use patchwork_core::search::ModSummary;
use patchwork_core::{KnownVersions, Lookup, Patchwork};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::{App, CmdResult, msg};

/// How many fetches of each kind a refresh runs at once.
const WORKERS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Versions,
    Summary,
}

impl Kind {
    /// What couldn't be done, as in "Couldn't get the details of 2 mod(s)".
    fn doing(self) -> &'static str {
        match self {
            Kind::Versions => "check for new versions of",
            Kind::Summary => "get the details of",
        }
    }
}

/// A mod to fetch something for.
#[derive(Clone)]
struct Job {
    /// The profile it was found in.
    profile: ProfileRef,
    game: String,
    /// What its versions are looked up for.
    lookup: Lookup,
    source: String,
    id: String,
    name: String,
    /// `<source>:<id>`, which the UI knows the mod by.
    key: String,
}

impl Job {
    /// The mod among every game's, since the same key can turn up in two.
    fn mod_id(&self) -> String {
        game_mod(&self.game, &self.key)
    }
}

fn game_mod(game: &str, key: &str) -> String {
    format!("{game}/{key}")
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum DetailsEvent {
    Versions {
        game: String,
        key: String,
        versions: Vec<String>,
        /// What they mean for the mod; see `UiMod::update`.
        update: Option<String>,
    },
    Summary {
        game: String,
        key: String,
        summary: Option<Box<ModSummary>>,
    },
    /// Some of a refresh's fetches failed; `message` sums them up.
    Failed {
        message: String,
        failures: Vec<String>,
    },
}

#[derive(Default)]
pub struct Details {
    /// Bumped by each refresh, so an earlier one's workers stop taking on
    /// more.
    generation: AtomicU64,
    /// Fetches in flight, by kind and [`Job::mod_id`], so a refresh doesn't
    /// repeat them.
    fetching: Mutex<HashSet<(Kind, String)>>,
    /// Fetches that failed; not retried until a restart.
    failed: Mutex<HashSet<(Kind, String)>>,
    /// Mods whose summaries are out of date for reasons their fetch times
    /// can't tell; see [`Details::outdate`].
    outdated: Mutex<HashSet<String>>,
}

impl Details {
    /// Has the next refresh fetch these mods' summaries whether or not
    /// they're stale, as after their locked versions change.
    pub fn outdate(&self, game: &str, keys: impl IntoIterator<Item = String>) {
        let mut outdated = self.outdated.lock().unwrap();
        outdated.extend(keys.into_iter().map(|key| game_mod(game, &key)));
    }

    /// Fetches one thing for `job` and sends it to the window, unless it's
    /// already being fetched or failed before. For versions, says whether
    /// they show the summary is now behind.
    async fn fetch(
        &self,
        handle: &AppHandle,
        pw: &Patchwork,
        kind: Kind,
        job: &Job,
    ) -> CmdResult<bool> {
        let id = (kind, job.mod_id());
        if self.failed.lock().unwrap().contains(&id)
            || !self.fetching.lock().unwrap().insert(id.clone())
        {
            return Ok(false);
        }
        let (game, key) = (job.game.clone(), job.key.clone());
        let result = match kind {
            Kind::Versions => {
                pw.versions(&job.lookup, &job.source, &job.id)
                    .await
                    .map(|versions| {
                        let _ = handle.emit(
                            "patchwork://details",
                            DetailsEvent::Versions {
                                game,
                                key,
                                versions,
                                update: known_update(pw, job),
                            },
                        );
                        // a newer version than the summary knows of means the
                        // mod's details (when it was updated, say) are out of
                        // date too
                        let summary = pw.known_summary(&job.game, &job.source, &job.id);
                        let versions = pw.known_versions(&job.lookup, &job.source, &job.id);
                        summary.zip(versions).is_some_and(|(s, v)| s.is_behind(&v))
                    })
            }
            Kind::Summary => {
                let result = pw.summary(&job.game, &job.source, &job.id).await;
                self.outdated.lock().unwrap().remove(&id.1);
                result.map(|summary| {
                    let _ = handle.emit(
                        "patchwork://details",
                        DetailsEvent::Summary {
                            game,
                            key,
                            summary: summary.map(Box::new),
                        },
                    );
                    false
                })
            }
        };
        self.fetching.lock().unwrap().remove(&id);
        if result.is_err() {
            self.failed.lock().unwrap().insert(id);
        }
        result.map_err(msg)
    }
}

/// What [`Patchwork::known_update`] says of `job`'s mod, as the profile is
/// now: it may have been pinned or locked while its versions were fetched.
fn known_update(pw: &Patchwork, job: &Job) -> Option<String> {
    let loaded = pw.load_profile(&job.profile).ok()?;
    let m = loaded
        .mods
        .iter()
        .find(|m| mod_key(&m.source, &m.id) == job.key)?;
    let lock = pw.load_lock(&job.profile).ok().flatten();
    pw.known_update(&job.lookup, m, lock.as_ref())
}

/// Starts fetching whatever's stale about `profile`'s mods, and stops any
/// earlier refresh from starting more fetches. Fetches already in flight
/// carry on, and aren't started again.
pub fn refresh(handle: &AppHandle, profile: &ProfileRef) -> CmdResult<()> {
    let app = handle.state::<App>();
    let pw = &app.pw;
    let loaded = pw.load_profile(profile).map_err(msg)?;
    let lookup = pw.lookup(&loaded).map_err(msg)?;
    let generation = app.details.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let mut versions = VecDeque::new();
    let mut summaries = VecDeque::new();
    let loaded_keys: HashSet<String> = loaded
        .mods
        .iter()
        .map(|m| mod_key(&m.source, &m.id))
        .collect();
    {
        let outdated = app.details.outdated.lock().unwrap();
        for m in loaded.mods {
            let job = Job {
                profile: profile.clone(),
                game: loaded.game.clone(),
                lookup: lookup.clone(),
                key: mod_key(&m.source, &m.id),
                source: m.source,
                id: m.id,
                name: m.name,
            };
            let versioned = pw.lists_versions(&job.source).unwrap_or(false);
            let known = versioned
                .then(|| pw.known_versions(&job.lookup, &job.source, &job.id))
                .flatten();
            let summary_stale = outdated.contains(&job.mod_id())
                || pw
                    .known_summary(&job.game, &job.source, &job.id)
                    .is_none_or(|s| s.is_stale(known.as_ref()));
            if versioned && known.as_ref().is_none_or(KnownVersions::is_stale) {
                versions.push_back(job.clone());
            }
            if summary_stale {
                summaries.push_back(job);
            }
        }
        // the dependencies' pictures and descriptions, for the Mods tab's
        // Dependencies section; they're only in the lockfile
        let lock = pw.load_lock(profile).ok().flatten();
        for dep in lock.iter().flat_map(|l| &l.mods) {
            let key = dep.key();
            if loaded_keys.contains(&key) {
                continue;
            }
            let job = Job {
                profile: profile.clone(),
                game: loaded.game.clone(),
                lookup: lookup.clone(),
                source: dep.source.clone(),
                id: dep.id.clone(),
                name: dep.name.clone(),
                key,
            };
            let stale = outdated.contains(&job.mod_id())
                || pw
                    .known_summary(&job.game, &job.source, &job.id)
                    .is_none_or(|s| s.is_stale(None));
            if stale {
                summaries.push_back(job);
            }
        }
    }
    start(handle, generation, Kind::Versions, versions);
    start(handle, generation, Kind::Summary, summaries);
    Ok(())
}

/// Works through `jobs` with up to [`WORKERS`] fetches at once, then says
/// in the window if any failed.
fn start(handle: &AppHandle, generation: u64, kind: Kind, jobs: VecDeque<Job>) {
    if jobs.is_empty() {
        return;
    }
    let handle = handle.clone();
    tauri::async_runtime::spawn(async move {
        let workers = WORKERS.min(jobs.len());
        let queue = Arc::new(Mutex::new(jobs));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let workers: Vec<_> = (0..workers)
            .map(|_| {
                tauri::async_runtime::spawn(work(
                    handle.clone(),
                    generation,
                    kind,
                    queue.clone(),
                    failures.clone(),
                ))
            })
            .collect();
        for worker in workers {
            let _ = worker.await;
        }
        let failures = std::mem::take(&mut *failures.lock().unwrap());
        for kind in [Kind::Versions, Kind::Summary] {
            let failures: Vec<String> = failures
                .iter()
                .filter(|(k, _)| *k == kind)
                .map(|(_, f)| f.clone())
                .collect();
            if let Some(first) = failures.first() {
                for f in &failures {
                    eprintln!("{f}");
                }
                let message = format!(
                    "Couldn't {} {} mod(s); {first}",
                    kind.doing(),
                    failures.len()
                );
                let _ = handle.emit(
                    "patchwork://details",
                    DetailsEvent::Failed { message, failures },
                );
            }
        }
    });
}

async fn work(
    handle: AppHandle,
    generation: u64,
    kind: Kind,
    queue: Arc<Mutex<VecDeque<Job>>>,
    failures: Arc<Mutex<Vec<(Kind, String)>>>,
) {
    let app = handle.state::<App>();
    let details = &app.details;
    while details.generation.load(Ordering::SeqCst) == generation {
        let Some(job) = queue.lock().unwrap().pop_front() else {
            return;
        };
        let fail = |kind: Kind, e: String| {
            failures
                .lock()
                .unwrap()
                .push((kind, format!("{}: {e}", job.name)));
        };
        match details.fetch(&handle, &app.pw, kind, &job).await {
            Ok(true) => {
                if let Err(e) = details.fetch(&handle, &app.pw, Kind::Summary, &job).await {
                    fail(Kind::Summary, e);
                }
            }
            Ok(false) => {}
            Err(e) => fail(kind, e),
        }
    }
}
