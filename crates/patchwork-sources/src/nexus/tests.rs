use super::search::{ModsPage, effective_sort, mods_filter, node_to_summary};
use super::*;
use patchwork_core::game::GameDef;
use serde_json::json;

fn settings_with_key() -> toml::Table {
    toml::toml! { api_key = "key" }
}

// trimmed responses for SkyUI (skyrimspecialedition/12604)
const MOD: &str = r#"{
    "name": "SkyUI",
    "summary": "Elegant, PC-friendly interface mod",
    "version": "5.2SE",
    "author": "SkyUI Team",
    "mod_id": 12604,
    "game_id": 1704,
    "domain_name": "skyrimspecialedition",
    "status": "published",
    "available": true
}"#;

const FILES: &str = r#"{
    "files": [
        {
            "file_id": 1000,
            "name": "SkyUI",
            "version": "5.1SE",
            "category_name": "OLD_VERSION",
            "is_primary": false,
            "file_name": "SkyUI_5_1_SE-12604-5-1SE.zip",
            "uploaded_timestamp": 1500000000
        },
        {
            "file_id": 2000,
            "name": "SkyUI",
            "version": "5.2SE",
            "category_name": "MAIN",
            "is_primary": true,
            "file_name": "SkyUI_5_2_SE-12604-5-2SE.zip",
            "uploaded_timestamp": 1510000000
        },
        {
            "file_id": 3000,
            "name": "SkyUI Extras",
            "version": "5.2SE",
            "category_name": "OPTIONAL",
            "is_primary": false,
            "file_name": "SkyUI_Extras.7z",
            "uploaded_timestamp": 1520000000
        },
        {
            "file_id": 4000,
            "name": "Something new",
            "version": "6.0",
            "category_name": "SOME_NEW_CATEGORY",
            "is_primary": false,
            "file_name": "new.zip",
            "uploaded_timestamp": 1530000000
        }
    ],
    "file_updates": []
}"#;

const LINKS: &str = r#"[{
    "name": "Nexus Global Content Delivery Network",
    "short_name": "Nexus CDN",
    "URI": "https://cf-files.nexusmods.com/cdn/1704/12604/SkyUI_5_2_SE-12604-5-2SE.zip?md5=abc&expires=1&user_id=1"
}]"#;

fn skyrim() -> GameDef {
    toml::from_str(
        r#"
        schema = 1
        id = "skyrim-se"
        name = "Skyrim Special Edition"

        [sources.nexus]
        game_domain = "skyrimspecialedition"

        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap()
}

#[test]
fn parses_api_responses() {
    let files: NexusFiles = serde_json::from_str(FILES).unwrap();
    assert_eq!(files.files[0].category_name, Some(FileCategory::OldVersion));
    assert_eq!(files.files[3].category_name, Some(FileCategory::Other));
    let links: Vec<NexusDownloadLink> = serde_json::from_str(LINKS).unwrap();
    assert!(links[0].uri.starts_with("https://cf-files.nexusmods.com/"));
}

#[test]
fn urls_round_trip() {
    let game = skyrim();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
        settings: None,
        env: None,
    };
    let nexus = Nexus::default();
    let url = nexus.mod_url(&ctx, "12604", Some("5.2SE")).unwrap();
    assert_eq!(
        url,
        "https://www.nexusmods.com/skyrimspecialedition/mods/12604"
    );
    let spec = nexus
        .parse_url(&ctx, &format!("{url}?tab=files"))
        .unwrap()
        .unwrap();
    assert_eq!((spec.id.as_str(), spec.version), ("12604", None));

    assert!(
        nexus
            .parse_url(&ctx, "https://thunderstore.io/c/x/p/a/b/")
            .unwrap()
            .is_none()
    );
    let err = nexus
        .parse_url(&ctx, "https://nexusmods.com/fallout4/mods/1")
        .unwrap_err();
    assert!(err.to_string().contains("fallout4"), "{err}");
}

#[test]
fn picks_files() {
    let files: NexusFiles = serde_json::from_str(FILES).unwrap();
    let pick = |v| pick_file(&files.files, v, |_| 1).map(|f| f.file_id);
    assert_eq!(pick(None), Some(2000));
    assert_eq!(pick(Some("5.2SE")), Some(2000));
    assert_eq!(pick(Some("5.1SE")), Some(1000));
    assert_eq!(pick(Some("9.9")), None);
}

/// Two main files, one for each runtime, as SKSE plugins tend to have.
const RUNTIME_FILES: &str = r#"{
    "files": [
        {
            "file_id": 1,
            "name": "Engine Fixes AE",
            "version": "7",
            "category_name": "MAIN",
            "is_primary": true,
            "file_name": "Engine Fixes AE-17230-7.zip",
            "uploaded_timestamp": 1700000000
        },
        {
            "file_id": 2,
            "name": "Engine Fixes",
            "version": "7",
            "category_name": "MAIN",
            "is_primary": false,
            "file_name": "Engine Fixes-17230-7.zip",
            "uploaded_timestamp": 1690000000,
            "description": "For Skyrim 1.5.97 only."
        }
    ]
}"#;

fn labelled_skyrim() -> GameDef {
    let mut game = skyrim();
    game.versions = toml::from_str(r#"labels = { AE = ">=1.6", "1.5.97" = "1.5.97" }"#).unwrap();
    game
}

#[test]
fn picks_the_file_for_the_installed_game() {
    let game = labelled_skyrim();
    let files: NexusFiles = serde_json::from_str(RUNTIME_FILES).unwrap();
    let pick = |installed: Option<&str>, version| {
        pick_file(&files.files, version, |f| fit(&game, installed, f)).map(|f| f.file_id)
    };
    assert_eq!(pick(Some("1.5.97"), None), Some(2));
    assert_eq!(pick(Some("1.6.1170"), None), Some(1));
    // without a version to go on, the primary file as before
    assert_eq!(pick(None, None), Some(1));
    assert_eq!(pick(Some("2.0.0.0.x"), None), Some(1));

    let versions = |i: usize| game_versions(&game, &files.files[i]).map(|v| v.to_string());
    assert_eq!(versions(0).as_deref(), Some(">=1.6"));
    assert_eq!(versions(1).as_deref(), Some("1.5.97"));
    // and nothing guessed for games without labels
    assert_eq!(game_versions(&skyrim(), &files.files[0]), None);
}

#[test]
fn converts_api_response() {
    let mod_: NexusMod = serde_json::from_str(MOD).unwrap();
    let files: NexusFiles = serde_json::from_str(FILES).unwrap();
    let resolved = nexus_to_resolved_mod(&skyrim(), &mod_, &files.files[1]);

    assert_eq!(resolved.id, "12604");
    assert_eq!(resolved.name, "SkyUI");
    assert_eq!(resolved.version.as_deref(), Some("5.2SE"));
    assert_eq!(resolved.format, ArtifactFormat::Archive);
    assert_eq!(
        resolved.artifact,
        ArtifactLocation::Source {
            file: "2000".to_owned(),
            name: "SkyUI_5_2_SE-12604-5-2SE.zip".to_owned(),
        }
    );
    assert_eq!(
        resolved.artifact.file_name(),
        "SkyUI_5_2_SE-12604-5-2SE.zip"
    );
    assert!(resolved.dependencies.is_empty());
    assert_eq!(resolved.game_versions, None);
}

#[test]
fn file_pages_open_the_file() {
    assert_eq!(
        file_page("skyrimspecialedition", "12604", "2000"),
        "https://www.nexusmods.com/skyrimspecialedition/mods/12604?tab=files&file_id=2000"
    );
}

#[test]
fn parses_nxm_links() {
    let link =
        "nxm://skyrimspecialedition/mods/12604/files/2000?key=a-B_c&expires=1700000000&user_id=1";
    assert_eq!(
        parse_nxm(link),
        Some(NxmLink {
            domain: "skyrimspecialedition",
            id: "12604",
            file: "2000",
            key: "a-B_c",
            expires: "1700000000",
        })
    );
    for bad in [
        "https://www.nexusmods.com/skyrimspecialedition/mods/12604",
        "nxm://skyrimspecialedition/mods/12604/files/2000",
        "nxm://skyrimspecialedition/mods/12604/files/2000?expires=1",
        "nxm://skyrimspecialedition/mods/SkyUI/files/2000?key=a&expires=1",
        "nxm://skyrimspecialedition/collections/abc/revisions/1?key=a&expires=1",
    ] {
        assert_eq!(parse_nxm(bad), None, "{bad}");
    }
}

#[tokio::test]
async fn ignores_links_for_other_games() {
    let game = skyrim();
    let settings = settings_with_key();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
        settings: Some(&settings),
        env: None,
    };
    // nothing is fetched, so this works offline
    let accepted = Nexus::default()
        .accept_link(&ctx, "nxm://fallout4/mods/1/files/2?key=a&expires=1")
        .await
        .unwrap();
    assert_eq!(accepted, None);
}

#[tokio::test]
async fn needs_game_domain_config() {
    let mut game = skyrim();
    game.sources.clear();
    let settings = settings_with_key();
    let ctx = SourceContext {
        game: &game,
        config: None,
        settings: Some(&settings),
        env: None,
    };
    let err = Nexus::default()
        .resolve(&ctx, "12604", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("isn't on Nexus"), "{err}");
}

#[tokio::test]
async fn needs_api_key() {
    let game = skyrim();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
        settings: None,
        env: None,
    };
    let err = Nexus::default()
        .resolve(&ctx, "12604", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("needs an API key"), "{err}");
}

#[tokio::test]
async fn rejects_non_numeric_ids() {
    let game = skyrim();
    let settings = settings_with_key();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
        settings: Some(&settings),
        env: None,
    };
    let err = Nexus::default()
        .resolve(&ctx, "SkyUI", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("numeric"), "{err}");
}

// This talks to nexusmods.com and needs NEXUS_API_KEY set; run it with
// `cargo test -- --ignored`.
#[tokio::test]
#[ignore = "needs network and a Nexus API key"]
async fn test_get_mod() {
    let game = skyrim();
    let mut settings = toml::Table::new();
    settings.insert(
        "api_key".to_owned(),
        std::env::var("NEXUS_API_KEY").unwrap().into(),
    );
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
        settings: Some(&settings),
        env: None,
    };
    let resolved = Nexus::default().resolve(&ctx, "12604", None).await.unwrap();
    assert_eq!(resolved.id, "12604");
    assert_eq!(resolved.name, "SkyUI");
    assert!(resolved.version.is_some());
}

// a trimmed v2 GraphQL `mods` result
const MODS_PAGE: &str = r#"{
    "totalCount": 218,
    "nodes": [{
        "modId": 12604,
        "name": "SkyUI",
        "summary": "Elegant, PC-friendly interface mod",
        "author": "SkyUI Team",
        "updatedAt": "2026-05-05T22:41:21Z",
        "downloads": 26132765,
        "endorsements": 505574,
        "thumbnailUrl": "https://staticdelivery.nexusmods.com/mods/1704/images/thumbnails/12604/12604.png",
        "version": "6.9",
        "modCategory": {"name": "User Interface"},
        "uploader": {"name": "schlangster"}
    }, {
        "modId": 1,
        "name": "Uncategorized",
        "summary": null,
        "author": null,
        "updatedAt": null,
        "downloads": null,
        "endorsements": null,
        "thumbnailUrl": null,
        "version": null,
        "modCategory": null,
        "uploader": null
    }]
}"#;

#[test]
fn converts_search_results() {
    let page: ModsPage = serde_json::from_str(MODS_PAGE).unwrap();
    assert_eq!(page.total_count, 218);
    let mut nodes = page.nodes.into_iter();
    let skyui = node_to_summary("skyrimspecialedition", nodes.next().unwrap());
    assert_eq!(skyui.source, "nexus");
    assert_eq!(skyui.id, "12604");
    assert_eq!(
        skyui.url.as_deref(),
        Some("https://www.nexusmods.com/skyrimspecialedition/mods/12604")
    );
    assert_eq!(
        skyui.author_url.as_deref(),
        Some("https://www.nexusmods.com/profile/schlangster/mods")
    );
    assert_eq!(skyui.rating, Some(505574));
    assert_eq!(skyui.latest_version.as_deref(), Some("6.9"));
    assert_eq!(skyui.categories[0].id, "User Interface");
    let bare = node_to_summary("skyrimspecialedition", nodes.next().unwrap());
    assert!(bare.categories.is_empty());
    assert_eq!(bare.author_url, None);
}

#[test]
fn builds_search_filters() {
    let home = SearchQuery::default();
    assert_eq!(
        mods_filter("skyrimspecialedition", &home),
        json!({
            "gameDomainName": [{ "value": "skyrimspecialedition", "op": "EQUALS" }],
            "adultContent": [{ "value": false, "op": "EQUALS" }],
            "filter": [],
        })
    );
    assert_eq!(effective_sort(&home), SortBy::Downloads);

    let query = SearchQuery {
        text: Some("skyui".to_owned()),
        categories: vec!["User Interface".to_owned(), "Audio".to_owned()],
        ..Default::default()
    };
    let filter = mods_filter("skyrimspecialedition", &query);
    let groups = filter["filter"].as_array().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0]["name"][0]["value"], "skyui");
    assert_eq!(groups[1]["op"], "OR");
    assert_eq!(groups[1]["categoryName"][1]["value"], "Audio");
    assert_eq!(effective_sort(&query), SortBy::Relevance);

    // leaving categories out has to rule out each one, so its group is ANDed
    let query = SearchQuery {
        excluded_categories: vec!["Utilities".to_owned(), "Modders Resources".to_owned()],
        ..Default::default()
    };
    let filter = mods_filter("skyrimspecialedition", &query);
    assert_eq!(
        filter["filter"],
        json!([{
            "op": "AND",
            "categoryName": [
                { "value": "Utilities", "op": "NOT_EQUALS" },
                { "value": "Modders Resources", "op": "NOT_EQUALS" },
            ],
        }])
    );
}

// Searching doesn't need an API key, but it does need the network.
#[tokio::test]
#[ignore = "needs network"]
async fn searches() {
    let game = skyrim();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
        settings: None,
        env: None,
    };
    let nexus = Nexus::default();
    let query = SearchQuery {
        text: Some("skyui".to_owned()),
        ..Default::default()
    };
    let found = nexus.search(&ctx, &query).await.unwrap();
    assert_eq!(found.sort, SortBy::Relevance);
    assert_eq!(found.items[0].id, "12604");

    let categories = nexus.search_categories(&ctx).await.unwrap();
    assert!(categories.iter().any(|c| c.id == "User Interface"));
}

#[test]
fn lists_file_versions_newest_first() {
    let files: NexusFiles = serde_json::from_str(FILES).unwrap();
    assert_eq!(file_versions(files.files), ["6.0", "5.2SE", "5.1SE"]);
}

#[tokio::test]
#[ignore = "needs network"]
async fn summarizes_one_mod() {
    let game = skyrim();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
        settings: None,
        env: None,
    };
    let nexus = Nexus::default();
    let skyui = nexus.summary(&ctx, "12604").await.unwrap().unwrap();
    assert_eq!(skyui.name, "SkyUI");
    assert!(skyui.icon_url.is_some());
    assert!(nexus.summary(&ctx, "999999999").await.unwrap().is_none());
}

#[test]
fn changelogs_come_newest_first() {
    let changelogs: NexusChangelogs = serde_json::from_str(
        r#"{"1.0": ["First release"], "1.10": ["Later", " "], "1.2": ["Fixed a crash", "Faster"]}"#,
    )
    .unwrap();
    let entries = changelog_entries(changelogs);
    let versions: Vec<_> = entries.iter().map(|e| e.version.as_deref()).collect();
    assert_eq!(versions, [Some("1.10"), Some("1.2"), Some("1.0")]);
    assert_eq!(entries[0].notes, "- Later");
    assert_eq!(entries[1].notes, "- Fixed a crash\n- Faster");
}

#[test]
fn free_text_changelog_versions_keep_nexus_order_reversed() {
    let changelogs: NexusChangelogs =
        serde_json::from_str(r#"{"5.1SE": ["a"], "5.2SE": ["b"], "5.0": ["c"]}"#).unwrap();
    let versions: Vec<_> = changelog_entries(changelogs)
        .into_iter()
        .map(|e| e.version.unwrap())
        .collect();
    assert_eq!(versions, ["5.0", "5.2SE", "5.1SE"]);
}

#[test]
fn no_changelogs_is_an_empty_list() {
    let changelogs: NexusChangelogs = serde_json::from_str("[]").unwrap();
    assert!(changelog_entries(changelogs).is_empty());
}
