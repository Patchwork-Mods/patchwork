//! FOMOD installers: a `fomod/ModuleConfig.xml` in a mod that says which of
//! its files to install where, partly from options the user picks in a
//! series of steps, rather than the mod's files being deployed as they're
//! packed. Most of Bethesda games' bigger mods on Nexus come this way.
//!
//! Nothing is installed when the mod is added. The profile keeps the options
//! picked ([`Selection`]s, by name, so they mostly survive the mod being
//! updated) and each deploy works out the files from them again. A group
//! nothing was saved for gets the installer's own defaults: its required
//! and recommended options.
//!
//! Games turn it on with `[ext.fomod.installer]` in their definition,
//! whose `folder` is the one in the game's that installers' destinations
//! are relative to, and that the files their conditions ask about are in.
use std::collections::HashMap;

use patchwork_core::deploy::ModFile;
use patchwork_core::options::{ItemState, Pick};
use patchwork_core::relpath::RelPath;
use roxmltree::{Document, Node};
use serde::{Deserialize, Serialize};

mod hooks;

pub use hooks::{FOMOD_EXTENSION, FomodInstaller};

/// The options picked for one group of a mod's installer, as a profile
/// saves them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    pub step: String,
    pub group: String,
    /// The options picked, by name.
    #[serde(default)]
    pub options: Vec<String>,
}

/// Where a file an installer asks about is, for its conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileState {
    /// There, and loaded by the game.
    Active,
    /// There, but not loaded.
    Inactive,
    Missing,
}

/// A mod's installer, read from its `ModuleConfig.xml`.
#[derive(Debug, Clone)]
pub struct Installer {
    pub name: String,
    /// The folder in the mod that the installer's paths are relative to:
    /// the one with `fomod` in it, lowercased, ending in `/` unless it's the
    /// mod's top.
    base: String,
    needs: Option<Condition>,
    required: Vec<Install>,
    steps: Vec<Step>,
    conditional: Vec<(Condition, Vec<Install>)>,
}

#[derive(Debug, Clone)]
struct Step {
    name: String,
    visible: Option<Condition>,
    groups: Vec<Group>,
}

#[derive(Debug, Clone)]
struct Group {
    name: String,
    kind: Pick,
    options: Vec<Choice>,
}

/// One of a group's options. (FOMOD calls them plugins, which here means
/// something else.)
#[derive(Debug, Clone)]
struct Choice {
    name: String,
    description: String,
    image: Option<String>,
    files: Vec<Install>,
    flags: Vec<(String, String)>,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Fixed(ItemState),
    /// The first pattern whose condition holds says, or else `default`.
    Depends {
        default: ItemState,
        patterns: Vec<(Condition, ItemState)>,
    },
}

/// A `file` or `folder` to install.
#[derive(Debug, Clone)]
struct Install {
    source: String,
    /// `None` if the installer leaves it out: the same as `source`.
    destination: Option<String>,
    folder: bool,
    /// Higher wins when two install the same file.
    priority: i64,
    /// Installed even when its option isn't picked.
    always: bool,
    /// Installed even when its option isn't picked, so long as it could be.
    if_usable: bool,
}

#[derive(Debug, Clone)]
enum Condition {
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Flag {
        name: String,
        value: String,
    },
    File {
        name: String,
        state: FileState,
    },
    /// Game and mod manager versions, which patchwork doesn't check.
    Met,
}

/// An installer worked out for a set of [`Selection`]s.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    /// The steps the installer shows, given what's picked in earlier ones.
    pub steps: Vec<StepView>,
    /// What's picked, as a profile saves it: each shown group's options,
    /// then any saved for groups of steps that aren't shown now, so they're
    /// kept for if they are again.
    pub selections: Vec<Selection>,
    /// What to install where, as `(destination, file in the mod)`.
    pub files: Vec<(RelPath, RelPath)>,
    pub warnings: Vec<String>,
}

impl Resolved {
    /// Whether any shown group got the installer's defaults, having
    /// nothing saved.
    #[cfg(test)]
    pub fn defaulted(&self) -> bool {
        self.steps
            .iter()
            .flat_map(|s| &s.groups)
            .any(|g| g.defaulted)
    }
}

/// A step of an installer, for showing.
#[derive(Debug, Clone)]
pub struct StepView {
    pub name: String,
    pub groups: Vec<GroupView>,
}

#[derive(Debug, Clone)]
pub struct GroupView {
    pub name: String,
    pub kind: Pick,
    pub options: Vec<OptionView>,
    /// Nothing was saved for it, so it has the installer's defaults.
    pub defaulted: bool,
}

#[derive(Debug, Clone)]
pub struct OptionView {
    pub name: String,
    pub description: String,
    /// Its picture, as a file in the mod.
    pub image: Option<RelPath>,
    pub kind: ItemState,
    pub picked: bool,
}

/// The mod's installer config, if it has one, and the folder its paths are
/// relative to (see [`Installer::parse`]): the shallowest
/// `fomod/ModuleConfig.xml`, in any case.
pub fn find(files: &[ModFile]) -> Option<(&ModFile, String)> {
    files
        .iter()
        .filter_map(|f| {
            let lower = f.rel.as_str().to_lowercase();
            let base = lower.strip_suffix("fomod/moduleconfig.xml")?;
            (base.is_empty() || base.ends_with('/')).then(|| (f, base.to_owned()))
        })
        .min_by_key(|(f, _)| f.rel.components().count())
}

impl Installer {
    /// Reads `ModuleConfig.xml`'s contents. `base` is the folder in the
    /// mod its paths are relative to, from [`find`].
    pub fn parse(bytes: &[u8], base: String) -> Result<Self, String> {
        let text = decode(bytes)?;
        let doc = Document::parse(&text).map_err(|e| format!("it isn't valid XML: {e}"))?;
        let config = doc.root_element();
        Ok(Self {
            name: child(config, "moduleName").map(text_of).unwrap_or_default(),
            base,
            needs: child(config, "moduleDependencies").map(condition),
            required: child(config, "requiredInstallFiles")
                .map(installs)
                .unwrap_or_default(),
            steps: child(config, "installSteps")
                .map(|s| {
                    ordered(s, children(s, "installStep").map(step).collect(), |s| {
                        &s.name
                    })
                })
                .unwrap_or_default(),
            conditional: child(config, "conditionalFileInstalls")
                .and_then(|c| child(c, "patterns"))
                .map(|p| {
                    children(p, "pattern")
                        .map(|p| {
                            let when = child(p, "dependencies").map_or(Condition::Met, condition);
                            (when, child(p, "files").map(installs).unwrap_or_default())
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
    }

    /// Works out the installer for `saved` picks. `files` are the mod's,
    /// and `state` says where a file the installer's conditions ask about
    /// is, relative to the folder installers install to.
    pub fn resolve(
        &self,
        files: &[RelPath],
        saved: &[Selection],
        state: &dyn Fn(&str) -> FileState,
    ) -> Resolved {
        let by_lower: HashMap<String, &RelPath> = files
            .iter()
            .map(|f| (f.as_str().to_lowercase(), f))
            .collect();
        let mut out = Resolved::default();
        let mut flags: HashMap<String, String> = HashMap::new();
        let mut used = vec![false; saved.len()];
        // Everything to install, with its option's place for ties.
        let mut installs: Vec<&Install> = self.required.iter().collect();

        if let Some(needs) = &self.needs
            && !needs.holds(&flags, state)
        {
            out.warnings.push(
                "its installer says it needs other mods or files that aren't there".to_owned(),
            );
        }
        for step in &self.steps {
            if step
                .visible
                .as_ref()
                .is_some_and(|v| !v.holds(&flags, state))
            {
                continue;
            }
            let mut view = StepView {
                name: step.name.clone(),
                groups: Vec::new(),
            };
            for group in &step.groups {
                let kinds: Vec<ItemState> = group
                    .options
                    .iter()
                    .map(|c| c.kind.get(&flags, state))
                    .collect();
                let found = saved
                    .iter()
                    .enumerate()
                    .position(|(i, s)| !used[i] && s.step == step.name && s.group == group.name);
                if let Some(i) = found {
                    used[i] = true;
                }
                let picked = pick(group, &kinds, found.map(|i| saved[i].options.as_slice()));
                for ((choice, &kind), &on) in group.options.iter().zip(&kinds).zip(&picked) {
                    if on {
                        for (name, value) in &choice.flags {
                            flags.insert(name.clone(), value.clone());
                        }
                    }
                    let usable = kind != ItemState::NotUsable;
                    installs.extend(
                        choice
                            .files
                            .iter()
                            .filter(|f| on || f.always || (f.if_usable && usable)),
                    );
                }
                out.selections.push(Selection {
                    step: step.name.clone(),
                    group: group.name.clone(),
                    options: group
                        .options
                        .iter()
                        .zip(&picked)
                        .filter(|(_, on)| **on)
                        .map(|(c, _)| c.name.clone())
                        .collect(),
                });
                view.groups.push(GroupView {
                    name: group.name.clone(),
                    kind: group.kind,
                    options: group
                        .options
                        .iter()
                        .zip(kinds)
                        .zip(picked)
                        .map(|((c, kind), picked)| OptionView {
                            name: c.name.clone(),
                            description: c.description.clone(),
                            image: c.image.as_deref().and_then(|i| self.file(&by_lower, i)),
                            kind,
                            picked,
                        })
                        .collect(),
                    defaulted: found.is_none(),
                });
            }
            out.steps.push(view);
        }
        for (when, files) in &self.conditional {
            if when.holds(&flags, state) {
                installs.extend(files);
            }
        }
        // Picks for groups the installer still has, in steps that aren't
        // shown now.
        out.selections.extend(
            saved
                .iter()
                .zip(&used)
                .filter(|(s, used)| {
                    !**used
                        && self.steps.iter().any(|step| {
                            step.name == s.step && step.groups.iter().any(|g| g.name == s.group)
                        })
                })
                .map(|(s, _)| s.clone()),
        );

        // Stable, so files of the same priority go in the order they came.
        installs.sort_by_key(|i| i.priority);
        let mut at: HashMap<String, usize> = HashMap::new();
        for install in installs {
            for (dst, src) in self.expand(install, files, &by_lower, &mut out.warnings) {
                match at.get(&dst.as_str().to_lowercase()) {
                    Some(&i) => out.files[i] = (dst, src),
                    None => {
                        at.insert(dst.as_str().to_lowercase(), out.files.len());
                        out.files.push((dst, src));
                    }
                }
            }
        }
        out
    }

    /// The file in the mod at `path`, relative to the installer.
    fn file(&self, by_lower: &HashMap<String, &RelPath>, path: &str) -> Option<RelPath> {
        let key = format!("{}{}", self.base, clean(path).to_lowercase());
        by_lower.get(&key).map(|f| (*f).clone())
    }

    /// The files `install` puts where.
    fn expand(
        &self,
        install: &Install,
        files: &[RelPath],
        by_lower: &HashMap<String, &RelPath>,
        warnings: &mut Vec<String>,
    ) -> Vec<(RelPath, RelPath)> {
        let source = clean(&install.source);
        let destination = install.destination.as_deref().map_or(source.clone(), clean);
        let mut found = Vec::new();
        if install.folder {
            let prefix = format!("{}{}", self.base, source.to_lowercase());
            let prefix = if prefix.is_empty() || prefix.ends_with('/') {
                prefix
            } else {
                format!("{prefix}/")
            };
            for f in files {
                let lower = f.as_str().to_lowercase();
                if let Some(rest) = lower.strip_prefix(&prefix) {
                    // `rest` as it's cased in the mod
                    let rest = &f.as_str()[f.as_str().len() - rest.len()..];
                    found.push((join(&destination, rest), f.clone()));
                }
            }
        } else if let Some(f) = self.file(by_lower, &source) {
            // An empty destination is the folder installed to.
            let to = if destination.is_empty() {
                f.file_name().to_owned()
            } else {
                destination
            };
            found.push((to, f));
        }
        if found.is_empty() {
            let what = if install.folder { "folder" } else { "file" };
            warnings.push(format!(
                "its installer installs the {what} `{}`, which isn't in it",
                install.source
            ));
        }
        found
            .into_iter()
            .filter_map(|(to, src)| match RelPath::new(&to) {
                Ok(to) => Some((to, src)),
                Err(e) => {
                    warnings.push(format!("its installer would install {src} to {e}"));
                    None
                }
            })
            .collect()
    }
}

/// Which of `group`'s options are picked: the ones `saved`, or the
/// defaults if nothing is, made to fit what the group allows.
fn pick(group: &Group, kinds: &[ItemState], saved: Option<&[String]>) -> Vec<bool> {
    let usable = |i: usize| kinds[i] != ItemState::NotUsable;
    let mut on: Vec<bool> = group
        .options
        .iter()
        .enumerate()
        .map(|(i, c)| {
            usable(i)
                && match saved {
                    Some(names) => names.contains(&c.name),
                    None => kinds[i] == ItemState::Recommended,
                }
        })
        .collect();
    for (i, kind) in kinds.iter().enumerate() {
        if *kind == ItemState::Required {
            on[i] = true;
        }
    }
    let first_usable = (0..on.len()).find(|&i| usable(i));
    match group.kind {
        Pick::All => on = (0..on.len()).map(usable).collect(),
        Pick::ExactlyOne | Pick::AtMostOne => {
            let keep = kinds
                .iter()
                .position(|k| *k == ItemState::Required)
                .or_else(|| on.iter().position(|o| *o));
            let keep = match (keep, group.kind) {
                (None, Pick::ExactlyOne) => first_usable,
                (keep, _) => keep,
            };
            on = (0..on.len()).map(|i| Some(i) == keep).collect();
        }
        Pick::AtLeastOne => {
            if !on.contains(&true)
                && let Some(i) = first_usable
            {
                on[i] = true;
            }
        }
        Pick::Any => {}
    }
    on
}

impl Kind {
    fn get(&self, flags: &HashMap<String, String>, state: &dyn Fn(&str) -> FileState) -> ItemState {
        match self {
            Kind::Fixed(kind) => *kind,
            Kind::Depends { default, patterns } => patterns
                .iter()
                .find(|(when, _)| when.holds(flags, state))
                .map_or(*default, |(_, kind)| *kind),
        }
    }
}

impl Condition {
    fn holds(&self, flags: &HashMap<String, String>, state: &dyn Fn(&str) -> FileState) -> bool {
        match self {
            Condition::All(all) => all.iter().all(|c| c.holds(flags, state)),
            Condition::Any(any) => any.iter().any(|c| c.holds(flags, state)),
            // A flag that was never set is empty.
            Condition::Flag { name, value } => flags.get(name).map_or("", String::as_str) == value,
            Condition::File { name, state: want } => state(&clean(name)) == *want,
            Condition::Met => true,
        }
    }
}

/// `ModuleConfig.xml`'s text. They're usually UTF-8 or UTF-16 with a byte
/// order mark, though some are UTF-16 without one, or Windows-1252.
fn decode(bytes: &[u8]) -> Result<String, String> {
    let utf16 = |bytes: &[u8], le: bool| {
        let units: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| {
                if le {
                    u16::from_le_bytes(pair)
                } else {
                    u16::from_be_bytes(pair)
                }
            })
            .collect();
        String::from_utf16(&units).map_err(|_| "it isn't valid UTF-16".to_owned())
    };
    let text = match bytes {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true)?,
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false)?,
        [b'<', 0, ..] => utf16(bytes, true)?,
        [0, b'<', ..] => utf16(bytes, false)?,
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    };
    // What the declaration says the encoding was doesn't matter now, and
    // the parser would only object to it.
    Ok(match text.trim_start().strip_prefix("<?xml") {
        Some(rest) => rest
            .split_once("?>")
            .map_or(text.clone(), |(_, r)| r.to_owned()),
        None => text,
    })
}

/// A path from an installer, with `/` between its parts and no empty or
/// `.` ones.
fn clean(path: &str) -> String {
    path.split(['/', '\\'])
        .filter(|p| !p.is_empty() && *p != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn join(dir: &str, rest: &str) -> String {
    match (dir.is_empty(), rest.is_empty()) {
        (true, _) => rest.to_owned(),
        (_, true) => dir.to_owned(),
        _ => format!("{dir}/{rest}"),
    }
}

// Installers are written by hand often enough that names are matched
// ignoring case.

fn children<'a, 'i>(node: Node<'a, 'i>, tag: &'static str) -> impl Iterator<Item = Node<'a, 'i>> {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case(tag))
}

fn child<'a, 'i>(node: Node<'a, 'i>, tag: &'static str) -> Option<Node<'a, 'i>> {
    children(node, tag).next()
}

fn attr<'a>(node: Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value())
}

fn text_of(node: Node<'_, '_>) -> String {
    node.text().unwrap_or_default().trim().to_owned()
}

/// `items` in the order the `order` attribute of `node` asks for. Without
/// one, that's by name.
fn ordered<T>(node: Node<'_, '_>, mut items: Vec<T>, name: impl Fn(&T) -> &str) -> Vec<T> {
    let key = |t: &T| name(t).to_lowercase();
    match attr(node, "order").map(str::to_ascii_lowercase).as_deref() {
        Some("explicit") => {}
        Some("descending") => items.sort_by_key(|t| std::cmp::Reverse(key(t))),
        _ => items.sort_by_key(key),
    }
    items
}

fn step(node: Node<'_, '_>) -> Step {
    Step {
        name: attr(node, "name").unwrap_or_default().to_owned(),
        visible: child(node, "visible").map(condition),
        groups: child(node, "optionalFileGroups")
            .map(|g| ordered(g, children(g, "group").map(group).collect(), |g| &g.name))
            .unwrap_or_default(),
    }
}

fn group(node: Node<'_, '_>) -> Group {
    let kind = match attr(node, "type").unwrap_or_default() {
        "SelectAtLeastOne" => Pick::AtLeastOne,
        "SelectAtMostOne" => Pick::AtMostOne,
        "SelectExactlyOne" => Pick::ExactlyOne,
        "SelectAll" => Pick::All,
        _ => Pick::Any,
    };
    Group {
        name: attr(node, "name").unwrap_or_default().to_owned(),
        kind,
        options: child(node, "plugins")
            .map(|p| ordered(p, children(p, "plugin").map(choice).collect(), |c| &c.name))
            .unwrap_or_default(),
    }
}

fn choice(node: Node<'_, '_>) -> Choice {
    let types = child(node, "typeDescriptor");
    let kind = match types {
        Some(t) => match (child(t, "type"), child(t, "dependencyType")) {
            (Some(t), _) => Kind::Fixed(option_kind(t)),
            (None, Some(d)) => Kind::Depends {
                default: child(d, "defaultType").map_or(ItemState::Optional, option_kind),
                patterns: child(d, "patterns")
                    .map(|p| {
                        children(p, "pattern")
                            .map(|p| {
                                (
                                    child(p, "dependencies").map_or(Condition::Met, condition),
                                    child(p, "type").map_or(ItemState::Optional, option_kind),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            (None, None) => Kind::Fixed(ItemState::Optional),
        },
        None => Kind::Fixed(ItemState::Optional),
    };
    Choice {
        name: attr(node, "name").unwrap_or_default().to_owned(),
        description: child(node, "description").map(text_of).unwrap_or_default(),
        image: child(node, "image")
            .and_then(|i| attr(i, "path"))
            .filter(|p| !p.trim().is_empty())
            .map(str::to_owned),
        files: child(node, "files").map(installs).unwrap_or_default(),
        flags: child(node, "conditionFlags")
            .map(|f| {
                children(f, "flag")
                    .map(|f| (attr(f, "name").unwrap_or_default().to_owned(), text_of(f)))
                    .collect()
            })
            .unwrap_or_default(),
        kind,
    }
}

fn option_kind(node: Node<'_, '_>) -> ItemState {
    match attr(node, "name").unwrap_or_default() {
        "Required" => ItemState::Required,
        "Recommended" => ItemState::Recommended,
        "NotUsable" => ItemState::NotUsable,
        "CouldBeUsable" => ItemState::CouldBeUsable,
        _ => ItemState::Optional,
    }
}

/// A `files` element's `file`s and `folder`s.
fn installs(node: Node<'_, '_>) -> Vec<Install> {
    let yes = |n: Node<'_, '_>, name| attr(n, name).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    node.children()
        .filter(|c| c.is_element())
        .filter_map(|c| {
            let folder = match c.tag_name().name().to_ascii_lowercase().as_str() {
                "file" => false,
                "folder" => true,
                _ => return None,
            };
            Some(Install {
                source: attr(c, "source").unwrap_or_default().to_owned(),
                destination: attr(c, "destination").map(str::to_owned),
                folder,
                priority: attr(c, "priority")
                    .and_then(|p| p.trim().parse().ok())
                    .unwrap_or(0),
                always: yes(c, "alwaysInstall"),
                if_usable: yes(c, "installIfUsable"),
            })
        })
        .collect()
}

/// A composite dependency: its `operator` applied to the conditions in it.
fn condition(node: Node<'_, '_>) -> Condition {
    let parts = node
        .children()
        .filter(|c| c.is_element())
        .filter_map(
            |c| match c.tag_name().name().to_ascii_lowercase().as_str() {
                "dependencies" => Some(condition(c)),
                "flagdependency" => Some(Condition::Flag {
                    name: attr(c, "flag").unwrap_or_default().to_owned(),
                    value: attr(c, "value").unwrap_or_default().to_owned(),
                }),
                "filedependency" => Some(Condition::File {
                    name: attr(c, "file").unwrap_or_default().to_owned(),
                    state: match attr(c, "state").unwrap_or_default() {
                        "Active" => FileState::Active,
                        "Inactive" => FileState::Inactive,
                        _ => FileState::Missing,
                    },
                }),
                // gameDependency, fommDependency, foseDependency and the like
                name if name.ends_with("dependency") => Some(Condition::Met),
                _ => None,
            },
        )
        .collect();
    if attr(node, "operator").is_some_and(|o| o.eq_ignore_ascii_case("or")) {
        Condition::Any(parts)
    } else {
        Condition::All(parts)
    }
}

#[cfg(test)]
mod tests;
