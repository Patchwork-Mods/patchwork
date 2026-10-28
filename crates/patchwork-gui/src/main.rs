// Don't open a console window alongside the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod changelog;
mod cli;
mod daemon;
mod details;
mod selfupdate;
mod sync;

use std::collections::BTreeMap;
use std::fmt::Display;
use std::sync::Mutex;

use base64::prelude::*;
use patchwork_core::conflicts::ModConflicts;
use patchwork_core::deploy::{Check, DeployOp, DeployPlan, DeployTarget, RootOrigin};
use patchwork_core::environment::GameEnv;
use patchwork_core::event::{Event, Reporter, Silent};
use patchwork_core::fomod::{InstallerView, Selection};
use patchwork_core::game::{DeployerSpec, GameDef, LinkMethod};
use patchwork_core::issue::Issue;
use patchwork_core::launch::{self, LaunchOption, Via};
use patchwork_core::lock::{Lockfile, mod_key};
use patchwork_core::matching::{self, ModText};
use patchwork_core::order::SortKey;
use patchwork_core::paths::Paths;
use patchwork_core::plugins::{LoadOrder, PluginEntry};
use patchwork_core::profile::{ProfileMod, ProfileRef};
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::settings::{Origin, SettingState};
use patchwork_core::source::{ModSpec, SourceFile};
use patchwork_core::{DeployStatus, GameInstall, HistoryEntry, ImportOptions, Patchwork};
use patchwork_deployers::{BethesdaDeployer, RulesDeployer, SmfDeployer};
use patchwork_formats::{Archive, R2modman, ThunderstoreCode, UrlList};
use patchwork_sources::{LocalSource, Nexus, Thunderstore, UrlSource};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// Commands fail with a message for the UI to show.
type CmdResult<T> = Result<T, String>;

fn msg(e: impl Display) -> String {
    e.to_string()
}

/// [`msg`] for commands that lock. When locking is waiting on mods the user
/// has to download themselves, the window is sent them as
/// `patchwork://pending` to work through.
fn lock_msg(handle: &AppHandle) -> impl Fn(patchwork_core::Error) -> String + '_ {
    move |e| match e {
        patchwork_core::Error::NeedsDownload(pending) => {
            let message = match pending.len() {
                1 => "1 mod has to be downloaded from its page first".to_owned(),
                n => format!("{n} mods have to be downloaded from their pages first"),
            };
            let _ = handle.emit("patchwork://pending", pending);
            message
        }
        e => e.to_string(),
    }
}

struct App {
    pw: Patchwork,
    /// The plan from the last `plan_deploy`, waiting for the user to confirm
    /// it with `apply_plan`.
    plan: Mutex<Option<DeployPlan>>,
    details: details::Details,
    /// What an update has to wait for. Commands that write to games or
    /// download hold a guard from it while they run.
    work: selfupdate::Work,
    sync: sync::Sync,
    daemon: daemon::Daemon,
}

/// Forwards core progress to the window as `patchwork://event`.
struct Emit(AppHandle);

impl Reporter for Emit {
    fn report(&self, event: Event) {
        let event = match event {
            Event::Resolving { name } => UiEvent::Resolving { name },
            Event::Fetching { name } => UiEvent::Fetching { name },
            Event::Building { step } => UiEvent::Building { step },
            Event::Applying(op) => UiEvent::Applying(op.into()),
            Event::Warning(message) => UiEvent::Warning { message },
        };
        // Nothing useful to do if the window's gone.
        let _ = self.0.emit("patchwork://event", event);
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum UiEvent {
    Resolving { name: String },
    Fetching { name: String },
    Building { step: String },
    Applying(UiOp),
    Warning { message: String },
}

#[derive(Clone, Serialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum UiOp {
    Remove { dst: String },
    Backup { dst: String },
    Place { dst: String, mod_name: String },
}

impl From<DeployOp> for UiOp {
    fn from(op: DeployOp) -> Self {
        match op {
            DeployOp::Remove { dst } => UiOp::Remove {
                dst: dst.to_string(),
            },
            DeployOp::Backup { dst } => UiOp::Backup {
                dst: dst.to_string(),
            },
            DeployOp::Place { dst, mod_name, .. } => UiOp::Place {
                dst: dst.to_string(),
                mod_name,
            },
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Overview {
    games: Vec<GameInfo>,
    profiles: Vec<ProfileInfo>,
    current: Option<String>,
    /// Shared rule sets a new game's deployer can use.
    rulesets: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GameInfo {
    id: String,
    name: String,
    install: Option<String>,
    /// Where the definition came from, e.g. `builtin` or `builtin + user`.
    origin: String,
    /// Something an `<img>` can load: the definition's URL, or a local
    /// image file inlined as a data URL, since the webview can't read paths.
    image: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileInfo {
    id: String,
    label: String,
    game: String,
    name: String,
    mods: usize,
    /// `"sync"` if it's kept in step with the user's other computers or
    /// shared with friends, `"follow"` if this one only follows it.
    sync: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileDetail {
    id: String,
    label: String,
    game: String,
    name: String,
    mods: Vec<UiMod>,
    /// Mods with any of these tags go in the Mods tab's hidden section.
    hidden_tags: Vec<String>,
    /// Whether the game has plugins to put in order, on a Plugins tab.
    has_plugins: bool,
    /// Everything worth warning about, from [`Patchwork::issues`].
    issues: Vec<Issue>,
    /// The names of mods added here to a profile this computer follows,
    /// which whoever shares it doesn't have.
    unshared: Vec<String>,
    install: Option<String>,
    /// Files patchwork has deployed for this profile's game, if it's this
    /// profile that's deployed.
    deployed_files: Option<usize>,
    /// Files patchwork has deployed to the game, whichever profile they're
    /// from, so it can offer to undeploy them.
    game_files: usize,
    /// Whether deploying would change anything, so the button can say so.
    deploy_status: DeployStatus,
    /// Whether the profile has changed since it was last deployed, so it
    /// can be reverted.
    has_changes: bool,
    search_sources: Vec<String>,
    /// The ways the game can be started, for the Play button; none until
    /// it's known where it's installed.
    launch_options: Vec<LaunchOption>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiMod {
    #[serde(flatten)]
    profile: ProfileMod,
    /// `<source>:<id>`, which events about the mod name it by.
    key: String,
    /// What to show for its id; see [`patchwork_core::Patchwork::display_id`].
    shown_id: String,
    /// The version in the lockfile, if it's been locked.
    locked: Option<String>,
    /// Whether the mod's source has versions to pick from.
    versioned: bool,
    /// Its versions as last fetched, newest first, if they ever have been.
    /// `refresh_details` fetches them again once they're stale.
    versions: Option<Vec<String>>,
    /// What a search would show for it (picture, summary, ...), as last
    /// fetched. The same goes for it as for `versions`.
    summary: Option<ModSummary>,
    /// Whether it has a FOMOD installer its game uses, to pick options in.
    /// Only known once it's been downloaded.
    installer: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanSummary {
    ops: Vec<UiOp>,
    unchanged: usize,
    warnings: Vec<String>,
}

fn profile_ref(id: &str) -> CmdResult<ProfileRef> {
    id.parse().map_err(msg)
}

#[tauri::command]
fn overview(app: State<'_, App>) -> CmdResult<Overview> {
    let pw = &app.pw;
    // Picks up definitions dropped into the registry folders by hand.
    pw.reload_registry().map_err(msg)?;
    let registry = pw.registry();
    let games = registry
        .games()
        .filter(|entry| !entry.value.hidden)
        .map(|entry| {
            let game = &entry.value;
            Ok(GameInfo {
                id: game.id.clone(),
                name: game.name.clone(),
                install: pw
                    .game_install(&game.id)
                    .map_err(msg)?
                    .map(|i| i.path.display().to_string()),
                origin: entry.origins(),
                image: game.image.as_deref().and_then(image_src),
            })
        })
        .collect::<CmdResult<_>>()?;
    let mut warnings: Vec<String> = registry
        .problems
        .iter()
        .map(|p| format!("skipped registry entry: {p}"))
        .collect();
    let synced = patchwork_sync::synced_profiles(&pw.paths().sync()).unwrap_or_else(|e| {
        warnings.push(format!("couldn't read which profiles are synced: {e:#}"));
        Vec::new()
    });
    let profiles = pw
        .profiles()
        .map_err(msg)?
        .into_iter()
        .map(|(r, p)| ProfileInfo {
            id: r.to_string(),
            label: p.label(),
            mods: p.mods.len(),
            sync: synced
                .iter()
                .find(|s| s.profile == r.uuid)
                .map(|s| if s.writable { "sync" } else { "follow" }),
            game: p.game,
            name: p.name,
        })
        .collect();
    Ok(Overview {
        games,
        profiles,
        current: pw.current_profile().map_err(msg)?.map(|r| r.to_string()),
        rulesets: registry.data_ids("rulesets").map(str::to_owned).collect(),
        warnings,
    })
}

/// Turns a game's `image` into something the webview can show, or `None` if
/// it's a file that can't be read, so the card falls back to initials.
fn image_src(image: &str) -> Option<String> {
    if image.contains("://") || image.starts_with("data:") {
        return Some(image.to_owned());
    }
    let path = std::path::Path::new(image);
    let mime = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    let bytes = std::fs::read(path).ok()?;
    Some(format!(
        "data:{mime};base64,{}",
        BASE64_STANDARD.encode(bytes)
    ))
}

#[tauri::command]
fn profile(app: State<'_, App>, id: String) -> CmdResult<ProfileDetail> {
    profile_detail(&app.pw, id)
}

/// Everything the UI shows about a profile. Commands that change one return
/// this too, to save the UI asking again.
fn profile_detail(pw: &Patchwork, id: String) -> CmdResult<ProfileDetail> {
    let r = profile_ref(&id)?;
    let mut loaded = pw.load_profile(&r).map_err(msg)?;
    let install = pw
        .game_install(&loaded.game)
        .map_err(msg)?
        .map(|i| i.path.display().to_string());
    let target = pw.deploy_target(&loaded).map_err(msg)?;
    let state = pw.deploy_state(&target).map_err(msg)?;
    let deployed_files =
        (state.profile.as_deref() == Some(id.as_str())).then_some(state.files.len());
    let game_files = state.files.len();
    let deploy_status = pw.deploy_status(&r).map_err(msg)?;
    let has_changes = pw.has_changes(&r).map_err(msg)?;
    let search_sources = pw
        .search_sources(&loaded.game)
        .map_err(msg)?
        .into_iter()
        .map(str::to_owned)
        .collect();
    let lock = pw.load_lock(&r).map_err(msg)?;
    let issues = pw.issues(&r).map_err(msg)?;
    let following = patchwork_sync::synced_profiles(&pw.paths().sync())
        .unwrap_or_default()
        .into_iter()
        .any(|s| s.profile == r.uuid && !s.writable);
    let unshared = if following {
        pw.unshared_mods(&r).map_err(msg)?
    } else {
        Vec::new()
    };
    let has_plugins = pw.has_plugins(&r).map_err(msg)?;
    let installers = pw.installers(&r).map_err(msg)?;
    let launch_options = pw
        .launch_options(&loaded.game)
        .map_err(msg)?
        .unwrap_or_default();
    let label = loaded.label();
    let name = loaded.name.clone();
    let hidden_tags = std::mem::take(&mut loaded.hidden_tags);
    let mods = loaded
        .mods
        .into_iter()
        .map(|m| {
            let key = mod_key(&m.source, &m.id);
            let versioned = pw.lists_versions(&m.source).unwrap_or(false);
            let known = versioned
                .then(|| pw.known_versions(&loaded.game, &m.source, &m.id))
                .flatten();
            let summary = pw.known_summary(&loaded.game, &m.source, &m.id);
            UiMod {
                shown_id: pw.display_id(lock.as_ref(), &m),
                versions: known.map(|k| k.versions),
                summary: summary.and_then(|k| k.summary),
                versioned,
                locked: lock
                    .iter()
                    .flat_map(|l| &l.mods)
                    .find(|l| l.key() == key)
                    .and_then(|l| l.version.clone()),
                key,
                installer: installers.contains(&m.name),
                profile: m,
            }
        })
        .collect();
    Ok(ProfileDetail {
        label,
        name,
        game: loaded.game,
        mods,
        hidden_tags,
        has_plugins,
        issues,
        unshared,
        id,
        install,
        deployed_files,
        game_files,
        deploy_status,
        has_changes,
        search_sources,
        launch_options,
    })
}

/// What the "Add game" form collects.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct NewGame {
    id: String,
    name: String,
    install: Option<String>,
    /// Thunderstore community slug, e.g. `lethal-company`.
    thunderstore: Option<String>,
    /// Nexus game domain, e.g. `skyrimspecialedition`.
    nexus: Option<String>,
    rulesets: Vec<String>,
    link: LinkMethod,
}

/// Saves a new game definition, then points it at its install folder if one
/// was given.
#[tauri::command]
fn create_game(app: State<'_, App>, game: NewGame) -> CmdResult<()> {
    let filled = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let mut sources = std::collections::BTreeMap::new();
    if let Some(community) = filled(game.thunderstore) {
        sources.insert(
            "thunderstore".to_owned(),
            toml::Table::from_iter([("community".to_owned(), community.into())]),
        );
    }
    if let Some(domain) = filled(game.nexus) {
        sources.insert(
            "nexus".to_owned(),
            toml::Table::from_iter([("game_domain".to_owned(), domain.into())]),
        );
    }
    let mut config = toml::Table::new();
    if !game.rulesets.is_empty() {
        config.insert("rulesets".to_owned(), game.rulesets.into());
    }
    let def = GameDef {
        schema: 0,
        id: game.id.trim().to_owned(),
        name: game.name.trim().to_owned(),
        description: None,
        image: None,
        hidden: false,
        sources,
        deployer: DeployerSpec {
            kind: "rules".to_owned(),
            config,
        },
        link: game.link,
        detect: Default::default(),
        versions: Default::default(),
        env: Default::default(),
        roots: Default::default(),
        launch: Default::default(),
        fomod: None,
        capture: None,
    };
    app.pw.add_game(def.clone()).map_err(msg)?;
    if let Some(path) = filled(game.install) {
        app.pw.set_game_path(&def.id, path.as_ref()).map_err(|e| {
            format!(
                "{} was added, but its install folder couldn't be set: {e}",
                def.name
            )
        })?;
    }
    Ok(())
}

#[tauri::command]
fn create_profile(app: State<'_, App>, game: String, name: String) -> CmdResult<String> {
    let created = app.pw.create_profile(&game, &name).map_err(msg)?;
    app.pw.use_profile(&created).map_err(msg)?;
    Ok(created.to_string())
}

#[tauri::command]
fn use_profile(app: State<'_, App>, id: String) -> CmdResult<()> {
    app.pw.use_profile(&profile_ref(&id)?).map_err(msg)
}

#[tauri::command]
fn remove_profile(app: State<'_, App>, id: String) -> CmdResult<()> {
    app.pw.remove_profile(&profile_ref(&id)?).map_err(msg)
}

#[tauri::command]
fn rename_profile(app: State<'_, App>, id: String, name: String) -> CmdResult<()> {
    app.pw
        .rename_profile(&profile_ref(&id)?, &name)
        .map_err(msg)
}

/// Copies a profile to a new name, returning the copy's id.
#[tauri::command]
fn duplicate_profile(app: State<'_, App>, id: String, name: String) -> CmdResult<String> {
    let copy = app
        .pw
        .duplicate_profile(&profile_ref(&id)?, &name)
        .map_err(msg)?;
    Ok(copy.to_string())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FormatInfo {
    id: String,
    description: String,
    /// What exported files end in, or `None` for text (a code, links) to
    /// copy and paste.
    extension: Option<String>,
}

/// The formats `game`'s profiles can be exported to and imported from.
#[tauri::command]
fn profile_formats(app: State<'_, App>, game: String) -> CmdResult<Vec<FormatInfo>> {
    let registry = app.pw.registry();
    let game = &registry.game(&game).map_err(msg)?.value;
    Ok(app
        .pw
        .formats()
        .filter(|f| f.supports(game))
        .map(|f| FormatInfo {
            id: f.id().to_owned(),
            description: f.description().to_owned(),
            extension: f.extension().map(str::to_owned),
        })
        .collect())
}

/// Exports a profile. Writes it to `path` if given; otherwise it's text,
/// which is returned for the user to copy.
#[tauri::command]
async fn export_profile(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
    format: String,
    path: Option<String>,
) -> CmdResult<Option<String>> {
    let _work = app.work.start()?;
    let bytes = app
        .pw
        .export_profile(&profile_ref(&id)?, &format)
        .await
        .map_err(lock_msg(&handle))?;
    match path {
        Some(path) => {
            std::fs::write(&path, bytes).map_err(|e| format!("writing {path}: {e}"))?;
            Ok(None)
        }
        None => String::from_utf8(bytes).map(Some).map_err(msg),
    }
}

/// What to import a profile from: a file, or text that was pasted in.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum ImportInput {
    Path(String),
    Text(String),
}

/// Imports a profile for `game` and switches to it, returning its id.
#[tauri::command]
async fn import_profile(
    app: State<'_, App>,
    input: ImportInput,
    game: String,
    name: Option<String>,
    replace: bool,
) -> CmdResult<String> {
    let _work = app.work.start()?;
    let bytes = match input {
        ImportInput::Path(path) => {
            std::fs::read(&path).map_err(|e| format!("reading {path}: {e}"))?
        }
        ImportInput::Text(text) => text.trim().as_bytes().to_vec(),
    };
    let options = ImportOptions {
        format: None,
        game: Some(&game),
        name: name.as_deref().filter(|n| !n.is_empty()),
        replace,
    };
    let imported = app.pw.import_profile(&bytes, &options).await.map_err(msg)?;
    app.pw.use_profile(&imported.profile).map_err(msg)?;
    Ok(imported.profile.to_string())
}

#[tauri::command]
fn set_game_path(app: State<'_, App>, game: String, path: String) -> CmdResult<()> {
    app.pw
        .set_game_path(&game, path.as_ref())
        .map(drop)
        .map_err(msg)
}

/// Somewhere a game looks to be installed, for offering it as the game's
/// folder.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FoundInstall {
    path: String,
    /// What found it, e.g. `Steam library /home/me/.local/share/Steam`.
    origin: String,
}

#[tauri::command]
fn find_installs(app: State<'_, App>, game: String) -> CmdResult<Vec<FoundInstall>> {
    Ok(app
        .pw
        .find_installs(&game)
        .map_err(msg)?
        .into_iter()
        .map(|f| FoundInstall {
            path: f.path.display().to_string(),
            origin: f.origin.to_string(),
        })
        .collect())
}

/// A game's settings on this machine, for the game settings dialog.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GameSettings {
    /// `None` until it's been set up.
    install: Option<GameInstall>,
    /// What the game's definition says, for when `install` doesn't.
    default_link: LinkMethod,
    /// The settings the game's deployer takes.
    deployer: Vec<SettingState>,
    /// The game's other folders, for moving them.
    roots: Vec<RootInfo>,
    /// Whether it makes sense to set a Wine prefix: on systems other than
    /// Windows, for a game with any folders in one.
    uses_prefix: bool,
    /// The prefix the folders were looked for in.
    prefix: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RootInfo {
    id: String,
    /// Where it is, if that's known.
    path: Option<String>,
    /// Why it couldn't be found, otherwise.
    problem: Option<String>,
    origin: RootOrigin,
    allow: Vec<String>,
}

#[tauri::command]
fn game_settings(app: State<'_, App>, game: String) -> CmdResult<GameSettings> {
    let registry = app.pw.registry();
    let def = &registry.game(&game).map_err(msg)?.value;
    let resolved = app
        .pw
        .resolve_roots(&DeployTarget::Game(game.clone()))
        .map_err(msg)?;
    let roots = resolved
        .iter()
        .flat_map(|r| r.iter())
        .map(|(id, root)| RootInfo {
            id: id.to_string(),
            path: root.path.as_ref().ok().map(|p| p.display().to_string()),
            problem: root.path.as_ref().err().cloned(),
            origin: root.origin,
            allow: root.allow.iter().map(|a| a.as_str().to_owned()).collect(),
        })
        .collect();
    Ok(GameSettings {
        install: app.pw.game_install(&game).map_err(msg)?,
        default_link: def.link,
        deployer: app.pw.deployer_settings(&game).map_err(msg)?,
        roots,
        uses_prefix: !cfg!(windows) && !def.roots.is_empty(),
        prefix: resolved
            .as_ref()
            .and_then(|r| r.prefix())
            .map(|p| p.display().to_string()),
    })
}

/// A source's settings, for the settings dialog.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceSettings {
    id: String,
    name: String,
    settings: Vec<SettingState>,
}

/// Every source that takes settings, with what they're set to.
#[tauri::command]
fn source_settings(app: State<'_, App>) -> CmdResult<Vec<SourceSettings>> {
    app.pw
        .configurable_sources()
        .into_iter()
        .map(|(id, name)| {
            Ok(SourceSettings {
                id: id.to_owned(),
                name: name.to_owned(),
                settings: app.pw.source_settings(id).map_err(msg)?,
            })
        })
        .collect()
}

/// Sets one of a source's settings, or clears it if `value` is `None`.
/// Returns where it was saved.
#[tauri::command]
fn set_source_setting(
    app: State<'_, App>,
    source: String,
    key: String,
    value: Option<toml::Value>,
) -> CmdResult<Option<Origin>> {
    app.pw.set_source_setting(&source, &key, value).map_err(msg)
}

/// What a source makes of its settings, such as the account an API key
/// signs in to.
#[tauri::command]
async fn check_source_settings(app: State<'_, App>, source: String) -> CmdResult<Option<String>> {
    app.pw.check_source_settings(&source).await.map_err(msg)
}

/// What patchwork can tell about a game's install, such as its version, for
/// the game settings dialog. `None` until it's been set up.
#[tauri::command]
fn game_env(app: State<'_, App>, game: String) -> CmdResult<Option<GameEnvInfo>> {
    let env = app.pw.game_env(&DeployTarget::Game(game)).map_err(msg)?;
    Ok(env.map(|env| GameEnvInfo {
        summary: (!env.is_empty()).then(|| env.to_string()),
        env,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GameEnvInfo {
    #[serde(flatten)]
    env: GameEnv,
    /// How the CLI describes it, e.g. `1.6.1170 (Steam build 13190634)`.
    /// `None` when nothing could be found out.
    summary: Option<String>,
}

#[tauri::command]
fn set_game_settings(app: State<'_, App>, game: String, settings: GameInstall) -> CmdResult<()> {
    app.pw
        .set_game_install(&game, settings)
        .map(drop)
        .map_err(msg)
}

#[tauri::command]
fn set_enabled(
    app: State<'_, App>,
    id: String,
    name: String,
    enabled: bool,
) -> CmdResult<ProfileDetail> {
    app.pw
        .set_enabled(&profile_ref(&id)?, &name, enabled)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Undoes every change made to the profile since it was last deployed.
#[tauri::command]
fn revert_profile(app: State<'_, App>, id: String) -> CmdResult<ProfileDetail> {
    app.pw.revert(&profile_ref(&id)?).map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// The profile's past deploys, newest first.
#[tauri::command]
fn profile_history(app: State<'_, App>, id: String) -> CmdResult<Vec<HistoryEntry>> {
    let mut history = app.pw.history(&profile_ref(&id)?).map_err(msg)?;
    history.reverse();
    Ok(history)
}

/// Puts the profile back how it was at deploy `number` of its history.
#[tauri::command]
fn rollback_profile(app: State<'_, App>, id: String, number: u32) -> CmdResult<ProfileDetail> {
    app.pw.rollback(&profile_ref(&id)?, number).map_err(msg)?;
    profile_detail(&app.pw, id)
}

#[tauri::command]
fn remove_mod(app: State<'_, App>, id: String, name: String) -> CmdResult<()> {
    app.pw.remove_mod(&profile_ref(&id)?, &name).map_err(msg)
}

#[tauri::command]
fn set_hidden_tags(app: State<'_, App>, id: String, tags: Vec<String>) -> CmdResult<()> {
    app.pw
        .set_hidden_tags(&profile_ref(&id)?, &tags)
        .map_err(msg)
}

/// Sorts the profile's mods once by `keys` (written as `patchwork sort --by`
/// takes them, e.g. `dependents:desc`) and saves the new order. Returns the
/// order the mods were in before, for undoing it with `reorder_mods`, or
/// none if sorting didn't move anything.
#[tauri::command]
async fn sort_mods(
    app: State<'_, App>,
    id: String,
    keys: Vec<String>,
) -> CmdResult<Option<Vec<String>>> {
    let _work = app.work.start()?;
    let r = profile_ref(&id)?;
    let keys = keys
        .iter()
        .map(|k| k.parse::<SortKey>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(msg)?;
    let before: Vec<String> = app
        .pw
        .load_profile(&r)
        .map_err(msg)?
        .mods
        .into_iter()
        .map(|m| m.name)
        .collect();
    let after: Vec<String> = app
        .pw
        .sorted_mods(&r, &keys, &[])
        .await
        .map_err(msg)?
        .into_iter()
        .map(|m| m.name)
        .collect();
    if after == before {
        return Ok(None);
    }
    app.pw.reorder_mods(&r, &after).map_err(msg)?;
    Ok(Some(before))
}

/// Puts the profile's mods in the order of `names`, which has to name each
/// of them once.
#[tauri::command]
fn reorder_mods(app: State<'_, App>, id: String, names: Vec<String>) -> CmdResult<()> {
    app.pw.reorder_mods(&profile_ref(&id)?, &names).map_err(msg)
}

/// What the Plugins tab shows.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiLoadOrder {
    #[serde(flatten)]
    order: LoadOrder,
    /// Enabled mods that haven't been locked, whose plugins aren't known
    /// until they are; see [`lock_profile`].
    unlocked: Vec<String>,
}

fn ui_load_order(pw: &Patchwork, r: &ProfileRef) -> CmdResult<Option<UiLoadOrder>> {
    let Some(order) = pw.load_order(r).map_err(msg)? else {
        return Ok(None);
    };
    let lock = pw.load_lock(r).map_err(msg)?;
    let unlocked = pw
        .load_profile(r)
        .map_err(msg)?
        .mods
        .into_iter()
        .filter(|m| m.enabled)
        .filter(|m| {
            let key = mod_key(&m.source, &m.id);
            !lock.iter().flat_map(|l| &l.mods).any(|l| l.key() == key)
        })
        .map(|m| m.name)
        .collect();
    Ok(Some(UiLoadOrder { order, unlocked }))
}

/// The order the profile's plugins load in, for the Plugins tab.
#[tauri::command]
fn load_order(app: State<'_, App>, id: String) -> CmdResult<Option<UiLoadOrder>> {
    ui_load_order(&app.pw, &profile_ref(&id)?)
}

/// Saves the Plugins tab's order and switches, returning the load order as
/// it now is.
#[tauri::command]
fn set_load_order(
    app: State<'_, App>,
    id: String,
    plugins: Vec<PluginEntry>,
) -> CmdResult<Option<UiLoadOrder>> {
    let r = profile_ref(&id)?;
    app.pw.set_load_order(&r, plugins).map_err(msg)?;
    ui_load_order(&app.pw, &r)
}

/// A mod's installer as the installer dialog shows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiInstaller {
    #[serde(flatten)]
    view: InstallerView,
    /// The options' pictures that the dialog didn't say it already has, as
    /// something the webview can show, by their path in the mod.
    pictures: BTreeMap<String, String>,
}

/// `name`'s installer with `picks`, or with what the profile has saved.
/// `have` are the pictures the dialog already has, which aren't sent again.
/// Async so unpacking the mod happens off the main thread.
#[tauri::command]
async fn installer(
    app: State<'_, App>,
    id: String,
    name: String,
    picks: Option<Vec<Selection>>,
    have: Vec<String>,
) -> CmdResult<Option<UiInstaller>> {
    let Some(mut view) = app
        .pw
        .installer(&profile_ref(&id)?, &name, picks.as_deref())
        .map_err(msg)?
    else {
        return Ok(None);
    };
    let images = std::mem::take(&mut view.images);
    let pictures = images
        .into_iter()
        .filter(|(rel, _)| !have.contains(rel))
        .filter_map(|(rel, path)| Some((rel, image_src(path.to_str()?)?)))
        .collect();
    Ok(Some(UiInstaller { view, pictures }))
}

/// Saves the options picked in `name`'s installer, or with `None` goes back
/// to the ones it recommends.
#[tauri::command]
fn set_installer(
    app: State<'_, App>,
    id: String,
    name: String,
    picks: Option<Vec<Selection>>,
) -> CmdResult<ProfileDetail> {
    app.pw
        .set_installer(&profile_ref(&id)?, &name, picks)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// One mod's conflicts, for the Mods tab.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiModConflicts {
    name: String,
    #[serde(flatten)]
    conflicts: ModConflicts,
}

/// Which of the profile's mods provide the same files, and whose copies
/// win, in load order; see conflicts.rs in core. Only locked mods count.
/// Async so unpacking and comparing files happens off the main thread.
#[tauri::command]
async fn conflicts(app: State<'_, App>, id: String) -> CmdResult<Vec<UiModConflicts>> {
    let conflicts = app.pw.conflicts(&profile_ref(&id)?).map_err(msg)?;
    Ok(conflicts
        .by_mod()
        .into_iter()
        .map(|(name, conflicts)| UiModConflicts {
            name: name.to_owned(),
            conflicts,
        })
        .collect())
}

/// Starts fetching whatever's out of date about the profile's mods' versions
/// and summaries in the background; see details.rs.
#[tauri::command]
fn refresh_details(handle: AppHandle, id: String) -> CmdResult<()> {
    details::refresh(&handle, &profile_ref(&id)?)
}

/// The ids of the games on the game picker whose names match `query`, best
/// first, matched the same way as a profile's mods.
#[tauri::command]
fn find_games(app: State<'_, App>, query: String) -> Vec<String> {
    let registry = app.pw.registry();
    let games: Vec<&GameDef> = registry
        .games()
        .filter(|entry| !entry.value.hidden)
        .map(|entry| &entry.value)
        .collect();
    let texts: Vec<ModText> = games
        .iter()
        .map(|g| ModText {
            name: &g.name,
            author: None,
            summary: None,
        })
        .collect();
    matching::rank(&texts, &query)
        .into_iter()
        .map(|i| games[i].id.clone())
        .collect()
}

/// The keys of the profile's mods matching `query`, best first; see
/// `patchwork_core::matching`.
#[tauri::command]
async fn find_mods(app: State<'_, App>, id: String, query: String) -> CmdResult<Vec<String>> {
    let found = app.pw.find_mods(&profile_ref(&id)?, &query).map_err(msg)?;
    Ok(found
        .into_iter()
        .map(|(_, m)| mod_key(&m.source, &m.id))
        .collect())
}

/// Pins a mod to `version` (or unpins it with none) and locks the profile,
/// so the new version is fetched and any conflicts show up straight away.
#[tauri::command]
async fn set_version(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
    name: String,
    version: Option<String>,
) -> CmdResult<ProfileDetail> {
    let _work = app.work.start()?;
    let r = profile_ref(&id)?;
    app.pw.set_version(&r, &name, version).map_err(msg)?;
    app.pw.lock(&r, false).await.map_err(lock_msg(&handle))?;
    profile_detail(&app.pw, id)
}

/// A mod whose locked version changed when the lock was updated.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LockChange {
    name: String,
    /// `None` if it wasn't in the lockfile before, e.g. a new dependency.
    from: Option<String>,
    to: Option<String>,
}

/// Moves the mods in `keys` (as `<source>:<id>`) to their newest versions,
/// along with anything those need newer, and returns what changed. This is
/// what confirming `preview_updates` does. Mods that moved to a new version
/// have new details too, so the next `refresh_details` fetches them.
#[tauri::command]
async fn update_mods(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
    keys: Vec<String>,
) -> CmdResult<Vec<LockChange>> {
    let _work = app.work.start()?;
    let r = profile_ref(&id)?;
    let before = app.pw.load_lock(&r).map_err(msg)?;
    let after = app
        .pw
        .lock_updating(&r, &keys)
        .await
        .map_err(lock_msg(&handle))?;
    Ok(lock_changes(&app, before, after))
}

/// What moved between two lockfiles, marking the mods that did as needing
/// their details again.
fn lock_changes(app: &App, before: Option<Lockfile>, after: Lockfile) -> Vec<LockChange> {
    let mut changed = Vec::new();
    let changes = after
        .mods
        .into_iter()
        .filter_map(|m| {
            let key = m.key();
            let old = before.iter().flat_map(|l| &l.mods).find(|l| l.key() == key);
            match old {
                Some(old) if old.version == m.version && old.hash == m.hash => None,
                _ => {
                    changed.push(key);
                    Some(LockChange {
                        name: m.name,
                        from: old.and_then(|o| o.version.clone()),
                        to: m.version,
                    })
                }
            }
        })
        .collect();
    app.details.outdate(&after.game.id, changed);
    changes
}

/// A mod that's been added, and the profile with it.
#[derive(Serialize)]
struct Added {
    name: String,
    profile: ProfileDetail,
}

#[tauri::command]
async fn add_mod(
    app: State<'_, App>,
    id: String,
    source: String,
    mod_id: String,
) -> CmdResult<Added> {
    let _work = app.work.start()?;
    let spec = ModSpec {
        source,
        id: mod_id,
        version: None,
    };
    let added = app
        .pw
        .add_mod(&profile_ref(&id)?, spec, None, true)
        .await
        .map_err(msg)?;
    Ok(Added {
        name: added.name,
        profile: profile_detail(&app.pw, id)?,
    })
}

/// A search from a source's tab.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Search {
    game: String,
    source: String,
    text: String,
    sort: Option<SortBy>,
    /// Category ids; mods must be in at least one of them.
    categories: Vec<String>,
    /// Category ids; mods in any of them are left out.
    excluded_categories: Vec<String>,
    page: u32,
}

#[tauri::command]
async fn search(app: State<'_, App>, query: Search) -> CmdResult<SearchPage> {
    let Search {
        game,
        source,
        text,
        sort,
        categories,
        excluded_categories,
        page,
    } = query;
    let query = SearchQuery {
        text: Some(text),
        sort,
        categories,
        excluded_categories,
        page,
    };
    app.pw.search(&game, &source, &query).await.map_err(msg)
}

/// The categories `source`'s searches for `game` can be narrowed by.
#[tauri::command]
async fn search_categories(
    app: State<'_, App>,
    game: String,
    source: String,
) -> CmdResult<Vec<Category>> {
    app.pw.search_categories(&game, &source).await.map_err(msg)
}

/// With `verify`, every deployed file is read to check it hasn't changed,
/// like `patchwork deploy --verify`.
#[tauri::command]
async fn plan_deploy(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
    verify: Option<bool>,
) -> CmdResult<PlanSummary> {
    let _work = app.work.start()?;
    let check = if verify == Some(true) {
        Check::Full
    } else {
        Check::Quick
    };
    let plan = app
        .pw
        .plan_deploy_checked(&profile_ref(&id)?, check)
        .await
        .map_err(lock_msg(&handle))?;
    Ok(hold_plan(&app, plan))
}

/// Works out how to take everything patchwork deployed back out of `game`,
/// like `patchwork undeploy`, to be confirmed with `apply_plan` as well.
#[tauri::command]
fn plan_undeploy(app: State<'_, App>, game: String) -> CmdResult<PlanSummary> {
    let plan = app
        .pw
        .plan_undeploy(&DeployTarget::Game(game))
        .map_err(msg)?;
    Ok(hold_plan(&app, plan))
}

/// Keeps `plan` for `apply_plan` and summarises it for the UI.
fn hold_plan(app: &App, plan: DeployPlan) -> PlanSummary {
    let summary = PlanSummary {
        ops: plan.ops.iter().cloned().map(UiOp::from).collect(),
        unchanged: plan.unchanged,
        warnings: plan.warnings.clone(),
    };
    *app.plan.lock().unwrap() = Some(plan);
    summary
}

#[tauri::command]
fn apply_plan(app: State<'_, App>) -> CmdResult<usize> {
    let _work = app.work.start()?;
    let plan = app
        .plan
        .lock()
        .unwrap()
        .take()
        .ok_or("there's no deploy waiting to be applied")?;
    let state = app.pw.apply(&plan).map_err(|e| {
        format!("deploy failed partway; what was done is recorded and can be undeployed: {e}")
    })?;
    Ok(state.files.len())
}

/// Starts `game` the way `option` says (by id), or the default way, like
/// `patchwork launch --no-deploy`; deploying first is up to the UI. Says
/// how it was started, with a warning if Steam is needed and isn't running.
#[tauri::command]
async fn launch_game(
    app: State<'_, App>,
    game: String,
    option: Option<String>,
) -> CmdResult<String> {
    let _work = app.work.start()?;
    let launch = app.pw.plan_launch(&game, option.as_deref()).map_err(msg)?;
    let name = app
        .pw
        .launch_options(&game)
        .map_err(msg)?
        .into_iter()
        .flatten()
        .find(|o| o.id == launch.option)
        .map_or(launch.option.clone(), |o| o.name);
    let log = app.pw.paths().launch_log(&game);
    // it watches the game start for a few seconds
    let (launch, started) = tauri::async_runtime::spawn_blocking(move || {
        let started = launch.spawn(&log);
        (launch, started)
    })
    .await
    .map_err(msg)?;
    let started = started.map_err(msg)?;
    let how = match launch.via {
        Via::Steam { .. } => " through Steam",
        Via::Direct => "",
        Via::Proton { .. } => " with Proton",
        Via::Wine { .. } => " with Wine",
    };
    let mut said = format!("Started {name}{how}");
    if !started.added_overrides.is_empty() {
        said.push_str(&format!(
            ", after telling Wine to load its own {}",
            started.added_overrides.join(", ")
        ));
    }
    if launch.needs_steam() && launch::steam_running() == Some(false) {
        said.push_str("; Steam doesn't look to be running, and the game may need it");
    }
    Ok(said)
}

// Downloads the user has to start themselves (see `patchwork://pending`).

/// Locks the profile, e.g. once the user has seen to its pending downloads.
#[tauri::command]
async fn lock_profile(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
) -> CmdResult<ProfileDetail> {
    let _work = app.work.start()?;
    let r = profile_ref(&id)?;
    app.pw.lock(&r, false).await.map_err(lock_msg(&handle))?;
    profile_detail(&app.pw, id)
}

/// Uses a file the user downloaded themselves for a pending download.
#[tauri::command]
fn provide_download(app: State<'_, App>, file: SourceFile, path: String) -> CmdResult<()> {
    let _work = app.work.start()?;
    app.pw
        .provide(&file, std::path::Path::new(&path))
        .map_err(msg)?;
    Ok(())
}

/// Makes patchwork the handler for `nxm://` links, if it isn't already, so
/// Nexus's "Mod Manager Download" buttons hand their downloads to it. On
/// macOS that comes with installing the app instead.
#[tauri::command]
fn handle_links(handle: AppHandle) -> CmdResult<()> {
    #[cfg(any(windows, target_os = "linux"))]
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        let links = handle.deep_link();
        if !links.is_registered("nxm").unwrap_or(false) {
            links.register_all().map_err(msg)?;
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    let _ = handle;
    Ok(())
}

/// How a link from the browser went, sent to the window as
/// `patchwork://link`.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum LinkEvent {
    Downloading,
    Downloaded { file: SourceFile },
    Failed { message: String },
}

/// Downloads what each link from the browser is for, like Nexus's
/// `nxm://` ones, in the background.
fn accept_links(handle: &AppHandle, links: Vec<tauri::Url>) {
    for link in links {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            let app = handle.state::<App>();
            let _work = match app.work.start() {
                Ok(work) => work,
                Err(message) => {
                    let _ = handle.emit("patchwork://link", LinkEvent::Failed { message });
                    return;
                }
            };
            let _ = handle.emit("patchwork://link", LinkEvent::Downloading);
            let event = match app.pw.accept_link(link.as_str()).await {
                Ok(Some(file)) => LinkEvent::Downloaded { file },
                Ok(None) => LinkEvent::Failed {
                    message: format!("patchwork doesn't know what {link} is for"),
                },
                Err(e) => LinkEvent::Failed {
                    message: e.to_string(),
                },
            };
            let _ = handle.emit("patchwork://link", event);
        });
    }
}

/// The built-in look. It always loads first, straight from the binary, so
/// changes to it reach everyone; users' own CSS goes in files loaded after it.
const BUILTIN_CSS: &str = include_str!("default.css");

/// Written out as `<config>/themes/user.css` when that file doesn't exist,
/// so users can see what there is to change.
const USER_CSS: &str = include_str!("user.css");

#[derive(Serialize)]
struct Theme {
    builtin: &'static str,
    /// `<config>/themes/user.css`, applied on top of `builtin` for every game.
    user: Option<String>,
    /// `<config>/themes/<game>.css`, applied on top of `user`.
    game: Option<String>,
}

#[tauri::command]
fn theme(app: State<'_, App>, game: Option<String>) -> Theme {
    let dir = app.pw.paths().config.join("themes");
    if let Err(e) = write_theme_files(&dir) {
        eprintln!("couldn't write the theme files in {}: {e}", dir.display());
    }
    let read = |name: &str| match std::fs::read_to_string(dir.join(name)) {
        Ok(css) => Some(css),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            eprintln!("couldn't read {}: {e}", dir.join(name).display());
            None
        }
    };
    Theme {
        builtin: BUILTIN_CSS,
        user: read("user.css"),
        game: game.and_then(|g| read(&format!("{g}.css"))),
    }
}

/// Writes `user.css` if it's missing, and keeps `builtin.css` (never loaded,
/// just there to read for the ids and classes to target) up to date.
fn write_theme_files(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let user = dir.join("user.css");
    if !user.exists() {
        std::fs::write(user, USER_CSS)?;
    }
    let builtin = dir.join("builtin.css");
    let current = format!("{BUILTIN_HEADER}{BUILTIN_CSS}");
    if std::fs::read_to_string(&builtin).ok().as_deref() != Some(&current) {
        std::fs::write(builtin, current)?;
    }
    Ok(())
}

const BUILTIN_HEADER: &str = "\
/*
 * A copy of Patchwork's built-in look, for reference. It isn't loaded, and
 * it's replaced whenever Patchwork updates, so edits here are lost; put
 * changes in user.css (or <game>.css) instead.
 */

";

/// Opens patchwork with every source, deployer and format there is.
fn open(paths: Paths, reporter: impl Reporter + 'static) -> patchwork_core::Result<Patchwork> {
    Patchwork::builder(paths)
        .source(LocalSource)
        .source(Thunderstore)
        .source(Nexus::default())
        .source(UrlSource)
        .deployer(RulesDeployer)
        .deployer(BethesdaDeployer)
        .deployer(SmfDeployer)
        .format(Archive)
        .format(UrlList)
        .format(R2modman)
        .format(ThunderstoreCode::default())
        .reporter(reporter)
        .open()
}

fn main() {
    if std::env::args().any(|a| a == daemon::FLAG) {
        if let Err(e) = daemon::run(|paths| open(paths, Silent)) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    tauri::Builder::default()
        // first, so a second launch (say, for an `nxm://` link) hands its
        // link to this one and exits before doing anything else
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .setup(|app| {
            let paths = Paths::from_env()?;
            // the daemon has patchwork open while it runs, so it steps aside
            // until the app's closed
            let paused = daemon::pause_for_app(&paths);
            let pw = open(paths, Emit(app.handle().clone()))?;
            app.manage(App {
                pw,
                plan: Mutex::new(None),
                work: Default::default(),
                details: details::Details::default(),
                sync: Default::default(),
                daemon: daemon::Daemon::new(paused),
            });
            sync::start_if_synced(app.handle());

            use tauri_plugin_deep_link::DeepLinkExt;
            let handle = app.handle().clone();
            app.deep_link()
                .on_open_url(move |event| accept_links(&handle, event.urls()));
            // patchwork may have been started by a link, before it was
            // listening for them
            if let Some(links) = app.deep_link().get_current()? {
                accept_links(app.handle(), links);
            }
            // An AppImage has no installer to tell the system it handles
            // `nxm://` links, so it does that itself each time it starts (it
            // may have moved). Windows' installer does it too, but only for
            // the user who installed it.
            use tauri::utils::config::BundleType;
            if let Some(BundleType::AppImage | BundleType::Nsis) = selfupdate::installed_from()
                && let Err(e) = app.deep_link().register_all()
            {
                eprintln!("couldn't register for nxm:// links: {e}");
            }
            cli::refresh_copy(&app.state::<App>());
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            overview,
            profile,
            create_game,
            create_profile,
            use_profile,
            remove_profile,
            rename_profile,
            duplicate_profile,
            profile_formats,
            export_profile,
            import_profile,
            set_game_path,
            find_installs,
            game_settings,
            game_env,
            set_game_settings,
            source_settings,
            set_source_setting,
            check_source_settings,
            set_enabled,
            remove_mod,
            revert_profile,
            profile_history,
            rollback_profile,
            set_hidden_tags,
            sort_mods,
            reorder_mods,
            load_order,
            set_load_order,
            installer,
            set_installer,
            conflicts,
            refresh_details,
            find_games,
            find_mods,
            set_version,
            changelog::preview_updates,
            changelog::mod_changelog,
            update_mods,
            add_mod,
            search,
            search_categories,
            plan_deploy,
            plan_undeploy,
            apply_plan,
            launch_game,
            lock_profile,
            provide_download,
            handle_links,
            theme,
            selfupdate::check_for_update,
            selfupdate::install_update,
            selfupdate::cancel_update,
            cli::cli_status,
            daemon::daemon_status,
            daemon::daemon_missed,
            daemon::daemon_install,
            daemon::daemon_restart,
            daemon::daemon_uninstall,
            cli::install_cli,
            sync::sync_status,
            sync::sync_share,
            sync::sync_join,
            sync::sync_leave,
            sync::sync_linked,
            sync::sync_link,
            sync::sync_unlink,
        ])
        .run(tauri::generate_context!())
        .expect("error while running patchwork");
}
