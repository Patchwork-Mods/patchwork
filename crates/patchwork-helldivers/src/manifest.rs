//! The `manifest.json` that mods made for the community's HELLDIVERS 2 mod
//! manager have, which says what to choose between: a transform that keeps
//! the patches picked, and options to pick them in, both registered as
//! `helldivers.manifest`.
//!
//! A manifest lists options. In the current format (`"Version": 1`) each
//! is an object that can be turned off, and can have sub-options of which
//! exactly one is used; each of those `Include`s folders. In the older
//! format (no `Version`), options are just folder names, of which exactly
//! one is used. Either way only patches directly in the included folders
//! are deployed, in the order the manifest lists them. Without any options,
//! the patches next to the manifest are.
//!
//! The transform puts the picked patches next to each other, numbered in
//! that order, so the deployer sees a mod with no variants left in it.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;

use patchwork_core::deploy::{InstalledMod, ModFile};
use patchwork_core::options::{
    Answer, Form, Group, GroupKind, Item, ItemState, ModContext, ModOptions, ModPeek, ModTransform,
    Page, Pick, Transformed,
};
use patchwork_core::relpath::RelPath;
use patchwork_core::{Error, Result};
use serde::{Deserialize, Serialize};

use crate::{PARTS, PatchName};

const MANIFEST: &str = "manifest.json";

/// The group that turns options on and off.
const OPTIONS: &str = "options";
/// The group of an older manifest's options, of which one is picked.
const VARIANT: &str = "variant";

pub struct Manifest;

/// What the extension keeps for a mod.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Saved {
    /// Options turned off, by name. Kept as the ones that are off, so
    /// options a newer version of the mod adds are on.
    #[serde(default)]
    off: Vec<String>,
    /// The sub-option picked for each option, by name, and the folder
    /// picked from an older manifest's under "".
    #[serde(default)]
    picks: BTreeMap<String, String>,
}

/// A manifest, as read.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Raw {
    #[serde(default)]
    version: Option<u32>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    options: Vec<RawOption>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawOption {
    Folder(String),
    Full(Choice),
}

/// Something to pick: an option or a sub-option.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Choice {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    include: Vec<String>,
    #[serde(default)]
    sub_options: Vec<Choice>,
}

/// What a manifest offers.
#[derive(Debug)]
enum Layout {
    /// No options: the patches next to the manifest.
    Plain,
    /// The older format: one of these folders.
    OneOf(Vec<Choice>),
    /// Options to turn on and off, each with sub-options to pick one of.
    Options(Vec<Choice>),
}

struct Read {
    /// The folder the manifest is in, which its paths are relative to.
    base: String,
    name: String,
    layout: Layout,
}

impl Read {
    fn from(m: &InstalledMod) -> Result<Option<Self>> {
        let Some(file) = find(&m.files) else {
            return Ok(None);
        };
        let bytes = fs::read(&file.src).map_err(|e| error(format!("{}: {e}", file.rel)))?;
        let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
        let raw: Raw = serde_json::from_slice(bytes)
            .map_err(|e| error(format!("its manifest.json can't be read: {e}")))?;
        let one_of = raw.version.is_none()
            && raw
                .options
                .iter()
                .all(|o| matches!(o, RawOption::Folder(_)));
        let choices: Vec<Choice> = raw
            .options
            .into_iter()
            .map(|o| match o {
                RawOption::Folder(folder) => Choice {
                    name: folder.clone(),
                    include: vec![folder],
                    ..Choice::default()
                },
                RawOption::Full(choice) => choice,
            })
            .collect();
        let layout = if choices.is_empty() {
            Layout::Plain
        } else if one_of {
            Layout::OneOf(choices)
        } else {
            Layout::Options(choices)
        };
        let base = file.rel.parent().map(|p| p.to_string()).unwrap_or_default();
        Ok(Some(Self {
            base,
            name: raw.name,
            layout,
        }))
    }

    /// What's picked: what's saved, with `answers` over it.
    fn picked(&self, saved: Saved, answers: Option<&[Answer]>) -> Saved {
        let mut picked = saved;
        let Some(answers) = answers else {
            return picked;
        };
        for answer in answers {
            if answer.group == OPTIONS {
                if let Layout::Options(options) = &self.layout {
                    picked.off = options
                        .iter()
                        .map(|o| o.name.clone())
                        .filter(|name| !answer.items.contains(name))
                        .collect();
                }
            } else if answer.group == VARIANT {
                if let Some(item) = answer.items.first() {
                    picked.picks.insert(String::new(), item.clone());
                }
            } else if let Some(option) = answer.group.strip_prefix("option:")
                && let Some(item) = answer.items.first()
            {
                picked.picks.insert(option.to_owned(), item.clone());
            }
        }
        picked
    }

    /// The options that are on, each with the sub-option picked for it, if
    /// it has any.
    fn chosen<'a>(&'a self, picked: &Saved) -> Vec<(&'a Choice, Option<&'a Choice>)> {
        let pick = |key: &str, of: &'a [Choice]| {
            picked
                .picks
                .get(key)
                .and_then(|name| of.iter().find(|c| c.name == *name))
                .or(of.first())
        };
        match &self.layout {
            Layout::Plain => Vec::new(),
            Layout::OneOf(folders) => pick("", folders).map(|c| (c, None)).into_iter().collect(),
            Layout::Options(options) => options
                .iter()
                .filter(|o| !picked.off.contains(&o.name))
                .map(|o| (o, pick(&o.name, &o.sub_options)))
                .collect(),
        }
    }

    /// The folders `picked` includes, in order, lowercased and relative to
    /// the mod.
    fn folders(&self, picked: &Saved) -> Vec<String> {
        let folders: Vec<&String> = match &self.layout {
            Layout::Plain => return vec![self.base.to_lowercase()],
            _ => self
                .chosen(picked)
                .into_iter()
                .flat_map(|(o, sub)| {
                    o.include
                        .iter()
                        .chain(sub.into_iter().flat_map(|s| &s.include))
                })
                .collect(),
        };
        let mut seen = HashSet::new();
        folders
            .into_iter()
            .map(|f| self.under_base(f).to_lowercase())
            .filter(|f| seen.insert(f.clone()))
            .collect()
    }

    /// `path`, a path in the manifest, relative to the mod.
    fn under_base(&self, path: &str) -> String {
        let path = path.replace('\\', "/");
        let path = path.trim_matches('/');
        match (self.base.is_empty(), path.is_empty()) {
            (true, _) => path.to_owned(),
            (false, true) => self.base.clone(),
            (false, false) => format!("{}/{path}", self.base),
        }
    }

    /// The picture at `path`, a path in the manifest, as the mod has it
    /// (manifests don't always get the case right).
    fn image(&self, m: &InstalledMod, path: Option<&str>) -> Option<RelPath> {
        let want = self.under_base(path?).to_lowercase();
        m.files
            .iter()
            .find(|f| f.rel.as_str().to_lowercase() == want)
            .map(|f| f.rel.clone())
    }
}

/// The mod's manifest: the one nearest its top, at the top or in a folder
/// at the top.
fn find(files: &[ModFile]) -> Option<&ModFile> {
    files
        .iter()
        .filter(|f| f.rel.file_name().eq_ignore_ascii_case(MANIFEST))
        .filter(|f| f.rel.as_str().matches('/').count() <= 1)
        .min_by_key(|f| f.rel.as_str().matches('/').count())
}

/// `m`'s patches in `folders` (lowercased), renumbered so they're one after
/// another for each archive, in the order of `folders`, as `(destination,
/// file in the mod)`.
fn renumbered(m: &InstalledMod, folders: &[String]) -> Vec<(RelPath, RelPath)> {
    let mut next: BTreeMap<String, u32> = BTreeMap::new();
    let mut files = Vec::new();
    for folder in folders {
        // Each patch's parts, by archive and number, so they stay together.
        let mut patches: BTreeMap<(String, u32), [Option<&RelPath>; 3]> = BTreeMap::new();
        for f in &m.files {
            let parent = f.rel.parent().map(|p| p.as_str().to_lowercase());
            if parent.as_deref().unwrap_or("") != folder {
                continue;
            }
            if let Some(name) = PatchName::parse(f.rel.file_name()) {
                patches.entry((name.archive, name.index)).or_default()[name.part]
                    .get_or_insert(&f.rel);
            }
        }
        for ((archive, _), parts) in patches {
            let n = next.entry(archive.clone()).or_insert(0);
            for (suffix, rel) in PARTS.iter().zip(parts) {
                if let Some(rel) = rel
                    && let Ok(dst) = RelPath::new(&format!("{archive}.patch_{n}{suffix}"))
                {
                    files.push((dst, rel.clone()));
                }
            }
            *n += 1;
        }
    }
    files
}

impl ModTransform for Manifest {
    fn applies(&self, _: &ModContext<'_>, m: &ModPeek<'_>) -> bool {
        m.has_near_top(MANIFEST)
    }

    fn transform(&self, ctx: &ModContext<'_>, m: &InstalledMod) -> Result<Transformed> {
        let Some(read) = Read::from(m)? else {
            return Ok(as_packed(m));
        };
        let picked = read.picked(ctx.state()?, None);
        Ok(Transformed {
            files: renumbered(m, &read.folders(&picked)),
            warnings: Vec::new(),
        })
    }
}

impl ModOptions for Manifest {
    fn title(&self) -> &str {
        "Options"
    }

    fn applies(&self, _: &ModContext<'_>, m: &ModPeek<'_>) -> bool {
        // Only manifests with something to pick.
        m.read_near_top(MANIFEST).is_some_and(|bytes| {
            let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes);
            serde_json::from_slice::<Raw>(bytes).is_ok_and(|raw| !raw.options.is_empty())
        })
    }

    fn form(
        &self,
        ctx: &ModContext<'_>,
        m: &InstalledMod,
        answers: Option<&[Answer]>,
    ) -> Result<Form> {
        let Some(read) = Read::from(m)? else {
            return Ok(Form {
                title: m.name.clone(),
                ..Form::default()
            });
        };
        let saved = ctx.saved();
        let picked = read.picked(ctx.state()?, answers);
        let chosen = read.chosen(&picked);
        let item = |c: &Choice, picked: bool| Item {
            id: c.name.clone(),
            title: c.name.clone(),
            description: c.description.clone().filter(|d| !d.trim().is_empty()),
            image: read.image(m, c.image.as_deref()),
            state: ItemState::Optional,
            picked,
        };
        let mut groups = Vec::new();
        let mut answered = Vec::new();
        match &read.layout {
            Layout::Plain => {}
            Layout::OneOf(folders) => {
                let pick = chosen.first().map(|(c, _)| c.name.clone());
                groups.push(Group {
                    id: VARIANT.to_owned(),
                    title: "Variant".to_owned(),
                    kind: GroupKind::Pick {
                        pick: Pick::ExactlyOne,
                        items: folders
                            .iter()
                            .map(|c| item(c, Some(&c.name) == pick.as_ref()))
                            .collect(),
                    },
                    defaulted: !saved,
                });
                answered.push(Answer::picks(VARIANT, pick.into_iter().collect()));
            }
            Layout::Options(options) => {
                let on: BTreeSet<&str> = chosen.iter().map(|(o, _)| o.name.as_str()).collect();
                groups.push(Group {
                    id: OPTIONS.to_owned(),
                    title: "Options".to_owned(),
                    kind: GroupKind::Pick {
                        pick: Pick::Any,
                        items: options
                            .iter()
                            .map(|o| item(o, on.contains(o.name.as_str())))
                            .collect(),
                    },
                    defaulted: !saved,
                });
                answered.push(Answer::picks(
                    OPTIONS,
                    options
                        .iter()
                        .filter(|o| on.contains(o.name.as_str()))
                        .map(|o| o.name.clone())
                        .collect(),
                ));
                for o in options.iter().filter(|o| !o.sub_options.is_empty()) {
                    let id = format!("option:{}", o.name);
                    let pick = picked
                        .picks
                        .get(&o.name)
                        .filter(|name| o.sub_options.iter().any(|s| s.name == **name))
                        .or(o.sub_options.first().map(|s| &s.name))
                        .cloned();
                    // Options that are off keep what's picked for them, but
                    // aren't shown.
                    if on.contains(o.name.as_str()) {
                        groups.push(Group {
                            id: id.clone(),
                            title: o.name.clone(),
                            kind: GroupKind::Pick {
                                pick: Pick::ExactlyOne,
                                items: o
                                    .sub_options
                                    .iter()
                                    .map(|s| item(s, Some(&s.name) == pick.as_ref()))
                                    .collect(),
                            },
                            defaulted: !picked.picks.contains_key(&o.name),
                        });
                    }
                    answered.push(Answer::picks(id, pick.into_iter().collect()));
                }
            }
        }
        let patches = renumbered(m, &read.folders(&picked))
            .iter()
            .filter(|(dst, _)| PatchName::parse(dst.file_name()).is_some_and(|p| p.part == 0))
            .count();
        let title = if read.name.trim().is_empty() {
            m.name.clone()
        } else {
            read.name.clone()
        };
        Ok(Form {
            pages: vec![Page {
                title: title.clone(),
                groups,
            }],
            title,
            summary: Some(format!(
                "Deploys {patches} patch{}.",
                if patches == 1 { "" } else { "es" }
            )),
            warnings: Vec::new(),
            answers: answered,
        })
    }

    fn save(
        &self,
        ctx: &ModContext<'_>,
        m: &InstalledMod,
        answers: &[Answer],
    ) -> Result<toml::Table> {
        let Some(read) = Read::from(m)? else {
            return Ok(toml::Table::new());
        };
        let picked = read.picked(ctx.state()?, Some(answers));
        toml::Table::try_from(&picked).map_err(|e| error(format!("can't save the picks: {e}")))
    }
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

fn error(message: String) -> Error {
    Error::Extension {
        id: "helldivers".to_owned(),
        message,
    }
}
