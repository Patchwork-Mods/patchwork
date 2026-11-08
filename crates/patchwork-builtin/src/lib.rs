//! The extensions that come with patchwork. Frontends register them all
//! with [`register`].
//!
//! The ones that aren't about any game are here, gathering up the sources,
//! deployers and formats from the crates those live in. Games' own are in
//! a crate each, behind a feature of the same name (all on by default):
//! `bethesda` ([`Bethesda`], [`Fomod`] and [`Loot`]), `helldivers`
//! ([`Helldivers`]) and `smf` ([`Smf`]). The ids
//! can't change once released, since game definitions and profiles name
//! them.

mod bepinex;
mod plain;

pub use bepinex::BepInExConfig;
use patchwork_core::Builder;
use patchwork_core::ext::{Extension, Registrar};
use patchwork_deployers::RulesDeployer;
use patchwork_formats::{Archive, R2modman, ThunderstoreCode, UrlList};
use patchwork_sources::{GitHub, LocalSource, ModIo, Modrinth, Nexus, Thunderstore, UrlSource};
pub use plain::{IniConfig, JsonConfig, TomlConfig};

#[cfg(feature = "bethesda")]
pub use patchwork_bethesda::{Bethesda, Fomod, Loot};
#[cfg(feature = "helldivers")]
pub use patchwork_helldivers::Helldivers;
#[cfg(feature = "smf")]
pub use patchwork_smf::Smf;

/// Adds every extension this build has to `builder`.
pub fn register(builder: Builder) -> Builder {
    let builder = builder
        .extension(Generic)
        .extension(ThunderstoreExtension)
        .extension(NexusExtension)
        .extension(ModrinthExtension)
        .extension(GitHubExtension)
        .extension(ModIoExtension)
        .extension(BepInEx);
    #[cfg(feature = "bethesda")]
    let builder = builder.extension(Bethesda).extension(Fomod).extension(Loot);
    #[cfg(feature = "helldivers")]
    let builder = builder.extension(Helldivers);
    #[cfg(feature = "smf")]
    let builder = builder.extension(Smf);
    builder
}

/// What isn't about any game in particular: local files and URLs, the
/// data-driven `rules` deployer, patchwork's own sharing formats, and
/// plain INI, JSON and TOML config files (`generic.ini`, `generic.json`
/// and `generic.toml`).
pub struct Generic;

impl Extension for Generic {
    fn id(&self) -> &'static str {
        "generic"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.source(LocalSource)
            .source(UrlSource)
            .deployer(RulesDeployer)
            .format(Archive)
            .format(UrlList)
            .config_format("ini", IniConfig)
            .config_format("json", JsonConfig)
            .config_format("toml", TomlConfig);
    }
}

/// Thunderstore, and the r2modman profile codes that go with it.
pub struct ThunderstoreExtension;

impl Extension for ThunderstoreExtension {
    fn id(&self) -> &'static str {
        "thunderstore"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.source(Thunderstore)
            .format(R2modman)
            .format(ThunderstoreCode::default());
    }
}

pub struct NexusExtension;

impl Extension for NexusExtension {
    fn id(&self) -> &'static str {
        "nexus"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.source(Nexus::default());
    }
}

pub struct ModrinthExtension;

impl Extension for ModrinthExtension {
    fn id(&self) -> &'static str {
        "modrinth"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.source(Modrinth);
    }
}

/// Mods from the files on GitHub repos' releases, for any game.
pub struct GitHubExtension;

impl Extension for GitHubExtension {
    fn id(&self) -> &'static str {
        "github"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.source(GitHub);
    }
}

/// mod.io, for games that also load its mods from a folder of their own.
pub struct ModIoExtension;

impl Extension for ModIoExtension {
    fn id(&self) -> &'static str {
        "modio"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.source(ModIo);
    }
}

/// BepInEx, the mod loader most Thunderstore games use: reading and
/// writing its config files (`bepinex.cfg`), which its ruleset says are
/// config.
pub struct BepInEx;

impl Extension for BepInEx {
    fn id(&self) -> &'static str {
        "bepinex"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.config_format("cfg", BepInExConfig);
    }
}
