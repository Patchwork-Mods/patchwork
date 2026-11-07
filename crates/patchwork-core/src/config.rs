//! Config files as forms: settings that mods (or the game) read from a
//! file, which the user changes from patchwork rather than by editing the
//! file.
//!
//! Which files are config, and how to read them, is data: a game
//! definition's or ruleset's `[[config]]` (see [`ConfigDef`]) names a
//! [`ConfigFormat`] for the files it matches. Formats are hooks that turn a
//! file's bytes into [`Group`]s of [`Field`](crate::options::Field)s and
//! write changed values back into the bytes; they never touch the
//! filesystem.
//!
//! A profile keeps only the settings the user changed, for each file (see
//! [`Profile::config`](crate::profile::Profile::config)), not copies of the
//! files. Deploying applies them over whatever the file is: the copy a mod
//! ships, or the one the game made in its own folder. The result is written
//! into patchwork's own folder and deployed as a copy the game is expected
//! to rewrite. Before deploying again (or undeploying), whatever the game
//! changed in those copies is read back into the deployed profile, so
//! settings changed in game aren't lost.

use std::collections::BTreeMap;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::deploy::{Dest, RootId};
use crate::game::FilePattern;
use crate::options::{Answer, Group, Value};
use crate::{Error, Result};

/// Reads and writes one kind of config file, registered with
/// [`Registrar::config_format`](crate::ext::Registrar::config_format) as
/// `<extension>.<name>`, which `[[config]]` data names it by.
pub trait ConfigFormat: Send + Sync {
    /// The settings in `file`, as groups of fields set to what the file
    /// has. Each group's id and each field's id within it are what
    /// [`ConfigFormat::write`] gets the values back by, and what profiles
    /// save them under, so they should stay the same as long as the
    /// setting does.
    fn read(&self, ctx: &ConfigContext<'_>, file: &[u8]) -> Result<Contents>;

    /// `file` with each of `values`' settings set, and everything else
    /// left as it is. A setting `file` hasn't got is added if the format
    /// can, or left out with an error otherwise. Values have been checked
    /// against the fields `read` gave them.
    fn write(&self, ctx: &ConfigContext<'_>, file: &[u8], values: &[Answer]) -> Result<Vec<u8>>;
}

/// What a [`ConfigFormat`] gets to work with.
pub struct ConfigContext<'a> {
    path: &'a Dest,
    config: &'a toml::Table,
}

impl<'a> ConfigContext<'a> {
    pub fn new(path: &'a Dest, config: &'a toml::Table) -> Self {
        Self { path, config }
    }

    /// Where the file is deployed.
    pub fn path(&self) -> &Dest {
        self.path
    }

    /// The format's `[ext.<extension>.<name>]` table in the game
    /// definition, or an empty one.
    pub fn config(&self) -> &toml::Table {
        self.config
    }
}

/// A config file's settings, from [`ConfigFormat::read`].
#[derive(Debug, Clone, Default)]
pub struct Contents {
    /// What the file is for, if it says, like the name of the mod that
    /// made it. Frontends otherwise show the file's name.
    pub title: Option<String>,
    /// Each with [`GroupKind::Fields`](crate::options::GroupKind::Fields).
    pub groups: Vec<Group>,
    /// Whole sentences about what couldn't be read.
    pub warnings: Vec<String>,
}

impl Contents {
    /// Each setting's value in the file, by group and field id.
    pub fn values(&self) -> Edits {
        let mut values = Edits::new();
        for group in &self.groups {
            for field in group.fields() {
                values
                    .entry(group.id.clone())
                    .or_default()
                    .insert(field.id.clone(), field.value.clone());
            }
        }
        values
    }
}

/// Settings changed in one config file, by group id and then field id.
pub type Edits = BTreeMap<String, BTreeMap<String, Value>>;

/// `edits` as answers, a group at a time.
pub fn answers(edits: &Edits) -> Vec<Answer> {
    edits
        .iter()
        .map(|(group, values)| Answer::values(group.clone(), values.clone()))
        .collect()
}

/// Which files are config, and what reads them: a `[[config]]` table in a
/// game definition or ruleset.
///
/// ```toml
/// [[config]]
/// files = "BepInEx/config/**/*.cfg"   # or a list; matched ignoring case
/// format = "bepinex.cfg"              # <extension>.<name>
/// root = "game"                       # the default
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigDef {
    #[serde(with = "one_or_many")]
    pub files: Vec<FilePattern>,
    pub format: String,
    #[serde(default = "RootId::game", skip_serializing_if = "RootId::is_game")]
    pub root: RootId,
}

/// Matches paths against several [`ConfigDef`]s; the first that matches a
/// path says what reads it.
pub struct ConfigDefs {
    defs: Vec<(ConfigDef, GlobSet)>,
}

impl ConfigDefs {
    pub fn new(defs: Vec<ConfigDef>) -> Result<Self> {
        let defs = defs
            .into_iter()
            .map(|def| {
                let mut set = GlobSetBuilder::new();
                for pattern in &def.files {
                    let glob = GlobBuilder::new(pattern.as_str())
                        .case_insensitive(true)
                        .literal_separator(true)
                        .build()
                        .map_err(|e| Error::invalid(format!("bad config pattern: {e}")))?;
                    set.add(glob);
                }
                let set = set
                    .build()
                    .map_err(|e| Error::invalid(format!("bad config patterns: {e}")))?;
                Ok((def, set))
            })
            .collect::<Result<_>>()?;
        Ok(Self { defs })
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// What says `dest` is config, if anything does.
    pub fn find(&self, dest: &Dest) -> Option<&ConfigDef> {
        self.defs
            .iter()
            .find(|(def, set)| def.root == dest.root && set.is_match(dest.path.as_str()))
            .map(|(def, _)| def)
    }

    /// The folders to look in for config files, by root: each pattern's
    /// folders up to the first with a wildcard in it.
    pub fn folders(&self) -> Vec<(RootId, String)> {
        let mut folders: Vec<(RootId, String)> = Vec::new();
        for (def, _) in &self.defs {
            for pattern in &def.files {
                let fixed: Vec<&str> = pattern
                    .as_str()
                    .split('/')
                    .take_while(|part| !part.contains(['*', '?', '[', '{']))
                    .collect();
                // The last part is the file's name, unless it's a wildcard.
                let dirs = if fixed.len() == pattern.as_str().split('/').count() {
                    &fixed[..fixed.len() - 1]
                } else {
                    &fixed[..]
                };
                let folder = (def.root.clone(), dirs.join("/"));
                // One folder inside another is walked along with it.
                let covered = folders.iter().any(|(root, f)| {
                    *root == folder.0
                        && (f.is_empty()
                            || folder.1 == *f
                            || folder.1.starts_with(&format!("{f}/")))
                });
                if !covered {
                    folders.retain(|(root, f)| {
                        !(*root == folder.0
                            && (folder.1.is_empty() || f.starts_with(&format!("{}/", folder.1))))
                    });
                    folders.push(folder);
                }
            }
        }
        folders
    }
}

/// `edits` checked against `contents`' fields: values are kept as the
/// fields keep them, and ones for settings it hasn't got or that don't
/// fit are left out, with what's wrong with each.
pub fn checked_edits(contents: &Contents, edits: &Edits) -> (Edits, Vec<String>) {
    let mut kept = Edits::new();
    let mut problems = Vec::new();
    for (group_id, values) in edits {
        let group = contents.groups.iter().find(|g| g.id == *group_id);
        for (id, value) in values {
            let field = group.and_then(|g| g.fields().iter().find(|f| f.id == *id));
            let Some(field) = field else {
                problems.push(format!("there's no setting `{group_id}/{id}`"));
                continue;
            };
            match field.check(value) {
                Ok(value) => {
                    kept.entry(group_id.clone())
                        .or_default()
                        .insert(id.clone(), value);
                }
                Err(e) => problems.push(e),
            }
        }
    }
    (kept, problems)
}

/// The settings in `now` that aren't what they are in `before`.
pub fn changed(before: &Edits, now: &Edits) -> Edits {
    let mut changed = Edits::new();
    for (group, values) in now {
        for (id, value) in values {
            if before.get(group).and_then(|v| v.get(id)) != Some(value) {
                changed
                    .entry(group.clone())
                    .or_default()
                    .insert(id.clone(), value.clone());
            }
        }
    }
    changed
}

mod one_or_many {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::game::FilePattern;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(FilePattern),
        Many(Vec<FilePattern>),
    }

    pub fn serialize<S: Serializer>(v: &[FilePattern], s: S) -> Result<S::Ok, S::Error> {
        match v {
            [one] => one.serialize(s),
            many => many.serialize(s),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<FilePattern>, D::Error> {
        Ok(match OneOrMany::deserialize(d)? {
            OneOrMany::One(one) => vec![one],
            OneOrMany::Many(many) => many,
        })
    }
}

#[cfg(test)]
mod tests;
