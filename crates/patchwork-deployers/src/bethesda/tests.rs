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
    bytes.extend_from_slice(&[0; 12]);
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
            provision: None,
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
        let built = BethesdaDeployer.build(&ctx, &build, &[]).unwrap();
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
        read_header(&path).unwrap(),
        Header {
            master: true,
            light: true,
            masters: vec!["Skyrim.esm".to_owned(), "Base.esm".to_owned()],
        }
    );
    fs::write(&path, plugin_bytes(0, &[])).unwrap();
    assert_eq!(read_header(&path).unwrap(), Header::default());

    for (bytes, why) in [
        (b"TES4".to_vec(), "too short"),
        (
            b"TES3\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0".to_vec(),
            "TES4",
        ),
        (plugin_bytes(0, &["A.esm"])[..40].to_vec(), "cut short"),
    ] {
        fs::write(&path, bytes).unwrap();
        let err = read_header(&path).unwrap_err();
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
