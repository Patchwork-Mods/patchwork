use super::*;
use crate::game::Detect;

fn game(steam: &str) -> GameDef {
    toml::from_str(&format!(
        "schema = 1\nid = \"g\"\nname = \"G\"\n[deployer]\nkind = \"rules\"\n[detect]\nsteam = {steam}"
    ))
    .unwrap()
}

/// Puts `app` in the library at `library`, as Steam would.
fn install(library: &Path, app: u32, dir: &str) -> PathBuf {
    let steamapps = library.join("steamapps");
    let path = steamapps.join("common").join(dir);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        steamapps.join(format!("appmanifest_{app}.acf")),
        format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"{app}\"\n\t\"installdir\"\t\t\"{dir}\"\n}}\n"),
    )
    .unwrap();
    path.canonicalize().unwrap()
}

fn library_folders(root: &Path, body: &str) {
    let steamapps = root.join("steamapps");
    std::fs::create_dir_all(&steamapps).unwrap();
    std::fs::write(
        steamapps.join("libraryfolders.vdf"),
        format!("\"libraryfolders\"\n{{\n{body}\n}}\n"),
    )
    .unwrap();
}

fn paths(found: Vec<Found>) -> Vec<PathBuf> {
    found.into_iter().map(|f| f.path).collect()
}

#[test]
fn finds_games_in_every_library() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Steam");
    let other = tmp.path().join("SteamLibrary");
    let in_root = install(&root, 1, "One");
    let elsewhere = install(&other, 2, "Two");
    library_folders(
        &root,
        &format!(
            "\"0\" {{ \"path\" \"{}\" \"apps\" {{ \"1\" \"0\" }} }}\n\
             \"1\" {{ \"path\" \"{}\" \"apps\" {{ \"2\" \"0\" }} }}",
            root.display(),
            other.display()
        ),
    );

    let locator = Locator::steam([root.clone()]);
    assert_eq!(paths(locator.find(&game("[1]"))), [in_root]);
    let found = locator.find(&game("[3, 2]"));
    assert_eq!(found[0].path, elsewhere);
    assert_eq!(
        found[0].origin,
        Origin::Steam {
            app: 2,
            library: other.canonicalize().unwrap()
        }
    );
    assert!(locator.find(&game("[3]")).is_empty());
}

#[test]
fn reads_the_old_library_list() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Steam");
    let other = tmp.path().join("SteamLibrary");
    let path = install(&other, 2, "Two");
    // before 2021, the root wasn't listed and the others were just paths
    library_folders(
        &root,
        &format!(
            "\"TimeNextStatsReport\" \"1600000000\"\n\"ContentStatsID\" \"123\"\n\"1\" \"{}\"",
            other.display()
        ),
    );
    let locator = Locator::steam([root]);
    assert_eq!(paths(locator.find(&game("[2]"))), [path]);
}

#[test]
fn unescapes_windows_paths() {
    let folders = r#""libraryfolders" { "0" { "path" "D:\\SteamLibrary" } }"#;
    let parsed = vdf::parse(folders).unwrap();
    let library = parsed.get("libraryfolders").unwrap().get("0").unwrap();
    assert_eq!(library.get_str("path"), Some(r"D:\SteamLibrary"));
}

#[test]
fn skips_manifests_whose_folder_is_gone() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Steam");
    let path = install(&root, 1, "One");
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(root.join("steamapps/appmanifest_2.acf"), "not keyvalues {").unwrap();
    let locator = Locator::steam([root]);
    assert!(locator.find(&game("[1, 2]")).is_empty());
}

#[cfg(unix)]
#[test]
fn installs_reachable_twice_are_found_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("Steam");
    let path = install(&root, 1, "One");
    let link = tmp.path().join("steam-link");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    // and the root lists itself, as newer Steams do
    library_folders(
        &root,
        &format!("\"0\" {{ \"path\" \"{}\" }}", link.display()),
    );
    let locator = Locator::steam([root, link, tmp.path().join("missing")]);
    assert_eq!(paths(locator.find(&game("[1]"))), [path]);
}

#[test]
fn proton_prefixes_are_next_to_the_steam_install() {
    let tmp = tempfile::tempdir().unwrap();
    let steamapps = tmp.path().join("steamapps");
    let game_dir = steamapps.join("common/Game");
    std::fs::create_dir_all(&game_dir).unwrap();
    let game = game("[10, 20]");
    assert_eq!(proton_prefix(&game, &game_dir), None);
    // a prefix only counts once Proton has made it
    std::fs::create_dir_all(steamapps.join("compatdata/10/pfx")).unwrap();
    assert_eq!(proton_prefix(&game, &game_dir), None);
    std::fs::create_dir_all(steamapps.join("compatdata/20/pfx/drive_c")).unwrap();
    assert_eq!(
        proton_prefix(&game, &game_dir),
        Some(steamapps.join("compatdata/20/pfx"))
    );
    // not in a Steam library
    assert_eq!(proton_prefix(&game, tmp.path()), None);

    // another app installed to the same folder
    let other = install(tmp.path(), 30, "Game");
    std::fs::remove_dir_all(steamapps.join("compatdata/20")).unwrap();
    std::fs::create_dir_all(steamapps.join("compatdata/30/pfx/drive_c")).unwrap();
    assert_eq!(
        proton_prefix(&game, &other),
        Some(
            other
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("compatdata/30/pfx")
        )
    );
}

#[test]
fn steam_installs_prefer_the_games_own_app() {
    let tmp = tempfile::tempdir().unwrap();
    let path = install(tmp.path(), 30, "Game");
    install(tmp.path(), 20, "Game");
    let library = tmp.path().canonicalize().unwrap();
    assert_eq!(
        steam_install(&game("[20]"), &path),
        Some(SteamInstall {
            app: 20,
            library: library.clone()
        })
    );
    // another app in the same folder will do
    assert_eq!(
        steam_install(&game("[99]"), &path),
        Some(SteamInstall { app: 20, library })
    );
    assert_eq!(steam_install(&game("[20]"), tmp.path()), None);
}

#[test]
fn proton_is_found_from_the_prefix_it_ran() {
    let tmp = tempfile::tempdir().unwrap();
    let compat = tmp.path().join("compatdata/10");
    std::fs::create_dir_all(&compat).unwrap();
    assert_eq!(proton_install(&compat), None);

    let proton = tmp.path().join("common/Proton 9.0 (Beta)");
    std::fs::create_dir_all(&proton).unwrap();
    std::fs::write(
        compat.join("config_info"),
        format!(
            "9.0-203\n{0}/files/share/fonts/\n{0}/files/lib/wine/\n",
            proton.display()
        ),
    )
    .unwrap();
    // only once it's really there
    assert_eq!(proton_install(&compat), None);
    std::fs::write(proton.join("proton"), "").unwrap();
    assert_eq!(proton_install(&compat), Some(proton.clone()));

    // older Protons
    std::fs::write(
        compat.join("config_info"),
        format!("5.0-10\n{}/dist/share/fonts/\n", proton.display()),
    )
    .unwrap();
    assert_eq!(proton_install(&compat), Some(proton));
}

fn store_game(toml: &str) -> GameDef {
    toml::from_str(&format!(
        "schema = 1\nid = \"g\"\nname = \"Fallout 3\"\n[deployer]\nkind = \"rules\"\n[detect]\nfolder = \"Fallout 3\"\nexe = [\"Fallout3.exe\"]\n{toml}"
    ))
    .unwrap()
}

/// A game folder at `dir` under `root`, with `exe` in it if there is one.
fn game_dir(root: &Path, dir: &str, exe: Option<&str>) -> PathBuf {
    let path = root.join(dir);
    std::fs::create_dir_all(&path).unwrap();
    if let Some(exe) = exe {
        std::fs::write(path.join(exe), "").unwrap();
    }
    path.canonicalize().unwrap()
}

fn json(path: &Path, value: serde_json::Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, value.to_string()).unwrap();
}

fn with_stores(stores: Vec<StoreInstall>) -> Locator {
    Locator {
        stores,
        ..Locator::default()
    }
}

#[test]
fn finds_games_heroic_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let heroic = tmp.path().join("heroic");
    let gog = game_dir(tmp.path(), "Games/Fallout 3 GOTY", Some("Fallout3.exe"));
    let epic = game_dir(tmp.path(), "Games/FalloutNV", None);
    let dlc = game_dir(tmp.path(), "Games/DLC", None);
    json(
        &heroic.join("gog_store/installed.json"),
        serde_json::json!({ "installed": [
            { "appName": "1454315831", "install_path": gog, "is_dlc": false },
            { "appName": "1", "install_path": dlc, "is_dlc": true },
            { "appName": "2", "install_path": tmp.path().join("gone") },
        ]}),
    );
    json(
        &heroic.join("legendaryConfig/legendary/installed.json"),
        serde_json::json!({
            "5daeb974a22a435988892319b3a4f476": {
                "app_name": "5daeb974a22a435988892319b3a4f476",
                "title": "Fallout: New Vegas",
                "install_path": epic,
                "is_dlc": false,
            }
        }),
    );
    let installs = stores::heroic(&heroic);
    assert_eq!(installs.len(), 2, "{installs:?}");
    let locator = with_stores(installs);

    // by id
    let nv = store_game("epic = [\"5daeb974a22a435988892319b3a4f476\"]");
    let nv = GameDef {
        name: "Fallout: New Vegas".to_owned(),
        detect: Detect {
            folder: None,
            exe: vec!["FalloutNV.exe".to_owned()],
            ..nv.detect
        },
        ..nv
    };
    let found = locator.find(&nv);
    assert_eq!(paths(found.clone()), std::slice::from_ref(&epic));
    assert_eq!(
        found[0].origin,
        Origin::Epic {
            app: "5daeb974a22a435988892319b3a4f476".to_owned(),
            launcher: Launcher::Heroic
        }
    );
    assert_eq!(found[0].origin.to_string(), "Heroic (Epic)");
    // by its exe and a folder named like it, with no ids listed
    let found = locator.find(&store_game(""));
    assert_eq!(paths(found.clone()), [gog]);
    assert_eq!(found[0].origin.to_string(), "Heroic (GOG)");
}

#[test]
fn an_exe_alone_isnt_enough() {
    let tmp = tempfile::tempdir().unwrap();
    // Enderal ships a SkyrimSE.exe
    let enderal = game_dir(tmp.path(), "Enderal", Some("Fallout3.exe"));
    let locator = with_stores(vec![StoreInstall {
        store: Store::Gog,
        id: "1708684988".to_owned(),
        title: Some("Enderal: Forgotten Stories".to_owned()),
        path: enderal.clone(),
        launcher: Launcher::Galaxy,
    }]);
    assert!(locator.find(&store_game("")).is_empty());
    // its name in the store will do instead of its folder
    let mut renamed = locator.stores.clone();
    renamed[0].title = Some("Fallout 3: Game of the Year Edition".to_owned());
    assert_eq!(
        paths(with_stores(renamed).find(&store_game(""))),
        std::slice::from_ref(&enderal)
    );
    // and its id, without the exe
    std::fs::remove_file(enderal.join("Fallout3.exe")).unwrap();
    assert_eq!(
        paths(locator.find(&store_game("gog = [1708684988]"))),
        [enderal]
    );
}

#[test]
fn reads_epic_launcher_manifests() {
    let tmp = tempfile::tempdir().unwrap();
    let manifests = tmp.path().join("Manifests");
    let game = game_dir(tmp.path(), "Fallout 3", Some("Fallout3.exe"));
    let item = |name: &str, app: &str, main: &str, incomplete: bool| {
        json(
            &manifests.join(name),
            serde_json::json!({
                "AppName": app,
                "MainGameAppName": main,
                "DisplayName": "Fallout 3",
                "InstallLocation": game,
                "bIsIncompleteInstall": incomplete,
            }),
        )
    };
    item(
        "A.item",
        "adeae8bbfc94427db57c7dfecce3f1d4",
        "adeae8bbfc94427db57c7dfecce3f1d4",
        false,
    );
    item("B.item", "dlc", "adeae8bbfc94427db57c7dfecce3f1d4", false);
    item("C.item", "half", "half", true);
    std::fs::write(manifests.join("D.txt"), "not a manifest").unwrap();
    let installs = stores::epic_manifests(&manifests);
    assert_eq!(installs.len(), 1, "{installs:?}");
    let found = with_stores(installs).find(&store_game(""));
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].origin.to_string(), "Epic Games Launcher");
}

#[test]
fn installs_known_twice_are_found_once() {
    let tmp = tempfile::tempdir().unwrap();
    let game = game_dir(tmp.path(), "Fallout 3", Some("Fallout3.exe"));
    let install = |launcher| StoreInstall {
        store: Store::Epic,
        id: "x".to_owned(),
        title: None,
        path: game.clone(),
        launcher,
    };
    let locator = with_stores(vec![
        install(Launcher::Heroic),
        install(Launcher::Legendary),
    ]);
    assert_eq!(locator.find(&store_game("")).len(), 1);
}

#[test]
fn heroic_prefixes_come_from_its_game_settings() {
    let tmp = tempfile::tempdir().unwrap();
    let heroic = tmp.path().join("heroic");
    let game = game_dir(tmp.path(), "Game", None);
    json(
        &heroic.join("gog_store/installed.json"),
        serde_json::json!({ "installed": [{ "appName": "7", "install_path": game }] }),
    );
    let roots = || [heroic.clone()];
    assert_eq!(stores::heroic_prefix_in(roots(), &game), None);

    let prefix = tmp.path().join("Prefixes/Game");
    json(
        &heroic.join("GamesConfig/7.json"),
        serde_json::json!({ "7": { "winePrefix": prefix } }),
    );
    // only once it's been made
    assert_eq!(stores::heroic_prefix_in(roots(), &game), None);
    std::fs::create_dir_all(prefix.join("drive_c")).unwrap();
    assert_eq!(
        stores::heroic_prefix_in(roots(), &game),
        Some(prefix.clone())
    );
    // Proton's prefixes are a level down
    std::fs::create_dir_all(prefix.join("pfx/drive_c")).unwrap();
    assert_eq!(
        stores::heroic_prefix_in(roots(), &game),
        Some(prefix.join("pfx"))
    );
    assert_eq!(stores::heroic_prefix_in(roots(), tmp.path()), None);
}
