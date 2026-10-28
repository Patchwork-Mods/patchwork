use std::fs;
use std::path::PathBuf;

use patchwork_core::deploy::{DeployState, ModFile, Roots};
use patchwork_core::event::Silent;
use patchwork_core::game::{DeployerSpec, GameDef};
use patchwork_core::plugins::PluginEntry;
use patchwork_core::registry::Registry;

use super::*;

const CONFIG: &str = r#"
[[rules]]
folder = "Data"
to = "Data/{path}"

[[rules]]
to = "Data/{path}"

[plugins]
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
            fomod: None,
            capture: None,
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

    fn plugins(&self, mods: &[InstalledMod]) -> Plugins {
        let registry = Registry::default();
        let ctx = DeployContext {
            game: &self.def,
            config: &self.def.deployer.config,
            registry: &registry,
        };
        let state = DeployState::empty("skyrim-se", &Roots::game(&self.game));
        BethesdaDeployer
            .plugins(&ctx, &GameFiles::new(&self.game, &state), mods)
            .unwrap()
            .unwrap()
    }

    fn build(&self, order: &LoadOrder) -> (DesiredState, String) {
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
            load_order: Some(order),
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
    let plugins = env.plugins(&mods);
    let names: Vec<_> = plugins.found.iter().map(|p| p.name.as_str()).collect();
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
    let get = |name: &str| plugins.found.iter().find(|p| p.name == name).unwrap();
    assert_eq!(get("Base.esm").mod_name, "Wrapped");
    assert!(get("Base.esm").master && !get("Base.esm").light);
    assert_eq!(get("Patch.esp").masters, ["Skyrim.esm", "Base.esm"]);
    assert!(!get("Patch.esp").master);
    assert!(get("Small.esl").master && get("Small.esl").light);
    assert!(get("Flagged.esp").master);
    assert_eq!(plugins.warnings.len(), 1);
    assert!(
        plugins.warnings[0].contains("Broken.esp"),
        "{:?}",
        plugins.warnings
    );
    assert_eq!(plugins.implicit, ["Skyrim.esm", "Update.esm"]);
    assert_eq!(plugins.limit, Some(254));
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
    let plugins = env.plugins(&[]);
    assert_eq!(
        plugins.implicit,
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
    let plugins = env.plugins(&[]);
    assert_eq!(plugins.light_implicit, ["ccFlagged.esm", "ccLight.esl"]);
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
        PluginEntry {
            name: "B.esp".to_owned(),
            enabled: true,
        },
        PluginEntry {
            name: "A.esp".to_owned(),
            enabled: false,
        },
    ];
    let order = LoadOrder::new(&saved, env.plugins(&mods));
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
fn games_without_plugins_config_have_none() {
    let mut env = Env::new();
    env.def.deployer.config.remove("plugins");
    let registry = Registry::default();
    let ctx = DeployContext {
        game: &env.def,
        config: &env.def.deployer.config,
        registry: &registry,
    };
    let state = DeployState::empty("skyrim-se", &Roots::game(&env.game));
    let game = GameFiles::new(&env.game, &state);
    assert!(
        BethesdaDeployer
            .plugins(&ctx, &game, &[])
            .unwrap()
            .is_none()
    );
    assert!(!BethesdaDeployer.builds(&ctx));
    assert!(BethesdaDeployer.fingerprint(&ctx, &game).unwrap().is_none());
}
