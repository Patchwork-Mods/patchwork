use std::collections::BTreeSet;

use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use patchwork_core::config::ConfigDef;
use patchwork_core::deploy::{
    DeployContext, Deployer, DesiredState, Dest, InstalledMod, Placement,
};
use patchwork_core::relpath::RelPath;
use patchwork_core::settings::{Choice, SettingKind, SettingSpec};
use patchwork_core::{Error, Result};
use serde::Deserialize;

/// A data-driven deployer: each file in a mod is matched against a list of
/// rules that say where it goes. Covers any game where installing a mod means
/// putting its files in the right folders. Patterns ignore case, since mods
/// are rarely consistent about it.
///
/// Mods are often packed with an extra folder around them, or sometimes
/// without the folder the game expects. If `root` is set, each mod's real top
/// is found first: the shallowest folder with a marker directly inside it.
/// Everything above that folder is stripped before the rules see the paths,
/// and files outside it are left out.
///
/// ```toml
/// [deployer]
/// kind = "rules"
/// rulesets = ["bepinex"]     # shared rules from the registry, tried after `rules`
/// unmatched = "warn"         # or "error"
///
/// [[deployer.rules]]
/// files = "*.pak"
/// to = "Content/Paks/~mods/{file}"
///
/// [[deployer.rules]]
/// folder = "plugins"         # a folder with this name, at any depth
/// to = "BepInEx/plugins/{mod}/{path}"
///
/// [[deployer.rules]]
/// kinds = "shader"           # mods whose source says they're this kind
/// to = "shaderpacks/{file}"
///
/// [[deployer.rules]]
/// sources = "modio"          # mods from these sources
/// to = "Mods/{mod}/{path}"
///
/// [[deployer.rules]]
/// ruleset = "bepinex"        # a ruleset's rules, at this point in the list
///
/// [deployer.root]
/// markers = ["Data", "meshes", "*.esp"]  # folder or file names
/// unless = ["fomod/ModuleConfig.xml"]    # mods with these are left as they are
/// ```
///
/// A ruleset can also say which files are config, with `[[config]]` as in a
/// game definition (see [`patchwork_core::config::ConfigDef`]); games that
/// use it get those too.
pub struct RulesDeployer;

const UNMATCHED: SettingSpec = SettingSpec::new(
    "unmatched",
    "Files no rule matches",
    SettingKind::Choice {
        choices: &[
            Choice {
                value: "warn",
                label: "Leave them out and warn",
            },
            Choice {
                value: "error",
                label: "Stop the deploy",
            },
        ],
    },
)
.help("What to do with files in mods that the game's rules don't say where to put.");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    rules: Vec<Rule>,
    #[serde(default)]
    rulesets: Vec<String>,
    #[serde(default)]
    unmatched: Unmatched,
    root: Option<RootConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RootConfig {
    /// Folder names or file globs that a mod's top folder directly contains.
    markers: Vec<String>,
    /// Globs for paths from the archive's top. Mods with any of them are
    /// left exactly as they are.
    #[serde(default)]
    unless: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ruleset {
    #[serde(default)]
    rules: Vec<Rule>,
    /// Which files are config; see [`patchwork_core::config`].
    #[serde(default)]
    config: Vec<ConfigDef>,
}

#[derive(Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Unmatched {
    #[default]
    Warn,
    Error,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    /// Includes a ruleset's rules in place of this one, which can't then set
    /// anything else.
    ruleset: Option<String>,
    files: Option<String>,
    folder: Option<String>,
    mods: Option<Patterns>,
    /// Globs for the kind of mod its source says it is. Mods whose source
    /// doesn't say never match.
    kinds: Option<Patterns>,
    /// Globs for the id of the source the mod came from, like `modio`, for
    /// games whose sources package mods differently.
    sources: Option<Patterns>,
    #[serde(default)]
    strip: usize,
    to: Option<String>,
    #[serde(default)]
    skip: bool,
}

/// One glob, or a list of them.
#[derive(Deserialize)]
#[serde(untagged)]
enum Patterns {
    One(String),
    Many(Vec<String>),
}

struct CompiledRule {
    files: GlobMatcher,
    folder: Option<String>,
    mods: Option<GlobSet>,
    kinds: Option<GlobSet>,
    sources: Option<GlobSet>,
    strip: usize,
    to: String,
    skip: bool,
}

struct CompiledRoot {
    markers: GlobSet,
    unless: GlobSet,
}

/// Everything the game's config says, ready to plan with.
struct Setup {
    rules: Vec<CompiledRule>,
    root: Option<CompiledRoot>,
    unmatched: Unmatched,
}

enum Outcome {
    Place(RelPath),
    Skip,
    Unmatched,
}

impl RulesDeployer {
    fn load(&self, ctx: &DeployContext<'_>) -> Result<Setup> {
        let config: Config = toml::Value::Table(ctx.config.clone())
            .try_into()
            .map_err(|e| self.error(format!("bad config for {}: {e}", ctx.game.id)))?;
        let lookup = |id: &str| ctx.registry.data("rulesets", id).map(|e| &e.value);
        self.setup(config, &lookup)
    }

    fn setup<'a>(
        &self,
        config: Config,
        lookup: &impl Fn(&str) -> Option<&'a toml::Table>,
    ) -> Result<Setup> {
        let mut rules = Vec::new();
        self.expand(lookup, config.rules, &mut rules, 0)?;
        for id in config.rulesets {
            self.include(lookup, &id, &mut rules, 0)?;
        }
        let root = config.root.map(|r| self.compile_root(r)).transpose()?;
        Ok(Setup {
            rules,
            root,
            unmatched: config.unmatched,
        })
    }

    /// Compiles `rules` onto `out`, replacing includes with what they include.
    fn expand<'a>(
        &self,
        lookup: &impl Fn(&str) -> Option<&'a toml::Table>,
        rules: Vec<Rule>,
        out: &mut Vec<CompiledRule>,
        depth: usize,
    ) -> Result<()> {
        for rule in rules {
            match &rule.ruleset {
                Some(id) => {
                    let alone = rule.files.is_none()
                        && rule.folder.is_none()
                        && rule.mods.is_none()
                        && rule.kinds.is_none()
                        && rule.sources.is_none()
                        && rule.strip == 0
                        && rule.to.is_none()
                        && !rule.skip;
                    if !alone {
                        return Err(self.error(format!(
                            "a rule that includes `{id}` can't set anything else"
                        )));
                    }
                    self.include(lookup, id, out, depth + 1)?;
                }
                None => out.push(self.compile(rule)?),
            }
        }
        Ok(())
    }

    /// The `[[config]]` of every ruleset `config` uses, directly or through
    /// another, in the order their rules are tried.
    fn config_defs<'a>(
        &self,
        lookup: &impl Fn(&str) -> Option<&'a toml::Table>,
        config: &Config,
    ) -> Result<Vec<ConfigDef>> {
        let mut defs = Vec::new();
        let included = config.rules.iter().filter_map(|r| r.ruleset.as_deref());
        for id in included.chain(config.rulesets.iter().map(String::as_str)) {
            self.ruleset_config(lookup, id, &mut defs, 0)?;
        }
        Ok(defs)
    }

    fn ruleset_config<'a>(
        &self,
        lookup: &impl Fn(&str) -> Option<&'a toml::Table>,
        id: &str,
        out: &mut Vec<ConfigDef>,
        depth: usize,
    ) -> Result<()> {
        if depth > 8 {
            return Err(self.error(format!("rulesets nest too deeply at `{id}`")));
        }
        let table = lookup(id).ok_or_else(|| self.error(format!("no ruleset named `{id}`")))?;
        let ruleset: Ruleset = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e| self.error(format!("bad ruleset `{id}`: {e}")))?;
        for rule in &ruleset.rules {
            if let Some(id) = &rule.ruleset {
                self.ruleset_config(lookup, id, out, depth + 1)?;
            }
        }
        for def in ruleset.config {
            if !out.contains(&def) {
                out.push(def);
            }
        }
        Ok(())
    }

    fn include<'a>(
        &self,
        lookup: &impl Fn(&str) -> Option<&'a toml::Table>,
        id: &str,
        out: &mut Vec<CompiledRule>,
        depth: usize,
    ) -> Result<()> {
        // Deep enough for any sensible nesting, and stops include cycles.
        if depth > 8 {
            return Err(self.error(format!("rulesets nest too deeply at `{id}`")));
        }
        let table = lookup(id).ok_or_else(|| self.error(format!("no ruleset named `{id}`")))?;
        let ruleset: Ruleset = toml::Value::Table(table.clone())
            .try_into()
            .map_err(|e| self.error(format!("bad ruleset `{id}`: {e}")))?;
        self.expand(lookup, ruleset.rules, out, depth)
    }

    fn glob(&self, pattern: &str) -> Result<Glob> {
        GlobBuilder::new(pattern)
            .literal_separator(true)
            .case_insensitive(true)
            .build()
            .map_err(|e| self.error(format!("bad pattern `{pattern}`: {e}")))
    }

    fn glob_set(&self, patterns: &[String]) -> Result<GlobSet> {
        let mut set = GlobSetBuilder::new();
        for pattern in patterns {
            set.add(self.glob(pattern)?);
        }
        set.build().map_err(|e| self.error(e.to_string()))
    }

    fn compile_root(&self, root: RootConfig) -> Result<CompiledRoot> {
        if root.markers.is_empty() {
            return Err(self.error("`root` needs at least one marker".to_owned()));
        }
        Ok(CompiledRoot {
            markers: self.glob_set(&root.markers)?,
            unless: self.glob_set(&root.unless)?,
        })
    }

    fn patterns(&self, patterns: Option<Patterns>) -> Result<Option<GlobSet>> {
        match patterns {
            None => Ok(None),
            Some(Patterns::One(p)) => self.glob_set(&[p]).map(Some),
            Some(Patterns::Many(ps)) => self.glob_set(&ps).map(Some),
        }
    }

    fn compile(&self, rule: Rule) -> Result<CompiledRule> {
        let mods = self.patterns(rule.mods)?;
        let kinds = self.patterns(rule.kinds)?;
        let sources = self.patterns(rule.sources)?;
        if let Some(folder) = &rule.folder
            && (folder.is_empty() || folder.contains(['/', '\\']))
        {
            return Err(self.error(format!("bad folder `{folder}`: must be a single name")));
        }
        Ok(CompiledRule {
            files: self
                .glob(rule.files.as_deref().unwrap_or("**"))?
                .compile_matcher(),
            folder: rule.folder,
            mods,
            kinds,
            sources,
            strip: rule.strip,
            to: rule.to.unwrap_or_else(|| "{path}".to_owned()),
            skip: rule.skip,
        })
    }

    /// Adds where `m`'s files go to `desired`.
    fn plan_mod(&self, setup: &Setup, m: &InstalledMod, desired: &mut DesiredState) -> Result<()> {
        let top = match &setup.root {
            Some(root) => root.find(m, &mut desired.warnings),
            None => Vec::new(),
        };
        let mut outside = 0;
        for file in &m.files {
            let rel = if top.is_empty() {
                file.rel.clone()
            } else {
                let parts: Vec<_> = file.rel.components().collect();
                let inside = parts.len() > top.len()
                    && parts.iter().zip(&top).all(|(p, t)| p.to_lowercase() == *t);
                if !inside {
                    outside += 1;
                    continue;
                }
                RelPath::new(&parts[top.len()..].join("/")).map_err(|e| self.error(e))?
            };
            let outcome = place(&setup.rules, &Who::of(m), &rel)
                .map_err(|e| self.error(format!("`{}`: {e}", m.name)))?;
            match outcome {
                Outcome::Place(dst) => desired.placements.push(Placement {
                    dst: Dest::game(dst),
                    src: m.src(file),
                    mod_name: m.name.clone(),
                }),
                Outcome::Skip => {}
                Outcome::Unmatched => {
                    let message = format!("no rule for {} in `{}`", file.rel, m.name);
                    if setup.unmatched == Unmatched::Error {
                        return Err(self.error(message));
                    }
                    desired.warnings.push(format!("{message}; skipped it"));
                }
            }
        }
        if outside > 0 {
            desired.warnings.push(format!(
                "left out {outside} file(s) of `{}` that aren't inside its top folder `{}`",
                m.name,
                top.join("/")
            ));
        }
        Ok(())
    }

    fn error(&self, message: String) -> Error {
        Error::Deployer {
            kind: self.kind().to_owned(),
            message,
        }
    }
}

impl CompiledRoot {
    /// The folders to strip from the front of `m`'s paths, lowercased: the
    /// shallowest folder with a marker directly inside it. None if the mod
    /// is already laid out from its top, if nothing marks it, if several
    /// folders could be the top, or if it has a file matching `unless`.
    fn find(&self, m: &InstalledMod, warnings: &mut Vec<String>) -> Vec<String> {
        if let Some(file) = m
            .files
            .iter()
            .find(|f| self.unless.is_match(f.rel.as_str()))
        {
            warnings.push(format!(
                "`{}` has {}, which patchwork can't handle yet; deployed its files as packed",
                m.name, file.rel
            ));
            return Vec::new();
        }
        // Each folder with a marker directly inside, shallowest first. Names
        // are lowercased so differently cased copies of a folder count once.
        let mut found = BTreeSet::new();
        for file in &m.files {
            let parts: Vec<_> = file.rel.components().collect();
            for (depth, name) in parts.iter().enumerate() {
                if self.markers.is_match(name) {
                    let folder: Vec<_> = parts[..depth].iter().map(|p| p.to_lowercase()).collect();
                    found.insert((depth, folder));
                }
            }
        }
        let Some((depth, _)) = found.first() else {
            return Vec::new();
        };
        let mut tops: Vec<_> = found
            .iter()
            .take_while(|(d, _)| d == depth)
            .map(|(_, folder)| folder.clone())
            .collect();
        if tops.len() > 1 {
            let names: Vec<_> = tops.iter().map(|t| format!("`{}`", t.join("/"))).collect();
            warnings.push(format!(
                "`{}` could start at any of {}; deployed its files as packed",
                m.name,
                names.join(", ")
            ));
            return Vec::new();
        }
        tops.pop().unwrap_or_default()
    }
}

/// What rules can match a mod by, besides its files.
struct Who<'a> {
    name: &'a str,
    /// What its source says it is, if it says.
    kind: Option<&'a str>,
    /// Its source's id.
    source: &'a str,
}

impl<'a> Who<'a> {
    fn of(m: &'a InstalledMod) -> Self {
        Who {
            name: &m.name,
            kind: m.kind.as_deref(),
            source: &m.source,
        }
    }
}

/// Where the file at `rel` in the mod `who` goes.
fn place(rules: &[CompiledRule], who: &Who<'_>, rel: &RelPath) -> Result<Outcome, String> {
    let mod_name = who.name;
    let parts: Vec<_> = rel.components().collect();
    // Like r2modman, the outermost folder that any rule names claims the
    // file, so `config/plugins/x.cfg` is config even if plugins comes first.
    let dirs = &parts[..parts.len() - 1];
    let named = dirs.iter().position(|dir| {
        rules
            .iter()
            .filter_map(|r| r.folder.as_deref())
            .any(|f| f.eq_ignore_ascii_case(dir))
    });
    for rule in rules {
        if !rule.files.is_match(rel.as_str())
            || rule.mods.as_ref().is_some_and(|m| !m.is_match(mod_name))
            || rule
                .kinds
                .as_ref()
                .is_some_and(|k| !who.kind.is_some_and(|kind| k.is_match(kind)))
            || rule
                .sources
                .as_ref()
                .is_some_and(|s| !s.is_match(who.source))
        {
            continue;
        }
        let rest = match &rule.folder {
            None => &parts[..],
            Some(folder) => match named {
                Some(i) if parts[i].eq_ignore_ascii_case(folder) => &parts[i + 1..],
                _ => continue,
            },
        };
        if rest.len() <= rule.strip {
            continue;
        }
        if rule.skip {
            return Ok(Outcome::Skip);
        }
        let dst = rule
            .to
            .replace("{mod}", &mod_name.replace(['/', '\\'], "_"))
            .replace("{path}", &rest[rule.strip..].join("/"))
            .replace("{file}", rel.file_name());
        return RelPath::new(&dst).map(Outcome::Place);
    }
    Ok(Outcome::Unmatched)
}

impl Deployer for RulesDeployer {
    fn kind(&self) -> &str {
        "rules"
    }

    fn settings(&self) -> &[SettingSpec] {
        &[UNMATCHED]
    }

    fn config_files(&self, ctx: &DeployContext<'_>) -> Result<Vec<ConfigDef>> {
        let config: Config = toml::Value::Table(ctx.config.clone())
            .try_into()
            .map_err(|e| self.error(format!("bad config for {}: {e}", ctx.game.id)))?;
        let lookup = |id: &str| ctx.registry.data("rulesets", id).map(|e| &e.value);
        self.config_defs(&lookup, &config)
    }

    fn plan(&self, ctx: &DeployContext<'_>, mods: &[InstalledMod]) -> Result<DesiredState> {
        let setup = self.load(ctx)?;
        let mut desired = DesiredState::default();
        for m in mods {
            self.plan_mod(&setup, m, &mut desired)?;
        }
        Ok(desired)
    }
}

#[cfg(test)]
mod tests;
