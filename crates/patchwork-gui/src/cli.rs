//! The `patchwork` CLI, which is bundled next to the app (`externalBin` in
//! tauri.release.conf.json) so it's there however patchwork was installed.
//! The Windows installer and the .deb and .rpm put it on PATH themselves; a
//! macOS app or an AppImage can't, so the app offers to.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;
use tauri::utils::config::BundleType;

use crate::selfupdate::installed_from;
use crate::{App, CmdResult, msg};

const NAME: &str = if cfg!(windows) {
    "patchwork.exe"
} else {
    "patchwork"
};

/// The CLI that came with this copy of the app, if one did.
fn bundled() -> Option<PathBuf> {
    let path = std::env::current_exe().ok()?.parent()?.join(NAME);
    path.is_file().then_some(path)
}

/// Where the CLI goes to be on PATH, when the app has to put it there: a
/// link into the app on macOS, so it follows the app's updates, and a copy
/// out of an AppImage, which is only mounted while it runs.
fn target() -> Option<PathBuf> {
    match installed_from()? {
        BundleType::App => Some(PathBuf::from("/usr/local/bin").join(NAME)),
        BundleType::AppImage => Some(home()?.join(".local/bin").join(NAME)),
        _ => None,
    }
}

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Records that the AppImage's copy is ours, so it's kept up to date and
/// someone else's `patchwork` there is left alone.
fn copied_marker(app: &App) -> PathBuf {
    app.pw.paths().data.join("appimage-cli")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    /// Whether the app can put the CLI on PATH, and hasn't yet.
    can_install: bool,
}

#[tauri::command]
pub fn cli_status() -> CliStatus {
    let (Some(src), Some(dst)) = (bundled(), target()) else {
        return CliStatus { can_install: false };
    };
    let installed = match installed_from() {
        Some(BundleType::App) => std::fs::read_link(&dst).is_ok_and(|l| l == src),
        _ => same_contents(&src, &dst),
    };
    CliStatus {
        can_install: !installed,
    }
}

/// Puts the bundled CLI on PATH, returning what to tell the user.
#[tauri::command]
pub async fn install_cli(app: State<'_, App>) -> CmdResult<String> {
    let (Some(src), Some(dst)) = (bundled(), target()) else {
        return Err("this copy of patchwork can't install its command-line tool".to_owned());
    };
    if installed_from() == Some(BundleType::App) {
        link_with_admin(&src, &dst)?;
        return Ok(format!(
            "Installed the patchwork command at {}",
            dst.display()
        ));
    }
    copy(&src, &dst).map_err(msg)?;
    std::fs::write(copied_marker(&app), dst.to_string_lossy().as_bytes()).map_err(msg)?;
    let on_path = std::env::var_os("PATH")
        .is_some_and(|p| std::env::split_paths(&p).any(|d| Some(d.as_path()) == dst.parent()));
    Ok(if on_path {
        format!("Installed the patchwork command at {}", dst.display())
    } else {
        format!(
            "Installed the patchwork command at {}; add {} to your PATH to use it",
            dst.display(),
            dst.parent().unwrap_or(&dst).display()
        )
    })
}

/// Refreshes the AppImage's copy of the CLI after an update, if the app put
/// one on PATH before.
pub fn refresh_copy(app: &App) {
    if installed_from() != Some(BundleType::AppImage) {
        return;
    }
    let Ok(dst) = std::fs::read_to_string(copied_marker(app)) else {
        return;
    };
    let dst = PathBuf::from(dst);
    if let Some(src) = bundled()
        && dst.exists()
        && !same_contents(&src, &dst)
        && let Err(e) = copy(&src, &dst)
    {
        eprintln!(
            "couldn't update the patchwork command at {}: {e}",
            dst.display()
        );
    }
}

fn same_contents(a: &Path, b: &Path) -> bool {
    matches!((std::fs::read(a), std::fs::read(b)), (Ok(a), Ok(b)) if a == b)
}

/// Copies through a temporary file, so a copy that's running isn't written
/// over mid-run.
fn copy(src: &Path, dst: &Path) -> std::io::Result<()> {
    let dir = dst.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{NAME}.tmp"));
    std::fs::copy(src, &tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, dst)
}

/// `/usr/local/bin` usually needs an admin, so this asks for one the way
/// macOS apps do.
fn link_with_admin(src: &Path, dst: &Path) -> CmdResult<()> {
    // Run from the disk image, the link would break once it's ejected.
    if src.starts_with("/Volumes") {
        return Err("move Patchwork to Applications first, then try again".to_owned());
    }
    let dir = dst.parent().unwrap_or(Path::new("/"));
    // The paths go in as arguments, so they don't need quoting for AppleScript.
    let output = std::process::Command::new("osascript")
        .args([
            "-e",
            "on run argv",
            "-e",
            "do shell script \"mkdir -p \" & quoted form of item 3 of argv & \
             \" && ln -sf \" & quoted form of item 1 of argv & \" \" & \
             quoted form of item 2 of argv with administrator privileges",
            "-e",
            "end run",
        ])
        .arg(src)
        .arg(dst)
        .arg(dir)
        .output()
        .map_err(msg)?;
    if output.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&output.stderr);
        // -128 is the user cancelling the password prompt.
        if err.contains("-128") {
            Err("cancelled".to_owned())
        } else {
            Err(format!(
                "couldn't install the patchwork command: {}",
                err.trim()
            ))
        }
    }
}
