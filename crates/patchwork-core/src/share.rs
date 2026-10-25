//! Exporting and importing profiles.
//!
//! Each way of sharing a profile (a patchwork zip, a list of URLs, an
//! r2modman code, ...) is a [`ProfileFormat`]. Formats convert between bytes
//! and a [`SharedProfile`]; the host takes care of locking the profile before
//! an export and of turning a `SharedProfile` into a real profile on import.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use crate::event::{Event, Reporter};
use crate::game::GameDef;
use crate::hash::ContentHash;
use crate::lock::Lockfile;
use crate::profile::ProfileMod;
use crate::settings::Settings;
use crate::source::{Source, SourceContext};
use crate::{Error, Result};

/// A profile on its way to or from another machine. Formats fill in as much
/// as they can carry; most can't carry everything.
#[derive(Debug, Clone, Default)]
pub struct SharedProfile {
    pub game: Option<String>,
    pub name: Option<String>,
    /// The profile's id, so a re-import can be matched up with the original.
    pub id: Option<Uuid>,
    pub mods: Vec<ProfileMod>,
    /// The profile's [`Profile::hidden_tags`](crate::profile::Profile::hidden_tags),
    /// or `None` if the format can't carry them, in which case an import
    /// leaves the existing profile's alone.
    pub hidden_tags: Option<Vec<String>>,
    /// Only present on export when the format asks for it with
    /// [`ProfileFormat::needs_lock`].
    pub lock: Option<Lockfile>,
    /// Artifacts that can't be fetched again elsewhere (local mods), keyed by
    /// the hash they're locked at.
    pub bundled: BTreeMap<ContentHash, Bundled>,
}

/// An artifact that travels with a profile.
#[derive(Debug, Clone)]
pub struct Bundled {
    /// The file or directory. On export this is in the store; on import it's
    /// wherever the format unpacked it to under [`FormatContext::scratch`].
    pub path: PathBuf,
    /// The artifact's original file name, which the store doesn't keep.
    pub name: String,
}

pub struct FormatContext<'a> {
    /// The profile's game on export. On import, the game the user asked for,
    /// or the only one the format supports.
    pub game: Option<&'a GameDef>,
    pub sources: &'a BTreeMap<String, Arc<dyn Source>>,
    /// The user's settings for `sources`.
    pub settings: Arc<Settings>,
    pub reporter: &'a dyn Reporter,
    /// An empty directory for unpacking bundled artifacts into on import.
    pub scratch: &'a Path,
}

impl FormatContext<'_> {
    /// The game, for formats that can't say which game a profile is for.
    pub fn game(&self) -> Result<&GameDef> {
        self.game.ok_or_else(|| {
            Error::invalid("couldn't tell which game this profile is for; pass --game")
        })
    }

    /// A source and its context for the current game, if it's registered.
    pub fn source(&self, id: &str) -> Result<Option<(&dyn Source, SourceContext<'_>)>> {
        let game = self.game()?;
        Ok(self.sources.get(id).map(|source| {
            let ctx = SourceContext {
                game,
                config: game.sources.get(id),
                settings: self.settings.source(id),
                env: None,
            };
            (source.as_ref(), ctx)
        }))
    }

    pub fn warn(&self, message: impl Into<String>) {
        self.reporter.report(Event::Warning(message.into()));
    }
}

/// A way of sharing profiles.
#[async_trait]
pub trait ProfileFormat: Send + Sync {
    /// What users pass to `--format`.
    fn id(&self) -> &str;

    /// One line for `patchwork profile formats`.
    fn description(&self) -> &str;

    /// The extension for exported files, or `None` if exports are short text
    /// meant to be printed and pasted (a code, a list of URLs).
    fn extension(&self) -> Option<&str>;

    /// Whether this format can hold profiles for `game`; e.g. Thunderstore
    /// codes only make sense for games on Thunderstore.
    fn supports(&self, _game: &GameDef) -> bool {
        true
    }

    /// Whether the profile has to be locked before it's exported, which
    /// fills in [`SharedProfile::lock`] and [`SharedProfile::bundled`].
    fn needs_lock(&self) -> bool {
        false
    }

    /// Whether `input` looks like this format, for picking one on import
    /// when the user doesn't say.
    fn detect(&self, input: &[u8]) -> bool;

    async fn export(&self, ctx: &FormatContext<'_>, profile: &SharedProfile) -> Result<Vec<u8>>;

    async fn import(&self, ctx: &FormatContext<'_>, input: &[u8]) -> Result<SharedProfile>;
}

/// Turns a name from elsewhere (which might be `My Cool Pack!`) into a valid
/// profile name.
pub fn sanitize_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "imported".to_owned()
    } else {
        out.to_owned()
    }
}

#[cfg(test)]
mod tests;
