//! The daemon: a background process that does patchwork's [`Task`]s, such
//! as keeping synced profiles in step ([`sync`]), while neither the app nor
//! the CLI is running. Each task can be turned on or off in
//! `<config>/daemon.toml` (see [`settings`]).
//!
//! Only one process can have patchwork open at a time (see
//! [`Patchwork::open`]), so the daemon steps aside for the others: anything
//! that's about to open patchwork asks it to [`pause`] first, and it picks
//! up again once whatever paused it has closed. The app and the CLI do
//! what they need themselves while they're open, as they would without a
//! daemon. The daemon only has patchwork open while a task wants it (see
//! [`Task::wanted`]).
//!
//! The daemon listens on a local TCP port, which it writes to
//! `<daemon>/daemon.json` along with a token that only this user can read,
//! so no one else on the machine can talk to it. Each request is a line of
//! JSON and so is each answer.
//!
//! [`service`] has it started whenever the user logs in.

use std::collections::{BTreeMap, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use jiff::Timestamp;
use patchwork_core::Patchwork;
use patchwork_core::paths::Paths;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};

mod notify;
pub mod service;
pub mod settings;
pub mod sync;
pub mod task;
pub mod updates;

pub use settings::{DaemonSettings, TaskSettings};
pub use task::{Cx, Every, Job, Periodic, Run, Schedule, Task, Wanted};

/// How long a task that failed waits before it's tried again, and how
/// often the daemon tries again to open patchwork while something else has
/// it open.
const RETRY_EVERY: Duration = Duration::from_secs(5);

/// How often an idle daemon asks its tasks again whether they want
/// anything (see [`Task::wanted`]).
const IDLE_CHECK_EVERY: Duration = Duration::from_secs(60);

/// How long [`pause`] waits for the daemon to let go of patchwork.
const PAUSE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long talking to the daemon can take otherwise.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

/// How many times [`stop`] checks whether the daemon's gone, a tenth of a
/// second apart. Stopping its tasks can take a moment.
const STOP_CHECKS: u32 = 100;

/// How many times a starting daemon checks whether the last one's gone,
/// a quarter of a second apart.
const LOCK_TRIES: u32 = 40;

/// How much of what it's done the daemon remembers, for [`status`].
const ACTIVITY_KEPT: usize = 50;

/// The log is started afresh once it's this big.
const LOG_LIMIT: u64 = 1024 * 1024;

/// Every task patchwork has. A frontend can run the daemon with others.
pub fn tasks() -> Vec<Box<dyn Task>> {
    vec![Box::new(sync::Sync), Box::new(Periodic(updates::Updates))]
}

/// Where a running daemon can be reached, as kept in `<daemon>/daemon.json`.
#[derive(Serialize, Deserialize)]
struct Address {
    port: u16,
    token: String,
    pid: u32,
}

#[derive(Serialize, Deserialize)]
struct Request {
    token: String,
    #[serde(flatten)]
    kind: RequestKind,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "request")]
enum RequestKind {
    Status,
    /// Stop every task and let go of patchwork until this connection
    /// closes.
    Pause,
    /// Exit.
    Stop,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "answer")]
enum Answer {
    Status(Status),
    Paused,
    Stopping,
    Error { message: String },
}

/// What the daemon's doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum State {
    /// Opening patchwork to start a task.
    Starting,
    /// Running tasks with patchwork open.
    // what daemons that only synced called it
    #[serde(alias = "syncing")]
    Running,
    /// Paused by the app or the CLI, which do what they need themselves
    /// while open.
    Paused,
    /// Something else has patchwork open without having paused the daemon.
    Waiting,
    /// No task wants anything for now.
    Idle,
}

impl State {
    /// Whether patchwork's open, and so whether pausing has to wait.
    fn busy(self) -> bool {
        matches!(self, State::Starting | State::Running)
    }
}

/// How a running daemon is doing, from [`status`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The patchwork version it's running.
    pub version: String,
    pub pid: u32,
    /// The program it's running from.
    pub exe: Option<PathBuf>,
    pub started: Timestamp,
    pub state: State,
    /// How many things have it paused.
    pub paused_by: usize,
    /// What it's done lately, newest last.
    pub activity: Vec<Activity>,
    pub log: PathBuf,
    /// Each of its tasks, as of when it last looked at them.
    #[serde(default)]
    pub tasks: Vec<TaskStatus>,
}

/// Something the daemon did, or that went wrong.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub at: Timestamp,
    pub message: String,
    pub error: bool,
    /// The task it was, if it was one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// How one of the daemon's tasks is doing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub id: String,
    pub description: String,
    pub enabled: bool,
    pub running: bool,
    /// When it next wants to run, if it's waiting to.
    pub next: Option<Timestamp>,
}

/// What a running daemon shares between answering requests and running
/// tasks.
struct Shared {
    state: watch::Sender<State>,
    /// How many connections have it paused.
    holds: watch::Sender<usize>,
    stop: Notify,
    activity: Mutex<VecDeque<Activity>>,
    tasks: Mutex<BTreeMap<&'static str, TaskStatus>>,
    log: Mutex<Option<File>>,
    log_path: PathBuf,
    started: Timestamp,
    token: String,
}

impl Shared {
    fn record(&self, task: Option<&str>, message: String, error: bool) {
        let at = Timestamp::now();
        let message = match task {
            Some(task) => format!("{task}: {message}"),
            None => message,
        };
        eprintln!("{message}");
        if let Some(log) = self.log.lock().unwrap().as_mut() {
            let mark = if error { " error:" } else { "" };
            let _ = writeln!(log, "{at}{mark} {message}");
        }
        let mut activity = self.activity.lock().unwrap();
        if activity.len() == ACTIVITY_KEPT {
            activity.pop_front();
        }
        activity.push_back(Activity {
            at,
            message,
            error,
            task: task.map(str::to_owned),
        });
    }

    fn status(&self) -> Status {
        Status {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            exe: std::env::current_exe().ok(),
            started: self.started,
            state: *self.state.borrow(),
            paused_by: *self.holds.borrow(),
            activity: self.activity.lock().unwrap().iter().cloned().collect(),
            log: self.log_path.clone(),
            tasks: self.tasks.lock().unwrap().values().cloned().collect(),
        }
    }

    fn set(&self, state: State) {
        self.state.send_replace(state);
    }

    fn set_task(&self, task: &dyn Task, enabled: bool, running: bool, next: Option<Timestamp>) {
        self.tasks.lock().unwrap().insert(
            task.id(),
            TaskStatus {
                id: task.id().to_owned(),
                description: task.description().to_owned(),
                enabled,
                running,
                next,
            },
        );
    }

    /// Waits up to `limit` for something to pause the daemon, or stop it.
    /// True if it should stop.
    async fn wait(&self, limit: Duration) -> bool {
        let mut holds = self.holds.subscribe();
        tokio::select! {
            _ = tokio::time::sleep(limit) => false,
            _ = holds.wait_for(|h| *h > 0) => false,
            _ = self.stop.notified() => true,
        }
    }
}

/// Runs the daemon until it's told to [`stop`], doing `tasks` (usually
/// [`tasks`]). `open` opens patchwork, which happens each time a task wants
/// it; if something else has it open, the daemon waits for it to close.
pub async fn serve(
    paths: &Paths,
    tasks: Vec<Box<dyn Task>>,
    open: impl Fn() -> patchwork_core::Result<Patchwork>,
) -> Result<()> {
    let dir = paths.daemon();
    fs::create_dir_all(&dir).with_context(|| dir.display().to_string())?;
    let lock_path = dir.join("daemon.lock");
    let lock = File::create(&lock_path).with_context(|| lock_path.display().to_string())?;
    // one that's just been told to stop may not have quite gone yet
    let mut tries = 0;
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(fs::TryLockError::WouldBlock) if tries < LOCK_TRIES => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Err(fs::TryLockError::WouldBlock) => {
                eprintln!("the daemon is already running");
                return Ok(());
            }
            Err(fs::TryLockError::Error(e)) => {
                return Err(e).context(lock_path.display().to_string());
            }
        }
    }

    let log_path = log_file(paths);
    if fs::metadata(&log_path).is_ok_and(|m| m.len() > LOG_LIMIT) {
        let _ = fs::rename(&log_path, log_path.with_extension("log.old"));
    }
    let log = File::options()
        .create(true)
        .append(true)
        .open(&log_path)
        .ok();

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .context("listening for the app and the CLI")?;
    let shared = Arc::new(Shared {
        state: watch::Sender::new(State::Starting),
        holds: watch::Sender::new(0),
        stop: Notify::new(),
        activity: Mutex::default(),
        tasks: Mutex::default(),
        log: Mutex::new(log),
        log_path,
        started: Timestamp::now(),
        token: uuid::Uuid::new_v4().simple().to_string(),
    });
    let address = Address {
        port: listener.local_addr()?.port(),
        token: shared.token.clone(),
        pid: std::process::id(),
    };
    let address_path = address_file(&dir);
    let _ = fs::remove_file(&address_path);
    write_private(&address_path, &serde_json::to_vec(&address)?)?;
    shared.record(
        None,
        format!("started (patchwork {})", env!("CARGO_PKG_VERSION")),
        false,
    );

    let accepting = tokio::spawn(accept(listener, shared.clone()));
    run_until_stopped(paths, &tasks, &shared, open).await;
    accepting.abort();
    shared.record(None, "stopped".to_owned(), false);
    // only if it's still ours: a new daemon may have started already
    if fs::read(&address_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Address>(&b).ok())
        .is_some_and(|a| a.token == shared.token)
    {
        let _ = fs::remove_file(&address_path);
    }
    drop(lock);
    Ok(())
}

/// A task as the daemon's running it, with what it needs to.
struct Entry<'a> {
    task: &'a dyn Task,
    cx: Cx,
}

impl Entry<'_> {
    /// When it next wants patchwork open, but not before `retry` if it's
    /// failed.
    fn wanted(&self, retry: Option<Timestamp>) -> Wanted {
        match (self.task.wanted(&self.cx), retry) {
            (Wanted::Never, _) => Wanted::Never,
            (wanted, Some(retry)) if wanted.at().is_some_and(|at| at < retry) => Wanted::At(retry),
            (wanted, _) => wanted,
        }
    }
}

/// The enabled tasks, each with the settings it's to run with, as of now.
fn entries<'a>(
    paths: &Paths,
    tasks: &'a [Box<dyn Task>],
    shared: &Arc<Shared>,
    stopping: &watch::Sender<bool>,
) -> Vec<Entry<'a>> {
    let settings = DaemonSettings::load(paths).unwrap_or_else(|e| {
        shared.record(
            None,
            format!("couldn't read the daemon's settings, so going by the defaults: {e:#}"),
            true,
        );
        DaemonSettings::default()
    });
    shared.tasks.lock().unwrap().clear();
    let mut entries = Vec::new();
    for task in tasks {
        let task = task.as_ref();
        let own = settings.task(task.id());
        let enabled = settings.on() && own.enabled.unwrap_or_else(|| task.default_enabled());
        shared.set_task(task, enabled, false, None);
        if !enabled {
            continue;
        }
        let recording = shared.clone();
        let id = task.id();
        entries.push(Entry {
            task,
            cx: Cx {
                paths: paths.clone(),
                settings: own,
                id,
                stopping: stopping.subscribe(),
                record: Box::new(move |message, error| recording.record(Some(id), message, error)),
            },
        });
    }
    entries
}

async fn run_until_stopped(
    paths: &Paths,
    tasks: &[Box<dyn Task>],
    shared: &Arc<Shared>,
    open: impl Fn() -> patchwork_core::Result<Patchwork>,
) {
    let mut holds = shared.holds.subscribe();
    let stopping = watch::Sender::new(false);
    // read again each time it picks up after a pause, which is when
    // changes to the settings take effect
    let mut entries = entries(paths, tasks, shared, &stopping);
    // when each failed task can be tried again
    let mut retries = vec![None; entries.len()];
    loop {
        // set before looking at holds, so that pausing waits for this
        shared.set(State::Starting);
        if *holds.borrow_and_update() > 0 {
            shared.set(State::Paused);
            tokio::select! {
                _ = holds.wait_for(|h| *h == 0) => {
                    entries = self::entries(paths, tasks, shared, &stopping);
                    retries = vec![None; entries.len()];
                    continue;
                }
                _ = shared.stop.notified() => return,
            }
        }
        let wanted: Vec<Wanted> = entries
            .iter()
            .zip(&retries)
            .map(|(e, r)| e.wanted(*r))
            .collect();
        for (entry, wanted) in entries.iter().zip(&wanted) {
            shared.set_task(entry.task, true, false, wanted.at());
        }
        // after asking, so that anything wanted now is due
        let now = Timestamp::now();
        if !wanted.iter().any(|w| w.due(now)) {
            shared.set(State::Idle);
            if shared.wait(until(next_due(&wanted))).await {
                return;
            }
            continue;
        }
        let pw = match open() {
            Ok(pw) => pw,
            Err(e) => {
                if !matches!(e, patchwork_core::Error::Busy(_)) {
                    shared.record(None, format!("couldn't open patchwork: {e}"), true);
                }
                shared.set(State::Waiting);
                if shared.wait(RETRY_EVERY).await {
                    return;
                }
                continue;
            }
        };
        shared.set(State::Running);
        stopping.send_replace(false);
        let stop = session(&pw, &entries, &mut retries, shared, &stopping).await;
        drop(pw);
        if stop {
            return;
        }
    }
}

/// How long to wait for `next`, at most [`IDLE_CHECK_EVERY`].
fn until(next: Option<Timestamp>) -> Duration {
    let Some(next) = next else {
        return IDLE_CHECK_EVERY;
    };
    let wait = Timestamp::now().duration_until(next);
    Duration::try_from(wait)
        .unwrap_or(Duration::ZERO)
        .min(IDLE_CHECK_EVERY)
}

fn next_due(wanted: &[Wanted]) -> Option<Timestamp> {
    wanted.iter().filter_map(|w| w.at()).min()
}

/// Runs whichever tasks are due with patchwork open, starting others as
/// they come due, until none is running or due, or the daemon's paused or
/// stopped. True if it should stop.
async fn session(
    pw: &Patchwork,
    entries: &[Entry<'_>],
    retries: &mut [Option<Timestamp>],
    shared: &Shared,
    stopping: &watch::Sender<bool>,
) -> bool {
    let mut holds = shared.holds.subscribe();
    let mut running = FuturesUnordered::new();
    let mut busy = vec![false; entries.len()];
    let mut stop = false;
    loop {
        let mut waiting = Vec::new();
        for (i, entry) in entries.iter().enumerate() {
            if busy[i] {
                continue;
            }
            let wanted = entry.wanted(retries[i]);
            if wanted.due(Timestamp::now()) {
                busy[i] = true;
                shared.set_task(entry.task, true, true, None);
                let (task, cx) = (entry.task, &entry.cx);
                running.push(async move { (i, task.run(pw, cx).await) });
            } else {
                shared.set_task(entry.task, true, false, wanted.at());
                waiting.push(wanted);
            }
        }
        let next = next_due(&waiting);
        if running.is_empty() {
            return false;
        }
        let done = tokio::select! {
            done = running.next() => done,
            // something else might have come due
            _ = tokio::time::sleep(until(next)) => None,
            _ = holds.wait_for(|h| *h > 0) => break,
            _ = shared.stop.notified() => {
                stop = true;
                break;
            }
        };
        if let Some((i, result)) = done {
            busy[i] = false;
            retries[i] = finished(&entries[i], result);
        }
    }
    // tell everything still running to stop, and wait for it to
    stopping.send_replace(true);
    while let Some((i, result)) = running.next().await {
        retries[i] = finished(&entries[i], result);
    }
    stop
}

/// Records how a task's run went: when it can be tried again, if it failed.
fn finished(entry: &Entry, result: Result<()>) -> Option<Timestamp> {
    let e = result.err()?;
    entry.cx.record_error(format!("{e:#}"));
    Some(Timestamp::now() + RETRY_EVERY)
}

async fn accept(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(answer(stream, shared.clone()));
    }
}

async fn answer(stream: tokio::net::TcpStream, shared: Arc<Shared>) {
    let (read, mut write) = stream.into_split();
    let mut read = tokio::io::BufReader::new(read);
    let mut line = String::new();
    let request = match tokio::time::timeout(ANSWER_TIMEOUT, read.read_line(&mut line)).await {
        Ok(Ok(_)) => serde_json::from_str::<Request>(&line).ok(),
        _ => None,
    };
    let send = |answer: Answer| {
        let mut line = serde_json::to_vec(&answer).unwrap_or_default();
        line.push(b'\n');
        line
    };
    let Some(request) = request.filter(|r| r.token == shared.token) else {
        let _ = write
            .write_all(&send(Answer::Error {
                message: "not a request from patchwork".to_owned(),
            }))
            .await;
        return;
    };
    match request.kind {
        RequestKind::Status => {
            let _ = write
                .write_all(&send(Answer::Status(shared.status())))
                .await;
        }
        RequestKind::Stop => {
            let _ = write.write_all(&send(Answer::Stopping)).await;
            shared.stop.notify_one();
        }
        RequestKind::Pause => {
            shared.holds.send_modify(|h| *h += 1);
            let mut state = shared.state.subscribe();
            let _ = state.wait_for(|s| !s.busy()).await;
            if write.write_all(&send(Answer::Paused)).await.is_ok() {
                // held until whoever paused it closes the connection
                let mut rest = Vec::new();
                let _ = tokio::io::AsyncReadExt::read_to_end(&mut read, &mut rest).await;
            }
            shared.holds.send_modify(|h| *h -= 1);
        }
    }
}

fn address_file(dir: &Path) -> PathBuf {
    dir.join("daemon.json")
}

/// Where the daemon writes what it does.
pub fn log_file(paths: &Paths) -> PathBuf {
    paths.daemon().join("daemon.log")
}

/// Writes a file only this user can read, where that's up to the file.
fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| path.display().to_string())?;
        file.write_all(contents)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, contents).with_context(|| path.display().to_string())
    }
}

/// Keeps the daemon paused until it's dropped, or the process exits.
#[derive(Debug)]
pub struct Paused(#[allow(dead_code)] TcpStream);

/// Talks to the daemon, if one's running: `None` if not.
fn request(
    paths: &Paths,
    kind: RequestKind,
    timeout: Duration,
) -> Result<Option<(Answer, TcpStream)>> {
    // daemons from before it did more than sync kept their address with
    // sync's files, and one may still be running from before an update
    let found = [paths.daemon(), paths.sync()]
        .iter()
        .find_map(|dir| fs::read(address_file(dir)).ok());
    let Some(bytes) = found else {
        return Ok(None);
    };
    let Ok(address) = serde_json::from_slice::<Address>(&bytes) else {
        return Ok(None);
    };
    let to = SocketAddr::from((Ipv4Addr::LOCALHOST, address.port));
    // left behind by a daemon that didn't get to clean up
    let Ok(mut stream) = TcpStream::connect_timeout(&to, CONNECT_TIMEOUT) else {
        return Ok(None);
    };
    stream.set_read_timeout(Some(timeout))?;
    let mut line = serde_json::to_vec(&Request {
        token: address.token,
        kind,
    })?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut answer = String::new();
    BufReader::new(&stream)
        .read_line(&mut answer)
        .context("the daemon didn't answer")?;
    if answer.is_empty() {
        // something else took the port
        return Ok(None);
    }
    let answer: Answer = serde_json::from_str(&answer).context("the daemon's answer")?;
    if let Answer::Error { message } = answer {
        bail!("the daemon: {message}");
    }
    stream.set_read_timeout(None)?;
    Ok(Some((answer, stream)))
}

/// How the daemon is doing, if it's running.
pub fn status(paths: &Paths) -> Result<Option<Status>> {
    match request(paths, RequestKind::Status, ANSWER_TIMEOUT)? {
        Some((Answer::Status(status), _)) => Ok(Some(status)),
        Some(_) => Err(anyhow!("the daemon answered something else")),
        None => Ok(None),
    }
}

/// Has the daemon stop its tasks and let go of patchwork, if it's running,
/// until what's returned is dropped. Call before opening patchwork.
pub fn pause(paths: &Paths) -> Result<Option<Paused>> {
    match request(paths, RequestKind::Pause, PAUSE_TIMEOUT)? {
        Some((Answer::Paused, stream)) => Ok(Some(Paused(stream))),
        Some(_) => Err(anyhow!("the daemon answered something else")),
        None => Ok(None),
    }
}

/// Stops the daemon, if it's running, waiting for it to stop. True if it
/// was running.
pub fn stop(paths: &Paths) -> Result<bool> {
    if request(paths, RequestKind::Stop, ANSWER_TIMEOUT)?.is_none() {
        return Ok(false);
    }
    for _ in 0..STOP_CHECKS {
        if [paths.daemon(), paths.sync()]
            .iter()
            .all(|dir| !address_file(dir).exists())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(true)
}

/// Has the daemon start whenever the user logs in, replacing any that's
/// running, and starts it.
pub fn install(paths: &Paths, program: &service::Program) -> Result<()> {
    stop(paths)?;
    service::install(program)?;
    let record = installed_record(paths);
    if let Err(e) = Installed::of(program).write(&record) {
        // only matters to [`refresh`]
        eprintln!("couldn't write {}: {e:#}", record.display());
    }
    Ok(())
}

/// Stops the daemon, and stops it starting when the user logs in.
pub fn uninstall(paths: &Paths) -> Result<()> {
    service::uninstall()?;
    let _ = fs::remove_file(installed_record(paths));
    stop(paths)?;
    Ok(())
}

/// What [`install`] had the system run, to tell later whether that's gone.
fn installed_record(paths: &Paths) -> PathBuf {
    paths.daemon().join("installed.json")
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct Installed {
    exe: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

impl Installed {
    fn of(program: &service::Program) -> Self {
        Self {
            exe: program.exe.clone(),
            args: program.args.clone(),
            env: program
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.to_string_lossy().into_owned()))
                .collect(),
        }
    }

    fn read(path: &Path) -> Option<Self> {
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }

    fn write(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    /// Whether `now` should replace this: it's the same frontend run the
    /// same way, and the program this names is gone, as it is once a
    /// package manager has updated patchwork into a new directory and
    /// cleaned up the old one.
    fn replaced_by(&self, now: &Self) -> bool {
        self.args == now.args && self.env == now.env && self.exe != now.exe && !self.exe.exists()
    }
}

/// Points the login entry [`install`] made at `program` if it still names
/// a copy of `program` that's gone, restarting the daemon from it. True if
/// it did. Cheap enough to call each time a frontend starts.
pub fn refresh(paths: &Paths, program: &service::Program) -> Result<bool> {
    let Some(before) = Installed::read(&installed_record(paths)) else {
        return Ok(false);
    };
    if !before.replaced_by(&Installed::of(program)) || service::installed()?.is_none() {
        return Ok(false);
    }
    install(paths, program)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_entry_follows_its_program_when_that_moves() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("0.3.0/patchwork");
        let new = dir.path().join("0.4.0/patchwork");
        fs::create_dir_all(new.parent().unwrap()).unwrap();
        fs::write(&new, "").unwrap();
        let entry = |exe: &Path, args: &[&str]| Installed {
            exe: exe.to_owned(),
            args: args.iter().map(|&a| a.to_owned()).collect(),
            env: Vec::new(),
        };
        let now = entry(&new, &["daemon", "run"]);
        // gone, and the same frontend: follow it
        assert!(entry(&old, &["daemon", "run"]).replaced_by(&now));
        // the app's, not the CLI's
        assert!(!entry(&old, &["--daemon"]).replaced_by(&now));
        // still there, so maybe another copy someone wants kept
        fs::create_dir_all(old.parent().unwrap()).unwrap();
        fs::write(&old, "").unwrap();
        assert!(!entry(&old, &["daemon", "run"]).replaced_by(&now));
        // a different PATCHWORK_HOME's
        let mut other = entry(&dir.path().join("gone"), &["daemon", "run"]);
        other
            .env
            .push(("PATCHWORK_HOME".to_owned(), "/scratch".to_owned()));
        assert!(!other.replaced_by(&now));

        let record = dir.path().join("daemon/installed.json");
        now.write(&record).unwrap();
        assert_eq!(Installed::read(&record), Some(now));
    }
}
