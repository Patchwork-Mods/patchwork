//! Setting up what a game needs to run that isn't a mod, such as the game
//! itself and its mod loader, for games whose profiles patchwork can start
//! in instances of their own (see [`crate::game::ProvisionSpec`]).
//!
//! A [`Provisioner`] works out what a profile's picks for the game's
//! `[env]` need: the files to download and how to start the game with
//! them. Like a source, it may ask the internet what a setup is made of,
//! but it only describes the files; the host downloads, checks and places
//! them. They go in a folder shared by every profile set up by the same
//! kind of provisioner (see [`crate::paths::Paths::runtime`]), since
//! profiles on the same setup need the same files, and most of them
//! (libraries, say) are shared between setups too.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::deploy::Os;
use crate::environment::GameEnv;
use crate::game::GameDef;
use crate::relpath::RelPath;

pub struct ProvisionContext<'a> {
    pub game: &'a GameDef,
    /// The rest of the game definition's `[provision]` table.
    pub config: &'a toml::Table,
    /// What the profile picks for the game's `[env]`.
    pub env: &'a GameEnv,
    /// What it's being set up to run on.
    pub os: Os,
    /// As in [`std::env::consts::ARCH`].
    pub arch: &'a str,
}

/// Knows how to set up a game to run, for some kind of game.
#[async_trait]
pub trait Provisioner: Send + Sync {
    fn kind(&self) -> &str;

    /// What's needed to run the game as `ctx.env` says.
    async fn plan(&self, ctx: &ProvisionContext<'_>) -> Result<Setup>;
}

/// Everything a game needs to run as a profile picks, from
/// [`Provisioner::plan`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setup {
    /// Names it among its provisioner's setups, so it has to be fit for a
    /// file name. Profiles that pick the same get the same id.
    pub id: String,
    pub files: Vec<RuntimeFile>,
    pub launch: Launch,
}

/// A file to download into the runtime folder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeFile {
    /// Where it goes in the runtime folder. A file that's already there
    /// (and the right size, if `size` is known) isn't downloaded again.
    pub path: RelPath,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Checked once it's downloaded, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    /// Whether it's a zip whose files the game needs unpacked as well.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unpack: Option<Unpack>,
    /// Whether it's a program, to be marked runnable where that's needed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub executable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Unpack {
    /// The folder in the runtime folder they go in.
    pub to: RelPath,
    /// Leaves out paths in the zip that start with any of these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
}

/// How to start a game that's been set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "via", rename_all = "camelCase")]
pub enum Launch {
    Java(JavaLaunch),
}

/// Running a class with Java.
///
/// Arguments can have `${name}` placeholders in them. Those named in
/// `vars` and `dirs` are filled in from them; the rest are left to
/// whatever starts the game (where it's started in, and who's signed in,
/// say), with `${classpath}` being `classpath` joined up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaLaunch {
    /// The major version of Java it needs, if it says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java: Option<u32>,
    /// The Java to run it with, a file in the runtime folder, if the
    /// provisioner downloads one. Otherwise it's the system's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_exe: Option<RelPath>,
    pub main_class: String,
    /// Files in the runtime folder.
    pub classpath: Vec<RelPath>,
    pub jvm_args: Vec<String>,
    pub game_args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub vars: BTreeMap<String, String>,
    /// Placeholders that are folders in the runtime folder. They're made
    /// when it's set up, if no file goes in them.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dirs: BTreeMap<String, RelPath>,
}
