use super::search::{self, ModsPage, effective_sort, mods_filter, node_to_summary};
use super::*;
use patchwork_core::game::GameDef;
use patchwork_core::source::Need;
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
    // the primary file's version leads, though "Something new" is newer
    assert_eq!(
        file_versions(&files.files, true, |_| 1),
        ["5.2SE", "6.0", "5.1SE"]
    );
}

/// SMIM (skyrimspecialedition/659): an optional file numbered on its own
/// was uploaded after the main file.
const SMIM_FILES: &str = r#"{
    "files": [
        {
            "file_id": 1,
            "name": "SMIM SE 2-07",
            "version": "2.07",
            "category_name": "OLD_VERSION",
            "is_primary": false,
            "file_name": "SMIM SE 2-07-659-2-07.7z",
            "uploaded_timestamp": 1600000000
        },
        {
            "file_id": 2,
            "name": "SMIM SE 2-08",
            "version": "2.08",
            "category_name": "MAIN",
            "is_primary": true,
            "file_name": "SMIM SE 2-08-659-2-08.7z",
            "uploaded_timestamp": 1650000000
        },
        {
            "file_id": 3,
            "name": "Some patch",
            "version": "1.0",
            "category_name": "OPTIONAL",
            "is_primary": false,
            "file_name": "Some patch-659-1-0.7z",
            "uploaded_timestamp": 1700000000
        },
        {
            "file_id": 4,
            "name": "Some extra",
            "version": "1.1",
            "category_name": "MISCELLANEOUS",
            "is_primary": false,
            "file_name": "Some extra-659-1-1.7z",
            "uploaded_timestamp": 1710000000
        }
    ]
}"#;

#[test]
fn add_ons_dont_count_as_the_latest_version() {
    let files: NexusFiles = serde_json::from_str(SMIM_FILES).unwrap();
    assert_eq!(file_versions(&files.files, true, |_| 1), ["2.08", "2.07"]);
    // a mod added for the optional file lists that file's versions
    let patch: Vec<NexusFile> = files
        .files
        .into_iter()
        .filter(|f| picked(Some(FilePick::Named("Some patch")), f))
        .collect();
    assert_eq!(file_versions(&patch, false, |_| 1), ["1.0"]);
}

#[test]
fn the_latest_version_suits_the_installed_game() {
    let game = labelled_skyrim();
    let files: NexusFiles = serde_json::from_str(RUNTIME_FILES).unwrap();
    let mut files = files.files;
    files[1].version = Some("6".to_owned());
    let versions = |installed| file_versions(&files, true, |f| fit(&game, installed, f));
    assert_eq!(versions(Some("1.5.97")), ["6", "7"]);
    assert_eq!(versions(Some("1.6.1170")), ["7", "6"]);
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

#[test]
fn requirements_are_unsure_soft_dependencies() {
    let node: search::RequirementsNode = serde_json::from_str(
        r#"{
          "modId": 12604,
          "gameId": 1704,
          "modRequirements": {
            "nexusRequirements": { "nodes": [
              { "modId": "30379", "modName": "Skyrim Script Extender (SKSE64)", "notes": "",
                "url": "", "externalRequirement": false, "gameId": "1704" },
              { "modId": "17", "modName": "Old &amp; Classic", "notes": " only for the classic menus ",
                "url": "", "externalRequirement": false, "gameId": "110" },
              { "modId": null, "modName": "Some Tool", "notes": null,
                "url": "https://example.com/tool", "externalRequirement": true, "gameId": null }
            ] },
            "dlcRequirements": [
              { "gameExpansion": { "name": "Saints &amp; Seducers" }, "notes": "" }
            ]
          }
        }"#,
    )
    .unwrap();
    let soft = search::requirements_to_soft(node);
    let seen: Vec<_> = soft
        .iter()
        .map(|d| {
            assert_eq!(d.need, Need::Unknown);
            (
                d.key.as_deref(),
                d.name.as_deref(),
                d.url.as_deref(),
                d.notes.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            (
                Some("nexus:30379"),
                Some("Skyrim Script Extender (SKSE64)"),
                None,
                None
            ),
            (
                None,
                Some("Old & Classic"),
                Some("https://www.nexusmods.com/mods/17?game_id=110"),
                Some("only for the classic menus")
            ),
            (
                None,
                Some("Some Tool"),
                Some("https://example.com/tool"),
                None
            ),
            (None, Some("Saints & Seducers (DLC)"), None, None),
        ]
    );
}

// Requirements come from the GraphQL API, which doesn't need a key.
#[tokio::test]
#[ignore = "needs network"]
async fn fetches_requirements() {
    let soft = Nexus::default()
        .requirements("skyrimspecialedition", 12604)
        .await
        .unwrap();
    assert!(
        soft.iter().any(|d| d.key.as_deref() == Some("nexus:30379")),
        "{soft:?}"
    );
}

/// A plugin and its preloader, both main files, like SSE Engine Fixes has,
/// with an older upload of the plugin.
const SPLIT_FILES: &str = r#"{
    "files": [
        {
            "file_id": 10,
            "name": "Engine Fixes 7.0.17 - Main File",
            "version": "7.0.17",
            "category_name": "OLD_VERSION",
            "is_primary": false,
            "file_name": "EngineFixes-17230-7-0-17.7z",
            "uploaded_timestamp": 1680000000
        },
        {
            "file_id": 11,
            "name": "Engine Fixes 7.0.18 - Main File",
            "version": "7.0.18",
            "category_name": "MAIN",
            "is_primary": true,
            "file_name": "EngineFixes-17230-7-0-18.7z",
            "uploaded_timestamp": 1690000000
        },
        {
            "file_id": 12,
            "name": "Engine Fixes - SKSE64 Preloader",
            "version": "7.0.18",
            "category_name": "MAIN",
            "is_primary": false,
            "file_name": "Preloader-17230-7-0-18.7z",
            "uploaded_timestamp": 1690000001
        },
        {
            "file_id": 13,
            "name": "Engine Fixes - SKSE64 Preloader",
            "version": "7.0.18",
            "category_name": "MAIN",
            "is_primary": false,
            "file_name": "Preloader-17230-7-0-18b.7z",
            "uploaded_timestamp": 1690000002
        },
        {
            "file_id": 14,
            "name": "Some Patch",
            "version": "1",
            "category_name": "OPTIONAL",
            "is_primary": false,
            "file_name": "Patch.7z",
            "uploaded_timestamp": 1690000003
        }
    ]
}"#;

#[test]
fn parses_ids() {
    let id = |s| NexusId::parse(s).unwrap();
    assert_eq!(
        id("266"),
        NexusId {
            mod_id: 266,
            file: None
        }
    );
    assert_eq!(
        id("266/"),
        NexusId {
            mod_id: 266,
            file: None
        }
    );
    assert_eq!(
        id("17230/Engine Fixes - SKSE64 Preloader"),
        NexusId {
            mod_id: 17230,
            file: Some(FilePick::Named("Engine Fixes - SKSE64 Preloader"))
        }
    );
    // a slash in a file's name is part of the pattern
    assert_eq!(
        id("1/SE/AE * Patch").file,
        Some(FilePick::Named("SE/AE * Patch"))
    );
    assert_eq!(id("1/@42").file, Some(FilePick::Id(42)));
    for s in ["17230/@42", "17230/Main *", "266"] {
        assert_eq!(id(s).to_string(), s);
    }
    for bad in ["SkyUI", "", "1/@x", "/Main"] {
        assert!(NexusId::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn offers_each_main_file() {
    let files: NexusFiles = serde_json::from_str(SPLIT_FILES).unwrap();
    let id = NexusId::parse("17230").unwrap();
    let choices = file_choices(&id, &files.files, |_| 1);
    let ids: Vec<&str> = choices.iter().map(|c| c.id.as_str()).collect();
    // primary first; the preloader uploaded twice is one choice; the
    // optional and old files aren't offered
    assert_eq!(
        ids,
        [
            "17230/Engine Fixes * - Main File",
            "17230/Engine Fixes - SKSE64 Preloader"
        ]
    );
    assert_eq!(choices[0].name, "Engine Fixes 7.0.18 - Main File");
    let description = choices[1].description.as_deref().unwrap();
    assert!(description.contains("named exactly"), "{description}");

    // once picked, there's nothing left to choose
    let picked_id = NexusId::parse(ids[1]).unwrap();
    assert!(file_choices(&picked_id, &files.files, |_| 1).is_empty());

    // and each picks its own file, the newest upload of it
    let pick = |id: &str, version| {
        let pick = NexusId::parse(id).unwrap().file;
        pick_file(
            files.files.iter().filter(|f| picked(pick, f)),
            version,
            |_| 1,
        )
        .map(|f| f.file_id)
    };
    assert_eq!(pick(ids[0], None), Some(11));
    assert_eq!(pick(ids[0], Some("7.0.17")), Some(10));
    assert_eq!(pick(ids[1], None), Some(13));
    assert_eq!(pick("17230/nothing like it", None), None);

    let err = no_file("17230/x", Some(FilePick::Named("x")), None, &files.files);
    assert!(err.to_string().contains("Some Patch"), "{err}");
}

#[test]
fn offers_no_choice_between_runtimes() {
    // one file for each version of the game is pick_file's to sort out
    let game = labelled_skyrim();
    let files: NexusFiles = serde_json::from_str(RUNTIME_FILES).unwrap();
    let id = NexusId::parse("17230").unwrap();
    let choices = |installed: Option<&str>| {
        file_choices(&id, &files.files, |f| fit(&game, installed, f)).len()
    };
    assert_eq!(choices(Some("1.6.1170")), 0);
    assert_eq!(choices(Some("1.5.97")), 0);
    // without a version to go on, both
    assert_eq!(choices(None), 2);

    let single: NexusFiles = serde_json::from_str(FILES).unwrap();
    assert!(file_choices(&NexusId::parse("12604").unwrap(), &single.files, |_| 1).is_empty());
}

#[test]
fn links_to_one_file() {
    let game = skyrim();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
        settings: None,
        env: None,
    };
    let nexus = Nexus::default();
    let id = |url: &str| nexus.parse_url(&ctx, url).unwrap().map(|s| s.id);
    assert_eq!(
        id("https://www.nexusmods.com/skyrimspecialedition/mods/17230?tab=files&file_id=12")
            .as_deref(),
        Some("17230/@12")
    );
    assert_eq!(
        id("nxm://skyrimspecialedition/mods/17230/files/12?key=a&expires=1").as_deref(),
        Some("17230/@12")
    );
    assert_eq!(id("nxm://fallout4/mods/1/files/2"), None);
    assert_eq!(
        nexus.mod_url(&ctx, "17230/@12", None).as_deref(),
        Some("https://www.nexusmods.com/skyrimspecialedition/mods/17230?tab=files&file_id=12")
    );
    assert_eq!(
        nexus.mod_url(&ctx, "17230/Main *", None).as_deref(),
        Some("https://www.nexusmods.com/skyrimspecialedition/mods/17230")
    );

    // which `choices` turns into the file's name
    let files: NexusFiles = serde_json::from_str(SPLIT_FILES).unwrap();
    let choices = file_choices(&NexusId::parse("17230/@11").unwrap(), &files.files, |_| 1);
    let ids: Vec<&str> = choices.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["17230/Engine Fixes * - Main File"]);
    // even an optional file
    let choices = file_choices(&NexusId::parse("17230/@14").unwrap(), &files.files, |_| 1);
    assert_eq!(choices[0].id, "17230/Some Patch");
}

#[test]
fn names_mods_after_their_file() {
    assert_eq!(
        file_mod_name(
            "SSE Engine Fixes (skse64 plugin)",
            "Engine Fixes - SKSE64 Preloader"
        ),
        "Engine Fixes - SKSE64 Preloader"
    );
    assert_eq!(
        file_mod_name("SSE Engine Fixes", "Engine Fixes * - Main File"),
        "Engine Fixes - Main File"
    );
    assert_eq!(file_mod_name("SkyUI", "Main File"), "SkyUI - Main File");
    assert_eq!(file_mod_name("SkyUI", "SkyUI v*"), "SkyUI");
    assert_eq!(file_mod_name("SkyUI", "*"), "SkyUI");
}
