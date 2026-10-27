//! Two machines, each with its own patchwork and iroh node, syncing over
//! localhost.

use std::fs;
use std::path::Path;
use std::time::Duration;

use iroh::endpoint::presets;
use patchwork_core::Patchwork;
use patchwork_core::game::GameDef;
use patchwork_core::order::Position;
use patchwork_core::paths::Paths;
use patchwork_core::source::ModSpec;
use patchwork_deployers::RulesDeployer;
use patchwork_sources::LocalSource;
use patchwork_sync::Node;

const TIMEOUT: Duration = Duration::from_secs(20);

const GAME: &str = r#"
schema = 1
id = "my-game"
name = "My Game"

[deployer]
kind = "rules"
"#;

fn patchwork(root: &Path) -> Patchwork {
    let pw = Patchwork::builder(Paths::rooted(root))
        .source(LocalSource)
        .deployer(RulesDeployer)
        .open()
        .unwrap();
    let game: GameDef = toml::from_str(GAME).unwrap();
    pw.add_game(game).unwrap();
    pw
}

async fn node(pw: &Patchwork) -> Node {
    Node::spawn_with(&pw.paths().sync(), presets::Minimal)
        .await
        .unwrap()
}

fn local(path: &Path) -> ModSpec {
    ModSpec {
        source: "local".to_owned(),
        id: path.to_string_lossy().into_owned(),
        version: None,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_and_its_local_mods_reach_the_other_machine() {
    let tmp = tempfile::tempdir().unwrap();
    let mods = tmp.path().join("mods");
    fs::create_dir_all(mods.join("Folder/sub")).unwrap();
    fs::write(mods.join("Cool.pak"), "cool bytes").unwrap();
    fs::write(mods.join("Folder/a.txt"), "a").unwrap();
    fs::write(mods.join("Folder/sub/b.txt"), "b").unwrap();

    let a = patchwork(&tmp.path().join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    a.add_mod(&profile, local(&mods.join("Cool.pak")), None, true)
        .await
        .unwrap();
    a.add_mod(&profile, local(&mods.join("Folder")), None, false)
        .await
        .unwrap();
    a.lock(&profile, false).await.unwrap();
    let a_node = node(&a).await;
    let ticket = a_node.share(&a, &profile, true).await.unwrap();

    let b = patchwork(&tmp.path().join("b"));
    let b_node = node(&b).await;
    let joined = b_node.join(&b, &ticket, TIMEOUT).await.unwrap();
    assert_eq!(joined.profile, profile);
    assert!(joined.warnings.is_empty(), "{:?}", joined.warnings);

    let loaded = b.load_profile(&profile).unwrap();
    let names: Vec<_> = loaded.mods.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Cool", "Folder"]);
    assert!(!loaded.mods[1].enabled);
    // both local mods' files came along, so b can lock and deploy it

    assert!(b.missing_local_files(&profile).unwrap().is_empty());
    let lock = b.lock(&profile, false).await.unwrap();
    assert_eq!(lock.mods.len(), 2);

    // a change on b goes to a
    b.set_enabled(&profile, "Folder", true).unwrap();
    b.move_mod(&profile, "Folder", &Position::Top).unwrap();
    let b_run = b_node.sync_once(&b, Duration::from_secs(5));
    let a_run = a_node.sync_once(&a, TIMEOUT);
    let (b_out, a_out) = tokio::join!(b_run, a_run);
    b_out.unwrap();
    let a_out = a_out.unwrap();
    assert!(
        a_out[0].applied.as_ref().is_some_and(|x| x.changed),
        "{a_out:?}"
    );
    let loaded = a.load_profile(&profile).unwrap();
    let names: Vec<_> = loaded.mods.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Folder", "Cool"]);
    assert!(loaded.mods[0].enabled);

    a_node.shutdown().await.unwrap();
    b_node.shutdown().await.unwrap();
}

/// Waits for `check` to hold, while `node` keeps `pw` in step.
async fn running_until(node: &Node, pw: &Patchwork, check: impl Fn() -> bool) {
    let wait = async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::select! {
        result = node.run(pw, |_| {}) => panic!("stopped running: {result:?}"),
        _ = tokio::time::timeout(TIMEOUT, wait) => {}
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_running_node_takes_changes_as_they_come() {
    let tmp = tempfile::tempdir().unwrap();
    let mods = tmp.path().join("mods");
    fs::create_dir_all(&mods).unwrap();
    fs::write(mods.join("One.pak"), "one").unwrap();
    fs::write(mods.join("Two.pak"), "two").unwrap();

    let a = patchwork(&tmp.path().join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    a.add_mod(&profile, local(&mods.join("One.pak")), None, true)
        .await
        .unwrap();
    let a_node = node(&a).await;
    let ticket = a_node.share(&a, &profile, true).await.unwrap();
    let b = patchwork(&tmp.path().join("b"));
    let b_node = node(&b).await;
    b_node.join(&b, &ticket, TIMEOUT).await.unwrap();

    // b adds a mod and syncs once while a runs; a gets the mod, locked by
    // b, and its file
    b.add_mod(&profile, local(&mods.join("Two.pak")), None, true)
        .await
        .unwrap();
    let b_sync = async {
        b_node.sync_once(&b, Duration::from_secs(5)).await.unwrap();
    };
    let a_gets_it = running_until(&a_node, &a, || {
        a.missing_local_files(&profile).is_ok_and(|m| m.is_empty())
            && a.load_lock(&profile)
                .unwrap()
                .is_some_and(|l| l.mods.len() == 2)
    });
    tokio::join!(b_sync, a_gets_it);
    let names: Vec<_> = a
        .load_profile(&profile)
        .unwrap()
        .mods
        .into_iter()
        .map(|m| m.name)
        .collect();
    assert_eq!(names, ["One", "Two"]);

    a_node.shutdown().await.unwrap();
    b_node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_followed_profile_stays_as_its_owner_has_it() {
    let tmp = tempfile::tempdir().unwrap();
    let a = patchwork(&tmp.path().join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    a.set_hidden_tags(&profile, &["library".to_owned()])
        .unwrap();
    let a_node = node(&a).await;
    let ticket = a_node.share(&a, &profile, false).await.unwrap();

    let b = patchwork(&tmp.path().join("b"));
    let b_node = node(&b).await;
    b_node.join(&b, &ticket, TIMEOUT).await.unwrap();
    assert!(!b_node.synced_profile(&profile).unwrap().unwrap().writable);
    // and it can't be passed on as something others can change
    let err = b_node.share(&b, &profile, true).await.unwrap_err();
    assert!(err.to_string().contains("only follows"), "{err}");

    // b's own change doesn't go anywhere, and is undone by the next sync
    b.set_hidden_tags(&profile, &[]).unwrap();
    let (a_out, b_out) = tokio::join!(
        a_node.sync_once(&a, Duration::from_secs(5)),
        b_node.sync_once(&b, Duration::from_secs(5)),
    );
    a_out.unwrap();
    b_out.unwrap();
    assert_eq!(a.load_profile(&profile).unwrap().hidden_tags, ["library"]);
    assert_eq!(b.load_profile(&profile).unwrap().hidden_tags, ["library"]);

    a_node.shutdown().await.unwrap();
    b_node.shutdown().await.unwrap();
}
