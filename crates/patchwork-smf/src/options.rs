//! SMF mods' options, picked the way SMF's mod manager picks them: the
//! checkboxes and selects are the user's to pick, and conditionals are
//! decided by what else is in the load order.
//!
//! What's picked is saved as SMF saves it (in its config's `modOptions`):
//! checkboxes by name and selects as `group:name`. The deployer reads it
//! from [`InstalledMod::ext`]; until anything's saved, mods get the options
//! they enable by default, as SMF gives them.

use std::collections::HashSet;
use std::fs;

use patchwork_core::deploy::{InstalledMod, ModFile};
use patchwork_core::options::{
    Answer, Form, Group, Item, ItemState, ModContext, ModOptions, ModPeek, Page, Pick,
};
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::manifest::{Manifest, ModOption, OptionKind};

/// The extension's id, which names its state in profiles.
pub const SMF_EXTENSION: &str = "smf";

const MANIFEST: &str = "manifest.json";

/// What the extension keeps for a mod.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// Kept even when empty, so a mod with everything unpicked counts as
    /// picked once it's been saved.
    #[serde(default)]
    options: Vec<String>,
}

/// The options picked for `m`, as [`ModOption::key`]s, or `None` if nothing's
/// been saved for it.
pub fn picked(m: &InstalledMod) -> Result<Option<Vec<String>>, String> {
    let Some(state) = m.ext.get(SMF_EXTENSION) else {
        return Ok(None);
    };
    let saved: Saved = toml::Value::Table(state.clone())
        .try_into()
        .map_err(|e| format!("the options saved for it can't be read: {e}"))?;
    Ok(Some(saved.options))
}

/// `m`'s manifests, in the order they're deployed.
pub fn manifests(m: &InstalledMod) -> Vec<&ModFile> {
    let mut manifests: Vec<&ModFile> = m
        .files
        .iter()
        .filter(|f| f.rel.file_name().eq_ignore_ascii_case(MANIFEST))
        .collect();
    manifests.sort_by(|a, b| a.rel.cmp(&b.rel));
    manifests
}

/// What SMF's `config.loadOrder` would have for `mods`: their manifest
/// ids, or the names of mods without one.
pub fn load_order(mods: &[InstalledMod]) -> HashSet<String> {
    let mut ids = HashSet::new();
    for m in mods {
        let manifests = manifests(m);
        if manifests.is_empty() {
            ids.insert(m.name.clone());
        }
        for file in manifests {
            // One that can't be read fails when it's deployed.
            if let Some(manifest) = fs::read_to_string(&file.src)
                .ok()
                .and_then(|text| Manifest::parse(&text).ok())
            {
                ids.insert(manifest.id);
            }
        }
    }
    ids
}

/// Options to pick for SMF mods, registered as `smf.options`.
pub struct SmfOptions;

impl ModOptions for SmfOptions {
    fn title(&self) -> &str {
        "Options"
    }

    fn applies(&self, _: &ModContext<'_>, m: &ModPeek<'_>) -> bool {
        m.read_near_top(MANIFEST)
            .and_then(|data| String::from_utf8(data).ok())
            .and_then(|text| Manifest::parse(&text).ok())
            .is_some_and(|manifest| manifest.has_choices())
    }

    fn form(
        &self,
        ctx: &ModContext<'_>,
        m: &InstalledMod,
        answers: Option<&[Answer]>,
    ) -> Result<Form> {
        let Some((root, manifest)) = read(m)? else {
            return Ok(Form {
                title: m.name.clone(),
                ..Form::default()
            });
        };
        let layout = Layout::of(&manifest);
        let saved = saved(ctx)?;
        let picked = merged(&manifest, &layout, saved.as_deref(), answers.unwrap_or(&[]))
            .map_err(|e| invalid(m, e))?;
        let defaulted = |group: &str| {
            saved.is_none() && !answers.into_iter().flatten().any(|a| a.group == group)
        };
        let load_order = load_order(ctx.mods());
        let image = |option: &ModOption| {
            let image = option.image.as_deref()?.replace('\\', "/");
            let image = image.trim_start_matches("./");
            let path = match &root {
                Some(root) => format!("{root}/{image}"),
                None => image.to_owned(),
            };
            RelPath::new(&path).ok()
        };
        let item = |option: &ModOption, state: ItemState, picked: bool| Item {
            id: option.name.clone(),
            title: option.name.clone(),
            description: option.tooltip.clone(),
            image: image(option),
            state,
            picked,
        };
        let chosen = |option: &ModOption| {
            let state = if option.enabled_by_default {
                ItemState::Recommended
            } else {
                ItemState::Optional
            };
            item(option, state, picked.contains(&option.key()))
        };

        let mut groups = Vec::new();
        let checkboxes: Vec<&ModOption> = manifest
            .options
            .iter()
            .filter(|o| o.kind == OptionKind::Checkbox)
            .collect();
        if !checkboxes.is_empty() {
            groups.push(Group {
                id: layout.checkboxes.clone(),
                title: layout.checkboxes.clone(),
                pick: Pick::Any,
                items: checkboxes.into_iter().map(chosen).collect(),
                defaulted: defaulted(&layout.checkboxes),
            });
        }
        for name in &layout.selects {
            groups.push(Group {
                id: name.clone(),
                title: name.clone(),
                pick: Pick::ExactlyOne,
                items: manifest
                    .options
                    .iter()
                    .filter(|o| o.kind == OptionKind::Select && group_of(o) == name)
                    .map(chosen)
                    .collect(),
                defaulted: defaulted(name),
            });
        }

        let mut warnings = Vec::new();
        let automatic: Vec<Item> = manifest
            .options
            .iter()
            .filter(|o| !o.pickable())
            .map(|option| {
                let on = option.on(None, &load_order).unwrap_or_else(|e| {
                    warnings.push(format!("\"{}\" is left off, since {e}", option.name));
                    false
                });
                let mut item = item(
                    option,
                    if on {
                        ItemState::Required
                    } else {
                        ItemState::NotUsable
                    },
                    on,
                );
                if let Some(condition) = &option.condition {
                    item.description = Some(match item.description {
                        Some(tooltip) => format!("{tooltip}\n\nOn when: {condition}"),
                        None => format!("On when: {condition}"),
                    });
                }
                item
            })
            .collect();
        if !automatic.is_empty() {
            groups.push(Group {
                id: layout.automatic.clone(),
                title: format!("{} (decided by the other mods enabled)", layout.automatic),
                pick: Pick::Any,
                items: automatic,
                defaulted: false,
            });
        }

        let answers = groups
            .iter()
            .map(|g| Answer {
                group: g.id.clone(),
                items: g
                    .items
                    .iter()
                    .filter(|i| i.picked)
                    .map(|i| i.id.clone())
                    .collect(),
            })
            .collect();
        Ok(Form {
            title: manifest.name.clone().unwrap_or_else(|| m.name.clone()),
            pages: vec![Page {
                title: "Options".to_owned(),
                groups,
            }],
            summary: None,
            warnings,
            answers,
        })
    }

    fn save(
        &self,
        ctx: &ModContext<'_>,
        m: &InstalledMod,
        answers: &[Answer],
    ) -> Result<toml::Table> {
        let Some((_, manifest)) = read(m)? else {
            return Err(invalid(m, "it has no manifest.json".to_owned()));
        };
        let layout = Layout::of(&manifest);
        let saved = saved(ctx)?;
        let options =
            merged(&manifest, &layout, saved.as_deref(), answers).map_err(|e| invalid(m, e))?;
        match toml::Value::try_from(Saved { options }) {
            Ok(toml::Value::Table(table)) => Ok(table),
            _ => unreachable!("Saved is a table"),
        }
    }
}

/// What a form's groups are called. Selects' are their own names; the
/// checkboxes and the conditionals get names no select has.
struct Layout {
    checkboxes: String,
    selects: Vec<String>,
    automatic: String,
}

impl Layout {
    fn of(manifest: &Manifest) -> Self {
        let mut selects: Vec<String> = Vec::new();
        for option in &manifest.options {
            if option.kind == OptionKind::Select && !selects.iter().any(|s| s == group_of(option)) {
                selects.push(group_of(option).to_owned());
            }
        }
        let free = |base: &str| {
            let mut name = base.to_owned();
            while selects.contains(&name) {
                name.push('*');
            }
            name
        };
        Self {
            checkboxes: free("Options"),
            automatic: free("Automatic"),
            selects,
        }
    }
}

fn group_of(option: &ModOption) -> &str {
    option.group.as_deref().unwrap_or_default()
}

/// The keys picked once `answers` replace what's picked in their groups:
/// what's `saved`, or the defaults.
fn merged(
    manifest: &Manifest,
    layout: &Layout,
    saved: Option<&[String]>,
    answers: &[Answer],
) -> Result<Vec<String>, String> {
    let pickable: Vec<&ModOption> = manifest.options.iter().filter(|o| o.pickable()).collect();
    let group_id = |option: &ModOption| match option.kind {
        OptionKind::Select => group_of(option).to_owned(),
        _ => layout.checkboxes.clone(),
    };
    let mut picked: Vec<String> = match saved {
        Some(saved) => saved.to_vec(),
        None => pickable
            .iter()
            .filter(|o| o.enabled_by_default)
            .map(|o| o.key())
            .collect(),
    };
    for answer in answers {
        if answer.group == layout.automatic {
            continue;
        }
        let in_group: Vec<&&ModOption> = pickable
            .iter()
            .filter(|o| group_id(o) == answer.group)
            .collect();
        if in_group.is_empty() {
            return Err(format!("it has no options \"{}\"", answer.group));
        }
        let mut chosen = Vec::new();
        for item in &answer.items {
            let option = in_group
                .iter()
                .find(|o| o.name == *item)
                .ok_or_else(|| format!("\"{}\" has no option \"{item}\"", answer.group))?;
            chosen.push(option.key());
        }
        let select = in_group.iter().any(|o| o.kind == OptionKind::Select);
        if select && chosen.len() != 1 {
            let names: Vec<&str> = in_group.iter().map(|o| o.name.as_str()).collect();
            return Err(format!(
                "pick one of \"{}\" for \"{}\"",
                names.join("\", \""),
                answer.group
            ));
        }
        let keys: Vec<String> = in_group.iter().map(|o| o.key()).collect();
        picked.retain(|k| !keys.contains(k));
        picked.extend(chosen);
    }
    Ok(picked)
}

fn saved(ctx: &ModContext<'_>) -> Result<Option<Vec<String>>> {
    Ok(ctx
        .saved()
        .then(|| ctx.state::<Saved>())
        .transpose()?
        .map(|s| s.options))
}

/// `m`'s first manifest, and the folder it's in.
fn read(m: &InstalledMod) -> Result<Option<(Option<RelPath>, Manifest)>> {
    let Some(file) = manifests(m).into_iter().next() else {
        return Ok(None);
    };
    let text = fs::read_to_string(&file.src)
        .map_err(|e| invalid(m, format!("its {} can't be read: {e}", file.rel)))?;
    let manifest = Manifest::parse(&text).map_err(|e| {
        invalid(
            m,
            format!("its {} isn't a valid SMF manifest: {e}", file.rel),
        )
    })?;
    Ok(Some((file.rel.parent(), manifest)))
}

fn invalid(m: &InstalledMod, message: String) -> Error {
    Error::invalid(format!("`{}`: {message}", m.name))
}
