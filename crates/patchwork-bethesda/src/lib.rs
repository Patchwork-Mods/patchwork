//! Games on Bethesda's engines (Skyrim, Fallout and the like), and the FOMOD
//! installers most of their bigger mods come with.
//!
//! Two extensions: [`Bethesda`] has the deployer, the load order and the
//! plugin limit, and [`Fomod`] the installers. FOMOD is its own extension so
//! that other games can turn it on without the rest.

mod fomod;
mod plugins;

use patchwork_core::ext::{Extension, Registrar};

use fomod::{FOMOD_EXTENSION, FomodInstaller};
use plugins::{BethesdaDeployer, BethesdaLoadOrder, EXTENSION, LIMIT, LIST, PluginLimit};

/// Games on Bethesda's engines. Its `plugins` list is the order the game
/// loads mods' plugins in, and its `limit` check warns when more are enabled
/// than the game can load.
pub struct Bethesda;

impl Extension for Bethesda {
    /// The deployer looks for its load order under this id.
    fn id(&self) -> &'static str {
        EXTENSION
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(BethesdaDeployer)
            .ordered_list(LIST, BethesdaLoadOrder)
            .check(LIMIT, PluginLimit);
    }
}

/// FOMOD installers: the options picked in a mod's installer say which of
/// its files are installed where. Games turn it on with
/// `[ext.fomod.installer]`.
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
