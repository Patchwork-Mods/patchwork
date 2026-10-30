use super::*;
use crate::registry::Registry;

/// [`super::plan`] with only the built-in runners.
fn plan(game: &GameDef, option: Option<&str>, setup: &Setup) -> Result<Launch> {
    let (registered, registry) = (BTreeMap::new(), Registry::default());
    super::plan(game, option, setup, &Runners::new(&registered, &registry))
}

/// [`super::plan_tool`] with only the built-in runners.
fn plan_tool(game: &GameDef, setup: &Setup, exe: &Path, args: Vec<String>) -> Result<Launch> {
    let (registered, registry) = (BTreeMap::new(), Registry::default());
    super::plan_tool(
        game,
        setup,
        exe,
        args,
        &Runners::new(&registered, &registry),
    )
}

fn game(extra: &str) -> GameDef {
    toml::from_str(&format!(
        "schema = 1\nid = \"g\"\nname = \"G\"\n[deployer]\nkind = \"rules\"\n[detect]\nsteam = [10]\nexe = [\"Game.exe\"]\n{extra}"
    ))
    .unwrap()
}

fn rel(path: &str) -> RelPath {
    RelPath::new(path).unwrap()
}

/// A game folder with `files` in it, set up as Steam installs it on Linux
/// and run through Proton.
fn steam_setup(tmp: &Path, files: &[&str]) -> Setup {
    let install = tmp.join("steamapps/common/Game");
    for file in files {
        let path = rel(file).under(&install);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }
    Setup {
        os: Os::Linux,
        install,
        steam: Some(SteamInstall {
            app: 10,
            library: tmp.to_owned(),
        }),
        steam_client: Some(tmp.to_owned()),
        prefix: Some(tmp.join("steamapps/compatdata/10/pfx")),
        proton: Some(tmp.join("Proton 9.0")),
        runtime: None,
        fhs: None,
        deployed: Vec::new(),
        launch: None,
        runner: None,
        runner_settings: BTreeMap::new(),
    }
}

fn ids(options: &[LaunchOption]) -> Vec<(&str, bool)> {
    options.iter().map(|o| (o.id.as_str(), o.default)).collect()
}

#[test]
fn the_usual_way_is_always_there_and_first() {
    let tmp = tempfile::tempdir().unwrap();
    let game = game("[launch.b]\nname = \"B\"\n[launch.a]\nargs = [\"-x\"]");
    let options = options(&game, tmp.path(), None);
    assert_eq!(ids(&options), [("game", true), ("a", false), ("b", false)]);
    assert_eq!(options[0].name, "G");
    assert_eq!(options[1].name, "a");
    assert_eq!(options[2].name, "B");
}

#[test]
fn the_default_is_only_used_once_its_exe_is_there() {
    let tmp = tempfile::tempdir().unwrap();
    let game = game("[launch.skse]\nexe = \"skse64_loader.exe\"\ndefault = true");
    let options = options(&game, tmp.path(), None);
    assert_eq!(ids(&options), [("game", true), ("skse", false)]);
    assert_eq!(
        options[1].unavailable.as_deref(),
        Some("skse64_loader.exe isn't in the game's folder")
    );

    std::fs::write(tmp.path().join("skse64_loader.exe"), "").unwrap();
    let options = super::options(&game, tmp.path(), None);
    assert_eq!(ids(&options), [("game", false), ("skse", true)]);
    assert_eq!(options[1].unavailable, None);
}

#[test]
fn definitions_are_checked() {
    let err = |extra: &str| {
        let text =
            format!("schema = 1\nid = \"g\"\nname = \"G\"\n[deployer]\nkind = \"rules\"\n{extra}");
        toml::from_str::<GameDef>(&text).unwrap_err().to_string()
    };
    assert!(
        err("[launch.a]\ndefault = true\n[launch.b]\ndefault = true")
            .contains("only one way to launch can be the default, not a and b")
    );
    assert!(err("[launch.Big]\n").contains("isn't a valid launch id"));
    assert!(err("[launch.a]\ndll_overrides = [\"dxgi=n,b\"]").contains("isn't a DLL name"));
    assert!(err("[launch.a]\nexe = \"../x.exe\"").contains("points outside"));
}

#[test]
fn steam_games_start_through_steam_when_nothing_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = steam_setup(tmp.path(), &["Game.exe"]);
    let game = game("[launch.game]\nargs = [\"-skipintro\"]");
    let launch = plan(&game, None, &setup).unwrap();
    assert_eq!(launch.via, Via::Steam { app: 10 });
    assert_eq!(launch.program, PathBuf::from("steam"));
    assert_eq!(launch.args, ["-applaunch", "10", "-skipintro"]);
    assert!(launch.env.is_empty());
    assert!(!launch.needs_steam());
}

#[test]
fn flatpak_steam_is_run_through_flatpak() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.steam_client = Some(PathBuf::from(
        "/home/u/.var/app/com.valvesoftware.Steam/.local/share/Steam",
    ));
    let launch = plan(&game(""), None, &setup).unwrap();
    assert_eq!(launch.program, PathBuf::from("flatpak"));
    assert_eq!(
        launch.args,
        ["run", "com.valvesoftware.Steam", "-applaunch", "10"]
    );
}

#[test]
fn another_exe_runs_through_proton() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = steam_setup(tmp.path(), &["Game.exe", "skse64_loader.exe"]);
    let game = game("[launch.skse]\nexe = \"skse64_loader.exe\"\nargs = [\"-a\"]\ndefault = true");
    let launch = plan(&game, None, &setup).unwrap();
    assert_eq!(launch.option, "skse");
    assert_eq!(
        launch.via,
        Via::Proton {
            proton: tmp.path().join("Proton 9.0")
        }
    );
    assert_eq!(launch.program, tmp.path().join("Proton 9.0/proton"));
    let exe = setup.install.join("skse64_loader.exe");
    assert_eq!(launch.args, ["run", &*exe.to_string_lossy(), "-a"]);
    assert_eq!(launch.cwd, setup.install);
    let env: Vec<(&str, &str)> = launch
        .env
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let compat = tmp.path().join("steamapps/compatdata/10");
    assert_eq!(
        env,
        [
            (
                "STEAM_COMPAT_CLIENT_INSTALL_PATH",
                &*tmp.path().to_string_lossy()
            ),
            ("STEAM_COMPAT_DATA_PATH", &*compat.to_string_lossy()),
            ("SteamAppId", "10"),
            ("SteamGameId", "10"),
        ]
    );
    assert!(launch.needs_steam());

    // the usual way is still there
    let launch = plan(&game, Some("game"), &setup).unwrap();
    assert_eq!(launch.via, Via::Steam { app: 10 });
}

#[test]
fn deployed_proxy_dlls_get_overridden() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.deployed = vec![
        rel("winhttp.dll"),
        rel("doorstop_config.ini"),
        rel("BepInEx/core/version.dll"),
        rel("DXGI.DLL"),
    ];
    let game = game(
        "[launch.game]\ndll_overrides = [\"d3dcompiler_47\"]\nenv = { WINEDLLOVERRIDES = \"mscoree=d\" }",
    );
    let launch = plan(&game, None, &setup).unwrap();
    assert!(matches!(launch.via, Via::Proton { .. }));
    assert_eq!(
        launch.env["WINEDLLOVERRIDES"],
        "d3dcompiler_47=n,b;winhttp=n,b;dxgi=n,b;mscoree=d"
    );
}

#[test]
fn proxy_dlls_only_count_next_to_the_exe() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["bin/x64/Game.exe"]);
    setup.deployed = vec![rel("winhttp.dll"), rel("bin/X64/dinput8.dll")];
    let mut game = game("");
    game.detect.exe = vec!["bin/x64/Game.exe".to_owned()];
    let launch = plan(&game, None, &setup).unwrap();
    assert_eq!(launch.prefix_overrides.unwrap().dlls, ["dinput8"]);

    let game_env = |mut g: GameDef| {
        g.launch = self::game("[launch.game]\nenv = { A = \"1\" }").launch;
        g
    };
    let launch = plan(&game_env(game), None, &setup).unwrap();
    assert_eq!(launch.env["WINEDLLOVERRIDES"], "dinput8=n,b");
    assert_eq!(launch.cwd, setup.install.join("bin/x64"));
}

#[test]
fn overrides_go_in_the_prefix_for_steam_to_start_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.deployed = vec![rel("winhttp.dll")];
    let launch = plan(&game(""), None, &setup).unwrap();
    assert_eq!(launch.via, Via::Steam { app: 10 });
    assert!(launch.env.is_empty());
    assert_eq!(
        launch.prefix_overrides,
        Some(PrefixOverrides {
            prefix: tmp.path().join("steamapps/compatdata/10/pfx"),
            dlls: vec!["winhttp".to_owned()],
        })
    );

    // without the prefix they can't be
    setup.prefix = None;
    let err = plan(&game(""), None, &setup).unwrap_err().to_string();
    assert!(err.contains("winhttp.dll"), "{err}");
    assert!(err.contains("start it from Steam once"), "{err}");
}

#[test]
fn proton_runs_inside_its_runtime() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe", "skse64_loader.exe"]);
    let runtime = tmp.path().join("SteamLinuxRuntime_sniper");
    setup.runtime = Some((1628350, Some(runtime.clone())));
    let game = game("[launch.skse]\nexe = \"skse64_loader.exe\"\nargs = [\"-a\"]");
    let launch = plan(&game, Some("skse"), &setup).unwrap();
    assert_eq!(launch.program, runtime.join("_v2-entry-point"));
    let proton = tmp.path().join("Proton 9.0/proton");
    let exe = setup.install.join("skse64_loader.exe");
    assert_eq!(
        launch.args,
        [
            "--verb=waitforexitandrun",
            "--",
            &*proton.to_string_lossy(),
            "waitforexitandrun",
            &*exe.to_string_lossy(),
            "-a"
        ]
    );

    // through steam-run on NixOS
    setup.fhs = Some(PathBuf::from("/bin/steam-run"));
    let launch = plan(&game, Some("skse"), &setup).unwrap();
    assert_eq!(launch.program, PathBuf::from("/bin/steam-run"));
    assert_eq!(
        launch.args[0],
        runtime.join("_v2-entry-point").to_string_lossy()
    );

    // and not without it
    setup.runtime = Some((1628350, None));
    let err = plan(&game, Some("skse"), &setup).unwrap_err().to_string();
    assert!(err.contains("Steam app 1628350"), "{err}");
}

#[test]
fn overrides_are_added_to_the_registry() {
    let reg = "WINE REGISTRY Version 2\n\n[Software\\\\Wine\\\\DllOverrides] 1767971658\n#time=1dc\n\"d3d11\"=\"builtin\"\n\"dxgi\"=\"n,b\"\n\n[Software\\\\Wine\\\\X] 1\n\"winhttp\"=\"builtin\"\n";
    let dlls = ["winhttp", "d3d11", "dxgi"].map(str::to_owned);
    let (text, added) = with_overrides(reg, &dlls);
    assert_eq!(added, ["winhttp", "d3d11"]);
    assert_eq!(
        text,
        "WINE REGISTRY Version 2\n\n[Software\\\\Wine\\\\DllOverrides] 1767971658\n#time=1dc\n\"winhttp\"=\"native,builtin\"\n\"d3d11\"=\"native,builtin\"\n\"dxgi\"=\"n,b\"\n\n[Software\\\\Wine\\\\X] 1\n\"winhttp\"=\"builtin\"\n"
    );
    // a second time there's nothing to add
    assert_eq!(with_overrides(&text, &dlls), (text.clone(), Vec::new()));

    // and a prefix without the key gets it
    let (text, added) = with_overrides("WINE REGISTRY Version 2\n", &dlls[..1]);
    assert_eq!(added, ["winhttp"]);
    assert!(
        text.contains("\n\n[Software\\\\Wine\\\\DllOverrides] "),
        "{text}"
    );
    assert!(
        text.ends_with("\n\"winhttp\"=\"native,builtin\"\n"),
        "{text}"
    );
}

#[cfg(unix)]
#[test]
fn games_that_fail_straight_away_say_why() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("logs/game.log");
    let launch = |script: &str| Launch {
        option: "game".to_owned(),
        via: Via::Direct,
        program: PathBuf::from("sh"),
        args: vec!["-c".to_owned(), script.to_owned()],
        env: BTreeMap::new(),
        cwd: tmp.path().to_owned(),
        prefix_overrides: None,
    };
    let err = launch("echo _r_debug not found; exit 1")
        .spawn(&log)
        .unwrap_err()
        .to_string();
    assert!(err.contains("_r_debug not found"), "{err}");
    assert!(err.contains("logs/game.log"), "{err}");

    // ones that hand off to something else and exit are fine
    assert_eq!(launch("exit 0").spawn(&log).unwrap(), Started::default());
}

#[test]
fn nothing_is_overridden_on_windows() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.os = Os::Windows;
    setup.prefix = None;
    setup.proton = None;
    setup.deployed = vec![rel("winhttp.dll")];
    let launch = plan(&game(""), None, &setup).unwrap();
    assert_eq!(launch.via, Via::Steam { app: 10 });
    assert_eq!(launch.program, tmp.path().join("steam.exe"));

    let game = game("[launch.game]\nenv = { A = \"1\" }");
    let launch = plan(&game, None, &setup).unwrap();
    assert_eq!(launch.via, Via::Direct);
    assert_eq!(launch.program, setup.install.join("Game.exe"));
    assert_eq!(
        launch.env.keys().collect::<Vec<_>>(),
        ["A", "SteamAppId", "SteamGameId"]
    );
}

#[test]
fn native_builds_run_directly() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["game.x86_64"]);
    setup.prefix = None;
    setup.proton = None;
    setup.deployed = vec![rel("winhttp.dll")];
    let mut game = game("[launch.game]\nargs = [\"-a\"]\nenv = { A = \"1\" }");
    game.detect.exe = vec!["game.x86_64".to_owned()];
    let launch = plan(&game, None, &setup).unwrap();
    assert_eq!(launch.via, Via::Direct);
    assert_eq!(launch.program, setup.install.join("game.x86_64"));
    assert_eq!(launch.args, ["-a"]);
    assert!(!launch.env.contains_key("WINEDLLOVERRIDES"));
}

#[test]
fn windows_games_from_elsewhere_need_a_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.steam = None;
    setup.steam_client = None;
    setup.prefix = None;
    setup.proton = None;
    let err = plan(&game(""), None, &setup).unwrap_err().to_string();
    assert!(err.contains("patchwork game set g prefix <path>"), "{err}");

    setup.prefix = Some(tmp.path().join("wine"));
    let launch = plan(&game(""), None, &setup).unwrap();
    assert_eq!(
        launch.via,
        Via::Wine {
            prefix: tmp.path().join("wine")
        }
    );
    assert_eq!(launch.program, PathBuf::from("wine"));
    let exe = setup.install.join("Game.exe");
    assert_eq!(launch.args, [&*exe.to_string_lossy()]);
    assert_eq!(
        launch.env["WINEPREFIX"],
        tmp.path().join("wine").to_string_lossy()
    );
    assert!(!launch.env.contains_key("SteamAppId"));
}

#[test]
fn a_steam_game_needs_its_proton() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.proton = None;
    let game = game("[launch.game]\nenv = { A = \"1\" }");
    let err = plan(&game, None, &setup).unwrap_err().to_string();
    assert!(err.contains("start it from Steam once"), "{err}");

    // but a prefix of the user's own is run with Wine
    setup.prefix = Some(tmp.path().join("wine"));
    assert!(matches!(
        plan(&game, None, &setup).unwrap().via,
        Via::Wine { .. }
    ));
}

#[test]
fn options_have_to_exist_and_be_available() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = steam_setup(tmp.path(), &["Game.exe"]);
    let game = game("[launch.skse]\nexe = \"skse64_loader.exe\"");
    let err = plan(&game, Some("nope"), &setup).unwrap_err().to_string();
    assert!(err.contains("it can be game, skse"), "{err}");
    let err = plan(&game, Some("skse"), &setup).unwrap_err().to_string();
    assert!(
        err.contains("skse64_loader.exe isn't in the game's folder"),
        "{err}"
    );
}

#[test]
fn games_without_a_known_exe_say_so() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &[]);
    setup.steam = None;
    let err = plan(&game(""), None, &setup).unwrap_err().to_string();
    assert!(err.contains("doesn't know which file starts G"), "{err}");
}

#[test]
fn commands_print_as_a_shell_would_take_them() {
    let launch = Launch {
        option: "game".to_owned(),
        via: Via::Direct,
        program: PathBuf::from("/games/My Game/game"),
        args: vec!["-a".to_owned(), "it's".to_owned()],
        env: BTreeMap::from([("WINEDLLOVERRIDES".to_owned(), "winhttp=n,b".to_owned())]),
        cwd: PathBuf::from("/games/My Game"),
        prefix_overrides: None,
    };
    assert_eq!(
        launch.to_string(),
        r"WINEDLLOVERRIDES=winhttp=n,b '/games/My Game/game' -a 'it'\''s'"
    );
}

#[test]
fn tools_run_from_their_own_folder_in_the_games_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = steam_setup(tmp.path(), &["Game.exe"]);
    let exe = tmp.path().join("DynDOLOD/DynDOLODx64.exe");
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "").unwrap();
    let launch = plan_tool(&game(""), &setup, &exe, vec!["-SSE".to_owned()]).unwrap();
    assert_eq!(launch.option, TOOL);
    assert_eq!(launch.program, tmp.path().join("Proton 9.0/proton"));
    assert_eq!(launch.args, ["run", &*exe.to_string_lossy(), "-SSE"]);
    assert_eq!(launch.cwd, exe.parent().unwrap());
    // it's not the game, so Steam isn't told it is
    assert!(!launch.needs_steam());

    let missing = tmp.path().join("missing.exe");
    assert!(plan_tool(&game(""), &setup, &missing, Vec::new()).is_err());
}

fn with_runner(toml: &str) -> Registry {
    let mut registry = Registry::default();
    registry.add_data(runner::KIND, "mine", toml);
    registry
}

fn plan_with(registry: &Registry, game: &GameDef, setup: &Setup) -> Result<Launch> {
    let registered = BTreeMap::new();
    super::plan(game, None, setup, &Runners::new(&registered, registry))
}

/// A file that a runner's command can name as its program.
fn program(tmp: &Path) -> String {
    let path = tmp.join("mine-run");
    std::fs::write(&path, "").unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn runners_listed_for_a_launch_replace_the_usual_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = steam_setup(tmp.path(), &["Game.exe"]);
    let game = game("[launch.game]\nrunners = [\"proton\"]");
    // Steam would otherwise be first
    let launch = plan(&game, None, &setup).unwrap();
    assert!(matches!(launch.via, Via::Proton { .. }));
}

#[test]
fn the_first_runner_that_can_is_used_and_the_rest_explain() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.proton = None;
    let list = "[launch.game]\nrunners = [\"native\", \"proton\", \"wine\"]";
    // native doesn't suit it, so what's said is what Proton's missing
    let err = plan(&game(list), None, &setup).unwrap_err().to_string();
    assert!(err.contains("can't find which Proton"), "{err}");

    setup.prefix = Some(tmp.path().join("wine"));
    let launch = plan(&game(list), None, &setup).unwrap();
    assert!(matches!(launch.via, Via::Wine { .. }));

    // with nothing better to say, it says why it doesn't suit
    let only_steam = game("[launch.game]\nenv = { A = \"1\" }\nrunners = [\"steam\"]");
    let err = plan(&only_steam, None, &setup).unwrap_err().to_string();
    assert!(err.contains("Steam can only start"), "{err}");

    let unknown = game("[launch.game]\nrunners = [\"nope\"]");
    let err = plan(&unknown, None, &setup).unwrap_err().to_string();
    assert!(err.contains("no runner named `nope`"), "{err}");
}

#[test]
fn runners_can_be_data() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.deployed = vec![rel("winhttp.dll")];
    let registry = with_runner(&format!(
        "name = \"Mine\"\ncommand = [\"{}\", \"--app=${{steam_app}}\", \"${{exe}}\", \"${{args...}}\"]\nenv = {{ PFX = \"${{prefix}}\", A = \"runner\" }}\n[applies]\nexe = \"*.EXE\"\nos = [\"linux\"]",
        program(tmp.path())
    ));
    let game = game(
        "[launch.game]\nargs = [\"-a\", \"-b\"]\nenv = { A = \"game\" }\nrunners = [\"mine\"]",
    );
    let launch = plan_with(&registry, &game, &setup).unwrap();
    let exe = setup.install.join("Game.exe");
    assert_eq!(
        launch.via,
        Via::Runner {
            name: "Mine".to_owned()
        }
    );
    assert_eq!(launch.program, tmp.path().join("mine-run"));
    assert_eq!(
        launch.args,
        ["--app=10", &*exe.to_string_lossy(), "-a", "-b"]
    );
    assert_eq!(launch.cwd, setup.install);
    assert_eq!(
        launch.env["PFX"],
        tmp.path()
            .join("steamapps/compatdata/10/pfx")
            .to_string_lossy()
    );
    assert_eq!(launch.env["A"], "game");
    assert_eq!(launch.env["WINEDLLOVERRIDES"], "winhttp=n,b");
    assert_eq!(launch.env["SteamAppId"], "10");
}

#[test]
fn runners_from_data_say_when_they_cant_run_something() {
    let tmp = tempfile::tempdir().unwrap();
    let prog = program(tmp.path());
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    let game = game("[launch.game]\nrunners = [\"mine\"]");
    let err_with = |toml: String, setup: &Setup| {
        plan_with(&with_runner(&toml), &game, setup)
            .unwrap_err()
            .to_string()
    };

    let err = err_with(
        format!("command = [\"{prog}\"]\n[applies]\nos = [\"windows\"]"),
        &setup,
    );
    assert!(err.contains("isn't for this system"), "{err}");
    let err = err_with(
        format!("command = [\"{prog}\"]\n[applies]\nexe = \"*.jar\""),
        &setup,
    );
    assert!(err.contains("doesn't run"), "{err}");
    let err = err_with(format!("command = [\"{prog}\", \"${{nope}}\"]"), &setup);
    assert!(err.contains("`${nope}`"), "{err}");
    let err = err_with("command = [\"no-such-program-here\"]".to_owned(), &setup);
    assert!(err.contains("needs no-such-program-here"), "{err}");

    let err = err_with("command = []".to_owned(), &setup);
    assert!(err.contains("bad runner `mine`"), "{err}");
    let err = err_with("command = [\"a\"]\nnope = 1".to_owned(), &setup);
    assert!(err.contains("bad runner `mine`"), "{err}");

    setup.steam = None;
    setup.prefix = None;
    let err = err_with(format!("command = [\"{prog}\", \"${{prefix}}\"]"), &setup);
    assert!(err.contains("patchwork game set g prefix"), "{err}");
    let err = err_with(
        format!("command = [\"{prog}\", \"${{steam_app}}\"]"),
        &setup,
    );
    assert!(err.contains("needs a game from Steam"), "{err}");
}

#[test]
fn a_file_can_replace_a_built_in_runner() {
    let tmp = tempfile::tempdir().unwrap();
    let setup = steam_setup(tmp.path(), &["Game.exe"]);
    let mut registry = Registry::default();
    registry.add_data(
        runner::KIND,
        "steam",
        &format!("command = [\"{}\"]", program(tmp.path())),
    );
    let launch = plan_with(&registry, &game(""), &setup).unwrap();
    assert_eq!(
        launch.via,
        Via::Runner {
            name: "steam".to_owned()
        }
    );
}

#[test]
fn the_bundled_runners_parse() {
    let table = toml::from_str(include_str!("../../../../registry/runners/umu.toml")).unwrap();
    runner::Template::parse("umu", &table).unwrap();
}

#[test]
fn runners_dont_count_as_deploy_inputs() {
    let mut registry = Registry::default();
    registry.add_data(runner::KIND, "mine", "command = [\"a\"]");
    registry.add_data("rulesets", "r", "rules = []");
    let kinds: Vec<&str> = registry.data_hashes().map(|(kind, ..)| kind).collect();
    assert_eq!(kinds, ["rulesets"]);
}

#[test]
fn a_picked_runner_replaces_the_launchs_list_and_doesnt_fall_back() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.runner = Some("proton".to_owned());
    let launch = plan(&game("[launch.game]\nrunners = [\"steam\"]"), None, &setup).unwrap();
    assert!(matches!(launch.via, Via::Proton { .. }));

    setup.runner = Some("native".to_owned());
    let err = plan(&game(""), None, &setup).unwrap_err().to_string();
    assert!(err.contains("can't run by itself"), "{err}");
}

#[test]
fn tools_use_the_picked_runner_if_it_can_run_them() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    let exe = setup.install.join("Game.exe");
    setup.runner = Some("steam".to_owned());
    let tool = plan_tool(&game(""), &setup, &exe, Vec::new()).unwrap();
    assert!(matches!(tool.via, Via::Proton { .. }));

    let registry = with_runner(&format!(
        "command = [\"{}\", \"${{exe}}\"]",
        program(tmp.path())
    ));
    setup.runner = Some("mine".to_owned());
    let registered = BTreeMap::new();
    let tool = super::plan_tool(
        &game(""),
        &setup,
        &exe,
        Vec::new(),
        &Runners::new(&registered, &registry),
    )
    .unwrap();
    assert!(matches!(tool.via, Via::Runner { .. }));
}

#[test]
fn a_picked_launch_option_becomes_the_default_while_it_can_be_used() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.exe"), "").unwrap();
    let game = game(
        "[launch.a]\nexe = \"a.exe\"\n[launch.b]\nexe = \"b.exe\"\n[launch.game]\nname = \"G\"",
    );
    assert_eq!(
        ids(&options(&game, tmp.path(), Some("a"))),
        [("game", false), ("a", true), ("b", false)]
    );
    for gone in ["b", "nope"] {
        assert_eq!(
            ids(&options(&game, tmp.path(), Some(gone))),
            [("game", true), ("a", false), ("b", false)]
        );
    }
}

#[test]
fn wine_runs_the_program_it_was_set_to() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.steam = None;
    setup.prefix = Some(tmp.path().join("wine"));
    let settings = |program: String| {
        BTreeMap::from([(
            "wine".to_owned(),
            toml::Table::from_iter([("program".to_owned(), toml::Value::from(program))]),
        )])
    };
    setup.runner_settings = settings(program(tmp.path()));
    let launch = plan(&game(""), None, &setup).unwrap();
    assert_eq!(launch.program, tmp.path().join("mine-run"));

    setup.runner_settings = settings("no-such-wine-here".to_owned());
    let err = plan(&game(""), None, &setup).unwrap_err().to_string();
    assert!(err.contains("no-such-wine-here"), "{err}");
}

#[test]
fn a_chosen_proton_has_to_be_there() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    setup.runner = Some("proton".to_owned());
    setup.runner_settings = BTreeMap::from([(
        "proton".to_owned(),
        toml::Table::from_iter([("folder".to_owned(), toml::Value::from("set"))]),
    )]);
    let err = plan(&game(""), None, &setup).unwrap_err().to_string();
    assert!(err.contains("there's no Proton in"), "{err}");

    let proton = setup.proton.clone().unwrap();
    std::fs::create_dir_all(&proton).unwrap();
    std::fs::write(proton.join("proton"), "").unwrap();
    assert!(plan(&game(""), None, &setup).is_ok());
}

#[test]
fn runners_from_data_have_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let mut setup = steam_setup(tmp.path(), &["Game.exe"]);
    let registry = with_runner(&format!(
        "command = [\"{}\", \"--with=${{extra}}\", \"${{mode}}\"]\nenv = {{ PICK = \"${{extra}}\", MODE = \"${{mode}}\" }}\n[settings.extra]\nlabel = \"Extra\"\nkind = \"path\"\n[settings.mode]\nlabel = \"Mode\"\ndefault = \"fast\"",
        program(tmp.path())
    ));
    let game = game("[launch.game]\nrunners = [\"mine\"]");

    // with no value it's left out, and the default is used
    let launch = plan_with(&registry, &game, &setup).unwrap();
    assert_eq!(launch.args, ["fast"]);
    assert!(!launch.env.contains_key("PICK"));
    assert_eq!(launch.env["MODE"], "fast");

    setup.runner_settings = BTreeMap::from([(
        "mine".to_owned(),
        toml::Table::from_iter([
            ("extra".to_owned(), toml::Value::from("/x")),
            ("mode".to_owned(), toml::Value::from("slow")),
        ]),
    )]);
    let launch = plan_with(&registry, &game, &setup).unwrap();
    assert_eq!(launch.args, ["--with=/x", "slow"]);
    assert_eq!(launch.env["PICK"], "/x");

    let registered = BTreeMap::new();
    let choices = runner_choices(&game, &setup, &Runners::new(&registered, &registry));
    assert_eq!(choices.automatic.as_deref(), Some("mine"));
    let ids: Vec<&str> = choices.runners.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["steam", "native", "proton", "wine", "mine"]);
    let mine = choices.runners.iter().find(|r| r.id == "mine").unwrap();
    assert_eq!(mine.settings[0].value, Some(toml::Value::from("/x")));
    assert!(mine.settings[0].origin.is_some());
    let native = choices.runners.iter().find(|r| r.id == "native").unwrap();
    assert!(native.unavailable.is_some());

    let reserved = "command = [\"a\"]\n[settings.exe]\nlabel = \"E\"";
    let table = toml::from_str(reserved).unwrap();
    assert!(runner::Template::parse("mine", &table).is_err());
}
