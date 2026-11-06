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
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    /// and go as the user picks. Values in `answers` haven't been checked
    /// against their fields yet, so ones that don't fit should be treated
    /// as unset; core tells the user what's wrong with them.
    fn form(
        &self,
        ctx: &ModContext<'_>,
        m: &InstalledMod,
        answers: Option<&[Answer]>,
    ) -> Result<Form>;

    /// Turns `answers` into the state to save for the mod, replacing what
    /// was saved. Values are checked against the fields they're for first,
    /// as [`Field::check`] does.
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
        self.find_near_top(path).is_some()
    }

    /// The contents of `path`, found as [`ModPeek::has_near_top`] finds it,
    /// for hooks that need to look inside a small file to know whether they
    /// apply. `None` if it isn't there or can't be read.
    pub fn read_near_top(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.find_near_top(path)?).ok()
    }

    fn find_near_top(&self, path: &str) -> Option<PathBuf> {
        let want = path.to_lowercase();
        match self.files {
            Peek::Files(files) => files
                .iter()
                .find(|f| {
                    let lower = f.rel.as_str().to_lowercase();
                    lower == want
                        || lower
                            .strip_suffix(&want)
                            .and_then(|top| top.strip_suffix('/'))
                            .is_some_and(|top| !top.contains('/'))
                })
                .map(|f| f.src.clone()),
            Peek::Dir(dir) => in_dir(dir, &want).or_else(|| {
                let mut tops: Vec<_> = std::fs::read_dir(dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.path())
                    .collect();
                tops.sort();
                tops.iter().find_map(|top| in_dir(top, &want))
            }),
        }
    }
}

/// Where `dir` has `path` (lowercase, `/`-separated), in any case.
fn in_dir(dir: &Path, path: &str) -> Option<PathBuf> {
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
        at = found?.path();
    }
    Some(at)
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

/// Items to pick, or fields to set, under a title.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// What [`Answer::group`] names it by. Unique in the form, unless two
    /// groups really are the same; answers then go to them in order.
    pub id: String,
    pub title: String,
    #[serde(flatten)]
    pub kind: GroupKind,
    /// Nothing's been picked or set in it, so it has the defaults.
    pub defaulted: bool,
}

impl Group {
    /// How many of its items can be picked, or `None` if it has fields
    /// instead.
    pub fn pick(&self) -> Option<Pick> {
        match &self.kind {
            GroupKind::Pick { pick, .. } => Some(*pick),
            GroupKind::Fields { .. } => None,
        }
    }

    /// Its items to pick, or none if it has fields instead.
    pub fn items(&self) -> &[Item] {
        match &self.kind {
            GroupKind::Pick { items, .. } => items,
            GroupKind::Fields { .. } => &[],
        }
    }

    /// Its fields to set, or none if it has items to pick instead.
    pub fn fields(&self) -> &[Field] {
        match &self.kind {
            GroupKind::Pick { .. } => &[],
            GroupKind::Fields { fields } => fields,
        }
    }
}

/// What a [`Group`] has. Frontends see it as the group's `kind`, with the
/// variant's own fields beside the group's.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum GroupKind {
    /// Items to pick, like an installer's options. What's picked is the
    /// group's [`Answer::items`].
    Pick { pick: Pick, items: Vec<Item> },
    /// Values to set, like a config file's settings. What's set is the
    /// group's [`Answer::values`].
    Fields { fields: Vec<Field> },
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

/// A value to set, in a [`GroupKind::Fields`] group.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// What [`Answer::values`] names it by, unique in its group.
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    #[serde(flatten)]
    pub kind: FieldKind,
    /// What it's set to now.
    pub value: Value,
    /// What it's set to if the user doesn't say, if there's such a thing.
    pub default: Option<Value>,
}

impl Field {
    /// `value` as this field keeps it, or why it can't be: whole numbers
    /// are taken for numbers, and colours are made lowercase.
    pub fn check(&self, value: &Value) -> std::result::Result<Value, String> {
        fn range<T: PartialOrd + std::fmt::Display>(
            v: T,
            min: Option<T>,
            max: Option<T>,
        ) -> std::result::Result<(), String> {
            match (min, max) {
                (Some(min), _) if v < min => Err(format!("{v} is less than {min}")),
                (_, Some(max)) if v > max => Err(format!("{v} is more than {max}")),
                _ => Ok(()),
            }
        }
        let wrong = || {
            Err(format!(
                "`{}` should be {}, not {value}",
                self.title,
                self.kind.describe()
            ))
        };
        let checked = match (&self.kind, value) {
            (FieldKind::Toggle, Value::Bool(_))
            | (FieldKind::Text { multiline: true }, Value::Text(_)) => value.clone(),
            (FieldKind::Text { multiline: false }, Value::Text(text)) => {
                if text.contains('\n') {
                    return Err(format!("`{}` should be one line", self.title));
                }
                value.clone()
            }
            (FieldKind::Int { min, max, .. }, Value::Int(v)) => {
                range(*v, *min, *max).map_err(|e| format!("`{}`: {e}", self.title))?;
                value.clone()
            }
            (FieldKind::Float { min, max, .. }, Value::Int(_) | Value::Float(_)) => {
                let v = match value {
                    Value::Int(v) => *v as f64,
                    Value::Float(v) => *v,
                    _ => unreachable!(),
                };
                if !v.is_finite() {
                    return wrong();
                }
                range(v, *min, *max).map_err(|e| format!("`{}`: {e}", self.title))?;
                Value::Float(v)
            }
            (FieldKind::Choice { choices }, Value::Text(id)) => {
                if !choices.iter().any(|c| c.id == *id) {
                    return wrong();
                }
                value.clone()
            }
            (FieldKind::Color { alpha }, Value::Text(text)) => {
                let digits = text.strip_prefix('#').unwrap_or("");
                let len_ok = digits.len() == 6 || (*alpha && digits.len() == 8);
                if !len_ok || !digits.chars().all(|c| c.is_ascii_hexdigit()) {
                    return wrong();
                }
                Value::Text(text.to_ascii_lowercase())
            }
            _ => return wrong(),
        };
        Ok(checked)
    }
}

/// What a [`Field`] holds, and what frontends show to set it with.
/// Frontends see it as the field's `type`, with the variant's own fields
/// beside the field's.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum FieldKind {
    /// On or off: a [`Value::Bool`].
    Toggle,
    /// A whole number: a [`Value::Int`]. `step` is a hint for frontends.
    Int {
        min: Option<i64>,
        max: Option<i64>,
        step: Option<i64>,
    },
    /// A number: a [`Value::Float`]. `step` is a hint for frontends.
    Float {
        min: Option<f64>,
        max: Option<f64>,
        step: Option<f64>,
    },
    /// Text: a [`Value::Text`], of one line unless `multiline`.
    Text { multiline: bool },
    /// One of `choices`: a [`Value::Text`] that's a [`Choice::id`].
    Choice { choices: Vec<Choice> },
    /// A colour: a [`Value::Text`] like `#ff8800`, or `#ff880080` with
    /// `alpha`.
    Color { alpha: bool },
}

impl FieldKind {
    /// What it should be, for messages: "a whole number from 0 to 10".
    pub fn describe(&self) -> String {
        fn range<T: std::fmt::Display>(what: &str, min: &Option<T>, max: &Option<T>) -> String {
            match (min, max) {
                (Some(min), Some(max)) => format!("{what} from {min} to {max}"),
                (Some(min), None) => format!("{what} of at least {min}"),
                (None, Some(max)) => format!("{what} of at most {max}"),
                (None, None) => what.to_owned(),
            }
        }
        match self {
            FieldKind::Toggle => "true or false".to_owned(),
            FieldKind::Int { min, max, .. } => range("a whole number", min, max),
            FieldKind::Float { min, max, .. } => range("a number", min, max),
            FieldKind::Text { .. } => "text".to_owned(),
            FieldKind::Choice { choices } => {
                let ids: Vec<&str> = choices.iter().map(|c| c.id.as_str()).collect();
                format!("one of {}", ids.join(", "))
            }
            FieldKind::Color { alpha: false } => "a colour like #ff8800".to_owned(),
            FieldKind::Color { alpha: true } => "a colour like #ff8800 or #ff880080".to_owned(),
        }
    }
}

/// One of a [`FieldKind::Choice`]'s choices.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    /// What the field's [`Value::Text`] is when it's chosen.
    pub id: String,
    pub title: String,
}

/// What a [`Field`] is set to. Frontends send and get plain JSON values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Bool(v) => write!(f, "{v}"),
            Value::Int(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Text(v) => write!(f, "{v:?}"),
        }
    }
}

/// What's picked or set in one group.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Answer {
    /// The group's [`Group::id`].
    pub group: String,
    /// Its picked items' [`Item::id`]s, for a [`GroupKind::Pick`] group.
    #[serde(default)]
    pub items: Vec<String>,
    /// Its fields' values, by [`Field::id`], for a [`GroupKind::Fields`]
    /// group. Fields left out keep their defaults.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub values: BTreeMap<String, Value>,
}

impl Answer {
    /// Picking `items` in `group`.
    pub fn picks(group: impl Into<String>, items: Vec<String>) -> Self {
        Self {
            group: group.into(),
            items,
            values: BTreeMap::new(),
        }
    }

    /// Setting `values` in `group`.
    pub fn values(group: impl Into<String>, values: BTreeMap<String, Value>) -> Self {
        Self {
            group: group.into(),
            items: Vec::new(),
            values,
        }
    }
}

/// `answers` with their values as `form`'s fields keep them, or why one
/// can't be set. Values for groups that aren't in the form now are kept as
/// they are, since they might be shown again.
pub(crate) fn checked(form: &Form, answers: &[Answer]) -> Result<Vec<Answer>> {
    let groups: Vec<&Group> = form.pages.iter().flat_map(|p| &p.groups).collect();
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    answers
        .iter()
        .map(|answer| {
            // The nth answer for an id goes to the nth group with it.
            let nth = seen.entry(&answer.group).or_default();
            let group = groups.iter().filter(|g| g.id == answer.group).nth(*nth);
            *nth += 1;
            let mut answer = answer.clone();
            let Some(group) = group else {
                return Ok(answer);
            };
            for (id, value) in &mut answer.values {
                let field = group.fields().iter().find(|f| f.id == *id).ok_or_else(|| {
                    Error::invalid(format!("`{}` has no setting `{id}`", group.title))
                })?;
                *value = field.check(value).map_err(Error::invalid)?;
            }
            Ok(answer)
        })
        .collect()
}

#[cfg(test)]
mod tests;
