use patchwork_core::game::GameDef;

use super::*;

fn game() -> GameDef {
    toml::from_str(
        r#"
        schema = 1
        id = "core-keeper"
        name = "Core Keeper"

        [sources.modio]
        game = "corekeeper"
        id = 5289

        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap()
}

fn ctx<'a>(game: &'a GameDef, settings: Option<&'a toml::Table>) -> SourceContext<'a> {
    SourceContext {
        game,
        config: game.sources.get(ID),
        settings,
        env: None,
    }
}

// trimmed from the shape of mod.io's Mod object
const MOD: &str = r#"{
    "id": 4321,
    "game_id": 5289,
    "name": "Cool Mod",
    "name_id": "cool-mod",
    "summary": "Makes things cooler.",
    "profile_url": "https://mod.io/g/corekeeper/m/cool-mod",
    "logo": {"filename": "logo.png", "thumb_320x180": "https://thumb/logo.png"},
    "submitted_by": {"id": 1, "username": "someone", "profile_url": "https://mod.io/u/someone"},
    "date_updated": 1760000000,
    "dependencies": true,
    "modfile": {
        "id": 99,
        "mod_id": 4321,
        "date_added": 1760000000,
        "filename": "cool-mod-1.2.zip",
        "version": "1.2",
        "changelog": "Cooler.",
        "filehash": {"md5": "abc"},
        "download": {"binary_url": "https://api.mod.io/v1/games/5289/mods/4321/files/99/download", "date_expires": 1760003600}
    },
    "stats": {"mod_id": 4321, "downloads_total": 1234, "ratings_positive": 56},
    "tags": [{"name": "Items", "name_localized": "Items"}],
    "platforms": []
}"#;

fn files() -> Vec<Modfile> {
    serde_json::from_str(
        r#"[
            {"id": 97, "date_added": 1700000000, "filename": "a.zip", "version": "1.0", "changelog": "First."},
            {"id": 99, "date_added": 1760000000, "filename": "c.zip", "version": "1.2", "changelog": "Cooler."},
            {"id": 98, "date_added": 1750000000, "filename": "b.zip", "version": "", "changelog": null}
        ]"#,
    )
    .unwrap()
}

#[test]
fn reads_a_mod() {
    let m: Mod = serde_json::from_str(MOD).unwrap();
    let file = m.modfile.clone().unwrap();
    let deps = vec![DependencyMod {
        name_id: "core-lib".to_owned(),
    }];
    let resolved = to_resolved(&m, &file, deps);
    assert_eq!(resolved.id, "cool-mod");
    assert_eq!(resolved.name, "cool-mod");
    assert_eq!(resolved.version.as_deref(), Some("1.2"));
    assert_eq!(
        resolved.artifact,
        ArtifactLocation::Source {
            file: "99".to_owned(),
            name: "cool-mod-1.2.zip".to_owned(),
        }
    );
    assert_eq!(resolved.format, ArtifactFormat::Archive);
    assert_eq!(resolved.dependencies.len(), 1);
    assert_eq!(resolved.dependencies[0].source, "modio");
    assert_eq!(resolved.dependencies[0].id, "core-lib");
    assert!(resolved.dependencies[0].req.is_any());

    let summary = mod_summary("corekeeper", m);
    assert_eq!(summary.id, "cool-mod");
    assert_eq!(summary.name, "Cool Mod");
    assert_eq!(summary.author.as_deref(), Some("someone"));
    assert_eq!(summary.downloads, Some(1234));
    assert_eq!(summary.rating, Some(56));
    assert_eq!(summary.latest_version.as_deref(), Some("1.2"));
    assert_eq!(summary.icon_url.as_deref(), Some("https://thumb/logo.png"));
    assert_eq!(summary.categories[0].id, "Items");
    assert!(summary.updated.is_some());
}

#[test]
fn reads_a_game() {
    let game: Game = serde_json::from_str(
        r#"{
            "id": 5289, "name": "Core Keeper", "name_id": "corekeeper",
            "api_access_options": 3,
            "tag_options": [
                {"name": "Type", "type": "checkboxes", "tags": ["Items", "Maps"], "hidden": false},
                {"name": "Internal", "type": "dropdown", "tags": ["Secret"], "hidden": true}
            ]
        }"#,
    )
    .unwrap();
    assert_eq!(game.api_access_options & ACCESS_OPEN, ACCESS_OPEN);
    assert_eq!(game.tag_options.len(), 2);
    assert!(game.tag_options[1].hidden);
}

#[test]
fn versions_fall_back_to_file_ids() {
    let files = files();
    assert_eq!(file_version(&files[0]), "1.0");
    assert_eq!(file_version(&files[2]), "98");
}

#[test]
fn picks_files() {
    let files = files();
    assert_eq!(pick(&files, "1.2").map(|f| f.id), Some(99));
    assert_eq!(pick(&files, "98").map(|f| f.id), Some(98));
    // a file id finds its file even when that file has a version
    assert_eq!(pick(&files, "97").map(|f| f.id), Some(97));
    assert_eq!(pick(&files, "2.0").map(|f| f.id), None);
}

#[test]
fn compares_versions() {
    let modio = ModIo;
    assert_eq!(modio.compare_versions("1.2", "v1.10"), Some(Ordering::Less));
    assert_eq!(
        modio.compare_versions("beta", "beta"),
        Some(Ordering::Equal)
    );
    assert_eq!(modio.compare_versions("beta", "1.0"), None);
}

#[test]
fn urls() {
    let game = game();
    let ctx = ctx(&game, None);
    let url = ModIo.mod_url(&ctx, "cool-mod", Some("1.2")).unwrap();
    assert_eq!(url, "https://mod.io/g/corekeeper/m/cool-mod");
    let spec = ModIo.parse_url(&ctx, &url).unwrap().unwrap();
    assert_eq!(spec.id, "cool-mod");
    assert_eq!(spec.source, "modio");
    // tabs on the page, a trailing slash and www. are fine
    let spec = ModIo
        .parse_url(&ctx, "https://www.mod.io/g/corekeeper/m/cool-mod/comments/")
        .unwrap()
        .unwrap();
    assert_eq!(spec.id, "cool-mod");
    assert!(
        ModIo
            .parse_url(&ctx, "https://mod.io/g/corekeeper")
            .unwrap()
            .is_none()
    );
    assert!(
        ModIo
            .parse_url(&ctx, "https://example.com/g/x/m/y")
            .unwrap()
            .is_none()
    );
    assert!(
        ModIo
            .parse_url(&ctx, "https://mod.io/g/othergame/m/y")
            .is_err()
    );
}

#[test]
fn search_params_say_what_was_asked() {
    let query = SearchQuery {
        text: Some(" sword ".to_owned()),
        sort: None,
        categories: vec!["Items".to_owned(), "Maps".to_owned()],
        excluded_categories: vec!["NSFW".to_owned()],
        page: 2,
    };
    let params = search_params(&query, SortBy::Updated);
    let get = |k: &str| {
        params
            .iter()
            .find(|(p, _)| *p == k)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("_sort"), Some("-date_updated"));
    assert_eq!(get("_limit"), Some("20"));
    assert_eq!(get("_offset"), Some("40"));
    assert_eq!(get("_q"), Some("sword"));
    assert_eq!(get("tags-in"), Some("Items,Maps"));
    assert_eq!(get("tags-not-in"), Some("NSFW"));

    let params = search_params(&SearchQuery::default(), SortBy::Downloads);
    assert!(!params.iter().any(|(p, _)| *p == "_q" || *p == "tags-in"));
}

#[test]
fn reads_config() {
    let game = game();
    let config = config(&ctx(&game, None)).unwrap();
    assert_eq!(config.game, "corekeeper");
    assert_eq!(config.id, Some(5289));
    assert_eq!(config.platform, "windows");
}

#[tokio::test]
async fn needs_an_api_key() {
    let game = game();
    let empty = toml::toml! { api_key = "  " };
    for settings in [None, Some(&empty)] {
        let err = ModIo
            .resolve(&ctx(&game, settings), "cool-mod", None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("API key"), "{err}");
    }
}

#[tokio::test]
async fn games_without_it_say_so() {
    let mut game = game();
    game.sources.clear();
    let key = toml::toml! { api_key = "key" };
    let err = ModIo
        .resolve(&ctx(&game, Some(&key)), "cool-mod", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("isn't on mod.io"), "{err}");
}
