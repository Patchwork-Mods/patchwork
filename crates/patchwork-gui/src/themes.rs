//! Themes: CSS files loaded after the built-in look.
//!
//! A theme is plain CSS. A comment in it that starts with `@theme` says
//! what it is, as TOML up to the first blank line, then a paragraph that
//! describes it:
//!
//! ```css
//! /*
//!  * @theme
//!  * name = "Risk of Rain 2"
//!  * game = "risk-of-rain-2"
//!  * modes = ["dark"]
//!  *
//!  * Night-sky HUD panels with cut corners.
//!  */
//! ```
//!
//! A theme without `game` is for every game, and is picked in `gui.toml`. A
//! game's theme is used whenever that game is open, and replaces the picked
//! theme unless it says `layer = true`. A game's `accent` colour in its
//! definition goes over the picked theme, but under the game's own theme.
//! Each game can have a theme of its own picked too, in `[theme.games]`.
//! Last of all comes `user.css`, the user's own changes on top of whatever
//! theme is in use.
//!
//! Themes come built in, from registries, or from `<config>/themes/*.css`.
//! The themes for every game that come with patchwork are in this crate's
//! `themes/`; games' own come with the built-in registry, in
//! `registry/themes/`, and an installed registry or a
//! `$PATCHWORK_REGISTRY_PATH` directory can have a `themes/` folder too.
//! They're read in that order, the same as the registry's layers, and a
//! file with the same name as an earlier one takes its place. A file without
//! a header is named after the file, and one named after a game
//! (`<game>.css`) is that game's theme.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use patchwork_core::game::Accent;
use serde::{Deserialize, Serialize};

/// The themes that come with patchwork, by id.
const BUILTIN: &[(&str, &str)] = &[
    ("blueprint", include_str!("../themes/blueprint.css")),
    ("phosphor", include_str!("../themes/phosphor.css")),
    ("sorbet", include_str!("../themes/sorbet.css")),
];

// The games' themes in the built-in registry, as `REGISTRY`.
include!(concat!(env!("OUT_DIR"), "/registry_themes.rs"));

/// Files in the themes folder that aren't themes, or whose names mean
/// something else in `gui.toml`.
const RESERVED: &[&str] = &["user", "builtin", DEFAULT, NONE];

/// The theme used when none is picked: just the built-in look.
pub const DEFAULT: &str = "default";

/// Picked for a game in `[theme.games]`: no game theme or accent, just the
/// theme picked for every game.
pub const NONE: &str = "none";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeOrigin {
    /// Comes with patchwork, in the binary.
    Builtin,
    /// From an installed registry or a `$PATCHWORK_REGISTRY_PATH` directory.
    Registry,
    /// The user's own, in `<config>/themes/`.
    User,
}

#[derive(Debug, Clone, Serialize)]
pub struct ThemeInfo {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The game it's for, or `None` if it's for every game.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game: Option<String>,
    /// The modes it has looks for. Empty if it doesn't say.
    pub modes: Vec<Mode>,
    /// Whether a game theme goes on top of the picked theme rather than
    /// taking its place.
    pub layer: bool,
    pub origin: ThemeOrigin,
    /// Sent so the picker can preview it and show its colours.
    pub css: String,
}

/// What a `@theme` comment can say.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    name: Option<String>,
    description: Option<String>,
    game: Option<String>,
    #[serde(default)]
    modes: Vec<Mode>,
    #[serde(default)]
    layer: bool,
}

/// The TOML and the paragraph after it from a CSS file's `@theme` comment,
/// or `None` if it has none.
fn header_text(css: &str) -> Option<(String, Option<String>)> {
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        let after = &rest[start + 2..];
        let end = after.find("*/").unwrap_or(after.len());
        let comment = &after[..end];
        rest = &after[end..];
        // Each line without its leading space and `*`.
        let lines: Vec<&str> = comment
            .lines()
            .map(|l| {
                let l = l.trim_start();
                let l = l.strip_prefix('*').unwrap_or(l);
                l.strip_prefix(' ').unwrap_or(l).trim_end()
            })
            .skip_while(|l| l.is_empty())
            .collect();
        let Some((first, body)) = lines.split_first() else {
            continue;
        };
        let Some(after_tag) = first.strip_prefix("@theme") else {
            continue;
        };
        if !after_tag.trim().is_empty() {
            continue;
        }
        let mut body = body.iter();
        let toml: Vec<&str> = body
            .by_ref()
            .take_while(|l| !l.is_empty())
            .copied()
            .collect();
        let paragraph: Vec<&str> = body
            .skip_while(|l| l.is_empty())
            .take_while(|l| !l.is_empty())
            .copied()
            .collect();
        let paragraph = (!paragraph.is_empty()).then(|| paragraph.join(" "));
        return Some((toml.join("\n"), paragraph));
    }
    None
}

/// The id of the theme in `path`, or `None` if it isn't a theme: not CSS,
/// or one of the [`RESERVED`] names.
pub fn theme_id(path: &Path) -> Option<&str> {
    if path.extension().is_none_or(|e| e != "css") {
        return None;
    }
    let id = path.file_stem()?.to_str()?;
    (!RESERVED.contains(&id)).then_some(id)
}

/// Makes a name out of a file name: `night-owl` → `Night owl`.
fn name_from_id(id: &str) -> String {
    let words = id.replace(['-', '_'], " ");
    let mut chars = words.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Reads a theme from its CSS. `games` are the ids of the games there are,
/// so a file without a header named after one is that game's theme.
fn parse(
    id: &str,
    css: String,
    origin: ThemeOrigin,
    games: &dyn Fn(&str) -> bool,
) -> Result<ThemeInfo, String> {
    let (header, paragraph) = match header_text(&css) {
        Some((toml, paragraph)) => {
            let header: Header = toml::from_str(&toml).map_err(|e| e.message().to_owned())?;
            (header, paragraph)
        }
        None => (
            Header {
                game: games(id).then(|| id.to_owned()),
                ..Header::default()
            },
            None,
        ),
    };
    Ok(ThemeInfo {
        id: id.to_owned(),
        name: header.name.unwrap_or_else(|| name_from_id(id)),
        description: header.description.or(paragraph),
        game: header.game,
        modes: header.modes,
        layer: header.layer,
        origin,
        css,
    })
}

/// Every theme there is.
#[derive(Debug, Default)]
pub struct Catalog {
    /// By id.
    pub themes: BTreeMap<String, ThemeInfo>,
    /// Themes that couldn't be read, as messages for the user.
    pub problems: Vec<String>,
}

impl Catalog {
    /// The built-in themes, then `files` (id, CSS and where it's from) on
    /// top, in order.
    fn new(files: Vec<(String, String, ThemeOrigin)>, games: &dyn Fn(&str) -> bool) -> Self {
        let mut catalog = Catalog::default();
        let builtin = BUILTIN
            .iter()
            .chain(REGISTRY)
            .map(|&(id, css)| (id.to_owned(), css.to_owned(), ThemeOrigin::Builtin));
        for (id, css, origin) in builtin.chain(files) {
            match parse(&id, css, origin, games) {
                Ok(theme) => {
                    catalog.themes.insert(id, theme);
                }
                Err(e) => catalog.problems.push(format!("theme {id}.css: {e}")),
            }
        }
        catalog
    }

    /// The built-in themes, then the ones in `dirs`, in order.
    pub fn load(dirs: &[(ThemeOrigin, PathBuf)], games: &dyn Fn(&str) -> bool) -> Self {
        let mut files = Vec::new();
        let mut problems = Vec::new();
        for (origin, dir) in dirs {
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    problems.push(format!("couldn't read {}: {e}", dir.display()));
                    continue;
                }
            };
            let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            paths.sort();
            for path in paths {
                let Some(id) = theme_id(&path) else {
                    continue;
                };
                match std::fs::read_to_string(&path) {
                    Ok(css) => files.push((id.to_owned(), css, *origin)),
                    Err(e) => problems.push(format!("couldn't read {}: {e}", path.display())),
                }
            }
        }
        let mut catalog = Catalog::new(files, games);
        catalog.problems.extend(problems);
        catalog
    }

    /// The themes to use, picked theme first, with `prefs` and `game` open.
    /// `accent` is the open game's colour, from its definition.
    pub fn resolve(
        &self,
        prefs: &ThemePrefs,
        game: Option<&str>,
        accent: Option<&Accent>,
    ) -> Resolved<'_> {
        let mut problems = Vec::new();
        let mut find = |id: &str| {
            let found = self.themes.get(id);
            if found.is_none() {
                problems.push(format!("there's no theme called {id:?}"));
            }
            found
        };
        let global = match prefs.theme.as_deref() {
            None | Some(DEFAULT) => None,
            Some(id) => find(id),
        };
        let pick = game.and_then(|g| prefs.games.get(g)).map(String::as_str);
        // Whether the game's accent can be used, and its theme.
        let (global, accent_on, game_theme) = match (game, pick) {
            (None, _) => (global, false, None),
            (Some(_), Some(NONE)) => (global, false, None),
            (Some(_), Some(DEFAULT)) => (None, false, None),
            (Some(game), pick) => match pick.and_then(find) {
                // A theme for every game picked for this one takes the
                // place of the one picked for every game, and of the
                // game's own look.
                Some(t) if t.game.is_none() => (Some(t), false, None),
                Some(t) => (global, true, Some(t)),
                None => match prefs.game_themes {
                    GameThemes::On => (global, true, self.game_theme(game)),
                    GameThemes::Accent => (global, true, None),
                    GameThemes::Off => (global, false, None),
                },
            },
        };
        let global = match game_theme {
            Some(t) if !t.layer => None,
            _ => global,
        };
        Resolved {
            global,
            game: game_theme,
            accent: accent.filter(|_| accent_on).map(AccentColors::new),
            problems,
        }
    }

    /// `game`'s own theme, if it has one. The user's own come first, then a
    /// file named after the game, then by id.
    pub fn game_theme(&self, game: &str) -> Option<&ThemeInfo> {
        self.themes
            .values()
            .filter(|t| t.game.as_deref() == Some(game))
            .min_by_key(|t| (t.origin != ThemeOrigin::User, t.id != game, &t.id))
    }
}

pub struct Resolved<'a> {
    /// The theme picked for every game, unless a game's theme replaces it.
    pub global: Option<&'a ThemeInfo>,
    pub game: Option<&'a ThemeInfo>,
    /// The open game's colour, which goes over `global` but under `game`.
    pub accent: Option<AccentColors>,
    pub problems: Vec<String>,
}

/// What a game's accent sets: `--pw-accent`, and `--pw-accent-text` for
/// text on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccentColors {
    pub color: String,
    pub text: String,
}

impl AccentColors {
    fn new(accent: &Accent) -> Self {
        // The built-in look's dark and light text colours.
        let text = if accent.is_light() {
            "#141413"
        } else {
            "#ffffff"
        };
        AccentColors {
            color: accent.as_str().to_owned(),
            text: text.to_owned(),
        }
    }
}

impl Resolved<'_> {
    /// The modes the themes in use have looks for, or empty if they don't
    /// say. When the two disagree, the game's theme wins.
    pub fn modes(&self) -> Vec<Mode> {
        let mut modes: Option<Vec<Mode>> = None;
        for theme in [self.global, self.game].into_iter().flatten() {
            if theme.modes.is_empty() {
                continue;
            }
            let both: Vec<Mode> = match &modes {
                Some(m) => m
                    .iter()
                    .copied()
                    .filter(|m| theme.modes.contains(m))
                    .collect(),
                None => Vec::new(),
            };
            modes = Some(if both.is_empty() {
                theme.modes.clone()
            } else {
                both
            });
        }
        modes.unwrap_or_default()
    }
}

/// Light or dark: as the system is, or always one of them.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameThemes {
    /// Games' own themes are used when they have one, otherwise their
    /// accent colour.
    #[default]
    On,
    /// Only games' accent colours.
    Accent,
    Off,
}

/// The `[theme]` table in `<config>/gui.toml`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThemePrefs {
    /// The id of the theme for every game; `None` (or `"default"`) for just
    /// the built-in look.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    pub mode: ModePref,
    pub game_themes: GameThemes,
    /// Themes picked for single games, by game id: a theme's id, [`NONE`]
    /// for no game theme, or [`DEFAULT`] for the built-in look. A game not
    /// here follows `game_themes`.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub games: BTreeMap<String, String>,
    /// Keys this version doesn't know, kept when saving.
    #[serde(flatten)]
    pub rest: toml::Table,
}

/// `<config>/gui.toml`: the app's own preferences. Unknown keys are left
/// alone so newer versions can add some.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiPrefs {
    pub theme: ThemePrefs,
    #[serde(flatten)]
    pub rest: toml::Table,
}

impl GuiPrefs {
    pub fn load(config: &Path) -> Result<Self, String> {
        patchwork_core::fsutil::read_toml(&config.join("gui.toml"))
            .map(Option::unwrap_or_default)
            .map_err(|e| e.to_string())
    }

    pub fn save(&self, config: &Path) -> Result<(), String> {
        patchwork_core::fsutil::write_toml(&config.join("gui.toml"), self)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_games(_: &str) -> bool {
        false
    }

    fn catalog(files: &[(&str, &str)]) -> Catalog {
        let files = files
            .iter()
            .map(|(id, css)| (id.to_string(), css.to_string(), ThemeOrigin::User))
            .collect();
        Catalog::new(files, &|g| g == "ror2" || g == "valheim")
    }

    #[test]
    fn builtin_themes_have_headers() {
        let catalog = Catalog::new(Vec::new(), &no_games);
        assert!(catalog.problems.is_empty(), "{:?}", catalog.problems);
        let sorbet = &catalog.themes["sorbet"];
        assert_eq!(sorbet.name, "Sorbet");
        assert_eq!(sorbet.modes, [Mode::Light, Mode::Dark]);
        assert!(
            sorbet
                .description
                .as_deref()
                .unwrap()
                .starts_with("Soft, round")
        );
        assert!(sorbet.game.is_none());
        for (id, css) in BUILTIN {
            assert!(header_text(css).is_some(), "{id} has no @theme comment");
            assert!(!catalog.themes[*id].modes.is_empty(), "{id} has no modes");
        }
    }

    #[test]
    fn registry_game_themes() {
        let catalog = Catalog::new(Vec::new(), &no_games);
        let ror2 = &catalog.themes["risk-of-rain-2"];
        assert_eq!(ror2.game.as_deref(), Some("risk-of-rain-2"));
        assert_eq!(ror2.name, "Risk of Rain 2");
        assert_eq!(ror2.origin, ThemeOrigin::Builtin);
        for (id, css) in REGISTRY {
            let Some((toml, _)) = header_text(css) else {
                panic!("registry/themes/{id}.css has no @theme comment");
            };
            let header: Header = toml::from_str(&toml).unwrap();
            assert!(
                header.game.is_some(),
                "registry/themes/{id}.css has no game"
            );
            assert!(!RESERVED.contains(id), "{id} is a reserved name");
            assert!(
                !BUILTIN.iter().any(|(b, _)| b == id),
                "{id} is a built-in theme's name"
            );
        }
    }

    #[test]
    fn loads_layers_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let registry = root.join("registry");
        let user = root.join("user");
        std::fs::create_dir_all(&registry).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        let header = |name: &str| format!("/* @theme\nname = \"{name}\" */");
        std::fs::write(registry.join("mine.css"), header("Registry")).unwrap();
        std::fs::write(registry.join("theirs.css"), header("Theirs")).unwrap();
        std::fs::write(user.join("mine.css"), header("User")).unwrap();
        std::fs::write(user.join("user.css"), header("Not a theme")).unwrap();
        std::fs::write(user.join("notes.txt"), "").unwrap();
        let dirs = [
            (ThemeOrigin::Registry, registry),
            (ThemeOrigin::User, user),
            (ThemeOrigin::User, root.join("missing")),
        ];
        let c = Catalog::load(&dirs, &no_games);
        assert!(c.problems.is_empty(), "{:?}", c.problems);
        assert_eq!(c.themes["mine"].name, "User");
        assert_eq!(c.themes["mine"].origin, ThemeOrigin::User);
        assert_eq!(c.themes["theirs"].origin, ThemeOrigin::Registry);
        assert!(!c.themes.contains_key("user") && !c.themes.contains_key("notes"));
    }

    #[test]
    fn reads_header_after_other_comments() {
        let css = "/* licence */\n:root{}\n/*\n * @theme\n * name = \"A\"\n * game = \"x\"\n * layer = true\n *\n * Line one\n * line two.\n *\n * Not this.\n */";
        let (toml, para) = header_text(css).unwrap();
        assert_eq!(toml, "name = \"A\"\ngame = \"x\"\nlayer = true");
        assert_eq!(para.as_deref(), Some("Line one line two."));
    }

    #[test]
    fn unstarred_header() {
        let css = "/* @theme\n   name = \"B\"\n*/";
        assert_eq!(header_text(css).unwrap().0.trim(), "name = \"B\"");
        assert!(header_text("/* @themes are nice */").is_none());
    }

    #[test]
    fn headerless_files() {
        let c = catalog(&[("night-owl", ":root{}"), ("ror2", ":root{}")]);
        assert_eq!(c.themes["night-owl"].name, "Night owl");
        assert_eq!(c.themes["night-owl"].game, None);
        assert_eq!(c.themes["ror2"].game.as_deref(), Some("ror2"));
    }

    #[test]
    fn bad_header_is_a_problem() {
        let c = catalog(&[("bad", "/* @theme\nnmae = \"x\" */")]);
        assert!(!c.themes.contains_key("bad"));
        assert_eq!(c.problems.len(), 1);
    }

    #[test]
    fn user_file_replaces_builtin() {
        let c = catalog(&[("sorbet", "/* @theme\nname = \"Mine\" */")]);
        assert_eq!(c.themes["sorbet"].name, "Mine");
        assert_eq!(c.themes["sorbet"].origin, ThemeOrigin::User);
    }

    #[test]
    fn game_theme_replaces_picked_one() {
        let c = catalog(&[
            ("ror2", ":root{}"),
            (
                "valheim-extra",
                "/* @theme\ngame = \"valheim\"\nlayer = true */",
            ),
        ]);
        let prefs = ThemePrefs {
            theme: Some("sorbet".into()),
            ..ThemePrefs::default()
        };

        let r = c.resolve(&prefs, None, None);
        assert_eq!(r.global.unwrap().id, "sorbet");
        assert!(r.game.is_none());

        let r = c.resolve(&prefs, Some("ror2"), None);
        assert!(r.global.is_none());
        assert_eq!(r.game.unwrap().id, "ror2");

        let r = c.resolve(&prefs, Some("valheim"), None);
        assert_eq!(r.global.unwrap().id, "sorbet");
        assert_eq!(r.game.unwrap().id, "valheim-extra");

        let off = ThemePrefs {
            game_themes: GameThemes::Off,
            ..prefs
        };
        let r = c.resolve(&off, Some("ror2"), None);
        assert_eq!(r.global.unwrap().id, "sorbet");
        assert!(r.game.is_none());
    }

    fn accent() -> Accent {
        Accent::try_from("#ffd700".to_owned()).unwrap()
    }

    #[test]
    fn game_accent() {
        let c = catalog(&[("ror2", ":root{}")]);
        let a = accent();
        let prefs = |game_themes| ThemePrefs {
            theme: Some("sorbet".into()),
            game_themes,
            ..ThemePrefs::default()
        };
        let gold = Some(AccentColors {
            color: "#ffd700".into(),
            text: "#141413".into(),
        });

        // Under the game's theme, which comes on top.
        let r = c.resolve(&prefs(GameThemes::On), Some("ror2"), Some(&a));
        assert_eq!(r.game.unwrap().id, "ror2");
        assert_eq!(r.accent, gold);
        // Over the picked theme when the game has none.
        let r = c.resolve(&prefs(GameThemes::On), Some("valheim"), Some(&a));
        assert_eq!(r.global.unwrap().id, "sorbet");
        assert_eq!(r.accent, gold);

        let r = c.resolve(&prefs(GameThemes::Accent), Some("ror2"), Some(&a));
        assert_eq!(r.global.unwrap().id, "sorbet");
        assert!(r.game.is_none());
        assert_eq!(r.accent, gold);

        let r = c.resolve(&prefs(GameThemes::Off), Some("ror2"), Some(&a));
        assert!(r.game.is_none() && r.accent.is_none());

        // Not with no game open, whatever is passed.
        let r = c.resolve(&prefs(GameThemes::On), None, Some(&a));
        assert!(r.accent.is_none());
    }

    #[test]
    fn theme_picked_for_a_game() {
        let c = catalog(&[
            ("ror2", ":root{}"),
            ("ror2-alt", "/* @theme\ngame = \"ror2\"\nlayer = true */"),
        ]);
        let a = accent();
        let with = |pick: &str, game_themes| ThemePrefs {
            theme: Some("sorbet".into()),
            game_themes,
            games: [("ror2".to_owned(), pick.to_owned())].into(),
            ..ThemePrefs::default()
        };
        let resolve = |pick, game_themes| {
            let r = c.resolve(&with(pick, game_themes), Some("ror2"), Some(&a));
            (
                r.global.map(|t| t.id.as_str()),
                r.game.map(|t| t.id.as_str()),
                r.accent.is_some(),
                r.problems.len(),
            )
        };

        // Another game theme, even with game themes off.
        assert_eq!(
            resolve("ror2-alt", GameThemes::Off),
            (Some("sorbet"), Some("ror2-alt"), true, 0)
        );
        // A theme for every game, in place of the picked one and the game's look.
        assert_eq!(
            resolve("phosphor", GameThemes::On),
            (Some("phosphor"), None, false, 0)
        );
        assert_eq!(
            resolve(NONE, GameThemes::On),
            (Some("sorbet"), None, false, 0)
        );
        assert_eq!(resolve(DEFAULT, GameThemes::On), (None, None, false, 0));
        // An unknown one is reported, and the game follows game_themes.
        assert_eq!(
            resolve("nope", GameThemes::On),
            (None, Some("ror2"), true, 1)
        );
        // Other games aren't affected.
        let r = c.resolve(&with("phosphor", GameThemes::On), Some("valheim"), None);
        assert_eq!(r.global.unwrap().id, "sorbet");
    }

    #[test]
    fn game_theme_preference() {
        let c = catalog(&[
            ("b", "/* @theme\ngame = \"valheim\" */"),
            ("a", "/* @theme\ngame = \"valheim\" */"),
            ("valheim", ""),
        ]);
        assert_eq!(c.game_theme("valheim").unwrap().id, "valheim");
        let c = catalog(&[
            ("b", "/* @theme\ngame = \"valheim\" */"),
            ("a", "/* @theme\ngame = \"valheim\" */"),
        ]);
        assert_eq!(c.game_theme("valheim").unwrap().id, "a");
    }

    #[test]
    fn unknown_or_default_theme() {
        let c = catalog(&[]);
        let r = c.resolve(&ThemePrefs::default(), None, None);
        assert!(r.global.is_none() && r.problems.is_empty());
        let prefs = ThemePrefs {
            theme: Some("nope".into()),
            ..ThemePrefs::default()
        };
        let r = c.resolve(&prefs, None, None);
        assert!(r.global.is_none());
        assert_eq!(r.problems.len(), 1);
    }

    #[test]
    fn modes_in_use() {
        let c = catalog(&[
            (
                "dark-game",
                "/* @theme\ngame = \"ror2\"\nmodes = [\"dark\"]\nlayer = true */",
            ),
            (
                "light-game",
                "/* @theme\ngame = \"valheim\"\nmodes = [\"light\"]\nlayer = true */",
            ),
            ("any", ""),
        ]);
        let pick = |theme: &str| ThemePrefs {
            theme: Some(theme.into()),
            ..ThemePrefs::default()
        };
        assert_eq!(
            c.resolve(&pick("sorbet"), None, None).modes(),
            [Mode::Light, Mode::Dark]
        );
        assert_eq!(c.resolve(&pick("any"), None, None).modes(), []);
        assert_eq!(
            c.resolve(&pick("sorbet"), Some("ror2"), None).modes(),
            [Mode::Dark]
        );
        // Phosphor is dark only, so the light game theme wins.
        assert_eq!(
            c.resolve(&pick("phosphor"), Some("valheim"), None).modes(),
            [Mode::Light]
        );
        assert_eq!(
            c.resolve(&pick("any"), Some("ror2"), None).modes(),
            [Mode::Dark]
        );
    }

    #[test]
    fn prefs_keep_unknown_keys() {
        let mut p: GuiPrefs = toml::from_str("later = 1\n[theme]\nsoon = \"x\"").unwrap();
        p.theme.mode = ModePref::Dark;
        let text = toml::to_string(&p).unwrap();
        let back: toml::Table = toml::from_str(&text).unwrap();
        assert_eq!(back["later"].as_integer(), Some(1));
        assert_eq!(back["theme"]["soon"].as_str(), Some("x"));
        assert_eq!(back["theme"]["mode"].as_str(), Some("dark"));
    }

    #[test]
    fn prefs_file() {
        let p: GuiPrefs =
            toml::from_str("[theme]\ntheme = \"phosphor\"\ngame_themes = \"off\"\nlater = 1")
                .unwrap();
        assert_eq!(p.theme.theme.as_deref(), Some("phosphor"));
        assert_eq!(p.theme.game_themes, GameThemes::Off);
        let p: GuiPrefs = toml::from_str("").unwrap();
        assert_eq!(p.theme.game_themes, GameThemes::On);
        assert_eq!(p.theme.mode, ModePref::System);
        let p: GuiPrefs =
            toml::from_str("[theme]\ngame_themes = \"accent\"\n[theme.games]\nror2 = \"none\"")
                .unwrap();
        assert_eq!(p.theme.game_themes, GameThemes::Accent);
        assert_eq!(p.theme.games["ror2"], NONE);
    }
}
