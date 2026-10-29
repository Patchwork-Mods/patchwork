//! FOMOD installers as an extension's hooks: a transform that installs
//! what the picks say, and options to pick them in.

use std::cell::OnceCell;
use std::collections::HashMap;
use std::fs;

use patchwork_core::deploy::InstalledMod;
use patchwork_core::options::{
    Answer, Form, Group, Item, ModContext, ModOptions, ModPeek, ModTransform, Page, Transformed,
};
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};
use serde::{Deserialize, Serialize};

use super::{FileState, Installer, Resolved, Selection, find};

/// The extension's id, which names its state in profiles.
pub const FOMOD_EXTENSION: &str = "fomod";

/// Where installers are found, from [`find`]'s point of view.
const CONFIG: &str = "fomod/ModuleConfig.xml";

/// Both the transform and the options, registered as `fomod.installer`.
pub struct FomodInstaller;

/// What the extension keeps for a mod.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// Kept even when empty, so an installer with nothing to pick counts
    /// as picked once it's been saved.
    #[serde(default)]
    picks: Vec<Selection>,
}

/// The game's `[ext.fomod.installer]`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    /// The folder in the game's that installers' destinations are
    /// relative to.
    #[serde(default)]
    folder: Option<RelPath>,
}

impl ModTransform for FomodInstaller {
    fn applies(&self, _: &ModContext<'_>, m: &ModPeek<'_>) -> bool {
        m.has_near_top(CONFIG)
    }

    fn transform(&self, ctx: &ModContext<'_>, m: &InstalledMod) -> Result<Transformed> {
        let config = config(ctx)?;
        let Some(installer) = read(m)? else {
            return Ok(as_packed(m));
        };
        let saved: Option<Saved> = ctx.saved().then(|| ctx.state()).transpose()?;
        let resolved = resolve(
            ctx,
            &config,
            &installer,
            m,
            saved.as_ref().map(|s| &s.picks[..]),
        );
        let mut warnings = Vec::new();
        if saved.is_none() {
            warnings.push(format!(
                "`{}` has an installer whose options haven't been picked, so it gets the ones it recommends; pick them with its Installer button, or `patchwork options {}`",
                m.name, m.name
            ));
        }
        warnings.extend(
            resolved
                .warnings
                .iter()
                .map(|w| format!("`{}`: {w}", m.name)),
        );
        let files = resolved
            .files
            .into_iter()
            .filter_map(|(dst, from)| {
                let dst = match &config.folder {
                    Some(folder) => RelPath::new(&format!("{folder}/{dst}")).ok()?,
                    None => dst,
                };
                Some((dst, from))
            })
            .collect();
        Ok(Transformed { files, warnings })
    }
}

impl ModOptions for FomodInstaller {
    fn title(&self) -> &str {
        "Installer"
    }

    fn applies(&self, _: &ModContext<'_>, m: &ModPeek<'_>) -> bool {
        m.has_near_top(CONFIG)
    }

    fn form(
        &self,
        ctx: &ModContext<'_>,
        m: &InstalledMod,
        answers: Option<&[Answer]>,
    ) -> Result<Form> {
        let config = config(ctx)?;
        let Some(installer) = read(m)? else {
            return Ok(Form {
                title: m.name.clone(),
                ..Form::default()
            });
        };
        let picks = match answers {
            Some(answers) => answers.iter().map(selection).collect(),
            None => ctx.state::<Saved>()?.picks,
        };
        let resolved = resolve(ctx, &config, &installer, m, Some(&picks));
        let files = resolved.files.len();
        Ok(Form {
            title: if installer.name.is_empty() {
                m.name.clone()
            } else {
                installer.name.clone()
            },
            pages: resolved
                .steps
                .into_iter()
                .map(|step| Page {
                    groups: step
                        .groups
                        .into_iter()
                        .map(|g| Group {
                            id: group_id(&step.name, &g.name),
                            title: g.name,
                            pick: g.kind,
                            items: g
                                .options
                                .into_iter()
                                .map(|o| Item {
                                    id: o.name.clone(),
                                    title: o.name,
                                    description: Some(o.description)
                                        .filter(|d| !d.trim().is_empty()),
                                    image: o.image,
                                    state: o.kind,
                                    picked: o.picked,
                                })
                                .collect(),
                            defaulted: g.defaulted,
                        })
                        .collect(),
                    title: step.name,
                })
                .collect(),
            summary: Some(format!(
                "Installs {files} file{}.",
                if files == 1 { "" } else { "s" }
            )),
            warnings: resolved.warnings,
            answers: resolved
                .selections
                .into_iter()
                .map(|s| Answer {
                    group: group_id(&s.step, &s.group),
                    items: s.options,
                })
                .collect(),
        })
    }

    fn save(
        &self,
        ctx: &ModContext<'_>,
        m: &InstalledMod,
        answers: &[Answer],
    ) -> Result<toml::Table> {
        // Through the installer, so what's saved is what it'd make of them.
        let form = self.form(ctx, m, Some(answers))?;
        let saved = Saved {
            picks: form.answers.iter().map(selection).collect(),
        };
        toml::Table::try_from(&saved).map_err(|e| error(format!("can't save the picks: {e}")))
    }
}

fn config(ctx: &ModContext<'_>) -> Result<Config> {
    toml::Value::Table(ctx.config().clone())
        .try_into()
        .map_err(|e| error(format!("`[ext.fomod.installer]` isn't right: {e}")))
}

/// `m`'s installer, if it has one.
fn read(m: &InstalledMod) -> Result<Option<Installer>> {
    let Some((config, base)) = find(&m.files) else {
        return Ok(None);
    };
    fs::read(&config.src)
        .map_err(|e| e.to_string())
        .and_then(|bytes| Installer::parse(&bytes, base))
        .map(Some)
        .map_err(|why| error(format!("its installer couldn't be read: {why}")))
}

/// `m`'s files, untouched.
fn as_packed(m: &InstalledMod) -> Transformed {
    Transformed {
        files: m
            .files
            .iter()
            .map(|f| (f.rel.clone(), f.rel.clone()))
            .collect(),
        warnings: Vec::new(),
    }
}

/// `installer` worked out for `picks`, or the defaults if `None`, with
/// files its conditions ask about looked for in the profile's mods and the
/// game's own, only if they do ask.
fn resolve(
    ctx: &ModContext<'_>,
    config: &Config,
    installer: &Installer,
    m: &InstalledMod,
    picks: Option<&[Selection]>,
) -> Resolved {
    let present = OnceCell::new();
    let state = |f: &str| {
        present
            .get_or_init(|| Present::new(ctx, config.folder.as_ref()))
            .state(f)
    };
    let rels: Vec<RelPath> = m.files.iter().map(|f| f.rel.clone()).collect();
    installer.resolve(&rels, picks.unwrap_or_default(), &state)
}

/// Groups are told apart by step and name, with `/` between them, and `\`
/// before any `/` or `\` in the names themselves.
fn group_id(step: &str, group: &str) -> String {
    let escape = |s: &str| s.replace('\\', "\\\\").replace('/', "\\/");
    format!("{}/{}", escape(step), escape(group))
}

fn selection(answer: &Answer) -> Selection {
    let mut parts = vec![String::new()];
    let mut chars = answer.group.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => parts.last_mut().unwrap().extend(chars.next()),
            '/' => parts.push(String::new()),
            c => parts.last_mut().unwrap().push(c),
        }
    }
    let group = parts.pop().unwrap_or_default();
    Selection {
        step: parts.join("/"),
        group,
        options: answer.items.clone(),
    }
}

fn error(message: String) -> Error {
    Error::Extension {
        id: FOMOD_EXTENSION.to_owned(),
        message,
    }
}

/// The files that installers' conditions can ask about: the mods',
/// wherever they're packed in them, and the game's own.
struct Present<'a> {
    /// Each mod file's path, lowercased, by its lowercased name.
    by_name: HashMap<String, Vec<String>>,
    ctx: &'a ModContext<'a>,
    folder: Option<&'a RelPath>,
}

impl<'a> Present<'a> {
    fn new(ctx: &'a ModContext<'a>, folder: Option<&'a RelPath>) -> Self {
        let mut by_name: HashMap<String, Vec<String>> = HashMap::new();
        for f in ctx.mods().iter().flat_map(|m| &m.files) {
            by_name
                .entry(f.rel.file_name().to_lowercase())
                .or_default()
                .push(f.rel.as_str().to_lowercase());
        }
        Self {
            by_name,
            ctx,
            folder,
        }
    }

    /// Whether `file` (relative to where installers install to) is there.
    /// Patchwork can't tell whether one that is gets loaded, so it's taken
    /// to be.
    fn state(&self, file: &str) -> FileState {
        let lower = file.to_lowercase();
        let name = lower.rsplit('/').next().unwrap_or_default();
        let in_mods = self.by_name.get(name).is_some_and(|paths| {
            paths
                .iter()
                .any(|p| *p == lower || p.ends_with(&format!("/{lower}")))
        });
        let in_game = || {
            let Some(files) = self.ctx.files() else {
                return false;
            };
            [file, lower.as_str()].iter().any(|f| {
                let rel = match self.folder {
                    Some(folder) => RelPath::new(&format!("{folder}/{f}")),
                    None => RelPath::new(f),
                };
                rel.is_ok_and(|rel| files.original(&rel).is_some())
            })
        };
        if in_mods || in_game() {
            FileState::Active
        } else {
            FileState::Missing
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_ids_give_back_their_step_and_group() {
        for (step, group) in [("Main", "Look"), ("A/B", "C\\D"), ("", "/"), ("x\\", "")] {
            let answer = Answer {
                group: group_id(step, group),
                items: vec!["one".to_owned()],
            };
            let back = selection(&answer);
            assert_eq!((back.step.as_str(), back.group.as_str()), (step, group));
            assert_eq!(back.options, ["one"]);
        }
    }
}
