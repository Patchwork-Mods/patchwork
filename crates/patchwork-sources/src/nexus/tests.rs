use super::*;
use patchwork_core::game::GameDef;

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
    };
    let nexus = Nexus::new(None);
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
    let pick = |v| pick_file(&files.files, v).map(|f| f.file_id);
    assert_eq!(pick(None), Some(2000));
    assert_eq!(pick(Some("5.2SE")), Some(2000));
    assert_eq!(pick(Some("5.1SE")), Some(1000));
    assert_eq!(pick(Some("9.9")), None);
}

#[test]
fn converts_api_response() {
    let mod_: NexusMod = serde_json::from_str(MOD).unwrap();
    let files: NexusFiles = serde_json::from_str(FILES).unwrap();
    let links: Vec<NexusDownloadLink> = serde_json::from_str(LINKS).unwrap();
    let url = links[0].uri.clone();
    let resolved = nexus_to_resolved_mod(&mod_, &files.files[1], url.clone());

    assert_eq!(resolved.id, "12604");
    assert_eq!(resolved.name, "SkyUI");
    assert_eq!(resolved.version.as_deref(), Some("5.2SE"));
    assert_eq!(resolved.format, ArtifactFormat::Zip);
    assert_eq!(resolved.artifact, ArtifactLocation::Url { url });
    assert!(resolved.dependencies.is_empty());
}

#[tokio::test]
async fn needs_game_domain_config() {
    let mut game = skyrim();
    game.sources.clear();
    let ctx = SourceContext {
        game: &game,
        config: None,
    };
    let err = Nexus::new(Some("key".to_owned()))
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
    };
    let err = Nexus::new(None)
        .resolve(&ctx, "12604", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains(API_KEY_VAR), "{err}");
}

#[tokio::test]
async fn rejects_non_numeric_ids() {
    let game = skyrim();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
    };
    let err = Nexus::new(Some("key".to_owned()))
        .resolve(&ctx, "SkyUI", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("numeric"), "{err}");
}

// This talks to nexusmods.com and needs NEXUS_API_KEY set (and a premium
// account, since only premium users can get download links through the API);
// run it with `cargo test -- --ignored`.
#[tokio::test]
#[ignore = "needs network and a Nexus API key"]
async fn test_get_mod() {
    let game = skyrim();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("nexus"),
    };
    let resolved = Nexus::from_env()
        .resolve(&ctx, "12604", None)
        .await
        .unwrap();
    assert_eq!(resolved.id, "12604");
    assert_eq!(resolved.name, "SkyUI");
    assert!(resolved.version.is_some());
}
