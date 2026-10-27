use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;

use super::*;

fn known_summary(latest: &str, fetched: &str) -> KnownSummary {
    KnownSummary {
        id: "a-b".to_owned(),
        fetched: fetched.parse().unwrap(),
        summary: Some(ModSummary {
            source: "thunderstore".to_owned(),
            id: "a-b".to_owned(),
            name: "b".to_owned(),
            url: None,
            author: None,
            author_url: None,
            summary: None,
            icon_url: None,
            downloads: None,
            rating: None,
            updated: None,
            latest_version: Some(latest.to_owned()),
            categories: Vec::new(),
        }),
    }
}

fn known_versions(newest: &str, fetched: &str) -> KnownVersions {
    KnownVersions {
        id: "a-b".to_owned(),
        fetched: fetched.parse().unwrap(),
        versions: vec![newest.to_owned(), "1.0.0".to_owned()],
    }
}

#[test]
fn summary_is_behind_once_later_versions_have_a_newer_one() {
    let s = known_summary("1.1.0", "2026-10-01T00:00:00Z");
    let later = "2026-10-02T00:00:00Z";
    assert!(s.is_behind(&known_versions("1.2.0", later)));
    assert!(!s.is_behind(&known_versions("1.1.0", later)));
    // versions from before the summary was fetched don't count
    let earlier = "2026-09-30T00:00:00Z";
    assert!(!s.is_behind(&known_versions("1.2.0", earlier)));
}

/// Answers one HTTP request with `status` and `body`, returning its URL.
fn serve_once(status: &'static str, body: &'static [u8]) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/mod.zip", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 1024];
        let _ = stream.read(&mut request).unwrap();
        write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .unwrap();
        stream.write_all(body).unwrap();
    });
    url
}

#[tokio::test]
async fn downloads_to_a_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("downloads/download.tmp");
    download(&serve_once("200 OK", b"zip bytes"), &dest)
        .await
        .unwrap();
    assert_eq!(fs::read(&dest).unwrap(), b"zip bytes");
}

#[tokio::test]
async fn http_errors_fail_the_download() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("download.tmp");
    let err = download(&serve_once("404 Not Found", b""), &dest)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("404"), "{err}");
}

struct NoDeployer;

impl Deployer for NoDeployer {
    fn kind(&self) -> &str {
        "none"
    }

    fn plan(&self, _: &DeployContext<'_>, _: &[InstalledMod]) -> Result<deploy::DesiredState> {
        Ok(Default::default())
    }
}

fn new_game(id: &str, kind: &str) -> GameDef {
    GameDef {
        schema: 0,
        id: id.to_owned(),
        name: "My Game".to_owned(),
        description: None,
        image: None,
        hidden: false,
        sources: BTreeMap::new(),
        deployer: crate::game::DeployerSpec {
            kind: kind.to_owned(),
            config: toml::Table::new(),
        },
        link: Default::default(),
        detect: Default::default(),
        versions: Default::default(),
        roots: Default::default(),
        launch: Default::default(),
    }
}

#[test]
fn added_games_are_saved_and_usable_straight_away() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();

    pw.add_game(new_game("my-game", "none")).unwrap();
    let entry = pw.registry().game("my-game").unwrap().clone();
    assert_eq!(entry.origin, crate::registry::Origin::User);
    assert_eq!(entry.value.schema, GAME_SCHEMA);
    pw.create_profile("my-game", "default").unwrap();

    let duplicate = pw.add_game(new_game("my-game", "none")).unwrap_err();
    assert!(duplicate.to_string().contains("already"), "{duplicate}");
    let builtin = pw.add_game(new_game("lethal-company", "none")).unwrap_err();
    assert!(builtin.to_string().contains("already"), "{builtin}");
    assert!(pw.add_game(new_game("My Game", "none")).is_err());
    assert!(pw.add_game(new_game("other", "missing")).is_err());
}

/// Resolves every mod to the same URL.
struct FixedUrl(String);

#[async_trait::async_trait]
impl Source for FixedUrl {
    fn id(&self) -> &str {
        "fixed"
    }

    async fn resolve(
        &self,
        _: &SourceContext<'_>,
        id: &str,
        _: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        Ok(crate::source::ResolvedMod {
            id: id.to_owned(),
            name: id.to_owned(),
            version: Some("1.0".to_owned()),
            artifact: ArtifactLocation::Url {
                url: self.0.clone(),
            },
            format: ArtifactFormat::File,
            dependencies: Vec::new(),
            game_versions: None,
        })
    }
}

#[tokio::test]
async fn updating_skips_downloads_that_havent_changed() {
    let tmp = tempfile::tempdir().unwrap();
    // the server answers once, so a second download would fail
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(FixedUrl(serve_once("200 OK", b"mod bytes")))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let spec = ModSpec {
        source: "fixed".to_owned(),
        id: "a".to_owned(),
        version: None,
    };
    pw.add_mod(&profile, spec, None, true).await.unwrap();

    let first = pw.lock(&profile, true).await.unwrap();
    let second = pw.lock(&profile, true).await.unwrap();
    assert_eq!(first.mods[0].hash, second.mods[0].hash);
}

/// Resolves every mod to a file of its own, which downloads from `url`, or
/// needs the user if there's no `url`. Counts how often it's asked. Takes
/// links like `own://<id>`, which download from `url` either way.
struct OwnFiles {
    url: Option<String>,
    downloads: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Source for OwnFiles {
    fn id(&self) -> &str {
        "own"
    }

    async fn resolve(
        &self,
        _: &SourceContext<'_>,
        id: &str,
        _: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        Ok(crate::source::ResolvedMod {
            id: id.to_owned(),
            name: id.to_owned(),
            version: Some("1.0".to_owned()),
            artifact: ArtifactLocation::Source {
                file: "7".to_owned(),
                name: format!("{id}.txt"),
            },
            format: ArtifactFormat::File,
            dependencies: Vec::new(),
            game_versions: None,
        })
    }

    async fn download(&self, _: &SourceContext<'_>, id: &str, file: &str) -> Result<Download> {
        assert_eq!(file, "7");
        self.downloads.fetch_add(1, SeqCst);
        Ok(match &self.url {
            Some(url) => Download::Url(url.clone()),
            None => Download::NeedsUser {
                page: format!("https://example.com/{id}"),
            },
        })
    }

    async fn accept_link(
        &self,
        _: &SourceContext<'_>,
        link: &str,
    ) -> Result<Option<crate::source::Handoff>> {
        Ok(link
            .strip_prefix("own://")
            .map(|id| crate::source::Handoff {
                id: id.to_owned(),
                file: "7".to_owned(),
                url: self.url.clone().unwrap(),
            }))
    }
}

/// A profile with mods `ids` from [`OwnFiles`], and its download count.
async fn own_files_profile(
    tmp: &Path,
    url: Option<String>,
    ids: &[&str],
) -> (Patchwork, ProfileRef, Arc<AtomicUsize>) {
    let downloads = Arc::new(AtomicUsize::new(0));
    let source = OwnFiles {
        url,
        downloads: downloads.clone(),
    };
    let pw = Patchwork::builder(Paths::rooted(tmp))
        .source(source)
        .deployer(NoDeployer)
        .open()
        .unwrap();
    let mut game = new_game("my-game", "none");
    game.sources.insert("own".to_owned(), toml::Table::new());
    pw.add_game(game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    for id in ids {
        let spec = ModSpec {
            source: "own".to_owned(),
            id: id.to_string(),
            version: None,
        };
        pw.add_mod(&profile, spec, None, true).await.unwrap();
    }
    (pw, profile, downloads)
}

fn pending(err: Error) -> Vec<PendingDownload> {
    match err {
        Error::NeedsDownload(pending) => pending,
        err => panic!("expected NeedsDownload, got {err}"),
    }
}

#[tokio::test]
async fn source_files_are_downloaded_once() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve_once("200 OK", b"mod bytes");
    let (pw, profile, downloads) = own_files_profile(tmp.path(), Some(url), &["a"]).await;

    let first = pw.lock(&profile, true).await.unwrap();
    let second = pw.lock(&profile, true).await.unwrap();
    assert_eq!(first.mods[0].hash, second.mods[0].hash);
    assert_eq!(first.mods[0].artifact.file_name(), "a.txt");
    assert_eq!(downloads.load(SeqCst), 1);

    // a lockfile lost along with the store's copy of its download still
    // finds the file through the source, rather than trusting a stale entry
    fs::remove_dir_all(tmp.path().join("data/store")).unwrap();
    let err = pw.lock(&profile, false).await.unwrap_err();
    assert!(err.to_string().contains("mod.zip"), "{err}");
    assert_eq!(downloads.load(SeqCst), 2);
}

#[tokio::test]
async fn every_download_the_user_has_to_do_is_listed_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, profile, _) = own_files_profile(tmp.path(), None, &["a", "b"]).await;

    let err = pw.lock(&profile, false).await.unwrap_err();
    assert!(err.to_string().contains("https://example.com/b"), "{err}");
    let pending = pending(err);
    assert_eq!(
        pending[0],
        PendingDownload {
            name: "a".to_owned(),
            page: "https://example.com/a".to_owned(),
            file: SourceFile {
                game: "my-game".to_owned(),
                source: "own".to_owned(),
                id: "a".to_owned(),
                file: "7".to_owned(),
            },
        }
    );
    assert_eq!(pending[1].name, "b");
    assert!(pw.load_lock(&profile).unwrap().is_none());
}

#[tokio::test]
async fn provided_files_are_used_by_the_next_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, profile, _) = own_files_profile(tmp.path(), None, &["a", "b"]).await;
    let file = tmp.path().join("a.zip");
    fs::write(&file, b"a bytes").unwrap();

    let pending = pending(pw.lock(&profile, false).await.unwrap_err());
    let hash = pw.provide(&pending[0].file, &file).unwrap();
    let still = self::pending(pw.lock(&profile, false).await.unwrap_err());
    assert_eq!(still.len(), 1);
    assert_eq!(still[0].name, "b");

    fs::write(&file, b"b bytes").unwrap();
    pw.provide(&still[0].file, &file).unwrap();
    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(lock.mods[0].hash, hash);
    assert_ne!(lock.mods[1].hash, hash);
}

#[tokio::test]
async fn links_from_the_browser_download_their_file() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve_once("200 OK", b"mod bytes");
    let (pw, profile, downloads) = own_files_profile(tmp.path(), Some(url), &["a"]).await;

    assert_eq!(pw.accept_link("https://example.com").await.unwrap(), None);
    let file = pw.accept_link("own://a").await.unwrap().unwrap();
    assert_eq!(
        (file.game.as_str(), file.key()),
        ("my-game", "own:a".to_owned())
    );
    // the lock doesn't need the source to download it again
    pw.lock(&profile, false).await.unwrap();
    assert_eq!(downloads.load(SeqCst), 0);
}

/// Resolves every mod to `<id>-<version>.txt` in a folder, at whichever
/// version is current.
struct Versioned {
    dir: PathBuf,
    version: std::sync::Mutex<String>,
}

#[async_trait::async_trait]
impl Source for Versioned {
    fn id(&self) -> &str {
        "versioned"
    }

    async fn resolve(
        &self,
        _: &SourceContext<'_>,
        id: &str,
        requested: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        let current = self.version.lock().unwrap().clone();
        let version = requested.unwrap_or(&current).to_owned();
        let path = self.dir.join(format!("{id}-{version}.txt"));
        fs::write(&path, format!("{id} {version}")).unwrap();
        Ok(crate::source::ResolvedMod {
            id: id.to_owned(),
            name: id.to_owned(),
            version: Some(version),
            artifact: ArtifactLocation::Path { path },
            format: ArtifactFormat::File,
            dependencies: Vec::new(),
            game_versions: None,
        })
    }
}

#[tokio::test]
async fn updating_some_mods_leaves_the_rest_locked() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mods");
    fs::create_dir_all(&dir).unwrap();
    let source = std::sync::Arc::new(Versioned {
        dir,
        version: std::sync::Mutex::new("1.0".to_owned()),
    });
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(SharedSource(source.clone()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    for id in ["a", "b"] {
        let spec = ModSpec {
            source: "versioned".to_owned(),
            id: id.to_owned(),
            version: None,
        };
        pw.add_mod(&profile, spec, None, true).await.unwrap();
    }
    pw.lock(&profile, false).await.unwrap();

    *source.version.lock().unwrap() = "2.0".to_owned();
    let lock = pw
        .lock_updating(&profile, &["versioned:a".to_owned()])
        .await
        .unwrap();
    let version = |id: &str| {
        let m = lock.mods.iter().find(|m| m.id == id).unwrap();
        m.version.clone().unwrap()
    };
    assert_eq!(version("a"), "2.0");
    assert_eq!(version("b"), "1.0");
}

/// Lists the same versions for every mod, counting how often it's asked.
struct SlowVersions(std::sync::Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Source for SlowVersions {
    fn id(&self) -> &str {
        "slow"
    }

    async fn resolve(
        &self,
        _: &SourceContext<'_>,
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        unimplemented!()
    }

    fn lists_versions(&self) -> bool {
        true
    }

    async fn versions(&self, _: &SourceContext<'_>, _: &str) -> Result<Vec<String>> {
        self.0.fetch_add(1, SeqCst);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        Ok(vec!["2.0".to_owned(), "1.0".to_owned()])
    }
}

#[tokio::test]
async fn versions_being_fetched_are_waited_on_not_fetched_again() {
    let tmp = tempfile::tempdir().unwrap();
    let fetches = std::sync::Arc::new(AtomicUsize::new(0));
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(SlowVersions(fetches.clone()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();

    let (a, b) = futures_util::future::join(
        pw.versions("my-game", "slow", "a"),
        pw.versions("my-game", "slow", "a"),
    )
    .await;
    assert_eq!(a.unwrap(), ["2.0", "1.0"]);
    assert_eq!(b.unwrap(), ["2.0", "1.0"]);
    assert_eq!(fetches.load(SeqCst), 1);

    // ones fetched recently enough are used as they are
    let since = Timestamp::now() - SignedDuration::from_mins(5);
    pw.versions_since("my-game", "slow", "a", since)
        .await
        .unwrap();
    assert_eq!(fetches.load(SeqCst), 1);
    pw.versions("my-game", "slow", "a").await.unwrap();
    assert_eq!(fetches.load(SeqCst), 2);
}

#[tokio::test]
async fn reverting_undoes_changes_since_the_last_deploy() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mods");
    fs::create_dir_all(&dir).unwrap();
    let source = std::sync::Arc::new(Versioned {
        dir,
        version: std::sync::Mutex::new("1.0".to_owned()),
    });
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(SharedSource(source.clone()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let spec = |id: &str| ModSpec {
        source: "versioned".to_owned(),
        id: id.to_owned(),
        version: None,
    };
    pw.add_mod(&profile, spec("a"), None, true).await.unwrap();
    // nothing to go back to before the first deploy
    assert!(!pw.has_changes(&profile).unwrap());
    assert!(pw.revert(&profile).is_err());

    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert!(!pw.has_changes(&profile).unwrap());

    pw.add_mod(&profile, spec("b"), None, true).await.unwrap();
    pw.set_enabled(&profile, "a", false).unwrap();
    *source.version.lock().unwrap() = "2.0".to_owned();
    pw.lock(&profile, true).await.unwrap();
    assert!(pw.has_changes(&profile).unwrap());

    pw.revert(&profile).unwrap();
    assert!(!pw.has_changes(&profile).unwrap());
    let loaded = pw.load_profile(&profile).unwrap();
    assert_eq!(loaded.mods.len(), 1);
    assert!(loaded.mods[0].enabled);
    let lock = pw.load_lock(&profile).unwrap().unwrap();
    assert_eq!(lock.mods.len(), 1);
    assert_eq!(lock.mods[0].version.as_deref(), Some("1.0"));
}

#[tokio::test]
async fn history_keeps_each_different_deploy_to_roll_back_to() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mods");
    fs::create_dir_all(&dir).unwrap();
    let source = std::sync::Arc::new(Versioned {
        dir,
        version: std::sync::Mutex::new("1.0".to_owned()),
    });
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(SharedSource(source.clone()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let spec = |id: &str| ModSpec {
        source: "versioned".to_owned(),
        id: id.to_owned(),
        version: None,
    };
    let deploy = async || pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert!(pw.history(&profile).unwrap().is_empty());
    assert!(pw.rollback(&profile, 1).is_err());

    pw.add_mod(&profile, spec("a"), None, true).await.unwrap();
    pw.add_mod(&profile, spec("b"), None, true).await.unwrap();
    deploy().await;
    // deploying the same thing again doesn't add to the history
    deploy().await;
    assert_eq!(pw.history(&profile).unwrap().len(), 1);

    *source.version.lock().unwrap() = "2.0".to_owned();
    pw.lock(&profile, true).await.unwrap();
    pw.set_enabled(&profile, "b", false).unwrap();
    pw.add_mod(&profile, spec("c"), None, true).await.unwrap();
    assert_eq!(
        pw.rollback_changes(&profile, 1).unwrap(),
        vec![
            Change::Updated {
                name: "a".to_owned(),
                from: Some("2.0".to_owned()),
                to: Some("1.0".to_owned()),
            },
            Change::Updated {
                name: "b".to_owned(),
                from: Some("2.0".to_owned()),
                to: Some("1.0".to_owned()),
            },
            // not locked yet, so its version isn't known
            Change::Removed {
                name: "c".to_owned(),
                version: None,
            },
            Change::Enabled {
                name: "b".to_owned()
            },
        ]
    );
    deploy().await;

    let history = pw.history(&profile).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history[0].changes.is_empty());
    assert!(!history[0].current);
    assert_eq!(history[0].enabled, 2);
    assert_eq!(history[1].number, 2);
    assert_eq!(history[1].changes.len(), 4);
    assert!(history[1].current);
    assert!(history[1].deployed_at.is_some());

    pw.rollback(&profile, 1).unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    assert_eq!(loaded.mods.len(), 2);
    assert!(loaded.mods.iter().all(|m| m.enabled));
    let lock = pw.load_lock(&profile).unwrap().unwrap();
    assert!(
        lock.mods
            .iter()
            .all(|m| m.version.as_deref() == Some("1.0"))
    );
    // locking again keeps the old versions rather than updating them
    pw.lock(&profile, false).await.unwrap();
    let lock = pw.load_lock(&profile).unwrap().unwrap();
    assert!(
        lock.mods
            .iter()
            .all(|m| m.version.as_deref() == Some("1.0"))
    );
    assert!(pw.history(&profile).unwrap()[0].current);
    // rolling back is undone like any other change
    assert!(pw.has_changes(&profile).unwrap());
    pw.revert(&profile).unwrap();
    assert_eq!(pw.load_profile(&profile).unwrap().mods.len(), 3);

    pw.rollback(&profile, 1).unwrap();
    deploy().await;
    let numbers: Vec<u32> = pw
        .history(&profile)
        .unwrap()
        .iter()
        .map(|e| e.number)
        .collect();
    assert_eq!(numbers, [1, 2, 3]);
}

#[tokio::test]
async fn history_drops_the_oldest_deploys() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mods");
    fs::create_dir_all(&dir).unwrap();
    let source = std::sync::Arc::new(Versioned {
        dir,
        version: std::sync::Mutex::new("0".to_owned()),
    });
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(SharedSource(source.clone()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let spec = ModSpec {
        source: "versioned".to_owned(),
        id: "a".to_owned(),
        version: None,
    };
    pw.add_mod(&profile, spec, None, true).await.unwrap();
    for n in 0..HISTORY_LIMIT + 2 {
        *source.version.lock().unwrap() = n.to_string();
        pw.lock(&profile, true).await.unwrap();
        pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    }
    let history = pw.history(&profile).unwrap();
    assert_eq!(history.len(), HISTORY_LIMIT);
    assert_eq!(history[0].number, 3);
    assert_eq!(history.last().unwrap().number as usize, HISTORY_LIMIT + 2);
}

#[tokio::test]
async fn removing_a_profile_needs_it_undeployed_first() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let other = pw.create_profile("my-game", "other").unwrap();
    pw.use_profile(&profile).unwrap();

    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    let err = pw.remove_profile(&profile).unwrap_err();
    assert!(err.to_string().contains("undeploy"), "{err}");

    pw.apply(
        &pw.plan_undeploy(&DeployTarget::Game("my-game".into()))
            .unwrap(),
    )
    .unwrap();
    pw.remove_profile(&profile).unwrap();
    assert!(pw.load_profile(&profile).is_err());
    assert!(pw.find_profile("my-game/default").is_err());
    assert_eq!(pw.current_profile().unwrap(), None);
    // only the current profile is forgotten
    pw.use_profile(&other).unwrap();
    pw.remove_profile(&pw.create_profile("my-game", "third").unwrap())
        .unwrap();
    assert_eq!(pw.current_profile().unwrap(), Some(other));
}

/// Lets a test keep hold of a source it's handed to [`Patchwork`].
struct SharedSource(std::sync::Arc<Versioned>);

#[async_trait::async_trait]
impl Source for SharedSource {
    fn id(&self) -> &str {
        self.0.id()
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        version: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        self.0.resolve(ctx, id, version).await
    }
}

/// Builds `data.txt` from the game's own copy, counting how often it runs.
#[derive(Default)]
struct Builder {
    builds: AtomicUsize,
    version: std::sync::Mutex<String>,
}

struct SharedBuilder(std::sync::Arc<Builder>);

impl Deployer for SharedBuilder {
    fn kind(&self) -> &str {
        "builder"
    }

    fn plan(&self, _: &DeployContext<'_>, _: &[InstalledMod]) -> Result<deploy::DesiredState> {
        Ok(Default::default())
    }

    fn builds(&self, _: &DeployContext<'_>) -> bool {
        true
    }

    fn build(
        &self,
        _: &DeployContext<'_>,
        build: &BuildContext<'_>,
        _: &[InstalledMod],
    ) -> Result<deploy::DesiredState> {
        self.0.builds.fetch_add(1, SeqCst);
        let dst = crate::relpath::RelPath::new("data.txt").unwrap();
        let original = fs::read_to_string(build.game.original(&dst).unwrap()).unwrap();
        fs::write(build.out.join("data.txt"), format!("{original} patched")).unwrap();
        Ok(deploy::DesiredState {
            placements: vec![deploy::Placement {
                dst: deploy::Dest::game(dst.clone()),
                src: deploy::Src::Built(dst),
                mod_name: "built".to_owned(),
            }],
            warnings: Vec::new(),
        })
    }

    fn fingerprint(&self, _: &DeployContext<'_>, _: &GameFiles<'_>) -> Result<Option<ContentHash>> {
        Ok(Some(ContentHash::of(
            self.0.version.lock().unwrap().as_bytes(),
        )))
    }
}

#[tokio::test]
async fn built_files_are_reused_until_the_game_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let builder = std::sync::Arc::new(Builder::default());
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(SharedBuilder(builder.clone()))
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "builder")).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    fs::write(game.join("data.txt"), "vanilla").unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let data = || fs::read_to_string(game.join("data.txt")).unwrap();
    let generated = tmp.path().join("data/generated/my-game");
    let builds = || fs::read_dir(&generated).unwrap().count();

    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(data(), "vanilla patched");
    assert_eq!(builder.builds.load(SeqCst), 1);
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);

    // the same inputs reuse the last build
    assert!(pw.plan_deploy(&profile).await.unwrap().is_noop());
    assert_eq!(builder.builds.load(SeqCst), 1);

    // a game update builds again, from the game's own file rather than the
    // deployed one
    *builder.version.lock().unwrap() = "2".to_owned();
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Stale);
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(builder.builds.load(SeqCst), 2);
    assert_eq!(data(), "vanilla patched");
    // the old build is gone: one directory and its record
    assert_eq!(builds(), 2);

    pw.apply(
        &pw.plan_undeploy(&DeployTarget::Game("my-game".into()))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(data(), "vanilla");
    assert_eq!(builds(), 0);
}

#[test]
fn renaming_a_profile_keeps_its_id_and_checks_the_name() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    pw.create_profile("my-game", "taken").unwrap();

    pw.rename_profile(&profile, "renamed").unwrap();
    assert_eq!(pw.find_profile("my-game/renamed").unwrap(), profile);
    assert!(pw.find_profile("my-game/default").is_err());
    // renaming to its own name does nothing
    pw.rename_profile(&profile, "renamed").unwrap();

    let taken = pw.rename_profile(&profile, "taken").unwrap_err();
    assert!(taken.to_string().contains("already exists"), "{taken}");
    assert!(pw.rename_profile(&profile, "has spaces").is_err());
}

/// Local files, like the real `local` source.
struct LocalFiles;

#[async_trait::async_trait]
impl Source for LocalFiles {
    fn id(&self) -> &str {
        "local"
    }

    async fn resolve(
        &self,
        _: &SourceContext<'_>,
        id: &str,
        _: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        let path = PathBuf::from(id);
        if !path.exists() {
            return Err(Error::invalid(format!("{id} isn't there")));
        }
        Ok(crate::source::ResolvedMod {
            id: id.to_owned(),
            name: path.file_stem().unwrap().to_string_lossy().into_owned(),
            version: None,
            format: ArtifactFormat::of_path(&path),
            artifact: ArtifactLocation::Path { path },
            dependencies: Vec::new(),
            game_versions: None,
        })
    }
}

/// patchwork with the `local` source, rooted at `root`.
fn with_local_files(root: &Path) -> Patchwork {
    let pw = Patchwork::builder(Paths::rooted(root))
        .source(LocalFiles)
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    pw
}

fn local_spec(path: &Path) -> ModSpec {
    ModSpec {
        source: "local".to_owned(),
        id: path.to_string_lossy().into_owned(),
        version: None,
    }
}

#[test]
fn duplicating_a_profile_copies_its_mods_and_lockfile() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let mut loaded = pw.load_profile(&profile).unwrap();
    loaded.mods.push(ProfileMod {
        name: "mod".to_owned(),
        source: "local".to_owned(),
        id: ContentHash::of(b"mod bytes").to_string(),
        version: None,
        enabled: false,
        game_versions: None,
    });
    loaded.hidden_tags = vec!["library".to_owned()];
    pw.save_profile(&profile, &loaded).unwrap();
    let lock = Lockfile {
        schema: LOCK_SCHEMA,
        game: LockedGame {
            id: "my-game".to_owned(),
        },
        mods: Vec::new(),
    };
    fsutil::write_toml(&pw.lock_file(&profile), &lock).unwrap();

    let copy = pw.duplicate_profile(&profile, "copy").unwrap();
    assert_ne!(copy, profile);
    assert_eq!(pw.find_profile("my-game/copy").unwrap(), copy);
    let copied = pw.load_profile(&copy).unwrap();
    assert_eq!(copied.hidden_tags, loaded.hidden_tags);
    assert_eq!(copied.mods, loaded.mods);
    assert!(pw.lock_file(&copy).is_file());

    // the copy stands on its own
    pw.remove_profile(&profile).unwrap();
    assert!(pw.load_profile(&copy).is_ok());
    let taken = pw.duplicate_profile(&copy, "copy").unwrap_err();
    assert!(taken.to_string().contains("already exists"), "{taken}");
}

#[tokio::test]
async fn local_mods_are_known_by_their_hash() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("mods/Cool.pak");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "v1").unwrap();
    let pw = with_local_files(&tmp.path().join("a"));
    let profile = pw.create_profile("my-game", "default").unwrap();

    let added = pw
        .add_mod(&profile, local_spec(&file), None, true)
        .await
        .unwrap();
    let v1 = ContentHash::of(b"v1");
    assert_eq!(added.id, v1.to_string());
    assert_eq!(added.name, "Cool");
    // the same file twice is the same mod
    let again = pw
        .add_mod(&profile, local_spec(&file), Some("other".to_owned()), true)
        .await
        .unwrap_err();
    assert!(
        again.to_string().contains("already has that mod"),
        "{again}"
    );
    // it's shown by its file's name, not its hash
    assert_eq!(pw.display_id(None, &added), "Cool.pak");

    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(lock.mods[0].id, v1.to_string());
    assert_eq!(lock.mods[0].hash, v1);
    assert_eq!(
        lock.mods[0].artifact,
        ArtifactLocation::Local {
            name: "Cool.pak".to_owned()
        }
    );
    // nothing about where it is on this machine is in the profile
    for file in [pw.profile_file(&profile), pw.lock_file(&profile)] {
        let text = fs::read_to_string(&file).unwrap();
        assert!(!text.contains(&*tmp.path().to_string_lossy()), "{text}");
    }

    // changes to the file are picked up when it's updated, like any mod,
    // and it keeps its id
    fs::write(&file, "v2").unwrap();
    assert_eq!(pw.lock(&profile, false).await.unwrap().mods[0].hash, v1);
    let lock = pw.lock(&profile, true).await.unwrap();
    assert_eq!(lock.mods[0].hash, ContentHash::of(b"v2"));
    assert_eq!(lock.mods[0].id, v1.to_string());
}

#[tokio::test]
async fn local_mods_from_another_machine_come_from_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("mods/Cool.pak");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "v1").unwrap();
    let a = with_local_files(&tmp.path().join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    a.add_mod(&profile, local_spec(&file), None, true)
        .await
        .unwrap();
    a.lock(&profile, false).await.unwrap();

    // the profile's own folder is all another machine gets
    let b = with_local_files(&tmp.path().join("b"));
    fs::create_dir_all(b.profile_dir(&profile)).unwrap();
    for name in ["profile.toml", "profile.lock"] {
        fs::copy(
            a.profile_dir(&profile).join(name),
            b.profile_dir(&profile).join(name),
        )
        .unwrap();
    }
    for update in [false, true] {
        let err = b.lock(&profile, update).await.unwrap_err();
        assert!(err.to_string().contains("isn't on this one"), "{err}");
    }

    // the lockfile still has its name
    let loaded = b.load_profile(&profile).unwrap();
    let lock = b.load_lock(&profile).unwrap();
    assert_eq!(b.display_id(lock.as_ref(), &loaded.mods[0]), "Cool.pak");
    assert_eq!(
        b.display_id(None, &loaded.mods[0]),
        &loaded.mods[0].id[..12]
    );

    b.store.ingest_path(&file, ArtifactFormat::File).unwrap();
    // there's nothing to update it from here, so it stays as it was
    for update in [false, true] {
        let lock = b.lock(&profile, update).await.unwrap();
        assert_eq!(lock.mods[0].hash, ContentHash::of(b"v1"));
    }
}

#[tokio::test]
async fn profiles_from_before_local_mod_ids_are_migrated() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("mods/Cool.pak");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "cool").unwrap();
    let unlocked = tmp.path().join("mods/New.pak");
    fs::write(&unlocked, "new").unwrap();

    let pw = with_local_files(&tmp.path().join("root"));
    let profile = pw.create_profile("my-game", "default").unwrap();
    let dir = pw.profile_dir(&profile);
    let imported = dir.join("files/abc/Imported.pak");
    fs::create_dir_all(imported.parent().unwrap()).unwrap();
    fs::write(&imported, "imported").unwrap();
    let cool = ContentHash::of(b"cool");
    let imported_hash = ContentHash::of(b"imported");
    pw.store.ingest_path(&file, ArtifactFormat::File).unwrap();

    // how it was: ids and artifacts were paths, and everything was in the
    // profile's own folder
    let local_mod = |path: &Path| ProfileMod {
        name: path.file_stem().unwrap().to_string_lossy().into_owned(),
        source: "local".to_owned(),
        id: path.to_string_lossy().into_owned(),
        version: None,
        enabled: true,
        game_versions: None,
    };
    let locked = |path: &Path, hash| LockedMod {
        name: path.file_stem().unwrap().to_string_lossy().into_owned(),
        source: "local".to_owned(),
        id: path.to_string_lossy().into_owned(),
        requested: None,
        version: None,
        hash,
        format: ArtifactFormat::File,
        artifact: ArtifactLocation::Path {
            path: path.to_owned(),
        },
        dependencies: Vec::new(),
        requires: BTreeMap::new(),
        game_versions: None,
    };
    let mut loaded = pw.load_profile(&profile).unwrap();
    loaded.mods = vec![local_mod(&file), local_mod(&imported), local_mod(&unlocked)];
    pw.save_profile(&profile, &loaded).unwrap();
    let lock = Lockfile {
        schema: 2,
        game: LockedGame {
            id: "my-game".to_owned(),
        },
        mods: vec![locked(&file, cool), locked(&imported, imported_hash)],
    };
    fsutil::write_toml(&pw.lock_file(&profile), &lock).unwrap();
    let mut deployed = toml::Table::new();
    deployed.insert(
        "mod".to_owned(),
        toml::Value::try_from(&loaded.mods[..2]).unwrap(),
    );
    deployed.insert("lock".to_owned(), toml::Value::try_from(&lock).unwrap());
    fsutil::write_toml(&dir.join("deployed.toml"), &deployed).unwrap();
    fsutil::write_toml(&dir.join("history/1.toml"), &deployed).unwrap();
    drop(pw);

    let pw = Patchwork::builder(Paths::rooted(&tmp.path().join("root")))
        .source(LocalFiles)
        .deployer(NoDeployer)
        .open()
        .unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    let ids: Vec<&str> = loaded.mods.iter().map(|m| m.id.as_str()).collect();
    let new = ContentHash::of(b"new").to_string();
    assert_eq!(
        ids,
        [cool.to_string(), imported_hash.to_string(), new.clone()]
    );
    let lock = pw.load_lock(&profile).unwrap().unwrap();
    assert_eq!(lock.schema, LOCK_SCHEMA);
    assert!(
        lock.mods
            .iter()
            .all(|m| matches!(m.artifact, ArtifactLocation::Local { .. }))
    );

    // imported files are in the store instead of the profile's folder
    assert!(!dir.join("files").exists());
    assert!(pw.store.contains(&imported_hash, ArtifactFormat::File));
    assert_eq!(pw.local_origin(&cool.to_string()).unwrap(), Some(file));
    assert_eq!(pw.local_origin(&new).unwrap(), Some(unlocked));
    assert_eq!(pw.local_origin(&imported_hash.to_string()).unwrap(), None);

    // and what was deployed moved out, with the same ids
    assert!(!dir.join("deployed.toml").exists());
    assert!(!dir.join("history").exists());
    let entries = pw.history(&profile).unwrap();
    assert_eq!(entries.len(), 1);
    pw.revert(&profile).unwrap();
    let reverted = pw.load_profile(&profile).unwrap();
    assert_eq!(reverted.mods[0].id, cool.to_string());

    // it all still locks
    pw.lock(&profile, false).await.unwrap();
}

#[tokio::test]
async fn a_game_update_shows_up_as_stale_and_an_issue() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "none");
    def.detect.version = vec![crate::game::VersionProbe::Json {
        file: crate::relpath::RelPath::new("version.json").unwrap(),
        key: "version".to_owned(),
    }];
    pw.add_game(def).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    let set_version = |v: &str| {
        fs::write(
            game.join("version.json"),
            format!(r#"{{"version": "{v}"}}"#),
        )
        .unwrap()
    };
    set_version("1.0");
    pw.set_game_path("my-game", &game).unwrap();
    let target = DeployTarget::Game("my-game".into());
    assert_eq!(
        pw.game_env(&target).unwrap().unwrap().version.as_deref(),
        Some("1.0")
    );
    let profile = pw.create_profile("my-game", "default").unwrap();
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
    assert_eq!(pw.issues(&profile).unwrap(), []);

    set_version("1.1");
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Stale);
    let issues = pw.issues(&profile).unwrap();
    assert_eq!(issues.len(), 1);
    assert!(
        issues[0].message.contains("(1.0 -> 1.1)"),
        "{}",
        issues[0].message
    );
    assert!(matches!(
        &issues[0].kind,
        crate::issue::IssueKind::GameChanged { from, to }
            if from.version.as_deref() == Some("1.0") && to.version.as_deref() == Some("1.1")
    ));
    // only the deployed profile has it
    let other = pw.create_profile("my-game", "other").unwrap();
    assert_eq!(pw.issues(&other).unwrap(), []);

    // deploying again, even with nothing to change, records the new version
    let plan = pw.plan_deploy(&profile).await.unwrap();
    assert!(plan.is_noop());
    pw.apply(&plan).unwrap();
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
    assert_eq!(pw.issues(&profile).unwrap(), []);
}

/// Mods that are folders under `dir`, which all say they're for 1.5.97.
/// Remembers which game version it was last told about.
struct ForOldGame {
    dir: PathBuf,
    saw: Arc<std::sync::Mutex<Option<String>>>,
}

#[async_trait::async_trait]
impl Source for ForOldGame {
    fn id(&self) -> &str {
        "old"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        _: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        *self.saw.lock().unwrap() = ctx.env.and_then(|e| e.version.clone());
        Ok(crate::source::ResolvedMod {
            id: id.to_owned(),
            name: id.to_owned(),
            version: Some("1.0".to_owned()),
            artifact: ArtifactLocation::Path {
                path: self.dir.join(id),
            },
            format: ArtifactFormat::Dir,
            dependencies: Vec::new(),
            game_versions: Some("1.5.97".parse().unwrap()),
        })
    }
}

#[tokio::test]
async fn mods_for_other_versions_of_the_game_are_issues() {
    let tmp = tempfile::tempdir().unwrap();
    let mods = tmp.path().join("mods");
    for (id, file) in [("plugin", "SKSE/Plugins/fix.dll"), ("textures", "a.dds")] {
        let path = mods.join(id).join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, id).unwrap();
    }
    let saw = Arc::new(std::sync::Mutex::new(None));
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(ForOldGame {
            dir: mods,
            saw: saw.clone(),
        })
        .deployer(NoDeployer)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "none");
    def.sources.insert("old".to_owned(), toml::Table::new());
    def.detect.version = vec![crate::game::VersionProbe::Json {
        file: crate::relpath::RelPath::new("version.json").unwrap(),
        key: "version".to_owned(),
    }];
    def.versions = toml::from_str(r#"files = ["**/SKSE/Plugins/*.dll"]"#).unwrap();
    pw.add_game(def).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    fs::write(game.join("version.json"), r#"{"version": "1.6.1170"}"#).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    for id in ["plugin", "textures"] {
        let spec = ModSpec {
            source: "old".to_owned(),
            id: id.to_owned(),
            version: None,
        };
        pw.add_mod(&profile, spec, None, true).await.unwrap();
    }
    assert_eq!(saw.lock().unwrap().as_deref(), Some("1.6.1170"));

    // only the mod with a DLL cares what version it's on
    let lock = pw.lock(&profile, false).await.unwrap();
    let locked = |id: &str| lock.mods.iter().find(|m| m.id == id).unwrap().clone();
    assert!(locked("plugin").game_versions.is_some());
    assert_eq!(locked("textures").game_versions, None);
    let issues = pw.issues(&profile).unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].mods, ["plugin"]);
    assert!(
        issues[0]
            .message
            .contains("is for My Game 1.5.97, but 1.6.1170 is installed"),
        "{}",
        issues[0].message
    );
    let plan = pw.plan_deploy(&profile).await.unwrap();
    assert_eq!(plan.warnings, [issues[0].message.clone()]);

    // the profile can say otherwise, for either kind of mod
    pw.set_game_versions(&profile, "plugin", Some("1.6.*".parse().unwrap()))
        .unwrap();
    pw.set_game_versions(&profile, "textures", Some("<1.6".parse().unwrap()))
        .unwrap();
    let issues = pw.issues(&profile).unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].mods, ["textures"]);
    // disabled mods aren't deployed, so they don't count
    pw.set_enabled(&profile, "textures", false).unwrap();
    assert_eq!(pw.issues(&profile).unwrap(), []);
    pw.set_game_versions(&profile, "plugin", None).unwrap();
    assert_eq!(pw.issues(&profile).unwrap().len(), 1);
}

const TOKEN: SettingSpec = SettingSpec::new("token", "Token", crate::settings::SettingKind::Secret);

/// Takes a token, and says what it is when its settings are checked.
struct Keyed;

#[async_trait::async_trait]
impl Source for Keyed {
    fn id(&self) -> &str {
        "keyed"
    }

    fn settings(&self) -> &[SettingSpec] {
        &[TOKEN]
    }

    async fn check_settings(&self, settings: Option<&toml::Table>) -> Result<Option<String>> {
        Ok(settings
            .and_then(|s| s.get("token"))
            .and_then(|t| t.as_str())
            .map(|t| format!("token {t}")))
    }

    async fn resolve(
        &self,
        _: &SourceContext<'_>,
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        unreachable!()
    }
}

#[tokio::test]
async fn source_settings_are_saved_and_kept_secret() {
    let tmp = tempfile::tempdir().unwrap();
    let open = || {
        Patchwork::builder(Paths::rooted(tmp.path()))
            .source(Keyed)
            .open()
            .unwrap()
    };
    let pw = open();
    assert_eq!(pw.configurable_sources(), vec![("keyed", "keyed")]);
    assert_eq!(pw.check_source_settings("keyed").await.unwrap(), None);

    // there's no keychain for rooted paths, so it goes in the file
    let origin = pw
        .set_source_setting("keyed", "token", Some("abc".into()))
        .unwrap();
    assert_eq!(origin, Some(Origin::File));
    assert_eq!(
        pw.check_source_settings("keyed").await.unwrap().as_deref(),
        Some("token abc")
    );
    let states = pw.source_settings("keyed").unwrap();
    assert_eq!(states[0].origin, Some(Origin::File));
    assert_eq!(states[0].value, None, "secrets aren't handed back out");

    assert!(
        pw.set_source_setting("keyed", "other", Some("x".into()))
            .is_err()
    );
    assert!(
        pw.set_source_setting("keyed", "token", Some("  ".into()))
            .is_err()
    );

    drop(pw);
    let pw = open();
    assert_eq!(
        pw.check_source_settings("keyed").await.unwrap().as_deref(),
        Some("token abc")
    );
    pw.set_source_setting("keyed", "token", None).unwrap();
    assert_eq!(pw.source_settings("keyed").unwrap()[0].origin, None);
    assert_eq!(pw.check_source_settings("keyed").await.unwrap(), None);
}

const GREETING: SettingSpec =
    SettingSpec::new("greeting", "Greeting", crate::settings::SettingKind::Text);

/// Builds `hello.txt` saying its config's `greeting`.
struct Greeter;

impl Deployer for Greeter {
    fn kind(&self) -> &str {
        "greeter"
    }

    fn settings(&self) -> &[SettingSpec] {
        &[GREETING]
    }

    fn plan(&self, _: &DeployContext<'_>, _: &[InstalledMod]) -> Result<deploy::DesiredState> {
        Ok(Default::default())
    }

    fn builds(&self, _: &DeployContext<'_>) -> bool {
        true
    }

    fn build(
        &self,
        ctx: &DeployContext<'_>,
        build: &BuildContext<'_>,
        _: &[InstalledMod],
    ) -> Result<deploy::DesiredState> {
        let greeting = ctx.config["greeting"].as_str().unwrap();
        fs::write(build.out.join("hello.txt"), greeting).unwrap();
        let dst = crate::relpath::RelPath::new("hello.txt").unwrap();
        Ok(deploy::DesiredState {
            placements: vec![deploy::Placement {
                dst: deploy::Dest::game(dst.clone()),
                src: deploy::Src::Built(dst),
                mod_name: "built".to_owned(),
            }],
            warnings: Vec::new(),
        })
    }
}

#[tokio::test]
async fn game_settings_override_the_definition() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(Greeter)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "greeter");
    def.deployer
        .config
        .insert("greeting".to_owned(), "hello".into());
    pw.add_game(def).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let hello = || fs::read_to_string(game.join("hello.txt")).unwrap();

    let settings = pw.deployer_settings("my-game").unwrap();
    assert_eq!(settings[0].default, Some("hello".into()));
    assert_eq!(settings[0].value, None);

    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(hello(), "hello");

    let mut install = pw.game_install("my-game").unwrap().unwrap();
    install.deployer.insert("rules".to_owned(), "x".into());
    let err = pw.set_game_install("my-game", install.clone()).unwrap_err();
    assert!(err.to_string().contains("no setting `rules`"), "{err}");
    install.deployer.clear();
    install.deployer.insert("greeting".to_owned(), "hi".into());
    pw.set_game_install("my-game", install.clone()).unwrap();
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Stale);
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(hello(), "hi");
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);

    // setting the path again keeps the rest
    pw.set_game_path("my-game", &game).unwrap();
    assert_eq!(
        pw.deployer_settings("my-game").unwrap()[0].value,
        Some("hi".into())
    );

    // files already placed can't change how they're linked
    install.link = Some(LinkMethod::Symlink);
    let err = pw.set_game_install("my-game", install.clone()).unwrap_err();
    assert!(err.to_string().contains("undeploy"), "{err}");
    pw.apply(
        &pw.plan_undeploy(&DeployTarget::Game("my-game".into()))
            .unwrap(),
    )
    .unwrap();
    pw.set_game_install("my-game", install).unwrap();
    assert_eq!(
        pw.plan_deploy(&profile).await.unwrap().link,
        LinkMethod::Symlink
    );
}

/// Builds the load order a game keeps in its AppData folder.
struct LoadOrder;

impl Deployer for LoadOrder {
    fn kind(&self) -> &str {
        "load-order"
    }

    fn plan(&self, _: &DeployContext<'_>, _: &[InstalledMod]) -> Result<deploy::DesiredState> {
        Ok(Default::default())
    }

    fn builds(&self, _: &DeployContext<'_>) -> bool {
        true
    }

    fn build(
        &self,
        _: &DeployContext<'_>,
        build: &BuildContext<'_>,
        _: &[InstalledMod],
    ) -> Result<deploy::DesiredState> {
        fs::write(build.out.join("plugins.txt"), "modded").unwrap();
        Ok(deploy::DesiredState {
            placements: vec![deploy::Placement {
                dst: "appdata:plugins.txt".parse().unwrap(),
                src: deploy::Src::Built(crate::relpath::RelPath::new("plugins.txt").unwrap()),
                mod_name: "built".to_owned(),
            }],
            warnings: Vec::new(),
        })
    }
}

#[tokio::test]
async fn games_deploy_to_their_other_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(LoadOrder)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "load-order");
    def.detect.steam = vec![1];
    def.roots = toml::from_str(
        r#"
        [appdata]
        path = "{localappdata}/My Game"
        allow = ["plugins.txt"]
        "#,
    )
    .unwrap();
    pw.add_game(def).unwrap();
    let game = tmp.path().join("steamapps/common/My Game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let appdata_id = RootId::new("appdata").unwrap();

    // nothing to deploy into until there's a Proton prefix
    #[cfg(target_os = "linux")]
    {
        let err = pw.plan_deploy(&profile).await.unwrap_err();
        assert!(err.to_string().contains("Wine prefix"), "{err}");

        // a Steam compatdata folder is taken to mean the prefix in it
        let compatdata = tmp.path().join("steamapps/compatdata/1");
        fs::create_dir_all(compatdata.join("pfx/drive_c/users/steamuser/AppData/Local/My Game"))
            .unwrap();
        let resolved = pw
            .resolve_roots(&DeployTarget::Game("my-game".into()))
            .unwrap()
            .unwrap();
        let (_, appdata) = resolved.iter().next().unwrap();
        assert_eq!(appdata.origin, deploy::RootOrigin::Prefix);
        assert!(
            appdata
                .path
                .as_ref()
                .unwrap()
                .ends_with("AppData/Local/My Game")
        );

        let mut install = pw.game_install("my-game").unwrap().unwrap();
        install.prefix = Some(compatdata.clone());
        let saved = pw.set_game_install("my-game", install).unwrap();
        assert_eq!(
            saved.prefix,
            Some(compatdata.canonicalize().unwrap().join("pfx"))
        );
    }

    let mut install = pw.game_install("my-game").unwrap().unwrap();
    install
        .roots
        .insert(RootId::new("elsewhere").unwrap(), tmp.path().to_owned());
    let err = pw.set_game_install("my-game", install.clone()).unwrap_err();
    assert!(
        err.to_string().contains("no folder called `elsewhere`"),
        "{err}"
    );
    install.roots.clear();
    install
        .roots
        .insert(appdata_id.clone(), tmp.path().join("missing"));
    assert!(pw.set_game_install("my-game", install.clone()).is_err());

    let appdata = tmp.path().join("appdata");
    fs::create_dir_all(&appdata).unwrap();
    fs::write(appdata.join("plugins.txt"), "original").unwrap();
    install.roots.insert(appdata_id.clone(), appdata.clone());
    install.link = Some(LinkMethod::Symlink);
    pw.set_game_install("my-game", install.clone()).unwrap();

    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    let plugins = appdata.canonicalize().unwrap().join("plugins.txt");
    assert_eq!(fs::read_to_string(&plugins).unwrap(), "modded");
    assert!(!plugins.symlink_metadata().unwrap().file_type().is_symlink());
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);

    // it can't move while something's deployed there
    install
        .roots
        .insert(appdata_id.clone(), tmp.path().to_owned());
    let err = pw.set_game_install("my-game", install).unwrap_err();
    assert!(err.to_string().contains("undeploy"), "{err}");

    pw.apply(
        &pw.plan_undeploy(&DeployTarget::Game("my-game".into()))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&plugins).unwrap(), "original");
}
