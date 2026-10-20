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
    pub origin: Origin,
    /// Hash of the file's text, recorded in lockfiles so a shared profile can
    /// tell when it's being used with a different definition.
    pub hash: ContentHash,
}

/// Game definitions and other shared data, merged from several layers. Later
/// layers override earlier ones:
///
/// 1. entries built into the binary
/// 2. installed registries, `<data>/registries/<name>/`, in name order
/// 3. directories listed in `$PATCHWORK_REGISTRY_PATH`, in order
/// 4. the user's own files under `<config>/`
///
/// Each layer is laid out as `<kind>/<id>.toml`. `games` is understood by the
/// core; any other kind (like `rulesets`) is kept as raw TOML for whichever
/// deployer or source wants it.
#[derive(Debug, Default)]
pub struct Registry {
    games: BTreeMap<String, Entry<GameDef>>,
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
            registry.load_layer(&registries.join(&name), Origin::Registry(name));
        }

        for dir in &paths.extra_registries {
            if !dir.is_dir() {
                registry.problems.push(Error::invalid(format!(
                    "PATCHWORK_REGISTRY_PATH entry {} is not a directory",
                    dir.display()
                )));
                continue;
            }
            registry.load_layer(dir, Origin::Dir(dir.clone()));
        }

        registry.load_layer(paths.user_registry(), Origin::User);
        Ok(registry)
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
        let hash = ContentHash::of(text.as_bytes());
        if kind == "games" {
            let schema = toml::from_str::<SchemaOnly>(text)
                .map_err(|e| Error::parse(path, e))?
                .schema;
            if schema > GAME_SCHEMA {
                return Err(Error::Schema {
                    kind: "game",
                    path: path.to_owned(),
                    found: schema,
                    supported: GAME_SCHEMA,
                });
            }
            let value: GameDef = toml::from_str(text).map_err(|e| Error::parse(path, e))?;
            if value.id != id {
                return Err(Error::parse(
                    path,
                    format!("id `{}` doesn't match the file name", value.id),
                ));
            }
            self.games.insert(
                id.to_owned(),
                Entry {
                    value,
                    origin,
                    hash,
                },
            );
        } else {
            let value: toml::Table = toml::from_str(text).map_err(|e| Error::parse(path, e))?;
            self.data.insert(
                (kind.to_owned(), id.to_owned()),
                Entry {
                    value,
                    origin,
                    hash,
                },
            );
        }
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

    /// The hash of every entry that isn't a game, keyed by kind and id.
    pub fn data_hashes(&self) -> impl Iterator<Item = (&str, &str, ContentHash)> {
        self.data
            .iter()
            .map(|((kind, id), entry)| (kind.as_str(), id.as_str(), entry.hash))
    }
}

/// Checked before the full parse, so a newer file gets a clear "update
/// patchwork" error instead of a confusing field error.
#[derive(serde::Deserialize)]
struct SchemaOnly {
    schema: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_load_cleanly() {
        let tmp = tempfile::tempdir().unwrap();
        let registry = Registry::load(&Paths::rooted(tmp.path())).unwrap();
        assert!(registry.problems.is_empty(), "{:?}", registry.problems);
        assert_eq!(
            registry.game("lethal-company").unwrap().origin,
            Origin::Builtin
        );
        assert!(registry.data("rulesets", "bepinex").is_some());
    }

    #[test]
    fn extra_registry_dirs_override_builtins_but_not_user() {
        let tmp = tempfile::tempdir().unwrap();
        let mut paths = Paths::rooted(tmp.path());
        let dev = tmp.path().join("dev");
        let missing = tmp.path().join("missing");
        paths.extra_registries = vec![dev.clone(), missing];

        let def = |name: &str, id: &str| {
            format!("schema = 1\nid = \"{id}\"\nname = \"{name}\"\n[deployer]\nkind = \"rules\"\n")
        };
        fs::create_dir_all(dev.join("games")).unwrap();
        fs::write(
            dev.join("games/lethal-company.toml"),
            def("Dev", "lethal-company"),
        )
        .unwrap();
        fs::write(dev.join("games/new-game.toml"), def("New", "new-game")).unwrap();
        let user_games = paths.user_registry().join("games");
        fs::create_dir_all(&user_games).unwrap();
        fs::write(user_games.join("new-game.toml"), def("Mine", "new-game")).unwrap();

        let registry = Registry::load(&paths).unwrap();
        let lc = registry.game("lethal-company").unwrap();
        assert_eq!(lc.origin, Origin::Dir(dev));
        assert_eq!(lc.value.name, "Dev");
        assert_eq!(registry.game("new-game").unwrap().origin, Origin::User);
        assert_eq!(registry.problems.len(), 1, "{:?}", registry.problems);
    }

    #[test]
    fn layers_override_and_problems_are_collected() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::rooted(tmp.path());
        let games = paths.user_registry().join("games");
        fs::create_dir_all(&games).unwrap();
        fs::write(
            games.join("lethal-company.toml"),
            "schema = 1\nid = \"lethal-company\"\nname = \"Mine\"\n[deployer]\nkind = \"rules\"\n",
        )
        .unwrap();
        fs::write(games.join("future.toml"), "schema = 99\nid = \"future\"\n").unwrap();
        fs::write(games.join("typo.toml"), "schema = 1\nid = \"other\"\n").unwrap();

        let registry = Registry::load(&paths).unwrap();
        let lc = registry.game("lethal-company").unwrap();
        assert_eq!(lc.origin, Origin::User);
        assert_eq!(lc.value.name, "Mine");
        assert_eq!(registry.problems.len(), 2);
        assert!(matches!(
            registry.problems[0],
            Error::Schema { found: 99, .. }
        ));
    }
}
