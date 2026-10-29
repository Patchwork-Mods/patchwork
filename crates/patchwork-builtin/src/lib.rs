//! The extensions that come with patchwork, each bundling what one game or
//! family of games needs. Frontends register the ones they want with
//! [`patchwork_core::Builder::extension`].
//!
//! For now they gather up the sources, deployers, provisioners and formats
//! from the crates those live in; those will move into a crate per
//! extension. The ids can't change once released, since game definitions
//! and profiles name them.

mod fomod;

pub use fomod::FOMOD_EXTENSION;
use fomod::FomodInstaller;
use patchwork_core::ext::{Extension, Registrar};
use patchwork_deployers::{
    BETHESDA_EXTENSION, BETHESDA_LIMIT, BETHESDA_LIST, BethesdaDeployer, BethesdaLoadOrder,
    PluginLimit, RulesDeployer, SmfDeployer,
};
use patchwork_formats::{Archive, R2modman, ThunderstoreCode, UrlList};
use patchwork_provisioners::MinecraftProvisioner;
use patchwork_sources::{LocalSource, Modrinth, Nexus, Thunderstore, UrlSource};

/// What isn't about any game in particular: local files and URLs, the
/// data-driven `rules` deployer, and patchwork's own sharing formats.
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
            .format(UrlList);
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

/// Games on Bethesda's engines: Skyrim, Fallout and the like. Its
/// `plugins` list is the order the game loads mods' plugins in, and its
/// `limit` check warns when more are enabled than the game can load.
pub struct Bethesda;

impl Extension for Bethesda {
    /// The deployer looks for its load order under this id.
    fn id(&self) -> &'static str {
        BETHESDA_EXTENSION
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(BethesdaDeployer)
            .ordered_list(BETHESDA_LIST, BethesdaLoadOrder)
            .check(BETHESDA_LIMIT, PluginLimit);
    }
}

/// FOMOD installers, which most of Bethesda games' bigger mods come with:
/// the options picked in a mod's installer say which of its files are
/// installed where. Games turn it on with `[ext.fomod.installer]`.
pub struct Fomod;

impl Extension for Fomod {
    fn id(&self) -> &'static str {
        FOMOD_EXTENSION
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.mod_transform("installer", FomodInstaller)
            .mod_options("installer", FomodInstaller);
    }
}

/// HITMAN World of Assassination, with mods made for Simple Mod Framework.
pub struct Smf;

impl Extension for Smf {
    fn id(&self) -> &'static str {
        "smf"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(SmfDeployer);
    }
}

pub struct Minecraft;

impl Extension for Minecraft {
    fn id(&self) -> &'static str {
        "minecraft"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.provisioner(MinecraftProvisioner);
    }
}
