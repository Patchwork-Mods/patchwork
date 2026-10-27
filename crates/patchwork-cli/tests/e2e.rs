//! Drives the real binary against a fake game directory.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Env {
    tmp: tempfile::TempDir,
}

impl Env {
    fn new() -> Self {
        Self {
            tmp: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.tmp.path().join(rel)
    }

    /// `$PATH` with the env's `bin` first, so programs patchwork runs (like
    /// `steam`) can be stood in for.
    fn search_path(&self) -> std::ffi::OsString {
        let path = std::env::var_os("PATH").unwrap_or_default();
        std::env::join_paths(std::iter::once(self.path("bin")).chain(std::env::split_paths(&path)))
            .unwrap()
    }

    fn run(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_patchwork"))
            .args(args)
            .env("PATCHWORK_HOME", self.path("home"))
            .env("PATCHWORK_STEAM_PATH", self.path("Steam"))
            .env("PATH", self.search_path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            out.status.success(),
            "patchwork {args:?} failed:\n{stdout}\n{stderr}"
        );
        stdout
    }

    /// The directory of the only profile, which is named by its id.
    fn profile_dir(&self) -> PathBuf {
        let dirs: Vec<_> = fs::read_dir(self.path("home/data/profiles"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(dirs.len(), 1, "{dirs:?}");
        dirs.into_iter().next().unwrap()
    }

    fn fails(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_patchwork"))
            .args(args)
            .env("PATCHWORK_HOME", self.path("home"))
            .env("PATCHWORK_STEAM_PATH", self.path("Steam"))
            .env("PATH", self.search_path())
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "patchwork {args:?} should have failed"
        );
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
}

fn write_zip(path: &Path, entries: &[(&str, &str)]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    for (name, contents) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(contents.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

fn read(path: PathBuf) -> String {
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
#[ignore = "needs network"]
fn installs_from_thunderstore() {
    let env = Env::new();
    let game = env.path("Lethal Company");
    fs::create_dir_all(&game).unwrap();

    env.run(&["game", "set-path", "lethal-company", game.to_str().unwrap()]);
    env.run(&["profile", "new", "lethal-company/main"]);
    env.run(&[
        "add",
        "thunderstore:notnotnotswipez-MoreCompany",
        "--version",
        "1.11.0",
    ]);
    env.run(&["deploy"]);

    // BepInExPack comes in as a dependency
    assert!(game.join("winhttp.dll").is_file());
    assert!(game.join("BepInEx/core/BepInEx.dll").is_file());
    // MoreCompany ships its own BepInEx/plugins/, which gets a folder of its own
    assert!(
        game.join("BepInEx/plugins/notnotnotswipez-MoreCompany/MoreCompany.dll")
            .is_file()
    );
    let lock = read(env.profile_dir().join("profile.lock"));
    assert!(lock.contains("1.11.0"), "{lock}");
    assert!(env.run(&["deploy"]).contains("nothing to do"));
}

#[test]
fn thunderstore_style_mods_round_trip() {
    let env = Env::new();
    let game = env.path("Lethal Company");
    fs::create_dir_all(&game).unwrap();
    fs::write(game.join("doorstop_config.ini"), "vanilla").unwrap();

    let pack = env.path("BepInEx-BepInExPack-5.4.2100.zip");
    write_zip(
        &pack,
        &[
            ("manifest.json", "{}"),
            ("BepInExPack/winhttp.dll", "loader"),
            ("BepInExPack/doorstop_config.ini", "modded"),
            ("BepInExPack/BepInEx/core/BepInEx.dll", "core"),
        ],
    );
    let plugin = env.path("MoreCompany.zip");
    write_zip(
        &plugin,
        &[("icon.png", "png"), ("plugins/MoreCompany.dll", "more")],
    );

    env.run(&["game", "set-path", "lethal-company", game.to_str().unwrap()]);
    env.run(&["profile", "new", "lethal-company/main"]);
    env.run(&[
        "add",
        pack.to_str().unwrap(),
        "--name",
        "BepInEx-BepInExPack",
    ]);
    env.run(&["add", &format!("local:{}", plugin.display())]);
    assert!(env.run(&["list"]).contains("[x] MoreCompany"));
    // matches keep their numbers in the load order
    let found = env.run(&["list", "more", "compny"]);
    assert!(found.contains("2. [x] MoreCompany"), "{found}");
    assert!(!found.contains("BepInExPack"), "{found}");

    let dry = env.run(&["deploy", "--dry-run"]);
    assert!(
        dry.contains("doorstop_config.ini (backing up original)"),
        "{dry}"
    );
    assert!(!game.join("winhttp.dll").exists());

    env.run(&["deploy"]);
    assert_eq!(read(game.join("winhttp.dll")), "loader");
    assert_eq!(read(game.join("doorstop_config.ini")), "modded");
    assert_eq!(read(game.join("BepInEx/core/BepInEx.dll")), "core");
    assert_eq!(
        read(game.join("BepInEx/plugins/MoreCompany/MoreCompany.dll")),
        "more"
    );
    assert!(!game.join("manifest.json").exists());
    assert!(env.profile_dir().join("profile.lock").is_file());

    assert!(env.run(&["deploy"]).contains("nothing to do"));

    env.run(&["disable", "MoreCompany"]);
    env.run(&["deploy"]);
    assert!(!game.join("BepInEx/plugins").exists());
    assert!(game.join("winhttp.dll").exists());

    // The store keeps working without the original files.
    fs::remove_file(&plugin).unwrap();
    env.run(&["enable", "MoreCompany"]);
    env.run(&["deploy"]);
    assert!(
        game.join("BepInEx/plugins/MoreCompany/MoreCompany.dll")
            .exists()
    );

    env.run(&["undeploy"]);
    let left: Vec<_> = fs::read_dir(&game)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left, ["doorstop_config.ini"]);
    assert_eq!(read(game.join("doorstop_config.ini")), "vanilla");
}

#[test]
fn games_can_be_defined_at_runtime() {
    let env = Env::new();
    let games = env.path("home/config/games");
    fs::create_dir_all(&games).unwrap();
    fs::write(
        games.join("my-ue-game.toml"),
        r#"
schema = 1
id = "my-ue-game"
name = "My UE Game"

[deployer]
kind = "rules"
unmatched = "error"

[[deployer.rules]]
files = "*.pak"
to = "Content/Paks/~mods/{file}"
"#,
    )
    .unwrap();
    let game = env.path("game");
    fs::create_dir_all(&game).unwrap();
    let pak = env.path("cool_P.pak");
    fs::write(&pak, "pak").unwrap();

    assert!(
        env.run(&["game", "list"])
            .contains("my-ue-game  My UE Game [user]")
    );
    env.run(&["game", "set-path", "my-ue-game", game.to_str().unwrap()]);
    env.run(&["profile", "new", "my-ue-game/default"]);
    env.run(&["add", pak.to_str().unwrap()]);
    env.run(&["deploy"]);
    assert_eq!(read(game.join("Content/Paks/~mods/cool_P.pak")), "pak");

    // A file no rule covers is an error for this game.
    let txt = env.path("notes.txt");
    fs::write(&txt, "?").unwrap();
    env.run(&["add", txt.to_str().unwrap()]);
    assert!(env.fails(&["deploy"]).contains("no rule for notes.txt"));
}

#[test]
fn games_are_found_in_steam() {
    let env = Env::new();
    let steamapps = env.path("Steam/steamapps");
    let game = steamapps.join("common/Lethal Company");
    fs::create_dir_all(&game).unwrap();
    fs::write(
        steamapps.join("appmanifest_1966720.acf"),
        "\"AppState\" { \"appid\" \"1966720\" \"installdir\" \"Lethal Company\" }",
    )
    .unwrap();
    let game = game.canonicalize().unwrap();

    env.run(&["profile", "new", "lethal-company/main"]);
    let err = env.fails(&["deploy"]);
    assert!(err.contains("game detect lethal-company --set"), "{err}");

    let out = env.run(&["game", "detect"]);
    assert!(
        out.contains(&format!("lethal-company  {}", game.display())),
        "{out}"
    );
    assert!(!out.contains("(set up)"), "{out}");
    env.run(&["game", "detect", "lethal-company", "--set"]);
    assert!(env.run(&["game", "detect"]).contains("(set up)"));
    assert!(
        env.run(&["game", "list"])
            .contains(&game.display().to_string())
    );
    assert!(
        env.fails(&["game", "detect", "risk-of-rain-2"])
            .contains("couldn't find where Risk of Rain 2 is installed")
    );
}

#[test]
fn search_needs_a_searchable_source() {
    let env = Env::new();
    env.run(&["profile", "new", "lethal-company/main"]);
    // checked before anything goes over the network
    let err = env.fails(&["search", "morecompany", "--sort", "relevance"]);
    assert!(
        err.contains("thunderstore can't sort by relevance"),
        "{err}"
    );
    let err = env.fails(&["search", "x", "--source", "local"]);
    assert!(err.contains("can't be searched"), "{err}");

    let games = env.path("home/config/games");
    fs::create_dir_all(&games).unwrap();
    fs::write(
        games.join("plain.toml"),
        "schema = 1\nid = \"plain\"\nname = \"Plain\"\n\n[deployer]\nkind = \"rules\"\n",
    )
    .unwrap();
    let err = env.fails(&["search", "--game", "plain"]);
    assert!(
        err.contains("none of plain's sources can be searched"),
        "{err}"
    );
}

#[test]
#[ignore = "needs network"]
fn searches_thunderstore() {
    let env = Env::new();
    env.run(&["profile", "new", "lethal-company/main"]);
    assert!(
        env.run(&["search"])
            .contains("(thunderstore:BepInEx-BepInExPack)")
    );
    let found = env.run(&["search", "morecompany", "-c", "Mods", "--sort", "rating"]);
    assert!(
        found.contains("(thunderstore:notnotnotswipez-MoreCompany)"),
        "{found}"
    );
    assert!(
        env.run(&["search", "lethallib"])
            .contains("(thunderstore:Evaisa-LethalLib)")
    );
    let without = env.run(&["search", "lethallib", "-x", "Libraries"]);
    assert!(
        !without.contains("(thunderstore:Evaisa-LethalLib)"),
        "{without}"
    );
    assert!(env.run(&["search", "--list-categories"]).contains("moons"));
}

#[test]
fn profiles_are_found_by_name() {
    let env = Env::new();
    env.run(&["profile", "new", "lethal-company/main"]);
    env.run(&["profile", "new", "lethal-company/other"]);
    assert!(
        env.fails(&["profile", "new", "lethal-company/main"])
            .contains("already exists")
    );
    assert!(
        env.fails(&["profile", "new", "lethal-company/a/b"])
            .contains("isn't a valid")
    );

    let list = env.run(&["profile", "list"]);
    assert!(list.contains("  lethal-company/main"), "{list}");
    assert!(list.contains("* lethal-company/other"), "{list}");

    env.run(&["profile", "use", "lethal-company/main"]);
    assert!(
        env.run(&["status"])
            .contains("profile: lethal-company/main")
    );
    assert!(
        env.run(&["-p", "lethal-company/other", "list"])
            .contains("lethal-company/other has no mods")
    );
    assert!(
        env.fails(&["profile", "use", "lethal-company/nope"])
            .contains("no profile named")
    );

    env.run(&["profile", "copy", "lethal-company/other", "copied"]);
    env.run(&["profile", "rename", "lethal-company/copied", "renamed"]);
    let list = env.run(&["profile", "list"]);
    assert!(list.contains("  lethal-company/renamed"), "{list}");
    assert!(!list.contains("lethal-company/copied"), "{list}");
    assert!(
        env.fails(&["profile", "rename", "lethal-company/renamed", "main"])
            .contains("already exists")
    );

    env.run(&["profile", "remove", "lethal-company/other"]);
    let list = env.run(&["profile", "list"]);
    assert!(!list.contains("lethal-company/other"), "{list}");
    assert!(
        env.fails(&["profile", "remove", "lethal-company/other"])
            .contains("no profile named")
    );
}

#[test]
fn old_profiles_are_migrated() {
    let env = Env::new();
    let old = env.path("home/data/profiles/lethal-company/main");
    fs::create_dir_all(&old).unwrap();
    fs::write(
        old.join("profile.toml"),
        r#"
schema = 1
game = "lethal-company"

[[mod]]
name = "MoreCompany"
source = "thunderstore"
id = "notnotnotswipez-MoreCompany"
"#,
    )
    .unwrap();
    fs::write(old.join("profile.lock"), "schema = 1").unwrap();
    fs::write(
        env.path("home/data/state.toml"),
        "current_profile = \"lethal-company/main\"\n",
    )
    .unwrap();

    assert!(env.run(&["list"]).contains("[x] MoreCompany"));
    assert!(!env.path("home/data/profiles/lethal-company").exists());
    let dir = env.profile_dir();
    assert!(dir.join("profile.lock").is_file());
    let profile = read(dir.join("profile.toml"));
    assert!(profile.contains("schema = 2"), "{profile}");
    assert!(profile.contains("name = \"main\""), "{profile}");
}

#[test]
fn profiles_move_between_machines_as_zips() {
    let alice = Env::new();
    let plugin = alice.path("MoreCompany.zip");
    write_zip(&plugin, &[("plugins/MoreCompany.dll", "more")]);
    let folder = alice.path("Tweaks");
    fs::create_dir_all(folder.join("plugins")).unwrap();
    fs::write(folder.join("plugins/Tweaks.dll"), "tweaks").unwrap();

    alice.run(&["profile", "new", "lethal-company/main"]);
    alice.run(&["add", plugin.to_str().unwrap()]);
    alice.run(&["add", folder.to_str().unwrap(), "--disabled"]);
    let export = alice.path("main.zip");
    alice.run(&["profile", "export", "-o", export.to_str().unwrap()]);

    // Bob has none of Alice's files.
    let bob = Env::new();
    let game = bob.path("Lethal Company");
    fs::create_dir_all(&game).unwrap();
    bob.run(&["game", "set-path", "lethal-company", game.to_str().unwrap()]);
    let out = bob.run(&["profile", "import", export.to_str().unwrap()]);
    assert!(
        out.contains("imported lethal-company/main (2 mods, from patchwork)"),
        "{out}"
    );
    let list = bob.run(&["list"]);
    assert!(
        list.contains("[x] MoreCompany  (local:MoreCompany.zip)"),
        "{list}"
    );
    assert!(list.contains("[ ] Tweaks  (local:Tweaks)"), "{list}");
    bob.run(&["enable", "Tweaks"]);
    bob.run(&["deploy"]);
    assert_eq!(
        read(game.join("BepInEx/plugins/MoreCompany/MoreCompany.dll")),
        "more"
    );
    assert_eq!(
        read(game.join("BepInEx/plugins/Tweaks/Tweaks.dll")),
        "tweaks"
    );

    // It's recognized as the same profile the second time around.
    let err = bob.fails(&["profile", "import", export.to_str().unwrap()]);
    assert!(err.contains("already exists"), "{err}");
    bob.run(&["remove", "Tweaks"]);
    let out = bob.run(&["profile", "import", export.to_str().unwrap(), "--replace"]);
    assert!(out.contains("updated lethal-company/main"), "{out}");
    assert!(bob.run(&["list"]).contains("Tweaks"));
    bob.run(&["deploy"]);
    let out = bob.run(&[
        "profile",
        "import",
        export.to_str().unwrap(),
        "--name",
        "copy",
    ]);
    assert!(out.contains("imported lethal-company/copy"), "{out}");
    assert!(
        bob.run(&["profile", "list"])
            .contains("* lethal-company/copy")
    );
}

#[test]
fn hidden_tags_travel_with_zips() {
    let alice = Env::new();
    alice.run(&["profile", "new", "lethal-company/main"]);
    let file = alice.profile_dir().join("profile.toml");
    let text = read(file.clone()).replace(
        "name = \"main\"\n",
        "name = \"main\"\nhidden_tags = [\"Libraries\"]\n",
    );
    fs::write(&file, text).unwrap();
    let export = alice.path("main.zip");
    alice.run(&["profile", "export", "-o", export.to_str().unwrap()]);

    let bob = Env::new();
    bob.run(&["profile", "import", export.to_str().unwrap()]);
    let imported = read(bob.profile_dir().join("profile.toml"));
    assert!(
        imported.contains("hidden_tags = [\"Libraries\"]"),
        "{imported}"
    );
}

#[test]
fn profiles_can_be_shared_as_links() {
    let alice = Env::new();
    alice.run(&["profile", "new", "lethal-company/main"]);
    alice.run(&["add", "url:https://example.com/mods/Cool.Mod.zip"]);
    alice.run(&["add", "url:https://example.com/Other.dll", "--disabled"]);
    let local = alice.path("Local.zip");
    write_zip(&local, &[("plugins/a.dll", "a")]);
    alice.run(&["add", local.to_str().unwrap()]);
    let links = alice.run(&["profile", "export", "--format", "urls"]);
    assert_eq!(links.trim(), "https://example.com/mods/Cool.Mod.zip");

    let bob = Env::new();
    let out = bob.run(&[
        "profile",
        "import",
        "https://example.com/a.zip, https://example.com/b.pak",
        "--name",
        "linked",
        "--game",
        "lethal-company",
    ]);
    assert!(out.contains("(2 mods, from urls)"), "{out}");
    let list = bob.run(&["list"]);
    assert!(
        list.contains("[x] a  (url:https://example.com/a.zip)"),
        "{list}"
    );
    assert!(
        list.contains("[x] b  (url:https://example.com/b.pak)"),
        "{list}"
    );

    let err = bob.fails(&["profile", "import", "not a thing"]);
    assert!(err.contains("couldn't tell what format"), "{err}");
}

#[test]
fn r2modman_profiles_are_imported() {
    let env = Env::new();
    let r2z = env.path("Friends.r2z");
    write_zip(
        &r2z,
        &[
            (
                "export.r2x",
                "profileName: Friends Pack\nmods:\n  - name: BepInEx-BepInExPack\n    version:\n      major: 5\n      minor: 4\n      patch: 2100\n    enabled: true\n  - name: notnotnotswipez-MoreCompany\n    version:\n      major: 1\n      minor: 11\n      patch: 0\n    enabled: false\n",
            ),
            ("BepInEx/config/MoreCompany.cfg", "x"),
        ],
    );
    let out = env.run(&[
        "profile",
        "import",
        r2z.to_str().unwrap(),
        "--game",
        "lethal-company",
    ]);
    assert!(
        out.contains("imported lethal-company/Friends-Pack (2 mods, from r2modman)"),
        "{out}"
    );
    let list = env.run(&["list"]);
    assert!(
        list.contains("[x] BepInEx-BepInExPack  (thunderstore:BepInEx-BepInExPack @ 5.4.2100)"),
        "{list}"
    );
    assert!(list.contains("[ ] notnotnotswipez-MoreCompany"), "{list}");

    assert!(
        env.run(&["profile", "formats"])
            .contains("thunderstore-code")
    );
}

#[test]
fn mods_can_be_moved_and_sorted() {
    let env = Env::new();
    env.run(&["profile", "new", "lethal-company/main"]);
    for name in ["beta", "Alpha", "gamma"] {
        let zip = env.path(&format!("{name}.zip"));
        write_zip(&zip, &[("plugins/x.dll", name)]);
        env.run(&["add", zip.to_str().unwrap(), "--name", name]);
    }
    let order = |env: &Env| -> Vec<String> {
        env.run(&["list"])
            .lines()
            .map(|l| l.split_whitespace().nth(2).unwrap().to_owned())
            .collect()
    };

    assert!(env.run(&["move", "gamma", "--top"]).contains("to 1 of 3"));
    assert_eq!(order(&env), ["gamma", "beta", "Alpha"]);
    env.run(&["move", "gamma", "--after", "beta"]);
    assert_eq!(order(&env), ["beta", "gamma", "Alpha"]);
    env.run(&["move", "Alpha", "--to", "2"]);
    assert_eq!(order(&env), ["beta", "Alpha", "gamma"]);
    assert!(env.fails(&["move", "nope", "--top"]).contains("`nope`"));
    env.fails(&["move", "beta"]);

    let preview = env.run(&["sort", "--by", "name:desc", "--dry-run"]);
    assert!(preview.starts_with("1. [x] gamma"), "{preview}");
    assert_eq!(order(&env), ["beta", "Alpha", "gamma"]);
    env.run(&["sort", "--by", "name"]);
    assert_eq!(order(&env), ["Alpha", "beta", "gamma"]);

    // local mods have no tags or dependencies, so these keep the order
    env.run(&["move", "gamma", "--top"]);
    env.run(&["sort", "--by", "tags", "--by", "dependents:desc"]);
    assert_eq!(order(&env), ["gamma", "Alpha", "beta"]);
    assert!(
        env.fails(&["sort", "--by", "size"])
            .contains("unknown sort")
    );
}

/// A Skyrim plugin's bytes: just the header record, flagged as a master
/// if `master`, needing `masters`.
fn plugin(master: bool, masters: &[&str]) -> Vec<u8> {
    let mut data = Vec::new();
    for m in masters {
        data.extend_from_slice(b"MAST");
        data.extend_from_slice(&(m.len() as u16 + 1).to_le_bytes());
        data.extend_from_slice(m.as_bytes());
        data.push(0);
    }
    let mut bytes = b"TES4".to_vec();
    bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&(master as u32).to_le_bytes());
    bytes.extend_from_slice(&[0; 12]);
    bytes.extend_from_slice(&data);
    bytes
}

#[test]
fn skyrim_plugins_load_in_the_order_set() {
    let env = Env::new();
    let game = env.path("Skyrim Special Edition");
    fs::create_dir_all(game.join("Data")).unwrap();
    fs::write(game.join("Data/Skyrim.esm"), plugin(true, &[])).unwrap();
    let appdata = env.path("AppData/Local/Skyrim Special Edition");
    fs::create_dir_all(&appdata).unwrap();
    fs::write(appdata.join("plugins.txt"), "*Mine.esp\r\n").unwrap();
    env.run(&["game", "set-path", "skyrim-se", game.to_str().unwrap()]);
    env.run(&[
        "game",
        "set",
        "skyrim-se",
        "root.appdata",
        appdata.to_str().unwrap(),
    ]);
    env.run(&["profile", "new", "skyrim-se/main"]);

    for (name, files) in [
        ("base", vec![("Base.esm", plugin(true, &["Skyrim.esm"]))]),
        (
            "patches",
            vec![
                (
                    "Data/Patch.esp",
                    plugin(false, &["Skyrim.esm", "Base.esm", "Required.esp"]),
                ),
                ("Data/Required.esp", plugin(false, &[])),
            ],
        ),
    ] {
        let dir = env.path(&format!("mods/{name}"));
        for (rel, bytes) in files {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        env.run(&["add", dir.to_str().unwrap(), "--name", name]);
    }
    env.run(&["lock"]);
    let order = |env: &Env| -> Vec<String> {
        env.run(&["plugins", "list"])
            .lines()
            .filter(|l| !l.starts_with("always"))
            .map(|l| l.split("  (").next().unwrap().trim().to_owned())
            .collect()
    };
    assert_eq!(
        order(&env),
        [
            "1. [x] Base.esm (master)",
            "2. [x] Patch.esp",
            "3. [x] Required.esp"
        ]
    );

    // Patch.esp needs Required.esp first
    let out = Command::new(env!("CARGO_BIN_EXE_patchwork"))
        .args(["plugins", "list"])
        .env("PATCHWORK_HOME", env.path("home"))
        .output()
        .unwrap();
    let warnings = String::from_utf8_lossy(&out.stderr);
    assert!(
        warnings.contains("needs Required.esp, which loads after it"),
        "{warnings}"
    );

    assert!(
        env.run(&["plugins", "move", "Required.esp", "--before", "Patch.esp"])
            .contains("to 2 of 3")
    );
    // masters stay on top
    assert!(
        env.run(&["plugins", "move", "Required.esp", "--top"])
            .contains("to 2 of 3")
    );
    env.run(&["plugins", "disable", "patch.esp"]);
    assert_eq!(
        order(&env),
        [
            "1. [x] Base.esm (master)",
            "2. [x] Required.esp",
            "3. [ ] Patch.esp"
        ]
    );
    assert!(read(env.profile_dir().join("profile.toml")).contains("[[plugin]]"));

    env.run(&["deploy"]);
    assert!(game.join("Data/Patch.esp").is_file());
    let lines = |text: String| -> Vec<String> {
        text.lines()
            .filter(|l| !l.starts_with('#'))
            .map(str::to_owned)
            .collect()
    };
    assert_eq!(
        lines(read(appdata.join("plugins.txt"))),
        ["*Base.esm", "*Required.esp", "Patch.esp"]
    );
    assert!(env.run(&["deploy"]).contains("nothing to do"));

    // Changing the order makes the deploy stale, and the game rewriting the
    // file is undone by deploying again.
    env.run(&["plugins", "enable", "Patch.esp"]);
    fs::write(appdata.join("plugins.txt"), "*Base.esm\r\n").unwrap();
    env.run(&["deploy"]);
    assert_eq!(
        lines(read(appdata.join("plugins.txt"))),
        ["*Base.esm", "*Required.esp", "*Patch.esp"]
    );

    // the game's own load order comes back, even after the game's changed ours
    fs::write(appdata.join("plugins.txt"), "*Base.esm\r\n").unwrap();
    env.run(&["undeploy"]);
    assert_eq!(read(appdata.join("plugins.txt")), "*Mine.esp\r\n");
    assert!(!game.join("Data/Patch.esp").exists());
}

#[test]
fn profiles_roll_back_to_earlier_deploys() {
    let env = Env::new();
    let games = env.path("home/config/games");
    fs::create_dir_all(&games).unwrap();
    fs::write(
        games.join("my-game.toml"),
        r#"
schema = 1
id = "my-game"
name = "My Game"

[deployer]
kind = "rules"

[[deployer.rules]]
files = "*.txt"
to = "mods/{file}"
"#,
    )
    .unwrap();
    let game = env.path("game");
    fs::create_dir_all(&game).unwrap();
    env.run(&["game", "set-path", "my-game", game.to_str().unwrap()]);
    env.run(&["profile", "new", "my-game/default"]);
    assert!(env.run(&["history"]).contains("no deploys"));
    assert!(env.fails(&["rollback"]).contains("hasn't been deployed"));

    let a = env.path("a.txt");
    fs::write(&a, "old").unwrap();
    env.run(&["add", a.to_str().unwrap()]);
    env.run(&["deploy"]);

    fs::write(&a, "new").unwrap();
    let b = env.path("b.txt");
    fs::write(&b, "b").unwrap();
    env.run(&["add", b.to_str().unwrap()]);
    env.run(&["lock", "--update"]);
    env.run(&["deploy"]);
    assert_eq!(read(game.join("mods/a.txt")), "new");

    let out = env.run(&["history"]);
    assert!(out.contains("#1  "), "{out}");
    assert!(out.contains("1 mods enabled\n#2  "), "{out}");
    assert!(out.contains("2 mods enabled  (current)\n"), "{out}");
    assert!(out.contains("    + b "), "{out}");
    assert!(out.contains("    ~ a "), "{out}");

    let out = env.run(&["rollback", "1", "--dry-run"]);
    assert!(out.contains("- b "), "{out}");
    assert!(!out.contains("rolled"), "{out}");
    env.run(&["rollback", "1"]);
    // the old copy comes from the store, even though the file has changed
    env.run(&["deploy"]);
    assert_eq!(read(game.join("mods/a.txt")), "old");
    assert!(!game.join("mods/b.txt").exists());
    assert!(env.run(&["history"]).contains("#3  "));
    assert!(env.run(&["rollback"]).contains("already like deploy #3"));
    assert!(env.fails(&["rollback", "9"]).contains("no deploy 9"));
}

#[test]
fn files_a_game_update_replaced_are_put_back() {
    let env = Env::new();
    let games = env.path("home/config/games");
    fs::create_dir_all(&games).unwrap();
    fs::write(
        games.join("my-game.toml"),
        r#"
schema = 1
id = "my-game"
name = "My Game"

[deployer]
kind = "rules"

[[deployer.rules]]
files = "*.txt"
to = "mods/{file}"
"#,
    )
    .unwrap();
    let game = env.path("game");
    fs::create_dir_all(&game).unwrap();
    env.run(&["game", "set-path", "my-game", game.to_str().unwrap()]);
    env.run(&["profile", "new", "my-game/default"]);
    for name in ["a", "b"] {
        let file = env.path(&format!("{name}.txt"));
        fs::write(&file, name).unwrap();
        env.run(&["add", file.to_str().unwrap()]);
    }
    env.run(&["deploy"]);
    assert!(!env.run(&["status"]).contains("warning"));

    fs::remove_file(game.join("mods/a.txt")).unwrap();
    let out = env.run(&["status"]);
    assert!(
        out.contains("1 file patchwork deployed is missing"),
        "{out}"
    );
    env.run(&["deploy"]);
    assert_eq!(read(game.join("mods/a.txt")), "a");

    // the same size, so only reading it shows it's changed
    fs::write(game.join("mods/b.txt"), "B").unwrap();
    assert!(!env.run(&["status"]).contains("warning"));
    assert!(env.run(&["deploy"]).contains("nothing to do"));
    env.run(&["deploy", "--verify"]);
    assert_eq!(read(game.join("mods/b.txt")), "b");
}

#[test]
fn conflicts_show_which_mod_wins() {
    let env = Env::new();
    env.run(&["profile", "new", "skyrim-se/main"]);
    for (name, files) in [
        (
            "textures",
            [("Data/Textures/Sky.dds", "a"), ("Data/readme.txt", "same")],
        ),
        (
            "better",
            [("data/textures/sky.DDS", "b"), ("Data/readme.txt", "same")],
        ),
    ] {
        let dir = env.path(&format!("mods/{name}"));
        for (rel, contents) in files {
            let path = dir.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        env.run(&["add", dir.to_str().unwrap(), "--name", name]);
    }
    // nothing's known about mods that haven't been locked
    assert!(env.run(&["conflicts"]).contains("no two mods"));
    env.run(&["lock"]);

    let out = env.run(&["conflicts"]);
    assert!(
        out.contains("textures  (other mods' copies of all of these are deployed)\n  overridden by better (1 file)"),
        "{out}"
    );
    assert!(
        out.contains("better\n  overrides textures (1 file)"),
        "{out}"
    );
    assert!(out.contains("1 more file is the same"), "{out}");

    // spelled the way the first mod in the load order spells it
    env.run(&["move", "better", "--top"]);
    let out = env.run(&["conflicts", "textures"]);
    assert!(
        out.contains("textures\n  overrides better (1 file)\n    Data/textures/sky.DDS"),
        "{out}"
    );
    assert!(env.fails(&["conflicts", "nope"]).contains("no mod named"));
}

#[test]
#[cfg(target_os = "linux")]
fn games_launch_through_steam_or_proton() {
    use std::os::unix::fs::PermissionsExt;

    let env = Env::new();
    let steamapps = env.path("Steam/steamapps");
    let game = steamapps.join("common/Lethal Company");
    fs::create_dir_all(&game).unwrap();
    fs::write(game.join("Lethal Company.exe"), "").unwrap();
    fs::write(
        steamapps.join("appmanifest_1966720.acf"),
        "\"AppState\" { \"appid\" \"1966720\" \"installdir\" \"Lethal Company\" }",
    )
    .unwrap();
    // the prefix Proton made the last time Steam ran the game, and the
    // Proton that made it
    let compat = steamapps.join("compatdata/1966720");
    fs::create_dir_all(compat.join("pfx/drive_c")).unwrap();
    fs::write(compat.join("pfx/user.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let proton = steamapps.join("common/Proton 9.0");
    fs::create_dir_all(&proton).unwrap();
    fs::write(proton.join("proton"), "").unwrap();
    fs::write(
        compat.join("config_info"),
        format!("9.0-203\n{}/files/share/fonts/\n", proton.display()),
    )
    .unwrap();
    // Steam, which here writes down how it was run
    let ran = env.path("ran");
    fs::create_dir_all(env.path("bin")).unwrap();
    fs::write(
        env.path("bin/steam"),
        format!(
            "#!/bin/sh\necho \"$@\" > '{0}.tmp'\nmv '{0}.tmp' '{0}'\n",
            ran.display()
        ),
    )
    .unwrap();
    fs::set_permissions(env.path("bin/steam"), fs::Permissions::from_mode(0o755)).unwrap();

    env.run(&["game", "detect", "lethal-company", "--set"]);
    env.run(&["profile", "new", "lethal-company/main"]);
    let list = env.run(&["launch", "--list"]);
    assert_eq!(list, "game  Lethal Company (default)\n");

    // nothing to change, so Steam starts it
    let dry = env.run(&["launch", "--dry-run"]);
    assert!(
        dry.contains("would deploy lethal-company/main first"),
        "{dry}"
    );
    assert!(dry.contains("Steam (app 1966720)"), "{dry}");
    assert!(dry.contains("steam -applaunch 1966720"), "{dry}");

    // BepInEx needs Wine to load its winhttp.dll, which Steam can't be
    // told, so it's put in the prefix's registry for Steam to start it
    let pack = env.path("BepInEx-BepInExPack-5.4.2100.zip");
    write_zip(
        &pack,
        &[
            ("BepInExPack/winhttp.dll", "loader"),
            ("BepInExPack/doorstop_config.ini", "modded"),
        ],
    );
    env.run(&[
        "add",
        pack.to_str().unwrap(),
        "--name",
        "BepInEx-BepInExPack",
    ]);
    let out = env.run(&["launch"]);
    assert!(out.contains("load the game's own winhttp"), "{out}");
    assert_eq!(read(ran.clone()), "-applaunch 1966720\n");
    assert_eq!(read(game.join("winhttp.dll")), "loader");
    let reg = read(compat.join("pfx/user.reg"));
    assert!(reg.ends_with("\n\"winhttp\"=\"native,builtin\"\n"), "{reg}");

    let dry = env.run(&["launch", "--dry-run"]);
    assert!(!dry.contains("would deploy"), "{dry}");
    assert!(dry.contains("loads the game's own winhttp"), "{dry}");

    // the second time it's already there
    let out = env.run(&["launch"]);
    assert!(!out.contains("winhttp"), "{out}");
    assert_eq!(read(compat.join("pfx/user.reg")), reg);

    assert!(
        env.fails(&["launch", "skse"])
            .contains("can't be launched as `skse`; it can be game")
    );
}
