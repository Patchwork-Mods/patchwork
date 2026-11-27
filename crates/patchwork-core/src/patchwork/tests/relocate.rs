//! Moving the data folder; see `patchwork/relocate.rs`.

use super::*;
use crate::paths::Locations;

/// Puts each mod's files in the game's folder as they are.
struct AsIs;

impl Deployer for AsIs {
    fn kind(&self) -> &str {
        "as-is"
    }

    fn plan(&self, _: &DeployContext<'_>, mods: &[InstalledMod]) -> Result<deploy::DesiredState> {
        let placements = mods
            .iter()
            .flat_map(|m| {
                m.files.iter().map(|f| deploy::Placement {
                    dst: f.rel.to_string().parse().unwrap(),
                    src: deploy::Src::Mod {
                        hash: m.hash,
                        rel: f.rel.clone(),
                    },
                    mod_name: m.name.clone(),
                })
            })
            .collect();
        Ok(deploy::DesiredState {
            placements,
            ..Default::default()
        })
    }
}

fn open_at(root: &Path) -> Patchwork {
    Patchwork::builder(Paths::rooted(root).located().unwrap())
        .source(LocalFiles)
        .deployer(AsIs)
        .open()
        .unwrap()
}

/// patchwork rooted at `tmp/home`, with a local mod deployed into
/// `tmp/game` by symlinking.
async fn deployed(tmp: &Path) -> (Patchwork, ProfileRef, PathBuf) {
    let file = tmp.join("mods/Cool.pak");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "cool").unwrap();
    let pw = open_at(&tmp.join("home"));
    pw.add_game(new_game("my-game", "as-is")).unwrap();
    let game = tmp.join("game");
    fs::create_dir_all(&game).unwrap();
    let mut install = GameInstall::new(game.clone());
    install.link = Some(LinkMethod::Symlink);
    pw.set_game_install("my-game", install).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    pw.add_mod(&profile, local_spec(&file), None, true)
        .await
        .unwrap();
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    (pw, profile, game.join("Cool.pak"))
}

#[tokio::test]
async fn the_data_folder_moves_with_what_is_deployed_from_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, profile, deployed_file) = deployed(tmp.path()).await;
    let old = pw.paths().data.clone();
    assert!(fs::read_link(&deployed_file).unwrap().starts_with(&old));

    let to = tmp.path().join("elsewhere/patchwork");
    let plan = pw.plan_data_move(&to).unwrap();
    assert!(plan.conflicts.is_empty());
    assert!(plan.empty);
    assert!(plan.bytes > 0 && plan.files > 0);
    let mut seen = Vec::new();
    pw.move_data(&to, &Cancel::never(), &mut |p| seen.push(p))
        .unwrap();
    assert_eq!(seen.last().map(|p| p.done), Some(plan.bytes));

    // the game's link points at the new place, and still reads the mod
    assert!(fs::read_link(&deployed_file).unwrap().starts_with(&to));
    assert_eq!(fs::read_to_string(&deployed_file).unwrap(), "cool");
    drop(pw);

    // opening again uses it, and removes the old folder
    let pw = open_at(&tmp.path().join("home"));
    assert_eq!(pw.paths().data, to);
    assert!(!old.exists());
    assert_eq!(Locations::load(pw.paths()).unwrap().leftover, None);
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
    pw.apply(
        &pw.plan_undeploy(&DeployTarget::Game("my-game".into()))
            .unwrap(),
    )
    .unwrap();
    assert!(deployed_file.symlink_metadata().is_err());
}

#[tokio::test]
async fn a_folder_with_the_same_names_in_it_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, _, deployed_file) = deployed(tmp.path()).await;
    let to = tmp.path().join("taken");
    fs::create_dir_all(to.join("store")).unwrap();
    fs::write(to.join("notes.txt"), "mine").unwrap();
    let plan = pw.plan_data_move(&to).unwrap();
    assert_eq!(plan.conflicts, ["store"]);
    assert!(!plan.empty);
    let err = pw
        .move_data(&to, &Cancel::never(), &mut |_| {})
        .unwrap_err();
    assert!(err.to_string().contains("store"), "{err}");
    // nothing moved, and what was there is left be
    assert!(pw.paths().store().is_dir());
    assert_eq!(fs::read_to_string(to.join("notes.txt")).unwrap(), "mine");
    assert!(
        fs::read_link(&deployed_file)
            .unwrap()
            .starts_with(&pw.paths().data)
    );

    // other things in the folder are fine
    fs::remove_dir(to.join("store")).unwrap();
    pw.move_data(&to, &Cancel::never(), &mut |_| {}).unwrap();
    assert!(to.join("store").is_dir());
    assert_eq!(fs::read_to_string(to.join("notes.txt")).unwrap(), "mine");
}

#[tokio::test]
async fn a_cancelled_move_leaves_everything_where_it_was() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, _, deployed_file) = deployed(tmp.path()).await;
    let to = tmp.path().join("new");
    let cancel = Cancel::default();
    cancel.cancel();
    let err = pw.move_data(&to, &cancel, &mut |_| {}).unwrap_err();
    assert!(matches!(err, Error::Cancelled), "{err}");
    assert!(!to.exists());
    assert!(pw.paths().store().is_dir());
    assert_eq!(Locations::load(pw.paths()).unwrap(), Locations::default());
    assert_eq!(fs::read_to_string(&deployed_file).unwrap(), "cool");
}

#[test]
fn the_data_folder_cant_move_into_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = open_at(tmp.path());
    let data = pw.paths().data.clone();
    for to in [data.clone(), data.join("inner"), tmp.path().to_owned()] {
        assert!(pw.plan_data_move(&to).is_err(), "{}", to.display());
    }
    assert!(pw.plan_data_move(Path::new("relative")).is_err());
}

#[test]
fn a_moved_data_folder_that_is_gone_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    Locations {
        data: Some(tmp.path().join("unplugged")),
        ..Default::default()
    }
    .save(&paths)
    .unwrap();
    let err = paths.located().unwrap_err();
    assert!(err.to_string().contains("unplugged"), "{err}");
}

#[cfg(unix)]
#[tokio::test]
async fn copying_to_another_drive_keeps_links_and_hard_links() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, profile, deployed_file) = deployed(tmp.path()).await;
    let old = pw.paths().data.clone();
    // a hard link and a symlink inside the folder, like a profile's own
    // folder deployed by linking
    let stored = old.join("instances/x/a");
    fs::create_dir_all(stored.parent().unwrap()).unwrap();
    fs::write(&stored, "a").unwrap();
    fs::hard_link(&stored, old.join("instances/x/b")).unwrap();
    std::os::unix::fs::symlink(&stored, old.join("instances/x/c")).unwrap();

    let to = tmp.path().join("other-drive");
    pw.move_by(Movable::Data, &to, false, &Cancel::never(), &mut |_| {})
        .unwrap();
    // copied, not moved, so the old folder is still whole until reopening
    assert!(old.join("store").is_dir());
    assert!(fs::read_link(&deployed_file).unwrap().starts_with(&to));
    assert_eq!(fs::read_to_string(&deployed_file).unwrap(), "cool");
    use std::os::unix::fs::MetadataExt;
    let ino = |p: &str| fs::metadata(to.join(p)).unwrap().ino();
    assert_eq!(ino("instances/x/a"), ino("instances/x/b"));
    assert_eq!(
        fs::read_link(to.join("instances/x/c")).unwrap(),
        to.join("instances/x/a")
    );
    drop(pw);

    let pw = open_at(&tmp.path().join("home"));
    assert!(!old.exists());
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
}

#[tokio::test]
async fn the_store_moves_on_its_own_and_back() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, profile, deployed_file) = deployed(tmp.path()).await;
    let old = pw.paths().store();
    assert_eq!(old, pw.paths().default_store());
    assert!(fs::read_link(&deployed_file).unwrap().starts_with(&old));

    let to = tmp.path().join("big-drive/mods");
    let plan = pw.plan_move(Movable::Store, &to).unwrap();
    assert_eq!(plan.what, Movable::Store);
    assert!(plan.conflicts.is_empty() && plan.bytes > 0);
    // copied, as between drives
    pw.move_by(Movable::Store, &to, false, &Cancel::never(), &mut |_| {})
        .unwrap();
    assert!(fs::read_link(&deployed_file).unwrap().starts_with(&to));
    assert_eq!(fs::read_to_string(&deployed_file).unwrap(), "cool");
    drop(pw);

    let pw = open_at(&tmp.path().join("home"));
    assert_eq!(pw.paths().store(), to);
    assert!(!old.exists());
    let locations = Locations::load(pw.paths()).unwrap();
    assert_eq!(locations.store.as_deref(), Some(to.as_path()));
    assert_eq!(locations.store_leftover, None);
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
    // what's added now goes there
    let other = tmp.path().join("mods/Other.pak");
    fs::write(&other, "other").unwrap();
    pw.add_mod(&profile, local_spec(&other), None, true)
        .await
        .unwrap();
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    let other_deployed = tmp.path().join("game/Other.pak");
    assert!(fs::read_link(&other_deployed).unwrap().starts_with(&to));

    // and back to the data folder, renamed this time
    let back = pw.paths().default_store();
    pw.move_folder(Movable::Store, &back, &Cancel::never(), &mut |_| {})
        .unwrap();
    drop(pw);
    let pw = open_at(&tmp.path().join("home"));
    assert_eq!(pw.paths().store(), back);
    assert_eq!(Locations::load(pw.paths()).unwrap(), Locations::default());
    assert!(!to.exists());
    for file in [&deployed_file, &other_deployed] {
        assert!(fs::read_link(file).unwrap().starts_with(&back));
    }
    assert_eq!(fs::read_to_string(&other_deployed).unwrap(), "other");
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
}

#[tokio::test]
async fn the_data_folder_moves_without_a_store_moved_on_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, profile, deployed_file) = deployed(tmp.path()).await;
    let store = tmp.path().join("mods-drive");
    pw.move_folder(Movable::Store, &store, &Cancel::never(), &mut |_| {})
        .unwrap();
    drop(pw);
    let pw = open_at(&tmp.path().join("home"));

    // not in or around the store
    for to in [store.join("data"), tmp.path().to_owned()] {
        assert!(pw.plan_data_move(&to).is_err(), "{}", to.display());
    }
    let to = tmp.path().join("new-data");
    pw.move_data(&to, &Cancel::never(), &mut |_| {}).unwrap();
    drop(pw);
    let pw = open_at(&tmp.path().join("home"));
    assert_eq!(pw.paths().data, to);
    assert_eq!(pw.paths().store(), store);
    assert!(fs::read_link(&deployed_file).unwrap().starts_with(&store));
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
}

#[test]
fn the_store_only_goes_in_the_data_folder_where_it_usually_is() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = open_at(&tmp.path().join("home"));
    let data = pw.paths().data.clone();
    for to in [
        data.join("elsewhere"),
        data.clone(),
        pw.paths().store().join("inner"),
        pw.paths().config.join("mods"),
        tmp.path().to_owned(),
    ] {
        assert!(
            pw.plan_move(Movable::Store, &to).is_err(),
            "{}",
            to.display()
        );
    }
    // not made yet, which is nothing to move
    let plan = pw
        .plan_move(Movable::Store, &tmp.path().join("mods"))
        .unwrap();
    assert_eq!((plan.bytes, plan.files), (0, 0));
}

#[test]
fn a_moved_store_that_is_gone_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    Locations {
        store: Some(tmp.path().join("unplugged")),
        ..Default::default()
    }
    .save(&paths)
    .unwrap();
    let err = paths.located().unwrap_err();
    assert!(err.to_string().contains("unplugged"), "{err}");
}
