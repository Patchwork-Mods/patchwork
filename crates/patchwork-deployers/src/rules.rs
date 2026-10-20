use globset::{GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder};
use patchwork_core::deploy::{DeployContext, Deployer, DesiredState, InstalledMod, Placement};
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};
use serde::Deserialize;

/// A data-driven deployer: each file in a mod is matched against a list of
/// rules that say where it goes. Covers any game where installing a mod means
/// putting its files in the right folders. Patterns ignore case, since mods
/// are rarely consistent about it.
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
/// ```
pub struct RulesDeployer;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    rules: Vec<Rule>,
    #[serde(default)]
    rulesets: Vec<String>,
    #[serde(default)]
    unmatched: Unmatched,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ruleset {
    rules: Vec<Rule>,
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
    #[serde(default = "any_file")]
    files: String,
    folder: Option<String>,
    mods: Option<Patterns>,
    #[serde(default)]
    strip: usize,
    to: Option<String>,
    #[serde(default)]
    skip: bool,
}

fn any_file() -> String {
    "**".to_owned()
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
    strip: usize,
    to: String,
    skip: bool,
}

enum Outcome {
    Place(RelPath),
    Skip,
    Unmatched,
}

impl RulesDeployer {
    fn load(&self, ctx: &DeployContext<'_>) -> Result<(Vec<CompiledRule>, Unmatched)> {
        let config: Config = toml::Value::Table(ctx.config.clone())
            .try_into()
            .map_err(|e| self.error(format!("bad config for {}: {e}", ctx.game.id)))?;
        let mut rules = config.rules;
        for id in &config.rulesets {
            let entry = ctx
                .registry
                .data("rulesets", id)
                .ok_or_else(|| self.error(format!("no ruleset named `{id}`")))?;
            let ruleset: Ruleset = toml::Value::Table(entry.value.clone())
                .try_into()
                .map_err(|e| self.error(format!("bad ruleset `{id}`: {e}")))?;
            rules.extend(ruleset.rules);
        }
        let rules = rules
            .into_iter()
            .map(|r| self.compile(r))
            .collect::<Result<_>>()?;
        Ok((rules, config.unmatched))
    }

    fn compile(&self, rule: Rule) -> Result<CompiledRule> {
        let glob = |pattern: &str| {
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .case_insensitive(true)
                .build()
                .map_err(|e| self.error(format!("bad pattern `{pattern}`: {e}")))
        };
        let mods = match rule.mods {
            None => None,
            Some(patterns) => {
                let patterns = match patterns {
                    Patterns::One(p) => vec![p],
                    Patterns::Many(ps) => ps,
                };
                let mut set = GlobSetBuilder::new();
                for pattern in &patterns {
                    set.add(glob(pattern)?);
                }
                Some(set.build().map_err(|e| self.error(e.to_string()))?)
            }
        };
        if let Some(folder) = &rule.folder
            && (folder.is_empty() || folder.contains(['/', '\\']))
        {
            return Err(self.error(format!("bad folder `{folder}`: must be a single name")));
        }
        Ok(CompiledRule {
            files: glob(&rule.files)?.compile_matcher(),
            folder: rule.folder,
            mods,
            strip: rule.strip,
            to: rule.to.unwrap_or_else(|| "{path}".to_owned()),
            skip: rule.skip,
        })
    }

    fn error(&self, message: String) -> Error {
        Error::Deployer {
            kind: self.kind().to_owned(),
            message,
        }
    }
}

fn place(rules: &[CompiledRule], mod_name: &str, rel: &RelPath) -> Result<Outcome, String> {
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

    fn plan(&self, ctx: &DeployContext<'_>, mods: &[InstalledMod]) -> Result<DesiredState> {
        let (rules, unmatched) = self.load(ctx)?;
        let mut desired = DesiredState::default();
        for m in mods {
            for file in &m.files {
                let outcome = place(&rules, &m.name, &file.rel)
                    .map_err(|e| self.error(format!("`{}`: {e}", m.name)))?;
                match outcome {
                    Outcome::Place(dst) => desired.placements.push(Placement {
                        dst,
                        src: file.src.clone(),
                        mod_name: m.name.clone(),
                    }),
                    Outcome::Skip => {}
                    Outcome::Unmatched => {
                        let message = format!("no rule for {} in `{}`", file.rel, m.name);
                        if unmatched == Unmatched::Error {
                            return Err(self.error(message));
                        }
                        desired.warnings.push(format!("{message}; skipped it"));
                    }
                }
            }
        }
        Ok(desired)
    }
}

#[cfg(test)]
mod tests;
