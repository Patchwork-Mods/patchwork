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

use patchwork_deployers::RulesDeployer;

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
/// # The game's own plugins that it only loads when they're in the load
/// # order (Enderal's, for one). They're in it like mods' plugins.
/// shipped = ["Enderal - Forgotten Stories.esm"]
/// limit = 254                     # how many full plugins it can load
/// light_limit = 4096              # and how many light ones
/// # Starfield only: its header flags differ, and it has medium plugins.
/// light_flag = 0x100              # 0x200 if left out
/// medium_flag = 0x400             # no medium plugins if left out
/// medium_limit = 256
/// ```
pub struct BethesdaDeployer;

/// The order the game loads mods' plugins in: the Bethesda extension's
/// `plugins` list, configured by the game's `[ext.bethesda.plugins]` (see
/// [`BethesdaDeployer`]).
pub struct BethesdaLoadOrder;

/// Warns when more plugins are enabled than the game can load: more full
/// ones (neither light nor medium) than the `limit` in
/// `[ext.bethesda.plugins]`, or more light or medium ones than its
/// `light_limit` or `medium_limit`. The Bethesda
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
    #[serde(default)]
    shipped: Vec<String>,
    ccc: Option<String>,
    limit: Option<usize>,
    light_limit: Option<usize>,
    light_flag: Option<u32>,
    medium_flag: Option<u32>,
    medium_limit: Option<usize>,
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

    /// The size of each of `implicit`, going by the header of the copy
    /// among `present`, or the extension if it can't be read (or isn't
    /// there, as for the game's masters on a test install).
    fn implicit_scales(
        &self,
        game: &GameFiles<'_>,
        implicit: &[String],
        present: &[String],
    ) -> Vec<Scale> {
        implicit
            .iter()
            .map(|name| {
                let flags = present
                    .iter()
                    .find(|p| p.eq_ignore_ascii_case(name))
                    .and_then(|p| RelPath::new(&format!("{}/{p}", self.folder)).ok())
                    .and_then(|rel| game.original(&rel))
                    .and_then(|path| read_header(&path).ok())
                    .map_or(0, |h| h.flags);
                self.scale(flags, name)
            })
            .collect()
    }

    /// How much of the load order a plugin with these header flags takes
    /// up. The `.esl` extension makes any plugin light; a medium flag on a
    /// light plugin is ignored.
    fn scale(&self, flags: u32, name: &str) -> Scale {
        let light = flags & self.light_flag.unwrap_or(LIGHT_FLAG) != 0
            || name.to_lowercase().ends_with(".esl");
        if light {
            Scale::Light
        } else if self.medium_flag.is_some_and(|f| flags & f != 0) {
            Scale::Medium
        } else {
            Scale::Full
        }
    }
}

/// How much of the load order a plugin takes up: games load a few hundred
/// full plugins, and thousands of light ones (Starfield's "small" ones).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scale {
    Full,
    Medium,
    Light,
}

/// Whether a plugin loads with the masters, by its header's flags or its
/// extension (the game goes by both).
fn is_master(flags: u32, name: &str) -> bool {
    let lower = name.to_lowercase();
    flags & MASTER_FLAG != 0 || lower.ends_with(".esm") || lower.ends_with(".esl")
}

/// What a plugin's header says about it: its flags, which mean different
/// things in different games, and the masters it needs.
#[derive(Debug, Default, PartialEq, Eq)]
struct Header {
    flags: u32,
    masters: Vec<String>,
}

const MASTER_FLAG: u32 = 0x1;
/// The light flag in every game but Starfield.
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
    Ok(Header { flags, masters })
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
const MEDIUM: &str = "medium";

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
            let header = read_header(&src.src).unwrap_or_else(|why| {
                warnings.push(format!(
                    "couldn't read {name} from `{}`: {why}",
                    placement.mod_name
                ));
                Header::default()
            });
            let master = is_master(header.flags, &name);
            let scale = plugins.scale(header.flags, &name);
            found.push(plugin_item(
                name,
                placement.mod_name,
                master,
                scale,
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
        let scales = plugins.implicit_scales(game, &implicit, &present);
        // The game's own plugins that aren't loaded unless they're listed
        // go first, as they would in a fresh plugins.txt.
        let mut shipped = Vec::new();
        for name in &plugins.shipped {
            let Some(on_disk) = present.iter().find(|p| p.eq_ignore_ascii_case(name)) else {
                continue;
            };
            let taken = |n: &String| n.eq_ignore_ascii_case(on_disk);
            if implicit.iter().any(taken) || found.iter().any(|p| taken(&p.name)) {
                continue;
            }
            let header = RelPath::new(&format!("{}/{on_disk}", plugins.folder))
                .ok()
                .and_then(|rel| game.original(&rel))
                .and_then(|path| read_header(&path).ok())
                .unwrap_or_default();
            shipped.push(plugin_item(
                on_disk.clone(),
                String::new(),
                is_master(header.flags, on_disk),
                plugins.scale(header.flags, on_disk),
                header.masters,
            ));
        }
        found.splice(0..0, shipped);
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
            .zip(scales)
            .map(|(name, scale)| ListItem {
                can_disable: false,
                ..plugin_item(name, String::new(), true, scale, Vec::new())
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
        let has = |item: &ListItem, badge: &str| item.badges.iter().any(|b| b == badge);
        let enabled: Vec<_> = list
            .fixed
            .iter()
            .chain(list.items.iter().filter(|i| i.enabled).map(|i| &i.item))
            .collect();
        let light = enabled.iter().filter(|i| has(i, LIGHT)).count();
        let medium = enabled.iter().filter(|i| has(i, MEDIUM)).count();
        let full = enabled.len() - light - medium;
        let mut problems = Vec::new();
        if let Some(limit) = config.limit
            && full > limit
        {
            let smaller = if config.medium_flag.is_some() {
                "light or medium"
            } else {
                "light"
            };
            problems.push(Problem::warning(format!(
                "{full} plugins that aren't {smaller} are enabled, counting the game's own, but the game can only load {limit}; turn some off in the load order, or use {smaller} versions of them"
            )));
        }
        if let Some(limit) = config.medium_limit
            && medium > limit
        {
            problems.push(Problem::warning(format!(
                "{medium} medium plugins are enabled, counting the game's own, but the game can only load {limit}; turn some off in the load order, or use light versions of them"
            )));
        }
        if let Some(limit) = config.light_limit
            && light > limit
        {
            problems.push(Problem::warning(format!(
                "{light} light plugins are enabled, counting the game's own, but the game can only load {limit}; turn some off in the load order, or merge some of them"
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
    scale: Scale,
    masters: Vec<String>,
) -> ListItem {
    let mut badges = Vec::new();
    match scale {
        Scale::Light => badges.push(LIGHT.to_owned()),
        Scale::Medium => badges.push(MEDIUM.to_owned()),
        Scale::Full if master => badges.push(MASTER.to_owned()),
        Scale::Full => {}
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
