use super::*;

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
