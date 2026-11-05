//! Deployers that aren't about any game in particular. Games' own deployers
//! live in their extensions' crates (`patchwork-bethesda`, `patchwork-smf`).

mod rules;

pub use rules::RulesDeployer;
