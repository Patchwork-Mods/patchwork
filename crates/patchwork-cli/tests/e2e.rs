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
    env.run(&["add", pack.to_str().unwrap(), "--name", "BepInExPack"]);
    env.run(&["add", &format!("local:{}", plugin.display())]);
    assert!(env.run(&["list"]).contains("[x] MoreCompany"));

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
    assert!(
        env.path("home/data/profiles/lethal-company/main/profile.lock")
            .is_file()
    );

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
