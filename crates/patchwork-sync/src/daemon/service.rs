//! Having the system start the [daemon](super) whenever the user logs in: a
//! systemd user service on Linux (or an autostart entry where there's no
//! systemd), a LaunchAgent on macOS, and a `Run` registry value on Windows.
//! Each is only for the user installing it, and needs no admin rights.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};

/// What the system runs to start the daemon.
#[derive(Debug, Clone)]
pub struct Program {
    pub exe: PathBuf,
    pub args: Vec<String>,
    /// Patchwork's own environment variables, which say where its files
    /// are, so the daemon syncs the same ones.
    pub env: Vec<(String, OsString)>,
}

impl Program {
    /// This program, run with `args`. An AppImage is run from the AppImage
    /// itself, since where it's mounted changes each time.
    pub fn current(args: &[&str]) -> Result<Self> {
        let exe = match std::env::var_os("APPIMAGE") {
            Some(appimage) => PathBuf::from(appimage),
            None => std::env::current_exe().context("finding this program")?,
        };
        let env = ["PATCHWORK_HOME", "PATCHWORK_REGISTRY_PATH"]
            .into_iter()
            .filter_map(|k| Some((k.to_owned(), std::env::var_os(k)?)))
            .collect();
        Ok(Self {
            exe,
            args: args.iter().map(|&a| a.to_owned()).collect(),
            env,
        })
    }

    /// Starts it in the background, outliving this process.
    fn spawn(&self) -> Result<()> {
        let mut command = quiet(&self.exe);
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // out of the terminal's process group, so closing it or
            // pressing Ctrl-C there doesn't stop the daemon too
            command.process_group(0);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("starting {}", self.exe.display()))?;
        // so it doesn't linger as a zombie once it exits
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

/// A command that doesn't flash up a console window on Windows.
fn quiet(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    command
}

/// Runs a command to completion, failing with what it said if it fails.
fn run(command: &mut Command) -> Result<()> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("running {:?}", command.get_program()))?;
    if !output.status.success() {
        bail!(
            "{:?} failed: {}",
            command.get_program(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

// Windows keeps everything in the registry
#[cfg_attr(windows, allow(dead_code))]
fn write(path: &Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
    }
    fs::write(path, contents).with_context(|| path.display().to_string())
}

#[cfg_attr(windows, allow(dead_code))]
fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| path.display().to_string())
        }
        _ => Ok(()),
    }
}

#[cfg_attr(windows, allow(dead_code))]
fn base_dirs() -> Result<directories::BaseDirs> {
    directories::BaseDirs::new().context("couldn't find the home directory")
}

/// How the daemon's set up to start, if it is, for saying so.
pub fn installed() -> Result<Option<&'static str>> {
    imp::installed()
}

/// Has the daemon start whenever the user logs in, and starts it now.
pub fn install(program: &Program) -> Result<()> {
    imp::install(program)
}

/// Stops the daemon starting when the user logs in. It's left running, if
/// it is; [`super::stop`] stops it.
pub fn uninstall() -> Result<()> {
    imp::uninstall()
}

/// Starts the daemon now, the way the system would at login if it's
/// installed, or with `program` if it isn't (or that's the only way).
pub fn start(program: &Program) -> Result<()> {
    imp::start(program)
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    const UNIT: &str = "patchwork-sync.service";

    fn unit_file() -> Result<PathBuf> {
        Ok(base_dirs()?.config_dir().join("systemd/user").join(UNIT))
    }

    fn autostart_file() -> Result<PathBuf> {
        Ok(base_dirs()?
            .config_dir()
            .join("autostart/patchwork-sync.desktop"))
    }

    fn systemctl() -> Command {
        let mut command = Command::new("systemctl");
        command.arg("--user");
        command
    }

    /// Whether there's a systemd user manager to install into.
    fn has_systemd() -> bool {
        systemctl()
            .arg("show-environment")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    pub fn installed() -> Result<Option<&'static str>> {
        if unit_file()?.is_file() {
            Ok(Some("a systemd user service"))
        } else if autostart_file()?.is_file() {
            Ok(Some("an autostart entry"))
        } else {
            Ok(None)
        }
    }

    pub fn install(program: &Program) -> Result<()> {
        if !has_systemd() {
            write(&autostart_file()?, &desktop_entry(program))?;
            return program.spawn();
        }
        write(&unit_file()?, &unit(program))?;
        run(systemctl().arg("daemon-reload"))?;
        run(systemctl().args(["enable", UNIT]))?;
        // restart rather than start, so an older one's replaced
        run(systemctl().args(["restart", UNIT]))
    }

    pub fn uninstall() -> Result<()> {
        let unit = unit_file()?;
        if unit.is_file() {
            run(systemctl().args(["disable", UNIT]))?;
            remove(&unit)?;
            run(systemctl().arg("daemon-reload"))?;
        }
        remove(&autostart_file()?)
    }

    pub fn start(program: &Program) -> Result<()> {
        if unit_file()?.is_file() {
            run(systemctl().args(["start", UNIT]))
        } else {
            program.spawn()
        }
    }

    fn unit(program: &Program) -> String {
        // systemd's own quoting, with % and $ escaped from its expansions
        let quote = |s: &str| {
            let s = s
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%")
                .replace('$', "$$");
            format!("\"{s}\"")
        };
        let mut exec = quote(&program.exe.to_string_lossy());
        for arg in &program.args {
            exec.push(' ');
            exec.push_str(&quote(arg));
        }
        let mut env = String::new();
        for (key, value) in &program.env {
            env.push_str(&format!(
                "Environment={}\n",
                quote(&format!("{key}={}", value.to_string_lossy()))
            ));
        }
        format!(
            "# Written by patchwork; remove it from Patchwork's settings, or with\n\
             # `patchwork daemon uninstall`.\n\
             [Unit]\n\
             Description=Patchwork sync\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             ExecStart={exec}\n\
             {env}\
             Restart=on-failure\n\
             RestartSec=10\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n"
        )
    }

    fn desktop_entry(program: &Program) -> String {
        // the desktop entry spec's quoting
        let quote = |s: &str| {
            let s = s
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('`', "\\`")
                .replace('$', "\\$");
            format!("\"{s}\"")
        };
        let mut exec = String::new();
        if !program.env.is_empty() {
            exec.push_str("env ");
            for (key, value) in &program.env {
                exec.push_str(&quote(&format!("{key}={}", value.to_string_lossy())));
                exec.push(' ');
            }
        }
        exec.push_str(&quote(&program.exe.to_string_lossy()));
        for arg in &program.args {
            exec.push(' ');
            exec.push_str(&quote(arg));
        }
        // a literal % has to be doubled in Exec
        let exec = exec.replace('%', "%%");
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Patchwork sync\n\
             Comment=Keeps Patchwork profiles in step with your other computers\n\
             Exec={exec}\n\
             Terminal=false\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n"
        )
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    const LABEL: &str = "dev.patchwork.sync";

    fn plist_file() -> Result<PathBuf> {
        Ok(base_dirs()?
            .home_dir()
            .join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist")))
    }

    /// launchctl's name for the user's login session.
    fn domain() -> Result<String> {
        let output = Command::new("id")
            .arg("-u")
            .output()
            .context("running id")?;
        let uid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !output.status.success() || uid.is_empty() {
            bail!("couldn't find this user's id");
        }
        Ok(format!("gui/{uid}"))
    }

    pub fn installed() -> Result<Option<&'static str>> {
        Ok(plist_file()?.is_file().then_some("a LaunchAgent"))
    }

    pub fn install(program: &Program) -> Result<()> {
        let plist = plist_file()?;
        write(&plist, &property_list(program))?;
        let domain = domain()?;
        // an older one has to go before the new one's loaded
        let _ =
            run(Command::new("launchctl").args(["bootout", format!("{domain}/{LABEL}").as_str()]));
        run(Command::new("launchctl")
            .args(["bootstrap", domain.as_str()])
            .arg(&plist))
    }

    pub fn uninstall() -> Result<()> {
        let plist = plist_file()?;
        if plist.is_file() {
            // fails if it isn't loaded, which is just as well
            let _ = run(Command::new("launchctl")
                .args(["bootout", format!("{}/{LABEL}", domain()?).as_str()]));
            remove(&plist)?;
        }
        Ok(())
    }

    pub fn start(program: &Program) -> Result<()> {
        if plist_file()?.is_file() {
            run(Command::new("launchctl")
                .args(["kickstart", format!("{}/{LABEL}", domain()?).as_str()]))
        } else {
            program.spawn()
        }
    }

    fn property_list(program: &Program) -> String {
        let escape = |s: &str| {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        let mut args = format!(
            "<string>{}</string>",
            escape(&program.exe.to_string_lossy())
        );
        for arg in &program.args {
            args.push_str(&format!("<string>{}</string>", escape(arg)));
        }
        let mut env = String::new();
        for (key, value) in &program.env {
            env.push_str(&format!(
                "<key>{}</key><string>{}</string>",
                escape(key),
                escape(&value.to_string_lossy())
            ));
        }
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- Written by patchwork; remove it from Patchwork's settings, or with `patchwork daemon uninstall`. -->
<plist version="1.0">
<dict>
	<key>Label</key><string>{LABEL}</string>
	<key>ProgramArguments</key><array>{args}</array>
	<key>EnvironmentVariables</key><dict>{env}</dict>
	<key>RunAtLoad</key><true/>
	<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
	<key>ProcessType</key><string>Background</string>
</dict>
</plist>
"#
        )
    }
}

#[cfg(windows)]
mod imp {
    use super::*;

    const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
    const VALUE: &str = "PatchworkSync";

    fn reg() -> Command {
        quiet("reg.exe")
    }

    pub fn installed() -> Result<Option<&'static str>> {
        let found = reg()
            .args(["query", RUN_KEY, "/v", VALUE])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        Ok(found.then_some("a startup entry"))
    }

    pub fn install(program: &Program) -> Result<()> {
        // a Run value can't set environment variables, so a PATCHWORK_HOME
        // install can't have a daemon that starts at login
        if !program.env.is_empty() {
            bail!(
                "the sync daemon can't start at login with PATCHWORK_HOME or \
                 PATCHWORK_REGISTRY_PATH set; run `patchwork daemon run` instead"
            );
        }
        let mut line = format!("\"{}\"", program.exe.display());
        for arg in &program.args {
            line.push_str(&format!(" \"{arg}\""));
        }
        run(reg().args([
            "add",
            RUN_KEY,
            "/v",
            VALUE,
            "/t",
            "REG_SZ",
            "/d",
            line.as_str(),
            "/f",
        ]))?;
        program.spawn()
    }

    pub fn uninstall() -> Result<()> {
        if installed()?.is_some() {
            run(reg().args(["delete", RUN_KEY, "/v", VALUE, "/f"]))?;
        }
        Ok(())
    }

    pub fn start(program: &Program) -> Result<()> {
        program.spawn()
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod imp {
    use super::*;

    pub fn installed() -> Result<Option<&'static str>> {
        Ok(None)
    }

    pub fn install(_: &Program) -> Result<()> {
        bail!("patchwork can't install the sync daemon on this system; run `patchwork daemon run`")
    }

    pub fn uninstall() -> Result<()> {
        Ok(())
    }

    pub fn start(program: &Program) -> Result<()> {
        program.spawn()
    }
}
