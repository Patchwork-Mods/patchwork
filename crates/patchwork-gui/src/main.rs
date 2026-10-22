// Don't open a console window alongside the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod changelog;
mod details;

use std::fmt::Display;
use std::sync::Mutex;

use base64::prelude::*;
use patchwork_core::deploy::{DeployOp, DeployPlan, DeployTarget};
use patchwork_core::event::{Event, Reporter};
use patchwork_core::game::{DeployerSpec, GameDef, LinkMethod};
use patchwork_core::lock::{Lockfile, mod_key};
use patchwork_core::matching::{self, ModText};
use patchwork_core::order::SortKey;
use patchwork_core::paths::Paths;
use patchwork_core::profile::{ProfileMod, ProfileRef};
use patchwork_core::search::{Category, ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::source::{ModSpec, SourceFile};
use patchwork_core::{DeployStatus, GameInstall, ImportOptions, Patchwork};
use patchwork_deployers::{RulesDeployer, SmfDeployer};
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
    /// Dependency requirements the locked versions don't meet.
    problems: Vec<String>,
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UiMod {
    #[serde(flatten)]
    profile: ProfileMod,
    /// `<source>:<id>`, which events about the mod name it by.
    key: String,
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
    let profiles = pw
        .profiles()
        .map_err(msg)?
        .into_iter()
        .map(|(r, p)| ProfileInfo {
            id: r.to_string(),
            label: p.label(),
            mods: p.mods.len(),
            game: p.game,
            name: p.name,
        })
        .collect();
    Ok(Overview {
        games,
        profiles,
        current: pw.current_profile().map_err(msg)?.map(|r| r.to_string()),
        rulesets: registry.data_ids("rulesets").map(str::to_owned).collect(),
        warnings: registry
            .problems
            .iter()
            .map(|p| format!("skipped registry entry: {p}"))
            .collect(),
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
    let problems = lock
        .as_ref()
        .map(|l| {
            pw.unmet_requirements(l)
                .iter()
                .map(|u| u.to_string())
                .collect()
        })
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
                versions: known.map(|k| k.versions),
                summary: summary.and_then(|k| k.summary),
                versioned,
                locked: lock
                    .iter()
                    .flat_map(|l| &l.mods)
                    .find(|l| l.key() == key)
                    .and_then(|l| l.version.clone()),
                key,
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
        problems,
        id,
        install,
        deployed_files,
        game_files,
        deploy_status,
        has_changes,
        search_sources,
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

/// A game's settings on this machine, for the game settings dialog. `None`
/// until it's been set up.
#[tauri::command]
fn game_settings(app: State<'_, App>, game: String) -> CmdResult<Option<GameInstall>> {
    app.pw.game_install(&game).map_err(msg)
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

#[tauri::command]
async fn plan_deploy(handle: AppHandle, app: State<'_, App>, id: String) -> CmdResult<PlanSummary> {
    let plan = app
        .pw
        .plan_deploy(&profile_ref(&id)?)
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

// Downloads the user has to start themselves (see `patchwork://pending`).

/// Locks the profile, e.g. once the user has seen to its pending downloads.
#[tauri::command]
async fn lock_profile(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
) -> CmdResult<ProfileDetail> {
    let r = profile_ref(&id)?;
    app.pw.lock(&r, false).await.map_err(lock_msg(&handle))?;
    profile_detail(&app.pw, id)
}

/// Uses a file the user downloaded themselves for a pending download.
#[tauri::command]
fn provide_download(app: State<'_, App>, file: SourceFile, path: String) -> CmdResult<()> {
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
            let _ = handle.emit("patchwork://link", LinkEvent::Downloading);
            let app = handle.state::<App>();
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

fn main() {
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
            let pw = Patchwork::builder(Paths::from_env()?)
                .source(LocalSource)
                .source(Thunderstore)
                .source(Nexus::from_env())
                .source(UrlSource)
                .deployer(RulesDeployer)
                .deployer(SmfDeployer)
                .format(Archive)
                .format(UrlList)
                .format(R2modman)
                .format(ThunderstoreCode::default())
                .reporter(Emit(app.handle().clone()))
                .open()?;
            app.manage(App {
                pw,
                plan: Mutex::new(None),
                details: details::Details::default(),
            });

            use tauri_plugin_deep_link::DeepLinkExt;
            let handle = app.handle().clone();
            app.deep_link()
                .on_open_url(move |event| accept_links(&handle, event.urls()));
            // patchwork may have been started by a link, before it was
            // listening for them
            if let Some(links) = app.deep_link().get_current()? {
                accept_links(app.handle(), links);
            }
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
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
            game_settings,
            set_game_settings,
            set_enabled,
            remove_mod,
            revert_profile,
            set_hidden_tags,
            sort_mods,
            reorder_mods,
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
            lock_profile,
            provide_download,
            handle_links,
            theme,
        ])
        .run(tauri::generate_context!())
        .expect("error while running patchwork");
}
