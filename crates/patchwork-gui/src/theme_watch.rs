//! Watches the themes folders, so a theme being written shows as it's
//! saved, without going back to All games and in again.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter};

/// Sent to the window when a theme or a file in a theme's folder changes,
/// to apply the themes again.
pub const CHANGED: &str = "patchwork://themes-changed";

/// How long to wait for more changes before telling the window, since one
/// save can be several events (an editor writing a temporary file and
/// moving it into place, say).
const SETTLE: Duration = Duration::from_millis(150);

/// Starts watching `dirs` (the `themes/` folders of every registry layer),
/// on a thread of its own for as long as the app runs. Folders that don't
/// exist are skipped, apart from the user's, which `user` names and which
/// is made if it's missing.
pub fn start(handle: AppHandle, dirs: Vec<PathBuf>, user: &Path) {
    if let Err(e) = std::fs::create_dir_all(user) {
        eprintln!("couldn't make {}: {e}", user.display());
    }
    let (tx, rx) = mpsc::channel();
    let watched = dirs.clone();
    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<_>| {
        if let Ok(event) = res
            && is_theme_change(&event, &watched)
        {
            let _ = tx.send(());
        }
    }) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("couldn't watch the themes folders: {e}");
            return;
        }
    };
    for dir in dirs.iter().filter(|d| d.is_dir()) {
        if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
            eprintln!("couldn't watch {}: {e}", dir.display());
        }
    }
    std::thread::spawn(move || {
        // Keeps watching while the thread runs.
        let _watcher = watcher;
        while rx.recv().is_ok() {
            while rx.recv_timeout(SETTLE).is_ok() {}
            let _ = handle.emit(CHANGED, ());
        }
    });
}

/// Whether `event` changed a theme, `user.css`, or anything in a theme's
/// folder (a folder in one of `dirs`). `builtin.css` is left out:
/// patchwork writes it, and it's never loaded.
fn is_theme_change(event: &notify::Event, dirs: &[PathBuf]) -> bool {
    matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    ) && event.paths.iter().any(|p| {
        let in_folder = p
            .parent()
            .is_some_and(|parent| !dirs.iter().any(|d| d == parent));
        let css = p.extension().is_some_and(|e| e == "css");
        (in_folder || css) && p.file_name().is_some_and(|n| n != "builtin.css")
    })
}
