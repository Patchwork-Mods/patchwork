use std::fs;
use std::path::PathBuf;

use patchwork_core::deploy::{DeployState, ModFile, Roots};
use patchwork_core::event::Silent;
use patchwork_core::game::{DeployerSpec, GameDef};
use patchwork_core::list::ListEntry;
use patchwork_core::registry::Registry;

use super::*;

const CONFIG: &str = r#"
[[rules]]
folder = "Data"
to = "Data/{path}"

[[rules]]
to = "Data/{path}"
"#;

const PLUGINS: &str = r#"
folder = "Data"
file = "appdata:plugins.txt"
implicit = ["Skyrim.esm", "Update.esm"]
ccc = "Skyrim.ccc"
limit = 254
"#;

/// A plugin's bytes: a TES4 record with `flags` and `masters`, then some
/// other record that shouldn't be read.
fn plugin_bytes(flags: u32, masters: &[&str]) -> Vec<u8> {
    plugin_bytes_sized(HEADER_SIZE, flags, masters)
}

/// [`plugin_bytes`] for a game whose record headers are `header_size`
/// bytes long.
fn plugin_bytes_sized(header_size: usize, flags: u32, masters: &[&str]) -> Vec<u8> {
    let mut data = Vec::new();
    let mut sub = |kind: &[u8], field: &[u8]| {
        data.extend_from_slice(kind);
        data.extend_from_slice(&(field.len() as u16).to_le_bytes());
        data.extend_from_slice(field);
    };
    sub(b"HEDR", &[0; 12]);
    sub(b"CNAM", b"someone\0");
    for master in masters {
        sub(b"MAST", format!("{master}\0").as_bytes());
        sub(b"DATA", &[0; 8]);
    }
    let mut bytes = b"TES4".to_vec();
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&flags.to_le_bytes());
    bytes.extend_from_slice(&vec![0; header_size - 12]);
    bytes.extend_from_slice(&data);
    bytes.extend_from_slice(b"GRUP\xff\xff\xff\xff");
    bytes
}

struct Env {
    tmp: tempfile::TempDir,
    game: PathBuf,
    def: GameDef,
}

impl Env {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let game = tmp.path().join("game");
        fs::create_dir_all(game.join("Data")).unwrap();
        let def = GameDef {
            schema: 1,
            id: "skyrim-se".to_owned(),
            name: "Skyrim".to_owned(),
            description: None,
            image: None,
            hidden: false,
            sources: Default::default(),
            deployer: DeployerSpec {
                kind: KIND.to_owned(),
                config: toml::from_str(CONFIG).unwrap(),
            },
            link: Default::default(),
            detect: Default::default(),
            versions: Default::default(),
            env: Default::default(),
            roots: Default::default(),
            launch: Default::default(),
            capture: None,
            instances: None,
            config: Vec::new(),
            ext: [(
                EXTENSION.to_owned(),
                toml::Table::from_iter([(
                    LIST.to_owned(),
                    toml::Value::Table(toml::from_str(PLUGINS).unwrap()),
                )]),
            )]
            .into(),
        };
        Self { tmp, game, def }
    }

    /// A mod called `name` with `files`, as paths and contents.
    fn installed(&self, name: &str, files: &[(&str, Vec<u8>)]) -> InstalledMod {
        let dir = self.tmp.path().join("store").join(name);
        let files = files
            .iter()
            .map(|(rel, bytes)| {
                let src = dir.join(rel);
                fs::create_dir_all(src.parent().unwrap()).unwrap();
                fs::write(&src, bytes).unwrap();
                ModFile {
                    rel: RelPath::new(rel).unwrap(),
                    src,
                }
            })
            .collect();
        InstalledMod {
            name: name.to_owned(),
            source: "local".to_owned(),
            id: name.to_owned(),
            version: None,
            hash: ContentHash::of(name.as_bytes()),
            files,
            kind: None,
            ext: Default::default(),
        }
    }

    /// What the load order finds in `mods`, and what it makes of `saved`.
    fn list(&self, saved: &[ListEntry], mods: &[InstalledMod]) -> (ListItems, List) {
        let registry = Registry::default();
        let ctx = DeployContext {
            game: &self.def,
            config: &self.def.deployer.config,
            registry: &registry,
        };
        let state = DeployState::empty("skyrim-se", &Roots::game(&self.game));
        let files = GameFiles::new(&self.game, &state);
        let config = self.def.hook_config(EXTENSION, LIST).unwrap();
        let lctx = ListContext::new(&ctx, &files, config);
        let found = BethesdaLoadOrder.items(&lctx, mods).unwrap();
        let list = List::new(
            format!("{EXTENSION}.{LIST}"),
            &BethesdaLoadOrder,
            saved,
            found.clone(),
        );
        (found, list)
    }

    /// What [`PluginLimit`] says about the load order `saved` makes of
    /// `mods`.
    fn limit(&self, saved: &[ListEntry], mods: &[InstalledMod]) -> Vec<Problem> {
        let list = self.list(saved, mods).1;
        let state = DeployState::empty("skyrim-se", &Roots::game(&self.game));
        let files = GameFiles::new(&self.game, &state);
        let empty = toml::Table::new();
        let lists = [list];
        let ctx = CheckContext::new(&self.def, &empty, EXTENSION, &files, &lists);
        PluginLimit.check(&ctx, mods)
    }

    fn items(&self, mods: &[InstalledMod]) -> ListItems {
        self.list(&[], mods).0
    }

    fn build(&self, order: &List) -> (DesiredState, String) {
        self.build_with(order, &[])
    }

    fn build_with(&self, order: &List, mods: &[InstalledMod]) -> (DesiredState, String) {
        let registry = Registry::default();
        let ctx = DeployContext {
            game: &self.def,
            config: &self.def.deployer.config,
            registry: &registry,
        };
        let state = DeployState::empty("skyrim-se", &Roots::game(&self.game));
        let out = self.tmp.path().join("out");
        fs::create_dir_all(&out).unwrap();
        let build = BuildContext {
            game: GameFiles::new(&self.game, &state),
            out: &out,
            cache: &out,
            reporter: &Silent,
            lists: std::slice::from_ref(order),
        };
        let built = BethesdaDeployer.build(&ctx, &build, mods).unwrap();
        (built, fs::read_to_string(out.join(OUT)).unwrap())
    }
}

#[test]
fn headers_say_what_a_plugin_is_and_needs() {
    let env = Env::new();
    let path = env.tmp.path().join("p.esp");
    fs::write(
        &path,
        plugin_bytes(MASTER_FLAG | LIGHT_FLAG, &["Skyrim.esm", "Base.esm"]),
    )
    .unwrap();
    assert_eq!(
        read_header(&path, HEADER_SIZE).unwrap(),
        Header {
            flags: MASTER_FLAG | LIGHT_FLAG,
            masters: vec!["Skyrim.esm".to_owned(), "Base.esm".to_owned()],
        }
    );
    fs::write(&path, plugin_bytes(0, &[])).unwrap();
    assert_eq!(read_header(&path, HEADER_SIZE).unwrap(), Header::default());

    for (bytes, why) in [
        (b"TES4".to_vec(), "too short"),
        (
            b"TES3\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0".to_vec(),
            "TES4",
        ),
        (plugin_bytes(0, &["A.esm"])[..40].to_vec(), "cut short"),
    ] {
        fs::write(&path, bytes).unwrap();
        let err = read_header(&path, HEADER_SIZE).unwrap_err();
        assert!(err.contains(why), "{err}");
    }
}

#[test]
fn plugins_are_found_where_the_rules_put_them() {
    let env = Env::new();
    let mods = [
        env.installed(
            "Wrapped",
            &[
                (
                    "Wrapped/Data/Base.esm",
                    plugin_bytes(MASTER_FLAG, &["Skyrim.esm"]),
                ),
                ("Wrapped/Data/meshes/x.nif", vec![]),
            ],
        ),
        env.installed(
            "Loose",
            &[
                ("Patch.esp", plugin_bytes(0, &["Skyrim.esm", "Base.esm"])),
                ("Small.esl", plugin_bytes(0, &[])),
                ("Flagged.esp", plugin_bytes(MASTER_FLAG, &[])),
                ("Broken.esp", b"nope".to_vec()),
                // not directly in Data, so not a plugin
                ("textures/odd.esp", vec![]),
            ],
        ),
    ];
    let plugins = env.items(&mods);
    let names: Vec<_> = plugins.items.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Base.esm",
            "Patch.esp",
            "Small.esl",
            "Flagged.esp",
            "Broken.esp"
        ]
    );
    let get = |name: &str| plugins.items.iter().find(|p| p.name == name).unwrap();
    assert_eq!(get("Base.esm").mod_name, "Wrapped");
    assert_eq!(
        (get("Base.esm").tier, &get("Base.esm").badges[..]),
        (MASTERS, &["master".to_owned()][..])
    );
    assert_eq!(get("Patch.esp").after, ["Skyrim.esm", "Base.esm"]);
    assert_eq!(get("Patch.esp").tier, OTHERS);
    assert!(get("Patch.esp").badges.is_empty());
    assert_eq!(
        (get("Small.esl").tier, &get("Small.esl").badges[..]),
        (MASTERS, &["light".to_owned()][..])
    );
    assert_eq!(get("Flagged.esp").tier, MASTERS);
    assert_eq!(plugins.warnings.len(), 1);
    assert!(
        plugins.warnings[0].contains("Broken.esp"),
        "{:?}",
        plugins.warnings
    );
    let fixed: Vec<_> = plugins.fixed.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(fixed, ["Skyrim.esm", "Update.esm"]);
}

#[test]
fn creation_club_plugins_that_are_there_load_first() {
    let env = Env::new();
    fs::write(
        env.game.join("Skyrim.ccc"),
        "ccBGSSSE001-Fish.esm\r\nccQDRSSE001-SurvivalMode.esl\r\nccMissing.esl\r\n",
    )
    .unwrap();
    for file in [
        "ccBGSSSE001-Fish.esm",
        "ccqdrsse001-survivalmode.esl",
        "Skyrim.esm",
        "Mine.esp",
    ] {
        fs::write(env.game.join("Data").join(file), "").unwrap();
    }
    let plugins = env.items(&[]);
    let fixed: Vec<_> = plugins.fixed.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        fixed,
        [
            "Skyrim.esm",
            "Update.esm",
            "ccBGSSSE001-Fish.esm",
            "ccQDRSSE001-SurvivalMode.esl"
        ]
    );
    // a plugin put there by hand is left out of the load order
    assert_eq!(plugins.warnings.len(), 1);
    assert!(
        plugins.warnings[0].ends_with(": Mine.esp"),
        "{:?}",
        plugins.warnings
    );
}

#[test]
fn light_creation_club_plugins_are_found_by_flag_or_extension() {
    let env = Env::new();
    fs::write(
        env.game.join("Skyrim.ccc"),
        "ccFlagged.esm\r\nccFull.esm\r\nccLight.esl\r\n",
    )
    .unwrap();
    let data = env.game.join("Data");
    fs::write(
        data.join("ccflagged.esm"),
        plugin_bytes(MASTER_FLAG | LIGHT_FLAG, &[]),
    )
    .unwrap();
    fs::write(data.join("ccFull.esm"), plugin_bytes(MASTER_FLAG, &[])).unwrap();
    fs::write(data.join("ccLight.esl"), "").unwrap();
    let plugins = env.items(&[]);
    let light: Vec<_> = plugins
        .fixed
        .iter()
        .filter(|p| p.badges.iter().any(|b| b == "light"))
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(light, ["ccFlagged.esm", "ccLight.esl"]);
}

#[test]
fn the_load_order_is_written_for_the_game() {
    let env = Env::new();
    let mods = [env.installed(
        "M",
        &[
            ("A.esp", plugin_bytes(0, &[])),
            ("B.esp", plugin_bytes(0, &[])),
            ("Base.esm", plugin_bytes(0, &[])),
        ],
    )];
    let saved = [
        ListEntry {
            name: "B.esp".to_owned(),
            enabled: true,
        },
        ListEntry {
            name: "A.esp".to_owned(),
            enabled: false,
        },
    ];
    let (_, order) = env.list(&saved, &mods);
    let (built, text) = env.build(&order);
    let lines: Vec<_> = text.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(lines, ["*Base.esm", "*B.esp", "A.esp"]);
    assert!(text.contains("\r\n"));
    assert_eq!(built.placements.len(), 1);
    assert_eq!(built.placements[0].dst.to_string(), "appdata:plugins.txt");
    assert_eq!(
        built.placements[0].src,
        Src::Built(RelPath::new(OUT).unwrap())
    );
}

#[test]
fn games_without_the_load_order_have_none() {
    let mut env = Env::new();
    env.def.ext.clear();
    let registry = Registry::default();
    let ctx = DeployContext {
        game: &env.def,
        config: &env.def.deployer.config,
        registry: &registry,
    };
    let state = DeployState::empty("skyrim-se", &Roots::game(&env.game));
    let game = GameFiles::new(&env.game, &state);
    assert!(!BethesdaDeployer.builds(&ctx));
    assert!(BethesdaDeployer.fingerprint(&ctx, &game).unwrap().is_none());
}

#[test]
fn too_many_full_plugins_are_warned_about() {
    let mut env = Env::new();
    let mods = [env.installed(
        "M",
        &[
            ("A.esp", plugin_bytes(0, &[])),
            ("B.esp", plugin_bytes(0, &[])),
            ("L.esl", plugin_bytes(0, &[])),
        ],
    )];
    let set_limit = |env: &mut Env, limit: i64| {
        env.def.ext.get_mut(EXTENSION).unwrap()[LIST]
            .as_table_mut()
            .unwrap()
            .insert("limit".to_owned(), toml::Value::Integer(limit));
    };
    // the game's two count, but light plugins don't
    set_limit(&mut env, 4);
    assert!(env.limit(&[], &mods).is_empty());
    set_limit(&mut env, 3);
    let problems = env.limit(&[], &mods);
    assert!(problems[0].message.starts_with("4 plugins"), "{problems:?}");
    // shown with the load order
    assert_eq!(problems[0].list.as_deref(), Some(LIST));

    // nor do disabled ones
    let off = [ListEntry {
        name: "A.esp".to_owned(),
        enabled: false,
    }];
    assert!(env.limit(&off, &mods).is_empty());
}

#[test]
fn too_many_light_plugins_are_warned_about() {
    let mut env = Env::new();
    let mods = [env.installed(
        "M",
        &[
            ("A.esl", plugin_bytes(0, &[])),
            ("B.esp", plugin_bytes(LIGHT_FLAG, &[])),
            ("C.esp", plugin_bytes(0, &[])),
        ],
    )];
    let set_limit = |env: &mut Env, limit: i64| {
        env.def.ext.get_mut(EXTENSION).unwrap()[LIST]
            .as_table_mut()
            .unwrap()
            .insert("light_limit".to_owned(), toml::Value::Integer(limit));
    };
    // light by extension or by flag; full ones don't count
    set_limit(&mut env, 2);
    assert!(env.limit(&[], &mods).is_empty());
    set_limit(&mut env, 1);
    let problems = env.limit(&[], &mods);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].message.starts_with("2 light plugins"),
        "{problems:?}"
    );
    assert_eq!(problems[0].list.as_deref(), Some(LIST));

    let off = [ListEntry {
        name: "A.esl".to_owned(),
        enabled: false,
    }];
    assert!(env.limit(&off, &mods).is_empty());
}

/// Starfield's light flag is 0x100, and 0x400 makes a plugin medium.
fn starfield(env: &mut Env, medium_limit: i64) {
    let table = env.def.ext.get_mut(EXTENSION).unwrap()[LIST]
        .as_table_mut()
        .unwrap();
    table.insert("light_flag".to_owned(), toml::Value::Integer(0x100));
    table.insert("medium_flag".to_owned(), toml::Value::Integer(0x400));
    table.insert(
        "medium_limit".to_owned(),
        toml::Value::Integer(medium_limit),
    );
    table.insert("limit".to_owned(), toml::Value::Integer(253));
}

#[test]
fn games_can_say_which_flags_make_plugins_light_or_medium() {
    let mut env = Env::new();
    starfield(&mut env, 256);
    let mods = [env.installed(
        "M",
        &[
            ("Small.esm", plugin_bytes(MASTER_FLAG | 0x100, &[])),
            ("Medium.esm", plugin_bytes(MASTER_FLAG | 0x400, &[])),
            // a medium flag on a light plugin is ignored
            ("Both.esp", plugin_bytes(0x100 | 0x400, &[])),
            // Skyrim's light flag is something else in Starfield
            ("Update.esp", plugin_bytes(0x200, &[])),
            ("Light.esl", plugin_bytes(0, &[])),
        ],
    )];
    let badges: Vec<_> = env
        .items(&mods)
        .items
        .iter()
        .map(|p| (p.name.clone(), p.badges.join(",")))
        .collect();
    let want = [
        ("Small.esm", "light"),
        ("Medium.esm", "medium"),
        ("Both.esp", "light"),
        ("Update.esp", ""),
        ("Light.esl", "light"),
    ];
    for (name, badge) in want {
        assert!(
            badges.iter().any(|(n, b)| n == name && b == badge),
            "{name}: {badges:?}"
        );
    }
}

#[test]
fn too_many_medium_plugins_are_warned_about() {
    let mut env = Env::new();
    let mods = [env.installed(
        "M",
        &[
            ("A.esm", plugin_bytes(MASTER_FLAG | 0x400, &[])),
            ("B.esm", plugin_bytes(MASTER_FLAG | 0x400, &[])),
            ("C.esp", plugin_bytes(0, &[])),
        ],
    )];
    starfield(&mut env, 2);
    assert!(env.limit(&[], &mods).is_empty());
    starfield(&mut env, 1);
    let problems = env.limit(&[], &mods);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].message.starts_with("2 medium plugins"),
        "{problems:?}"
    );
}

#[test]
fn plugins_the_game_ships_but_doesnt_load_are_in_the_load_order() {
    let mut env = Env::new();
    env.def.ext.get_mut(EXTENSION).unwrap()[LIST]
        .as_table_mut()
        .unwrap()
        .insert(
            "shipped".to_owned(),
            toml::Value::Array(vec![
                toml::Value::String("Enderal - Forgotten Stories.esm".to_owned()),
                toml::Value::String("Missing.esp".to_owned()),
            ]),
        );
    let data = env.game.join("Data");
    fs::write(
        data.join("enderal - forgotten stories.esm"),
        plugin_bytes(MASTER_FLAG, &["Skyrim.esm"]),
    )
    .unwrap();
    let mods = [env.installed("M", &[("A.esp", plugin_bytes(0, &[]))])];
    let (found, list) = env.list(&[], &mods);
    // named as on disk, first, and not warned about
    let names: Vec<_> = found.items.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["enderal - forgotten stories.esm", "A.esp"]);
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    assert_eq!(found.items[0].after, ["Skyrim.esm"]);
    // and written for the game, unlike the implicit ones
    let (_, text) = env.build(&list);
    assert!(
        text.contains("*enderal - forgotten stories.esm\r\n*A.esp"),
        "{text}"
    );
}

/// Sets `key` in the game's `[ext.bethesda.plugins]`.
fn set(env: &mut Env, key: &str, value: toml::Value) {
    env.def.ext.get_mut(EXTENSION).unwrap()[LIST]
        .as_table_mut()
        .unwrap()
        .insert(key.to_owned(), value);
}

fn set_time(path: &std::path::Path, secs: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

fn time_of(path: &std::path::Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

#[test]
fn oblivions_shorter_headers_are_read() {
    let mut env = Env::new();
    set(&mut env, "header_size", toml::Value::Integer(20));
    let mods = [env.installed(
        "M",
        &[(
            "Mod.esp",
            plugin_bytes_sized(20, 0, &["Oblivion.esm", "Base.esm"]),
        )],
    )];
    let found = env.items(&mods);
    assert!(found.warnings.is_empty(), "{:?}", found.warnings);
    assert_eq!(found.items[0].after, ["Oblivion.esm", "Base.esm"]);

    set(&mut env, "header_size", toml::Value::Integer(16));
    let registry = Registry::default();
    let ctx = DeployContext {
        game: &env.def,
        config: &env.def.deployer.config,
        registry: &registry,
    };
    let state = DeployState::empty("skyrim-se", &Roots::game(&env.game));
    let files = GameFiles::new(&env.game, &state);
    let config = env.def.hook_config(EXTENSION, LIST).unwrap();
    let lctx = ListContext::new(&ctx, &files, config);
    assert!(BethesdaLoadOrder.items(&lctx, &mods).is_err());
}

#[test]
fn games_that_go_by_dates_get_dated_copies_of_mods_plugins() {
    let mut env = Env::new();
    set(
        &mut env,
        "order",
        toml::Value::String("timestamps".to_owned()),
    );
    set(
        &mut env,
        "implicit",
        toml::Value::Array(vec![toml::Value::String("FalloutNV.esm".to_owned())]),
    );
    set(
        &mut env,
        "shipped",
        toml::Value::Array(vec![toml::Value::String("DeadMoney.esm".to_owned())]),
    );
    let data = env.game.join("Data");
    for (name, secs) in [
        ("FalloutNV.esm", 1_300_000_000),
        ("DeadMoney.esm", 1_310_000_000),
    ] {
        fs::write(data.join(name), plugin_bytes(MASTER_FLAG, &[])).unwrap();
        set_time(&data.join(name), secs);
    }
    let mods = [
        env.installed(
            "M",
            &[
                ("Data/Base.esm", plugin_bytes(MASTER_FLAG, &[])),
                ("A.esp", plugin_bytes(0, &[])),
                ("B.esp", plugin_bytes(0, &[])),
            ],
        ),
        env.installed("Off", &[("Off.esp", plugin_bytes(0, &[]))]),
    ];
    let saved = [
        ListEntry {
            name: "DeadMoney.esm".to_owned(),
            enabled: true,
        },
        ListEntry {
            name: "Base.esm".to_owned(),
            enabled: true,
        },
        ListEntry {
            name: "B.esp".to_owned(),
            enabled: true,
        },
        ListEntry {
            name: "A.esp".to_owned(),
            enabled: true,
        },
        ListEntry {
            name: "Off.esp".to_owned(),
            enabled: false,
        },
    ];
    let (_, order) = env.list(&saved, &mods);
    let (built, text) = env.build_with(&order, &mods);

    // only the enabled ones, the game's master too, without `*`s
    let lines: Vec<_> = text.lines().filter(|l| !l.starts_with('#')).collect();
    assert_eq!(
        lines,
        [
            "FalloutNV.esm",
            "DeadMoney.esm",
            "Base.esm",
            "B.esp",
            "A.esp"
        ]
    );
    assert!(built.warnings.is_empty(), "{:?}", built.warnings);

    // the enabled mods' plugins, dated in order after the game's own
    let out = env.tmp.path().join("out");
    let dated: Vec<_> = built.placements[1..]
        .iter()
        .map(|p| {
            let Src::Built(rel) = &p.src else {
                panic!("{p:?}")
            };
            (
                p.dst.to_string(),
                p.mod_name.as_str(),
                time_of(&rel.under(&out)),
            )
        })
        .collect();
    let names: Vec<_> = dated.iter().map(|(d, m, _)| (d.as_str(), *m)).collect();
    assert_eq!(
        names,
        [
            ("Data/Base.esm", "M"),
            ("Data/B.esp", "M"),
            ("Data/A.esp", "M")
        ]
    );
    let game_latest = time_of(&data.join("DeadMoney.esm"));
    assert!(dated[0].2 > game_latest);
    assert!(dated[0].2 < dated[1].2 && dated[1].2 < dated[2].2);
    // and the store's own files keep theirs
    assert_ne!(time_of(&mods[0].files[2].src), dated[1].2);

    // a game plugin after a mod's can't be moved there
    let late = [
        ListEntry {
            name: "Base.esm".to_owned(),
            enabled: true,
        },
        ListEntry {
            name: "DeadMoney.esm".to_owned(),
            enabled: true,
        },
    ];
    let (_, order) = env.list(&late, &mods);
    let (built, _) = env.build_with(&order, &mods);
    assert_eq!(built.warnings.len(), 1, "{:?}", built.warnings);
    assert!(built.warnings[0].starts_with("DeadMoney.esm is the game's own"));
}

#[test]
fn the_dates_of_the_games_plugins_are_fingerprinted() {
    let mut env = Env::new();
    env.def.ext.get_mut(EXTENSION).unwrap()[LIST]
        .as_table_mut()
        .unwrap()
        .remove("ccc");
    let fingerprint = |env: &Env| {
        let registry = Registry::default();
        let ctx = DeployContext {
            game: &env.def,
            config: &env.def.deployer.config,
            registry: &registry,
        };
        let state = DeployState::empty("skyrim-se", &Roots::game(&env.game));
        let game = GameFiles::new(&env.game, &state);
        BethesdaDeployer.fingerprint(&ctx, &game).unwrap()
    };
    let master = env.game.join("Data/FalloutNV.esm");
    fs::write(&master, plugin_bytes(MASTER_FLAG, &[])).unwrap();
    // nothing to cover without a ccc file, for games that don't go by dates
    assert!(fingerprint(&env).is_none());
    set(
        &mut env,
        "order",
        toml::Value::String("timestamps".to_owned()),
    );
    set_time(&master, 1_300_000_000);
    let before = fingerprint(&env).unwrap();
    set_time(&master, 1_400_000_000);
    assert_ne!(fingerprint(&env).unwrap(), before);
}
