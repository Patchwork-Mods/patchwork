//! Being the program the browser hands `nxm://` links to, so Nexus's "Mod
//! Manager Download" buttons send their downloads to patchwork: whether it
//! is (for Settings), and making it so. The deep-link plugin does the
//! registering, but on Linux it can fail without saying so, since
//! `xdg-mime default` exits 0 even when it couldn't save the default (say,
//! when home-manager owns `mimeapps.list`), so patchwork checks afterwards
//! and works out why.

use serde::Serialize;
use tauri::AppHandle;

use crate::{CmdResult, msg};

/// The schemes patchwork handles, as in `plugins.deep-link` in
/// tauri.conf.json.
const SCHEME: &str = "nxm";

/// Who the system hands `nxm://` links to.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LinkHandler {
    /// This copy of patchwork.
    Patchwork,
    /// Another copy of patchwork, like a build from elsewhere or an
    /// AppImage that's since moved, at `exec`.
    OtherCopy { exec: String },
    /// Some other program: the name of its `.desktop` file on Linux, and
    /// `None` on Windows, where nothing may handle them at all.
    Other { name: Option<String> },
    /// Nothing does.
    Nothing,
    /// The system takes it from the app itself (macOS), so there's
    /// nothing to change.
    #[cfg_attr(any(windows, target_os = "linux"), allow(dead_code))]
    Bundled,
}

/// Who handles `nxm://` links right now.
#[tauri::command]
pub async fn link_handler(handle: AppHandle) -> CmdResult<LinkHandler> {
    tauri::async_runtime::spawn_blocking(move || current(&handle))
        .await
        .map_err(msg)?
}

/// Makes patchwork the handler for `nxm://` links, if it isn't already,
/// and checks it took. If it didn't, the error says why and what to do
/// instead.
#[tauri::command]
pub async fn handle_links(handle: AppHandle) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || {
        if let LinkHandler::Patchwork | LinkHandler::Bundled = current(&handle)? {
            return Ok(());
        }
        register(&handle)
    })
    .await
    .map_err(msg)?
}

#[cfg(windows)]
fn current(handle: &AppHandle) -> CmdResult<LinkHandler> {
    use tauri_plugin_deep_link::DeepLinkExt;
    Ok(
        match handle.deep_link().is_registered(SCHEME).map_err(msg)? {
            true => LinkHandler::Patchwork,
            false => LinkHandler::Other { name: None },
        },
    )
}

#[cfg(windows)]
fn register(handle: &AppHandle) -> CmdResult<()> {
    use tauri_plugin_deep_link::DeepLinkExt;
    handle.deep_link().register_all().map_err(msg)?;
    match current(handle)? {
        LinkHandler::Patchwork => Ok(()),
        _ => Err(
            "Windows still sends nxm:// links somewhere else. Pick Patchwork for them under \
                  Settings > Apps > Default apps > Choose defaults by link type."
                .into(),
        ),
    }
}

#[cfg(target_os = "linux")]
fn current(handle: &AppHandle) -> CmdResult<LinkHandler> {
    linux::current(handle)
}

#[cfg(target_os = "linux")]
fn register(handle: &AppHandle) -> CmdResult<()> {
    linux::register(handle)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn current(_: &AppHandle) -> CmdResult<LinkHandler> {
    Ok(LinkHandler::Bundled)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn register(_: &AppHandle) -> CmdResult<()> {
    Ok(())
}

#[cfg(target_os = "linux")]
mod linux {
    use std::fs::OpenOptions;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use tauri::{AppHandle, Manager};
    use tauri_plugin_deep_link::DeepLinkExt;

    use super::{LinkHandler, SCHEME};
    use crate::{CmdResult, msg};

    fn mime() -> String {
        format!("x-scheme-handler/{SCHEME}")
    }

    /// What patchwork runs as: the AppImage, if it's in one, rather than
    /// the binary inside it.
    fn exe(handle: &AppHandle) -> CmdResult<PathBuf> {
        match handle.env().appimage {
            Some(appimage) => Ok(appimage.into()),
            None => std::env::current_exe().map_err(msg),
        }
    }

    /// The `.desktop` file the deep-link plugin writes for this binary,
    /// named the way it names it.
    fn handler_file() -> CmdResult<String> {
        let bin = tauri::utils::platform::current_exe().map_err(msg)?;
        let name = bin.file_name().ok_or("patchwork's binary has no name")?;
        Ok(format!("{}-handler.desktop", name.to_string_lossy()))
    }

    /// The `.desktop` file `xdg-mime` says handles `nxm://` links, if any.
    fn default_app() -> CmdResult<Option<String>> {
        let output = Command::new("xdg-mime")
            .args(["query", "default", &mime()])
            .output()
            .map_err(|e| format!("couldn't run xdg-mime: {e}"))?;
        // xdg-mime exits with 4 when nothing handles them
        if !output.status.success() && output.status.code() != Some(4) {
            return Err(format!(
                "xdg-mime failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from))
    }

    pub fn current(handle: &AppHandle) -> CmdResult<LinkHandler> {
        let Some(name) = default_app()? else {
            return Ok(LinkHandler::Nothing);
        };
        let Some(exec) = find_desktop_file(&name).and_then(|f| program(&f)) else {
            return Ok(LinkHandler::Other { name: Some(name) });
        };
        let ours = exe(handle)?;
        if same_program(&exec, &ours) {
            Ok(LinkHandler::Patchwork)
        } else if unwrapped(&exec).file_name() == unwrapped(&ours).file_name()
            || name == handler_file()?
        {
            Ok(LinkHandler::OtherCopy {
                exec: exec.display().to_string(),
            })
        } else {
            Ok(LinkHandler::Other { name: Some(name) })
        }
    }

    pub fn register(handle: &AppHandle) -> CmdResult<()> {
        // The plugin writes the .desktop file, then runs
        // update-desktop-database and `xdg-mime default`. If
        // update-desktop-database is missing it stops there, but a default
        // doesn't need it, so this sets the default itself either way.
        let plugin = handle.deep_link().register_all();
        let file = handler_file()?;
        let _ = Command::new("xdg-mime")
            .args(["default", &file, &mime()])
            .status();
        if let LinkHandler::Patchwork = current(handle)? {
            return Ok(());
        }

        let set_it = format!("set {} to {file}", mime());
        if let Some(list) = unwritable_mimeapps() {
            let target = std::fs::canonicalize(&list).unwrap_or_else(|_| list.clone());
            return Err(if target.starts_with("/nix/store") {
                format!(
                    "{} is managed by Nix (home-manager, most likely), so Patchwork can't change it. \
                     Add this to your home-manager configuration instead: \
                     xdg.mimeApps.defaultApplications.\"{}\" = \"{file}\";",
                    list.display(),
                    mime(),
                )
            } else {
                format!(
                    "{} can't be changed, so Patchwork can't {set_it} there.",
                    list.display()
                )
            });
        }
        Err(match plugin {
            Err(e) => format!("Couldn't register for {SCHEME}:// links: {e}"),
            Ok(()) => format!(
                "Registered, but the system still sends {SCHEME}:// links elsewhere. \
                 Your desktop may keep its own default; {set_it} in its settings."
            ),
        })
    }

    /// The `mimeapps.list` `xdg-mime default` writes to, if it can't be
    /// written to.
    fn unwritable_mimeapps() -> Option<PathBuf> {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::home_dir().map(|h| h.join(".config")))?;
        let list = config.join("mimeapps.list");
        // opening to append changes nothing, but fails on a read-only file
        // or filesystem
        let writable = match list.try_exists() {
            Ok(true) => OpenOptions::new().append(true).open(&list).is_ok(),
            _ => !config.metadata().is_ok_and(|m| m.permissions().readonly()),
        };
        (!writable).then_some(list)
    }

    /// Where `name` is, looking where the system looks for `.desktop`
    /// files.
    fn find_desktop_file(name: &str) -> Option<PathBuf> {
        let home = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::home_dir().map(|h| h.join(".local/share")));
        let dirs = std::env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        home.into_iter()
            .chain(std::env::split_paths(&dirs))
            .map(|d| d.join("applications").join(name))
            .find(|f| f.is_file())
    }

    /// The program a `.desktop` file runs, found on PATH if it isn't a
    /// whole path.
    fn program(file: &Path) -> Option<PathBuf> {
        let text = std::fs::read_to_string(file).ok()?;
        let exec = text.lines().find_map(|l| l.strip_prefix("Exec="))?.trim();
        let first = match exec.strip_prefix('"') {
            Some(rest) => rest.split('"').next()?,
            None => exec.split_whitespace().next()?,
        };
        let path = PathBuf::from(first);
        if path.is_absolute() {
            return Some(path);
        }
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join(&path))
            .find(|p| p.is_file())
    }

    fn same_program(a: &Path, b: &Path) -> bool {
        let real = |p: &Path| std::fs::canonicalize(unwrapped(p)).unwrap_or_else(|_| p.to_owned());
        real(a) == real(b)
    }

    /// Nix's wrappers run `bin/.patchwork-gui-wrapped` from
    /// `bin/patchwork-gui`, so the binary running isn't the one `.desktop`
    /// files name; this is the one they'd name.
    fn unwrapped(path: &Path) -> PathBuf {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        match name
            .as_deref()
            .and_then(|n| n.strip_prefix('.')?.strip_suffix("-wrapped"))
        {
            Some(inner) => path.with_file_name(inner),
            None => path.to_owned(),
        }
    }

    #[cfg(test)]
    mod tests {
        use std::path::{Path, PathBuf};

        use super::{program, unwrapped};

        #[test]
        fn reads_exec() {
            let dir = std::env::temp_dir().join(format!("patchwork-links-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join("patchwork.desktop");
            std::fs::write(
                &file,
                "[Desktop Entry]\nName=Patchwork\nExec=\"/opt/my apps/patchwork-gui\" %u\n",
            )
            .unwrap();
            assert_eq!(
                program(&file),
                Some(PathBuf::from("/opt/my apps/patchwork-gui"))
            );
            std::fs::write(&file, "[Desktop Entry]\nExec=/usr/bin/patchwork-gui %u\n").unwrap();
            assert_eq!(
                program(&file),
                Some(PathBuf::from("/usr/bin/patchwork-gui"))
            );
            std::fs::remove_dir_all(dir).unwrap();
        }

        #[test]
        fn unwraps_nix_wrappers() {
            assert_eq!(
                unwrapped(Path::new("/nix/store/x/bin/.patchwork-gui-wrapped")),
                Path::new("/nix/store/x/bin/patchwork-gui"),
            );
            assert_eq!(
                unwrapped(Path::new("/usr/bin/patchwork-gui")),
                Path::new("/usr/bin/patchwork-gui")
            );
        }
    }
}
