// Don't open a console window alongside the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod blockers;
mod changelog;
mod cli;
mod daemon;
mod details;
mod links;
mod selfupdate;
mod setup;
mod sync;
mod theme_files;
mod theme_options;
mod theme_watch;
mod themes;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Display;
use std::sync::Mutex;

use base64::prelude::*;
use patchwork_core::action::{ActionInfo, ActionReport};
use patchwork_core::cancel::Cancel;
use patchwork_core::compat::GameVersions;
use patchwork_core::conflicts::ModConflicts;
use patchwork_core::deploy::{Check, DeployOp, DeployPlan, DeployTarget, Dest, RootOrigin};
use patchwork_core::edits::{self, FieldInfo, FieldValue};
use patchwork_core::environment::GameEnv;
use patchwork_core::event::{Event, Reporter, Silent};
use patchwork_core::fix::SetsThen;
use patchwork_core::follow::Follows;
use patchwork_core::game::{DeployerSpec, GameDef, LinkMethod};
use patchwork_core::issue::Issue;
use patchwork_core::launch::{self, LaunchOption, Via};
use patchwork_core::list::{List, ListEntry};
use patchwork_core::locate::Locator;
use patchwork_core::lock::{DependencyInfo, Lockfile, SoftDependencyInfo, mod_key};
use patchwork_core::matching::{self, ModText};
use patchwork_core::options::Answer;
use patchwork_core::order::{self, SortKey};
use patchwork_core::paths::Paths;
use patchwork_core::profile::{Folder, ProfileMod, ProfileRef};
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::set::{FolderSource, OtherFolder};
use patchwork_core::settings::{Origin, SettingState};
use patchwork_core::source::{FileChoice, ModSpec, SourceFile};
use patchwork_core::{
    ConfigFileInfo, ConfigView, DeployStatus, GameInstall, HistoryEntry, ImportOptions, ListInfo,
    ModOptionsInfo, ModOptionsView, Patchwork, SaveTo,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use crate::blockers::CmdError;

/// Commands fail with a message for the UI to show, or with something in
/// the way that the user can clear; see blockers.rs.
type CmdResult<T> = Result<T, CmdError>;

/// `e` for the window: its message, and its blocker when it's a core error
/// that has one (see blockers.rs).
fn msg(e: impl Display + 'static) -> CmdError {
    CmdError::from_display(&e)
}

/// [`msg`] for commands that lock a profile. When locking is waiting on
/// mods the user has to download themselves, the window is sent them as
/// `patchwork://pending` to work through.
fn lock_msg(handle: &AppHandle) -> impl Fn(patchwork_core::Error) -> CmdError + '_ {
    move |e| match e {
        patchwork_core::Error::NeedsDownload(pending) => {
            let message = match pending.len() {
                1 => "1 mod has to be downloaded from its page first".to_owned(),
                n => format!("{n} mods have to be downloaded from their pages first"),
            };
            let _ = handle.emit("patchwork://pending", pending);
            message.into()
        }
        e => CmdError::from_core(&e),
    }
}

struct App {
    pw: Patchwork,
    /// The plan from the last `plan_undeploy`, waiting for the user to
    /// confirm it with `apply_plan`.
    plan: Mutex<Option<DeployPlan>>,
    /// Calls off the deploy or undeploy that's running, if there is one;
    /// see `cancel_job`.
    job: Mutex<Option<Cancel>>,
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
            Event::Downloading {
                name,
                downloaded,
                total,
            } => UiEvent::Downloading {
                name,
                downloaded,
                total,
            },
            Event::Downloaded { name } => UiEvent::Downloaded { name },
            Event::Building { step } => UiEvent::Building { step },
            Event::Applying(op) => UiEvent::Applying(op.into()),
            Event::Warning(message) => UiEvent::Warning { message },
            Event::Notice(message) => UiEvent::Notice { message },
        };
        // Nothing useful to do if the window's gone.
        let _ = self.0.emit("patchwork://event", event);
    }
}

#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum UiEvent {
    Resolving {
        name: String,
    },
    Fetching {
        name: String,
    },
    Downloading {
        name: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Downloaded {
        name: String,
    },
    Building {
        step: String,
    },
    Applying(UiOp),
    Warning {
        message: String,
    },
    Notice {
        message: String,
    },
}

#[derive(Clone, Serialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
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
    /// The profile last used for each game, by game id.
    last_profiles: BTreeMap<String, String>,
    /// Games pinned to the top of the game picker, by id; see `gui.toml`.
    favorites: BTreeSet<String>,
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
    /// Whether `install` is set but isn't a folder any more, as when the
    /// game's been uninstalled or is on a drive that isn't plugged in.
    missing: bool,
    /// Where the definition came from, e.g. `builtin` or `builtin + user`.
    origin: String,
    /// Something an `<img>` can load: the definition's URL, or a local
    /// image file inlined as a data URL, since the webview can't read paths.
    image: Option<String>,
    /// The sources mods for it can be searched on, for the Add mods tab,
    /// which works without a profile.
    search_sources: Vec<String>,
    /// The orders each of `search_sources` can sort its results in.
    search_sorts: SearchSorts,
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
    /// Every locked mod and replaced dependency, with what needs it, for
    /// the Mods tab's Dependencies section. Empty until it's locked.
    dependencies: Vec<UiDependency>,
    /// Mods with any of these tags go in the Mods tab's hidden section.
    hidden_tags: Vec<String>,
    /// Groups of mods, the profile's own then the linked folders of the
    /// shared sets it includes; each mod's `folder` says which it's in.
    folders: Vec<UiFolder>,
    /// The game's ordered lists (see [`patchwork_core::list`]), each on a
    /// tab of its own.
    lists: Vec<ListInfo>,
    /// Whether the game has config files, for the Config tab.
    has_config: bool,
    /// What extensions offer to run for the game (see
    /// [`patchwork_core::action`]); those about a list are on its tab.
    actions: Vec<ActionInfo>,
    /// Everything worth warning about, from [`Patchwork::issues`].
    issues: Vec<Issue>,
    /// The names of mods added here to a profile this computer follows,
    /// which whoever shares it doesn't have.
    unshared: Vec<String>,
    /// Whether it's synced with other computers or people, so its mods
    /// can be kept personal (see [`patchwork_core::personal`]).
    synced: bool,
    /// Where the profile is deployed to, if that's been set up: the game's
    /// folder, or its instance (which always is).
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
    /// The Steam beta branch the game is on, like `experimental`, if it's
    /// on one; shown beside the profile's name.
    steam_branch: Option<String>,
    /// The ways the game can be started, for the Play button; none until
    /// it's known where it's installed.
    launch_options: Vec<LaunchOption>,
    /// What it picks about the game it runs, where it's deployed to, and
    /// so on, for the profile settings dialog.
    setup: setup::ProfileSetup,
}

/// A folder of the profile's mods, as the Mods tab shows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiFolder {
    #[serde(flatten)]
    folder: Folder,
    /// Whether it's a shared set's linked folder: what's in it is the
    /// set's, so changing that changes every profile that includes it.
    linked: bool,
    /// For a linked folder, the other profiles that include its set, by
    /// name (they're all the same game's).
    also_in: Vec<String>,
    /// Whether it's the profile's personal layer (see
    /// [`patchwork_core::personal`]): a linked folder whose mods stay on
    /// this person's computers.
    personal: bool,
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
    /// Which versions of the game its source says the locked version works
    /// with, if it says; the profile's `game_versions` overrides it.
    source_game_versions: Option<GameVersions>,
    /// Whether the mod's source has versions to pick from.
    versioned: bool,
    /// Its versions as last fetched, newest first, if they ever have been.
    /// `refresh_details` fetches them again once they're stale.
    versions: Option<Vec<String>>,
    /// What a search would show for it (picture, summary, ...), as last
    /// fetched. The same goes for it as for `versions`.
    summary: Option<ModSummary>,
    /// The newer version it could move to, going by `versions`; see
    /// [`Patchwork::known_update`]. The UI's update notes and Update button
    /// go by this rather than working it out themselves.
    update: Option<String>,
    /// The choices to make about it, like a FOMOD installer's options.
    /// Only known once it's been downloaded.
    options: Vec<ModOptionsInfo>,
    /// Its place in the load order, which differs from its place in the
    /// profile once mods have priorities; see [`order::load_order`].
    load_index: usize,
    /// What it lists that it may not need, and whether the profile has
    /// each; empty until it's locked.
    soft_dependencies: Vec<UiSoftDependency>,
    /// Whether the user follows it, to be told about its new versions (see
    /// [`patchwork_core::follow`]).
    followed: bool,
}

/// A soft dependency of a profile's mod; see [`Lockfile::soft_dependencies`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiSoftDependency {
    #[serde(flatten)]
    info: SoftDependencyInfo,
    /// What a search would show for it, as last fetched, for ones the
    /// source doesn't name.
    summary: Option<ModSummary>,
}

/// A locked mod or a replaced dependency; see [`Lockfile::dependency_info`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiDependency {
    #[serde(flatten)]
    info: DependencyInfo,
    /// What a search would show for it, as last fetched.
    summary: Option<ModSummary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PlanSummary {
    ops: Vec<UiOp>,
    unchanged: usize,
    warnings: Vec<String>,
}

impl PlanSummary {
    fn of(plan: &DeployPlan) -> Self {
        PlanSummary {
            ops: plan.ops.iter().cloned().map(UiOp::from).collect(),
            unchanged: plan.unchanged,
            warnings: plan.warnings.clone(),
        }
    }
}

/// The orders each search source can sort its results in, by source id.
type SearchSorts = BTreeMap<String, Vec<SortBy>>;

/// The sources `game`'s mods can be searched on, and the orders each sorts by.
fn search_sources(pw: &Patchwork, game: &str) -> CmdResult<(Vec<String>, SearchSorts)> {
    let sources: Vec<String> = pw
        .search_sources(game)
        .map_err(msg)?
        .into_iter()
        .map(str::to_owned)
        .collect();
    let sorts = sources
        .iter()
        .map(|s| Ok((s.clone(), pw.search_sorts(s).map_err(msg)?.to_vec())))
        .collect::<CmdResult<_>>()?;
    Ok((sources, sorts))
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
            let (search_sources, search_sorts) = search_sources(pw, &game.id)?;
            let install = pw.game_install(&game.id).map_err(msg)?.map(|i| i.path);
            Ok(GameInfo {
                id: game.id.clone(),
                name: game.name.clone(),
                missing: install.as_ref().is_some_and(|p| !p.is_dir()),
                install: install.map(|p| p.display().to_string()),
                origin: entry.origins(),
                image: match &game.image {
                    Some(image) => image_src(image),
                    None => game.detect.steam.first().map(|&app| steam_capsule(app)),
                },
                search_sources,
                search_sorts,
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
    let favorites = themes::GuiPrefs::load(&pw.paths().config)
        .map(|p| p.favorites)
        .unwrap_or_else(|e| {
            warnings.push(e);
            BTreeSet::new()
        });
    Ok(Overview {
        games,
        profiles,
        current: pw.current_profile().map_err(msg)?.map(|r| r.to_string()),
        last_profiles: pw
            .last_profiles()
            .map_err(msg)?
            .into_iter()
            .map(|(game, p)| (game, p.to_string()))
            .collect(),
        favorites,
        rulesets: registry.data_ids("rulesets").map(str::to_owned).collect(),
        warnings,
    })
}

/// Steam's portrait library art for an app, used for games with no `image`.
/// If Steam has none, the image fails to load and the card shows initials.
fn steam_capsule(app: u32) -> String {
    format!(
        "https://shared.akamai.steamstatic.com/store_item_assets/steam/apps/{app}/library_600x900.jpg"
    )
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

/// `summary`, as [`Patchwork::mod_summary`] gave it for `m`, made ready to
/// show: a picture the user picked from this computer is read in, since the
/// window can't load files itself.
pub(crate) fn shown_summary(m: &ProfileMod, summary: Option<ModSummary>) -> Option<ModSummary> {
    let mut summary = summary?;
    let picked = edits::mod_field("picture").and_then(|f| m.edits.get(f));
    if picked.is_some() {
        summary.icon_url = summary.icon_url.as_deref().and_then(image_src);
    }
    Some(summary)
}

/// Whether the system's color picker can be used. On Linux the webview's
/// is GTK's color chooser, which aborts the whole app when GTK's settings
/// schema for it isn't installed (as on NixOS without a wrapper), so this
/// looks for the schema where GLib does; the page uses its own picker
/// without it.
/// Shows the window, which starts hidden so that it doesn't show blank and
/// unstyled while the page loads. The page calls this once its first screen
/// is drawn; see app.js.
#[tauri::command]
fn ready(window: tauri::WebviewWindow) {
    let _ = window.show();
}

/// How long to wait for the page to say it's [`ready`] before showing the
/// window anyway, so a page that fails to start still shows something.
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

#[tauri::command]
fn native_color_picker() -> bool {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        const SCHEMA: &[u8] = b"org.gtk.Settings.ColorChooser";
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        let mut dirs: Vec<std::path::PathBuf> = var("GSETTINGS_SCHEMA_DIR")
            .map(|v| std::env::split_paths(&v).collect())
            .unwrap_or_default();
        let data_home = var("XDG_DATA_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| var("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")));
        let data_dirs =
            var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
        dirs.extend(
            data_home
                .into_iter()
                .chain(std::env::split_paths(&data_dirs))
                .map(|d| d.join("glib-2.0/schemas")),
        );
        dirs.iter().any(|d| {
            std::fs::read(d.join("gschemas.compiled"))
                .is_ok_and(|bytes| bytes.windows(SCHEMA.len()).any(|w| w == SCHEMA))
        })
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    true
}

/// Everything about mod `name` that can be changed (see
/// [`patchwork_core::edits`]), for its Edit dialog.
#[tauri::command]
fn mod_fields(app: State<'_, App>, id: String, name: String) -> CmdResult<Vec<FieldInfo>> {
    app.pw.mod_fields(&profile_ref(&id)?, &name).map_err(msg)
}

/// What a game's Edit dialog shows: everything about it that can be
/// changed (see [`patchwork_core::edits::game`]).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GameFields {
    fields: Vec<FieldInfo>,
    /// Whether the user added the game themselves, so there's no definition
    /// for its fields to go back to.
    added: bool,
}

#[tauri::command]
fn game_fields(app: State<'_, App>, game: String) -> CmdResult<GameFields> {
    let mut fields = app.pw.game_fields(&game).map_err(msg)?;
    let registry = app.pw.registry();
    let added = registry.game_before_user(&game).is_none();
    // what the game picker shows when the definition has no picture
    let steam = registry
        .game_before_user(&game)
        .and_then(|_| registry.game(&game).ok())
        .and_then(|e| e.value.detect.steam.first().copied());
    if let Some(app_id) = steam
        && let Some(picture) = fields
            .iter_mut()
            .find(|f| f.kind == patchwork_core::edits::FieldKind::Picture && f.source.is_none())
    {
        picture.source = Some(FieldValue::Text(steam_capsule(app_id)));
    }
    Ok(GameFields { fields, added })
}

/// Changes fields of game `game` by id, in the user's own file for it;
/// `null` goes back to what its definition says.
#[tauri::command]
fn set_game_edits(
    app: State<'_, App>,
    game: String,
    changes: BTreeMap<String, Option<FieldValue>>,
) -> CmdResult<()> {
    app.pw.set_game_edits(&game, changes).map_err(msg)
}

/// A picture on this computer, as something the window can show, for the
/// Edit dialog to preview one before it's saved. `None` if it can't be read
/// or isn't a picture.
#[tauri::command]
fn picture_preview(path: String) -> Option<String> {
    image_src(&path)
}

/// Changes fields of mod `name` by id; `null` goes back to what its source
/// says, and fields left out stay as they are.
#[tauri::command]
fn set_mod_edits(
    app: State<'_, App>,
    id: String,
    name: String,
    changes: BTreeMap<String, Option<FieldValue>>,
) -> CmdResult<ProfileDetail> {
    app.pw
        .set_mod_edits(&profile_ref(&id)?, &name, changes)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
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
    let target = pw.deploy_target(&loaded).map_err(msg)?;
    let install = pw
        .target_root(&target)
        .map_err(msg)?
        .map(|p| p.display().to_string());
    let state = pw.deploy_state(&target).map_err(msg)?;
    let deployed_files =
        (state.profile.as_deref() == Some(id.as_str())).then_some(state.files.len());
    let game_files = state.files.len();
    let deploy_status = pw.deploy_status(&r).map_err(msg)?;
    let has_changes = pw.has_changes(&r).map_err(msg)?;
    let steam_branch = pw
        .game_env(&target)
        .map_err(msg)?
        .and_then(|env| env.steam_branch);
    let lock = pw.load_lock(&r).map_err(msg)?;
    let dependencies = lock
        .iter()
        .flat_map(|l| l.dependency_info(&loaded))
        .map(|info| UiDependency {
            summary: pw
                .known_summary(&loaded.game, &info.source, &info.id)
                .and_then(|k| k.summary),
            info,
        })
        .collect();
    let issues = pw.issues(&r).map_err(msg)?;
    let syncing = patchwork_sync::synced_profiles(&pw.paths().sync())
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.profile == r.uuid);
    let synced = syncing.is_some();
    let following = syncing.is_some_and(|s| !s.writable);
    let unshared = if following {
        pw.unshared_mods(&r).map_err(msg)?
    } else {
        Vec::new()
    };
    let lists = pw.lists(&r).map_err(msg)?;
    let has_config = pw.has_config_files(&r).map_err(msg)?;
    let actions = pw.actions(&r).map_err(msg)?;
    let mut options = pw.mods_with_options(&r).map_err(msg)?;
    let launch_options = pw
        .profile_launch_options(&loaded)
        .map_err(msg)?
        .unwrap_or_default();
    let setup = setup::profile_setup(pw, &loaded)?;
    let label = loaded.label();
    let lookup = pw.lookup(&loaded).map_err(msg)?;
    let name = loaded.name.clone();
    let hidden_tags = std::mem::take(&mut loaded.hidden_tags);
    let mut includers = if loaded.includes.is_empty() {
        HashMap::new()
    } else {
        pw.includers().map_err(msg)?
    };
    let folders = loaded
        .all_folders()
        .map(|f| {
            let linked = loaded.include(f.id).is_some();
            let also_in = includers
                .remove(&f.id)
                .unwrap_or_default()
                .into_iter()
                .filter(|(p, _)| *p != r)
                .map(|(_, label)| match label.split_once('/') {
                    Some((_, name)) => name.to_owned(),
                    None => label,
                })
                .collect();
            UiFolder {
                folder: f.clone(),
                linked,
                also_in,
                personal: f.id == loaded.id,
            }
        })
        .collect();
    let load_index: HashMap<String, usize> = order::load_order(&loaded.mods)
        .into_iter()
        .enumerate()
        .map(|(i, m)| (m.name.clone(), i))
        .collect();
    let mut soft_dependencies: HashMap<String, Vec<SoftDependencyInfo>> = loaded
        .mods
        .iter()
        .filter_map(|m| {
            let key = mod_key(&m.source, &m.id);
            let soft = lock.as_ref()?.soft_dependencies(&loaded, &key);
            Some((key, soft))
        })
        .collect();
    // only marks, so a list that can't be read is as good as none
    let follows = Follows::load(pw.paths()).unwrap_or_default();
    let mods = loaded
        .mods
        .into_iter()
        .map(|m| {
            let key = mod_key(&m.source, &m.id);
            let versioned = pw.lists_versions(&m.source).unwrap_or(false);
            let known = versioned
                .then(|| pw.known_versions(&lookup, &m.source, &m.id))
                .flatten();
            let summary = shown_summary(&m, pw.mod_summary(&loaded.game, &m));
            let locked = lock.iter().flat_map(|l| &l.mods).find(|l| l.has_key(&key));
            let update = versioned
                .then(|| pw.known_update(&lookup, &m, lock.as_ref()))
                .flatten();
            UiMod {
                update,
                followed: follows.mods.contains(&key),
                shown_id: pw.display_id(lock.as_ref(), &m),
                versions: known.map(|k| k.versions),
                summary,
                versioned,
                locked: locked.and_then(|l| l.version.clone()),
                source_game_versions: locked.and_then(|l| l.game_versions.clone()),
                soft_dependencies: soft_dependencies
                    .remove(&key)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|info| UiSoftDependency {
                        summary: info
                            .dependency
                            .key
                            .as_deref()
                            .and_then(|k| k.split_once(':'))
                            .and_then(|(source, id)| pw.known_summary(&loaded.game, source, id))
                            .and_then(|k| k.summary),
                        info,
                    })
                    .collect(),
                key,
                options: options.remove(&m.name).unwrap_or_default(),
                load_index: load_index[&m.name],
                profile: m,
            }
        })
        .collect();
    Ok(ProfileDetail {
        label,
        name,
        game: loaded.game,
        mods,
        dependencies,
        hidden_tags,
        folders,
        lists,
        has_config,
        actions,
        issues,
        unshared,
        synced,
        id,
        install,
        deployed_files,
        game_files,
        deploy_status,
        has_changes,
        steam_branch,
        launch_options,
        setup,
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
        accent: None,
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
        capture: None,
        instances: None,
        config: Vec::new(),
        ext: Default::default(),
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

/// Looks for every game on this machine, setting up the ones that aren't
/// and moving the ones whose folder has gone; see
/// [`Patchwork::detect_games`]. Async so the scan runs off the main thread.
#[tauri::command]
async fn detect_games(app: State<'_, App>) -> CmdResult<patchwork_core::Detected> {
    let locator = tauri::async_runtime::spawn_blocking(Locator::scan)
        .await
        .map_err(msg)?;
    Ok(app.pw.detect_games(&locator))
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
    /// The ways the game can be started. Empty until it's been set up.
    launch_options: Vec<LaunchOption>,
    /// What can run the game. `None` if patchwork starts it itself, or until
    /// it's been set up.
    runners: Option<launch::RunnerChoices>,
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
        launch_options: app
            .pw
            .launch_options(&game)
            .map_err(msg)?
            .unwrap_or_default(),
        runners: app.pw.runner_choices(&game).map_err(msg)?,
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

/// Remembers the way to start a game by default, as Play's menu does.
#[tauri::command]
fn set_launch_option(app: State<'_, App>, game: String, option: String) -> CmdResult<()> {
    let install = app
        .pw
        .game_install(&game)
        .map_err(msg)?
        .ok_or_else(|| format!("{game} hasn't been set up"))?;
    app.pw
        .set_game_install(
            &game,
            GameInstall {
                launch: Some(option),
                ..install
            },
        )
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

/// Gives a mod a new priority; mods with a higher one load later, so they
/// win when two provide the same file.
#[tauri::command]
fn set_priority(
    app: State<'_, App>,
    id: String,
    name: String,
    priority: i32,
) -> CmdResult<ProfileDetail> {
    app.pw
        .set_priority(&profile_ref(&id)?, &name, priority)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Undoes every change made to the profile since it was last deployed,
/// doing as `sets` says with shared sets changed since (a blocker if
/// there are any and it's left out).
#[tauri::command]
fn revert_profile(
    app: State<'_, App>,
    id: String,
    sets: Option<SetsThen>,
) -> CmdResult<ProfileDetail> {
    app.pw
        .go_back(&profile_ref(&id)?, None, sets)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// The profile's past deploys, newest first.
#[tauri::command]
fn profile_history(app: State<'_, App>, id: String) -> CmdResult<Vec<HistoryEntry>> {
    let mut history = app.pw.history(&profile_ref(&id)?).map_err(msg)?;
    history.reverse();
    Ok(history)
}

/// Puts the profile back how it was at deploy `number` of its history,
/// with `sets` as for [`revert_profile`].
#[tauri::command]
fn rollback_profile(
    app: State<'_, App>,
    id: String,
    number: u32,
    sets: Option<SetsThen>,
) -> CmdResult<ProfileDetail> {
    app.pw
        .go_back(&profile_ref(&id)?, Some(number), sets)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

#[tauri::command]
fn remove_mod(app: State<'_, App>, id: String, name: String) -> CmdResult<()> {
    app.pw.remove_mod(&profile_ref(&id)?, &name).map_err(msg)
}

/// Makes a folder called `name` with `mods` in it, where the first of
/// them is.
#[tauri::command]
fn add_folder(
    app: State<'_, App>,
    id: String,
    name: String,
    mods: Vec<String>,
) -> CmdResult<ProfileDetail> {
    app.pw
        .add_folder(&profile_ref(&id)?, &name, &mods)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

#[tauri::command]
fn rename_folder(
    app: State<'_, App>,
    id: String,
    folder: String,
    name: String,
) -> CmdResult<ProfileDetail> {
    app.pw
        .rename_folder(&profile_ref(&id)?, &folder, &name)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Removes a folder, leaving its mods where they are.
#[tauri::command]
fn remove_folder(app: State<'_, App>, id: String, folder: String) -> CmdResult<ProfileDetail> {
    app.pw
        .remove_folder(&profile_ref(&id)?, &folder)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Puts a mod in a folder (by id), or with none takes it out of its own.
#[tauri::command]
fn set_mod_folder(
    app: State<'_, App>,
    id: String,
    name: String,
    folder: Option<String>,
) -> CmdResult<ProfileDetail> {
    app.pw
        .set_mod_folder(&profile_ref(&id)?, &name, folder.as_deref())
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Moves mods into the profile's personal layer, or with `personal` false
/// back out of it to be synced to everyone.
#[tauri::command]
fn set_personal(
    app: State<'_, App>,
    id: String,
    names: Vec<String>,
    personal: bool,
) -> CmdResult<ProfileDetail> {
    app.pw
        .set_personal(&profile_ref(&id)?, &names, personal)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Turns all of a folder's mods on or off.
#[tauri::command]
fn set_folder_enabled(
    app: State<'_, App>,
    id: String,
    folder: String,
    enabled: bool,
) -> CmdResult<ProfileDetail> {
    app.pw
        .set_folder_enabled(&profile_ref(&id)?, &folder, enabled)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// The folders of the game's other profiles, for using one in this one
/// too.
#[tauri::command]
fn other_folders(app: State<'_, App>, id: String) -> CmdResult<Vec<OtherFolder>> {
    app.pw.other_folders(&profile_ref(&id)?).map_err(msg)
}

/// Uses a folder another profile has in this one too: the same folder
/// with `link`, otherwise a copy of its own.
#[tauri::command]
fn use_folder(
    app: State<'_, App>,
    id: String,
    source: FolderSource,
    link: bool,
) -> CmdResult<ProfileDetail> {
    app.pw
        .use_folder(&profile_ref(&id)?, &source, link)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Includes the shared set `set` in the profile, after its own mods.
#[tauri::command]
fn include_set(app: State<'_, App>, id: String, set: String) -> CmdResult<ProfileDetail> {
    app.pw
        .include_set(&profile_ref(&id)?, blockers::parse_set(&set)?, None)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Copies the shared set `set` into the profile as a folder of its own.
#[tauri::command]
fn copy_set(app: State<'_, App>, id: String, set: String) -> CmdResult<ProfileDetail> {
    app.pw
        .copy_set(&profile_ref(&id)?, blockers::parse_set(&set)?)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Stops the profile including the set `set`, keeping its mods as a
/// folder of the profile's own if `keep`.
#[tauri::command]
fn unlink_set(
    app: State<'_, App>,
    id: String,
    set: String,
    keep: bool,
) -> CmdResult<ProfileDetail> {
    app.pw
        .unlink_set(&profile_ref(&id)?, blockers::parse_set(&set)?, keep)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
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

/// What a list's tab shows.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiList {
    #[serde(flatten)]
    list: List,
    /// Enabled mods that haven't been locked, whose items aren't known
    /// until they are; see [`lock_profile`].
    unlocked: Vec<String>,
}

fn ui_list(pw: &Patchwork, r: &ProfileRef, list: &str) -> CmdResult<UiList> {
    let list = pw.list(r, list).map_err(msg)?;
    let lock = pw.load_lock(r).map_err(msg)?;
    let unlocked = pw
        .load_profile(r)
        .map_err(msg)?
        .mods
        .into_iter()
        .filter(|m| m.enabled)
        .filter(|m| {
            let key = mod_key(&m.source, &m.id);
            !lock.iter().flat_map(|l| &l.mods).any(|l| l.has_key(&key))
        })
        .map(|m| m.name)
        .collect();
    Ok(UiList { list, unlocked })
}

/// One of the profile's ordered lists, for its tab.
#[tauri::command]
fn list(app: State<'_, App>, id: String, list: String) -> CmdResult<UiList> {
    ui_list(&app.pw, &profile_ref(&id)?, &list)
}

/// Saves a list's tab's order and switches, returning the list as it now
/// is.
#[tauri::command]
fn set_list(
    app: State<'_, App>,
    id: String,
    list: String,
    entries: Vec<ListEntry>,
) -> CmdResult<UiList> {
    let r = profile_ref(&id)?;
    app.pw.set_list(&r, &list, entries).map_err(msg)?;
    ui_list(&app.pw, &r, &list)
}

/// Runs one of the profile's actions and saves what it changes. Async, as
/// actions may fetch things (LOOT's masterlist, say).
#[tauri::command]
async fn run_action(app: State<'_, App>, id: String, action: String) -> CmdResult<ActionReport> {
    app.pw
        .run_action(&profile_ref(&id)?, &action, true)
        .await
        .map_err(msg)
}

/// A mod's choices as the options dialog shows them.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiModOptions {
    #[serde(flatten)]
    view: ModOptionsView,
    /// The items' pictures that the dialog didn't say it already has, as
    /// something the webview can show, by their path in the mod.
    pictures: BTreeMap<String, String>,
}

/// `name`'s choices `options` with `answers`, or with what the profile has
/// saved. `have` are the pictures the dialog already has, which aren't sent
/// again. Async so unpacking the mod happens off the main thread.
#[tauri::command]
async fn mod_options(
    app: State<'_, App>,
    id: String,
    name: String,
    options: String,
    answers: Option<Vec<Answer>>,
    have: Vec<String>,
) -> CmdResult<UiModOptions> {
    let mut view = app
        .pw
        .mod_options(
            &profile_ref(&id)?,
            &name,
            Some(&options),
            answers.as_deref(),
        )
        .map_err(msg)?;
    let images = std::mem::take(&mut view.images);
    let pictures = images
        .into_iter()
        .filter(|(rel, _)| !have.contains(rel))
        .filter_map(|(rel, path)| Some((rel, image_src(path.to_str()?)?)))
        .collect();
    Ok(UiModOptions { view, pictures })
}

/// Saves `answers` as what's picked in `name`'s choices `options`, or with
/// `None` goes back to the defaults.
#[tauri::command]
fn set_mod_options(
    app: State<'_, App>,
    id: String,
    name: String,
    options: String,
    answers: Option<Vec<Answer>>,
) -> CmdResult<ProfileDetail> {
    app.pw
        .set_mod_options(
            &profile_ref(&id)?,
            &name,
            Some(&options),
            answers.as_deref(),
        )
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// The config files the profile can change settings in, for the Config
/// tab. Async since finding them means working out what mods deploy.
#[tauri::command]
async fn config_files(app: State<'_, App>, id: String) -> CmdResult<Vec<ConfigFileInfo>> {
    app.pw.config_files(&profile_ref(&id)?).map_err(msg)
}

/// The settings in the config file deployed to `path`, with `answers`
/// changing them, or with what the profile has saved.
#[tauri::command]
async fn config_form(
    app: State<'_, App>,
    id: String,
    path: String,
    answers: Option<Vec<Answer>>,
) -> CmdResult<ConfigView> {
    let path: Dest = path.parse().map_err(msg)?;
    app.pw
        .config_form(&profile_ref(&id)?, &path, answers.as_deref())
        .map_err(msg)
}

/// Saves `answers` as the settings changed in the config file deployed to
/// `path`, or with `None` forgets them, in the profile or the shared set
/// `to` says (by default, the view's `saveTo`).
#[tauri::command]
fn set_config(
    app: State<'_, App>,
    id: String,
    path: String,
    answers: Option<Vec<Answer>>,
    to: Option<SaveTo>,
) -> CmdResult<ProfileDetail> {
    let path: Dest = path.parse().map_err(msg)?;
    app.pw
        .set_config(&profile_ref(&id)?, &path, answers.as_deref(), to)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// One mod's conflicts, for the Mods tab.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiModConflicts {
    name: String,
    /// See [`ModConflicts::lost`].
    lost: bool,
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
            lost: conflicts.lost(),
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

/// Says which versions of the game mod `name` works with, as ranges any of
/// which will do (`1.5.97`, `1.6.*`, `>=1.6.640 <1.7`), in place of what its
/// source says; with `None`, goes back to what the source says.
#[tauri::command]
fn set_game_versions(
    app: State<'_, App>,
    id: String,
    name: String,
    versions: Option<Vec<String>>,
) -> CmdResult<ProfileDetail> {
    let versions = versions
        .map(GameVersions::try_from)
        .transpose()
        .map_err(msg)?;
    app.pw
        .set_game_versions(&profile_ref(&id)?, &name, versions)
        .map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Sets which dependencies (as `source:id`) mod `name` stands in for, and
/// locks the profile so they're swapped straight away.
#[tauri::command]
async fn set_replaces(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
    name: String,
    replaces: Vec<String>,
) -> CmdResult<ProfileDetail> {
    let _work = app.work.start()?;
    let r = profile_ref(&id)?;
    app.pw.set_replaces(&r, &name, &replaces).map_err(msg)?;
    app.pw.lock(&r, false).await.map_err(lock_msg(&handle))?;
    profile_detail(&app.pw, id)
}

/// Stops pointing out `version` as an update to pinned mod `name`, until a
/// newer one is out.
#[tauri::command]
fn dismiss_update(
    app: State<'_, App>,
    id: String,
    name: String,
    version: String,
) -> CmdResult<ProfileDetail> {
    let r = profile_ref(&id)?;
    app.pw.dismiss_update(&r, &name, &version).map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// Sets whether mod `name` ignores its soft dependency `ident`, so it isn't
/// pointed out as missing.
#[tauri::command]
fn set_ignored(
    app: State<'_, App>,
    id: String,
    name: String,
    ident: String,
    ignored: bool,
) -> CmdResult<ProfileDetail> {
    let r = profile_ref(&id)?;
    app.pw
        .set_ignored(&r, &name, &ident, ignored)
        .map_err(msg)?;
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
            let old = before
                .iter()
                .flat_map(|l| &l.mods)
                .find(|l| l.has_key(&key));
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
    version: Option<String>,
) -> CmdResult<Added> {
    let _work = app.work.start()?;
    let spec = ModSpec {
        source,
        id: mod_id,
        version,
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

/// A link the user pasted, as the mod it's for, with the files it could
/// be added as if there's a choice to make.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Linked {
    source: String,
    id: String,
    version: Option<String>,
    choices: Vec<FileChoice>,
}

/// Which source a link pasted for `game` is for, without looking anything
/// up, so the Mods tab's search box can say where Enter adds it from. None
/// when no source recognizes it.
#[tauri::command]
fn link_source(app: State<'_, App>, game: String, link: String) -> Option<String> {
    app.pw.parse_link(&game, &link).ok().map(|spec| spec.source)
}

#[tauri::command]
async fn parse_link(app: State<'_, App>, id: String, link: String) -> CmdResult<Linked> {
    let profile = profile_ref(&id)?;
    let loaded = app.pw.load_profile(&profile).map_err(msg)?;
    let spec = app.pw.parse_link(&loaded.game, &link).map_err(msg)?;
    let choices = app.pw.file_choices(&profile, &spec).await.map_err(msg)?;
    Ok(Linked {
        source: spec.source,
        id: spec.id,
        version: spec.version,
        choices,
    })
}

/// The files mod `mod_id` from `source` could be added to profile `id` as,
/// when there's a choice, like a Nexus mod with a plugin and a preloader.
#[tauri::command]
async fn file_choices(
    app: State<'_, App>,
    id: String,
    source: String,
    mod_id: String,
) -> CmdResult<Vec<FileChoice>> {
    let spec = ModSpec {
        source,
        id: mod_id,
        version: None,
    };
    app.pw
        .file_choices(&profile_ref(&id)?, &spec)
        .await
        .map_err(msg)
}

/// A search of one source, from the Add mods tab.
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

/// Locks and deploys profile `id` in one go, without asking first. With
/// `verify`, every deployed file is read to check it hasn't changed, like `patchwork deploy --verify`. `cancel_job` stops it.
#[tauri::command]
async fn deploy(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
    verify: Option<bool>,
) -> CmdResult<Deployed> {
    let _work = app.work.start()?;
    let job = Job::start(&app);
    let check = if verify == Some(true) {
        Check::Full
    } else {
        Check::Quick
    };
    let plan = app
        .pw
        .plan_deploy_checked(&profile_ref(&id)?, check, &job.0)
        .await;
    release_memory();
    let plan = match plan {
        Ok(plan) => plan,
        Err(patchwork_core::Error::Cancelled) => return Ok(Deployed::Cancelled { partway: false }),
        Err(e) => return Err(lock_msg(&handle)(e)),
    };
    apply_cancellable(handle, plan, job.0.clone()).await
}

/// How a deploy or undeploy went.
#[derive(Serialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum Deployed {
    Done {
        /// How many files were placed or removed.
        changes: usize,
        /// How many files are deployed now.
        files: usize,
        /// The ones that were already in place.
        unchanged: usize,
        warnings: Vec<String>,
    },
    /// Called off by `cancel_job`, `partway` through changing files in the
    /// game, or before anything changed.
    Cancelled { partway: bool },
}

/// The deploy or undeploy that's running, while it's kept.
struct Job<'a>(Cancel, &'a App);

impl<'a> Job<'a> {
    fn start(app: &'a App) -> Self {
        let cancel = Cancel::default();
        *app.job.lock().unwrap() = Some(cancel.clone());
        Job(cancel, app)
    }
}

impl Drop for Job<'_> {
    fn drop(&mut self) {
        let mut job = self.1.job.lock().unwrap();
        if job.as_ref().is_some_and(|c| c.same(&self.0)) {
            *job = None;
        }
    }
}

/// Stops the deploy or undeploy that's running, if there is one, before
/// the next file. One that's still working out what to deploy hasn't
/// changed anything yet.
#[tauri::command]
fn cancel_job(app: State<'_, App>) {
    if let Some(cancel) = &*app.job.lock().unwrap() {
        cancel.cancel();
    }
}

/// Applies `plan` off the async runtime's threads, stopping before the
/// next file once `cancel` is cancelled.
async fn apply_cancellable(
    handle: AppHandle,
    plan: DeployPlan,
    cancel: Cancel,
) -> CmdResult<Deployed> {
    tauri::async_runtime::spawn_blocking(move || {
        let app = handle.state::<App>();
        let (changes, unchanged) = (plan.ops.len(), plan.unchanged);
        let warnings = plan.warnings.clone();
        let result = app.pw.apply_cancellable(&plan, &cancel);
        drop(plan);
        release_memory();
        match result {
            Ok(state) => Ok(Deployed::Done {
                changes,
                files: state.files.len(),
                unchanged,
                warnings,
            }),
            Err(patchwork_core::Error::Cancelled) => Ok(Deployed::Cancelled { partway: true }),
            Err(e) => Err(format!(
                "deploy failed partway; what was done is recorded and can be undeployed: {e}"
            )
            .into()),
        }
    })
    .await
    .map_err(msg)?
}

/// Hands memory freed since the last call back to the OS. Building a deploy
/// can take gigabytes (converting Hitman's entities, say), and glibc keeps
/// what's freed in its per-thread arenas instead, so the GUI would hold on
/// to all of it until it quits.
fn release_memory() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // SAFETY: malloc_trim only releases free memory; it has no preconditions.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Works out how to take everything patchwork deployed back out of `game`,
/// or out of wherever profile `id` is deployed to (its instance, if it has
/// one), like `patchwork undeploy`, to be confirmed with `apply_plan` as
/// well.
#[tauri::command]
fn plan_undeploy(app: State<'_, App>, game: String, id: Option<String>) -> CmdResult<PlanSummary> {
    let target = match id {
        Some(id) => {
            let loaded = app.pw.load_profile(&profile_ref(&id)?).map_err(msg)?;
            app.pw.deploy_target(&loaded).map_err(msg)?
        }
        None => DeployTarget::Game(game),
    };
    let plan = app.pw.plan_undeploy(&target).map_err(msg)?;
    Ok(hold_plan(&app, plan))
}

/// Keeps `plan` for `apply_plan` and summarises it for the UI.
fn hold_plan(app: &App, plan: DeployPlan) -> PlanSummary {
    let summary = PlanSummary::of(&plan);
    *app.plan.lock().unwrap() = Some(plan);
    summary
}

/// Applies the plan from `plan_undeploy`. `cancel_job` stops it.
#[tauri::command]
async fn apply_plan(handle: AppHandle, app: State<'_, App>) -> CmdResult<Deployed> {
    let _work = app.work.start()?;
    let job = Job::start(&app);
    let plan = app
        .plan
        .lock()
        .unwrap()
        .take()
        .ok_or("there's no undeploy waiting to be applied")?;
    apply_cancellable(handle, plan, job.0.clone()).await
}

/// Starts the game of the profile `id` the way `option` says (by id), or
/// the default way, like `patchwork launch --no-deploy`: deploying first
/// is up to the UI. Says how it was started, with a warning if Steam is
/// needed and isn't running.
#[tauri::command]
async fn launch_game(app: State<'_, App>, id: String, option: Option<String>) -> CmdResult<String> {
    let _work = app.work.start()?;
    let r = profile_ref(&id)?;
    let loaded = app.pw.load_profile(&r).map_err(msg)?;
    let game = loaded.game.clone();
    let launch = app
        .pw
        .plan_profile_launch(&r, option.as_deref(), None)
        .map_err(msg)?;
    let name = app
        .pw
        .profile_launch_options(&loaded)
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
    let how = match &launch.via {
        Via::Steam { .. } => " through Steam".to_owned(),
        Via::Direct => String::new(),
        Via::Proton { .. } => " with Proton".to_owned(),
        Via::Wine { .. } => " with Wine".to_owned(),
        Via::Runner { name } => format!(" with {name}"),
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

/// How a link from the browser went, sent to the window as
/// `patchwork://link`. Once patchwork knows which file a link is for, the
/// events say so, and `Downloading` gives the `name` its progress comes
/// under in `patchwork://event`.
#[derive(Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum LinkEvent {
    Downloading {
        file: SourceFile,
        name: String,
    },
    Downloaded {
        file: SourceFile,
    },
    Failed {
        file: Option<SourceFile>,
        message: String,
    },
}

/// Downloads what each link from the browser is for, like Nexus's
/// `nxm://` ones, in the background.
fn accept_links(handle: &AppHandle, links: Vec<tauri::Url>) {
    for link in links {
        let handle = handle.clone();
        tauri::async_runtime::spawn(async move {
            // the key in the link is only good for this download, but
            // still isn't worth printing
            let shown = link.as_str().split('?').next().unwrap_or_default();
            let event = accept_link(&handle, &link).await;
            match &event {
                LinkEvent::Failed { message, .. } => eprintln!("link {shown}: {message}"),
                _ => eprintln!("link {shown}: downloaded"),
            }
            let _ = handle.emit("patchwork://link", event);
        });
    }
}

/// Downloads what `link` is for, telling the window when it starts, and
/// gives how it went.
async fn accept_link(handle: &AppHandle, link: &tauri::Url) -> LinkEvent {
    let failed = |file: Option<&SourceFile>, message: String| LinkEvent::Failed {
        file: file.cloned(),
        message,
    };
    let app = handle.state::<App>();
    let _work = match app.work.start() {
        Ok(work) => work,
        Err(e) => return failed(None, e.to_string()),
    };
    let linked = match app.pw.link_file(link.as_str()).await {
        Ok(Some(linked)) => linked,
        Ok(None) => return failed(None, format!("patchwork doesn't know what {link} is for")),
        Err(e) => return failed(None, e.to_string()),
    };
    let _ = handle.emit(
        "patchwork://link",
        LinkEvent::Downloading {
            file: linked.file.clone(),
            name: linked.name.clone(),
        },
    );
    match app.pw.download_linked(&linked).await {
        Ok(()) => LinkEvent::Downloaded { file: linked.file },
        Err(e) => failed(Some(&linked.file), e.to_string()),
    }
}

/// The built-in look. It always loads first, straight from the binary, so
/// changes to it reach everyone; users' own CSS goes in files loaded after it.
const BUILTIN_CSS: &str = include_str!("default.css");

/// Written out as `<config>/themes/user.css` when that file doesn't exist,
/// so users can see what there is to change.
const USER_CSS: &str = include_str!("user.css");

/// The CSS to load, in order: `builtin`, then `global`, `game` and `user`.
#[derive(Serialize)]
struct Theme {
    builtin: &'static str,
    /// The theme picked for every game, unless `game` takes its place.
    global: Option<AppliedTheme>,
    /// The open game's own theme.
    game: Option<AppliedTheme>,
    /// The open game's colour, between `global` and `game`.
    accent: Option<themes::AccentColors>,
    /// `<config>/themes/user.css`, the user's own changes, last of all.
    user: Option<String>,
    /// Light, dark or as the system is.
    mode: themes::ModePref,
    /// Whether to stop what moves whatever the system says.
    reduce_motion: bool,
    /// The modes the themes in use have looks for; empty for either.
    modes: Vec<themes::Mode>,
    /// Themes or preferences that couldn't be read.
    problems: Vec<String>,
}

#[derive(Serialize)]
struct AppliedTheme {
    id: String,
    name: String,
    css: String,
    /// For the picker's preview, which works out the modes again.
    modes: Vec<themes::Mode>,
    layer: bool,
    /// Its settings and their values, which theme.js turns into CSS.
    options: Vec<theme_options::ThemeOption>,
    values: std::collections::BTreeMap<String, theme_options::OptionValue>,
}

/// The themes there are, and the preferences that pick between them.
#[derive(Serialize)]
struct ThemeList {
    themes: Vec<themes::ThemeInfo>,
    prefs: themes::ThemePrefs,
    problems: Vec<String>,
}

/// The `themes/` folder of each registry layer after the built-in one, in
/// order, ending with the user's own `<config>/themes`.
fn theme_dirs(paths: &Paths) -> Vec<(themes::ThemeOrigin, std::path::PathBuf)> {
    let (layers, _) = patchwork_core::registry::Registry::layers(paths);
    layers
        .into_iter()
        .map(|(origin, dir)| {
            let origin = match origin {
                patchwork_core::registry::Origin::User => themes::ThemeOrigin::User,
                _ => themes::ThemeOrigin::Registry,
            };
            (origin, dir.join("themes"))
        })
        .collect()
}

/// Every theme (writing the files that should be in `<config>/themes`
/// first), the `gui.toml` preferences, and anything that went wrong.
fn load_themes(app: &App) -> (themes::Catalog, themes::ThemePrefs, Vec<String>) {
    let config = &app.pw.paths().config;
    let dir = config.join("themes");
    let mut problems = Vec::new();
    if let Err(e) = write_theme_files(&dir) {
        problems.push(format!(
            "couldn't write the theme files in {}: {e}",
            dir.display()
        ));
    }
    let registry = app.pw.registry();
    let dirs = theme_dirs(app.pw.paths());
    let mut catalog = themes::Catalog::load(&dirs, &|id| registry.game(id).is_ok());
    let prefs = match themes::GuiPrefs::load(config) {
        Ok(p) => p.theme,
        Err(e) => {
            problems.push(e);
            themes::ThemePrefs::default()
        }
    };
    catalog.set_values(&prefs);
    (catalog, prefs, problems)
}

/// The themes to use with `game` open. `pick`, when given, is used as the
/// theme picked for `game` instead of what `gui.toml` says, for a preview:
/// a value of `[theme.games]`, or `""` for none.
#[tauri::command]
fn theme(app: State<'_, App>, game: Option<String>, pick: Option<String>) -> Theme {
    let (catalog, mut prefs, mut problems) = load_themes(&app);
    if let (Some(game), Some(pick)) = (&game, pick) {
        if pick.is_empty() {
            prefs.games.remove(game);
        } else {
            prefs.games.insert(game.clone(), pick);
        }
    }
    let registry = app.pw.registry();
    let accent = game
        .as_deref()
        .and_then(|g| registry.game(g).ok())
        .and_then(|g| g.value.accent.as_ref());
    let resolved = catalog.resolve(&prefs, game.as_deref(), accent);
    problems.extend(catalog.problems.iter().cloned());
    problems.extend(resolved.problems.iter().cloned());
    let applied = |t: &themes::ThemeInfo| AppliedTheme {
        id: t.id.clone(),
        name: t.name.clone(),
        css: t.css.clone(),
        modes: t.modes.clone(),
        layer: t.layer,
        options: t.options.clone(),
        values: t.values.clone(),
    };
    let path = app.pw.paths().config.join("themes").join("user.css");
    let user = match std::fs::read_to_string(&path) {
        Ok(css) => Some(css),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            problems.push(format!("couldn't read {}: {e}", path.display()));
            None
        }
    };
    Theme {
        builtin: BUILTIN_CSS,
        mode: prefs.mode,
        reduce_motion: prefs.reduce_motion,
        modes: resolved.modes(),
        global: resolved.global.map(applied),
        game: resolved.game.map(applied),
        accent: resolved.accent,
        user,
        problems,
    }
}

/// Saves the `[theme]` preferences in `gui.toml`.
#[tauri::command]
fn set_theme_prefs(app: State<'_, App>, prefs: themes::ThemePrefs) -> CmdResult<()> {
    let config = &app.pw.paths().config;
    let mut file = themes::GuiPrefs::load(config)?;
    // Keep keys the frontend didn't send back.
    let rest = std::mem::take(&mut file.theme.rest);
    file.theme = themes::ThemePrefs { rest, ..prefs };
    Ok(file.save(config)?)
}

/// Pins `game` to the top of the game picker, or unpins it.
/// Whether the user would hear about a followed mod's new versions, and if
/// not, why not.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Followed {
    /// Why no notification would come, if one wouldn't.
    unheard: Option<&'static str>,
}

/// Follows the mod `source:id`, or stops following it.
#[tauri::command]
fn set_followed(
    app: State<'_, App>,
    source: String,
    id: String,
    follow: bool,
) -> CmdResult<Followed> {
    let paths = app.pw.paths();
    patchwork_core::follow::set_followed(paths, &source, &id, follow).map_err(msg)?;
    let settings = patchwork_daemon::DaemonSettings::load(paths).map_err(msg)?;
    let updates = settings.task("updates");
    let unheard = if !follow {
        None
    } else if patchwork_daemon::service::installed()
        .map_err(msg)?
        .is_none()
    {
        Some("the background daemon isn't installed")
    } else if !settings.on() || updates.enabled == Some(false) {
        Some("the background update check is off")
    } else if !updates.notifies() {
        Some("update notifications are off")
    } else {
        None
    };
    Ok(Followed { unheard })
}

#[tauri::command]
fn set_favorite(app: State<'_, App>, game: String, favorite: bool) -> CmdResult<()> {
    let config = &app.pw.paths().config;
    let mut file = themes::GuiPrefs::load(config)?;
    if favorite {
        file.favorites.insert(game);
    } else {
        file.favorites.remove(&game);
    }
    Ok(file.save(config)?)
}

#[tauri::command]
fn theme_list(app: State<'_, App>) -> ThemeList {
    let (catalog, prefs, mut problems) = load_themes(&app);
    problems.extend(catalog.problems);
    ThemeList {
        themes: catalog.themes.into_values().collect(),
        prefs,
        problems,
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

/// Opens patchwork with every extension there is.
fn open(paths: Paths, reporter: impl Reporter + 'static) -> patchwork_core::Result<Patchwork> {
    patchwork_builtin::register(Patchwork::builder(paths))
        .reporter(reporter)
        .open()
}

/// With `$PATCHWORK_HOME` set, the app is one instance per home rather than
/// one in all, so separate homes can run side by side (say, to try syncing
/// between them). Only Linux can be told this; elsewhere it stays one in all.
#[cfg(target_os = "linux")]
fn instance_id(identifier: &str) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let home = std::env::var_os("PATCHWORK_HOME")?;
    let home = std::path::absolute(&home).unwrap_or(home.into());
    // the same binary hashes alike, which is all that's needed
    let mut hasher = std::hash::DefaultHasher::new();
    home.hash(&mut hasher);
    Some(format!("{identifier}.home{:016x}", hasher.finish()))
}

fn main() {
    // where the build said GIO's modules are (see build.rs), so the webview
    // has TLS even when started outside the shell that built it
    if let Some(dir) = option_env!("PATCHWORK_GUI_GIO_MODULE_DIR")
        && std::env::var_os("GIO_MODULE_DIR").is_none()
    {
        // SAFETY: nothing else is running yet to read the environment
        unsafe { std::env::set_var("GIO_MODULE_DIR", dir) };
    }
    if std::env::args().any(|a| a == daemon::FLAG || a == daemon::OLD_FLAG) {
        if let Err(e) = daemon::run(|paths| open(paths, Silent)) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }
    let context = tauri::generate_context!();
    let single_instance = tauri_plugin_single_instance::Builder::new().callback(|app, _, _| {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
    });
    #[cfg(target_os = "linux")]
    let single_instance = match instance_id(&context.config().identifier) {
        Some(id) => single_instance.dbus_id(id),
        None => single_instance,
    };
    tauri::Builder::default()
        // first, so a second launch (say, for an `nxm://` link) hands its
        // link to this one and exits before doing anything else
        .plugin(single_instance.build())
        .plugin(tauri_plugin_deep_link::init())
        // the pictures in theme folders
        .register_uri_scheme_protocol(theme_files::SCHEME, |ctx, request| {
            let dirs: Vec<_> = ctx
                .app_handle()
                .try_state::<App>()
                .map(|app| theme_dirs(app.pw.paths()))
                .unwrap_or_default()
                .into_iter()
                .map(|(_, dir)| dir)
                .collect();
            theme_files::respond(&dirs, request.uri().path())
        })
        .setup(|app| {
            let paths = Paths::from_env()?;
            // the daemon has patchwork open while it runs, so it steps aside
            // until the app's closed
            let paused = daemon::pause_for_app(&paths);
            let pw = open(paths, Emit(app.handle().clone()))?;
            app.manage(App {
                pw,
                plan: Mutex::new(None),
                job: Mutex::new(None),
                work: Default::default(),
                details: details::Details::default(),
                sync: Default::default(),
                daemon: daemon::Daemon::new(paused),
            });
            sync::start_if_synced(app.handle());
            let state = app.state::<App>();
            let paths = state.pw.paths();
            let dirs = theme_dirs(paths).into_iter().map(|(_, d)| d).collect();
            theme_watch::start(app.handle().clone(), dirs, &paths.config.join("themes"));

            use tauri_plugin_deep_link::DeepLinkExt;
            let handle = app.handle().clone();
            app.deep_link()
                .on_open_url(move |event| accept_links(&handle, event.urls()));
            // patchwork may have been started by a link, before it was
            // listening for them
            if let Some(links) = app.deep_link().get_current()? {
                accept_links(app.handle(), links);
            }
            links::on_startup(app.handle());
            cli::refresh_copy(&app.state::<App>());
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(READY_TIMEOUT);
                if let Some(window) = handle.get_webview_window("main") {
                    let _ = window.show();
                }
            });
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            blockers::apply_fix,
            blockers::preview_undeploy,
            ready,
            native_color_picker,
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
            detect_games,
            game_settings,
            game_env,
            set_game_settings,
            setup::set_profile_env,
            setup::set_profile_instance,
            setup::tool_output,
            setup::capture,
            source_settings,
            set_source_setting,
            check_source_settings,
            set_enabled,
            set_priority,
            mod_fields,
            game_fields,
            set_game_edits,
            picture_preview,
            set_mod_edits,
            remove_mod,
            revert_profile,
            profile_history,
            rollback_profile,
            set_hidden_tags,
            add_folder,
            rename_folder,
            remove_folder,
            set_mod_folder,
            set_personal,
            set_folder_enabled,
            other_folders,
            use_folder,
            include_set,
            copy_set,
            unlink_set,
            sort_mods,
            reorder_mods,
            list,
            set_list,
            run_action,
            mod_options,
            set_mod_options,
            config_files,
            config_form,
            set_config,
            conflicts,
            refresh_details,
            find_games,
            find_mods,
            set_version,
            set_replaces,
            set_game_versions,
            set_ignored,
            dismiss_update,
            changelog::preview_updates,
            changelog::mod_changelog,
            update_mods,
            add_mod,
            file_choices,
            parse_link,
            link_source,
            search,
            search_categories,
            deploy,
            cancel_job,
            plan_undeploy,
            apply_plan,
            launch_game,
            set_launch_option,
            lock_profile,
            provide_download,
            links::handle_links,
            links::link_handler,
            theme,
            theme_list,
            set_theme_prefs,
            set_favorite,
            selfupdate::check_for_update,
            selfupdate::install_update,
            selfupdate::cancel_update,
            selfupdate::update_check_automatic,
            cli::cli_status,
            daemon::daemon_status,
            daemon::daemon_missed,
            daemon::daemon_install,
            daemon::daemon_restart,
            daemon::daemon_uninstall,
            daemon::daemon_tasks,
            daemon::daemon_set_task,
            daemon::daemon_set_notify,
            set_followed,
            daemon::daemon_set_enabled,
            cli::install_cli,
            sync::sync_status,
            sync::sync_share,
            sync::sync_join,
            sync::sync_leave,
            sync::sync_linked,
            sync::sync_link,
            sync::sync_unlink,
            sync::sync_peers,
            sync::sync_name_peer,
            sync::sync_remove_peers,
        ])
        .run(context)
        .expect("error while running patchwork");
}
