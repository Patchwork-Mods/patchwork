//! Hooks on single mods: changing which of a mod's files it installs
//! ([`ModTransform`]), and the choices the user makes about it that say
//! how ([`ModOptions`]), like a FOMOD installer's.
//!
//! Both work from the extension's state for the mod, which
//! [`ModOptions::save`] writes and [`ModContext::state`] reads. Core saves,
//! syncs and hashes it, so a changed pick makes the deploy out of date
//! without the extension doing anything. A mod nothing's been saved for
//! gets whatever the extension does by default, like an installer's
//! recommended options.
//!
//! Options are shown as a [`Form`] that frontends render without knowing
//! what it's for: pages of groups of items to pick, as an installer's steps
//! are.

use std::cell::OnceCell;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::deploy::{GameFiles, InstalledMod, ModFile};
use crate::game::GameDef;
use crate::relpath::RelPath;
use crate::{Error, Result};

/// Changes which of a mod's files it installs, before the deployer sees
/// them, registered with
/// [`Registrar::mod_transform`](crate::ext::Registrar::mod_transform).
///
/// It can only pick, rename and move the mod's own files. Anything that
/// makes new files, or combines several mods', belongs in
/// [`Deployer::build`](crate::deploy::Deployer::build).
pub trait ModTransform: Send + Sync {
    /// Whether it has anything to do for `m`. Called for every mod in
    /// every deploy, so it should only look for a file or two.
    fn applies(&self, ctx: &ModContext<'_>, m: &ModPeek<'_>) -> bool;

    /// The files `m` should install instead of the ones it's packed with.
    /// An error leaves the mod's files as they're packed, with the error
    /// as a warning.
    fn transform(&self, ctx: &ModContext<'_>, m: &InstalledMod) -> Result<Transformed>;
}

/// What a [`ModTransform`] makes of a mod.
#[derive(Debug, Clone, Default)]
pub struct Transformed {
    /// What to install where, as `(destination, file in the mod)`. Files
    /// that aren't in the mod are left out, with a warning.
    pub files: Vec<(RelPath, RelPath)>,
    /// Whole sentences, naming the mod.
    pub warnings: Vec<String>,
}

/// Choices the user makes about a mod, registered with
/// [`Registrar::mod_options`](crate::ext::Registrar::mod_options).
pub trait ModOptions: Send + Sync {
    /// What frontends call the choices, like "Installer".
    fn title(&self) -> &str;

    /// Whether `m` has choices to make. Called for every mod whenever a
    /// profile is shown, so it should only look for a file or two.
    fn applies(&self, ctx: &ModContext<'_>, m: &ModPeek<'_>) -> bool;

    /// The choices as they stand with `answers`, or with what's saved if
    /// `None`. Called again after each change, so items and pages can come
    /// and go as the user picks.
    fn form(
        &self,
        ctx: &ModContext<'_>,
        m: &InstalledMod,
        answers: Option<&[Answer]>,
    ) -> Result<Form>;

    /// Turns `answers` into the state to save for the mod, replacing what
    /// was saved.
    fn save(
        &self,
        ctx: &ModContext<'_>,
        m: &InstalledMod,
        answers: &[Answer],
    ) -> Result<toml::Table>;
}

/// What a [`ModTransform`] or [`ModOptions`] gets to work with.
pub struct ModContext<'a> {
    game: &'a GameDef,
    config: &'a toml::Table,
    state: &'a toml::Table,
    ext: &'a str,
    mods: Mods<'a>,
    files: Option<&'a GameFiles<'a>>,
}

/// The profile's mods, or how to get them when first asked for.
enum Mods<'a> {
    Ready(&'a [InstalledMod]),
    Lazy(
        &'a OnceCell<Vec<InstalledMod>>,
        &'a dyn Fn() -> Vec<InstalledMod>,
    ),
}

impl<'a> ModContext<'a> {
    /// Core makes these; this is for testing hooks on their own. `state` is
    /// extension `ext`'s for the mod, as [`ModContext::state`] reads it.
    pub fn new(
        game: &'a GameDef,
        config: &'a toml::Table,
        ext: &'a str,
        state: &'a toml::Table,
        mods: &'a [InstalledMod],
        files: Option<&'a GameFiles<'a>>,
    ) -> Self {
        Self {
            game,
            config,
            state,
            ext,
            mods: Mods::Ready(mods),
            files,
        }
    }

    /// Like [`ModContext::new`], with the mods only got if a hook asks.
    pub(crate) fn lazy(
        game: &'a GameDef,
        config: &'a toml::Table,
        ext: &'a str,
        state: &'a toml::Table,
        mods: (
            &'a OnceCell<Vec<InstalledMod>>,
            &'a dyn Fn() -> Vec<InstalledMod>,
        ),
        files: Option<&'a GameFiles<'a>>,
    ) -> Self {
        Self {
            game,
            config,
            state,
            ext,
            mods: Mods::Lazy(mods.0, mods.1),
            files,
        }
    }

    pub fn game(&self) -> &GameDef {
        self.game
    }

    /// The hook's `[ext.<extension>.<hook>]` table in the game definition.
    pub fn config(&self) -> &toml::Table {
        self.config
    }

    /// What the extension has saved for the mod, or the default if
    /// nothing.
    pub fn state<T: DeserializeOwned + Default>(&self) -> Result<T> {
        toml::Value::Table(self.state.clone())
            .try_into()
            .map_err(|e| Error::Extension {
                id: self.ext.to_owned(),
                message: format!("its saved state for the mod can't be read: {e}"),
            })
    }

    /// Whether anything's been saved for the mod.
    pub fn saved(&self) -> bool {
        !self.state.is_empty()
    }

    /// Every enabled mod in the profile, in load order, with its files as
    /// they're packed, for hooks that need to know what else is there.
    /// Listing them can take a while, so it's only done when asked.
    pub fn mods(&self) -> &[InstalledMod] {
        match &self.mods {
            Mods::Ready(mods) => mods,
            Mods::Lazy(cell, load) => cell.get_or_init(load),
        }
    }

    /// The game's own files, if it's been set up.
    pub fn files(&self) -> Option<&GameFiles<'a>> {
        self.files
    }
}

/// A quick look at a mod, for hooks' `applies`: enough to look for a file
/// without listing all of them.
pub struct ModPeek<'a> {
    name: &'a str,
    kind: Option<&'a str>,
    files: Peek<'a>,
}

enum Peek<'a> {
    Files(&'a [ModFile]),
    /// Where it's unpacked.
    Dir(&'a Path),
}

impl<'a> ModPeek<'a> {
    pub fn of(m: &'a InstalledMod) -> Self {
        Self {
            name: &m.name,
            kind: m.kind.as_deref(),
            files: Peek::Files(&m.files),
        }
    }

    /// A mod unpacked in `dir`.
    pub(crate) fn dir(name: &'a str, kind: Option<&'a str>, dir: &'a Path) -> Self {
        Self {
            name,
            kind,
            files: Peek::Dir(dir),
        }
    }

    pub fn name(&self) -> &str {
        self.name
    }

    /// What sort of thing its source says it is, if it says.
    pub fn kind(&self) -> Option<&str> {
        self.kind
    }

    /// Whether it has `path`, in any case, at its top or in a folder at its
    /// top (since mods are often packed inside one).
    pub fn has_near_top(&self, path: &str) -> bool {
        let want = path.to_lowercase();
        match self.files {
            Peek::Files(files) => files.iter().any(|f| {
                let lower = f.rel.as_str().to_lowercase();
                lower == want
                    || lower
                        .strip_suffix(&want)
                        .and_then(|top| top.strip_suffix('/'))
                        .is_some_and(|top| !top.contains('/'))
            }),
            Peek::Dir(dir) => {
                in_dir(dir, &want)
                    || std::fs::read_dir(dir)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .any(|e| {
                            e.file_type().is_ok_and(|t| t.is_dir()) && in_dir(&e.path(), &want)
                        })
            }
        }
    }
}

/// Whether `dir` has `path` (lowercase, `/`-separated), in any case.
fn in_dir(dir: &Path, path: &str) -> bool {
    let mut at = dir.to_owned();
    let mut parts = path.split('/').peekable();
    while let Some(part) = parts.next() {
        let last = parts.peek().is_none();
        let found = std::fs::read_dir(&at).ok().and_then(|entries| {
            entries.flatten().find(|e| {
                e.file_name().to_string_lossy().to_lowercase() == part
                    && e.file_type().is_ok_and(|t| t.is_dir() != last)
            })
        });
        match found {
            Some(e) => at = e.path(),
            None => return false,
        }
    }
    true
}

/// Choices about a mod, as frontends show them.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Form {
    /// What the choices are about, like the name an installer gives the
    /// mod. Frontends show it along with the mod's own name if they differ.
    pub title: String,
    /// Shown one at a time, in order, like an installer's steps.
    pub pages: Vec<Page>,
    /// What everything picked amounts to, like "installs 214 files".
    pub summary: Option<String>,
    pub warnings: Vec<String>,
    /// What's picked, to change and send back to [`ModOptions::form`] or
    /// [`ModOptions::save`]: each group's on the pages, along with any for
    /// groups that aren't shown now but might be again.
    pub answers: Vec<Answer>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub title: String,
    pub groups: Vec<Group>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// What [`Answer::group`] names it by. Unique in the form, unless two
    /// groups really are the same; answers then go to them in order.
    pub id: String,
    pub title: String,
    pub pick: Pick,
    pub items: Vec<Item>,
    /// Nothing's been picked in it, so it has the defaults.
    pub defaulted: bool,
}

/// How many of a group's items can be picked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Pick {
    ExactlyOne,
    AtMostOne,
    AtLeastOne,
    /// They're all picked, and can't be unpicked.
    All,
    Any,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    /// What [`Answer::items`] names it by, unique in its group.
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    /// A picture of it: a file in the mod, which core finds for frontends.
    pub image: Option<RelPath>,
    pub state: ItemState,
    pub picked: bool,
}

/// What can be done with an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ItemState {
    /// Always picked.
    Required,
    /// Picked unless the user says otherwise.
    Recommended,
    Optional,
    /// Can't be picked, usually because something it needs isn't there.
    NotUsable,
    /// Can be picked, but might not work.
    CouldBeUsable,
}

/// What's picked in one group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    /// The group's [`Group::id`].
    pub group: String,
    /// Its picked items' [`Item::id`]s.
    #[serde(default)]
    pub items: Vec<String>,
}
