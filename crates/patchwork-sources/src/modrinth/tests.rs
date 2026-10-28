use std::collections::BTreeMap;

use patchwork_core::environment::GameEnv;

use super::*;

fn minecraft() -> GameDef {
    toml::from_str(
        r#"
        schema = 1
        id = "minecraft"
        name = "Minecraft"

        [sources.modrinth]
        project_types = ["mod", "resourcepack"]
        loaders.quilt = ["quilt", "fabric"]

        [env.version]
        name = "Minecraft version"

        [env.loader]
        name = "Mod loader"
        choices = ["fabric", "quilt", "forge", "neoforge"]
        default = "fabric"

        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap()
}

fn env(version: Option<&str>, loader: &str) -> GameEnv {
    GameEnv {
        version: version.map(str::to_owned),
        axes: BTreeMap::from([(LOADER.to_owned(), loader.to_owned())]),
        ..Default::default()
    }
}

fn ctx<'a>(game: &'a GameDef, env: Option<&'a GameEnv>) -> SourceContext<'a> {
    SourceContext {
        game,
        config: game.sources.get(ID),
        settings: None,
        env,
    }
}

// trimmed from what /v2/project/sodium returns
const PROJECT: &str = r#"{
    "id": "AANobbMI",
    "slug": "sodium",
    "title": "Sodium",
    "description": "A high-performance rendering engine replacement for Minecraft.",
    "project_type": "mod",
    "icon_url": "https://cdn.modrinth.com/data/AANobbMI/icon.webp",
    "downloads": 256684799,
    "followers": 42251,
    "updated": "2026-10-15T17:51:44.107526Z",
    "categories": ["optimization"],
    "loaders": ["fabric", "neoforge", "quilt"]
}"#;

const VERSION: &str = r#"{
    "id": "SMxNOGZ6",
    "project_id": "AANobbMI",
    "name": "Sodium 0.8.13 for Fabric 1.21.1",
    "version_number": "mc1.21.1-0.8.13-fabric",
    "changelog": "Fixes things.",
    "date_published": "2026-08-28T20:55:02.424329Z",
    "version_type": "release",
    "game_versions": ["1.21", "1.21.1"],
    "loaders": ["fabric", "quilt"],
    "files": [
        {
            "url": "https://cdn.modrinth.com/data/AANobbMI/versions/SMxNOGZ6/sources.jar",
            "filename": "sodium-sources.jar",
            "primary": false
        },
        {
            "url": "https://cdn.modrinth.com/data/AANobbMI/versions/SMxNOGZ6/sodium-fabric-0.8.13%2Bmc1.21.1.jar",
            "filename": "sodium-fabric-0.8.13+mc1.21.1.jar",
            "primary": true
        }
    ],
    "dependencies": [
        {"version_id": null, "project_id": "P7dR8mSH", "file_name": null, "dependency_type": "required"},
        {"version_id": "abcd1234", "project_id": "eXts2L7r", "file_name": null, "dependency_type": "required"},
        {"version_id": "efgh5678", "project_id": null, "file_name": null, "dependency_type": "required"},
        {"version_id": null, "project_id": "optional1", "file_name": null, "dependency_type": "optional"},
        {"version_id": null, "project_id": "embedded1", "file_name": null, "dependency_type": "embedded"},
        {"version_id": null, "project_id": null, "file_name": "lib.jar", "dependency_type": "required"}
    ]
}"#;

fn version(number: &str, kind: &str) -> Version {
    let mut v: Version = serde_json::from_str(VERSION).unwrap();
    v.id = format!("id-{number}");
    v.version_number = number.to_owned();
    v.version_type = kind.to_owned();
    v
}

#[test]
fn converts_a_version() {
    let game = minecraft();
    let project: Project = serde_json::from_str(PROJECT).unwrap();
    let version: Version = serde_json::from_str(VERSION).unwrap();
    let numbers = HashMap::from([
        (
            "abcd1234".to_owned(),
            ("eXts2L7r".to_owned(), "11.0.5".to_owned()),
        ),
        (
            "efgh5678".to_owned(),
            ("Ha28R6CL".to_owned(), "2.0".to_owned()),
        ),
    ]);
    let config = config(&ctx(&game, None)).unwrap();
    let resolved = to_resolved(&game, &config, &project, version, &numbers).unwrap();

    assert_eq!(resolved.id, "AANobbMI");
    assert_eq!(resolved.name, "sodium");
    assert_eq!(resolved.version.as_deref(), Some("mc1.21.1-0.8.13-fabric"));
    assert_eq!(resolved.format, ArtifactFormat::File);
    // the primary file, not the first
    assert_eq!(
        resolved.artifact.file_name(),
        "sodium-fabric-0.8.13+mc1.21.1.jar"
    );
    assert_eq!(
        resolved.dependencies,
        vec![
            Dependency {
                source: ID.to_owned(),
                id: "P7dR8mSH".to_owned(),
                req: VersionReq::Any,
            },
            Dependency {
                source: ID.to_owned(),
                id: "eXts2L7r".to_owned(),
                req: VersionReq::Exact("11.0.5".to_owned()),
            },
            // the project comes from the version it asks for
            Dependency {
                source: ID.to_owned(),
                id: "Ha28R6CL".to_owned(),
                req: VersionReq::Exact("2.0".to_owned()),
            },
        ]
    );
    let supports = resolved.game_versions.unwrap();
    assert_eq!(supports.matches("1.21.1"), Some(true));
    assert_eq!(supports.matches("1.21.2"), Some(false));
    let loaders: Vec<_> = resolved.env.iter().collect();
    assert_eq!(
        loaders,
        [(LOADER, ["fabric".to_owned(), "quilt".to_owned()].as_slice())]
    );
}

#[test]
fn only_the_games_mod_loaders_count() {
    let game = minecraft();
    let mut project: Project = serde_json::from_str(PROJECT).unwrap();
    // a datapack that's also packed as a mod
    project.loaders = vec!["datapack".into(), "fabric".into()];
    let e = env(Some("1.21.1"), "fabric");
    let ctx = ctx(&game, Some(&e));
    let config = config(&ctx).unwrap();
    assert_eq!(
        Filter::new(&ctx, &config, Some(&project.loaders))
            .loader
            .as_deref(),
        Some("fabric")
    );
    // a datapack, resource pack or shader runs on no loader at all
    project.loaders = vec!["datapack".into()];
    let filter = Filter::new(&ctx, &config, Some(&project.loaders));
    assert_eq!(
        filter,
        Filter {
            game_version: Some("1.21.1".into()),
            loader: None,
            accepts: Vec::new(),
        }
    );
    let mut version = version("1.0", "release");
    version.loaders = vec!["iris".into(), "optifine".into()];
    let resolved = to_resolved(&game, &config, &project, version, &HashMap::new()).unwrap();
    assert!(resolved.env.is_empty());
}

#[test]
fn quilt_runs_fabric_mods() {
    let game = minecraft();
    let e = env(Some("1.21.1"), "quilt");
    let ctx = ctx(&game, Some(&e));
    let config = config(&ctx).unwrap();
    let project: Project = serde_json::from_str(PROJECT).unwrap();
    let filter = Filter::new(&ctx, &config, Some(&project.loaders));
    assert_eq!(filter.accepts, ["quilt", "fabric"]);
    assert_eq!(filter.describe(&game), "Minecraft 1.21.1 with quilt");
    // so a fabric-only file works for quilt profiles too, but not forge ones
    let mut version = version("1.0", "release");
    version.loaders = vec!["fabric".into()];
    let resolved = to_resolved(&game, &config, &project, version, &HashMap::new()).unwrap();
    let loaders: Vec<_> = resolved.env.iter().collect();
    assert_eq!(
        loaders,
        [(LOADER, ["fabric".to_owned(), "quilt".to_owned()].as_slice())]
    );
    // loaders accept just their own files unless the game says otherwise
    assert_eq!(config.accepts("forge"), ["forge"]);
}

#[test]
fn picks_the_newest_release_unless_asked_for_another() {
    let versions = || {
        vec![
            version("3.0-beta", "beta"),
            version("2.0", "release"),
            version("1.0", "release"),
        ]
    };
    let number = |v: Option<Version>| v.map(|v| v.version_number);
    assert_eq!(number(pick(versions(), None)).as_deref(), Some("2.0"));
    assert_eq!(
        number(pick(versions(), Some("3.0-beta"))).as_deref(),
        Some("3.0-beta")
    );
    // by id too
    assert_eq!(
        number(pick(versions(), Some("id-1.0"))).as_deref(),
        Some("1.0")
    );
    assert_eq!(number(pick(versions(), Some("4.0"))), None);
    // betas are better than nothing
    let betas = vec![version("2.0-beta", "beta"), version("1.0-alpha", "alpha")];
    assert_eq!(number(pick(betas, None)).as_deref(), Some("2.0-beta"));
}

#[test]
fn the_game_version_has_to_be_picked_first() {
    let game = minecraft();
    let unpicked = env(None, "fabric");
    let err = check_version_picked(&ctx(&game, Some(&unpicked))).unwrap_err();
    assert!(err.to_string().contains("Minecraft version"), "{err}");
    assert!(check_version_picked(&ctx(&game, None)).is_err());
    let picked = env(Some("1.21.1"), "fabric");
    assert!(check_version_picked(&ctx(&game, Some(&picked))).is_ok());
}

#[test]
fn searches_only_need_mods_to_run_on_the_loader() {
    let types = vec!["mod".to_owned(), "resourcepack".to_owned()];
    let filter = Filter {
        game_version: Some("1.21.1".into()),
        loader: Some("quilt".into()),
        accepts: vec!["quilt".into(), "fabric".into()],
    };
    let query = SearchQuery {
        categories: vec!["optimization".into(), "utility".into()],
        excluded_categories: vec!["cursed".into()],
        ..Default::default()
    };
    assert_eq!(
        facets(&types, &filter, &query),
        vec![
            vec!["project_type:mod", "project_type:resourcepack"],
            vec!["versions:1.21.1"],
            vec![
                "categories:quilt",
                "categories:fabric",
                "project_type:resourcepack"
            ],
            vec!["categories:optimization", "categories:utility"],
            vec!["categories!=cursed"],
        ]
    );
    // without mods, the loader doesn't matter
    let packs = vec!["resourcepack".to_owned()];
    assert_eq!(facets(&packs, &filter, &SearchQuery::default()).len(), 2);
}

#[test]
fn recognizes_project_urls() {
    let parse = |url| parse_project_url(url);
    assert_eq!(
        parse("https://modrinth.com/mod/sodium"),
        Some(("sodium".into(), None))
    );
    assert_eq!(
        parse("https://modrinth.com/shader/complementary-reimagined/version/r5.5.1"),
        Some(("complementary-reimagined".into(), Some("r5.5.1".into())))
    );
    assert_eq!(
        parse("https://modrinth.com/mod/sodium/versions?l=fabric"),
        Some(("sodium".into(), None))
    );
    assert_eq!(parse("https://modrinth.com/user/jellysquid3"), None);
    assert_eq!(parse("https://example.com/mod/sodium"), None);
}

#[test]
fn compares_numbered_versions_only() {
    let m = Modrinth;
    assert_eq!(m.compare_versions("v1.2", "1.10"), Some(Ordering::Less));
    assert_eq!(
        m.compare_versions("mc1.21-0.8", "mc1.21-0.8"),
        Some(Ordering::Equal)
    );
    assert_eq!(m.compare_versions("mc1.21-0.8", "mc1.21-0.9"), None);
}

#[test]
fn search_hits_leave_loaders_out_of_categories() {
    let hit: Hit = serde_json::from_str(
        r#"{
            "project_id": "AANobbMI",
            "slug": "sodium",
            "title": "Sodium",
            "description": "Fast.",
            "project_type": "mod",
            "author": "jellysquid3",
            "categories": ["fabric", "game-mechanics", "quilt"],
            "downloads": 10,
            "follows": 2,
            "icon_url": null,
            "date_modified": "2026-10-15T17:51:36.063140+00:00"
        }"#,
    )
    .unwrap();
    let summary = hit_to_summary(&minecraft(), hit);
    assert_eq!(
        summary.url.as_deref(),
        Some("https://modrinth.com/mod/sodium")
    );
    assert_eq!(
        summary.categories,
        vec![Category {
            id: "game-mechanics".into(),
            name: "Game mechanics".into(),
        }]
    );
}

#[tokio::test]
#[ignore = "talks to modrinth.com"]
async fn resolves_sodium_for_fabric() {
    let game = minecraft();
    let e = env(Some("1.21.1"), "fabric");
    let resolved = Modrinth
        .resolve(&ctx(&game, Some(&e)), "sodium", None)
        .await
        .unwrap();
    assert_eq!(resolved.id, "AANobbMI");
    assert!(resolved.version.unwrap().contains("fabric"));
    let e = env(Some("1.21.1"), "forge");
    let err = Modrinth
        .resolve(&ctx(&game, Some(&e)), "sodium", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("with forge"), "{err}");
}
