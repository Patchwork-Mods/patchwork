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
use patchwork_core::profile::ProfileRef;
use patchwork_core::source::ModSpec;
use patchwork_deployers::RulesDeployer;
use patchwork_sources::LocalSource;
use patchwork_sync::{Addresses, Joined, Node, Outcome};

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
    let ticket = a_node
        .share(&a, &profile, true, Addresses::Included)
        .await
        .unwrap();

    let b = patchwork(&tmp.path().join("b"));
    let b_node = node(&b).await;
    let joined = joined(b_node.join(&b, &ticket, TIMEOUT).await.unwrap());
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

fn joined(joined: Joined) -> Outcome {
    match joined {
        Joined::Profile(outcome) => outcome,
        Joined::Devices { .. } => panic!("joined devices, not a profile"),
    }
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
    let ticket = a_node
        .share(&a, &profile, true, Addresses::Included)
        .await
        .unwrap();
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
async fn a_follower_keeps_its_own_changes_to_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let a = patchwork(&tmp.path().join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    a.set_hidden_tags(&profile, &["library".to_owned()])
        .unwrap();
    let a_node = node(&a).await;
    let ticket = a_node
        .share(&a, &profile, false, Addresses::Included)
        .await
        .unwrap();

    let b = patchwork(&tmp.path().join("b"));
    let b_node = node(&b).await;
    b_node.join(&b, &ticket, TIMEOUT).await.unwrap();
    assert!(!b_node.synced_profile(&profile).unwrap().unwrap().writable);
    // and it can't be passed on as something others can change
    let err = b_node
        .share(&b, &profile, true, Addresses::Included)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("only follows"), "{err}");

    // b's own change stays on b, and doesn't go anywhere
    b.set_hidden_tags(&profile, &[]).unwrap();
    let (a_out, b_out) = tokio::join!(
        a_node.sync_once(&a, Duration::from_secs(5)),
        b_node.sync_once(&b, Duration::from_secs(5)),
    );
    a_out.unwrap();
    b_out.unwrap();
    assert_eq!(a.load_profile(&profile).unwrap().hidden_tags, ["library"]);
    assert!(b.load_profile(&profile).unwrap().hidden_tags.is_empty());

    a_node.shutdown().await.unwrap();
    b_node.shutdown().await.unwrap();
}

/// Runs `steps` while both nodes keep their patchworks in step.
async fn both_running(
    (a_node, a): (&Node, &Patchwork),
    (b_node, b): (&Node, &Patchwork),
    steps: impl Future<Output = ()>,
) {
    tokio::select! {
        result = a_node.run(a, |_| {}) => panic!("stopped running: {result:?}"),
        result = b_node.run(b, |_| {}) => panic!("stopped running: {result:?}"),
        () = steps => {}
    }
}

/// Waits for `check` to hold.
async fn until(check: impl Fn() -> bool) {
    let wait = async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(TIMEOUT, wait)
        .await
        .expect("timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn linked_computers_get_each_others_profiles_including_new_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let a = patchwork(&tmp.path().join("a"));
    let on_a = a.create_profile("my-game", "on-a").unwrap();
    let b = patchwork(&tmp.path().join("b"));
    let on_b = b.create_profile("my-game", "on-b").unwrap();
    let a_node = node(&a).await;
    let b_node = node(&b).await;

    let ticket = a_node.link_devices(&a).await.unwrap();
    assert!(a_node.devices_linked().unwrap());
    let joined = b_node.join(&b, &ticket, TIMEOUT).await.unwrap();
    assert!(
        matches!(joined, Joined::Devices { profiles: 1 }),
        "{joined:?}"
    );

    let has = |pw: &Patchwork, profile: &ProfileRef| pw.load_profile(profile).is_ok();
    let steps = async {
        until(|| has(&a, &on_b) && has(&b, &on_a)).await;
        // each lists the other, and not itself
        assert_eq!(b_node.device_peers().await.unwrap(), [a_node.id()]);

        // a profile made later goes over too, and so do changes to it
        let later = a.create_profile("my-game", "later").unwrap();
        until(|| has(&b, &later)).await;
        b.set_hidden_tags(&later, &["library".to_owned()]).unwrap();
        until(|| {
            a.load_profile(&later)
                .is_ok_and(|p| p.hidden_tags == ["library"])
        })
        .await;

        // one removed on a stops syncing there, and stays on b
        a.remove_profile(&later).unwrap();
        until(|| a_node.synced_profile(&later).unwrap().is_none()).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(!has(&a, &later));
        assert!(has(&b, &later));
    };
    both_running((&a_node, &a), (&b_node, &b), steps).await;

    a_node.shutdown().await.unwrap();
    b_node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn friends_pass_changes_on_without_whoever_shared() {
    let tmp = tempfile::tempdir().unwrap();
    let a = patchwork(&tmp.path().join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    let a_node = node(&a).await;
    let ticket = a_node
        .share(&a, &profile, true, Addresses::Included)
        .await
        .unwrap();
    let b = patchwork(&tmp.path().join("b"));
    let b_node = node(&b).await;
    b_node.join(&b, &ticket, TIMEOUT).await.unwrap();
    let c = patchwork(&tmp.path().join("c"));
    let c_node = node(&c).await;
    c_node.join(&c, &ticket, TIMEOUT).await.unwrap();
    // c hears about b from a before a goes
    a_node.sync_once(&a, Duration::from_secs(2)).await.unwrap();
    c_node.sync_once(&c, Duration::from_secs(5)).await.unwrap();
    a_node.shutdown().await.unwrap();

    b.set_hidden_tags(&profile, &["library".to_owned()])
        .unwrap();
    let c_gets_it = until(|| {
        c.load_profile(&profile)
            .is_ok_and(|p| p.hidden_tags == ["library"])
    });
    both_running((&b_node, &b), (&c_node, &c), c_gets_it).await;

    b_node.shutdown().await.unwrap();
    c_node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_keeps_syncing_after_its_id_is_reset() {
    let tmp = tempfile::tempdir().unwrap();
    let a = patchwork(&tmp.path().join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    let a_node = node(&a).await;
    let ticket = a_node
        .share(&a, &profile, true, Addresses::Included)
        .await
        .unwrap();
    let b = patchwork(&tmp.path().join("b"));
    let b_node = node(&b).await;
    b_node.join(&b, &ticket, TIMEOUT).await.unwrap();
    // a hears where b is, which offline it couldn't look up
    a_node.sync_once(&a, Duration::from_secs(2)).await.unwrap();

    let old = a_node.id();
    let new = a_node.reset_id().await.unwrap();
    assert_ne!(old, new);
    let a_node = node(&a).await;
    assert_eq!(a_node.id(), new);

    a.set_hidden_tags(&profile, &["library".to_owned()])
        .unwrap();
    let b_gets_it = until(|| {
        b.load_profile(&profile)
            .is_ok_and(|p| p.hidden_tags == ["library"])
    });
    both_running((&a_node, &a), (&b_node, &b), b_gets_it).await;

    a_node.shutdown().await.unwrap();
    b_node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ticket_can_leave_out_ip_addresses() {
    let tmp = tempfile::tempdir().unwrap();
    let a = patchwork(&tmp.path().join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    let a_node = node(&a).await;
    let addrs = async |addresses| {
        let ticket = a_node.share(&a, &profile, false, addresses).await.unwrap();
        let ticket: iroh_docs::DocTicket = ticket.parse().unwrap();
        ticket
            .nodes
            .iter()
            .map(|n| n.ip_addrs().count())
            .sum::<usize>()
    };
    assert!(addrs(Addresses::Included).await > 0);
    assert_eq!(addrs(Addresses::Hidden).await, 0);
    a_node.shutdown().await.unwrap();
}

/// Three friends sharing a profile, who all know where each other are.
async fn three_friends(tmp: &Path) -> ([Patchwork; 3], [Node; 3], ProfileRef) {
    let a = patchwork(&tmp.join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    let a_node = node(&a).await;
    let ticket = a_node
        .share(&a, &profile, true, Addresses::Included)
        .await
        .unwrap();
    let b = patchwork(&tmp.join("b"));
    let b_node = node(&b).await;
    b_node.join(&b, &ticket, TIMEOUT).await.unwrap();
    let c = patchwork(&tmp.join("c"));
    let c_node = node(&c).await;
    c_node.join(&c, &ticket, TIMEOUT).await.unwrap();
    // offline, each only finds the others at the addresses they announce
    let short = Duration::from_secs(3);
    let (a_out, b_out, c_out) = tokio::join!(
        a_node.sync_once(&a, short),
        b_node.sync_once(&b, short),
        c_node.sync_once(&c, short),
    );
    a_out.unwrap();
    b_out.unwrap();
    c_out.unwrap();
    let mut peers = a_node.peers(&profile).await.unwrap();
    peers.sort();
    let mut expected = vec![b_node.id(), c_node.id()];
    expected.sort();
    assert_eq!(peers, expected);
    ([a, b, c], [a_node, b_node, c_node], profile)
}

fn doc_of(node: &Node, profile: &ProfileRef) -> String {
    node.synced_profile(profile).unwrap().unwrap().doc
}

/// Waits up to `within` for `check` to hold.
async fn until_within(within: Duration, check: impl Fn() -> bool) {
    let wait = async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(within, wait).await.expect("timed out");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_removed_friend_stops_getting_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let ([a, b, c], [a_node, b_node, c_node], profile) = three_friends(tmp.path()).await;
    let old = doc_of(&a_node, &profile);

    let steps = async {
        let removed = a_node
            .remove_peers(&a, &profile, &[c_node.id()])
            .await
            .unwrap();
        assert_eq!(removed.told, 1);
        assert_eq!(removed.to_tell, 0);
        let new = doc_of(&a_node, &profile);
        assert_ne!(new, old);
        assert_eq!(doc_of(&b_node, &profile), new);
        assert_eq!(b_node.peers(&profile).await.unwrap(), [a_node.id()]);
        // c heard the old document was left, so takes nothing more from it
        until(|| c_node.synced_profile(&profile).unwrap().unwrap().moved).await;
        assert_eq!(doc_of(&c_node, &profile), old);

        // a and b still sync with each other
        a.set_hidden_tags(&profile, &["library".to_owned()])
            .unwrap();
        until(|| {
            b.load_profile(&profile)
                .is_ok_and(|p| p.hidden_tags == ["library"])
        })
        .await;
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(c.load_profile(&profile).unwrap().hidden_tags.is_empty());
    };
    tokio::select! {
        result = a_node.run(&a, |_| {}) => panic!("stopped running: {result:?}"),
        result = b_node.run(&b, |_| {}) => panic!("stopped running: {result:?}"),
        result = c_node.run(&c, |_| {}) => panic!("stopped running: {result:?}"),
        () = steps => {}
    }

    // nor can c remove anyone now
    assert!(
        c_node
            .remove_peers(&c, &profile, &[a_node.id()])
            .await
            .is_err()
    );

    a_node.shutdown().await.unwrap();
    b_node.shutdown().await.unwrap();
    c_node.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_friend_offline_during_a_removal_is_told_later() {
    let tmp = tempfile::tempdir().unwrap();
    let ([a, b, _c], [a_node, b_node, c_node], profile) = three_friends(tmp.path()).await;
    let c_id = c_node.id();
    b_node.shutdown().await.unwrap();
    c_node.shutdown().await.unwrap();

    let removed = a_node.remove_peers(&a, &profile, &[c_id]).await.unwrap();
    assert_eq!(removed.told, 0);
    assert_eq!(removed.to_tell, 1);
    let new = doc_of(&a_node, &profile);

    let b_node = node(&b).await;
    // b takes the new document before it answers, so a hearing back is
    // waited for too, rather than checked once b has moved
    let b_moves = until_within(Duration::from_secs(60), || {
        doc_of(&b_node, &profile) == new
            && a_node
                .synced_profile(&profile)
                .unwrap()
                .unwrap()
                .to_tell
                .is_empty()
    });
    both_running((&a_node, &a), (&b_node, &b), b_moves).await;

    a_node.shutdown().await.unwrap();
    b_node.shutdown().await.unwrap();
}
