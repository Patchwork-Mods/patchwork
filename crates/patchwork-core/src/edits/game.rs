//! What the user has changed about how a game is shown: its name, picture
//! and color, in place of what its definition says.
//!
//! Like a mod's [`super::MOD_FIELDS`], each thing that can be changed is a
//! [`GameField`], listed in [`GAME_FIELDS`], and frontends build their
//! editors from the list. Unlike a mod's, the changes aren't kept in a
//! profile but in the user's own registry file for the game,
//! `<config>/games/<id>.toml`, which the registry merges over every other
//! layer (see [`crate::registry::Registry`]). So they're plain game
//! definition keys, and anything else the user has written in that file by
//! hand (and its comments) is kept.

use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::DocumentMut;

use super::{FieldInfo, FieldKind, FieldValue};
use crate::error::{Error, IoContext, Result};
use crate::paths::Paths;

/// Something about a game the user can change.
pub struct GameField {
    /// What frontends name it by.
    pub id: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
    /// The game definition key it's saved as.
    pub key: &'static str,
}

/// Everything about a game that can be changed, in the order to show them.
/// They're all ones that don't change what's deployed, so changing them
/// doesn't make a deploy look out of date.
pub static GAME_FIELDS: &[GameField] = &[
    GameField {
        id: "name",
        label: "Name",
        kind: FieldKind::Text,
        key: "name",
    },
    GameField {
        id: "picture",
        label: "Picture",
        kind: FieldKind::Picture,
        key: "image",
    },
    GameField {
        id: "color",
        label: "Accent Color",
        kind: FieldKind::Color,
        key: "accent",
    },
];

/// The field with id `id`, if there is one.
pub fn game_field(id: &str) -> Option<&'static GameField> {
    GAME_FIELDS.iter().find(|f| f.id == id)
}

/// The user's own registry file for a game, `<config>/games/<id>.toml`,
/// as it's being changed.
pub struct UserGameFile {
    path: PathBuf,
    doc: DocumentMut,
}

impl UserGameFile {
    /// Game `id`'s file, or an empty one if there's none yet.
    pub fn load(paths: &Paths, id: &str) -> Result<Self> {
        let path = paths
            .user_registry()
            .join("games")
            .join(format!("{id}.toml"));
        let doc = match fs::read_to_string(&path) {
            Ok(text) => text.parse().map_err(|e| Error::parse(&path, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => DocumentMut::new(),
            Err(e) => return Err(e).at(&path),
        };
        Ok(Self { path, doc })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// What the file says for `field`, if it says something it can take.
    /// A picture's relative path is given from the file's folder, as the
    /// registry reads it.
    pub fn get(&self, field: &GameField) -> Option<FieldValue> {
        let text = self.doc.get(field.key)?.as_str()?;
        let value = field
            .kind
            .check(field.id, FieldValue::Text(text.to_owned()));
        match value.ok().flatten()? {
            FieldValue::Text(t) if field.kind == FieldKind::Picture => {
                let relative = !t.contains("://") && !Path::new(&t).is_absolute();
                Some(FieldValue::Text(match self.path.parent() {
                    Some(dir) if relative => dir.join(t).to_string_lossy().into_owned(),
                    _ => t,
                }))
            }
            value => Some(value),
        }
    }

    /// Sets `field` to `value`, which should have been through
    /// [`FieldKind::check`], or with `None` leaves it to the other layers.
    pub fn set(&mut self, field: &GameField, value: Option<FieldValue>) {
        match value {
            Some(FieldValue::Text(text)) => {
                self.doc[field.key] = toml_edit::value(text);
            }
            Some(FieldValue::List(items)) => {
                self.doc[field.key] =
                    toml_edit::value(items.into_iter().collect::<toml_edit::Array>());
            }
            None => {
                self.doc.remove(field.key);
            }
        }
    }

    /// The file's contents as a table, as the registry would read them.
    pub fn table(&self) -> Result<toml::Table> {
        toml::from_str(&self.doc.to_string()).map_err(|e| Error::parse(&self.path, e))
    }

    /// Writes the file, or removes it if it's been left with nothing in it.
    pub fn save(&self) -> Result<()> {
        let text = self.doc.to_string();
        if text.trim().is_empty() {
            return match fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e).at(&self.path),
                _ => Ok(()),
            };
        }
        crate::fsutil::atomic_write(&self.path, text.as_bytes())
    }

    /// Each of [`GAME_FIELDS`], with what the game is as every other layer
    /// has it (`before`, see [`crate::registry::Registry::game_before_user`])
    /// and what this file changes it to.
    pub fn fields(&self, before: Option<&toml::Table>) -> Vec<FieldInfo> {
        GAME_FIELDS
            .iter()
            .map(|f| FieldInfo {
                id: f.id,
                label: f.label,
                help: "",
                kind: f.kind,
                shows: true,
                // tidied like the user's, so they can be compared
                source: before
                    .and_then(|t| t.get(f.key)?.as_str())
                    .and_then(|s| f.kind.check(f.id, FieldValue::Text(s.to_owned())).ok()?),
                edited: self.get(f),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
