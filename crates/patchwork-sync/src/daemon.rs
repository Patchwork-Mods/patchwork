//! The sync daemon: a background process that keeps synced profiles in
//! step while neither the app nor the CLI is running.
//!
//! Only one process can have patchwork open at a time (see
//! [`Patchwork::open`]), so the daemon steps aside for the others: anything
//! that's about to open patchwork asks it to [`pause`] first, and it picks
//! up again once whatever paused it has closed. The app and the CLI run
//! their own [`Node`] while they're open, as they would without a daemon.
//!
//! The daemon listens on a local TCP port, which it writes to
//! `<sync>/daemon.json` along with a token that only this user can read, so
//! no one else on the machine can talk to it. Each request is a line of
//! JSON and so is each answer.
//!
//! [`service`] has it started whenever the user logs in.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use jiff::Timestamp;
use patchwork_core::Patchwork;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};

use crate::{Node, Outcome, devices_linked, synced_profiles, write_private};

pub mod service;

/// How often the daemon tries again to open patchwork while something else
/// has it open, or after syncing failed.
const RETRY_EVERY: Duration = Duration::from_secs(5);

/// How often an idle daemon checks whether anything's been synced since.
const IDLE_CHECK_EVERY: Duration = Duration::from_secs(60);

/// How long [`pause`] waits for the daemon to stop syncing.
const PAUSE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long talking to the daemon can take otherwise.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

/// How many times [`stop`] checks whether the daemon's gone, a tenth of a
/// second apart. Stopping syncing can take a moment.
const STOP_CHECKS: u32 = 100;

/// How many times a starting daemon checks whether the last one's gone,
/// a quarter of a second apart.
const LOCK_TRIES: u32 = 40;

/// How much of what it's done the daemon remembers, for [`status`].
const ACTIVITY_KEPT: usize = 50;

/// The log is started afresh once it's this big.
const LOG_LIMIT: u64 = 1024 * 1024;

/// Where a running daemon can be reached, as kept in `<sync>/daemon.json`.
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
    /// Stop syncing and let go of patchwork until this connection closes.
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
    /// Opening patchwork and starting to sync.
    Starting,
    /// Keeping synced profiles in step.
    Syncing,
    /// Paused by the app or the CLI, which sync themselves while open.
    Paused,
    /// Something else has patchwork open without having paused the daemon.
    Waiting,
    /// Nothing's synced on this computer.
    Idle,
}

impl State {
    /// Whether patchwork's open, and so whether pausing has to wait.
    fn busy(self) -> bool {
        matches!(self, State::Starting | State::Syncing)
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
}

/// Something the daemon did, or that went wrong.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Activity {
    pub at: Timestamp,
    pub message: String,
    pub error: bool,
}

/// What a running daemon shares between answering requests and syncing.
struct Shared {
    state: watch::Sender<State>,
    /// How many connections have it paused.
    holds: watch::Sender<usize>,
    stop: Notify,
    activity: Mutex<VecDeque<Activity>>,
    log: Mutex<Option<File>>,
    log_path: PathBuf,
    started: Timestamp,
    token: String,
}

impl Shared {
    fn record(&self, message: String, error: bool) {
        let at = Timestamp::now();
        eprintln!("{message}");
        if let Some(log) = self.log.lock().unwrap().as_mut() {
            let mark = if error { " error:" } else { "" };
            let _ = writeln!(log, "{at}{mark} {message}");
        }
        let mut activity = self.activity.lock().unwrap();
        if activity.len() == ACTIVITY_KEPT {
            activity.pop_front();
        }
        activity.push_back(Activity { at, message, error });
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
        }
    }

    fn set(&self, state: State) {
        self.state.send_replace(state);
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

/// Runs the daemon until it's told to [`stop`]. `open` opens patchwork,
/// which happens each time the daemon starts syncing again; if something
/// else has it open, the daemon waits for it to close.
pub async fn serve(dir: &Path, open: impl Fn() -> patchwork_core::Result<Patchwork>) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
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
                eprintln!("the sync daemon is already running");
                return Ok(());
            }
            Err(fs::TryLockError::Error(e)) => {
                return Err(e).context(lock_path.display().to_string());
            }
        }
    }

    let log_path = log_file(dir);
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
    let address_path = address_file(dir);
    let _ = fs::remove_file(&address_path);
    write_private(&address_path, &serde_json::to_vec(&address)?)?;
    shared.record(
        format!("started (patchwork {})", env!("CARGO_PKG_VERSION")),
        false,
    );

    let accepting = tokio::spawn(accept(listener, shared.clone()));
    sync_until_stopped(dir, &shared, open).await;
    accepting.abort();
    shared.record("stopped".to_owned(), false);
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

async fn sync_until_stopped(
    dir: &Path,
    shared: &Shared,
    open: impl Fn() -> patchwork_core::Result<Patchwork>,
) {
    let mut holds = shared.holds.subscribe();
    loop {
        // set before looking at holds, so that pausing waits for this
        shared.set(State::Starting);
        if *holds.borrow_and_update() > 0 {
            shared.set(State::Paused);
            tokio::select! {
                _ = holds.wait_for(|h| *h == 0) => continue,
                _ = shared.stop.notified() => return,
            }
        }
        let anything = synced_profiles(dir).is_ok_and(|s| !s.is_empty())
            || devices_linked(dir).unwrap_or(false);
        if !anything {
            shared.set(State::Idle);
            if shared.wait(IDLE_CHECK_EVERY).await {
                return;
            }
            continue;
        }
        let pw = match open() {
            Ok(pw) => pw,
            Err(patchwork_core::Error::Busy(_)) => {
                shared.set(State::Waiting);
                if shared.wait(RETRY_EVERY).await {
                    return;
                }
                continue;
            }
            Err(e) => {
                shared.record(format!("couldn't open patchwork: {e}"), true);
                shared.set(State::Waiting);
                if shared.wait(RETRY_EVERY).await {
                    return;
                }
                continue;
            }
        };
        let node = match Node::spawn(dir).await {
            Ok(node) => node,
            Err(e) => {
                drop(pw);
                shared.record(format!("couldn't start syncing: {e:#}"), true);
                shared.set(State::Waiting);
                if shared.wait(RETRY_EVERY).await {
                    return;
                }
                continue;
            }
        };
        shared.set(State::Syncing);
        let mut stopping = false;
        let mut failed = false;
        tokio::select! {
            result = node.run(&pw, |outcome| record_outcome(shared, &pw, outcome)) => {
                if let Err(e) = result {
                    shared.record(format!("syncing stopped: {e:#}"), true);
                    failed = true;
                }
            }
            _ = holds.wait_for(|h| *h > 0) => {}
            _ = shared.stop.notified() => stopping = true,
        }
        if let Err(e) = node.shutdown().await {
            shared.record(format!("couldn't stop syncing cleanly: {e:#}"), true);
        }
        drop(pw);
        if stopping {
            return;
        }
        if failed && shared.wait(RETRY_EVERY).await {
            return;
        }
    }
}

/// Records what a sync did, as `patchwork sync run` would print it.
fn record_outcome(shared: &Shared, pw: &Patchwork, outcome: Result<Outcome>) {
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(e) => {
            shared.record(format!("sync failed: {e:#}"), true);
            return;
        }
    };
    let label = pw
        .load_profile(&outcome.profile)
        .map(|p| p.label())
        .unwrap_or_else(|_| outcome.profile.to_string());
    match &outcome.applied {
        Some(applied) if applied.created => shared.record(format!("{label}: added"), false),
        Some(applied) if applied.changed => shared.record(format!("{label}: updated"), false),
        _ => {}
    }
    if outcome.received_files > 0 {
        shared.record(
            format!(
                "{label}: got {} local mod file(s) from other computers",
                outcome.received_files
            ),
            false,
        );
    }
    for warning in outcome.warnings {
        shared.record(format!("{label}: {warning}"), true);
    }
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
pub fn log_file(dir: &Path) -> PathBuf {
    dir.join("daemon.log")
}

/// Keeps the daemon paused until it's dropped, or the process exits.
#[derive(Debug)]
pub struct Paused(#[allow(dead_code)] TcpStream);

/// Talks to the daemon for `dir`, if one's running: `None` if not.
fn request(
    dir: &Path,
    kind: RequestKind,
    timeout: Duration,
) -> Result<Option<(Answer, TcpStream)>> {
    let Ok(bytes) = fs::read(address_file(dir)) else {
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
        .context("the sync daemon didn't answer")?;
    if answer.is_empty() {
        // something else took the port
        return Ok(None);
    }
    let answer: Answer = serde_json::from_str(&answer).context("the sync daemon's answer")?;
    if let Answer::Error { message } = answer {
        bail!("the sync daemon: {message}");
    }
    stream.set_read_timeout(None)?;
    Ok(Some((answer, stream)))
}

/// How the daemon for sync directory `dir` is doing, if it's running.
pub fn status(dir: &Path) -> Result<Option<Status>> {
    match request(dir, RequestKind::Status, ANSWER_TIMEOUT)? {
        Some((Answer::Status(status), _)) => Ok(Some(status)),
        Some(_) => Err(anyhow!("the sync daemon answered something else")),
        None => Ok(None),
    }
}

/// Has the daemon stop syncing and let go of patchwork, if it's running,
/// until what's returned is dropped. Call before opening patchwork.
pub fn pause(dir: &Path) -> Result<Option<Paused>> {
    match request(dir, RequestKind::Pause, PAUSE_TIMEOUT)? {
        Some((Answer::Paused, stream)) => Ok(Some(Paused(stream))),
        Some(_) => Err(anyhow!("the sync daemon answered something else")),
        None => Ok(None),
    }
}

/// Stops the daemon, if it's running, waiting for it to stop. True if it
/// was running.
pub fn stop(dir: &Path) -> Result<bool> {
    if request(dir, RequestKind::Stop, ANSWER_TIMEOUT)?.is_none() {
        return Ok(false);
    }
    for _ in 0..STOP_CHECKS {
        if !address_file(dir).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(true)
}

/// Has the daemon start whenever the user logs in, replacing any that's
/// running, and starts it.
pub fn install(dir: &Path, program: &service::Program) -> Result<()> {
    stop(dir)?;
    service::install(program)
}

/// Stops the daemon, and stops it starting when the user logs in.
pub fn uninstall(dir: &Path) -> Result<()> {
    service::uninstall()?;
    stop(dir)?;
    Ok(())
}
