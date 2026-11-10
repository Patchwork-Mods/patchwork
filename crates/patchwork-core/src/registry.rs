use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use crate::builtin;
use crate::game::{GAME_SCHEMA, GameDef};
use crate::hash::ContentHash;
use crate::paths::Paths;
use crate::{Error, Result};

/// Where a registry entry was loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    Registry(String),
    /// A directory from `$PATCHWORK_REGISTRY_PATH`.
    Dir(PathBuf),
    User,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Builtin => f.write_str("builtin"),
            Origin::Registry(name) => write!(f, "registry:{name}"),
            Origin::Dir(path) => write!(f, "dir:{}", path.display()),
            Origin::User => f.write_str("user"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry<T> {
    pub value: T,
    /// The layer the entry was first defined in.
    pub origin: Origin,
    /// Later layers merged on top, in order.
    pub overlays: Vec<Origin>,
    /// Hash of the merged entry, so deploys can tell when it's changed. For
    /// games it leaves out fields that don't affect what gets deployed.
    pub hash: ContentHash,
}

impl<T> Entry<T> {
    /// Every layer that contributed to the entry, e.g. `builtin + user`.
    pub fn origins(&self) -> String {
        std::iter::once(&self.origin)
            .chain(&self.overlays)
            .map(|o| o.to_string())
            .collect::<Vec<_>>()
            .join(" + ")
    }
}

/// Game definitions and other shared data, merged from several layers. Later
/// layers are merged over earlier ones:
///
/// 1. entries built into the binary
/// 2. installed registries, `<data>/registries/<name>/`, in name order
/// 3. directories listed in `$PATCHWORK_REGISTRY_PATH`, in order
/// 4. the user's own files under `<config>/`
///
/// Each layer is laid out as `<kind>/<id>.toml`. `games` is understood by the
/// core; any other kind (like `rulesets`) is kept as raw TOML for whichever
/// deployer or source wants it.
///
/// A file for an id that an earlier layer already defines is an overlay: its
/// tables are merged key by key into the earlier entry, and anything else
/// (strings, numbers, arrays) replaces what was there. So a user can change
/// one field of a builtin game without copying the rest, and still get
/// upstream fixes to everything else. An overlay that would leave the entry
/// invalid is reported and ignored.
#[derive(Debug, Default)]
pub struct Registry {
    games: BTreeMap<String, Entry<GameDef>>,
    /// Each game's merged TOML, which the next overlay is merged into.
    game_tables: BTreeMap<String, toml::Table>,
    data: BTreeMap<(String, String), Entry<toml::Table>>,
    /// Files that failed to load. One broken file shouldn't make every other
    /// game unusable, so these are reported rather than fatal.
    pub problems: Vec<Error>,
}

impl Registry {
    pub fn load(paths: &Paths) -> Result<Self> {
        let mut registry = Self::default();
        for (kind, id, text) in builtin::ENTRIES {
            let path = format!("<builtin>/{kind}/{id}.toml");
            registry.insert(Origin::Builtin, kind, id, text, Path::new(&path));
        }

        let (layers, problems) = Self::layers(paths);
        registry.problems.extend(problems);
        for (origin, dir) in layers {
            registry.load_layer(&dir, origin);
        }
        Ok(registry)
    }

    /// The directories of every layer after the built-in one, in the order
    /// they're loaded, each laid out as `<kind>/<id>.toml`. Other files
    /// that come with registries (like the GUI's themes) are found the same
    /// way. Also gives a problem for each `$PATCHWORK_REGISTRY_PATH` entry
    /// that isn't a directory.
    pub fn layers(paths: &Paths) -> (Vec<(Origin, PathBuf)>, Vec<Error>) {
        let mut layers = Vec::new();
        let mut problems = Vec::new();
        let registries = paths.registries();
        let mut names: Vec<_> = match fs::read_dir(&registries) {
            Ok(dir) => dir
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        names.sort();
        for name in names {
            layers.push((Origin::Registry(name.clone()), registries.join(&name)));
        }

        for dir in &paths.extra_registries {
            if !dir.is_dir() {
                problems.push(Error::invalid(format!(
                    "PATCHWORK_REGISTRY_PATH entry {} is not a directory",
                    dir.display()
                )));
                continue;
            }
            layers.push((Origin::Dir(dir.clone()), dir.clone()));
        }

        layers.push((Origin::User, paths.user_registry().to_owned()));
        (layers, problems)
    }

    fn load_layer(&mut self, root: &Path, origin: Origin) {
        let Ok(kinds) = fs::read_dir(root) else {
            return;
        };
        for kind_dir in kinds.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
            let kind = kind_dir.file_name().to_string_lossy().into_owned();
            let Ok(files) = fs::read_dir(kind_dir.path()) else {
                continue;
            };
            let mut files: Vec<_> = files
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
                .collect();
            files.sort();
            for path in files {
                let id = path.file_stem().unwrap_or_default().to_string_lossy();
                match fs::read_to_string(&path) {
                    Ok(text) => self.insert(origin.clone(), &kind, &id, &text, &path),
                    Err(source) => self.problems.push(Error::Io { path, source }),
                }
            }
        }
    }

    fn insert(&mut self, origin: Origin, kind: &str, id: &str, text: &str, path: &Path) {
        if let Err(e) = self.try_insert(origin, kind, id, text, path) {
            self.problems.push(e);
        }
    }

    fn try_insert(
        &mut self,
        origin: Origin,
        kind: &str,
        id: &str,
        text: &str,
        path: &Path,
    ) -> Result<()> {
        let table: toml::Table = toml::from_str(text).map_err(|e| Error::parse(path, e))?;
        if kind == "games" {
            return self.insert_game(origin, id, table, path);
        }
        let key = (kind.to_owned(), id.to_owned());
        let (value, origin, overlays) = match self.data.remove(&key) {
            Some(mut base) => {
                merge(&mut base.value, table);
                base.overlays.push(origin);
                (base.value, base.origin, base.overlays)
            }
            None => (table, origin, Vec::new()),
        };
        let hash = hash_table(&value, &[]);
        self.data.insert(
            key,
            Entry {
                value,
                origin,
                overlays,
                hash,
            },
        );
        Ok(())
    }

    fn insert_game(
        &mut self,
        origin: Origin,
        id: &str,
        mut table: toml::Table,
        path: &Path,
    ) -> Result<()> {
        // Checked before the full parse, so a newer file gets a clear "update
        // patchwork" error instead of a confusing field error. Overlays don't
        // need a schema, but are held to it if they have one.
        if let Some(schema) = table.get("schema") {
            let found = schema
                .as_integer()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| Error::parse(path, "`schema` must be a whole number"))?;
            if found > GAME_SCHEMA {
                return Err(Error::Schema {
                    kind: "game",
                    path: path.to_owned(),
                    found,
                    supported: GAME_SCHEMA,
                });
            }
        }
        if let Some(found) = table.get("id")
            && found.as_str() != Some(id)
        {
            return Err(Error::parse(
                path,
                format!("id {found} doesn't match the file name"),
            ));
        }
        if origin != Origin::Builtin {
            resolve_image(&mut table, path);
        }
        upgrade_game(&mut table);

        let (merged, origin, overlays) = match self.games.get(id) {
            Some(base) => {
                let mut merged = self.game_tables[id].clone();
                merge(&mut merged, table);
                let mut overlays = base.overlays.clone();
                overlays.push(origin);
                (merged, base.origin.clone(), overlays)
            }
            None => (table, origin, Vec::new()),
        };
        let value: GameDef = toml::Value::Table(merged.clone())
            .try_into()
            .map_err(|e| Error::parse(path, e))?;
        let hash = hash_table(&merged, COSMETIC_GAME_FIELDS);
        self.games.insert(
            id.to_owned(),
            Entry {
                value,
                origin,
                overlays,
                hash,
            },
        );
        self.game_tables.insert(id.to_owned(), merged);
        Ok(())
    }

    pub fn game(&self, id: &str) -> Result<&Entry<GameDef>> {
        self.games
            .get(id)
            .ok_or_else(|| Error::UnknownGame(id.to_owned()))
    }

    pub fn games(&self) -> impl Iterator<Item = &Entry<GameDef>> {
        self.games.values()
    }

    /// Adds an entry as a built-in one, without any files.
    #[cfg(test)]
    pub(crate) fn add_data(&mut self, kind: &str, id: &str, text: &str) {
        self.try_insert(Origin::Builtin, kind, id, text, Path::new("<test>"))
            .unwrap();
    }

    pub fn data(&self, kind: &str, id: &str) -> Option<&Entry<toml::Table>> {
        self.data.get(&(kind.to_owned(), id.to_owned()))
    }

    /// The ids of every entry of `kind`, e.g. the `rulesets` a game can use.
    pub fn data_ids<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a str> {
        self.data
            .keys()
            .filter(move |(k, _)| k == kind)
            .map(|(_, id)| id.as_str())
    }

    /// The hash of every entry that isn't a game, keyed by kind and id,
    /// except runners: how a game is started has no effect on what's deployed
    /// to it, like the `launch` in [`COSMETIC_GAME_FIELDS`].
    pub fn data_hashes(&self) -> impl Iterator<Item = (&str, &str, ContentHash)> {
        self.data
            .iter()
            .filter(|((kind, _), _)| kind != crate::launch::runner::KIND)
            .map(|((kind, id), entry)| (kind.as_str(), id.as_str(), entry.hash))
    }
}

/// Game fields that don't affect deploying, so changing them doesn't make
/// what was built for a game look out of date.
const COSMETIC_GAME_FIELDS: &[&str] = &[
    "name",
    "description",
    "image",
    "hidden",
    "accent",
    "detect",
    "launch",
];

/// Moves what game definitions used to have where it is now, so older
/// definitions (and overlays of them) keep working: `[deployer.plugins]`
/// is the Bethesda extension's `plugins` list, and `[fomod]` the FOMOD
/// extension's `installer`.
fn upgrade_game(table: &mut toml::Table) {
    if let Some(plugins) = table
        .get_mut("deployer")
        .and_then(|d| d.as_table_mut())
        .and_then(|d| d.remove("plugins"))
    {
        let (id, list) = crate::profile::LEGACY_PLUGINS;
        move_to_ext(table, id, list, plugins);
    }
    if let Some(fomod) = table.remove(LEGACY_FOMOD.0) {
        let (id, hook) = LEGACY_FOMOD;
        move_to_ext(table, id, hook, fomod);
    }
}

/// Where `[fomod]` went: the FOMOD extension's `installer` hooks.
const LEGACY_FOMOD: (&str, &str) = ("fomod", "installer");

/// Puts `value` in `table` as `[ext.<id>.<hook>]`, unless something's
/// already there.
fn move_to_ext(table: &mut toml::Table, id: &str, hook: &str, value: toml::Value) {
    let ext = table
        .entry("ext")
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(ext) = ext.as_table_mut() {
        let extension = ext
            .entry(id)
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        if let Some(extension) = extension.as_table_mut() {
            extension.entry(hook).or_insert(value);
        }
    }
}

/// Merges `overlay` into `base`: tables merge key by key, and any other value
/// replaces the old one.
fn merge(base: &mut toml::Table, overlay: toml::Table) {
    for (key, value) in overlay {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(base)), toml::Value::Table(overlay)) => merge(base, overlay),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

/// Makes a relative `image` path absolute, so it still points at the right
/// file once merged with entries from other directories.
fn resolve_image(table: &mut toml::Table, path: &Path) {
    let Some(toml::Value::String(image)) = table.get_mut("image") else {
        return;
    };
    if image.contains("://") || Path::new(image.as_str()).is_absolute() {
        return;
    }
    if let Some(dir) = path.parent() {
        *image = dir.join(image.as_str()).to_string_lossy().into_owned();
    }
}

/// Hashes a table independently of key order and formatting, so the hash only
/// changes when the content does.
fn hash_table(root: &toml::Table, skip: &[&str]) -> ContentHash {
    fn str(h: &mut blake3::Hasher, s: &str) {
        h.update(&(s.len() as u64).to_le_bytes());
        h.update(s.as_bytes());
    }
    fn table(h: &mut blake3::Hasher, t: &toml::Table, skip: &[&str]) {
        let mut entries: Vec<_> = t
            .iter()
            .filter(|(k, _)| !skip.contains(&k.as_str()))
            .collect();
        entries.sort_by_key(|(k, _)| k.as_str());
        h.update(b"t");
        h.update(&(entries.len() as u64).to_le_bytes());
        for (k, v) in entries {
            str(h, k);
            value(h, v);
        }
    }
    fn value(h: &mut blake3::Hasher, v: &toml::Value) {
        match v {
            toml::Value::String(s) => {
                h.update(b"s");
                str(h, s);
            }
            toml::Value::Integer(n) => {
                h.update(b"i");
                h.update(&n.to_le_bytes());
            }
            toml::Value::Float(f) => {
                h.update(b"f");
                h.update(&f.to_bits().to_le_bytes());
            }
            toml::Value::Boolean(b) => {
                h.update(&[b'b', u8::from(*b)]);
            }
            toml::Value::Datetime(d) => {
                h.update(b"d");
                str(h, &d.to_string());
            }
            toml::Value::Array(items) => {
                h.update(b"a");
                h.update(&(items.len() as u64).to_le_bytes());
                for item in items {
                    value(h, item);
                }
            }
            toml::Value::Table(t) => table(h, t, &[]),
        }
    }
    let mut h = blake3::Hasher::new();
    table(&mut h, root, skip);
    h.finalize().into()
}

#[cfg(test)]
mod tests;
