// Don't open a console window alongside the app on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod details;

use std::fmt::Display;
use std::sync::Mutex;

use patchwork_core::Patchwork;
use patchwork_core::deploy::{DeployOp, DeployPlan};
use patchwork_core::event::{Event, Reporter};
use patchwork_core::game::{DeployerSpec, GameDef, LinkMethod};
use patchwork_core::lock::mod_key;
use patchwork_core::order::SortKey;
use patchwork_core::paths::Paths;
use patchwork_core::profile::{ProfileMod, ProfileRef};
use patchwork_core::search::{ModSummary, SearchPage, SearchQuery, SortBy};
use patchwork_core::source::ModSpec;
use patchwork_deployers::RulesDeployer;
use patchwork_formats::{Archive, R2modman, ThunderstoreCode, UrlList};
use patchwork_sources::{LocalSource, Nexus, Thunderstore, UrlSource};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

/// Commands fail with a message for the UI to show.
type CmdResult<T> = Result<T, String>;

fn msg(e: impl Display) -> String {
    e.to_string()
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
    /// Where the definition came from, e.g. `builtin` or `user`.
    origin: String,
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
    mods: Vec<UiMod>,
    /// Mods with any of these tags go in the Mods tab's hidden section.
    hidden_tags: Vec<String>,
    /// Dependency requirements the locked versions don't meet.
    problems: Vec<String>,
    install: Option<String>,
    /// Files patchwork has deployed for this profile's game, if it's this
    /// profile that's deployed.
    deployed_files: Option<usize>,
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
        .map(|entry| {
            let game = &entry.value;
            Ok(GameInfo {
                id: game.id.clone(),
                name: game.name.clone(),
                install: pw
                    .game_install(&game.id)
                    .map_err(msg)?
                    .map(|i| i.path.display().to_string()),
                origin: entry.origin.to_string(),
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
    let state = pw.deploy_state(&loaded.game).map_err(msg)?;
    let deployed_files =
        (state.profile.as_deref() == Some(id.as_str())).then_some(state.files.len());
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
        game: loaded.game,
        mods,
        hidden_tags,
        problems,
        id,
        install,
        deployed_files,
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
        sources,
        deployer: DeployerSpec {
            kind: "rules".to_owned(),
            config,
        },
        link: game.link,
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
fn set_game_path(app: State<'_, App>, game: String, path: String) -> CmdResult<()> {
    app.pw
        .set_game_path(&game, path.as_ref())
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
    app: State<'_, App>,
    id: String,
    name: String,
    version: Option<String>,
) -> CmdResult<ProfileDetail> {
    let r = profile_ref(&id)?;
    app.pw.set_version(&r, &name, version).map_err(msg)?;
    app.pw.lock(&r, false).await.map_err(msg)?;
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

/// Relocks the profile at the newest versions, like `patchwork lock
/// --update`, and returns what changed. Pinned mods stay where they are.
/// Mods that moved to a new version have new details too, so the next
/// `refresh_details` fetches them.
#[tauri::command]
async fn update_lock(app: State<'_, App>, id: String) -> CmdResult<Vec<LockChange>> {
    let r = profile_ref(&id)?;
    let before = app.pw.load_lock(&r).map_err(msg)?;
    let after = app.pw.lock(&r, true).await.map_err(msg)?;
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
    Ok(changes)
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

#[tauri::command]
async fn search(
    app: State<'_, App>,
    game: String,
    source: String,
    text: String,
    sort: Option<SortBy>,
    page: u32,
) -> CmdResult<SearchPage> {
    let query = SearchQuery {
        text: Some(text),
        sort,
        categories: Vec::new(),
        page,
    };
    app.pw.search(&game, &source, &query).await.map_err(msg)
}

#[tauri::command]
async fn plan_deploy(app: State<'_, App>, id: String) -> CmdResult<PlanSummary> {
    let plan = app.pw.plan_deploy(&profile_ref(&id)?).await.map_err(msg)?;
    let summary = PlanSummary {
        ops: plan.ops.iter().cloned().map(UiOp::from).collect(),
        unchanged: plan.unchanged,
        warnings: plan.warnings.clone(),
    };
    *app.plan.lock().unwrap() = Some(plan);
    Ok(summary)
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

/// The built-in look, written out as `<config>/themes/default.css` when that
/// file doesn't exist so users have something to edit.
const DEFAULT_CSS: &str = include_str!("default.css");

#[derive(Serialize)]
struct Theme {
    /// `<config>/themes/default.css`, applied for every game.
    default: String,
    /// `<config>/themes/<game>.css`, applied on top of `default`.
    game: Option<String>,
}

#[tauri::command]
fn theme(app: State<'_, App>, game: Option<String>) -> Theme {
    let dir = app.pw.paths().config.join("themes");
    let default_path = dir.join("default.css");
    let default = match std::fs::read_to_string(&default_path) {
        Ok(css) => css,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // Still use the built-in look if it can't be written out.
            if let Err(e) = std::fs::create_dir_all(&dir)
                .and_then(|()| std::fs::write(&default_path, DEFAULT_CSS))
            {
                eprintln!("couldn't create {}: {e}", default_path.display());
            }
            DEFAULT_CSS.to_owned()
        }
        Err(e) => {
            eprintln!("couldn't read {}: {e}", default_path.display());
            DEFAULT_CSS.to_owned()
        }
    };
    let game = game.and_then(|g| std::fs::read_to_string(dir.join(format!("{g}.css"))).ok());
    Theme { default, game }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let pw = Patchwork::builder(Paths::from_env()?)
                .source(LocalSource)
                .source(Thunderstore)
                .source(Nexus::from_env())
                .source(UrlSource)
                .deployer(RulesDeployer)
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
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            overview,
            profile,
            create_game,
            create_profile,
            use_profile,
            set_game_path,
            set_enabled,
            remove_mod,
            set_hidden_tags,
            sort_mods,
            reorder_mods,
            refresh_details,
            find_mods,
            set_version,
            update_lock,
            add_mod,
            search,
            plan_deploy,
            apply_plan,
            theme,
        ])
        .run(tauri::generate_context!())
        .expect("error while running patchwork");
}
