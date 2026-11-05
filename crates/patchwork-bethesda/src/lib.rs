//! Games on Bethesda's engines (Skyrim, Fallout and the like), and the FOMOD
//! installers most of their bigger mods come with.
//!
//! Three extensions: [`Bethesda`] has the deployer, the load order and the
//! plugin limit, [`Fomod`] the installers, and [`Loot`] sorting the load
//! order with LOOT's masterlist. FOMOD is its own extension so that other
//! games can turn it on without the rest, and LOOT so that games LOOT
//! doesn't know can leave it out.

mod fomod;
mod loot;
mod plugins;

use patchwork_core::ext::{Extension, Registrar};

use fomod::{FOMOD_EXTENSION, FomodInstaller};
use loot::{LOOT_EXTENSION, LootSort, SORT};
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

/// Sorting the load order with LOOT's masterlist, as the `sort` action.
/// Games turn it on with `[ext.loot.sort]`, which says where their
/// masterlist is; it's only on where the Bethesda load order is too.
pub struct Loot;

impl Extension for Loot {
    fn id(&self) -> &'static str {
        LOOT_EXTENSION
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.uses("bethesda.plugins").action(SORT, LootSort);
    }
}
