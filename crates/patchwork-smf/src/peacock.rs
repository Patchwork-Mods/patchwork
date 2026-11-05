//! What Peacock, the replacement server for the game's online services,
//! needs to know about deployed mods. The game's archives can't tell it, so
//! like with SMF it reads a deploy summary when it starts: the plugins to
//! run, the unlockables and contracts it should serve, and which mods are
//! installed, for plugins to check.
//!
//! Peacock has to be told where the summary is (see [`setup_warning`]). The
//! folders it looks in by default depend on whether it runs natively or in
//! Wine, and on which prefix, whereas the game's folder is always there.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path};

use serde_json::{Value, json};

/// The folder in the game that everything for Peacock goes in.
pub const FOLDER: &str = "patchwork-peacock";
pub const SUMMARY: &str = "patchwork-peacock/deploySummary.json";

/// What mods give Peacock, gathered as they're staged.
#[derive(Default)]
pub struct ServerSide {
    /// Where each plugin's deployed in the game, in load order.
    pub plugins: Vec<String>,
    pub contracts: BTreeMap<String, Value>,
}

/// Where a mod's plugin at `plugin` (from its manifest) is deployed in the
/// game, or `None` if the mod's id or the path can't be part of a path.
pub fn plugin_path(id: &str, plugin: &str) -> Option<String> {
    let plugin = plugin.replace('\\', "/");
    let plugin = plugin.strip_prefix("./").unwrap_or(&plugin);
    let safe = |part: &str| {
        !matches!(part, "" | "." | "..") && !part.contains(['/', '\\', ':', '<', '>', '"', '|'])
    };
    (safe(id) && plugin.split('/').all(safe)).then(|| format!("{FOLDER}/plugins/{id}/{plugin}"))
}

/// The deploy summary, with [`ServerSide::plugins`] as absolute paths in the
/// game at `game`.
pub fn summary(
    game: &Path,
    ids: &HashSet<String>,
    server_side: &ServerSide,
    unlockables: Option<Value>,
) -> Vec<u8> {
    let mut order: Vec<_> = ids.iter().collect();
    order.sort();
    let plugins: Vec<_> = server_side
        .plugins
        .iter()
        .map(|rel| peacock_path(game, rel))
        .collect();
    let mut states = json!({ "peacockPlugins": plugins, "contracts": server_side.contracts });
    if let Some(unlockables) = unlockables {
        states["unlockables"] = unlockables;
    }
    serde_json::to_vec_pretty(&json!({ "loadOrder": order, "lastServerSideStates": states }))
        .unwrap()
}

/// Asks the user to point Peacock at the summary, which only they can do
/// since the setting's in Peacock's own folder, wherever that is.
pub fn setup_warning(game: &Path) -> String {
    let setting = format!("frameworkDeploySummaryPath={}", peacock_path(game, SUMMARY));
    let mut warning =
        format!("for Peacock to load the mods' plugins, set `{setting}` in its options.ini");
    if !cfg!(windows) {
        warning.push_str(&format!(
            " (or `frameworkDeploySummaryPath={}` if Peacock isn't running in Wine)",
            absolute(game).join(SUMMARY).display()
        ));
    }
    warning
}

/// `rel` in the game, as a path Peacock can open. On other systems it's
/// given as a Wine path, which Wine understands and Peacock's native builds
/// turn back into the real path.
fn peacock_path(game: &Path, rel: &str) -> String {
    let path = absolute(game).join(rel);
    if cfg!(windows) {
        return path.display().to_string().replace('/', "\\");
    }
    wine_path(&path)
}

fn wine_path(path: &Path) -> String {
    let parts: Vec<_> = path
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy()),
            _ => None,
        })
        .collect();
    format!("Z:\\{}", parts.join("\\"))
}

fn absolute(path: &Path) -> std::path::PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wine_paths_are_on_z() {
        assert_eq!(
            wine_path(Path::new("/home/me/HITMAN 3/a.js")),
            "Z:\\home\\me\\HITMAN 3\\a.js"
        );
    }

    #[test]
    fn plugin_paths_stay_in_the_mods_folder() {
        assert_eq!(
            plugin_path("A.B", "peacock plugins\\a.plugin.js").unwrap(),
            "patchwork-peacock/plugins/A.B/peacock plugins/a.plugin.js"
        );
        assert_eq!(
            plugin_path("A", "./a.plugin.js").unwrap(),
            "patchwork-peacock/plugins/A/a.plugin.js"
        );
        assert!(plugin_path("A", "../a.plugin.js").is_none());
        assert!(plugin_path("..", "a.plugin.js").is_none());
        assert!(plugin_path("A", "C:/a.plugin.js").is_none());
    }
}
