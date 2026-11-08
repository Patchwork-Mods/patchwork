//! Starting games, in one of the ways their definitions list (see
//! [`LaunchDefs`]). What's run comes from there, and what runs it from the
//! first of its [`runner`]s that can.
//!
//! By default, a game Steam installed is started through Steam unless it
//! needs another exe or environment variables, which Steam can't be asked
//! for. Wine DLL
//! overrides don't stop that: they're written into the Proton prefix's
//! registry first, the way r2modman does, since Steam's launch can't be
//! given `WINEDLLOVERRIDES`.
//!
//! Otherwise the exe is run directly: as it is on Windows and for native
//! builds, and on other systems through the Proton that Steam last ran it
//! with, or Wine for a prefix of the user's own. Proton is run inside the
//! Steam Linux Runtime container it asks for, as Steam runs it, since
//! newer ones don't work outside it; on NixOS that in turn is run through
//! `steam-run`, which gives it the usual Linux file layout.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::deploy::{DeployState, Os};
use crate::error::IoContext;
use crate::game::{GameDef, LaunchDef, LaunchDefs};
use crate::locate::{self, SteamInstall};
use crate::patchwork::GameInstall;
use crate::relpath::RelPath;
use crate::settings::{Origin, SettingState};
use crate::{Error, Result};

pub mod runner;

pub use runner::{Fit, RunContext, Runner, Runners};

/// DLLs that mods put next to a game's exe to be loaded in place of the
/// system's: loaders like BepInEx's (`winhttp`), ReShade and ENB (`dxgi`,
/// `d3d11`, ...), and ASI loaders (`dinput8`, `version`). Windows loads them
/// from the game's folder by itself, but Wine prefers its own copies unless
/// told otherwise.
const PROXY_DLLS: &[&str] = &[
    "d3d8",
    "d3d9",
    "d3d10",
    "d3d10core",
    "d3d11",
    "d3d12",
    "dbghelp",
    "dinput",
    "dinput8",
    "dsound",
    "dxgi",
    "opengl32",
    "version",
    "winhttp",
    "wininet",
    "winmm",
    "xinput1_3",
    "xinput1_4",
    "xinput9_1_0",
];

/// One way to start a game, as offered to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchOption {
    pub id: String,
    pub name: String,
    /// Whether it's what's used when the user doesn't pick one.
    pub default: bool,
    /// Why it can't be used right now, if it can't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

/// The ways `game`, installed in `install`, can be started: the usual way
/// first, then the definition's others by id. The default is `preferred` (the
/// user's pick) if it can be used, otherwise what the definition says.
pub fn options(game: &GameDef, install: &Path, preferred: Option<&str>) -> Vec<LaunchOption> {
    let usual = game.launch.get(LaunchDefs::GAME);
    let mut options = vec![LaunchOption {
        id: LaunchDefs::GAME.to_owned(),
        name: usual
            .and_then(|d| d.name.clone())
            .unwrap_or_else(|| game.name.clone()),
        default: false,
        unavailable: usual.and_then(|d| missing_exe(d, install)),
    }];
    options.extend(
        game.launch
            .iter()
            .filter(|(id, _)| *id != LaunchDefs::GAME)
            .map(|(id, def)| LaunchOption {
                id: id.to_owned(),
                name: def.name.clone().unwrap_or_else(|| id.to_owned()),
                default: false,
                unavailable: missing_exe(def, install),
            }),
    );
    let usable = |id: &&str| {
        options
            .iter()
            .any(|o| o.id == *id && o.unavailable.is_none())
    };
    let default = preferred
        .filter(usable)
        .or_else(|| {
            game.launch
                .iter()
                .find(|(_, def)| def.default)
                .map(|(id, _)| id)
                .filter(usable)
        })
        .unwrap_or(LaunchDefs::GAME);
    for option in &mut options {
        option.default = option.id == default;
    }
    options
}

fn missing_exe(def: &LaunchDef, install: &Path) -> Option<String> {
    let exe = def.exe.as_ref()?;
    (!exe.under(install).is_file()).then(|| format!("{exe} isn't in the game's folder"))
}

/// What's known about a game's install that decides how it's started.
#[derive(Debug, Clone)]
pub struct Setup {
    pub os: Os,
    /// The game's folder.
    pub install: PathBuf,
    /// The Steam app it is, if Steam installed it.
    pub steam: Option<SteamInstall>,
    /// The Steam install that has it, which Proton needs.
    pub steam_client: Option<PathBuf>,
    /// The Wine prefix it runs in on systems other than Windows.
    pub prefix: Option<PathBuf>,
    /// The Proton that last ran `prefix`, if Proton made it.
    pub proton: Option<PathBuf>,
    /// The Steam Linux Runtime `proton` runs inside, if it needs one: its
    /// app id, and its folder if it's installed.
    pub runtime: Option<(u32, Option<PathBuf>)>,
    /// What gives programs from outside the system, like Proton's, the
    /// usual Linux file layout, where it doesn't have one (NixOS).
    pub fhs: Option<PathBuf>,
    /// Files patchwork has deployed to the game's folder.
    pub deployed: Vec<RelPath>,
    /// The launch option the user picked, used when none is asked for.
    pub launch: Option<String>,
    /// The runner the user picked, used instead of the launch's own list.
    pub runner: Option<String>,
    /// The user's settings for runners, by runner id.
    pub runner_settings: BTreeMap<String, toml::Table>,
}

impl Setup {
    /// Looks at `game` as installed by `install`, with `state` deployed to
    /// it.
    pub fn find(game: &GameDef, install: &GameInstall, state: &DeployState) -> Self {
        let os = Os::current();
        let steam = locate::steam_install(game, &install.path);
        let steam_client = steam
            .as_ref()
            .and_then(|s| locate::steam_client(&s.library));
        let prefix = match (&install.prefix, os) {
            (Some(prefix), _) => Some(prefix.clone()),
            (None, Os::Windows) => None,
            (None, _) => locate::wine_prefix(game, &install.path),
        };
        // only a prefix Proton made can be run by one, chosen or not
        let compat_data = prefix
            .as_deref()
            .filter(|p| p.file_name().is_some_and(|n| n == "pfx"))
            .and_then(Path::parent);
        let proton = compat_data.and_then(|compat_data| {
            setting(&install.runners, runner::PROTON, runner::PROTON_FOLDER.key)
                .map(PathBuf::from)
                .or_else(|| locate::proton_install(compat_data))
        });
        let runtime = proton
            .as_deref()
            .and_then(locate::proton_runtime_app)
            .map(|app| {
                let path = steam_client
                    .as_deref()
                    .and_then(|client| locate::steam_app_path(client, app));
                (app, path)
            });
        let deployed = state
            .files
            .keys()
            .filter(|dest| dest.root.is_game())
            .map(|dest| dest.path.clone())
            .collect();
        Self {
            os,
            install: install.path.clone(),
            steam,
            steam_client,
            prefix,
            proton,
            runtime,
            fhs: fhs_wrapper(os),
            deployed,
            launch: install.launch.clone(),
            runner: install.runner.clone(),
            runner_settings: install.runners.clone(),
        }
    }

    /// The user's setting `key` for the runner `runner`, if it's text.
    pub fn setting(&self, runner: &str, key: &str) -> Option<&str> {
        setting(&self.runner_settings, runner, key)
    }
}

fn setting<'a>(
    settings: &'a BTreeMap<String, toml::Table>,
    runner: &str,
    key: &str,
) -> Option<&'a str> {
    settings.get(runner)?.get(key)?.as_str()
}

/// `steam-run` on NixOS, whose programs don't use the usual Linux file
/// layout that Steam's own expect.
pub(crate) fn fhs_wrapper(os: Os) -> Option<PathBuf> {
    if os != Os::Linux || !Path::new("/etc/NIXOS").exists() {
        return None;
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join("steam-run"))
        .find(|p| p.is_file())
}

/// A command that starts a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Launch {
    /// The [`LaunchOption`] it's for.
    pub option: String,
    pub via: Via,
    pub program: PathBuf,
    pub args: Vec<String>,
    /// Set on top of patchwork's own environment.
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    /// DLL overrides to make sure are in a Proton prefix's registry first,
    /// for a launch that can't set `WINEDLLOVERRIDES`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_overrides: Option<PrefixOverrides>,
}

/// Wine DLL overrides for a prefix's registry: the DLLs to load the game's
/// own copies of before Wine's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefixOverrides {
    /// The folder with `drive_c` and `user.reg` in it.
    pub prefix: PathBuf,
    pub dlls: Vec<String>,
}

/// What a [`Launch`] runs the game through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Via {
    /// Steam starts it, the way its Play button does.
    Steam { app: u32 },
    /// The exe is run as it is.
    Direct,
    /// The exe is run by the Proton at `proton`.
    Proton { proton: PathBuf },
    /// The exe is run by Wine, in the prefix the user set.
    Wine { prefix: PathBuf },
    /// The exe is run by a runner defined by data (see [`runner::Template`]).
    Runner { name: String },
}

impl fmt::Display for Via {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Via::Steam { app } => write!(f, "Steam (app {app})"),
            Via::Direct => f.write_str("directly"),
            Via::Proton { proton } => write!(f, "Proton at {}", proton.display()),
            Via::Wine { prefix } => write!(f, "Wine in {}", prefix.display()),
            Via::Runner { name } => write!(f, "with {name}"),
        }
    }
}

/// How to start `game` the way `option` says (by id), or the default way,
/// with the first of its runners that can.
pub fn plan(
    game: &GameDef,
    option: Option<&str>,
    setup: &Setup,
    runners: &Runners<'_>,
) -> Result<Launch> {
    let prepared = Prepared::new(game, option, setup)?;
    let ctx = prepared.context(game, setup);
    let ids = match &setup.runner {
        Some(runner) => std::slice::from_ref(runner),
        None => prepared.def.runners.as_slice(),
    };
    runners.pick(ids, &ctx)?.plan(&ctx)
}

/// What's to be run for a launch option, before it's known what runs it.
struct Prepared {
    option: String,
    def: LaunchDef,
    exe: Option<PathBuf>,
    env: BTreeMap<String, String>,
    overrides: Vec<String>,
}

impl Prepared {
    fn new(game: &GameDef, option: Option<&str>, setup: &Setup) -> Result<Self> {
        let options = options(game, &setup.install, setup.launch.as_deref());
        let chosen = match option {
            Some(id) => options.iter().find(|o| o.id == id).ok_or_else(|| {
                let ids: Vec<&str> = options.iter().map(|o| o.id.as_str()).collect();
                Error::invalid(format!(
                    "{} can't be launched as `{id}`; it can be {}",
                    game.name,
                    ids.join(", ")
                ))
            })?,
            None => options
                .iter()
                .find(|o| o.default)
                .expect("one option is always the default"),
        };
        if let Some(why) = &chosen.unavailable {
            return Err(Error::invalid(format!(
                "can't launch {}: {why}",
                chosen.name
            )));
        }
        let def = game.launch.get(&chosen.id).cloned().unwrap_or_default();

        let exe = match &def.exe {
            Some(exe) => Some(exe.clone()),
            None => usual_exe(game, &setup.install),
        };
        let under_wine = setup.os != Os::Windows
            && exe
                .as_ref()
                .is_some_and(|e| e.file_name().to_ascii_lowercase().ends_with(".exe"));
        let overrides = if under_wine {
            dll_overrides(&def, exe.as_ref(), &setup.deployed)
        } else {
            Vec::new()
        };
        let mut env = def.env.clone();
        if let Some(steam) = &setup.steam {
            // Without these, a Steam game started outside Steam asks Steam to
            // start it again, the usual way.
            env.insert("SteamAppId".to_owned(), steam.app.to_string());
            env.insert("SteamGameId".to_owned(), steam.app.to_string());
        }
        Ok(Self {
            option: chosen.id.clone(),
            exe: exe.map(|e| e.under(&setup.install)),
            env,
            overrides,
            def,
        })
    }

    fn context<'a>(&'a self, game: &'a GameDef, setup: &'a Setup) -> RunContext<'a> {
        RunContext {
            game,
            setup,
            option: &self.option,
            exe: self.exe.clone(),
            args: &self.def.args,
            env: &self.env,
            overrides: &self.overrides,
            custom: self.def.exe.is_some() || !self.def.env.is_empty(),
        }
    }
}

/// A runner, as offered to the user for a game.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerChoice {
    pub id: String,
    pub name: String,
    /// Why it can't run the game's default launch option, if it can't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
    pub settings: Vec<SettingState>,
}

/// The runners that can be picked for a game.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerChoices {
    /// The id of the runner used if none is picked, if one can be.
    pub automatic: Option<String>,
    /// The one the user picked.
    pub picked: Option<String>,
    pub runners: Vec<RunnerChoice>,
}

/// Every runner there is, and how each would do with `game`'s default launch
/// option. A runner whose definition is broken is listed with what's wrong.
pub fn runner_choices(game: &GameDef, setup: &Setup, runners: &Runners<'_>) -> RunnerChoices {
    let prepared = Prepared::new(game, None, setup);
    let ctx = prepared.as_ref().ok().map(|p| p.context(game, setup));
    let automatic = prepared
        .as_ref()
        .ok()
        .zip(ctx.as_ref())
        .and_then(|(p, ctx)| {
            let runner = runners.pick(&p.def.runners, ctx).ok()?;
            Some(runner.id().to_owned())
        });
    let choices = runners
        .ids()
        .into_iter()
        .map(|id| {
            let runner = match runners.get(&id) {
                Ok(runner) => runner,
                Err(e) => {
                    return RunnerChoice {
                        name: id.clone(),
                        id,
                        unavailable: Some(e.to_string()),
                        settings: Vec::new(),
                    };
                }
            };
            let unavailable = match (&prepared, &ctx) {
                (Err(e), _) => Some(e.to_string()),
                (_, Some(ctx)) => match runner.check(ctx) {
                    Fit::Ready => None,
                    Fit::Unsuited(why) | Fit::Missing(why) => Some(why),
                },
                (_, None) => None,
            };
            let set = setup.runner_settings.get(&id);
            let settings = runner
                .settings()
                .into_iter()
                .map(|setting| {
                    let value = set.and_then(|t| t.get(setting.spec.key)).cloned();
                    SettingState {
                        origin: value.as_ref().map(|_| Origin::File),
                        value,
                        ..setting
                    }
                })
                .collect();
            RunnerChoice {
                name: runner.name().to_owned(),
                id,
                unavailable,
                settings,
            }
        })
        .collect();
    RunnerChoices {
        automatic,
        picked: setup.runner.clone(),
        runners: choices,
    }
}

/// The id of the [`Launch::option`] that [`plan_tool`] makes.
pub const TOOL: &str = "tool";

/// How to run the tool at `exe`, which can be anywhere, for `game`: the
/// way the game's own exe would be run directly, so a Windows tool runs in
/// the game's Wine prefix and finds it there.
pub fn plan_tool(
    game: &GameDef,
    setup: &Setup,
    exe: &Path,
    args: Vec<String>,
    runners: &Runners<'_>,
) -> Result<Launch> {
    if !exe.is_file() {
        return Err(Error::invalid(format!("{} isn't a file", exe.display())));
    }
    let env = BTreeMap::new();
    // what the user picked, if it can run tools, which Steam can't
    let ids: Vec<String> = match &setup.runner {
        Some(runner) => std::iter::once(runner.as_str())
            .chain(runner::DEFAULT.iter().copied())
            .map(str::to_owned)
            .collect(),
        None => Vec::new(),
    };
    let ctx = RunContext {
        game,
        setup,
        option: TOOL,
        exe: Some(exe.to_owned()),
        args: &args,
        env: &env,
        overrides: &[],
        custom: true,
    };
    runners.pick(&ids, &ctx)?.plan(&ctx)
}

/// The exe the game usually starts with: the first of its definition's
/// `detect.exe` that's in its folder.
fn usual_exe(game: &GameDef, install: &Path) -> Option<RelPath> {
    game.detect
        .exe
        .iter()
        .filter_map(|exe| RelPath::new(exe).ok())
        .find(|exe| exe.under(install).is_file())
}

/// The DLLs Wine should load the game's own copies of: the ones `def`
/// asks for, and [`PROXY_DLLS`] that patchwork deployed next to `exe`.
fn dll_overrides(def: &LaunchDef, exe: Option<&RelPath>, deployed: &[RelPath]) -> Vec<String> {
    let dir = exe.and_then(RelPath::parent);
    let mut dlls: Vec<String> = def.dll_overrides.iter().map(|d| d.to_lowercase()).collect();
    for file in deployed {
        let same_dir = match (file.parent(), &dir) {
            (None, None) => true,
            (Some(a), Some(b)) => a.as_str().eq_ignore_ascii_case(b.as_str()),
            _ => false,
        };
        let name = file.file_name().to_ascii_lowercase();
        if let Some(stem) = name.strip_suffix(".dll")
            && same_dir
            && PROXY_DLLS.contains(&stem)
            && !dlls.iter().any(|d| d == stem)
        {
            dlls.push(stem.to_owned());
        }
    }
    dlls
}

/// Whether `prefix` is one Proton made, in a `compatdata/<app>` folder.
fn is_proton_prefix(prefix: &Path) -> bool {
    prefix.file_name().is_some_and(|n| n == "pfx")
        && prefix
            .parent()
            .and_then(Path::parent)
            .and_then(Path::file_name)
            .is_some_and(|n| n == "compatdata")
}

/// The program that tells Steam to start a game, and its first arguments.
fn steam_command(os: Os, client: Option<&Path>) -> Result<(PathBuf, Vec<String>)> {
    Ok(match os {
        Os::Windows => {
            let client =
                client.ok_or_else(|| Error::invalid("can't find where Steam is installed"))?;
            (client.join("steam.exe"), Vec::new())
        }
        Os::Macos => (
            PathBuf::from("open"),
            vec!["-a".to_owned(), "Steam".to_owned(), "--args".to_owned()],
        ),
        Os::Linux => {
            let flatpak = client.is_some_and(|c| {
                c.components()
                    .any(|part| part.as_os_str() == "com.valvesoftware.Steam")
            });
            if flatpak {
                (
                    PathBuf::from("flatpak"),
                    vec!["run".to_owned(), "com.valvesoftware.Steam".to_owned()],
                )
            } else {
                (PathBuf::from("steam"), Vec::new())
            }
        }
    })
}

/// How long [`Launch::spawn`] watches a game start for it failing.
pub const SETTLE: Duration = Duration::from_secs(3);

/// What [`Launch::spawn`] did besides starting the game.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Started {
    /// The DLLs it told the prefix's Wine to load the game's own copies of,
    /// which it hadn't been already.
    pub added_overrides: Vec<String>,
}

/// The `user.reg` key that DLL overrides for every program go in.
const DLL_OVERRIDES_KEY: &str = r"[Software\\Wine\\DllOverrides]";

/// Makes sure Wine in `prefix` loads the game's own copies of `dlls`
/// before its own, by its `user.reg`, and returns the ones that weren't
/// already. Wine keeps the registry in memory while anything runs in the
/// prefix and writes it back afterwards, so this only sticks while nothing
/// does.
pub fn add_prefix_overrides(prefix: &Path, dlls: &[String]) -> Result<Vec<String>> {
    let reg = prefix.join("user.reg");
    let text = match std::fs::read_to_string(&reg) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::invalid(format!(
                "{} hasn't been set up by Wine yet; start the game from Steam once, then try again",
                prefix.display()
            )));
        }
        text => text.at(&reg)?,
    };
    let (text, added) = with_overrides(&text, dlls);
    if !added.is_empty() {
        let tmp = prefix.join("user.reg.patchwork");
        std::fs::write(&tmp, text).at(&tmp)?;
        std::fs::rename(&tmp, &reg).at(&reg)?;
    }
    Ok(added)
}

/// `user_reg` with `dlls` overridden to native-then-builtin, and the ones
/// that weren't already. Ones already set to load the native copy first are
/// left as they are.
fn with_overrides(user_reg: &str, dlls: &[String]) -> (String, Vec<String>) {
    let mut lines: Vec<String> = user_reg.lines().map(str::to_owned).collect();
    let header = lines.iter().position(|l| {
        l.get(..DLL_OVERRIDES_KEY.len())
            .is_some_and(|h| h.eq_ignore_ascii_case(DLL_OVERRIDES_KEY))
    });
    let mut added = Vec::new();
    let Some(header) = header else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        if lines.last().is_some_and(|l| !l.is_empty()) {
            lines.push(String::new());
        }
        lines.push(format!("{DLL_OVERRIDES_KEY} {now}"));
        for dll in dlls {
            lines.push(format!("\"{dll}\"=\"native,builtin\""));
            added.push(dll.clone());
        }
        return (lines.join("\n") + "\n", added);
    };
    // the values go after the header and Wine's `#time=` comment
    let mut insert_at = header + 1;
    while lines.get(insert_at).is_some_and(|l| l.starts_with('#')) {
        insert_at += 1;
    }
    for dll in dlls {
        let end = lines[header + 1..]
            .iter()
            .position(|l| l.starts_with('['))
            .map_or(lines.len(), |i| header + 1 + i);
        let name = format!("\"{dll}\"=");
        let existing = lines[header + 1..end]
            .iter()
            .position(|l| l.to_ascii_lowercase().starts_with(&name))
            .map(|i| header + 1 + i);
        let value = format!("{name}\"native,builtin\"");
        match existing {
            Some(i) => {
                let current = lines[i][name.len()..]
                    .trim_matches('"')
                    .to_ascii_lowercase();
                if !current.starts_with('n') {
                    lines[i] = value;
                    added.push(dll.clone());
                }
            }
            None => {
                lines.insert(insert_at, value);
                added.push(dll.clone());
            }
        }
    }
    (lines.join("\n") + "\n", added)
}

impl Launch {
    /// Starts the game, with what it prints going to `log`, and returns
    /// once it's clear it didn't fail straight away (see [`SETTLE`]). Says
    /// which DLL overrides it had to add to the prefix's registry first.
    pub fn spawn(&self, log: &Path) -> Result<Started> {
        let added_overrides = match &self.prefix_overrides {
            Some(o) => add_prefix_overrides(&o.prefix, &o.dlls)?,
            None => Vec::new(),
        };
        let mut command = self.command(log)?;
        let mut child = command.spawn().at(&self.program)?;
        let since = Instant::now();
        while since.elapsed() < SETTLE {
            // Steam's command hands the game to Steam and exits, fine; one
            // that fails this soon is the game, or Proton, not starting
            if let Some(status) = child.try_wait().at(&self.program)? {
                if status.success() {
                    return Ok(Started { added_overrides });
                }
                let output = std::fs::read_to_string(log).unwrap_or_default();
                let tail: Vec<&str> = output.lines().rev().take(15).collect();
                let tail: Vec<&str> = tail.into_iter().rev().collect();
                return Err(Error::invalid(format!(
                    "the game didn't start ({status}){}; everything it printed is in {}",
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!(":\n{}", tail.join("\n"))
                    },
                    log.display()
                )));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // reaped in the background, so a long-running patchwork doesn't
        // collect finished games
        std::thread::spawn(move || child.wait());
        Ok(Started { added_overrides })
    }

    /// Runs it, with what it prints going to `log`, and waits for it to
    /// finish.
    pub fn run(&self, log: &Path) -> Result<ExitStatus> {
        self.command(log)?
            .spawn()
            .at(&self.program)?
            .wait()
            .at(&self.program)
    }

    /// The command that runs it, with what it prints going to `log`.
    fn command(&self, log: &Path) -> Result<Command> {
        if let Some(dir) = log.parent() {
            std::fs::create_dir_all(dir).at(dir)?;
        }
        // an instance that's had nothing deployed to it yet
        std::fs::create_dir_all(&self.cwd).at(&self.cwd)?;
        let out = std::fs::File::create(log).at(log)?;
        let err = out.try_clone().at(log)?;
        // Started by a shell that waits for it, rather than by patchwork,
        // since what gives it the usual Linux file layout (`steam-run`'s
        // bubblewrap) ends along with whatever started it, and patchwork
        // can exit first.
        #[cfg(unix)]
        let mut command = {
            let mut command = Command::new("/bin/sh");
            command
                .args(["-c", r#""$@"; exit $?"#, "sh"])
                .arg(&self.program)
                .args(&self.args);
            command
        };
        #[cfg(not(unix))]
        let mut command = {
            let mut command = Command::new(&self.program);
            command.args(&self.args);
            command
        };
        command
            .envs(&self.env)
            .current_dir(&self.cwd)
            .stdin(Stdio::null())
            .stdout(out)
            .stderr(err);
        // so the game outlives patchwork, and doesn't get its Ctrl-C
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        #[cfg(windows)]
        {
            const DETACHED_PROCESS: u32 = 0x8;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x200;
            std::os::windows::process::CommandExt::creation_flags(
                &mut command,
                DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP,
            );
        }
        Ok(command)
    }

    /// Whether the game needs Steam to be running already. Games run from
    /// Steam start it themselves.
    pub fn needs_steam(&self) -> bool {
        self.env.contains_key("SteamAppId")
    }
}

impl fmt::Display for Launch {
    /// The command as it could be typed into a shell.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (key, value) in &self.env {
            write!(f, "{key}={} ", quote(value))?;
        }
        write!(f, "{}", quote(&self.program.to_string_lossy()))?;
        for arg in &self.args {
            write!(f, " {}", quote(arg))?;
        }
        Ok(())
    }
}

fn quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,+@%".contains(c));
    if plain {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

/// Whether Steam looks to be running, where that can be told: on Linux,
/// from the pid it writes to `~/.steam/steam.pid`.
pub fn steam_running() -> Option<bool> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let home = directories::BaseDirs::new()?.home_dir().to_owned();
    let pid = std::fs::read_to_string(home.join(".steam/steam.pid")).ok()?;
    let pid: u32 = pid.trim().parse().ok()?;
    Some(Path::new(&format!("/proc/{pid}")).exists())
}

#[cfg(test)]
mod tests;
