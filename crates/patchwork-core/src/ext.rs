//! Extensions: bundles of sources, deployers and the like, usually for one
//! game or family of games, registered with [`crate::Builder::extension`].
//! See `docs/design/extensions.md` for where this is going.
//!
//! An extension turns on for a game when the game's definition has a table
//! for it (`[ext.<id>]`, see [`crate::game::GameDef::ext`]). What it keeps
//! for a profile, or for one of a profile's mods, lives under the same id in
//! [`crate::profile::Profile::ext`] and [`crate::profile::ProfileMod::ext`].
//! Core saves, syncs, versions and hashes that state for every extension;
//! extensions never touch those maps themselves.
//!
//! Sources, deployers, provisioners and profile formats keep the ids they
//! have always had (profiles and game definitions name them). Other hooks,
//! like [`OrderedList`]s, [`ModTransform`]s and [`Check`]s, are `<extension>.<name>`; each is configured by the
//! game's `[ext.<extension>.<name>]` table, and can be turned off with
//! `off = ["<name>"]` in `[ext.<extension>]`.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::check::Check;
use crate::deploy::Deployer;
use crate::launch::Runner;
use crate::list::OrderedList;
use crate::options::{ModOptions, ModTransform};
use crate::provision::Provisioner;
use crate::share::ProfileFormat;
use crate::source::Source;
use crate::{Error, Result};

/// What extensions keep, by extension id. Kept as TOML so it round-trips
/// through builds that don't have the extension.
pub type ExtTables = BTreeMap<String, toml::Table>;

/// Whose state an extension's is: the profile's, or one of its mods'.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateOf<'a> {
    Profile,
    /// The mod with this name.
    Mod(&'a str),
}

/// The key core keeps the version of an extension's state under, next to
/// the state itself.
pub const VERSION_KEY: &str = "v";

/// A bundle of things that add to what patchwork can do, usually for one
/// game or family of games.
pub trait Extension: Send + Sync {
    /// Unique and stable. It names the extension's tables in game
    /// definitions and profiles, so it can never change once released.
    /// Lowercase letters, digits and `-`.
    fn id(&self) -> &'static str;

    /// Bumped when the shape of its saved state changes. State saved by an
    /// older version goes through [`Extension::migrate`] when it's read;
    /// state saved by a newer one can't be read or overwritten.
    fn state_version(&self) -> u32 {
        1
    }

    /// Turns state saved at version `from` into the current version's.
    /// Only called for `from` below [`Extension::state_version`].
    fn migrate(&self, from: u32, state: toml::Table) -> Result<toml::Table> {
        let _ = state;
        Err(Error::Extension {
            id: self.id().to_owned(),
            message: format!("can't read state saved at version {from}"),
        })
    }

    fn register(&self, ext: &mut Registrar<'_>);
}

/// What an [`Extension`] registers through, in [`Extension::register`].
pub struct Registrar<'a> {
    ext: &'static str,
    registered: &'a mut Registered,
}

impl Registrar<'_> {
    /// The id of the extension registering.
    pub fn id(&self) -> &'static str {
        self.ext
    }

    pub fn source(&mut self, source: impl Source + 'static) -> &mut Self {
        self.registered.source(Some(self.ext), source);
        self
    }

    pub fn deployer(&mut self, deployer: impl Deployer + 'static) -> &mut Self {
        self.registered.deployer(Some(self.ext), deployer);
        self
    }

    pub fn provisioner(&mut self, provisioner: impl Provisioner + 'static) -> &mut Self {
        self.registered.provisioner(Some(self.ext), provisioner);
        self
    }

    pub fn format(&mut self, format: impl ProfileFormat + 'static) -> &mut Self {
        self.registered.format(Some(self.ext), format);
        self
    }

    /// Adds a way of running games, which their `[launch.<id>]` tables can
    /// list in `runners` by its id.
    pub fn runner(&mut self, runner: impl Runner + 'static) -> &mut Self {
        self.registered.runner(Some(self.ext), runner);
        self
    }

    /// Adds a list the user puts in order, as `<extension>.<name>`. Its
    /// config is the game's `[ext.<extension>.<name>]` table, and its saved
    /// order is kept in the extension's state under `name`.
    pub fn ordered_list(
        &mut self,
        name: &'static str,
        list: impl OrderedList + 'static,
    ) -> &mut Self {
        self.registered.ordered_list(self.ext, name, list);
        self
    }

    /// Adds something that changes which files mods install, as
    /// `<extension>.<name>`, configured by `[ext.<extension>.<name>]`.
    /// Transforms run in the order they're registered.
    pub fn mod_transform(
        &mut self,
        name: &'static str,
        transform: impl ModTransform + 'static,
    ) -> &mut Self {
        self.registered.mod_transform(self.ext, name, transform);
        self
    }

    /// Adds choices the user makes about mods, as `<extension>.<name>`,
    /// configured by `[ext.<extension>.<name>]`. What they pick is saved as
    /// the extension's state for the mod. A transform and options that go
    /// together can share a name, and so a config table.
    pub fn mod_options(
        &mut self,
        name: &'static str,
        options: impl ModOptions + 'static,
    ) -> &mut Self {
        self.registered.mod_options(self.ext, name, options);
        self
    }

    /// Adds something that looks for problems with profiles, as
    /// `<extension>.<name>`, configured by `[ext.<extension>.<name>]`.
    pub fn check(&mut self, name: &'static str, check: impl Check + 'static) -> &mut Self {
        self.registered.check(self.ext, name, check);
        self
    }
}

/// A hook, as registered by an extension.
pub(crate) struct Hook<T: ?Sized> {
    pub ext: &'static str,
    /// Its name within the extension.
    pub name: &'static str,
    pub hook: Arc<T>,
}

/// The id of hook `name` of extension `ext`.
pub fn hook_id(ext: &str, name: &str) -> String {
    format!("{ext}.{name}")
}

/// Everything registered with a [`crate::Builder`], directly or by
/// extensions.
#[derive(Default)]
pub(crate) struct Registered {
    pub sources: BTreeMap<String, Arc<dyn Source>>,
    pub deployers: BTreeMap<String, Arc<dyn Deployer>>,
    pub provisioners: BTreeMap<String, Arc<dyn Provisioner>>,
    pub formats: BTreeMap<String, Arc<dyn ProfileFormat>>,
    pub runners: BTreeMap<String, Arc<dyn Runner>>,
    pub extensions: BTreeMap<String, Arc<dyn Extension>>,
    /// By hook id.
    pub lists: BTreeMap<String, Hook<dyn OrderedList>>,
    /// In the order they were registered, which is the order they run in.
    pub transforms: Vec<Hook<dyn ModTransform>>,
    /// By hook id.
    pub options: BTreeMap<String, Hook<dyn ModOptions>>,
    /// By hook id.
    pub checks: BTreeMap<String, Hook<dyn Check>>,
    /// Which extension registered each thing, by `(kind, id)`, so a clash
    /// can say who's involved. `None` for things registered directly.
    owners: HashMap<(&'static str, String), Option<&'static str>>,
    /// Clashes and bad ids, which stop [`crate::Builder::open`].
    pub problems: Vec<String>,
}

impl Registered {
    pub fn extension(&mut self, extension: impl Extension + 'static) {
        let id = extension.id();
        if let Err(e) = validate_ext_id(id) {
            self.problems.push(e.to_string());
            return;
        }
        if self.extensions.contains_key(id) {
            self.problems
                .push(format!("extension `{id}` is registered twice"));
            return;
        }
        extension.register(&mut Registrar {
            ext: id,
            registered: self,
        });
        self.extensions.insert(id.to_owned(), Arc::new(extension));
    }

    pub fn source(&mut self, owner: Option<&'static str>, source: impl Source + 'static) {
        let id = source.id().to_owned();
        if self.claim("source", &id, owner) {
            self.sources.insert(id, Arc::new(source));
        }
    }

    pub fn deployer(&mut self, owner: Option<&'static str>, deployer: impl Deployer + 'static) {
        let id = deployer.kind().to_owned();
        if self.claim("deployer", &id, owner) {
            self.deployers.insert(id, Arc::new(deployer));
        }
    }

    pub fn provisioner(
        &mut self,
        owner: Option<&'static str>,
        provisioner: impl Provisioner + 'static,
    ) {
        let id = provisioner.kind().to_owned();
        if self.claim("provisioner", &id, owner) {
            self.provisioners.insert(id, Arc::new(provisioner));
        }
    }

    pub fn format(&mut self, owner: Option<&'static str>, format: impl ProfileFormat + 'static) {
        let id = format.id().to_owned();
        if self.claim("profile format", &id, owner) {
            self.formats.insert(id, Arc::new(format));
        }
    }

    pub fn runner(&mut self, owner: Option<&'static str>, runner: impl Runner + 'static) {
        let id = runner.id().to_owned();
        if self.claim("runner", &id, owner) {
            self.runners.insert(id, Arc::new(runner));
        }
    }

    pub fn ordered_list(
        &mut self,
        ext: &'static str,
        name: &'static str,
        list: impl OrderedList + 'static,
    ) {
        if let Some(id) = self.hook("ordered list", ext, name) {
            let hook = Hook {
                ext,
                name,
                hook: Arc::new(list) as Arc<dyn OrderedList>,
            };
            self.lists.insert(id, hook);
        }
    }

    pub fn mod_transform(
        &mut self,
        ext: &'static str,
        name: &'static str,
        transform: impl ModTransform + 'static,
    ) {
        if self.hook("mod transform", ext, name).is_some() {
            self.transforms.push(Hook {
                ext,
                name,
                hook: Arc::new(transform) as Arc<dyn ModTransform>,
            });
        }
    }

    pub fn mod_options(
        &mut self,
        ext: &'static str,
        name: &'static str,
        options: impl ModOptions + 'static,
    ) {
        if let Some(id) = self.hook("mod options", ext, name) {
            let hook = Hook {
                ext,
                name,
                hook: Arc::new(options) as Arc<dyn ModOptions>,
            };
            self.options.insert(id, hook);
        }
    }

    pub fn check(&mut self, ext: &'static str, name: &'static str, check: impl Check + 'static) {
        if let Some(id) = self.hook("check", ext, name) {
            let hook = Hook {
                ext,
                name,
                hook: Arc::new(check) as Arc<dyn Check>,
            };
            self.checks.insert(id, hook);
        }
    }

    /// The id of extension `ext`'s `kind` called `name`, if it's a valid
    /// name and nothing else has it.
    fn hook(&mut self, kind: &'static str, ext: &'static str, name: &str) -> Option<String> {
        if let Err(e) = validate_ext_id(name) {
            self.problems
                .push(format!("{e} (a {kind} of extension `{ext}`)"));
            return None;
        }
        let id = hook_id(ext, name);
        self.claim(kind, &id, Some(ext)).then_some(id)
    }

    /// Records `owner` as having registered the `kind` called `id`, unless
    /// something already has.
    fn claim(&mut self, kind: &'static str, id: &str, owner: Option<&'static str>) -> bool {
        let by = |owner: Option<&str>| match owner {
            Some(ext) => format!("extension `{ext}`"),
            None => "the builder".to_owned(),
        };
        match self.owners.get(&(kind, id.to_owned())) {
            Some(&first) => {
                self.problems.push(format!(
                    "{kind} `{id}` is registered by both {} and {}",
                    by(first),
                    by(owner)
                ));
                false
            }
            None => {
                self.owners.insert((kind, id.to_owned()), owner);
                true
            }
        }
    }
}

fn validate_ext_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if valid {
        Ok(())
    } else {
        Err(Error::invalid(format!(
            "`{id}` isn't a valid extension id; use lowercase letters, digits and `-`"
        )))
    }
}

/// `ext`'s state in `tables`, or the default if it has none. Keys in
/// `reserved` (its lists' saved orders) are left out.
pub(crate) fn read_state<T: DeserializeOwned + Default>(
    ext: &dyn Extension,
    tables: &ExtTables,
    reserved: &[&str],
) -> Result<T> {
    let mut table = current(ext, tables)?;
    for key in reserved {
        table.remove(*key);
    }
    toml::Value::Table(table)
        .try_into()
        .map_err(|e| ext_error(ext, format!("its saved state can't be read: {e}")))
}

/// Saves `value` as `ext`'s state in `tables`, at its current version,
/// keeping the `reserved` keys it already had.
pub(crate) fn write_state<T: Serialize>(
    ext: &dyn Extension,
    tables: &mut ExtTables,
    value: &T,
    reserved: &[&str],
) -> Result<()> {
    let mut old = current(ext, tables)?;
    let mut table = toml::Table::try_from(value)
        .map_err(|e| ext_error(ext, format!("its state can't be saved: {e}")))?;
    for key in [VERSION_KEY].iter().chain(reserved) {
        if table.contains_key(*key) {
            return Err(ext_error(
                ext,
                format!("its state can't have a `{key}` field, which core keeps"),
            ));
        }
    }
    for key in reserved {
        if let Some(kept) = old.remove(*key) {
            table.insert((*key).to_owned(), kept);
        }
    }
    store(ext, tables, table);
    Ok(())
}

/// The value of one key of `ext`'s state, or the default if it's not
/// there.
pub(crate) fn read_key<T: DeserializeOwned + Default>(
    ext: &dyn Extension,
    tables: &ExtTables,
    key: &str,
) -> Result<T> {
    match current(ext, tables)?.remove(key) {
        None => Ok(T::default()),
        Some(value) => value
            .try_into()
            .map_err(|e| ext_error(ext, format!("its saved `{key}` can't be read: {e}"))),
    }
}

/// Saves `value` as one key of `ext`'s state, leaving the rest as it is.
/// An empty array or table leaves the key out.
pub(crate) fn write_key<T: Serialize>(
    ext: &dyn Extension,
    tables: &mut ExtTables,
    key: &str,
    value: &T,
) -> Result<()> {
    let mut table = current(ext, tables)?;
    let value = toml::Value::try_from(value)
        .map_err(|e| ext_error(ext, format!("its `{key}` can't be saved: {e}")))?;
    let empty = match &value {
        toml::Value::Array(a) => a.is_empty(),
        toml::Value::Table(t) => t.is_empty(),
        _ => false,
    };
    if empty {
        table.remove(key);
    } else {
        table.insert(key.to_owned(), value);
    }
    store(ext, tables, table);
    Ok(())
}

/// `ext`'s state in `tables` at its current version, migrated if it was
/// saved at an older one, without the version itself.
fn current(ext: &dyn Extension, tables: &ExtTables) -> Result<toml::Table> {
    let Some(table) = tables.get(ext.id()) else {
        return Ok(toml::Table::new());
    };
    let mut table = table.clone();
    let saved = saved_version(ext, &table)?;
    table.remove(VERSION_KEY);
    if saved < ext.state_version() {
        table = ext.migrate(saved, table)?;
    }
    Ok(table)
}

/// Puts `table` in `tables` as `ext`'s state, with its version. State
/// that's empty (usually the default) is left out altogether.
fn store(ext: &dyn Extension, tables: &mut ExtTables, mut table: toml::Table) {
    if table.is_empty() {
        tables.remove(ext.id());
    } else {
        table.insert(
            VERSION_KEY.to_owned(),
            toml::Value::Integer(ext.state_version().into()),
        );
        tables.insert(ext.id().to_owned(), table);
    }
}

/// The version `table` was saved at, failing if it's newer than `ext`
/// reads. State without one (written by hand, say) counts as version 1.
fn saved_version(ext: &dyn Extension, table: &toml::Table) -> Result<u32> {
    let saved = match table.get(VERSION_KEY) {
        None => 1,
        Some(v) => v
            .as_integer()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| ext_error(ext, format!("`{VERSION_KEY}` must be a whole number")))?,
    };
    if saved > ext.state_version() {
        return Err(ext_error(
            ext,
            format!(
                "its state was saved by a newer patchwork (version {saved}, this build reads \
                 up to {}); update patchwork",
                ext.state_version()
            ),
        ));
    }
    Ok(saved)
}

fn ext_error(ext: &dyn Extension, message: String) -> Error {
    Error::Extension {
        id: ext.id().to_owned(),
        message,
    }
}

#[cfg(test)]
mod tests;
