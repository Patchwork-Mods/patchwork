//! What Peacock, the replacement server for the game's online services,
//! needs to know about deployed mods. The game's archives can't tell it, so
//! like with SMF it reads a deploy summary when it starts: the plugins to
//! run, the unlockables and contracts it should serve, and which mods are
//! installed, for plugins to check.
//!
//! Peacock has to be told where the summary is (see [`setup_warning`]). The
//! folders it looks in by default depend on whether it runs natively or in
//! Wine, and on which prefix, whereas the game's folder is always there.
//! When Peacock is in the game's folder too, its options.ini is checked so
//! users who've already set it aren't asked again.

use std::collections::{BTreeMap, HashSet};
use std::path::{Component, Path, PathBuf};

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
/// since the setting's in Peacock's own folder, or `None` if a Peacock in
/// the game's folder already points at it.
pub fn setup_warning(game: &Path) -> Option<String> {
    let wine = peacock_path(game, SUMMARY);
    let native = absolute(game).join(SUMMARY);
    let installs = installs(game);
    let points_here = |value: &str| {
        // Windows paths ignore case, and Peacock's Wine handling takes
        // either slash.
        value.replace('/', "\\").eq_ignore_ascii_case(&wine)
            || (!cfg!(windows) && Path::new(value) == native)
    };
    if installs
        .iter()
        .any(|options| summary_setting(options).is_some_and(|v| points_here(&v)))
    {
        return None;
    }
    let setting = format!("frameworkDeploySummaryPath={wine}");
    let file = match installs.first() {
        Some(options) => options.display().to_string(),
        None => "its options.ini".to_owned(),
    };
    let mut warning = format!("for Peacock to load the mods' plugins, set `{setting}` in {file}");
    if !cfg!(windows) {
        warning.push_str(&format!(
            " (or `frameworkDeploySummaryPath={}` if Peacock isn't running in Wine)",
            native.display()
        ));
    }
    Some(warning)
}

/// The options.ini of each Peacock in a folder directly in the game's,
/// which is where it's usually put. A Peacock folder has its server,
/// chunk0.js, next to its options.
fn installs(game: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(game) else {
        return Vec::new();
    };
    let mut found: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|dir| dir.join("chunk0.js").is_file())
        .map(|dir| dir.join("options.ini"))
        .filter(|options| options.is_file())
        .collect();
    found.sort();
    found
}

/// What `frameworkDeploySummaryPath` is set to in the options.ini at
/// `path`, if it's set to a path rather than left as `AUTO`.
fn summary_setting(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut in_peacock = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(section) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_peacock = section.trim().eq_ignore_ascii_case("peacock");
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if in_peacock && key.trim() == "frameworkDeploySummaryPath" {
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            return (!value.is_empty() && value != "AUTO").then(|| value.to_owned());
        }
    }
    None
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
    fn summary_setting_is_read_from_the_peacock_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("options.ini");
        let read = |text: &str| {
            std::fs::write(&path, text).unwrap();
            summary_setting(&path)
        };
        assert_eq!(
            read("; c\n[peacock]\nframeworkDeploySummaryPath=Z:\\a\\b.json\n").as_deref(),
            Some("Z:\\a\\b.json")
        );
        assert_eq!(read("[peacock]\nframeworkDeploySummaryPath=AUTO\n"), None);
        assert_eq!(read("[other]\nframeworkDeploySummaryPath=x\n"), None);
        assert_eq!(
            read("[peacock]\nframeworkDeploySummaryPath=\"/a b/c.json\"\n").as_deref(),
            Some("/a b/c.json")
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
