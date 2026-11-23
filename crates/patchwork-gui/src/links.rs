//! Being the program the browser hands `nxm://` links to, so Nexus's "Mod
//! Manager Download" buttons send their downloads to patchwork: whether it
//! is (for Settings), and making it so. The deep-link plugin does the
//! registering on Windows. On Linux patchwork does it itself: the plugin
//! quotes the program in the `.desktop` file it writes, which xdg-utils
//! can't read (so `xdg-open` and `xdg-mime query` act as if it weren't
//! there), and runs `xdg-mime default` even when `mimeapps.list` can't be
//! written (home-manager owns it, say), which fails with a stray error and
//! exits 0 anyway. So patchwork reads `mimeapps.list` itself, checks it
//! can write it before trying, and works out why when it can't.

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
    tauri::async_runtime::spawn_blocking(move || take_links(&handle))
        .await
        .map_err(msg)?
}

fn take_links(handle: &AppHandle) -> CmdResult<()> {
    if let LinkHandler::Patchwork | LinkHandler::Bundled = current(handle)? {
        return Ok(());
    }
    register(handle)
}

/// At startup, in the background: keeps the `.desktop` file patchwork
/// wrote for this copy readable (see the top of this file), and takes the
/// links for an AppImage, which has no installer to do it and may have
/// moved, and a Windows install, whose installer only did it for the user
/// who ran it.
pub fn on_startup(handle: &AppHandle) {
    let handle = handle.clone();
    std::thread::spawn(move || {
        #[cfg(target_os = "linux")]
        linux::refresh(&handle);
        use tauri::utils::config::BundleType;
        if let Some(BundleType::AppImage | BundleType::Nsis) = crate::selfupdate::installed_from()
            && let Err(e) = take_links(&handle)
        {
            eprintln!("couldn't register for {SCHEME}:// links: {e}");
        }
    });
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
    use std::process::{Command, Stdio};

    use tauri::{AppHandle, Manager};

    use super::{LinkHandler, SCHEME};
    use crate::{CmdResult, msg};

    fn mime() -> String {
        format!("x-scheme-handler/{SCHEME}")
    }

    /// What patchwork runs as: the AppImage, if it's in one, rather than
    /// the binary inside it, and the wrapper Nix runs it through rather
    /// than the binary it wraps.
    fn exe(handle: &AppHandle) -> CmdResult<PathBuf> {
        match handle.env().appimage {
            Some(appimage) => Ok(appimage.into()),
            None => {
                let bin = std::env::current_exe().map_err(msg)?;
                let wrapper = unwrapped(&bin);
                Ok(if wrapper.is_file() { wrapper } else { bin })
            }
        }
    }

    /// The `.desktop` file patchwork writes for this binary, named the way
    /// the deep-link plugin names it, so ones it wrote (and configs naming
    /// them) still work.
    fn handler_file() -> CmdResult<String> {
        let bin = tauri::utils::platform::current_exe().map_err(msg)?;
        let name = bin.file_name().ok_or("patchwork's binary has no name")?;
        Ok(format!("{}-handler.desktop", name.to_string_lossy()))
    }

    /// Where patchwork writes [`handler_file`].
    fn handler_dir(handle: &AppHandle) -> CmdResult<PathBuf> {
        Ok(handle.path().data_dir().map_err(msg)?.join("applications"))
    }

    /// The `.desktop` file that handles `nxm://` links, if any: the first
    /// one installed that the `mimeapps.list` files name, the way the
    /// system picks it, or failing that whatever `xdg-mime` says.
    fn default_app() -> CmdResult<Option<String>> {
        if let Some(name) = from_mimeapps() {
            return Ok(Some(name));
        }
        let output = Command::new("xdg-mime")
            .args(["query", "default", &mime()])
            .output()
            .map_err(|e| format!("couldn't run xdg-mime: {e}"))?;
        // xdg-mime exits with 4 when nothing handles them
        if !output.status.success() && output.status.code() != Some(4) {
            return Err(format!(
                "xdg-mime failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout
            .lines()
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from))
    }

    fn config_home() -> Option<PathBuf> {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::home_dir().map(|h| h.join(".config")))
    }

    fn data_home() -> Option<PathBuf> {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| std::env::home_dir().map(|h| h.join(".local/share")))
    }

    /// `$XDG_<var>`'s folders, or `default`'s if it's unset.
    fn dirs(var: &str, default: &str) -> Vec<PathBuf> {
        let value = std::env::var(var)
            .ok()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| default.into());
        std::env::split_paths(&value)
            .filter(|p| p.is_absolute())
            .collect()
    }

    /// The folders `.desktop` files are looked for in, first first.
    fn app_dirs() -> Vec<PathBuf> {
        data_home()
            .into_iter()
            .chain(dirs("XDG_DATA_DIRS", "/usr/local/share:/usr/share"))
            .map(|d| d.join("applications"))
            .collect()
    }

    /// The default for `nxm://` links the `mimeapps.list` files give, in
    /// the order the XDG MIME Applications spec reads them: each folder's
    /// desktop-specific lists, then its plain one.
    fn from_mimeapps() -> Option<String> {
        let desktops: Vec<String> = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .split(':')
            .filter(|d| !d.is_empty())
            .map(|d| format!("{}-", d.to_lowercase()))
            .collect();
        let folders = config_home()
            .into_iter()
            .chain(dirs("XDG_CONFIG_DIRS", "/etc/xdg"))
            .chain(app_dirs());
        let mime = mime();
        for folder in folders {
            for prefix in desktops.iter().map(String::as_str).chain([""]) {
                let Ok(text) =
                    std::fs::read_to_string(folder.join(format!("{prefix}mimeapps.list")))
                else {
                    continue;
                };
                if let Some(name) = default_in(&text, &mime)
                    .into_iter()
                    .find(|name| find_desktop_file(name).is_some())
                {
                    return Some(name);
                }
            }
        }
        None
    }

    /// The `.desktop` files a `mimeapps.list` makes the default for
    /// `mime`, in order.
    fn default_in(list: &str, mime: &str) -> Vec<String> {
        let mut in_defaults = false;
        for line in list.lines().map(str::trim) {
            if line.starts_with('[') {
                in_defaults = line == "[Default Applications]";
            } else if in_defaults
                && let Some((key, value)) = line.split_once('=')
                && key.trim() == mime
            {
                return value
                    .split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
            }
        }
        Vec::new()
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
        let file = match packaged_file(handle)? {
            Some(file) => file,
            None => write_handler(handle)?,
        };
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
            }
            .into());
        }
        let set = Command::new("xdg-mime")
            .args(["default", &file, &mime()])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output();
        if let LinkHandler::Patchwork = current(handle)? {
            return Ok(());
        }
        Err(match set {
            Err(e) => format!("Couldn't run xdg-mime to {set_it}: {e}"),
            Ok(out) if !out.status.success() => format!(
                "xdg-mime couldn't {set_it}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Ok(_) => format!(
                "Registered, but the system still sends {SCHEME}:// links elsewhere. \
                 Your desktop may keep its own default; {set_it} in its settings."
            ),
        }
        .into())
    }

    /// A `.desktop` file that came with this copy (like the one Nix's
    /// package installs) and opens `nxm://` links with it, if there is one.
    fn packaged_file(handle: &AppHandle) -> CmdResult<Option<String>> {
        let ours = exe(handle)?;
        let handler = handler_file()?;
        let mime = mime();
        for dir in app_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".desktop") || name == handler {
                    continue;
                }
                let path = entry.path();
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let handles = text
                    .lines()
                    .find_map(|l| l.strip_prefix("MimeType="))
                    .is_some_and(|types| types.split(';').any(|t| t.trim() == mime));
                if handles && program(&path).is_some_and(|exec| same_program(&exec, &ours)) {
                    return Ok(Some(name));
                }
            }
        }
        Ok(None)
    }

    /// Writes [`handler_file`] for this copy, if it isn't as it should be
    /// already, and gives its name.
    fn write_handler(handle: &AppHandle) -> CmdResult<String> {
        let dir = handler_dir(handle)?;
        let name = handler_file()?;
        let path = dir.join(&name);
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=Patchwork\nExec={} %u\n\
             Terminal=false\nMimeType={};\nNoDisplay=true\n",
            exec_arg(&exe(handle)?.to_string_lossy()),
            mime(),
        );
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            std::fs::create_dir_all(&dir).map_err(msg)?;
            std::fs::write(&path, text)
                .map_err(|e| format!("couldn't write {}: {e}", path.display()))?;
            // Only refreshes the cache of what handles what; defaults
            // don't need it, so it's fine if it isn't installed.
            let _ = Command::new("update-desktop-database")
                .arg(&dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        Ok(name)
    }

    /// Rewrites the `.desktop` file the deep-link plugin wrote for this
    /// copy, if there is one, so xdg-utils can read it (see the top of
    /// this file). One for another copy is left alone.
    pub fn refresh(handle: &AppHandle) {
        let (Ok(dir), Ok(name), Ok(ours)) = (handler_dir(handle), handler_file(), exe(handle))
        else {
            return;
        };
        if program(&dir.join(name)).is_some_and(|exec| same_program(&exec, &ours))
            && let Err(e) = write_handler(handle)
        {
            eprintln!("couldn't update patchwork's {SCHEME}:// handler: {e}");
        }
    }

    /// `program` as the first word of a `.desktop` file's `Exec`. It's
    /// only quoted when it has to be, since xdg-utils takes quotes as part
    /// of the name.
    fn exec_arg(program: &str) -> String {
        const RESERVED: &str = " \t\n\"'\\><~|&;$*?#()`";
        if !program.contains(|c| RESERVED.contains(c)) {
            return program.replace('%', "%%");
        }
        let mut quoted = String::from("\"");
        for c in program.chars() {
            match c {
                // escaped for the quoting, and the backslash again for the
                // file's own string escapes
                '"' | '`' | '$' => quoted.push_str("\\\\"),
                '\\' => quoted.push_str(r"\\\"),
                '%' => quoted.push('%'),
                _ => {}
            }
            quoted.push(c);
        }
        quoted.push('"');
        quoted
    }

    /// The `mimeapps.list` `xdg-mime default` writes to, if it can't be
    /// written to.
    fn unwritable_mimeapps() -> Option<PathBuf> {
        let config = config_home()?;
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
        app_dirs()
            .into_iter()
            .map(|d| d.join(name))
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
        let path = PathBuf::from(first.replace("%%", "%"));
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

        use super::{default_in, exec_arg, program, unwrapped};

        #[test]
        fn reads_defaults() {
            let list = "[Added Associations]\nx-scheme-handler/nxm=other.desktop\n\n\
                        [Default Applications]\ntext/html=firefox.desktop\n\
                        x-scheme-handler/nxm = a.desktop;b.desktop;\n";
            assert_eq!(
                default_in(list, "x-scheme-handler/nxm"),
                ["a.desktop", "b.desktop"]
            );
            assert!(default_in(list, "x-scheme-handler/http").is_empty());
        }

        #[test]
        fn quotes_exec_only_when_needed() {
            assert_eq!(
                exec_arg("/home/me/patchwork/target/debug/patchwork-gui"),
                "/home/me/patchwork/target/debug/patchwork-gui"
            );
            assert_eq!(exec_arg("/opt/100%/patchwork"), "/opt/100%%/patchwork");
            assert_eq!(
                exec_arg("/opt/my apps/patch\"work"),
                r#""/opt/my apps/patch\\"work""#
            );
        }

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
