//! Baldur's Gate 3: the `bg3` extension.
//!
//! Most BG3 mods are `.pak` packages, which go in the `Mods` folder in the
//! game's AppData. The game only loads the ones listed in
//! `PlayerProfiles/Public/modsettings.lsx`, in the order listed there, and
//! the list names each one by what its `Mods/<folder>/meta.lsx` (inside the
//! package) says: UUID, folder, name and version. Packages without a
//! `meta.lsx` are overrides, which the game loads without being listed.
//!
//! So the extension's `mods` list is the modules in the profile's packages,
//! read from their `meta.lsx`, in an order the user picks, with each one's
//! dependencies to check that order against. Its deployer lays out mods'
//! files with the rules deployer and writes `modsettings.lsx` from the list.
//! Its `extender` check warns when a mod needs Script Extender and it isn't
//! there.
//!
//! Mods installed from the in-game mod manager (or by hand) aren't in the
//! profile, but their packages are in the same `Mods` folder. The ones the
//! game's `modsettings.lsx` has turned on are kept in it, right after the
//! campaign, and shown as fixed entries in the list.
//!
//! ```toml
//! [deployer]
//! kind = "bg3"
//! # ... and anything the rules deployer takes; `.pak` files go in the
//! # root named by `paks` below, as `to = "mods:{file}"`
//!
//! [ext.bg3.mods]
//! paks = "mods"                       # the root mods' packages go in
//! file = "profile:modsettings.lsx"    # where the load order goes
//! # The game's own modules, which mods may name as dependencies.
//! builtin = ["991c9c7a-fb80-40cb-8f0d-b92d4e80e9b1"]
//!
//! # The campaign, which is always first in the load order.
//! [ext.bg3.mods.base]
//! uuid = "cb555efe-2d9e-131f-8195-a89329d218ea"
//! folder = "GustavX"
//! name = "GustavX"
//! version64 = 145241946983074840
//!
//! [ext.bg3.extender]
//! file = "bin/DWrite.dll"             # what Script Extender installs
//! ```

mod meta;
mod pak;

use std::collections::BTreeMap;
use std::path::PathBuf;

use patchwork_core::check::{Check, CheckContext, Problem};
use patchwork_core::deploy::{
    BuildContext, DeployContext, Deployer, DesiredState, Dest, GameFiles, InstalledMod, Placement,
    RootId, Src,
};
use patchwork_core::ext::{Extension, Registrar};
use patchwork_core::game::GameDef;
use patchwork_core::hash::ContentHash;
use patchwork_core::list::{ListContext, ListItem, ListItems, OrderedList};
use patchwork_core::relpath::RelPath;
use patchwork_core::settings::SettingSpec;
use patchwork_core::{Error, Result};
use patchwork_deployers::RulesDeployer;
use serde::Deserialize;

use meta::Module;
use pak::Pak;

/// The `bg3` extension. See the [crate docs](crate).
pub struct Bg3;

impl Extension for Bg3 {
    fn id(&self) -> &'static str {
        EXTENSION
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(Bg3Deployer)
            .ordered_list(LIST, Bg3LoadOrder)
            .check(EXTENDER, ExtenderCheck);
    }
}

/// Lays out mods' files with the rules deployer, and writes the load order
/// to `modsettings.lsx`.
pub struct Bg3Deployer;

/// The modules in the profile's packages, in the order the game loads them.
pub struct Bg3LoadOrder;

/// Warns when an enabled mod needs Script Extender but it isn't installed.
pub struct ExtenderCheck;

const KIND: &str = "bg3";
const EXTENSION: &str = "bg3";
const LIST: &str = "mods";
const EXTENDER: &str = "extender";
/// Shown as the mod the load order comes from.
const BUILT_BY: &str = "Load order";
/// What the load order is called in the build's output.
const OUT: &str = "modsettings.lsx";
/// The badge on modules that need Script Extender.
const NEEDS_EXTENDER: &str = "script extender";
/// The badge on modules that aren't from the profile's packages.
const NOT_IN_PROFILE: &str = "not in profile";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModsConfig {
    paks: String,
    file: String,
    base: Module,
    #[serde(default)]
    builtin: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtenderConfig {
    file: String,
}

fn error(message: String) -> Error {
    Error::Deployer {
        kind: KIND.to_owned(),
        message,
    }
}

fn parse<T: serde::de::DeserializeOwned>(
    game: &GameDef,
    hook: &str,
    table: &toml::Table,
) -> Result<T> {
    toml::Value::Table(table.clone()).try_into().map_err(|e| {
        error(format!(
            "bad `[ext.{EXTENSION}.{hook}]` for {}: {e}",
            game.id
        ))
    })
}

impl ModsConfig {
    /// The game's, if its load order is on.
    fn of(game: &GameDef) -> Result<Option<Self>> {
        if !game.hook_on(EXTENSION, LIST) {
            return Ok(None);
        }
        let empty = toml::Table::new();
        let table = game.hook_config(EXTENSION, LIST).unwrap_or(&empty);
        parse(game, LIST, table).map(Some)
    }

    fn paks(&self) -> Result<RootId> {
        RootId::new(&self.paks).map_err(|e| error(format!("bad `paks`: {e}")))
    }

    fn file(&self) -> Result<Dest> {
        self.file
            .parse()
            .map_err(|e| error(format!("bad `file`: {e}")))
    }

    fn is_builtin(&self, uuid: &str) -> bool {
        uuid.eq_ignore_ascii_case(&self.base.uuid)
            || self.builtin.iter().any(|b| b.eq_ignore_ascii_case(uuid))
    }
}

/// A module found in one of the profile's packages.
struct Found {
    module: Module,
    /// The name it has in the list: its own, unless another module has that.
    name: String,
    mod_name: String,
    pak: String,
    extender: bool,
}

/// The modules in the packages `mods` deploy, in the order the mods are in,
/// with a later copy of a module (by UUID) replacing an earlier one.
fn scan(
    config: &ModsConfig,
    ctx: &DeployContext<'_>,
    mods: &[InstalledMod],
    warnings: &mut Vec<String>,
) -> Result<Vec<Found>> {
    let root = config.paks()?;
    // Where each package goes, and where it comes from. A later mod's
    // package replaces an earlier one's of the same name.
    let mut paks: Vec<(RelPath, PathBuf, String)> = Vec::new();
    for placement in RulesDeployer.plan(ctx, mods)?.placements {
        let dst = &placement.dst;
        if dst.root != root || !dst.path.file_name().to_lowercase().ends_with(".pak") {
            continue;
        }
        let Src::Mod { hash, rel } = &placement.src else {
            continue;
        };
        let Some(src) = mods
            .iter()
            .filter(|m| m.hash == *hash)
            .flat_map(|m| &m.files)
            .find(|f| f.rel == *rel)
        else {
            continue;
        };
        paks.retain(|(p, _, _)| !p.as_str().eq_ignore_ascii_case(dst.path.as_str()));
        paks.push((dst.path.clone(), src.src.clone(), placement.mod_name));
    }

    let mut found: Vec<Found> = Vec::new();
    for (dst, src, mod_name) in &paks {
        let file = dst.file_name();
        if is_later_part(file, paks.iter().map(|(p, _, _)| p.file_name())) {
            continue;
        }
        // The game finds a package's later parts next to it in the Mods
        // folder, so they're looked for among what's deployed there.
        let part = |name: &str| {
            paks.iter()
                .find(|(p, _, _)| p.file_name().eq_ignore_ascii_case(name))
                .map(|(_, src, _)| src.clone())
        };
        let read = read_modules(src, file, &part);
        let modules = match read {
            Ok(modules) => modules,
            Err(why) => {
                warnings.push(format!(
                    "couldn't read {file} from `{mod_name}`, so it isn't in the load order: {why}"
                ));
                continue;
            }
        };
        for (module, extender) in modules {
            if config.is_builtin(&module.uuid) {
                continue;
            }
            if let Some(i) = found
                .iter()
                .position(|f| f.module.uuid.eq_ignore_ascii_case(&module.uuid))
            {
                let old = found.remove(i);
                warnings.push(format!(
                    "{} is in both {} (from `{}`) and {file} (from `{mod_name}`); the game loads the one in {file}",
                    module.name, old.pak, old.mod_name
                ));
            }
            found.push(Found {
                name: module.name.clone(),
                module,
                mod_name: mod_name.clone(),
                pak: file.to_owned(),
                extender,
            });
        }
    }

    // Two modules with the same name are told apart by their folders.
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for f in &found {
        *seen.entry(f.name.to_lowercase()).or_default() += 1;
    }
    for f in &mut found {
        if seen[&f.name.to_lowercase()] > 1 {
            f.name = format!("{} ({})", f.module.name, f.module.folder);
        }
    }
    Ok(found)
}

/// Whether `file` is a later part of a package split in parts
/// (`Big_1.pak`, after `Big.pak`) among `all`.
fn is_later_part<'a>(file: &str, mut all: impl Iterator<Item = &'a str>) -> bool {
    let stem = &file[..file.len() - ".pak".len()];
    let Some((first, n)) = stem.rsplit_once('_') else {
        return false;
    };
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let first = format!("{first}.pak");
    all.any(|p| p.eq_ignore_ascii_case(&first))
}

/// The modules the game's load order has from packages patchwork didn't
/// deploy: ones from the in-game mod manager, or put in the Mods folder by
/// hand. They're kept, right after the campaign, so deploying doesn't turn
/// them off. Ones the in-game mod manager has turned off aren't in the
/// game's load order, so they stay off.
///
/// It's read from the game's load order as it is now, which after a deploy
/// is patchwork's, with these same modules kept in it, so deploying doesn't
/// change it and the deploy stays current (see [`Deployer::fingerprint`])
/// until the game's list changes.
fn others(
    config: &ModsConfig,
    files: &GameFiles<'_>,
    warnings: &mut Vec<String>,
) -> Result<Vec<Module>> {
    let dest = config.file()?;
    let Some(settings) = files.current_at(&dest) else {
        return Ok(Vec::new());
    };
    let listed = match std::fs::read(&settings)
        .map_err(|e| e.to_string())
        .and_then(|text| meta::parse_modsettings(&String::from_utf8_lossy(&text)))
    {
        Ok(listed) => listed,
        Err(why) => {
            warnings.push(format!(
                "couldn't read the game's {}, so mods that aren't in the profile are left out of \
                 the load order: {why}",
                dest.path
            ));
            return Ok(Vec::new());
        }
    };
    let listed: Vec<Module> = listed
        .into_iter()
        .filter(|m| !config.is_builtin(&m.uuid))
        .collect();
    if listed.is_empty() {
        return Ok(Vec::new());
    }

    let root = config.paks()?;
    let mut in_paks: Vec<String> = Vec::new();
    for rel in files.list_at(&root, None)? {
        let file = rel.file_name();
        if !file.to_lowercase().ends_with(".pak") {
            continue;
        }
        let Some(path) = files.current_at(&Dest {
            root: root.clone(),
            path: rel.clone(),
        }) else {
            continue;
        };
        // Overrides and packages that can't be read have no modules to keep.
        let part = |name: &str| {
            let sibling = match rel.parent() {
                Some(parent) => format!("{parent}/{name}"),
                None => name.to_owned(),
            };
            files.current_at(&Dest {
                root: root.clone(),
                path: RelPath::new(&sibling).ok()?,
            })
        };
        if let Ok(modules) = read_modules(&path, file, &part) {
            in_paks.extend(modules.into_iter().map(|(m, _)| m.uuid.to_lowercase()));
        }
    }

    let mut kept: Vec<Module> = Vec::new();
    for m in listed {
        let uuid = m.uuid.to_lowercase();
        if in_paks.contains(&uuid) && !kept.iter().any(|k| k.uuid.eq_ignore_ascii_case(&uuid)) {
            kept.push(m);
        }
    }
    Ok(kept)
}

/// The modules in the package at `path`, named `file`, and whether each
/// needs Script Extender (has a `ScriptExtender/Config.json`). `part` finds
/// the package's later parts, by name.
fn read_modules(
    path: &std::path::Path,
    file: &str,
    part: &dyn Fn(&str) -> Option<PathBuf>,
) -> std::result::Result<Vec<(Module, bool)>, String> {
    let mut pak = Pak::open(path)?;
    let metas: Vec<_> = pak
        .entries
        .iter()
        .filter_map(|e| {
            let parts: Vec<_> = e.name.split('/').collect();
            match parts[..] {
                [mods, folder, meta]
                    if mods.eq_ignore_ascii_case("Mods")
                        && meta.eq_ignore_ascii_case("meta.lsx") =>
                {
                    Some((e.clone(), folder.to_owned()))
                }
                _ => None,
            }
        })
        .collect();
    let mut modules = Vec::new();
    for (entry, folder) in metas {
        let text = pak.read(&entry, file, part)?;
        let module = meta::parse(&String::from_utf8_lossy(&text))
            .map_err(|why| format!("its {}: {why}", entry.name))?;
        let config = format!("Mods/{folder}/ScriptExtender/Config.json");
        let extender = pak
            .entries
            .iter()
            .any(|e| e.name.eq_ignore_ascii_case(&config));
        modules.push((module, extender));
    }
    Ok(modules)
}

impl Deployer for Bg3Deployer {
    fn kind(&self) -> &str {
        KIND
    }

    fn settings(&self) -> &[SettingSpec] {
        RulesDeployer.settings()
    }

    fn plan(&self, ctx: &DeployContext<'_>, mods: &[InstalledMod]) -> Result<DesiredState> {
        RulesDeployer.plan(ctx, mods)
    }

    fn builds(&self, ctx: &DeployContext<'_>) -> bool {
        ModsConfig::of(ctx.game).is_ok_and(|c| c.is_some())
    }

    /// The modules from packages patchwork didn't deploy that the game
    /// loads, so turning one on or off in the in-game mod manager makes the
    /// deploy stale.
    fn fingerprint(
        &self,
        ctx: &DeployContext<'_>,
        game: &GameFiles<'_>,
    ) -> Result<Option<ContentHash>> {
        let Some(config) = ModsConfig::of(ctx.game)? else {
            return Ok(None);
        };
        let mut key: Vec<String> = others(&config, game, &mut Vec::new())?
            .iter()
            .map(|m| format!("{} {} {}", m.uuid.to_lowercase(), m.version64, m.md5))
            .collect();
        if key.is_empty() {
            return Ok(None);
        }
        // Sorted, since where they go among themselves can change by the
        // profile having one of them too.
        key.sort();
        Ok(Some(ContentHash::of(key.join("\n").as_bytes())))
    }

    fn build(
        &self,
        ctx: &DeployContext<'_>,
        build: &BuildContext<'_>,
        mods: &[InstalledMod],
    ) -> Result<DesiredState> {
        let id = format!("{EXTENSION}.{LIST}");
        let (Some(config), Some(order)) = (ModsConfig::of(ctx.game)?, build.list(&id)) else {
            return Ok(DesiredState::default());
        };
        // Problems reading packages are already among the list's.
        let found = scan(&config, ctx, mods, &mut Vec::new())?;
        let others = others(&config, &build.game, &mut Vec::new())?;
        let mut modules = vec![&config.base];
        // A profile's copy of a module wins over one put there otherwise.
        modules.extend(others.iter().filter(|o| {
            !found
                .iter()
                .any(|f| f.module.uuid.eq_ignore_ascii_case(&o.uuid))
        }));
        for item in order.items.iter().filter(|i| i.enabled) {
            if let Some(f) = found
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(&item.item.name))
            {
                modules.push(&f.module);
            }
        }
        let out = build.out.join(OUT);
        std::fs::write(&out, meta::modsettings(&modules))
            .map_err(|e| error(format!("couldn't write {}: {e}", out.display())))?;
        Ok(DesiredState {
            placements: vec![Placement {
                dst: config.file()?,
                src: Src::Built(RelPath::new(OUT).expect("valid path")),
                mod_name: BUILT_BY.to_owned(),
            }],
            warnings: Vec::new(),
        })
    }
}

impl OrderedList for Bg3LoadOrder {
    fn title(&self) -> &str {
        "Load order"
    }

    fn description(&self) -> Option<&str> {
        Some(
            "The game loads mods' modules one after another, and later ones win when two change \
             the same thing. Turned-off modules stay installed but aren't loaded. Packages \
             without a module of their own (overrides) are always loaded, so they aren't here. \
             The order is written to modsettings.lsx when you deploy.",
        )
    }

    fn items(&self, ctx: &ListContext<'_>, mods: &[InstalledMod]) -> Result<ListItems> {
        let config: ModsConfig = parse(ctx.game(), LIST, ctx.config())?;
        let mut warnings = Vec::new();
        let found = scan(&config, ctx.deploy(), mods, &mut warnings)?;
        let items = found
            .iter()
            .map(|f| {
                let after: Vec<String> = f
                    .module
                    .dependencies
                    .iter()
                    .filter(|d| !config.is_builtin(&d.uuid))
                    .map(|d| {
                        found
                            .iter()
                            .find(|o| o.module.uuid.eq_ignore_ascii_case(&d.uuid))
                            .map_or_else(|| d.name.clone(), |o| o.name.clone())
                    })
                    .collect();
                let mut detail = format!(
                    "{} {} in {}",
                    f.module.folder,
                    meta::version(f.module.version64),
                    f.pak
                );
                if !after.is_empty() {
                    detail.push_str(&format!("; needs {}", after.join(", ")));
                }
                ListItem {
                    name: f.name.clone(),
                    mod_name: f.mod_name.clone(),
                    tier: 0,
                    badges: if f.extender {
                        vec![NEEDS_EXTENDER.to_owned()]
                    } else {
                        Vec::new()
                    },
                    after,
                    detail: Some(detail),
                    can_disable: true,
                }
            })
            .collect();
        let base = ListItem {
            name: config.base.name.clone(),
            mod_name: String::new(),
            tier: 0,
            badges: Vec::new(),
            after: Vec::new(),
            detail: Some("The game's campaign, always loaded first".to_owned()),
            can_disable: false,
        };
        let mut fixed = vec![base];
        for o in others(&config, ctx.files(), &mut warnings)? {
            if found
                .iter()
                .any(|f| f.module.uuid.eq_ignore_ascii_case(&o.uuid))
            {
                continue;
            }
            fixed.push(ListItem {
                name: o.name.clone(),
                mod_name: String::new(),
                tier: 0,
                badges: vec![NOT_IN_PROFILE.to_owned()],
                after: Vec::new(),
                detail: Some(format!(
                    "{} {}; not in this profile, but turned on in the game (by its own mod \
                     manager, or another one), so it stays loaded, before the profile's",
                    o.folder,
                    meta::version(o.version64)
                )),
                can_disable: false,
            });
        }
        Ok(ListItems {
            items,
            fixed,
            tiers: vec!["Mods".to_owned()],
            warnings,
        })
    }
}

impl Check for ExtenderCheck {
    fn check(&self, ctx: &CheckContext<'_>, mods: &[InstalledMod]) -> Vec<Problem> {
        let Some(list) = ctx.list(LIST) else {
            return Vec::new();
        };
        let Ok(config) = parse::<ExtenderConfig>(ctx.game(), EXTENDER, ctx.config()) else {
            return Vec::new();
        };
        let Ok(file) = RelPath::new(&config.file) else {
            return Vec::new();
        };
        let needing: Vec<_> = list
            .items
            .iter()
            .filter(|i| i.enabled && i.item.badges.iter().any(|b| b == NEEDS_EXTENDER))
            .map(|i| i.item.name.as_str())
            .collect();
        if needing.is_empty() {
            return Vec::new();
        }
        let installed = ctx.files().original(&file).is_some()
            || mods
                .iter()
                .flat_map(|m| &m.files)
                .any(|f| f.rel.file_name().eq_ignore_ascii_case(file.file_name()));
        if installed {
            return Vec::new();
        }
        vec![
            Problem::warning(format!(
                "{} {} Script Extender, which isn't installed; add it as a mod (it's on Nexus Mods and GitHub as BG3SE), or turn {} off",
                needing.join(", "),
                if needing.len() == 1 { "needs" } else { "need" },
                if needing.len() == 1 { "it" } else { "them" },
            ))
            .about_list(LIST),
        ]
    }
}

#[cfg(test)]
mod tests;
