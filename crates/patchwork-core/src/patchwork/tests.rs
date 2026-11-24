use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;

use super::*;
use crate::relpath::RelPath;
use crate::set::FolderSource;
use crate::source::SoftDependency;

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
            file: None,
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

#[test]
fn updates_go_to_the_latest_version() {
    let versions = ["2.08".to_owned(), "2.07".to_owned()];
    assert_eq!(newer_version(&versions, Some("2.07")), Some("2.08"));
    assert_eq!(newer_version(&versions, Some("2.08")), None);
    // a mod at no version in particular has nothing to compare
    assert_eq!(newer_version(&versions, None), None);
    assert_eq!(newer_version(&[], Some("2.08")), None);
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
    let progress = Progress::default();
    download(&serve_once("200 OK", b"zip bytes"), &dest, "Mod", &progress)
        .await
        .unwrap();
    assert_eq!(fs::read(&dest).unwrap(), b"zip bytes");
    let seen = progress.0.lock().unwrap();
    // it starts at nothing and ends with all of it, sized from the response
    assert_eq!(seen.first(), Some(&("Mod".to_owned(), 0, Some(9))));
    assert_eq!(seen.last(), Some(&("Mod".to_owned(), 9, Some(9))));
}

/// Keeps the download progress it's given.
#[derive(Default)]
struct Progress(std::sync::Mutex<Vec<(String, u64, Option<u64>)>>);

impl crate::event::Reporter for Progress {
    fn report(&self, event: Event) {
        if let Event::Downloading {
            name,
            downloaded,
            total,
        } = event
        {
            self.0.lock().unwrap().push((name, downloaded, total));
        }
    }
}

#[tokio::test]
async fn web_pages_fail_the_download() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("download.tmp");
    let page = serve_once(
        "200 OK\r\nContent-Type: text/html; charset=utf-8",
        b"<html>a folder</html>",
    );
    let err = download(&page, &dest, "Mod", &Silent).await.unwrap_err();
    assert!(err.to_string().contains("web page"), "{err}");
    assert!(!dest.exists());
}

#[tokio::test]
async fn http_errors_fail_the_download() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("download.tmp");
    let err = download(&serve_once("404 Not Found", b""), &dest, "Mod", &Silent)
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

/// Stands in for patchwork-deployers' `rules`, which builtin games like
/// Lethal Company use: without a deployer of their kind, they're left out of
/// the registry.
struct FakeRules;

impl Deployer for FakeRules {
    fn kind(&self) -> &str {
        "rules"
    }

    fn plan(&self, _: &DeployContext<'_>, _: &[InstalledMod]) -> Result<deploy::DesiredState> {
        Ok(Default::default())
    }
}

#[test]
fn games_whose_deployer_is_missing_are_left_out() {
    let tmp = tempfile::tempdir().unwrap();
    let without = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    assert!(without.registry().game("lethal-company").is_err());
    drop(without);
    let with = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(FakeRules)
        .open()
        .unwrap();
    assert!(with.registry().game("lethal-company").is_ok());
    with.reload_registry().unwrap();
    assert!(with.registry().game("lethal-company").is_ok());
}

fn new_game(id: &str, kind: &str) -> GameDef {
    GameDef {
        schema: 0,
        id: id.to_owned(),
        name: "My Game".to_owned(),
        description: None,
        image: None,
        hidden: false,
        accent: None,
        sources: BTreeMap::new(),
        deployer: crate::game::DeployerSpec {
            kind: kind.to_owned(),
            config: toml::Table::new(),
        },
        link: Default::default(),
        detect: Default::default(),
        versions: Default::default(),
        env: Default::default(),
        roots: Default::default(),
        launch: Default::default(),
        capture: None,
        instances: None,
        config: Vec::new(),
        ext: Default::default(),
    }
}

#[test]
fn added_games_are_saved_and_usable_straight_away() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .deployer(FakeRules)
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
            soft_dependencies: Vec::new(),
            game_versions: None,
            env: Default::default(),
            kind: None,
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
            // `a/x` and `a/y` are two files of mod `a`
            name: id.split('/').next().unwrap().to_owned(),
            version: Some("1.0".to_owned()),
            artifact: ArtifactLocation::Source {
                file: "7".to_owned(),
                name: format!("{}.txt", id.replace('/', "-")),
            },
            format: ArtifactFormat::File,
            dependencies: Vec::new(),
            soft_dependencies: Vec::new(),
            game_versions: None,
            env: Default::default(),
            kind: None,
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

/// [`OwnFiles`]' mods, which can't be downloaded without a key.
struct KeyNeeded;

#[async_trait::async_trait]
impl Source for KeyNeeded {
    fn id(&self) -> &str {
        "own"
    }

    async fn resolve(
        &self,
        _: &SourceContext<'_>,
        _: &str,
        _: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        unreachable!()
    }

    async fn download(&self, _: &SourceContext<'_>, _: &str, _: &str) -> Result<Download> {
        Err(Error::blocked(
            "own needs a key",
            "Own needs a key",
            "Add one in Settings.",
            crate::fix::Fix::SourceSettings {
                source: "own".to_owned(),
                name: "Own".to_owned(),
            },
        ))
    }
}

/// A locked mod that can't be downloaded in the background because
/// something's in the way is an issue with a fix, rather than a warning.
#[tokio::test]
async fn a_blocked_download_is_an_issue() {
    let tmp = tempfile::tempdir().unwrap();
    let url = serve_once("200 OK", b"mod bytes");
    let (pw, profile, _) = own_files_profile(tmp.path(), Some(url), &["a"]).await;
    pw.lock(&profile, false).await.unwrap();
    drop(pw);
    fs::remove_dir_all(tmp.path().join("data/store")).unwrap();

    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(KeyNeeded)
        .deployer(NoDeployer)
        .open()
        .unwrap();
    let blocked = |pw: &Patchwork| {
        pw.issues(&profile)
            .unwrap()
            .into_iter()
            .filter(|i| matches!(i.kind, crate::issue::IssueKind::DownloadBlocked { .. }))
            .collect::<Vec<_>>()
    };
    // nothing's downloaded for a game that isn't set up here, so nothing's
    // in the way yet
    pw.download_locked(&profile).await.unwrap();
    assert!(blocked(&pw).is_empty());

    set_up_game(&pw, tmp.path());
    pw.download_locked(&profile).await.unwrap();
    let blocked = blocked(&pw);
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].mods, ["a"]);
    assert_eq!(blocked[0].severity, crate::issue::Severity::Error);
    assert_eq!(
        blocked[0].message,
        "`a` can't be downloaded: Own needs a key"
    );
    assert_eq!(blocked[0].fix, Some(crate::fix::Fix::Lock));
    // downloading them shows the blocker itself
    let err = pw.lock(&profile, false).await.unwrap_err();
    assert!(matches!(
        err.fix(),
        Some(crate::fix::Fix::SourceSettings { .. })
    ));
}

#[tokio::test]
async fn names_another_file_of_a_mod_after_the_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, profile, _) = own_files_profile(tmp.path(), None, &["a/main"]).await;
    let spec = |id: &str| ModSpec {
        source: "own".to_owned(),
        id: id.to_owned(),
        version: None,
    };
    let added = pw.add_mod(&profile, spec("a/preloader"), None, true).await;
    assert_eq!(added.unwrap().name, "a (a-preloader.txt)");
    // the same file twice is still refused
    let err = pw.add_mod(&profile, spec("a/main"), None, true).await;
    assert!(
        err.unwrap_err()
            .to_string()
            .contains("already has that mod")
    );
    // and a name asked for is kept to
    let err = pw
        .add_mod(&profile, spec("a/third"), Some("a".to_owned()), true)
        .await;
    assert!(err.unwrap_err().to_string().contains("named `a`"));
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
    assert_eq!(pw.link_file("https://example.com").await.unwrap(), None);
    // which file it's for is known before it downloads, with the name its
    // progress comes under
    let linked = pw.link_file("own://a").await.unwrap().unwrap();
    assert_eq!(linked.name, linked.file.key());
    let file = pw.accept_link("own://a").await.unwrap().unwrap();
    assert_eq!(file, linked.file);
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
            soft_dependencies: Vec::new(),
            game_versions: None,
            env: Default::default(),
            kind: None,
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

/// Mods whose files changed upstream since they were locked, here without
/// them in the store (as on a second PC the lockfile synced to), stop
/// locking, all in one error, until the new files are accepted, which keeps
/// their versions.
#[tokio::test]
async fn changed_files_are_kept_out_until_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mods");
    fs::create_dir_all(&dir).unwrap();
    let source = std::sync::Arc::new(Versioned {
        dir: dir.clone(),
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
    let before = pw.lock(&profile, false).await.unwrap().mods;

    for (m, id) in before.iter().zip(["a", "b"]) {
        fs::write(
            dir.join(format!("{id}-1.0.txt")),
            format!("{id} 1.0, repackaged"),
        )
        .unwrap();
        fs::remove_file(pw.store.path(&m.hash, m.format)).unwrap();
    }
    let err = pw.lock(&profile, false).await.unwrap_err();
    let Error::ArtifactChanged(changed) = &err else {
        panic!("locking should have stopped on the changed files: {err}");
    };
    assert_eq!(changed.len(), 2, "both changed mods are found at once");
    assert_eq!(
        err.to_string(),
        "`a` and `b` changed since they were locked"
    );
    let keys = vec!["versioned:a".to_owned(), "versioned:b".to_owned()];
    let names = vec!["a".to_owned(), "b".to_owned()];
    let blocker = err.blocker().unwrap();
    assert_eq!(blocker.title, "2 mods changed since they were locked");
    assert_eq!(
        blocker.fixes,
        [
            crate::fix::Fix::AcceptChanged {
                keys: keys.clone(),
                names: names.clone(),
            },
            crate::fix::Fix::UpdateMods {
                keys: keys.clone(),
                names: names.clone(),
            },
            crate::fix::Fix::RemoveMods { names },
        ]
    );
    let crate::error::ArtifactChanged { expected, got, .. } = changed[0].clone();
    assert_eq!(expected, before[0].hash);
    assert_ne!(got, before[0].hash);

    let accepted = pw.accept_changed(&profile, &keys).await.unwrap();
    assert_eq!(accepted, ["a", "b"]);
    let after = pw.lock(&profile, false).await.unwrap().mods[0].clone();
    assert_eq!(after.hash, got);
    assert_eq!(after.version.as_deref(), Some("1.0"));
}

/// A changed mod the profile pins can't be updated, and one that's only a
/// dependency can't be removed, so neither is offered.
#[test]
fn a_changed_file_only_offers_what_applies() {
    let changed = |in_profile, pinned| crate::error::ArtifactChanged {
        key: "s:a".to_owned(),
        name: "A".to_owned(),
        version: Some("1.0".to_owned()),
        expected: crate::hash::ContentHash::of(b"old"),
        got: crate::hash::ContentHash::of(b"new"),
        in_profile,
        pinned,
    };
    let labels = |c| {
        Error::ArtifactChanged(vec![c])
            .blocker()
            .unwrap()
            .fixes
            .iter()
            .map(|f| f.label())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        labels(changed(true, false)),
        ["Use the new file", "Update to latest", "Remove it"]
    );
    assert_eq!(
        labels(changed(true, true)),
        ["Use the new file", "Remove it"]
    );
    assert_eq!(
        labels(changed(false, false)),
        ["Use the new file", "Update to latest"]
    );
}

/// A changed file found while downloading a synced lockfile is an issue
/// with a fix until it's accepted, rather than just a warning.
#[tokio::test]
async fn a_changed_file_found_downloading_is_an_issue() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mods");
    fs::create_dir_all(&dir).unwrap();
    let source = std::sync::Arc::new(Versioned {
        dir: dir.clone(),
        version: std::sync::Mutex::new("1.0".to_owned()),
    });
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(SharedSource(source.clone()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    set_up_game(&pw, tmp.path());
    let profile = pw.create_profile("my-game", "default").unwrap();
    let spec = ModSpec {
        source: "versioned".to_owned(),
        id: "a".to_owned(),
        version: None,
    };
    pw.add_mod(&profile, spec, None, true).await.unwrap();
    let before = pw.lock(&profile, false).await.unwrap().mods[0].clone();
    let changed = |pw: &Patchwork| {
        pw.issues(&profile)
            .unwrap()
            .into_iter()
            .filter(|i| matches!(i.kind, crate::issue::IssueKind::ArtifactChanged { .. }))
            .collect::<Vec<_>>()
    };
    assert!(changed(&pw).is_empty());

    fs::write(dir.join("a-1.0.txt"), "a 1.0, repackaged").unwrap();
    fs::remove_file(pw.store.path(&before.hash, before.format)).unwrap();
    pw.download_locked(&profile).await.unwrap();
    let issues = changed(&pw);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].severity, crate::issue::Severity::Error);
    let Some(crate::fix::Fix::AcceptChanged { keys, .. }) = issues[0].fix.clone() else {
        panic!("{:?}", issues[0].fix);
    };
    let json = serde_json::to_value(&issues[0]).unwrap();
    assert_eq!(json["fix"]["type"], "acceptChanged");
    assert_eq!(json["fix"]["label"], "Use the new file");

    pw.accept_changed(&profile, &keys).await.unwrap();
    assert!(changed(&pw).is_empty());
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

/// Lists whatever versions it's given, newest first.
struct ListedVersions(std::sync::Arc<Mutex<Vec<String>>>);

#[async_trait::async_trait]
impl Source for ListedVersions {
    fn id(&self) -> &str {
        "listed"
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
        Ok(self.0.lock().unwrap().clone())
    }
}

#[tokio::test]
async fn a_pinned_mod_is_told_only_about_versions_not_yet_seen() {
    let tmp = tempfile::tempdir().unwrap();
    let listed = std::sync::Arc::new(Mutex::new(vec!["1.1".to_owned(), "1.0".to_owned()]));
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(ListedVersions(listed.clone()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let mut loaded = pw.load_profile(&profile).unwrap();
    loaded.mods.push(ProfileMod {
        name: "mod".to_owned(),
        source: "listed".to_owned(),
        id: "a".to_owned(),
        version: None,
        seen: None,
        enabled: true,
        priority: 0,
        game_versions: None,
        ext: Default::default(),
        replaces: Vec::new(),
        ignores: Vec::new(),
        edits: Default::default(),
        folder: None,
    });
    pw.save_profile(&profile, &loaded).unwrap();
    let lookup = pw.lookup(&loaded).unwrap();
    let update = || {
        let loaded = pw.load_profile(&profile).unwrap();
        pw.known_update(&lookup, &loaded.mods[0], None)
    };
    pw.versions(&lookup, "listed", "a").await.unwrap();

    // pinning to an older version than the latest known means it's been seen
    pw.set_version(&profile, "mod", Some("1.0".to_owned()))
        .unwrap();
    assert_eq!(
        pw.load_profile(&profile).unwrap().mods[0].seen.as_deref(),
        Some("1.1")
    );
    assert_eq!(update(), None);

    // a newer one is pointed out until it's dismissed
    listed.lock().unwrap().insert(0, "1.2".to_owned());
    pw.versions(&lookup, "listed", "a").await.unwrap();
    assert_eq!(update().as_deref(), Some("1.2"));
    pw.dismiss_update(&profile, "mod", "1.2").unwrap();
    assert_eq!(update(), None);
    listed.lock().unwrap().insert(0, "1.3".to_owned());
    pw.versions(&lookup, "listed", "a").await.unwrap();
    assert_eq!(update().as_deref(), Some("1.3"));

    // pinning to the latest leaves nothing seen, and unpinning forgets it
    pw.set_version(&profile, "mod", Some("1.3".to_owned()))
        .unwrap();
    assert_eq!(pw.load_profile(&profile).unwrap().mods[0].seen, None);
    pw.set_version(&profile, "mod", Some("1.0".to_owned()))
        .unwrap();
    pw.set_version(&profile, "mod", None).unwrap();
    assert_eq!(pw.load_profile(&profile).unwrap().mods[0].seen, None);
    assert!(pw.dismiss_update(&profile, "mod", "1.3").is_err());
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
        pw.versions(&Lookup::game("my-game"), "slow", "a"),
        pw.versions(&Lookup::game("my-game"), "slow", "a"),
    )
    .await;
    assert_eq!(a.unwrap(), ["2.0", "1.0"]);
    assert_eq!(b.unwrap(), ["2.0", "1.0"]);
    assert_eq!(fetches.load(SeqCst), 1);

    // ones fetched recently enough are used as they are
    let since = Timestamp::now() - SignedDuration::from_mins(5);
    pw.versions_since(&Lookup::game("my-game"), "slow", "a", since)
        .await
        .unwrap();
    assert_eq!(fetches.load(SeqCst), 1);
    pw.versions(&Lookup::game("my-game"), "slow", "a")
        .await
        .unwrap();
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
async fn going_back_keeps_folders_and_how_mods_are_filed() {
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
    for id in ["a", "b", "c"] {
        pw.add_mod(&profile, spec(id), None, true).await.unwrap();
    }
    let libs = pw.add_folder(&profile, "Libs", &["b".to_owned()]).unwrap();
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    let filed = || -> Vec<(String, Option<String>)> {
        let loaded = pw.load_profile(&profile).unwrap();
        loaded
            .mods
            .iter()
            .map(|m| (m.name.clone(), loaded.folder_of(m).map(|f| f.name.clone())))
            .collect()
    };
    let row = |name: &str, folder: Option<&str>| (name.to_owned(), folder.map(str::to_owned));

    // filing mods doesn't change what's deployed, and going back keeps it
    pw.add_folder(&profile, "UI", &["c".to_owned()]).unwrap();
    assert!(!pw.has_changes(&profile).unwrap());
    pw.remove_mod(&profile, "a").unwrap();
    assert!(pw.has_changes(&profile).unwrap());
    pw.revert(&profile).unwrap();
    assert!(!pw.has_changes(&profile).unwrap());
    assert_eq!(
        filed(),
        [row("a", None), row("b", Some("Libs")), row("c", Some("UI"))]
    );

    // a mod coming back is in the folder it was in, which comes back too,
    // renamed if its name has been taken since
    pw.remove_folder(&profile, "Libs").unwrap();
    pw.remove_mod(&profile, "b").unwrap();
    pw.rename_folder(&profile, "UI", "libs").unwrap();
    pw.revert(&profile).unwrap();
    assert!(!pw.has_changes(&profile).unwrap());
    let loaded = pw.load_profile(&profile).unwrap();
    let back = loaded.folders.iter().find(|f| f.id == libs.id).unwrap();
    assert_eq!(back.name, "Libs 2");
    assert_eq!(
        filed(),
        [
            row("a", None),
            row("b", Some("Libs 2")),
            row("c", Some("libs"))
        ]
    );

    // filing that moved a mod can't be kept without moving it again, so
    // it's filed as it was
    pw.set_mod_folder(&profile, "a", Some("libs")).unwrap();
    assert!(pw.has_changes(&profile).unwrap());
    pw.revert(&profile).unwrap();
    assert!(!pw.has_changes(&profile).unwrap());
    assert_eq!(
        filed(),
        [
            row("a", None),
            row("b", Some("Libs 2")),
            row("c", Some("libs"))
        ]
    );
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
    assert_eq!(
        err.fix(),
        Some(crate::fix::Fix::Undeploy {
            game: "my-game".into(),
            instance: None
        }),
        "{err}"
    );

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

#[test]
fn remembers_the_last_profile_of_each_game() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("game-a", "none")).unwrap();
    pw.add_game(new_game("game-b", "none")).unwrap();
    pw.create_profile("game-a", "first").unwrap();
    let a = pw.create_profile("game-a", "second").unwrap();
    let b = pw.create_profile("game-b", "only").unwrap();

    pw.use_profile(&a).unwrap();
    pw.use_profile(&b).unwrap();
    assert_eq!(pw.current_profile().unwrap(), Some(b.clone()));
    let last = pw.last_profiles().unwrap();
    assert_eq!(last.get("game-a"), Some(&a));
    assert_eq!(last.get("game-b"), Some(&b));

    // removing a game's last profile forgets it, even when it isn't current
    pw.remove_profile(&a).unwrap();
    let last = pw.last_profiles().unwrap();
    assert_eq!(last.get("game-a"), None);
    assert_eq!(last.get("game-b"), Some(&b));
    assert_eq!(pw.current_profile().unwrap(), Some(b));
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
            soft_dependencies: Vec::new(),
            game_versions: None,
            env: Default::default(),
            kind: None,
        })
    }
}

/// patchwork with the `local` source, rooted at `root`.
/// Sets up `my-game` as installed in a folder under `root`.
fn set_up_game(pw: &Patchwork, root: &Path) {
    let game = root.join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
}

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
        priority: 0,
        game_versions: None,
        ext: Default::default(),
        replaces: Vec::new(),
        seen: None,
        ignores: Vec::new(),
        edits: Default::default(),
        folder: None,
    });
    loaded.hidden_tags = vec!["library".to_owned()];
    pw.save_profile(&profile, &loaded).unwrap();
    let lock = Lockfile {
        schema: LOCK_SCHEMA,
        game: LockedGame {
            env: Default::default(),
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

#[test]
fn priorities_change_the_load_order_but_not_the_list() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let mut loaded = pw.load_profile(&profile).unwrap();
    for name in ["a", "b", "c"] {
        loaded.mods.push(ProfileMod {
            name: name.to_owned(),
            source: "local".to_owned(),
            id: name.to_owned(),
            version: None,
            enabled: true,
            priority: 0,
            game_versions: None,
            ext: Default::default(),
            replaces: Vec::new(),
            seen: None,
            ignores: Vec::new(),
            edits: Default::default(),
            folder: None,
        });
    }
    pw.save_profile(&profile, &loaded).unwrap();

    pw.set_priority(&profile, "a", 3).unwrap();
    pw.set_priority(&profile, "c", -1).unwrap();
    assert!(pw.set_priority(&profile, "missing", 1).is_err());
    let loaded = pw.load_profile(&profile).unwrap();
    let names = |mods: Vec<&ProfileMod>| -> Vec<String> {
        mods.into_iter().map(|m| m.name.clone()).collect()
    };
    assert_eq!(names(loaded.mods.iter().collect()), ["a", "b", "c"]);
    assert_eq!(
        names(crate::order::load_order(&loaded.mods)),
        ["c", "b", "a"]
    );
    // only priorities that aren't 0 are written down
    let text = fs::read_to_string(pw.profile_file(&profile)).unwrap();
    assert_eq!(text.matches("priority").count(), 2, "{text}");

    // and back to 0 leaves everything where it was
    pw.set_priority(&profile, "a", 0).unwrap();
    pw.set_priority(&profile, "c", 0).unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    assert_eq!(
        names(crate::order::load_order(&loaded.mods)),
        ["a", "b", "c"]
    );
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
        assert_eq!(err.fix(), Some(crate::fix::Fix::Sync));
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
        priority: 0,
        game_versions: None,
        ext: Default::default(),
        replaces: Vec::new(),
        seen: None,
        ignores: Vec::new(),
        edits: Default::default(),
        folder: None,
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
        soft_dependencies: Vec::new(),
        requires: BTreeMap::new(),
        game_versions: None,
        env: Default::default(),
        kind: None,
        replaced: Default::default(),
    };
    let mut loaded = pw.load_profile(&profile).unwrap();
    loaded.mods = vec![local_mod(&file), local_mod(&imported), local_mod(&unlocked)];
    pw.save_profile(&profile, &loaded).unwrap();
    let lock = Lockfile {
        schema: 2,
        game: LockedGame {
            env: Default::default(),
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
            soft_dependencies: Vec::new(),
            game_versions: Some("1.5.97".parse().unwrap()),
            env: Default::default(),
            kind: None,
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
    assert!(
        matches!(err.fix(), Some(crate::fix::Fix::Undeploy { .. })),
        "{err}"
    );
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
    assert!(
        matches!(err.fix(), Some(crate::fix::Fix::Undeploy { .. })),
        "{err}"
    );

    pw.apply(
        &pw.plan_undeploy(&DeployTarget::Game("my-game".into()))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&plugins).unwrap(), "original");
}

#[tokio::test]
async fn synced_profiles_are_taken_as_they_are_and_keep_local_changes() {
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
    let mut doc = a.start_sync(&profile).unwrap();
    assert!(a.unsynced_changes(&profile).unwrap().is_empty());

    let b = with_local_files(&tmp.path().join("b"));
    let mut changed = b.watch_profiles();
    let applied = b.apply_synced(&doc, true).unwrap();
    assert!(applied.created && applied.changed);
    assert_eq!(applied.profile, profile);
    assert_eq!(
        changed.try_recv().unwrap(),
        ProfileChange {
            profile: profile.clone(),
            synced: true
        }
    );
    // nothing was resolved: the lockfile is a's
    let lock = b.load_lock(&profile).unwrap().unwrap();
    assert_eq!(lock.mods[0].hash, ContentHash::of(b"v1"));

    // the local mod's file has to come from a
    let missing = b.missing_local_files(&profile).unwrap();
    assert_eq!(missing.len(), 1);
    assert!(a.missing_local_files(&profile).unwrap().is_empty());
    let on_another_pc = |pw: &Patchwork| {
        pw.issues(&profile)
            .unwrap()
            .into_iter()
            .any(|i| i.kind == crate::issue::IssueKind::OnAnotherPc)
    };
    // b hasn't the game, so the file isn't wanted there yet
    assert!(!on_another_pc(&b));
    // setting it up has sync catch up
    set_up_game(&b, &tmp.path().join("b"));
    assert_eq!(
        changed.try_recv().unwrap(),
        ProfileChange {
            profile: profile.clone(),
            synced: false
        }
    );
    assert!(on_another_pc(&b));
    assert!(!on_another_pc(&a));
    let from_a = a.stored_file(&missing[0]).unwrap();
    let wrong = tmp.path().join("wrong.pak");
    fs::write(&wrong, "v2").unwrap();
    let err = b.add_synced_file(&missing[0], &wrong).unwrap_err();
    assert!(err.to_string().contains("isn't the one"), "{err}");
    b.add_synced_file(&missing[0], &from_a).unwrap();
    assert!(b.missing_local_files(&profile).unwrap().is_empty());
    assert!(!on_another_pc(&b));
    b.download_locked(&profile).await.unwrap();

    // a change on each side: b's isn't published yet when a's arrives
    b.set_enabled(&profile, "Cool", false).unwrap();
    assert_eq!(
        changed.try_recv().unwrap(),
        ProfileChange {
            profile: profile.clone(),
            synced: false
        }
    );
    a.set_hidden_tags(&profile, &["library".to_owned()])
        .unwrap();
    doc.extend(a.unsynced_changes(&profile).unwrap());
    let applied = b.apply_synced(&doc, true).unwrap();
    assert_eq!(applied.unpublished.len(), 1);
    let loaded = b.load_profile(&profile).unwrap();
    assert_eq!(loaded.hidden_tags, ["library"]);
    assert!(!loaded.mods[0].enabled);
    // until it's been published, it's still to be
    assert_eq!(b.unsynced_changes(&profile).unwrap(), applied.unpublished);
    doc.extend(applied.unpublished);
    assert!(b.apply_synced(&doc, true).unwrap().unpublished.is_empty());

    // a follower's changes stay, unless the sharer changes the same thing
    b.set_enabled(&profile, "Cool", true).unwrap();
    b.set_hidden_tags(&profile, &["mine".to_owned()]).unwrap();
    a.apply_synced(&doc, true).unwrap();
    a.set_hidden_tags(&profile, &["theirs".to_owned()]).unwrap();
    doc.extend(a.unsynced_changes(&profile).unwrap());
    let applied = b.apply_synced(&doc, false).unwrap();
    assert_eq!(applied.unpublished.len(), 1);
    let loaded = b.load_profile(&profile).unwrap();
    assert!(loaded.mods[0].enabled);
    assert_eq!(loaded.hidden_tags, ["theirs"]);
}

#[tokio::test]
async fn a_follower_keeps_the_mods_it_adds_and_takes_the_rest() {
    let tmp = tempfile::tempdir().unwrap();
    let mods = tmp.path().join("mods");
    fs::create_dir_all(&mods).unwrap();
    for name in ["Shared.pak", "Mine.pak", "Later.pak"] {
        fs::write(mods.join(name), name).unwrap();
    }
    let a = with_local_files(&tmp.path().join("a"));
    let profile = a.create_profile("my-game", "default").unwrap();
    a.add_mod(&profile, local_spec(&mods.join("Shared.pak")), None, true)
        .await
        .unwrap();
    a.lock(&profile, false).await.unwrap();
    let mut doc = a.start_sync(&profile).unwrap();

    let b = with_local_files(&tmp.path().join("b"));
    b.apply_synced(&doc, false).unwrap();
    for m in b.missing_local_files(&profile).unwrap() {
        b.add_synced_file(&m, &a.stored_file(&m).unwrap()).unwrap();
    }
    assert!(b.unshared_mods(&profile).unwrap().is_empty());
    b.add_mod(&profile, local_spec(&mods.join("Mine.pak")), None, true)
        .await
        .unwrap();
    b.lock(&profile, false).await.unwrap();
    assert_eq!(b.unshared_mods(&profile).unwrap(), ["Mine"]);

    // the sharer adds one too, which moves the load order
    a.add_mod(&profile, local_spec(&mods.join("Later.pak")), None, true)
        .await
        .unwrap();
    a.lock(&profile, false).await.unwrap();
    doc.extend(a.unsynced_changes(&profile).unwrap());
    b.apply_synced(&doc, false).unwrap();
    for m in b.missing_local_files(&profile).unwrap() {
        b.add_synced_file(&m, &a.stored_file(&m).unwrap()).unwrap();
    }
    let names: Vec<String> = b
        .load_profile(&profile)
        .unwrap()
        .mods
        .into_iter()
        .map(|m| m.name)
        .collect();
    assert_eq!(names, ["Shared", "Later", "Mine"]);
    assert_eq!(b.unshared_mods(&profile).unwrap(), ["Mine"]);
    assert_eq!(b.load_lock(&profile).unwrap().unwrap().mods.len(), 3);
}

/// Places mods' files in the game's folder as they are.
struct AsPacked;

impl Deployer for AsPacked {
    fn kind(&self) -> &str {
        "as-packed"
    }

    fn plan(&self, _: &DeployContext<'_>, mods: &[InstalledMod]) -> Result<deploy::DesiredState> {
        Ok(deploy::DesiredState {
            placements: mods
                .iter()
                .flat_map(|m| {
                    m.files.iter().map(|f| deploy::Placement {
                        dst: deploy::Dest::game(f.rel.clone()),
                        src: m.src(f),
                        mod_name: m.name.clone(),
                    })
                })
                .collect(),
            warnings: Vec::new(),
        })
    }
}

/// Mods with a `looks/` folder install just one of the folders in it: the
/// look picked, or `plain`. They also have a shine to set, from 0 to 10.
struct Looks;

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Look {
    look: Option<String>,
    shine: Option<i64>,
}

impl Looks {
    fn picked(
        ctx: &crate::options::ModContext<'_>,
        answers: Option<&[crate::options::Answer]>,
    ) -> String {
        match answers {
            Some(answers) => answers[0].items[0].clone(),
            None => ctx
                .state::<Look>()
                .unwrap()
                .look
                .unwrap_or("plain".to_owned()),
        }
    }

    fn shine(
        ctx: &crate::options::ModContext<'_>,
        answers: Option<&[crate::options::Answer]>,
    ) -> i64 {
        let set = answers
            .into_iter()
            .flatten()
            .filter(|a| a.group == "extras")
            .find_map(|a| a.values.get("shine"));
        match set {
            Some(crate::options::Value::Int(shine)) if (0..=10).contains(shine) => *shine,
            _ => ctx.state::<Look>().unwrap().shine.unwrap_or(3),
        }
    }
}

impl crate::options::ModTransform for Looks {
    fn applies(&self, _: &crate::options::ModContext<'_>, m: &crate::options::ModPeek<'_>) -> bool {
        m.has_near_top("looks/plain/look.txt")
    }

    fn transform(
        &self,
        ctx: &crate::options::ModContext<'_>,
        m: &InstalledMod,
    ) -> Result<crate::options::Transformed> {
        let prefix = format!("looks/{}/", Self::picked(ctx, None));
        let folder = ctx.config()["folder"].as_str().unwrap();
        Ok(crate::options::Transformed {
            files: m
                .files
                .iter()
                .filter_map(|f| {
                    let rest = f.rel.as_str().split_once(&prefix)?.1;
                    Some((
                        RelPath::new(&format!("{folder}/{rest}")).ok()?,
                        f.rel.clone(),
                    ))
                })
                // and one that isn't there, which is left out
                .chain([(RelPath::new("x").unwrap(), RelPath::new("nope").unwrap())])
                .collect(),
            warnings: vec![format!("`{}` looks {}", m.name, Self::picked(ctx, None))],
        })
    }
}

impl crate::options::ModOptions for Looks {
    fn title(&self) -> &str {
        "Looks"
    }

    fn applies(&self, _: &crate::options::ModContext<'_>, m: &crate::options::ModPeek<'_>) -> bool {
        m.has_near_top("looks/plain/look.txt")
    }

    fn form(
        &self,
        ctx: &crate::options::ModContext<'_>,
        m: &InstalledMod,
        answers: Option<&[crate::options::Answer]>,
    ) -> Result<crate::options::Form> {
        use crate::options::{
            Answer, Field, FieldKind, Form, Group, GroupKind, Item, ItemState, Page, Pick, Value,
        };
        let picked = Self::picked(ctx, answers);
        let image = m.files.iter().find(|f| f.rel.file_name() == "fancy.png");
        Ok(Form {
            title: "Pretty Things".to_owned(),
            pages: vec![Page {
                title: "Main".to_owned(),
                groups: vec![
                    Group {
                        id: "look".to_owned(),
                        title: "Look".to_owned(),
                        kind: GroupKind::Pick {
                            pick: Pick::ExactlyOne,
                            items: ["plain", "fancy"]
                                .map(|look| Item {
                                    id: look.to_owned(),
                                    title: look.to_owned(),
                                    description: None,
                                    image: (look == "fancy").then(|| image.unwrap().rel.clone()),
                                    state: ItemState::Optional,
                                    picked: picked == look,
                                })
                                .into(),
                        },
                        defaulted: !ctx.saved(),
                    },
                    Group {
                        id: "extras".to_owned(),
                        title: "Extras".to_owned(),
                        kind: GroupKind::Fields {
                            fields: vec![Field {
                                id: "shine".to_owned(),
                                title: "Shine".to_owned(),
                                description: None,
                                kind: FieldKind::Int {
                                    min: Some(0),
                                    max: Some(10),
                                    step: None,
                                },
                                value: Value::Int(Self::shine(ctx, answers)),
                                default: Some(Value::Int(3)),
                            }],
                        },
                        defaulted: !ctx.saved(),
                    },
                ],
            }],
            summary: None,
            warnings: Vec::new(),
            answers: vec![
                Answer::picks("look", vec![picked]),
                Answer::values(
                    "extras",
                    [("shine".to_owned(), Value::Int(Self::shine(ctx, answers)))].into(),
                ),
            ],
        })
    }

    fn save(
        &self,
        ctx: &crate::options::ModContext<'_>,
        _: &InstalledMod,
        answers: &[crate::options::Answer],
    ) -> Result<toml::Table> {
        let look = Look {
            look: Some(Self::picked(ctx, Some(answers))),
            shine: Some(Self::shine(ctx, Some(answers))),
        };
        Ok(toml::Table::try_from(&look).unwrap())
    }
}

struct LooksExtension;

impl Extension for LooksExtension {
    fn id(&self) -> &'static str {
        "looks"
    }

    fn register(&self, ext: &mut crate::ext::Registrar<'_>) {
        ext.mod_transform("pick", Looks).mod_options("pick", Looks);
    }
}

#[tokio::test]
async fn mod_options_change_what_mods_install() {
    use crate::options::{Answer, Value};

    let tmp = tempfile::tempdir().unwrap();
    let packed = tmp.path().join("mods/Pretty");
    for (file, text) in [
        ("Pretty/looks/plain/look.txt", "plain"),
        ("Pretty/looks/fancy/look.txt", "fancy"),
        ("Pretty/fancy.png", "png"),
    ] {
        let path = packed.join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    let pw = Patchwork::builder(Paths::rooted(&tmp.path().join("pw")))
        .source(LocalFiles)
        .deployer(AsPacked)
        .extension(LooksExtension)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "as-packed");
    let mut looks = toml::Table::new();
    looks.insert(
        "pick".to_owned(),
        toml::Value::Table(toml::from_str(r#"folder = "Data""#).unwrap()),
    );
    def.ext.insert("looks".to_owned(), looks);
    pw.add_game(def).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    pw.add_mod(&profile, local_spec(&packed), None, true)
        .await
        .unwrap();
    let look = game.canonicalize().unwrap().join("Data/look.txt");

    // until something's picked, it gets the defaults
    let plan = pw.plan_deploy(&profile).await.unwrap();
    assert!(
        plan.warnings.iter().any(|w| w == "`Pretty` looks plain"),
        "{:?}",
        plan.warnings
    );
    assert!(
        plan.warnings.iter().any(|w| w.contains("hasn't got nope")),
        "{:?}",
        plan.warnings
    );
    pw.apply(&plan).unwrap();
    assert_eq!(fs::read_to_string(&look).unwrap(), "plain");
    let found = pw.mods_with_options(&profile).unwrap();
    assert_eq!(
        found["Pretty"],
        [crate::ModOptionsInfo {
            id: "looks.pick".to_owned(),
            title: "Looks".to_owned(),
            saved: false,
        }]
    );

    let view = pw.mod_options(&profile, "Pretty", None, None).unwrap();
    assert_eq!(view.id, "looks.pick");
    assert_eq!(view.form.title, "Pretty Things");
    assert!(!view.saved);
    assert!(view.form.pages[0].groups[0].defaulted);
    assert!(view.form.pages[0].groups[0].items()[0].picked);
    assert_eq!(view.images.len(), 1);
    assert!(view.images["Pretty/fancy.png"].ends_with("fancy.png"));

    // trying answers out doesn't save them
    let fancy = [Answer::picks("look", vec!["fancy".to_owned()])];
    let view = pw
        .mod_options(&profile, "Pretty", Some("looks.pick"), Some(&fancy))
        .unwrap();
    assert!(view.form.pages[0].groups[0].items()[1].picked);
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);

    pw.set_mod_options(&profile, "Pretty", None, Some(&view.form.answers))
        .unwrap();
    assert!(pw.mods_with_options(&profile).unwrap()["Pretty"][0].saved);
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Stale);
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(fs::read_to_string(&look).unwrap(), "fancy");

    // values are checked against their fields before the extension sees
    // them, with whole numbers taken for numbers and so on
    let mut answers = view.form.answers.clone();
    answers[1].values.insert("shine".to_owned(), Value::Int(11));
    let err = pw
        .set_mod_options(&profile, "Pretty", None, Some(&answers))
        .unwrap_err();
    assert!(err.to_string().contains("11 is more than 10"), "{err}");
    // and shown as warnings while trying them out
    let tried = pw
        .mod_options(&profile, "Pretty", None, Some(&answers))
        .unwrap();
    assert!(
        tried
            .form
            .warnings
            .iter()
            .any(|w| w.contains("11 is more than 10")),
        "{:?}",
        tried.form.warnings
    );
    answers[1]
        .values
        .insert("shine".to_owned(), Value::Text("lots".to_owned()));
    let err = pw
        .set_mod_options(&profile, "Pretty", None, Some(&answers))
        .unwrap_err();
    assert!(
        err.to_string().contains("a whole number from 0 to 10"),
        "{err}"
    );
    answers[1].values.insert("nope".to_owned(), Value::Int(1));
    answers[1].values.insert("shine".to_owned(), Value::Int(7));
    let err = pw
        .set_mod_options(&profile, "Pretty", None, Some(&answers))
        .unwrap_err();
    assert!(err.to_string().contains("no setting `nope`"), "{err}");
    answers[1].values.remove("nope");
    pw.set_mod_options(&profile, "Pretty", None, Some(&answers))
        .unwrap();
    let view = pw.mod_options(&profile, "Pretty", None, None).unwrap();
    assert_eq!(
        view.form.pages[0].groups[1].fields()[0].value,
        Value::Int(7)
    );
    assert!(view.form.pages[0].groups[0].items()[1].picked);

    // forgetting them goes back to the defaults
    pw.set_mod_options(&profile, "Pretty", None, None).unwrap();
    assert!(
        !pw.mod_options(&profile, "Pretty", None, None)
            .unwrap()
            .saved
    );
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(fs::read_to_string(&look).unwrap(), "plain");

    // and nothing's done for games that don't turn it on
    let err = pw
        .mod_options(&profile, "Pretty", Some("looks.other"), None)
        .unwrap_err();
    assert!(err.to_string().contains("`looks.other`"), "{err}");
}

#[tokio::test]
async fn what_tools_write_is_captured_as_a_mod() {
    let tmp = tempfile::tempdir().unwrap();
    let packed = tmp.path().join("mods/Body");
    fs::create_dir_all(packed.join("Data/meshes")).unwrap();
    fs::write(packed.join("Data/meshes/body.nif"), "base body").unwrap();
    let pw = Patchwork::builder(Paths::rooted(&tmp.path().join("pw")))
        .source(LocalFiles)
        .deployer(AsPacked)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "as-packed");
    def.capture = Some(crate::game::CaptureDef {
        folders: vec![RelPath::new("Data").unwrap()],
        ignore: vec!["**/*.log".to_owned().try_into().unwrap()],
    });
    pw.add_game(def).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(game.join("Data")).unwrap();
    fs::write(game.join("Data/Game.esm"), "the game's own").unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let game = game.canonicalize().unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    pw.add_mod(&profile, local_spec(&packed), None, true)
        .await
        .unwrap();

    // nothing to capture from until it's deployed
    assert!(pw.tool_output(&profile).is_err());
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert!(pw.tool_output(&profile).unwrap().is_empty());

    // a tool writes a plugin and rebuilds the body, and its log is left be
    fs::write(game.join("Data/Patch.esp"), "patch").unwrap();
    fs::remove_file(game.join("Data/meshes/body.nif")).unwrap();
    fs::write(game.join("Data/meshes/body.nif"), "built body").unwrap();
    fs::write(game.join("Data/tool.log"), "log").unwrap();
    // deploying again before capturing doesn't lose track of them
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    let output = pw.tool_output(&profile).unwrap();
    assert_eq!(output.files.len(), 1, "{output:?}");
    assert_eq!(output.files[0].path.as_str(), "Data/Patch.esp");
    // ... though the body went back to the mod's
    assert_eq!(
        fs::read_to_string(game.join("Data/meshes/body.nif")).unwrap(),
        "base body"
    );
    fs::remove_file(game.join("Data/meshes/body.nif")).unwrap();
    fs::write(game.join("Data/meshes/body.nif"), "built body").unwrap();

    let captured = pw.capture(&profile, "Tool Output").await.unwrap();
    assert!(captured.added);
    assert_eq!(captured.output.files.len(), 2);
    assert_eq!(
        captured.output.files[1].replaces.as_deref(),
        Some("Body"),
        "{:?}",
        captured.output
    );
    assert!(!game.join("Data/Patch.esp").exists());
    let loaded = pw.load_profile(&profile).unwrap();
    assert_eq!(loaded.mods.last().unwrap().name, "Tool Output");

    // deploying puts them back, from the mod, which wins over the body's
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(
        fs::read_to_string(game.join("Data/Patch.esp")).unwrap(),
        "patch"
    );
    assert_eq!(
        fs::read_to_string(game.join("Data/meshes/body.nif")).unwrap(),
        "built body"
    );
    assert!(pw.tool_output(&profile).unwrap().is_empty());

    // running the tool again adds to the same mod
    fs::write(game.join("Data/Another.esp"), "another").unwrap();
    let captured = pw.capture(&profile, "Tool Output").await.unwrap();
    assert!(!captured.added);
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(
        fs::read_to_string(game.join("Data/Another.esp")).unwrap(),
        "another"
    );
    assert_eq!(pw.load_profile(&profile).unwrap().mods.len(), 2);

    // a mod that wasn't captured isn't added to
    fs::write(game.join("Data/Third.esp"), "third").unwrap();
    assert!(pw.capture(&profile, "Body").await.is_err());
    assert!(game.join("Data/Third.esp").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn tools_run_through_patchwork_have_what_they_write_captured() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let packed = tmp.path().join("mods/Body");
    fs::create_dir_all(packed.join("Data/meshes")).unwrap();
    fs::write(packed.join("Data/meshes/body.nif"), "base body").unwrap();
    let pw = Patchwork::builder(Paths::rooted(&tmp.path().join("pw")))
        .source(LocalFiles)
        .deployer(AsPacked)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "as-packed");
    def.link = crate::game::LinkMethod::Hardlink;
    def.capture = Some(crate::game::CaptureDef {
        folders: vec![RelPath::new("Data").unwrap()],
        ignore: Vec::new(),
    });
    pw.add_game(def).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(game.join("Data")).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let game = game.canonicalize().unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    pw.add_mod(&profile, local_spec(&packed), None, true)
        .await
        .unwrap();
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    // there before the tool ran, so not its
    fs::write(game.join("Data/Stray.esp"), "stray").unwrap();

    // writes a plugin, and notes whether the body was read-only meanwhile
    let tool = tmp.path().join("tool.sh");
    fs::write(
        &tool,
        "#!/bin/sh\necho patch > \"$1/Data/Patch.esp\"\nls -l \"$1/Data/meshes/body.nif\" > \"$2\"\nexit $3\n",
    )
    .unwrap();
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
    let mode = tmp.path().join("mode");
    let args = |code: &str| {
        vec![
            game.to_string_lossy().into_owned(),
            mode.to_string_lossy().into_owned(),
            code.to_owned(),
        ]
    };

    // a tool that fails leaves what it wrote be
    let failing = pw.plan_tool(&profile, &tool, args("3")).unwrap();
    let ran = pw
        .run_tool(&profile, &failing, "Tool Output")
        .await
        .unwrap();
    assert!(!ran.success);
    assert!(ran.captured.is_none());
    assert!(game.join("Data/Patch.esp").exists());
    fs::remove_file(game.join("Data/Patch.esp")).unwrap();

    let launch = pw.plan_tool(&profile, &tool, args("0")).unwrap();
    // the name's checked before it runs
    assert!(pw.run_tool(&profile, &launch, "Body").await.is_err());
    let ran = pw.run_tool(&profile, &launch, "Tool Output").await.unwrap();
    assert!(ran.success, "{}", ran.status);
    assert_eq!(ran.protected, 1);
    assert!(
        fs::read_to_string(&mode).unwrap().starts_with("-r--r--r--"),
        "{}",
        fs::read_to_string(&mode).unwrap()
    );
    let captured = ran.captured.unwrap();
    assert!(captured.added);
    let paths: Vec<&str> = captured
        .output
        .files
        .iter()
        .map(|f| f.path.as_str())
        .collect();
    assert_eq!(paths, ["Data/Patch.esp"]);
    assert!(game.join("Data/Stray.esp").exists());
    // and the store's copy is writable again
    let body = game.join("Data/meshes/body.nif");
    assert!(!body.metadata().unwrap().permissions().readonly());
}

/// Resolves every mod to `<dir>/<loader>.jar` (which has to be there), at
/// a version naming the game version and loader it was told about. Every
/// mod says it's only for fabric.
struct ByLoader(PathBuf);

#[async_trait::async_trait]
impl Source for ByLoader {
    fn id(&self) -> &str {
        "by-loader"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        _: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        let env = ctx.env.expect("the profile picks its env");
        let loader = env.get("loader").unwrap();
        Ok(crate::source::ResolvedMod {
            id: id.to_owned(),
            name: id.to_owned(),
            version: Some(format!("{}+{loader}", env.get("version").unwrap())),
            artifact: ArtifactLocation::Path {
                path: self.0.join(format!("{loader}.jar")),
            },
            format: ArtifactFormat::File,
            dependencies: Vec::new(),
            soft_dependencies: Vec::new(),
            game_versions: None,
            env: compat::EnvSupport::default().with("loader", ["fabric"]),
            kind: None,
        })
    }
}

#[tokio::test]
async fn mods_are_locked_for_what_the_profile_picks_and_relocked_when_it_changes() {
    let tmp = tempfile::tempdir().unwrap();
    let jars = tmp.path().join("jars");
    fs::create_dir_all(&jars).unwrap();
    for loader in ["fabric", "quilt"] {
        fs::write(jars.join(format!("{loader}.jar")), loader).unwrap();
    }
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(ByLoader(jars))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    let mut def = new_game("blocks", "none");
    def.env = toml::from_str(
        r#"
        version = { name = "Game version" }
        loader = { name = "Mod loader", choices = ["fabric", "quilt"], default = "fabric" }
        "#,
    )
    .unwrap();
    pw.add_game(def).unwrap();
    let profile = pw.create_profile("blocks", "default").unwrap();
    pw.set_env(&profile, "version", Some("1.21.1")).unwrap();

    // no install, but what the profile picks is enough to go on
    let env = pw.profile_env(&profile).unwrap().unwrap();
    assert_eq!(env.version.as_deref(), Some("1.21.1"));
    assert_eq!(env.get("loader"), Some("fabric"));

    let spec = ModSpec {
        source: "by-loader".to_owned(),
        id: "a".to_owned(),
        version: None,
    };
    pw.add_mod(&profile, spec, None, true).await.unwrap();
    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(lock.mods[0].version.as_deref(), Some("1.21.1+fabric"));
    assert_eq!(
        lock.game.env.get("loader").map(String::as_str),
        Some("fabric")
    );
    assert_eq!(pw.issues(&profile).unwrap(), []);

    // picks are checked against the game's choices, ignoring case
    pw.set_env(&profile, "loader", Some("Quilt")).unwrap();
    assert_eq!(pw.load_profile(&profile).unwrap().env["loader"], "quilt");
    assert!(pw.set_env(&profile, "loader", Some("forge")).is_err());
    assert!(pw.set_env(&profile, "shaders", Some("iris")).is_err());

    // a changed pick relocks without being asked to update
    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(lock.mods[0].version.as_deref(), Some("1.21.1+quilt"));
    let issues = pw.issues(&profile).unwrap();
    assert_eq!(issues.len(), 1);
    assert!(
        issues[0]
            .message
            .contains("is for Mod loader fabric, but the profile uses quilt"),
        "{}",
        issues[0].message
    );

    // and going back to the default is a change too
    pw.set_env(&profile, "loader", None).unwrap();
    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(lock.mods[0].version.as_deref(), Some("1.21.1+fabric"));
    assert_eq!(pw.issues(&profile).unwrap(), []);
}

#[test]
fn env_defaults_have_to_be_among_the_choices() {
    let bad: std::result::Result<crate::game::EnvDefs, _> =
        toml::from_str(r#"loader = { name = "Loader", choices = ["fabric"], default = "forge" }"#);
    assert!(
        bad.unwrap_err()
            .to_string()
            .contains("isn't one of its choices")
    );
    let bad: std::result::Result<crate::game::EnvDefs, _> =
        toml::from_str(r#"Loader = { name = "Loader" }"#);
    assert!(bad.unwrap_err().to_string().contains("valid env id"));
}

/// Builds `options.txt` into the game's folder.
struct Options;

impl Deployer for Options {
    fn kind(&self) -> &str {
        "options"
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
        fs::write(build.out.join("options.txt"), "modded").unwrap();
        let dst = crate::relpath::RelPath::new("options.txt").unwrap();
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
async fn profiles_with_instances_are_each_deployed_to_their_own() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(Options)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "options");
    def.instances = Some(crate::game::InstancesDef { default: true });
    pw.add_game(def).unwrap();
    let a = pw.create_profile("my-game", "a").unwrap();
    let b = pw.create_profile("my-game", "b").unwrap();

    // no game folder needed, and deploying one leaves the other be
    pw.apply(&pw.plan_deploy(&a).await.unwrap()).unwrap();
    pw.apply(&pw.plan_deploy(&b).await.unwrap()).unwrap();
    for profile in [&a, &b] {
        let options = pw.instance_dir(profile).join("options.txt");
        assert_eq!(fs::read_to_string(options).unwrap(), "modded");
        assert_eq!(pw.deploy_status(profile).unwrap(), DeployStatus::Deployed);
    }

    // switching to the game's folder needs it undeployed first
    let err = pw.set_instance(&a, Some(false)).unwrap_err();
    assert_eq!(
        err.fix().unwrap().command(None),
        "patchwork --profile my-game/a undeploy",
        "{err}"
    );
    let a_loaded = pw.load_profile(&a).unwrap();
    let target = pw.deploy_target(&a_loaded).unwrap();
    pw.apply(&pw.plan_undeploy(&target).unwrap()).unwrap();
    assert!(!pw.instance_dir(&a).join("options.txt").exists());
    pw.set_instance(&a, Some(false)).unwrap();
    assert_eq!(
        pw.deploy_target(&a_loaded).unwrap(),
        DeployTarget::Game("my-game".into())
    );
    // which isn't set up
    assert!(pw.plan_deploy(&a).await.is_err());

    // copies use an instance if the original does, but not its files
    let c = pw.duplicate_profile(&a, "c").unwrap();
    assert!(!pw.uses_instance(&pw.load_profile(&c).unwrap()).unwrap());

    // what the game put in an instance outlives its profile
    let err = pw.remove_profile(&b).unwrap_err();
    assert!(
        matches!(
            err.fix(),
            Some(crate::fix::Fix::Undeploy {
                instance: Some(_),
                ..
            })
        ),
        "{err}"
    );
    let b_loaded = pw.load_profile(&b).unwrap();
    let target = pw.deploy_target(&b_loaded).unwrap();
    pw.apply(&pw.plan_undeploy(&target).unwrap()).unwrap();
    let saves = pw.instance_dir(&b).join("saves");
    fs::create_dir_all(&saves).unwrap();
    pw.remove_profile(&b).unwrap();
    assert!(saves.exists());
    assert!(
        !pw.paths
            .deployments()
            .join(format!("{}.json", target.key()))
            .exists()
    );
}

#[test]
fn only_games_with_instances_let_profiles_use_one() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = with_local_files(tmp.path());
    let profile = pw.create_profile("my-game", "default").unwrap();
    let err = pw.set_instance(&profile, Some(true)).unwrap_err();
    assert!(err.to_string().contains("can't have instances"), "{err}");
    pw.set_instance(&profile, None).unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    assert!(!pw.uses_instance(&loaded).unwrap());
}

/// Keeps a list of names for a profile or a mod.
struct Names;

impl Extension for Names {
    fn id(&self) -> &'static str {
        "names"
    }

    fn register(&self, _: &mut crate::ext::Registrar<'_>) {}
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct NamesState {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    names: Vec<String>,
}

fn names(names: &[&str]) -> NamesState {
    NamesState {
        names: names.iter().map(|n| n.to_string()).collect(),
    }
}

#[tokio::test]
async fn extension_state_is_kept_and_makes_deploys_stale() {
    use crate::ext::StateOf;

    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .extension(Names)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "none");
    def.ext.insert("names".to_owned(), toml::Table::new());
    pw.add_game(def).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);

    let state: NamesState = pw.ext_state(&profile, "names", StateOf::Profile).unwrap();
    assert_eq!(state, NamesState::default());
    pw.set_ext_state(&profile, "names", StateOf::Profile, &names(&["a"]))
        .unwrap();
    let state: NamesState = pw.ext_state(&profile, "names", StateOf::Profile).unwrap();
    assert_eq!(state, names(&["a"]));
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Stale);

    // what's deployed is kept in the history, so reverting brings back the
    // state as it was then
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
    pw.set_ext_state(&profile, "names", StateOf::Profile, &names(&["b"]))
        .unwrap();
    assert!(pw.has_changes(&profile).unwrap());
    let history = pw.history(&profile).unwrap();
    assert_eq!(
        history.last().unwrap().changes,
        [Change::Ext {
            id: "names".to_owned()
        }]
    );
    pw.revert(&profile).unwrap();
    let state: NamesState = pw.ext_state(&profile, "names", StateOf::Profile).unwrap();
    assert_eq!(state, names(&["a"]));
}

#[test]
fn extensions_are_only_on_for_games_that_ask_for_them() {
    use crate::ext::StateOf;

    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .extension(Names)
        .open()
        .unwrap();
    pw.add_game(new_game("plain", "none")).unwrap();
    let mut def = new_game("fancy", "none");
    def.ext.insert("names".to_owned(), toml::Table::new());
    def.ext.insert("elsewhere".to_owned(), toml::Table::new());
    pw.add_game(def).unwrap();

    let registry = pw.registry();
    let plain = &registry.game("plain").unwrap().value;
    let fancy = &registry.game("fancy").unwrap().value;
    assert!(pw.extensions_on(plain).is_empty());
    assert_eq!(pw.extensions_on(fancy), ["names"]);
    assert_eq!(pw.missing_extensions(fancy), ["elsewhere"]);

    let profile = pw.create_profile("plain", "default").unwrap();
    let err = pw
        .set_ext_state(&profile, "names", StateOf::Profile, &names(&["a"]))
        .unwrap_err();
    assert!(err.to_string().contains("isn't on for"), "{err}");

    // an extension this build lacks is an issue, and its state is left as
    // it was
    let profile = pw.create_profile("fancy", "default").unwrap();
    let issues = pw.issues(&profile).unwrap();
    assert_eq!(issues.len(), 1);
    assert_eq!(
        issues[0].kind,
        crate::issue::IssueKind::MissingExtension {
            id: "elsewhere".to_owned()
        }
    );
    let mut loaded = pw.load_profile(&profile).unwrap();
    let theirs: toml::Table = toml::from_str("v = 3\nwhatever = [1, 2]").unwrap();
    loaded.ext.insert("elsewhere".to_owned(), theirs.clone());
    pw.save_profile(&profile, &loaded).unwrap();
    pw.set_ext_state(&profile, "names", StateOf::Profile, &names(&["a"]))
        .unwrap();
    assert_eq!(pw.load_profile(&profile).unwrap().ext["elsewhere"], theirs);
}

#[test]
fn clashing_extensions_stop_patchwork_opening() {
    struct Again;

    impl Extension for Again {
        fn id(&self) -> &'static str {
            "again"
        }

        fn register(&self, ext: &mut crate::ext::Registrar<'_>) {
            ext.deployer(NoDeployer);
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let err = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .extension(Again)
        .open()
        .err()
        .unwrap();
    assert!(
        err.to_string()
            .contains("deployer `none` is registered by both the builder and extension `again`"),
        "{err}"
    );
}

/// A list of whatever the game's `[ext.lists.letters]` says, with the ones
/// in capitals in a tier of their own before the rest.
struct Letters;

impl crate::list::OrderedList for Letters {
    fn title(&self) -> &str {
        "Letters"
    }

    fn items(
        &self,
        ctx: &crate::list::ListContext<'_>,
        _: &[InstalledMod],
    ) -> Result<crate::list::ListItems> {
        let items = ctx.config()["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                let name = v.as_str().unwrap().to_owned();
                crate::list::ListItem {
                    tier: if name.chars().all(|c| c.is_ascii_uppercase()) {
                        0
                    } else {
                        1
                    },
                    name,
                    mod_name: "m".to_owned(),
                    badges: Vec::new(),
                    after: Vec::new(),
                    detail: None,
                    can_disable: true,
                }
            })
            .collect();
        Ok(crate::list::ListItems {
            items,
            ..Default::default()
        })
    }
}

struct Lists;

impl Extension for Lists {
    fn id(&self) -> &'static str {
        "lists"
    }

    fn register(&self, ext: &mut crate::ext::Registrar<'_>) {
        ext.deployer(Writer).ordered_list("letters", Letters);
    }
}

/// Writes the enabled letters into `order.txt`.
struct Writer;

impl Deployer for Writer {
    fn kind(&self) -> &str {
        "writer"
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
        let list = build.list("lists.letters").unwrap();
        let text: Vec<&str> = list
            .items
            .iter()
            .filter(|i| i.enabled)
            .map(|i| i.item.name.as_str())
            .collect();
        fs::write(build.out.join("order.txt"), text.join(" ")).unwrap();
        let dst = crate::relpath::RelPath::new("order.txt").unwrap();
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

fn lettered_game(off: &[&str]) -> GameDef {
    let mut def = new_game("my-game", "writer");
    let mut ext = toml::Table::new();
    ext.insert(
        "letters".to_owned(),
        toml::Value::Table(toml::from_str(r#"items = ["a", "b", "C", "d"]"#).unwrap()),
    );
    if !off.is_empty() {
        ext.insert(
            "off".to_owned(),
            toml::Value::Array(off.iter().map(|o| toml::Value::from(*o)).collect()),
        );
    }
    def.ext.insert("lists".to_owned(), ext);
    def
}

#[tokio::test]
async fn ordered_lists_are_saved_and_given_to_the_deployer() {
    use crate::list::ListEntry;

    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .extension(Lists)
        .open()
        .unwrap();
    pw.add_game(lettered_game(&[])).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let order = || -> Vec<String> {
        pw.list(&profile, "lists.letters")
            .unwrap()
            .items
            .into_iter()
            .map(|i| i.item.name)
            .collect()
    };

    assert_eq!(
        pw.lists(&profile).unwrap(),
        [ListInfo {
            id: "lists.letters".to_owned(),
            title: "Letters".to_owned()
        }]
    );
    assert_eq!(order(), ["C", "a", "b", "d"]);
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(
        fs::read_to_string(game.join("order.txt")).unwrap(),
        "C a b d"
    );

    // a move stays in its tier, and makes the deploy stale
    assert_eq!(
        pw.move_in_list(&profile, "lists.letters", "d", &Position::Top)
            .unwrap(),
        1
    );
    pw.set_list_enabled(&profile, "lists.letters", "b", false)
        .unwrap();
    assert_eq!(order(), ["C", "d", "a", "b"]);
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Stale);
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(fs::read_to_string(game.join("order.txt")).unwrap(), "C d a");
    let history = pw.history(&profile).unwrap();
    assert_eq!(
        history.last().unwrap().changes,
        [Change::List {
            id: "lists.letters".to_owned(),
            title: "Letters".to_owned()
        }]
    );

    // it's kept in the extension's state, where typed state can sit next
    // to it without either disturbing the other
    let saved = &pw.load_profile(&profile).unwrap().ext["lists"];
    assert_eq!(saved["letters"].as_array().unwrap().len(), 4);
    pw.set_ext_state(
        &profile,
        "lists",
        crate::ext::StateOf::Profile,
        &names(&["x"]),
    )
    .unwrap();
    assert_eq!(order(), ["C", "d", "a", "b"]);

    // items left out of a save keep their places
    pw.set_list(
        &profile,
        "lists.letters",
        vec![ListEntry {
            name: "a".to_owned(),
            enabled: true,
        }],
    )
    .unwrap();
    assert_eq!(order(), ["C", "d", "a", "b"]);
    let err = pw
        .set_list(
            &profile,
            "lists.letters",
            vec![
                ListEntry {
                    name: "a".to_owned(),
                    enabled: true,
                },
                ListEntry {
                    name: "A".to_owned(),
                    enabled: true,
                },
            ],
        )
        .unwrap_err();
    assert!(err.to_string().contains("twice"), "{err}");
}

#[test]
fn lists_can_be_turned_off_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .extension(Lists)
        .open()
        .unwrap();
    pw.add_game(lettered_game(&["letters", "lettters"]))
        .unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    assert!(pw.lists(&profile).unwrap().is_empty());
    assert!(pw.list(&profile, "lists.letters").is_err());
    let issues = pw.issues(&profile).unwrap();
    assert_eq!(
        issues.iter().map(|i| &i.kind).collect::<Vec<_>>(),
        [&crate::issue::IssueKind::UnknownHook {
            ext: "lists".to_owned(),
            name: "lettters".to_owned()
        }]
    );
}

/// Warns about each enabled vowel in its extension's `letters` list.
struct Vowels;

impl crate::check::Check for Vowels {
    fn check(
        &self,
        ctx: &crate::check::CheckContext<'_>,
        _: &[InstalledMod],
    ) -> Vec<crate::check::Problem> {
        let Some(list) = ctx.list("letters") else {
            return Vec::new();
        };
        list.items
            .iter()
            .filter(|i| i.enabled && "aeiou".contains(&i.item.name))
            .map(|i| {
                crate::check::Problem::warning(format!("{} is a vowel", i.item.name))
                    .about_mod(&i.item.mod_name)
                    .about_list("letters")
            })
            .collect()
    }
}

struct Checked;

impl Extension for Checked {
    fn id(&self) -> &'static str {
        "checked"
    }

    fn register(&self, ext: &mut crate::ext::Registrar<'_>) {
        ext.ordered_list("letters", Letters).check("vowels", Vowels);
    }
}

fn checked_game(off: &[&str]) -> GameDef {
    let mut def = lettered_game(&[]);
    let mut ext = def.ext["lists"].clone();
    if !off.is_empty() {
        ext.insert(
            "off".to_owned(),
            toml::Value::Array(off.iter().map(|o| toml::Value::from(*o)).collect()),
        );
    }
    def.ext.insert("checked".to_owned(), ext);
    def
}

#[tokio::test]
async fn checks_report_issues_and_problems_with_their_lists() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .extension(Lists)
        .extension(Checked)
        .open()
        .unwrap();
    pw.add_game(checked_game(&[])).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();

    let plan = pw.plan_deploy(&profile).await.unwrap();
    assert_eq!(plan.warnings, ["a is a vowel"]);
    let issues = pw.issues(&profile).unwrap();
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert_eq!(issues[0].message, "a is a vowel");
    assert_eq!(issues[0].mods, ["m"]);
    assert_eq!(
        issues[0].kind,
        crate::issue::IssueKind::Check {
            id: "checked.vowels".to_owned(),
            list: Some("checked.letters".to_owned()),
        }
    );
    // it's shown with the list it's about, and goes when it's dealt with
    let problems = |pw: &Patchwork| -> Vec<String> {
        pw.list(&profile, "checked.letters")
            .unwrap()
            .problems
            .into_iter()
            .map(|p| p.message)
            .collect()
    };
    assert_eq!(problems(&pw), ["a is a vowel"]);
    assert!(
        pw.list(&profile, "lists.letters")
            .unwrap()
            .problems
            .is_empty()
    );
    pw.set_list_enabled(&profile, "checked.letters", "a", false)
        .unwrap();
    assert!(problems(&pw).is_empty());
    assert!(pw.issues(&profile).unwrap().is_empty());
}

#[tokio::test]
async fn checks_can_be_turned_off_by_name() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .extension(Lists)
        .extension(Checked)
        .open()
        .unwrap();
    pw.add_game(checked_game(&["vowels"])).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    assert!(pw.plan_deploy(&profile).await.unwrap().warnings.is_empty());
    assert!(pw.issues(&profile).unwrap().is_empty());
}

/// Config files of `[section]`s and `key = value` lines, where values are
/// whole numbers or text.
struct KeyValues;

impl crate::config::ConfigFormat for KeyValues {
    fn read(
        &self,
        _: &crate::config::ConfigContext<'_>,
        file: &[u8],
    ) -> Result<crate::config::Contents> {
        use crate::options::{Field, FieldKind, Group, GroupKind, Value};
        let mut groups: Vec<Group> = Vec::new();
        for line in std::str::from_utf8(file).unwrap().lines() {
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                groups.push(Group {
                    id: name.to_owned(),
                    title: name.to_owned(),
                    kind: GroupKind::Fields { fields: Vec::new() },
                    defaulted: true,
                });
            } else if let Some((key, value)) = line.split_once(" = ")
                && let Some(Group {
                    kind: GroupKind::Fields { fields },
                    ..
                }) = groups.last_mut()
            {
                let (kind, value) = match value.parse() {
                    Ok(n) => (
                        FieldKind::Int {
                            min: Some(0),
                            max: Some(10),
                            step: None,
                        },
                        Value::Int(n),
                    ),
                    Err(_) => (
                        FieldKind::Text { multiline: false },
                        Value::Text(value.to_owned()),
                    ),
                };
                fields.push(Field {
                    id: key.to_owned(),
                    title: key.to_owned(),
                    description: None,
                    kind,
                    value,
                    default: None,
                });
            }
        }
        Ok(crate::config::Contents {
            title: None,
            groups,
            warnings: Vec::new(),
        })
    }

    fn write(
        &self,
        _: &crate::config::ConfigContext<'_>,
        file: &[u8],
        values: &[crate::options::Answer],
    ) -> Result<Vec<u8>> {
        let mut section = String::new();
        let mut out = String::new();
        for line in std::str::from_utf8(file).unwrap().lines() {
            let mut line = line.to_owned();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.to_owned();
            } else if let Some((key, _)) = line.split_once(" = ")
                && let Some(value) = values
                    .iter()
                    .filter(|a| a.group == section)
                    .find_map(|a| a.values.get(key))
            {
                line = match value {
                    crate::options::Value::Text(text) => format!("{key} = {text}"),
                    other => format!("{key} = {other}"),
                };
            }
            out.push_str(&line);
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
}

struct KeyValuesExtension;

impl Extension for KeyValuesExtension {
    fn id(&self) -> &'static str {
        "test"
    }

    fn register(&self, ext: &mut crate::ext::Registrar<'_>) {
        ext.config_format("kv", KeyValues);
    }
}

#[tokio::test]
async fn config_files_change_settings_and_keep_what_the_game_changes() {
    use crate::deploy::Dest;
    use crate::options::{Answer, Value};

    let tmp = tempfile::tempdir().unwrap();
    let packed = tmp.path().join("mods/Fast");
    fs::create_dir_all(packed.join("config")).unwrap();
    fs::write(
        packed.join("config/fast.cfg"),
        "[speed]\nlevel = 1\nname = quick\n",
    )
    .unwrap();
    let pw = Patchwork::builder(Paths::rooted(&tmp.path().join("pw")))
        .source(LocalFiles)
        .deployer(AsPacked)
        .extension(KeyValuesExtension)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "as-packed");
    def.config = vec![
        toml::from_str(
            r#"files = "config/**/*.cfg"
format = "test.kv""#,
        )
        .unwrap(),
    ];
    pw.add_game(def).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(game.join("config")).unwrap();
    // one the game made itself
    fs::write(game.join("config/game.cfg"), "[video]\nfov = 5\n").unwrap();
    fs::write(game.join("config/notes.txt"), "not config").unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let game = game.canonicalize().unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    pw.add_mod(&profile, local_spec(&packed), None, true)
        .await
        .unwrap();
    let fast: Dest = "config/fast.cfg".parse().unwrap();
    let own: Dest = "config/game.cfg".parse().unwrap();

    // only locked mods' files are known
    assert_eq!(pw.config_files(&profile).unwrap().len(), 1);
    pw.lock(&profile, false).await.unwrap();
    let files = pw.config_files(&profile).unwrap();
    let listed: Vec<_> = files
        .iter()
        .map(|f| (f.path.to_string(), f.from.clone(), f.edited))
        .collect();
    assert_eq!(
        listed,
        [
            ("config/fast.cfg".to_owned(), Some("Fast".to_owned()), false),
            ("config/game.cfg".to_owned(), None, false),
        ]
    );

    let view = pw.config_form(&profile, &fast, None).unwrap();
    assert!(!view.saved);
    let fields = view.form.pages[0].groups[0].fields();
    assert_eq!(fields[0].value, Value::Int(1));

    // only what's different from the file is kept, and values are checked
    let set = |level: i64| {
        vec![Answer::values(
            "speed",
            BTreeMap::from([
                ("level".to_owned(), Value::Int(level)),
                ("name".to_owned(), Value::Text("quick".to_owned())),
            ]),
        )]
    };
    assert!(
        pw.set_config(&profile, &fast, Some(&set(11)), None)
            .is_err()
    );
    pw.set_config(&profile, &fast, Some(&set(5)), None).unwrap();
    let saved = &pw.load_profile(&profile).unwrap().config;
    assert_eq!(saved.len(), 1);
    assert_eq!(
        saved[&fast]["speed"],
        BTreeMap::from([("level".to_owned(), Value::Int(5))])
    );
    let view = pw.config_form(&profile, &fast, None).unwrap();
    assert!(view.saved);
    assert_eq!(
        view.form.pages[0].groups[0].fields()[0].value,
        Value::Int(5)
    );
    // answers being changed are shown, with what's wrong with them
    let view = pw.config_form(&profile, &fast, Some(&set(12))).unwrap();
    assert_eq!(
        view.form.pages[0].groups[0].fields()[0].value,
        Value::Int(1)
    );
    assert!(!view.form.warnings.is_empty());

    let fov = |n: i64| {
        vec![Answer::values(
            "video",
            BTreeMap::from([("fov".to_owned(), Value::Int(n))]),
        )]
    };
    pw.set_config(&profile, &own, Some(&fov(9)), None).unwrap();
    // spelled another way, it's the same file
    let shouting: Dest = "CONFIG/GAME.CFG".parse().unwrap();
    pw.set_config(&profile, &shouting, Some(&fov(8)), None)
        .unwrap();
    assert_eq!(pw.load_profile(&profile).unwrap().config.len(), 2);

    let plan = pw.plan_deploy(&profile).await.unwrap();
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    pw.apply(&plan).unwrap();
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
    assert_eq!(
        fs::read_to_string(game.join("config/fast.cfg")).unwrap(),
        "[speed]\nlevel = 5\nname = quick\n"
    );
    assert_eq!(
        fs::read_to_string(game.join("config/game.cfg")).unwrap(),
        "[video]\nfov = 8\n"
    );

    // the game changes them, which isn't a stale deploy; deploying again
    // keeps what it changed, and doesn't write into the store's copy;
    // settings it added aren't kept, since they can't be put back
    fs::write(
        game.join("config/fast.cfg"),
        "[speed]\nlevel = 7\nname = quick\n",
    )
    .unwrap();
    fs::write(game.join("config/game.cfg"), "[video]\nfov = 8\nnew = 3\n").unwrap();
    assert_eq!(
        fs::read_to_string(packed.join("config/fast.cfg")).unwrap(),
        "[speed]\nlevel = 1\nname = quick\n"
    );
    assert_eq!(pw.deploy_status(&profile).unwrap(), DeployStatus::Deployed);
    let view = pw.config_form(&profile, &fast, None).unwrap();
    assert_eq!(
        view.form.pages[0].groups[0].fields()[0].value,
        Value::Int(7)
    );
    let saved = pw.load_profile(&profile).unwrap().config;
    assert!(!saved[&own]["video"].contains_key("new"), "{saved:?}");
    let plan = pw.plan_deploy(&profile).await.unwrap();
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
    pw.apply(&plan).unwrap();
    assert_eq!(
        fs::read_to_string(game.join("config/fast.cfg")).unwrap(),
        "[speed]\nlevel = 7\nname = quick\n"
    );

    // forgetting a file's settings puts the file back as it was, and
    // what the game had changed in it isn't read back again
    fs::write(game.join("config/game.cfg"), "[video]\nfov = 2\n").unwrap();
    pw.set_config(&profile, &own, None, None).unwrap();
    assert_eq!(pw.load_profile(&profile).unwrap().config.len(), 1);
    pw.apply(&pw.plan_deploy(&profile).await.unwrap()).unwrap();
    assert_eq!(
        fs::read_to_string(game.join("config/game.cfg")).unwrap(),
        "[video]\nfov = 5\n"
    );
    assert_eq!(pw.load_profile(&profile).unwrap().config.len(), 1);

    // undeploying keeps what the game changed, and removes the mod's copy
    fs::write(
        game.join("config/fast.cfg"),
        "[speed]\nlevel = 2\nname = quick\n",
    )
    .unwrap();
    let target = pw
        .deploy_target(&pw.load_profile(&profile).unwrap())
        .unwrap();
    pw.apply(&pw.plan_undeploy(&target).unwrap()).unwrap();
    assert!(!game.join("config/fast.cfg").exists());
    let saved = pw.load_profile(&profile).unwrap().config;
    assert_eq!(saved[&fast]["speed"]["level"], Value::Int(2));
}

/// Like [`Versioned`], but `app` needs `lib` (2.0 or newer), which needs
/// `base`.
struct WithDependencies(std::sync::Arc<Versioned>);

#[async_trait::async_trait]
impl Source for WithDependencies {
    fn id(&self) -> &str {
        "deps"
    }

    async fn resolve(
        &self,
        ctx: &SourceContext<'_>,
        id: &str,
        requested: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        let mut resolved = self.0.resolve(ctx, id, requested).await?;
        let dependency = |id: &str, req| crate::source::Dependency {
            source: "deps".to_owned(),
            id: id.to_owned(),
            req,
        };
        resolved.dependencies = match id {
            "app" => vec![dependency(
                "lib",
                crate::source::VersionReq::AtLeast("2.0".to_owned()),
            )],
            "lib" => vec![dependency("base", crate::source::VersionReq::Any)],
            _ => Vec::new(),
        };
        Ok(resolved)
    }
}

#[tokio::test]
async fn replaced_dependencies_give_way_to_the_mod_replacing_them() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("mods");
    fs::create_dir_all(&dir).unwrap();
    let source = std::sync::Arc::new(Versioned {
        dir,
        version: std::sync::Mutex::new("1.0".to_owned()),
    });
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(WithDependencies(source.clone()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    for id in ["app", "fork"] {
        let spec = ModSpec {
            source: "deps".to_owned(),
            id: id.to_owned(),
            version: None,
        };
        pw.add_mod(&profile, spec, None, true).await.unwrap();
    }
    let keys = |lock: &Lockfile| {
        let mut keys: Vec<String> = lock.mods.iter().map(LockedMod::key).collect();
        keys.sort();
        keys
    };
    let app = |lock: &Lockfile| lock.mods.iter().find(|m| m.id == "app").unwrap().clone();

    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(
        keys(&lock),
        ["deps:app", "deps:base", "deps:fork", "deps:lib"]
    );
    assert_eq!(pw.unmet_requirements(&lock).len(), 1, "lib 1.0 is too old");

    // a profile mod, itself, or a name nothing's locked as can't be replaced
    for bad in ["app", "fork", "nothing"] {
        let err = pw
            .set_replaces(&profile, "fork", &[bad.to_owned()])
            .unwrap_err();
        assert!(matches!(err, Error::Invalid(_)), "{bad}: {err}");
    }

    // by the name it's locked as; a newer app is out by now, but only the
    // replacement changed, so it stays where it was
    *source.version.lock().unwrap() = "2.0".to_owned();
    pw.set_replaces(&profile, "fork", &["lib".to_owned()])
        .unwrap();
    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(keys(&lock), ["deps:app", "deps:fork"]);
    let locked = app(&lock);
    assert_eq!(locked.version.as_deref(), Some("1.0"));
    assert_eq!(locked.dependencies, ["deps:fork"]);
    assert_eq!(
        locked.replaced,
        BTreeMap::from([("deps:lib".to_owned(), "deps:fork".to_owned())])
    );
    assert!(pw.unmet_requirements(&lock).is_empty());
    let order: Vec<String> = deploy_order(&pw.load_profile(&profile).unwrap(), &lock)
        .iter()
        .map(|m| m.key())
        .collect();
    assert_eq!(order, ["deps:fork", "deps:app"]);
    let info = lock.dependency_info(&pw.load_profile(&profile).unwrap());
    let about = |key: &str| info.iter().find(|i| i.key == key).unwrap();
    assert!(about("deps:fork").in_profile);
    assert_eq!(about("deps:fork").needed_by, ["app"]);
    assert_eq!(about("deps:lib").replaced_by.as_deref(), Some("fork"));
    assert_eq!(about("deps:lib").needed_by, ["app"]);
    assert!(!about("deps:lib").in_profile);
    assert!(about("deps:app").needed_by.is_empty());
    assert!(info.iter().all(|i| i.in_use));

    // with what needs them turned off, the dependencies aren't in use
    pw.set_enabled(&profile, "app", false).unwrap();
    let info = lock.dependency_info(&pw.load_profile(&profile).unwrap());
    let about = |key: &str| info.iter().find(|i| i.key == key).unwrap();
    assert!(!about("deps:app").in_use);
    assert!(!about("deps:lib").in_use);
    assert!(
        about("deps:fork").in_use,
        "fork is a profile mod of its own"
    );
    pw.set_enabled(&profile, "app", true).unwrap();

    pw.set_replaces(&profile, "fork", &[]).unwrap();
    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(
        keys(&lock),
        ["deps:app", "deps:base", "deps:fork", "deps:lib"]
    );
    assert_eq!(app(&lock).dependencies, ["deps:lib"]);
    assert!(app(&lock).replaced.is_empty());
}

/// Resolves every mod to the same download, listing an unsure soft
/// dependency, an optional one and one on another site.
struct WithSoft(String);

#[async_trait::async_trait]
impl Source for WithSoft {
    fn id(&self) -> &str {
        "soft"
    }

    async fn resolve(
        &self,
        _: &SourceContext<'_>,
        id: &str,
        _: Option<&str>,
    ) -> Result<crate::source::ResolvedMod> {
        let soft = |key: Option<&str>, name: &str, need, url: Option<&str>| SoftDependency {
            key: key.map(str::to_owned),
            name: Some(name.to_owned()),
            need,
            notes: None,
            url: url.map(str::to_owned),
        };
        Ok(crate::source::ResolvedMod {
            id: id.to_owned(),
            name: id.to_owned(),
            version: Some("1.0".to_owned()),
            artifact: ArtifactLocation::Url {
                url: self.0.clone(),
            },
            format: ArtifactFormat::File,
            dependencies: Vec::new(),
            soft_dependencies: vec![
                soft(Some("soft:lib"), "Lib", Need::Unknown, None),
                soft(Some("soft:extra"), "Extra", Need::Optional, None),
                soft(
                    None,
                    "Elsewhere",
                    Need::Unknown,
                    Some("https://example.com"),
                ),
            ],
            game_versions: None,
            env: Default::default(),
            kind: None,
        })
    }
}

/// Keeps the notices it's given.
#[derive(Clone, Default)]
struct Notices(Arc<std::sync::Mutex<Vec<String>>>);

impl crate::event::Reporter for Notices {
    fn report(&self, event: Event) {
        if let Event::Notice(notice) = event {
            self.0.lock().unwrap().push(notice);
        }
    }
}

#[tokio::test]
async fn soft_dependencies_are_shown_not_installed() {
    let tmp = tempfile::tempdir().unwrap();
    let notices = Notices::default();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .source(WithSoft(serve_once("200 OK", b"mod bytes")))
        .deployer(NoDeployer)
        .reporter(notices.clone())
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let spec = ModSpec {
        source: "soft".to_owned(),
        id: "app".to_owned(),
        version: None,
    };
    pw.add_mod(&profile, spec, None, true).await.unwrap();

    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(lock.mods.len(), 1, "soft dependencies aren't locked");
    let said = notices.0.lock().unwrap().clone();
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(said[0].contains("`Lib`, `Elsewhere`"), "{}", said[0]);
    assert!(!said[0].contains("Extra"), "optional ones aren't mentioned");

    // only the first time it's locked
    pw.lock(&profile, false).await.unwrap();
    assert_eq!(notices.0.lock().unwrap().len(), 1);

    pw.set_ignored(&profile, "app", "soft:lib", true).unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    assert_eq!(loaded.mods[0].ignores, ["soft:lib"]);
    let soft = lock.soft_dependencies(&loaded, "soft:app");
    let missing: Vec<&str> = soft
        .iter()
        .filter(|d| d.missing())
        .map(|d| d.ident.as_str())
        .collect();
    assert_eq!(missing, ["soft:extra", "https://example.com"]);

    pw.set_ignored(&profile, "app", "soft:lib", false).unwrap();
    assert!(
        pw.load_profile(&profile).unwrap().mods[0]
            .ignores
            .is_empty()
    );
}

#[test]
fn game_edits_go_in_the_users_file_over_the_definition() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .deployer(FakeRules)
        .open()
        .unwrap();
    let game = "lethal-company";
    let before = pw.registry().game(game).unwrap().clone();
    let text = |s: &str| Some(FieldValue::Text(s.to_owned()));

    let fields = pw.game_fields(game).unwrap();
    let name = fields.iter().find(|f| f.id == "name").unwrap();
    assert_eq!(name.source, text(&before.value.name));
    assert_eq!(name.edited, None);

    pw.set_game_edits(
        game,
        BTreeMap::from([
            ("name".to_owned(), text("LC")),
            ("color".to_owned(), text("#ff0000")),
        ]),
    )
    .unwrap();
    let after = pw.registry().game(game).unwrap().clone();
    assert_eq!(after.value.name, "LC");
    assert_eq!(after.value.accent.unwrap().as_str(), "#ff0000");
    assert_eq!(after.overlays, [crate::registry::Origin::User]);
    // only how it's shown changed, so nothing needs deploying again
    assert_eq!(after.hash, before.hash);
    let fields = pw.game_fields(game).unwrap();
    let name = fields.iter().find(|f| f.id == "name").unwrap();
    assert_eq!(name.source, text(&before.value.name));
    assert_eq!(name.edited, text("LC"));

    // a bad value saves nothing
    let bad = pw.set_game_edits(
        game,
        BTreeMap::from([("name".to_owned(), None), ("color".to_owned(), text("red"))]),
    );
    assert!(bad.is_err());
    assert_eq!(pw.registry().game(game).unwrap().value.name, "LC");
    assert!(
        pw.set_game_edits(game, BTreeMap::from([("nope".to_owned(), None)]))
            .is_err()
    );

    // put back, the file goes
    pw.set_game_edits(
        game,
        BTreeMap::from([("name".to_owned(), None), ("color".to_owned(), None)]),
    )
    .unwrap();
    let file = pw
        .paths()
        .user_registry()
        .join(format!("games/{game}.toml"));
    assert!(!file.exists());
    let reverted = pw.registry().game(game).unwrap().clone();
    assert_eq!(reverted.value.name, before.value.name);
    assert!(reverted.overlays.is_empty());
}

#[test]
fn added_games_keep_their_name() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();

    let fields = pw.game_fields("my-game").unwrap();
    assert!(fields.iter().all(|f| f.source.is_none()));
    let err = pw
        .set_game_edits("my-game", BTreeMap::from([("name".to_owned(), None)]))
        .unwrap_err();
    assert!(err.to_string().contains("name"), "{err}");

    pw.set_game_edits(
        "my-game",
        BTreeMap::from([("name".to_owned(), Some(FieldValue::Text("Renamed".into())))]),
    )
    .unwrap();
    let entry = pw.registry().game("my-game").unwrap().clone();
    assert_eq!(entry.value.name, "Renamed");
    // still the whole definition
    assert_eq!(entry.value.deployer.kind, "none");
}

/// Puts Steam app `app` in the library at `library`, as Steam would.
fn steam_install(library: &Path, app: u32, dir: &str) -> PathBuf {
    let steamapps = library.join("steamapps");
    let path = steamapps.join("common").join(dir);
    fs::create_dir_all(&path).unwrap();
    fs::write(
        steamapps.join(format!("appmanifest_{app}.acf")),
        format!("\"AppState\"\n{{\n\t\"appid\"\t\t\"{app}\"\n\t\"installdir\"\t\t\"{dir}\"\n}}\n"),
    )
    .unwrap();
    path.canonicalize().unwrap()
}

#[test]
fn detecting_games_sets_up_found_ones_and_moves_gone_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(&tmp.path().join("pw")))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    let steam = tmp.path().join("steam");
    let other = tmp.path().join("other");
    for (id, app) in [
        ("new", 1),
        ("kept", 2),
        ("moved", 3),
        ("gone", 4),
        ("twice", 5),
    ] {
        let mut def = new_game(id, "none");
        def.detect.steam = vec![app];
        pw.add_game(def).unwrap();
    }
    let new = steam_install(&steam, 1, "New");
    // set up somewhere of the user's own, which is left alone
    steam_install(&steam, 2, "Kept");
    let kept = tmp.path().join("my games/Kept");
    fs::create_dir_all(&kept).unwrap();
    pw.set_game_path("kept", &kept).unwrap();
    // set up, then its folder went and it turned up in another library
    let old = tmp.path().join("old/Moved");
    fs::create_dir_all(&old).unwrap();
    pw.set_game_path("moved", &old).unwrap();
    fs::remove_dir_all(&old).unwrap();
    let moved = steam_install(&steam, 3, "Moved");
    // set up, then its folder went for good
    let gone = tmp.path().join("old/Gone");
    fs::create_dir_all(&gone).unwrap();
    pw.set_game_path("gone", &gone).unwrap();
    fs::remove_dir_all(&gone).unwrap();
    steam_install(&steam, 5, "Twice");
    steam_install(&other, 5, "Twice");

    let detected = pw.detect_games(&Locator::steam([steam, other]));
    assert_eq!(detected.added, ["new"]);
    assert_eq!(detected.moved, ["moved"]);
    assert_eq!(detected.missing, ["gone"]);
    assert_eq!(detected.ambiguous, ["twice"]);
    assert!(detected.failed.is_empty(), "{:?}", detected.failed);
    let path = |id| pw.game_install(id).unwrap().map(|i| i.path);
    assert_eq!(path("new"), Some(new));
    assert_eq!(path("kept"), Some(kept.canonicalize().unwrap()));
    assert_eq!(path("moved"), Some(moved));
    assert!(path("gone").is_some());
    assert_eq!(path("twice"), None);

    // nothing more to do the second time
    let again = pw.detect_games(&Locator::default());
    assert!(again.added.is_empty() && again.moved.is_empty());
    assert_eq!(again.missing, ["gone"]);
}

#[test]
fn folders_keep_their_mods_together() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    let mut loaded = pw.load_profile(&profile).unwrap();
    for name in ["a", "b", "c", "d", "e"] {
        loaded.mods.push(ProfileMod {
            name: name.to_owned(),
            source: "local".to_owned(),
            id: name.to_owned(),
            version: None,
            enabled: true,
            priority: 0,
            game_versions: None,
            ext: Default::default(),
            replaces: Vec::new(),
            seen: None,
            ignores: Vec::new(),
            edits: Default::default(),
            folder: None,
        });
    }
    pw.save_profile(&profile, &loaded).unwrap();
    let names = || -> Vec<String> {
        let loaded = pw.load_profile(&profile).unwrap();
        loaded.mods.iter().map(|m| m.name.clone()).collect()
    };

    // a new folder goes where the first of its mods is
    let qol = pw
        .add_folder(&profile, " QOL ", &["b".to_owned(), "d".to_owned()])
        .unwrap();
    assert_eq!(qol.name, "QOL");
    assert_eq!(names(), ["a", "b", "d", "c", "e"]);
    let taken = pw.add_folder(&profile, "qol", &[]).unwrap_err();
    assert!(taken.to_string().contains("already has"), "{taken}");
    assert!(pw.add_folder(&profile, "  ", &[]).is_err());

    // moved in, a mod goes to the folder's end
    pw.set_mod_folder(&profile, "e", Some("qol")).unwrap();
    assert_eq!(names(), ["a", "b", "d", "e", "c"]);
    // taken out, to just after it
    pw.set_mod_folder(&profile, "b", None).unwrap();
    assert_eq!(names(), ["a", "d", "e", "b", "c"]);

    // a reorder that takes a folder apart is put back together
    let order = ["d", "a", "b", "c", "e"].map(str::to_owned);
    pw.reorder_mods(&profile, &order).unwrap();
    assert_eq!(names(), ["d", "e", "a", "b", "c"]);

    pw.set_folder_enabled(&profile, "QOL", false).unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    let off: Vec<&str> = loaded
        .mods
        .iter()
        .filter(|m| !m.enabled)
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(off, ["d", "e"]);

    pw.rename_folder(&profile, &qol.id.to_string(), "Client QOL")
        .unwrap();
    // renaming to its own name, in another case, is fine
    pw.rename_folder(&profile, "client qol", "Client qol")
        .unwrap();
    pw.remove_folder(&profile, "Client qol").unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    assert!(loaded.folders.is_empty());
    assert!(loaded.mods.iter().all(|m| m.folder.is_none()));
    assert_eq!(names(), ["d", "e", "a", "b", "c"]);
    assert!(pw.remove_folder(&profile, "Client qol").is_err());
}

/// A [`Versioned`] game with two profiles, `a` with mods `a`, `b` and `c`,
/// and `b` with mod `d`.
async fn two_profiles(tmp: &Path) -> (Patchwork, ProfileRef, ProfileRef) {
    let dir = tmp.join("mods");
    fs::create_dir_all(&dir).unwrap();
    let source = std::sync::Arc::new(Versioned {
        dir,
        version: std::sync::Mutex::new("1.0".to_owned()),
    });
    let pw = Patchwork::builder(Paths::rooted(tmp))
        .source(SharedSource(source))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let spec = |id: &str| ModSpec {
        source: "versioned".to_owned(),
        id: id.to_owned(),
        version: None,
    };
    let a = pw.create_profile("my-game", "a").unwrap();
    for id in ["a", "b", "c"] {
        pw.add_mod(&a, spec(id), None, true).await.unwrap();
    }
    let b = pw.create_profile("my-game", "b").unwrap();
    pw.add_mod(&b, spec("d"), None, true).await.unwrap();
    (pw, a, b)
}

fn mod_names(pw: &Patchwork, profile: &ProfileRef) -> Vec<String> {
    let loaded = pw.load_profile(profile).unwrap();
    loaded.mods.iter().map(|m| m.name.clone()).collect()
}

#[tokio::test]
async fn a_shared_folder_is_one_set_for_every_profile_that_includes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, a, b) = two_profiles(tmp.path()).await;
    pw.add_folder(&a, "QOL", &["b".to_owned(), "c".to_owned()])
        .unwrap();
    pw.set_enabled(&a, "b", false).unwrap();
    // saved as before there were sets until one's included
    let file = || fs::read_to_string(pw.profile_file(&a)).unwrap();
    assert!(file().contains("schema = 3"), "{}", file());

    let set = pw.share_folder(&a, "qol").unwrap();
    assert_eq!(set.name, "QOL");
    assert!(file().contains("schema = 4"), "{}", file());
    assert!(file().contains("[[include]]"), "{}", file());
    let loaded = pw.load_profile(&a).unwrap();
    assert_eq!(mod_names(&pw, &a), ["a", "b", "c"]);
    assert!(loaded.folders.is_empty());
    let b_mod = loaded.find("b").unwrap();
    assert_eq!(loaded.folder_of(b_mod).unwrap().name, "QOL");
    assert!(loaded.is_linked(b_mod));
    assert!(!b_mod.enabled, "still off in the profile that had it off");
    assert_eq!(pw.find_set("my-game", "qol").unwrap().mods.len(), 2);

    pw.include_set(&b, set.id, None).unwrap();
    assert_eq!(mod_names(&pw, &b), ["d", "b", "c"]);
    assert!(pw.load_profile(&b).unwrap().find("b").unwrap().enabled);
    assert_eq!(pw.profiles_including(set.id).unwrap().len(), 2);
    let mut labels: Vec<String> = pw.includers().unwrap()[&set.id]
        .iter()
        .map(|(_, label)| label.clone())
        .collect();
    labels.sort();
    assert_eq!(
        labels,
        [
            pw.load_profile(&a).unwrap().label(),
            pw.load_profile(&b).unwrap().label()
        ]
    );
    // locked with the set's mods
    let lock = pw.lock(&b, false).await.unwrap();
    let mut locked: Vec<&str> = lock.mods.iter().map(|m| m.name.as_str()).collect();
    locked.sort();
    assert_eq!(locked, ["b", "c", "d"]);

    // turning one off is only for that profile; anything else is the set's
    pw.set_enabled(&b, "c", false).unwrap();
    pw.set_priority(&b, "c", 3).unwrap();
    let in_a = pw.load_profile(&a).unwrap();
    assert!(in_a.find("c").unwrap().enabled);
    assert_eq!(in_a.find("c").unwrap().priority, 3);

    // filing a mod in the linked folder puts it in the set, for both
    pw.set_mod_folder(&b, "d", Some("QOL")).unwrap();
    assert_eq!(mod_names(&pw, &a), ["a", "b", "c", "d"]);
    assert_eq!(mod_names(&pw, &b), ["b", "c", "d"]);

    // renaming the linked folder renames the set; removing it isn't how
    pw.rename_folder(&b, "QOL", "Client QOL").unwrap();
    let in_a = pw.load_profile(&a).unwrap();
    assert_eq!(
        in_a.folder_of(in_a.find("b").unwrap()).unwrap().name,
        "Client QOL"
    );
    assert!(pw.remove_folder(&b, "Client QOL").is_err());
}

#[tokio::test]
async fn shared_folders_carry_their_mods_config_edits() {
    use crate::deploy::Dest;
    use crate::options::{Answer, Value};

    let tmp = tempfile::tempdir().unwrap();
    let packed = tmp.path().join("mods/Fast");
    fs::create_dir_all(packed.join("config")).unwrap();
    fs::write(packed.join("config/fast.cfg"), "[speed]\nlevel = 1\n").unwrap();
    let pw = Patchwork::builder(Paths::rooted(&tmp.path().join("pw")))
        .source(LocalFiles)
        .deployer(AsPacked)
        .extension(KeyValuesExtension)
        .open()
        .unwrap();
    let mut def = new_game("my-game", "as-packed");
    def.config = vec![toml::from_str("files = \"config/**/*.cfg\"\nformat = \"test.kv\"").unwrap()];
    pw.add_game(def).unwrap();
    let game = tmp.path().join("game");
    fs::create_dir_all(game.join("config")).unwrap();
    fs::write(game.join("config/game.cfg"), "[video]\nfov = 5\n").unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    let game = game.canonicalize().unwrap();
    let fast: Dest = "config/fast.cfg".parse().unwrap();
    let own: Dest = "config/game.cfg".parse().unwrap();
    let level = |n: i64| {
        vec![Answer::values(
            "speed",
            BTreeMap::from([("level".to_owned(), Value::Int(n))]),
        )]
    };
    let fov = |n: i64| {
        vec![Answer::values(
            "video",
            BTreeMap::from([("fov".to_owned(), Value::Int(n))]),
        )]
    };
    let shown = |p: &ProfileRef| match pw.config_form(p, &fast, None).unwrap().form.pages[0].groups
        [0]
    .fields()[0]
        .value
    {
        Value::Int(n) => n,
        ref other => panic!("{other:?}"),
    };

    let a = pw.create_profile("my-game", "a").unwrap();
    pw.add_mod(&a, local_spec(&packed), None, true)
        .await
        .unwrap();
    pw.lock(&a, false).await.unwrap();
    pw.set_config(&a, &fast, Some(&level(5)), None).unwrap();
    pw.set_config(&a, &own, Some(&fov(7)), None).unwrap();
    pw.add_folder(&a, "QOL", &["Fast".to_owned()]).unwrap();

    // sharing the folder takes its mod's settings with it, not the rest
    let set = pw.share_folder(&a, "QOL").unwrap();
    assert_eq!(set.config.len(), 1, "{:?}", set.config);
    let set_file = pw.paths.sets().join(set.id.to_string()).join("set.toml");
    assert!(fs::read_to_string(set_file).unwrap().contains("schema = 2"));
    let stored = fs::read_to_string(pw.profile_file(&a)).unwrap();
    assert!(!stored.contains("speed"), "{stored}");
    assert!(stored.contains("video"), "{stored}");
    assert_eq!(shown(&a), 5);

    let b = pw.create_profile("my-game", "b").unwrap();
    pw.include_set(&b, set.id, None).unwrap();
    pw.lock(&b, false).await.unwrap();
    let view = pw.config_form(&b, &fast, None).unwrap();
    assert_eq!(view.save_to, SaveTo::Set(set.id));
    assert_eq!(view.shared.len(), 1);
    assert!(!view.own);
    assert_eq!(shown(&b), 5);
    let files = pw.config_files(&b).unwrap();
    assert_eq!(files.iter().find(|f| f.path == fast).unwrap().sets, ["QOL"]);

    // in the profile alone, it's only that profile's
    pw.set_config(&b, &fast, Some(&level(6)), Some(SaveTo::Profile))
        .unwrap();
    assert_eq!((shown(&a), shown(&b)), (5, 6));
    assert!(pw.config_form(&b, &fast, None).unwrap().shared[0].overridden);
    // back to the file's own value is still the profile's
    pw.set_config(&b, &fast, Some(&level(1)), Some(SaveTo::Profile))
        .unwrap();
    assert_eq!((shown(&a), shown(&b)), (5, 1));
    // forgetting it goes back to the set's
    pw.set_config(&b, &fast, None, Some(SaveTo::Profile))
        .unwrap();
    assert_eq!(shown(&b), 5);

    // by default it goes to the set, for both, and the profile's own
    // change to the file goes with it
    pw.set_config(&b, &fast, Some(&level(6)), Some(SaveTo::Profile))
        .unwrap();
    pw.set_config(&b, &fast, Some(&level(3)), None).unwrap();
    assert_eq!((shown(&a), shown(&b)), (3, 3));
    assert!(!pw.config_form(&b, &fast, None).unwrap().own);

    pw.apply(&pw.plan_deploy(&a).await.unwrap()).unwrap();
    assert_eq!(
        fs::read_to_string(game.join("config/fast.cfg")).unwrap(),
        "[speed]\nlevel = 3\n"
    );
    // settings changed in game are the profile's own
    fs::write(game.join("config/fast.cfg"), "[speed]\nlevel = 8\n").unwrap();
    assert_eq!((shown(&a), shown(&b)), (8, 3));
    assert_eq!(
        pw.load_set(set.id).unwrap().unwrap().config[&fast]["speed"]["level"],
        Value::Int(3)
    );

    // going back keeps config edits as they are: the set's aren't copied
    // into the profile, and the profile's own stay its own
    pw.set_config(&a, &fast, Some(&level(4)), None).unwrap();
    pw.revert(&a).unwrap();
    assert_eq!(shown(&a), 4);
    let stored = fs::read_to_string(pw.profile_file(&a)).unwrap();
    assert!(!stored.contains("speed"), "{stored}");
    assert!(stored.contains("video"), "{stored}");

    // with its mod off, the set's settings are no reason to warn
    pw.set_enabled(&b, "Fast", false).unwrap();
    pw.lock(&b, false).await.unwrap();
    let plan = pw.plan_deploy(&b).await.unwrap();
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);

    // unlinked without its mods, its settings go; and with only `a` left,
    // the set's its own folder again, its settings with it
    pw.unlink_set(&b, set.id, false).unwrap();
    assert!(pw.load_profile(&b).unwrap().config.is_empty());
    let in_a = pw.load_profile(&a).unwrap();
    assert!(in_a.includes.is_empty());
    assert_eq!(in_a.config[&fast]["speed"]["level"], Value::Int(4));
    assert_eq!(in_a.config[&own]["video"]["fov"], Value::Int(7));

    // copied in, they're the profile's own, under what it has
    let folder = in_a.find_folder("QOL").unwrap().id;
    let from = FolderSource::Folder {
        profile: a.uuid,
        folder,
    };
    pw.use_folder(&b, &from, false).unwrap();
    assert_eq!(
        pw.load_profile(&b).unwrap().config[&fast]["speed"]["level"],
        Value::Int(4)
    );
}

#[tokio::test]
async fn a_profile_uses_another_profiles_folder_as_the_same_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, a, b) = two_profiles(tmp.path()).await;
    let qol = pw
        .add_folder(&a, "QOL", &["b".to_owned(), "c".to_owned()])
        .unwrap();
    let others = pw.other_folders(&b).unwrap();
    let from = FolderSource::Folder {
        profile: a.uuid,
        folder: qol.id,
    };
    assert_eq!(others.len(), 1, "{others:?}");
    assert_eq!(others[0].source, from);
    assert_eq!(others[0].mods, ["b", "c"]);
    assert_eq!(others[0].profiles, ["a"]);
    assert!(others[0].already.is_empty());
    assert_eq!(others[0].copy_only, None);
    // none of its own
    assert!(pw.other_folders(&a).unwrap().is_empty());

    pw.use_folder(&b, &from, true).unwrap();
    let set = pw.find_set("my-game", "QOL").unwrap();
    for p in [&a, &b] {
        let loaded = pw.load_profile(p).unwrap();
        assert!(loaded.is_linked(loaded.find("c").unwrap()), "{p}");
    }
    assert_eq!(mod_names(&pw, &b), ["d", "b", "c"]);
    // in it already, so there's nothing more to use
    assert!(pw.other_folders(&b).unwrap().is_empty());

    // a third profile sees the set once, with both that have it, and
    // which of its mods it has already
    let c = pw.create_profile("my-game", "c").unwrap();
    let b_mod = ModSpec {
        source: "versioned".to_owned(),
        id: "b".to_owned(),
        version: None,
    };
    pw.add_mod(&c, b_mod, None, true).await.unwrap();
    let others = pw.other_folders(&c).unwrap();
    assert_eq!(others.len(), 1, "{others:?}");
    assert_eq!(others[0].source, FolderSource::Set { set: set.id });
    assert_eq!(others[0].profiles, ["a", "b"]);
    assert_eq!(others[0].already, ["b"]);

    // or copies it in, which leaves it as it is
    pw.use_folder(&c, &others[0].source, false).unwrap();
    let loaded = pw.load_profile(&c).unwrap();
    assert!(loaded.includes.is_empty());
    assert_eq!(
        loaded.folder_of(loaded.find("c").unwrap()).unwrap().name,
        "QOL"
    );
    assert_eq!(pw.profiles_including(set.id).unwrap().len(), 2);
}

#[tokio::test]
async fn synced_profiles_use_other_profiles_folders_personally() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, a, b) = two_profiles(tmp.path()).await;
    let qol = pw.add_folder(&a, "QOL", &["b".to_owned()]).unwrap();
    pw.start_sync(&b).unwrap();
    let others = pw.other_folders(&b).unwrap();
    assert!(others[0].copy_only.is_none(), "{others:?}");
    assert!(others[0].personal, "{others:?}");
    let from = FolderSource::Folder {
        profile: a.uuid,
        folder: qol.id,
    };
    pw.use_folder(&b, &from, true).unwrap();
    let set = pw.find_set("my-game", "QOL").unwrap();
    // a shares it as ever, and b has it only here
    assert!(
        pw.load_profile(&a)
            .unwrap()
            .include(set.id)
            .is_some_and(|i| !i.personal)
    );
    let loaded = pw.load_profile(&b).unwrap();
    assert!(loaded.include(set.id).is_some_and(|i| i.personal));
    assert_eq!(mod_names(&pw, &b), ["d", "b"]);
    assert!(loaded.is_personal(loaded.find("b").unwrap()));
    let mut both = vec![a.clone(), b.clone()];
    both.sort();
    assert_eq!(pw.profiles_including(set.id).unwrap(), both);
    // so nothing's published
    assert!(pw.unsynced_changes(&b).unwrap().is_empty());

    // a set only a synced profile includes, personally, stays a set
    pw.unlink_set(&a, set.id, true).unwrap();
    assert!(pw.load_set(set.id).unwrap().is_some());
    assert_eq!(
        pw.profiles_including(set.id).unwrap(),
        std::slice::from_ref(&b)
    );
    // and stopping sharing it keeps its mods personal
    pw.unlink_set(&b, set.id, true).unwrap();
    assert!(pw.load_set(set.id).unwrap().is_none());
    let loaded = pw.load_profile(&b).unwrap();
    let m = loaded.find("b").unwrap();
    assert!(loaded.is_personal(m));
    assert_eq!(loaded.folder_of(m).unwrap().name, "Personal");
    assert!(pw.unsynced_changes(&b).unwrap().is_empty());

    // a synced profile's own folders can't be used as the same one
    // elsewhere, since they go to everyone
    pw.add_folder(&b, "Mine", &["d".to_owned()]).unwrap();
    let c = pw.create_profile("my-game", "c").unwrap();
    let theirs = pw.load_profile(&b).unwrap().find_folder("Mine").unwrap().id;
    let from = FolderSource::Folder {
        profile: b.uuid,
        folder: theirs,
    };
    let others = pw.other_folders(&c).unwrap();
    let mine = others.iter().find(|f| f.name == "Mine").unwrap();
    assert!(mine.copy_only.is_some() && !mine.personal, "{others:?}");
    let err = pw.use_folder(&c, &from, true).unwrap_err();
    let Some(crate::fix::Fix::CopyFolder { from: id, name, .. }) = err.fix() else {
        panic!("{err:?}");
    };
    assert_eq!((id, name), (b.uuid.to_string(), "Mine".to_owned()));
    // before anything changed
    assert!(pw.find_set("my-game", "Mine").is_err());
    pw.use_folder(&c, &from, false).unwrap();
    assert_eq!(mod_names(&pw, &c), ["d"]);
}

#[tokio::test]
async fn a_set_only_one_profile_includes_is_its_own_folder_again() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, a, b) = two_profiles(tmp.path()).await;
    pw.add_folder(&a, "QOL", &["b".to_owned()]).unwrap();
    let set = pw.share_folder(&a, "QOL").unwrap();
    pw.include_set(&b, set.id, None).unwrap();
    pw.set_enabled(&b, "b", false).unwrap();
    assert!(pw.remove_set(set.id).is_err());
    // unlinked from `a`, keeping a copy as its own folder
    pw.unlink_set(&a, set.id, true).unwrap();
    let own = |p: &ProfileRef| {
        let loaded = pw.load_profile(p).unwrap();
        let m = loaded.find("b").unwrap();
        assert!(!loaded.is_linked(m));
        assert!(loaded.includes.is_empty());
        (loaded.folder_of(m).unwrap().name.clone(), m.enabled)
    };
    assert_eq!(mod_names(&pw, &a), ["a", "b", "c"]);
    assert_eq!(own(&a), ("QOL".to_owned(), true));
    assert!(
        fs::read_to_string(pw.profile_file(&a))
            .unwrap()
            .contains("schema = 3")
    );
    // and `b`, the only one left, has it as its own too, as it was there
    assert_eq!(own(&b), ("QOL".to_owned(), false));
    assert!(pw.sets("my-game").unwrap().is_empty());

    // as when the profile's deleted instead
    let set = pw.share_folder(&a, "QOL").unwrap();
    pw.include_set(&b, set.id, None).unwrap();
    pw.remove_profile(&a).unwrap();
    assert_eq!(own(&b).0, "QOL");
    assert!(pw.sets("my-game").unwrap().is_empty());

    // and taken out of the last profile that has it, it's gone
    let set = pw.share_folder(&b, "QOL").unwrap();
    pw.unlink_set(&b, set.id, false).unwrap();
    assert!(pw.sets("my-game").unwrap().is_empty());
}

#[tokio::test]
async fn a_set_and_a_profile_with_the_same_mod_lock_it_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, a, b) = two_profiles(tmp.path()).await;
    pw.add_folder(&a, "QOL", &["a".to_owned(), "b".to_owned()])
        .unwrap();
    let set = pw.share_folder(&a, "QOL").unwrap();
    pw.add_mod(
        &b,
        ModSpec {
            source: "versioned".to_owned(),
            id: "a".to_owned(),
            version: None,
        },
        None,
        true,
    )
    .await
    .unwrap();
    pw.include_set(&b, set.id, Some("d")).unwrap();
    // the profile's own `a` wins
    assert_eq!(mod_names(&pw, &b), ["d", "b", "a"]);
    let loaded = pw.load_profile(&b).unwrap();
    assert!(!loaded.is_linked(loaded.find("a").unwrap()));
    let lock = pw.lock(&b, false).await.unwrap();
    assert_eq!(lock.mods.len(), 3);
    // and the set keeps its own
    pw.set_enabled(&b, "b", false).unwrap();
    assert_eq!(pw.find_set("my-game", "QOL").unwrap().mods.len(), 2);
}

#[tokio::test]
async fn synced_profiles_include_sets_personally() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, a, b) = two_profiles(tmp.path()).await;
    pw.add_folder(&a, "QOL", &["b".to_owned()]).unwrap();
    let set = pw.share_folder(&a, "QOL").unwrap();

    // one that includes a set can't start syncing
    let err = pw.start_sync(&a).unwrap_err();
    let Some(crate::fix::Fix::UnlinkSets { sets, names }) = err.fix() else {
        panic!("{err:?}");
    };
    assert_eq!(
        (sets, names),
        (vec![set.id.to_string()], vec!["QOL".to_owned()])
    );

    // but a synced one includes it in its personal layer
    pw.start_sync(&b).unwrap();
    pw.include_set(&b, set.id, None).unwrap();
    let loaded = pw.load_profile(&b).unwrap();
    assert!(loaded.include(set.id).is_some_and(|i| i.personal));
    assert!(loaded.personal().is_none());
    let m = loaded.find("b").unwrap();
    assert!(loaded.is_linked(m) && loaded.is_personal(m));
    assert_eq!(loaded.folder_of(m).unwrap().name, "QOL");
    let stored = fs::read_to_string(pw.profile_file(&b)).unwrap();
    assert!(!stored.contains("[[include]]"), "{stored}");
    let personal = pw.profile_state_dir(&b).join("personal.toml");
    let kept = fs::read_to_string(&personal).unwrap();
    assert!(kept.contains("schema = 2"), "{kept}");
    assert!(kept.contains(&set.id.to_string()), "{kept}");
    assert!(pw.unsynced_changes(&b).unwrap().is_empty());

    // its mods are locked apart from what's synced
    pw.lock(&b, false).await.unwrap();
    let shared = pw.load_shared_lock(&b).unwrap().unwrap();
    assert!(!shared.mods.iter().any(|m| m.id == "b"));
    assert!(
        pw.load_lock(&b)
            .unwrap()
            .unwrap()
            .mods
            .iter()
            .any(|m| m.id == "b")
    );
    let layer = pw.personal_layer(&b).unwrap().unwrap();
    assert!(layer.personal.mods.is_empty());
    assert_eq!(layer.personal.includes.len(), 1);
    assert!(layer.locked.iter().any(|m| m.id == "b"));

    // with personal mods of its own too, both keep their places
    pw.set_personal(&b, &["d".to_owned()], true).unwrap();
    assert_eq!(mod_names(&pw, &b), ["d", "b"]);
    let loaded = pw.load_profile(&b).unwrap();
    assert!(loaded.personal().is_some() && loaded.is_personal(loaded.find("d").unwrap()));
    assert_eq!(
        pw.personal_layer(&b)
            .unwrap()
            .unwrap()
            .personal
            .includes
            .len(),
        2
    );

    // a copy isn't synced, so it includes the set as its own
    let copy = pw.duplicate_profile(&b, "copy").unwrap();
    let loaded = pw.load_profile(&copy).unwrap();
    assert!(loaded.include(set.id).is_some_and(|i| !i.personal));
    assert!(!loaded.has_personal());
    assert_eq!(pw.profiles_including(set.id).unwrap().len(), 3);

    // and taking it out leaves just the personal mods
    pw.unlink_set(&b, set.id, false).unwrap();
    assert_eq!(mod_names(&pw, &b), ["d"]);
    let kept = fs::read_to_string(&personal).unwrap();
    assert!(kept.contains("schema = 1"), "{kept}");
}

#[tokio::test]
async fn going_back_asks_what_to_do_with_sets_changed_since() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, a, b) = two_profiles(tmp.path()).await;
    let game = tmp.path().join("game");
    fs::create_dir_all(&game).unwrap();
    pw.set_game_path("my-game", &game).unwrap();
    pw.add_folder(&a, "QOL", &["b".to_owned()]).unwrap();
    let set = pw.share_folder(&a, "QOL").unwrap();
    pw.include_set(&b, set.id, None).unwrap();
    pw.apply(&pw.plan_deploy(&a).await.unwrap()).unwrap();
    assert!(!pw.has_changes(&a).unwrap());

    // changed from the other profile
    pw.set_priority(&b, "b", 3).unwrap();
    assert!(pw.has_changes(&a).unwrap());
    let err = pw.revert(&a).unwrap_err();
    let fixes = err.blocker().unwrap().fixes;
    assert_eq!(fixes.len(), 3, "{fixes:?}");
    assert_eq!(
        fixes[0],
        crate::fix::Fix::GoBack {
            number: None,
            sets: crate::fix::SetsThen::Restore
        }
    );
    let priority = |p: &ProfileRef| {
        let loaded = pw.load_profile(p).unwrap();
        let m = loaded.find("b").unwrap();
        (m.priority, loaded.is_linked(m))
    };

    // keeping it as it is
    pw.set_enabled(&a, "c", false).unwrap();
    pw.go_back(&a, None, Some(crate::fix::SetsThen::Keep))
        .unwrap();
    assert!(pw.load_profile(&a).unwrap().find("c").unwrap().enabled);
    assert_eq!(priority(&a), (3, true));

    // putting it back, for both
    pw.go_back(&a, None, Some(crate::fix::SetsThen::Restore))
        .unwrap();
    assert_eq!(priority(&a), (0, true));
    assert_eq!(priority(&b), (0, true));
    assert!(!pw.has_changes(&a).unwrap());

    // copying it in as it was
    pw.set_priority(&b, "b", 5).unwrap();
    pw.go_back(&a, None, Some(crate::fix::SetsThen::Copy))
        .unwrap();
    assert_eq!(priority(&a), (0, false));
    // which leaves the set only in `b`, so it's `b`'s own folder now
    assert_eq!(priority(&b), (5, false));
    assert!(pw.load_profile(&a).unwrap().includes.is_empty());
    assert!(pw.profiles_including(set.id).unwrap().is_empty());
    assert!(pw.sets("my-game").unwrap().is_empty());
}

#[tokio::test]
async fn a_missing_set_is_an_issue_until_its_include_goes() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, a, _) = two_profiles(tmp.path()).await;
    pw.add_folder(&a, "QOL", &["b".to_owned()]).unwrap();
    let set = pw.share_folder(&a, "QOL").unwrap();
    fs::remove_dir_all(pw.paths().sets().join(set.id.to_string())).unwrap();
    assert_eq!(mod_names(&pw, &a), ["a", "c"]);
    let issues = pw.issues(&a).unwrap();
    let missing = issues
        .iter()
        .find(|i| matches!(i.kind, crate::issue::IssueKind::MissingSet { .. }))
        .unwrap();
    let Some(crate::fix::Fix::ForgetSet { set: id }) = &missing.fix else {
        panic!("{missing:?}");
    };
    // kept until then, in case it comes back
    pw.set_enabled(&a, "c", false).unwrap();
    assert_eq!(pw.load_profile(&a).unwrap().includes.len(), 1);
    pw.unlink_set(&a, id.parse().unwrap(), false).unwrap();
    assert!(pw.load_profile(&a).unwrap().includes.is_empty());
}

/// A [`WithDependencies`] game, at version 1.0 until it's changed, with a
/// profile that has `lib` and `other`.
async fn with_dependencies(tmp: &Path) -> (Patchwork, std::sync::Arc<Versioned>, ProfileRef) {
    let (pw, source) = dependencies_game(tmp);
    let profile = pw.create_profile("my-game", "default").unwrap();
    for id in ["lib", "other"] {
        pw.add_mod(&profile, deps_spec(id), None, true)
            .await
            .unwrap();
    }
    (pw, source, profile)
}

/// A [`WithDependencies`] game, at version 1.0 until it's changed.
fn dependencies_game(tmp: &Path) -> (Patchwork, std::sync::Arc<Versioned>) {
    let dir = tmp.join("mods");
    fs::create_dir_all(&dir).unwrap();
    let source = std::sync::Arc::new(Versioned {
        dir,
        version: std::sync::Mutex::new("1.0".to_owned()),
    });
    let pw = Patchwork::builder(Paths::rooted(&tmp.join("pw")))
        .source(WithDependencies(source.clone()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    (pw, source)
}

fn deps_spec(id: &str) -> ModSpec {
    ModSpec {
        source: "deps".to_owned(),
        id: id.to_owned(),
        version: None,
    }
}

#[tokio::test]
async fn personal_mods_stay_here_and_lock_on_top_of_whats_shared() {
    let tmp = tempfile::tempdir().unwrap();
    let (pw, source, profile) = with_dependencies(tmp.path()).await;
    let mine = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
    // a profile that isn't synced has nothing to keep apart
    assert!(pw.set_personal(&profile, &mine(&["lib"]), true).is_err());
    pw.lock(&profile, false).await.unwrap();
    pw.start_sync(&profile).unwrap();

    pw.add_mod(&profile, deps_spec("mine"), None, false)
        .await
        .unwrap();
    pw.set_personal(&profile, &mine(&["mine"]), true).unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    let m = loaded.find("mine").unwrap();
    assert!(loaded.is_personal(m) && !m.enabled);
    assert_eq!(loaded.folder_of(m).unwrap().name, "Personal");
    assert!(!loaded.is_personal(loaded.find("lib").unwrap()));
    // it's in its own file, not the profile's
    let stored = fs::read_to_string(pw.profile_file(&profile)).unwrap();
    assert!(!stored.contains("mine"), "{stored}");
    assert!(stored.contains("schema = 3"), "{stored}");
    // and it can't be renamed or removed like a folder
    assert!(pw.rename_folder(&profile, "Personal", "Mine").is_err());
    assert!(pw.remove_folder(&profile, "Personal").is_err());

    // so there's nothing to publish, or to say isn't shared
    assert!(pw.unsynced_changes(&profile).unwrap().is_empty());
    assert!(pw.unshared_mods(&profile).unwrap().is_empty());
    let lock = pw.lock(&profile, false).await.unwrap();
    assert_eq!(lock.mods.len(), 4);
    assert!(pw.unsynced_changes(&profile).unwrap().is_empty());
    let stored = fs::read_to_string(pw.lock_file(&profile)).unwrap();
    assert!(!stored.contains("mine"), "{stored}");
    let entries = pw.sync_entries(&profile).unwrap();
    assert!(!entries.keys().any(|k| k.contains("mine")));

    // what arrives from elsewhere leaves it be
    let mut doc = entries;
    let other = dependencies_game(&tmp.path().join("other")).0;
    other.apply_synced(&doc, true).unwrap();
    other
        .set_hidden_tags(&profile, &["library".to_owned()])
        .unwrap();
    doc.extend(other.unsynced_changes(&profile).unwrap());
    pw.apply_synced(&doc, true).unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    assert_eq!(loaded.hidden_tags, ["library"]);
    assert!(loaded.is_personal(loaded.find("mine").unwrap()));
    let lock = pw.load_lock(&profile).unwrap().unwrap();
    assert!(lock.mods.iter().any(|m| m.id == "mine"));

    // a personal mod can't move what's shared to a newer version
    *source.version.lock().unwrap() = "2.0".to_owned();
    pw.add_mod(&profile, deps_spec("app"), None, true)
        .await
        .unwrap();
    pw.set_personal(&profile, &mine(&["app"]), true).unwrap();
    let names = |pw: &Patchwork| mod_names(pw, &profile);
    assert_eq!(names(&pw), ["lib", "other", "mine", "app"]);
    let err = pw.lock(&profile, false).await.unwrap_err();
    let Some(crate::fix::Fix::RemoveMods { names }) = err.fix() else {
        panic!("{err:?}");
    };
    assert_eq!(names, ["app"]);
    let lib = |pw: &Patchwork| {
        let lock = pw.load_lock(&profile).unwrap().unwrap();
        let lib = lock.mods.iter().find(|m| m.id == "lib").unwrap();
        lib.version.clone().unwrap()
    };
    assert_eq!(lib(&pw), "1.0");

    // shared, it can
    pw.set_personal(&profile, &mine(&["app"]), false).unwrap();
    pw.lock(&profile, false).await.unwrap();
    assert_eq!(lib(&pw), "2.0");
    assert!(
        pw.unsynced_changes(&profile)
            .unwrap()
            .iter()
            .any(|(k, _)| k == "mod/deps:app")
    );

    // a copy isn't synced, so they're its own
    let copy = pw.duplicate_profile(&profile, "copy").unwrap();
    let loaded = pw.load_profile(&copy).unwrap();
    let m = loaded.find("mine").unwrap();
    assert!(!loaded.is_linked(m));
    assert_eq!(loaded.folder_of(m).unwrap().name, "Personal");
    assert_eq!(pw.load_lock(&copy).unwrap().unwrap().mods.len(), 5);

    // and taking the last one out leaves no personal layer
    pw.set_personal(&profile, &mine(&["mine"]), false).unwrap();
    let loaded = pw.load_profile(&profile).unwrap();
    assert!(loaded.personal().is_none());
    assert!(
        !pw.profile_state_dir(&profile)
            .join("personal.toml")
            .exists()
    );
    pw.lock(&profile, false).await.unwrap();
    assert!(
        !pw.profile_state_dir(&profile)
            .join("personal.lock")
            .exists()
    );
    assert!(
        fs::read_to_string(pw.lock_file(&profile))
            .unwrap()
            .contains("mine")
    );
}
