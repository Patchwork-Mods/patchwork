use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::time::UNIX_EPOCH;

use patchwork_core::check::{Check, CheckContext, Problem};
use patchwork_core::deploy::{
    BuildContext, DeployContext, Deployer, DesiredState, Dest, GameFiles, InstalledMod, Placement,
    Src,
};
use patchwork_core::game::GameDef;
use patchwork_core::hash::ContentHash;
use patchwork_core::list::{List, ListContext, ListItem, ListItems, OrderedList};
use patchwork_core::relpath::RelPath;
use patchwork_core::settings::SettingSpec;
use patchwork_core::{Error, Result};
use serde::Deserialize;

use crate::RulesDeployer;

/// Bethesda's games since Oblivion: mods' files are laid out by the same
/// rules as [`RulesDeployer`], and the plugins among them (`.esp`, `.esm`
/// and `.esl` files) are written to the file the game reads its load order
/// from, in the order the profile puts them in with [`BethesdaLoadOrder`].
///
/// ```toml
/// [deployer]
/// kind = "bethesda"
/// # ... and anything the rules deployer takes
///
/// [ext.bethesda.plugins]
/// folder = "Data"                 # where plugins go, in the game folder
/// file = "appdata:plugins.txt"    # the load order, in one of the game's roots
/// implicit = ["Skyrim.esm"]       # always loaded first
/// ccc = "Skyrim.ccc"              # lists Creation Club plugins, also loaded first
/// limit = 254                     # how many plugins that aren't light it can load
/// light_limit = 4096              # and how many light ones
/// ```
pub struct BethesdaDeployer;

/// The order the game loads mods' plugins in: the Bethesda extension's
/// `plugins` list, configured by the game's `[ext.bethesda.plugins]` (see
/// [`BethesdaDeployer`]).
pub struct BethesdaLoadOrder;

/// Warns when more plugins are enabled than the game can load: more that
/// aren't light than the `limit` in `[ext.bethesda.plugins]`, or more
/// light ones than its `light_limit`. The Bethesda
/// extension's `limit` check, so a game can turn it off with
/// `off = ["limit"]` in `[ext.bethesda]`.
pub struct PluginLimit;

const KIND: &str = "bethesda";
/// The extension the load order belongs to, and its name there.
pub const EXTENSION: &str = "bethesda";
pub const LIST: &str = "plugins";
/// The name of [`PluginLimit`] in the extension.
pub const LIMIT: &str = "limit";
/// Shown as the mod the load order comes from.
const BUILT_BY: &str = "Load order";
/// What the load order is called in the build's output.
const OUT: &str = "plugins.txt";
/// No plugin's header comes close to this; it stops a broken one from
/// asking for gigabytes.
const MAX_HEADER: u32 = 1 << 20;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginsConfig {
    folder: String,
    file: String,
    #[serde(default)]
    implicit: Vec<String>,
    ccc: Option<String>,
    limit: Option<usize>,
    light_limit: Option<usize>,
}

impl PluginsConfig {
    fn parse(game: &GameDef, table: &toml::Table) -> Result<Self> {
        toml::Value::Table(table.clone()).try_into().map_err(|e| {
            error(format!(
                "bad `[ext.{EXTENSION}.{LIST}]` for {}: {e}",
                game.id
            ))
        })
    }

    /// The game's, if its load order is on.
    fn of(game: &GameDef) -> Result<Option<Self>> {
        if !game.hook_on(EXTENSION, LIST) {
            return Ok(None);
        }
        let empty = toml::Table::new();
        let table = game.hook_config(EXTENSION, LIST).unwrap_or(&empty);
        Self::parse(game, table).map(Some)
    }
}

fn error(message: String) -> Error {
    Error::Deployer {
        kind: KIND.to_owned(),
        message,
    }
}

fn is_plugin(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".esp", ".esm", ".esl"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

impl PluginsConfig {
    /// The ccc file, in the game folder.
    fn ccc(&self) -> Result<Option<RelPath>> {
        self.ccc
            .as_deref()
            .map(|ccc| RelPath::new(ccc).map_err(|e| error(format!("bad `ccc`: {e}"))))
            .transpose()
    }

    fn file(&self) -> Result<Dest> {
        self.file
            .parse()
            .map_err(|e| error(format!("bad `file`: {e}")))
    }

    /// Whether `dst` is a plugin, directly in the plugins folder.
    fn holds(&self, dst: &Dest) -> bool {
        dst.root.is_game()
            && dst
                .path
                .parent()
                .is_some_and(|p| p.as_str().eq_ignore_ascii_case(&self.folder))
            && is_plugin(dst.path.file_name())
    }

    fn folder(&self) -> Result<RelPath> {
        RelPath::new(&self.folder).map_err(|e| error(format!("bad `folder`: {e}")))
    }

    /// The plugins the game always loads first: the ones it's set to, and
    /// the Creation Club plugins it lists that are among `present` (the
    /// game's own plugins).
    fn implicit(&self, game: &GameFiles<'_>, present: &[String]) -> Result<Vec<String>> {
        let mut implicit = self.implicit.clone();
        let Some(ccc) = self.ccc()? else {
            return Ok(implicit);
        };
        let Some(path) = game.original(&ccc) else {
            return Ok(implicit);
        };
        let text = std::fs::read(&path).map_err(|e| error(format!("{ccc}: {e}")))?;
        for line in String::from_utf8_lossy(&text).lines() {
            let name = line.trim();
            // Windows doesn't care about case, so the list may not match
            // what's on disk.
            let there = present.iter().any(|p| p.eq_ignore_ascii_case(name));
            if there && !implicit.iter().any(|i| i.eq_ignore_ascii_case(name)) {
                implicit.push(name.to_owned());
            }
        }
        Ok(implicit)
    }

    /// Those of `implicit` that are light, going by the header of the copy
    /// among `present`, or the extension if it can't be read (or isn't
    /// there, as for the game's masters on a test install).
    fn light(&self, game: &GameFiles<'_>, implicit: &[String], present: &[String]) -> Vec<String> {
        implicit
            .iter()
            .filter(|name| {
                let header = present
                    .iter()
                    .find(|p| p.eq_ignore_ascii_case(name))
                    .and_then(|p| RelPath::new(&format!("{}/{p}", self.folder)).ok())
                    .and_then(|rel| game.original(&rel))
                    .and_then(|path| read_header(&path).ok());
                header.is_some_and(|h| h.light) || name.to_lowercase().ends_with(".esl")
            })
            .cloned()
            .collect()
    }
}

/// What a plugin's header says about it.
#[derive(Debug, Default, PartialEq, Eq)]
struct Header {
    master: bool,
    light: bool,
    masters: Vec<String>,
}

const MASTER_FLAG: u32 = 0x1;
const LIGHT_FLAG: u32 = 0x200;

/// Reads the header record (`TES4`) at the start of a plugin. Skyrim's
/// and later games' records start with 24 bytes: the type, the size of the
/// record's data, its flags and three fields that don't matter here. The
/// data is subrecords, each a type, a 16-bit size and that much data;
/// `MAST` ones name the plugin's masters.
fn read_header(path: &Path) -> std::result::Result<Header, String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut head = [0u8; 24];
    file.read_exact(&mut head)
        .map_err(|_| "it's too short to be a plugin".to_owned())?;
    if &head[..4] != b"TES4" {
        return Err("it doesn't start with a TES4 record".to_owned());
    }
    let u32_at = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let size = u32_at(&head, 4);
    let flags = u32_at(&head, 8);
    if size > MAX_HEADER {
        return Err(format!("its header says it's {size} bytes long"));
    }
    let mut data = vec![0u8; size as usize];
    file.read_exact(&mut data)
        .map_err(|_| "its header is cut short".to_owned())?;

    let mut masters = Vec::new();
    let mut at = 0;
    // Set by an `XXXX` subrecord, for a next one too big for 16 bits.
    let mut next_size = None;
    while at + 6 <= data.len() {
        let kind = &data[at..at + 4];
        let len = next_size
            .take()
            .unwrap_or(u16::from_le_bytes([data[at + 4], data[at + 5]]) as usize);
        let start = at + 6;
        let end = start + len;
        if end > data.len() {
            return Err("its header is cut short".to_owned());
        }
        let field = &data[start..end];
        match kind {
            b"XXXX" if len == 4 => next_size = Some(u32_at(field, 0) as usize),
            b"MAST" => {
                let name = field.split(|&b| b == 0).next().unwrap_or_default();
                // Windows-1252, which is ASCII for every name in practice.
                masters.push(name.iter().map(|&b| b as char).collect());
            }
            _ => {}
        }
        at = end;
    }
    Ok(Header {
        master: flags & MASTER_FLAG != 0,
        light: flags & LIGHT_FLAG != 0,
        masters,
    })
}

/// The load order as the game reads it: a `*` before each enabled plugin.
fn plugins_txt(order: &List) -> String {
    let mut text = String::from(
        "# This file is used by the game to keep track of your downloaded content.\r\n# Patchwork wrote it; change the order in Patchwork instead, or it'll be put back.\r\n",
    );
    for p in &order.items {
        if p.enabled {
            text.push('*');
        }
        text.push_str(&p.item.name);
        text.push_str("\r\n");
    }
    text
}

impl Deployer for BethesdaDeployer {
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
        PluginsConfig::of(ctx.game).is_ok_and(|c| c.is_some())
    }

    fn build(
        &self,
        ctx: &DeployContext<'_>,
        build: &BuildContext<'_>,
        _mods: &[InstalledMod],
    ) -> Result<DesiredState> {
        let id = format!("{EXTENSION}.{LIST}");
        let (Some(plugins), Some(order)) = (PluginsConfig::of(ctx.game)?, build.list(&id)) else {
            return Ok(DesiredState::default());
        };
        let out = build.out.join(OUT);
        std::fs::write(&out, plugins_txt(order))
            .map_err(|e| error(format!("couldn't write {}: {e}", out.display())))?;
        Ok(DesiredState {
            placements: vec![Placement {
                dst: plugins.file()?,
                src: Src::Built(RelPath::new(OUT).expect("valid path")),
                mod_name: BUILT_BY.to_owned(),
            }],
            warnings: Vec::new(),
        })
    }

    /// Covers the ccc file, since its plugins are left out of the load
    /// order.
    fn fingerprint(
        &self,
        ctx: &DeployContext<'_>,
        game: &GameFiles<'_>,
    ) -> Result<Option<ContentHash>> {
        let Some(ccc) = PluginsConfig::of(ctx.game)?
            .map(|p| p.ccc())
            .transpose()?
            .flatten()
        else {
            return Ok(None);
        };
        let seen = match game.original(&ccc).and_then(|p| std::fs::metadata(p).ok()) {
            Some(meta) => {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                format!("{ccc}={}:{modified}", meta.len())
            }
            None => format!("{ccc}=none"),
        };
        Ok(Some(ContentHash::of(seen.as_bytes())))
    }
}

/// Masters always load first, whatever the user does.
const MASTERS: u8 = 0;
const OTHERS: u8 = 1;
const MASTER: &str = "master";
const LIGHT: &str = "light";

impl OrderedList for BethesdaLoadOrder {
    fn title(&self) -> &str {
        "Plugins"
    }

    fn description(&self) -> Option<&str> {
        Some(
            "The game loads these one after another, and later plugins win when two change the \
             same thing. This order is separate from the mods' order, and it's written for the \
             game when you deploy.",
        )
    }

    fn items(&self, ctx: &ListContext<'_>, mods: &[InstalledMod]) -> Result<ListItems> {
        let plugins = PluginsConfig::parse(ctx.game(), ctx.config())?;
        let game = ctx.files();
        let mut warnings = Vec::new();
        let mut found: Vec<ListItem> = Vec::new();
        for placement in RulesDeployer.plan(ctx.deploy(), mods)?.placements {
            if !plugins.holds(&placement.dst) {
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
            let name = placement.dst.path.file_name().to_owned();
            let lower = name.to_lowercase();
            let header = read_header(&src.src).unwrap_or_else(|why| {
                warnings.push(format!(
                    "couldn't read {name} from `{}`: {why}",
                    placement.mod_name
                ));
                Header::default()
            });
            // The game goes by the extension as well as the flags.
            let master = header.master || lower.ends_with(".esm") || lower.ends_with(".esl");
            let light = header.light || lower.ends_with(".esl");
            found.push(plugin_item(
                name,
                placement.mod_name,
                master,
                light,
                header.masters,
            ));
        }

        let present: Vec<String> = game
            .list(Some(&plugins.folder()?))?
            .into_iter()
            .map(|rel| rel.file_name().to_owned())
            .filter(|name| is_plugin(name))
            .collect();
        let implicit = plugins.implicit(game, &present)?;
        let light_implicit = plugins.light(game, &implicit, &present);
        let unlisted: Vec<_> = present
            .into_iter()
            .filter(|name| !implicit.iter().any(|i| i.eq_ignore_ascii_case(name)))
            .filter(|name| !found.iter().any(|p| p.name.eq_ignore_ascii_case(name)))
            .collect();
        if !unlisted.is_empty() {
            warnings.push(format!(
                "{} in {} {} from any of the profile's mods, so the game won't load {} while it's deployed (if a tool made {}, `patchwork capture` keeps {} as a mod): {}",
                if unlisted.len() == 1 { "a plugin" } else { "plugins" },
                plugins.folder,
                if unlisted.len() == 1 { "isn't" } else { "aren't" },
                if unlisted.len() == 1 { "it" } else { "them" },
                if unlisted.len() == 1 { "it" } else { "them" },
                if unlisted.len() == 1 { "it" } else { "them" },
                unlisted.join(", ")
            ));
        }
        let fixed = implicit
            .into_iter()
            .map(|name| {
                let light = light_implicit.contains(&name);
                ListItem {
                    can_disable: false,
                    ..plugin_item(name, String::new(), true, light, Vec::new())
                }
            })
            .collect();
        Ok(ListItems {
            items: found,
            fixed,
            tiers: vec!["Masters".to_owned(), "Plugins".to_owned()],
            warnings,
        })
    }
}

impl Check for PluginLimit {
    fn check(&self, ctx: &CheckContext<'_>, _mods: &[InstalledMod]) -> Vec<Problem> {
        let (Some(list), Ok(Some(config))) = (ctx.list(LIST), PluginsConfig::of(ctx.game())) else {
            return Vec::new();
        };
        let is_light = |item: &ListItem| item.badges.iter().any(|b| b == LIGHT);
        let enabled = list
            .fixed
            .iter()
            .chain(list.items.iter().filter(|i| i.enabled).map(|i| &i.item));
        let (light, full): (Vec<_>, Vec<_>) = enabled.partition(|i| is_light(i));
        let mut problems = Vec::new();
        if let Some(limit) = config.limit
            && full.len() > limit
        {
            problems.push(Problem::warning(format!(
                "{} plugins that aren't light are enabled, counting the game's own, but the game can only load {limit}; turn some off in the load order, or use light versions of them",
                full.len()
            )));
        }
        if let Some(limit) = config.light_limit
            && light.len() > limit
        {
            problems.push(Problem::warning(format!(
                "{} light plugins are enabled, counting the game's own, but the game can only load {limit}; turn some off in the load order, or merge some of them",
                light.len()
            )));
        }
        problems.into_iter().map(|p| p.about_list(LIST)).collect()
    }
}

/// A plugin as an item in the load order.
fn plugin_item(
    name: String,
    mod_name: String,
    master: bool,
    light: bool,
    masters: Vec<String>,
) -> ListItem {
    let mut badges = Vec::new();
    if light {
        badges.push(LIGHT.to_owned());
    } else if master {
        badges.push(MASTER.to_owned());
    }
    let detail = if masters.is_empty() {
        "Doesn't need any other plugins".to_owned()
    } else {
        format!("Needs {}", masters.join(", "))
    };
    ListItem {
        name,
        mod_name,
        tier: if master { MASTERS } else { OTHERS },
        badges,
        after: masters,
        detail: Some(detail),
        can_disable: true,
    }
}

#[cfg(test)]
mod tests;
