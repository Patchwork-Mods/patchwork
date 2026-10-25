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

#[test]
fn duplicating_a_profile_copies_its_local_files() {
    let tmp = tempfile::tempdir().unwrap();
    let pw = Patchwork::builder(Paths::rooted(tmp.path()))
        .deployer(NoDeployer)
        .open()
        .unwrap();
    pw.add_game(new_game("my-game", "none")).unwrap();
    let profile = pw.create_profile("my-game", "default").unwrap();
    // as if it had been imported with a local mod
    let file = pw.profile_dir(&profile).join("files/abc/mod.zip");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, b"mod bytes").unwrap();
    let mut loaded = pw.load_profile(&profile).unwrap();
    loaded.mods.push(ProfileMod {
        name: "mod".to_owned(),
        source: "local".to_owned(),
        id: file.to_string_lossy().into_owned(),
        version: None,
        enabled: false,
        game_versions: None,
    });
    loaded.hidden_tags = vec!["library".to_owned()];
    pw.save_profile(&profile, &loaded).unwrap();

    let copy = pw.duplicate_profile(&profile, "copy").unwrap();
    assert_ne!(copy, profile);
    assert_eq!(pw.find_profile("my-game/copy").unwrap(), copy);
    let copied = pw.load_profile(&copy).unwrap();
    assert_eq!(copied.hidden_tags, loaded.hidden_tags);
    assert!(!copied.mods[0].enabled);
    let copied_file = pw.profile_dir(&copy).join("files/abc/mod.zip");
    assert_eq!(copied.mods[0].id, copied_file.to_string_lossy());
    assert_eq!(fs::read(&copied_file).unwrap(), b"mod bytes");

    // the copy stands on its own
    pw.remove_profile(&profile).unwrap();
    assert!(copied_file.is_file());
    let taken = pw.duplicate_profile(&copy, "copy").unwrap_err();
    assert!(taken.to_string().contains("already exists"), "{taken}");
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
