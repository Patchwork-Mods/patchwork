use globset::{GlobBuilder, GlobMatcher};
use patchwork_core::deploy::{DeployContext, Deployer, DesiredState, InstalledMod, Placement};
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};
use serde::Deserialize;

/// A data-driven deployer: each file in a mod is matched against a list of
/// rules that say where it goes. Covers any game where installing a mod means
/// putting its files in the right folders.
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
    files: String,
    mods: Option<String>,
    #[serde(default)]
    strip: usize,
    to: Option<String>,
    #[serde(default)]
    skip: bool,
}

struct CompiledRule {
    files: GlobMatcher,
    mods: Option<GlobMatcher>,
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
                .build()
                .map(|g| g.compile_matcher())
                .map_err(|e| self.error(format!("bad pattern `{pattern}`: {e}")))
        };
        Ok(CompiledRule {
            files: glob(&rule.files)?,
            mods: rule.mods.as_deref().map(glob).transpose()?,
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
    for rule in rules {
        if !rule.files.is_match(rel.as_str())
            || rule.mods.as_ref().is_some_and(|m| !m.is_match(mod_name))
        {
            continue;
        }
        let parts: Vec<_> = rel.components().collect();
        if parts.len() <= rule.strip {
            continue;
        }
        if rule.skip {
            return Ok(Outcome::Skip);
        }
        let dst = rule
            .to
            .replace("{mod}", &mod_name.replace(['/', '\\'], "_"))
            .replace("{path}", &parts[rule.strip..].join("/"))
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
mod tests {
    use super::*;

    fn bepinex() -> Vec<CompiledRule> {
        let text = include_str!("../../../registry/rulesets/bepinex.toml");
        let ruleset: Ruleset = toml::from_str(text).unwrap();
        ruleset
            .rules
            .into_iter()
            .map(|r| RulesDeployer.compile(r).unwrap())
            .collect()
    }

    fn dst(rules: &[CompiledRule], mod_name: &str, path: &str) -> Option<String> {
        match place(rules, mod_name, &RelPath::new(path).unwrap()).unwrap() {
            Outcome::Place(p) => Some(p.to_string()),
            _ => None,
        }
    }

    #[test]
    fn bepinex_layout() {
        let rules = bepinex();
        let cases = [
            ("BepInExPack/winhttp.dll", Some("winhttp.dll")),
            (
                "BepInExPack/BepInEx/core/BepInEx.dll",
                Some("BepInEx/core/BepInEx.dll"),
            ),
            ("manifest.json", None),
            ("icon.png", None),
            ("plugins/Cool.dll", Some("BepInEx/plugins/Cool/Cool.dll")),
            ("config/cool.cfg", Some("BepInEx/config/cool.cfg")),
            ("Cool.dll", Some("BepInEx/plugins/Cool/Cool.dll")),
            (
                "assets/thing.bundle",
                Some("BepInEx/plugins/Cool/assets/thing.bundle"),
            ),
            (
                "BepInEx/plugins/X/x.dll",
                Some("BepInEx/plugins/Cool/X/x.dll"),
            ),
            (
                "BepInEx/patchers/X/x.dll",
                Some("BepInEx/patchers/Cool/X/x.dll"),
            ),
            ("core/x.dll", Some("BepInEx/core/Cool/x.dll")),
            ("monomod/x.mm.dll", Some("BepInEx/monomod/Cool/x.mm.dll")),
            ("BepInEx/config/x.cfg", Some("BepInEx/config/x.cfg")),
        ];
        for (path, expected) in cases {
            assert_eq!(dst(&rules, "Cool", path).as_deref(), expected, "{path}");
        }
    }

    #[test]
    fn mod_filters_and_escapes() {
        let rule = |files: &str, mods: Option<&str>, to: &str| {
            RulesDeployer
                .compile(Rule {
                    files: files.to_owned(),
                    mods: mods.map(str::to_owned),
                    strip: 0,
                    to: Some(to.to_owned()),
                    skip: false,
                })
                .unwrap()
        };
        let rules = [rule("*.pak", Some("Big*"), "Paks/{file}")];
        assert_eq!(
            dst(&rules, "BigMod", "a.pak").as_deref(),
            Some("Paks/a.pak")
        );
        assert_eq!(dst(&rules, "Small", "a.pak"), None);

        let evil = [rule("**", None, "../{path}")];
        assert!(place(&evil, "m", &RelPath::new("x").unwrap()).is_err());
    }
}
