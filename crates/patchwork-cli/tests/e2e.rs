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

    fn run(&self, args: &[&str]) -> String {
        let out = Command::new(env!("CARGO_BIN_EXE_patchwork"))
            .args(args)
            .env("PATCHWORK_HOME", self.path("home"))
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
    assert!(list.contains("[x] MoreCompany"), "{list}");
    assert!(list.contains("[ ] Tweaks"), "{list}");
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
