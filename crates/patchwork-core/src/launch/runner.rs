//! Runners: what actually runs a game once [`super::plan`] knows which exe,
//! arguments and environment it wants.
//!
//! A game's `[launch.<id>]` can list `runners`, tried in order. The first
//! that's [`Fit::Ready`] is used; without a list it's [`DEFAULT`]. Most are
//! built in or registered by extensions, and simple ones are data, in
//! `registry/runners/<id>.toml` (see [`Template`]), the way the `rules`
//! deployer's rulesets are.
//!
//! The user can pick one for a game, which is used instead of the list, and
//! give runners settings; both are kept with the game's install (see
//! [`crate::GameInstall`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use globset::{GlobBuilder, GlobMatcher};
use serde::Deserialize;

use super::{Launch, PrefixOverrides, Setup, Via, is_proton_prefix, steam_command};
use crate::deploy::Os;
use crate::game::GameDef;
use crate::registry::Registry;
use crate::settings::{SettingKind, SettingSpec, SettingState};
use crate::{Error, Result};

/// The registry kind that [`Template`]s are kept under.
pub const KIND: &str = "runners";

/// The runners used by a launch that doesn't list any.
pub const DEFAULT: &[&str] = &["steam", "native", "proton", "wine"];

pub(super) const PROTON: &str = "proton";
pub(super) const PROTON_FOLDER: SettingSpec =
    SettingSpec::new("folder", "Proton", SettingKind::Path)
        .help("The Proton folder to use, instead of the one Steam last ran the game with.");
const WINE: &str = "wine";
const WINE_PROGRAM: SettingSpec = SettingSpec::new("program", "Wine program", SettingKind::Text)
    .help("The Wine to run, by name or path, such as a build of your own.");

/// A setting with nothing set yet, for [`Runner::settings`].
fn unset(spec: SettingSpec, default: Option<&str>) -> SettingState {
    SettingState {
        spec,
        value: None,
        default: default.map(toml::Value::from),
        origin: None,
    }
}

/// Whether a [`Runner`] can run something, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fit {
    Ready,
    /// It isn't the right kind of runner for this, like Wine for a native
    /// build. Only said to the user if there's nothing better to say.
    Unsuited(String),
    /// It would be, but something it needs is missing, which is what the
    /// user has to be told if nothing after it can run the game either.
    Missing(String),
}

impl Fit {
    fn into_error(self) -> Error {
        match self {
            Fit::Ready => Error::invalid("the runner said it was ready and then wasn't"),
            Fit::Unsuited(why) | Fit::Missing(why) => Error::invalid(why),
        }
    }
}

/// What a [`Runner`] is asked to run.
#[derive(Debug, Clone)]
pub struct RunContext<'a> {
    pub game: &'a GameDef,
    pub setup: &'a Setup,
    /// The [`Launch::option`] to give what it plans.
    pub option: &'a str,
    /// What to run. `None` if the game's definition doesn't say and none of
    /// the files it lists are there, which only Steam can do without.
    pub exe: Option<PathBuf>,
    pub args: &'a [String],
    pub env: &'a BTreeMap<String, String>,
    /// Wine DLL overrides it needs, if it's run through Wine.
    pub overrides: &'a [String],
    /// Whether this is something other than the game started the usual way,
    /// which Steam can't be asked for.
    pub custom: bool,
}

impl RunContext<'_> {
    fn exe(&self) -> std::result::Result<&Path, Fit> {
        self.exe.as_deref().ok_or_else(|| {
            Fit::Missing(format!(
                "patchwork doesn't know which file starts {}; its definition needs `exe` in `[launch.{}]` or `[detect]`",
                self.game.name, self.option
            ))
        })
    }

    /// Whether `exe` is a Windows program on a system that isn't Windows.
    fn needs_wine(&self, exe: &Path) -> bool {
        self.setup.os != Os::Windows
            && exe
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    }

    /// The exe, for a runner that only runs Windows programs elsewhere.
    fn windows_exe(&self, runner: &str) -> std::result::Result<&Path, Fit> {
        let exe = self.exe()?;
        if self.needs_wine(exe) {
            Ok(exe)
        } else {
            Err(Fit::Unsuited(format!(
                "{runner} is for running Windows programs on other systems, which {} isn't",
                exe.display()
            )))
        }
    }

    fn cwd(&self, exe: &Path) -> PathBuf {
        exe.parent().unwrap_or(&self.setup.install).to_owned()
    }

    fn needs_prefix(&self) -> Fit {
        let game = &self.game.name;
        let id = &self.game.id;
        Fit::Missing(if self.setup.steam.is_none() {
            format!(
                "{game} is a Windows game that isn't from Steam, so patchwork needs to know which Wine prefix it runs in: `patchwork game set {id} prefix <path>`"
            )
        } else {
            format!(
                "patchwork needs to know which Wine prefix {game} runs in: `patchwork game set {id} prefix <path>`"
            )
        })
    }

    /// `env`, with its DLL overrides added to `WINEDLLOVERRIDES`.
    fn wine_env(&self) -> BTreeMap<String, String> {
        let mut env = self.env.clone();
        if !self.overrides.is_empty() {
            let mut value = self
                .overrides
                .iter()
                .map(|dll| format!("{dll}=n,b"))
                .collect::<Vec<_>>()
                .join(";");
            if let Some(own) = env.get("WINEDLLOVERRIDES") {
                value = format!("{value};{own}");
            }
            env.insert("WINEDLLOVERRIDES".to_owned(), value);
        }
        env
    }
}

/// A way of running a game. It only describes the command; the host runs it.
pub trait Runner: Send + Sync {
    /// What games' `runners` lists call it. Lowercase letters, digits and
    /// `-`, and it can never change once released.
    fn id(&self) -> &str;

    /// What the user sees it called.
    fn name(&self) -> &str {
        self.id()
    }

    /// The settings it takes, unset and with what they default to. It finds
    /// what they're set to with [`Setup::setting`].
    fn settings(&self) -> Vec<SettingState> {
        Vec::new()
    }

    /// Whether it can run what `ctx` says. Only looks, and doesn't change
    /// anything.
    fn check(&self, ctx: &RunContext<'_>) -> Fit;

    /// The command to run. Only called after [`Runner::check`] said
    /// [`Fit::Ready`].
    fn plan(&self, ctx: &RunContext<'_>) -> Result<Launch>;
}

/// Finds runners by id: [`Template`]s from the registry first, so a user's
/// file can replace any other, then ones registered with the builder, then
/// the built-in ones.
pub struct Runners<'a> {
    registered: &'a BTreeMap<String, Arc<dyn Runner>>,
    registry: &'a Registry,
}

impl<'a> Runners<'a> {
    pub fn new(registered: &'a BTreeMap<String, Arc<dyn Runner>>, registry: &'a Registry) -> Self {
        Self {
            registered,
            registry,
        }
    }

    pub fn get(&self, id: &str) -> Result<Arc<dyn Runner>> {
        if let Some(entry) = self.registry.data(KIND, id) {
            return Ok(Arc::new(Template::parse(id, &entry.value)?));
        }
        if let Some(runner) = self.registered.get(id) {
            return Ok(runner.clone());
        }
        Ok(match id {
            "steam" => Arc::new(Steam),
            "native" => Arc::new(Native),
            "proton" => Arc::new(Proton),
            "wine" => Arc::new(Wine),
            _ => return Err(Error::invalid(format!("no runner named `{id}`"))),
        })
    }

    /// The ids of every runner, the usual ones first.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = DEFAULT.iter().map(|id| (*id).to_owned()).collect();
        let others: BTreeSet<&str> = self
            .registry
            .data_ids(KIND)
            .chain(self.registered.keys().map(String::as_str))
            .collect();
        for id in others {
            if !DEFAULT.contains(&id) {
                ids.push(id.to_owned());
            }
        }
        ids
    }

    /// The first of `ids` that can run what `ctx` says, or [`DEFAULT`]'s if
    /// `ids` is empty.
    pub fn pick(&self, ids: &[String], ctx: &RunContext<'_>) -> Result<Arc<dyn Runner>> {
        let ids: Vec<&str> = if ids.is_empty() {
            DEFAULT.to_vec()
        } else {
            ids.iter().map(String::as_str).collect()
        };
        let mut missing = None;
        let mut unsuited = None;
        for id in &ids {
            let runner = self.get(id)?;
            match runner.check(ctx) {
                Fit::Ready => return Ok(runner),
                Fit::Missing(why) => {
                    missing.get_or_insert(why);
                }
                Fit::Unsuited(why) => {
                    unsuited.get_or_insert(why);
                }
            }
        }
        Err(Error::invalid(missing.or(unsuited).unwrap_or_else(|| {
            format!("nothing can run {}", ctx.game.name)
        })))
    }
}

/// Steam starts it, the way its Play button does.
struct Steam;

struct SteamCommand {
    app: u32,
    program: PathBuf,
    args: Vec<String>,
    prefix_overrides: Option<PrefixOverrides>,
}

impl Steam {
    fn find(ctx: &RunContext<'_>) -> std::result::Result<SteamCommand, Fit> {
        let Some(steam) = &ctx.setup.steam else {
            return Err(Fit::Unsuited(format!(
                "{} wasn't installed by Steam",
                ctx.game.name
            )));
        };
        // Steam can't be asked to start another exe or set variables
        if ctx.custom {
            return Err(Fit::Unsuited(
                "Steam can only start a game's usual exe, without changing its environment"
                    .to_owned(),
            ));
        }
        let prefix_overrides = if ctx.overrides.is_empty() {
            None
        } else {
            let prefix = ctx.setup.prefix.clone().ok_or_else(|| {
                Fit::Missing(format!(
                    "can't find {}'s Proton prefix, where Wine has to be told to load its mods' {}; start it from Steam once so Proton sets it up, then try again",
                    ctx.game.name,
                    ctx.overrides
                        .iter()
                        .map(|d| format!("{d}.dll"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
            Some(PrefixOverrides {
                prefix,
                dlls: ctx.overrides.to_vec(),
            })
        };
        let (program, args) = steam_command(ctx.setup.os, ctx.setup.steam_client.as_deref())
            .map_err(|e| Fit::Missing(e.to_string()))?;
        Ok(SteamCommand {
            app: steam.app,
            program,
            args,
            prefix_overrides,
        })
    }
}

impl Runner for Steam {
    fn id(&self) -> &str {
        "steam"
    }

    fn name(&self) -> &str {
        "Steam"
    }

    fn check(&self, ctx: &RunContext<'_>) -> Fit {
        Self::find(ctx).map_or_else(|fit| fit, |_| Fit::Ready)
    }

    fn plan(&self, ctx: &RunContext<'_>) -> Result<Launch> {
        let found = Self::find(ctx).map_err(Fit::into_error)?;
        let mut args = found.args;
        args.extend(["-applaunch".to_owned(), found.app.to_string()]);
        args.extend(ctx.args.iter().cloned());
        Ok(Launch {
            option: ctx.option.to_owned(),
            via: Via::Steam { app: found.app },
            program: found.program,
            args,
            env: BTreeMap::new(),
            cwd: ctx.setup.install.clone(),
            prefix_overrides: found.prefix_overrides,
        })
    }
}

/// The exe is run as it is: on Windows, and for native builds.
struct Native;

impl Native {
    fn find<'a>(ctx: &'a RunContext<'_>) -> std::result::Result<&'a Path, Fit> {
        let exe = ctx.exe()?;
        if ctx.needs_wine(exe) {
            return Err(Fit::Unsuited(format!(
                "{} is a Windows program, so it can't run by itself here",
                exe.display()
            )));
        }
        Ok(exe)
    }
}

impl Runner for Native {
    fn id(&self) -> &str {
        "native"
    }

    fn name(&self) -> &str {
        "Run directly"
    }

    fn check(&self, ctx: &RunContext<'_>) -> Fit {
        Self::find(ctx).map_or_else(|fit| fit, |_| Fit::Ready)
    }

    fn plan(&self, ctx: &RunContext<'_>) -> Result<Launch> {
        let exe = Self::find(ctx).map_err(Fit::into_error)?;
        Ok(Launch {
            option: ctx.option.to_owned(),
            via: Via::Direct,
            program: exe.to_owned(),
            args: ctx.args.to_vec(),
            env: ctx.env.clone(),
            cwd: ctx.cwd(exe),
            prefix_overrides: None,
        })
    }
}

/// The exe is run by the Proton that Steam last ran the game with, the way
/// Steam runs it: inside its runtime, if it has one, and through what gives
/// it the usual Linux file layout, where there isn't one.
struct Proton;

struct ProtonCommand<'a> {
    exe: &'a Path,
    prefix: &'a Path,
    proton: &'a Path,
    client: &'a Path,
    /// The runtime's folder, if it has one.
    runtime: Option<&'a Path>,
}

impl Proton {
    fn find<'a>(ctx: &'a RunContext<'_>) -> std::result::Result<ProtonCommand<'a>, Fit> {
        let exe = ctx.windows_exe("Proton")?;
        let setup = ctx.setup;
        let game = &ctx.game.name;
        if setup.steam.is_none() {
            return Err(Fit::Unsuited(format!(
                "patchwork only knows which Proton runs games from Steam, which {game} isn't"
            )));
        }
        let (Some(prefix), Some(proton)) = (&setup.prefix, &setup.proton) else {
            // a prefix of the user's own is run with Wine instead
            if setup
                .prefix
                .as_deref()
                .is_some_and(|p| !is_proton_prefix(p))
            {
                return Err(Fit::Unsuited(format!(
                    "{game} uses a Wine prefix Proton didn't make"
                )));
            }
            return Err(Fit::Missing(format!(
                "can't find which Proton runs {game}; start it from Steam once so Proton sets it up, then try again"
            )));
        };
        if setup.setting(PROTON, PROTON_FOLDER.key).is_some() && !proton.join("proton").is_file() {
            return Err(Fit::Missing(format!(
                "there's no Proton in {}, which {game}'s settings say to use",
                proton.display()
            )));
        }
        let client = setup.steam_client.as_deref().ok_or_else(|| {
            Fit::Missing(format!(
                "can't find the Steam install that has {}, which Proton needs",
                setup.install.display()
            ))
        })?;
        let runtime = match &setup.runtime {
            None => None,
            Some((_, Some(runtime))) => Some(runtime.as_path()),
            Some((app, None)) => {
                return Err(Fit::Missing(format!(
                    "{} needs the Steam Linux Runtime (Steam app {app}) to run in, and it isn't installed; start {game} from Steam once so it installs it",
                    proton.display(),
                )));
            }
        };
        Ok(ProtonCommand {
            exe,
            prefix,
            proton,
            client,
            runtime,
        })
    }
}

impl Runner for Proton {
    fn id(&self) -> &str {
        PROTON
    }

    fn name(&self) -> &str {
        "Proton"
    }

    fn settings(&self) -> Vec<SettingState> {
        vec![unset(PROTON_FOLDER, None)]
    }

    fn check(&self, ctx: &RunContext<'_>) -> Fit {
        Self::find(ctx).map_or_else(|fit| fit, |_| Fit::Ready)
    }

    fn plan(&self, ctx: &RunContext<'_>) -> Result<Launch> {
        let found = Self::find(ctx).map_err(Fit::into_error)?;
        let mut env = ctx.wine_env();
        let compat_data = found
            .prefix
            .parent()
            .expect("a prefix Proton made is in a folder");
        env.insert(
            "STEAM_COMPAT_DATA_PATH".to_owned(),
            compat_data.to_string_lossy().into_owned(),
        );
        env.insert(
            "STEAM_COMPAT_CLIENT_INSTALL_PATH".to_owned(),
            found.client.to_string_lossy().into_owned(),
        );
        // waiting for the game so the runtime stays up while it runs
        let mut command: Vec<PathBuf> = Vec::new();
        let verb = match found.runtime {
            None => "run",
            Some(runtime) => {
                command.extend([
                    runtime.join("_v2-entry-point"),
                    "--verb=waitforexitandrun".into(),
                    "--".into(),
                ]);
                "waitforexitandrun"
            }
        };
        command.push(found.proton.join("proton"));
        command.extend([verb.into(), found.exe.to_owned()]);
        if let Some(fhs) = &ctx.setup.fhs {
            command.insert(0, fhs.clone());
        }
        let mut command = command
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned());
        let program = PathBuf::from(command.next().expect("there's always Proton"));
        Ok(Launch {
            option: ctx.option.to_owned(),
            via: Via::Proton {
                proton: found.proton.to_owned(),
            },
            program,
            args: command.chain(ctx.args.iter().cloned()).collect(),
            env,
            cwd: ctx.cwd(found.exe),
            prefix_overrides: None,
        })
    }
}

/// The exe is run by the system's Wine, in the prefix the user set.
struct Wine;

impl Wine {
    fn find<'a>(ctx: &'a RunContext<'_>) -> std::result::Result<(&'a Path, &'a Path), Fit> {
        let exe = ctx.windows_exe("Wine")?;
        let Some(prefix) = ctx.setup.prefix.as_deref() else {
            return Err(ctx.needs_prefix());
        };
        if ctx.setup.steam.is_some() && is_proton_prefix(prefix) {
            return Err(Fit::Unsuited(format!(
                "{} is a prefix Proton made, which Wine shouldn't run things in",
                prefix.display()
            )));
        }
        if let Some(program) = ctx.setup.setting(WINE, WINE_PROGRAM.key)
            && !installed(program)
        {
            return Err(Fit::Missing(format!(
                "patchwork couldn't find {program}, which {}'s settings say to run it with",
                ctx.game.name
            )));
        }
        Ok((exe, prefix))
    }
}

impl Runner for Wine {
    fn id(&self) -> &str {
        WINE
    }

    fn name(&self) -> &str {
        "Wine"
    }

    fn settings(&self) -> Vec<SettingState> {
        vec![unset(WINE_PROGRAM, Some("wine"))]
    }

    fn check(&self, ctx: &RunContext<'_>) -> Fit {
        Self::find(ctx).map_or_else(|fit| fit, |_| Fit::Ready)
    }

    fn plan(&self, ctx: &RunContext<'_>) -> Result<Launch> {
        let (exe, prefix) = Self::find(ctx).map_err(Fit::into_error)?;
        let mut env = ctx.wine_env();
        env.insert(
            "WINEPREFIX".to_owned(),
            prefix.to_string_lossy().into_owned(),
        );
        Ok(Launch {
            option: ctx.option.to_owned(),
            via: Via::Wine {
                prefix: prefix.to_owned(),
            },
            program: PathBuf::from(ctx.setup.setting(WINE, WINE_PROGRAM.key).unwrap_or("wine")),
            args: std::iter::once(exe.to_string_lossy().into_owned())
                .chain(ctx.args.iter().cloned())
                .collect(),
            env,
            cwd: ctx.cwd(exe),
            prefix_overrides: None,
        })
    }
}

/// A runner defined by data: a command to fill in.
///
/// ```toml
/// # registry/runners/umu.toml
/// name = "umu"
/// # `${args...}` has to be a whole argument, and becomes the launch's
/// # arguments. The others are `${exe}`, `${install}` (the game's folder),
/// # `${prefix}` and `${steam_app}`; a runner that uses one that isn't known
/// # for a game can't run it.
/// command = ["umu-run", "${exe}", "${args...}"]
/// env = { WINEPREFIX = "${prefix}", PROTONPATH = "${proton}" }
/// # Settings the user can change, used as `${proton}` above. A command
/// # argument or variable that uses one with no value is left out.
/// [settings.proton]
/// label = "Proton"
/// kind = "path"     # or "text", which is the default
/// help = "The Proton folder to use."
/// # default = "..."
/// # What it's for. Leave either out if it doesn't matter.
/// [applies]
/// exe = "*.exe"     # matched against the exe's file name, ignoring case
/// os = ["linux"]    # any of windows, linux, macos
/// ```
pub struct Template {
    id: String,
    def: TemplateDef,
    exe: Option<GlobMatcher>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateDef {
    name: Option<String>,
    command: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    applies: Applies,
    #[serde(default)]
    settings: BTreeMap<String, TemplateSetting>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateSetting {
    label: String,
    #[serde(default)]
    kind: TemplateSettingKind,
    help: Option<String>,
    default: Option<String>,
}

#[derive(Debug, Default, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum TemplateSettingKind {
    #[default]
    Text,
    Path,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Applies {
    exe: Option<String>,
    #[serde(default)]
    os: Vec<Os>,
}

/// What stands for the launch's arguments in a [`Template`]'s command.
const ARGS: &str = "${args...}";

/// What a [`Template`]'s settings can't be called, since its placeholders
/// already mean these.
const RESERVED: &[&str] = &["exe", "install", "prefix", "steam_app", "args..."];

/// `text`, kept for good so it can be in a [`SettingSpec`]. Each is only kept
/// once, however many times a [`Template`] is parsed.
fn interned(text: &str) -> &'static str {
    static KEPT: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(found) = kept.get(text) {
        return found;
    }
    let text: &'static str = Box::leak(text.to_owned().into_boxed_str());
    kept.insert(text);
    text
}

impl Template {
    pub fn parse(id: &str, table: &toml::Table) -> Result<Self> {
        let bad = |why: String| Error::invalid(format!("bad runner `{id}`: {why}"));
        let def: TemplateDef = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e: toml::de::Error| bad(e.to_string()))?;
        if def.command.first().is_none_or(|program| program == ARGS) {
            return Err(bad("`command` has to start with a program".to_owned()));
        }
        if let Some(key) = def.settings.keys().find(|k| RESERVED.contains(&k.as_str())) {
            return Err(bad(format!("a setting can't be called `{key}`")));
        }
        let exe = def
            .applies
            .exe
            .as_deref()
            .map(|pattern| {
                GlobBuilder::new(pattern)
                    .case_insensitive(true)
                    .build()
                    .map(|glob| glob.compile_matcher())
                    .map_err(|e| bad(format!("`{pattern}` isn't a valid pattern: {e}")))
            })
            .transpose()?;
        Ok(Self {
            id: id.to_owned(),
            def,
            exe,
        })
    }

    /// What `${name}`s are filled in with for `ctx`.
    fn vars(&self, ctx: &RunContext<'_>) -> BTreeMap<String, String> {
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let setup = ctx.setup;
        let mut vars = BTreeMap::from([("install".to_owned(), path(&setup.install))]);
        if let Some(exe) = &ctx.exe {
            vars.insert("exe".to_owned(), path(exe));
        }
        if let Some(prefix) = &setup.prefix {
            vars.insert("prefix".to_owned(), path(prefix));
        }
        if let Some(steam) = &setup.steam {
            vars.insert("steam_app".to_owned(), steam.app.to_string());
        }
        for (key, setting) in &self.def.settings {
            let value = setup.setting(&self.id, key).or(setting.default.as_deref());
            if let Some(value) = value {
                vars.insert(key.clone(), value.to_owned());
            }
        }
        vars
    }

    /// Whether `text` uses a setting that has no value.
    fn lacks_setting(&self, text: &str, vars: &BTreeMap<String, String>) -> bool {
        placeholders(text)
            .any(|name| self.def.settings.contains_key(name) && !vars.contains_key(name))
    }

    /// What's stopping a placeholder in the template being filled, if
    /// anything is.
    fn unfilled(&self, ctx: &RunContext<'_>, vars: &BTreeMap<String, String>) -> Option<Fit> {
        let texts = self
            .def
            .command
            .iter()
            .filter(|arg| *arg != ARGS)
            .chain(self.def.env.values());
        if self.lacks_setting(&self.def.command[0], vars) {
            return Some(Fit::Missing(format!(
                "{} has to be told which program to run, in {}'s settings",
                self.name(),
                ctx.game.name
            )));
        }
        for name in texts.flat_map(|text| placeholders(text)) {
            if vars.contains_key(name) || self.def.settings.contains_key(name) {
                continue;
            }
            return Some(match name {
                "exe" => ctx.exe().err()?,
                "prefix" => ctx.needs_prefix(),
                "steam_app" => Fit::Unsuited(format!(
                    "{} needs a game from Steam, which {} isn't",
                    self.name(),
                    ctx.game.name
                )),
                _ => Fit::Unsuited(format!(
                    "runner `{}` uses `${{{name}}}`, which patchwork doesn't know about",
                    self.id
                )),
            });
        }
        None
    }
}

/// The names of `text`'s `${name}` placeholders.
fn placeholders(text: &str) -> impl Iterator<Item = &str> {
    text.split("${")
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
}

/// Whether `program` can be run: it's a file, if it's a path, otherwise it's
/// on `PATH`.
fn installed(program: &str) -> bool {
    let path = Path::new(program);
    if path.components().count() > 1 {
        return path.is_file();
    }
    std::env::var_os("PATH").is_some_and(|dirs| {
        std::env::split_paths(&dirs)
            .any(|dir| dir.join(program).is_file() || dir.join(format!("{program}.exe")).is_file())
    })
}

impl Runner for Template {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        self.def.name.as_deref().unwrap_or(&self.id)
    }

    fn settings(&self) -> Vec<SettingState> {
        self.def
            .settings
            .iter()
            .map(|(key, setting)| {
                let kind = match setting.kind {
                    TemplateSettingKind::Text => SettingKind::Text,
                    TemplateSettingKind::Path => SettingKind::Path,
                };
                let mut spec = SettingSpec::new(interned(key), interned(&setting.label), kind);
                spec.help = setting.help.as_deref().map(interned);
                unset(spec, setting.default.as_deref())
            })
            .collect()
    }

    fn check(&self, ctx: &RunContext<'_>) -> Fit {
        let applies = &self.def.applies;
        if !applies.os.is_empty() && !applies.os.contains(&ctx.setup.os) {
            return Fit::Unsuited(format!("{} isn't for this system", self.name()));
        }
        if let Some(pattern) = &self.exe {
            let exe = match ctx.exe() {
                Ok(exe) => exe,
                Err(fit) => return fit,
            };
            if !exe.file_name().is_some_and(|name| pattern.is_match(name)) {
                return Fit::Unsuited(format!("{} doesn't run {}", self.name(), exe.display()));
            }
        }
        let vars = self.vars(ctx);
        if let Some(fit) = self.unfilled(ctx, &vars) {
            return fit;
        }
        let program = fill(&self.def.command[0], &vars);
        if !installed(&program) {
            return Fit::Missing(format!(
                "{} needs {program}, and patchwork couldn't find it",
                self.name()
            ));
        }
        Fit::Ready
    }

    fn plan(&self, ctx: &RunContext<'_>) -> Result<Launch> {
        let vars = self.vars(ctx);
        if let Some(fit) = self.unfilled(ctx, &vars) {
            return Err(fit.into_error());
        }
        let mut command = self.def.command.iter().flat_map(|arg| {
            if arg == ARGS {
                ctx.args.to_vec()
            } else if self.lacks_setting(arg, &vars) {
                Vec::new()
            } else {
                vec![fill(arg, &vars)]
            }
        });
        let program = PathBuf::from(command.next().expect("parsing checked there's a program"));
        let args = command.collect();
        // the game's own variables are more specific than the runner's
        let mut env: BTreeMap<String, String> = self
            .def
            .env
            .iter()
            .filter(|(_, value)| !self.lacks_setting(value, &vars))
            .map(|(key, value)| (key.clone(), fill(value, &vars)))
            .collect();
        env.extend(ctx.wine_env());
        Ok(Launch {
            option: ctx.option.to_owned(),
            via: Via::Runner {
                name: self.name().to_owned(),
            },
            program,
            args,
            env,
            cwd: ctx
                .exe
                .as_deref()
                .map_or_else(|| ctx.setup.install.clone(), |exe| ctx.cwd(exe)),
            prefix_overrides: None,
        })
    }
}

/// `arg` with its `${name}` placeholders that are in `vars` filled in.
/// Others are left as they are.
pub(super) fn fill(arg: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(arg.len());
    let mut rest = arg;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let name = &after[..end];
                match vars.get(name) {
                    Some(value) => out.push_str(value),
                    None => out.push_str(&rest[start..start + 2 + end + 1]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}
