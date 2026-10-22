use super::*;
use patchwork_core::game::GameDef;

// a response for notnotnotswipez/MoreCompany
const PACKAGE: &str = r#"{
    "namespace": "notnotnotswipez",
    "name": "MoreCompany",
    "full_name": "notnotnotswipez-MoreCompany",
    "owner": "notnotnotswipez",
    "package_url": "https://thunderstore.io/package/notnotnotswipez/MoreCompany/",
    "date_created": "2023-11-16T04:46:27.899881Z",
    "date_updated": "2026-04-25T01:28:42.076106Z",
    "rating_score": -1,
    "is_pinned": false,
    "is_deprecated": false,
    "total_downloads": -1,
    "latest": {
        "namespace": "notnotnotswipez",
        "name": "MoreCompany",
        "version_number": "1.14.0",
        "full_name": "notnotnotswipez-MoreCompany-1.14.0",
        "description": "A stable lobby player count expansion mod. With cosmetics!",
        "icon": "https://ccdn.thunderstore.io/live/repository/icons/notnotnotswipez-MoreCompany-1.14.0.png",
        "dependencies": ["BepInEx-BepInExPack-5.4.2100"],
        "download_url": "https://thunderstore.io/package/download/notnotnotswipez/MoreCompany/1.14.0/",
        "downloads": 917031,
        "date_created": "2026-04-25T01:28:40.943994Z",
        "website_url": "https://github.com/notnotnotswipez/MoreCompany",
        "is_active": true
    },
    "community_listings": [{
        "has_nsfw_content": false,
        "categories": ["Mods", "BepInEx", "Tweaks & Quality Of Life"],
        "community": "lethal-company",
        "review_status": "approved"
    }]
}"#;

fn lethal_company() -> GameDef {
    toml::from_str(
        r#"
        schema = 1
        id = "lethal-company"
        name = "Lethal Company"

        [sources.thunderstore]
        community = "lethal-company"

        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap()
}

#[test]
fn converts_api_response() {
    let package: PackageExperimental = serde_json::from_str(PACKAGE).unwrap();
    let version = package.latest.clone();
    let resolved = thunderstore_to_resolved_mod(PackageVersion { package, version }).unwrap();

    assert_eq!(resolved.id, "notnotnotswipez-MoreCompany");
    assert_eq!(resolved.name, "notnotnotswipez-MoreCompany");
    assert_eq!(resolved.version.as_deref(), Some("1.14.0"));
    assert_eq!(resolved.format, ArtifactFormat::Zip);
    assert_eq!(
        resolved.artifact,
        ArtifactLocation::Url {
            url: "https://thunderstore.io/package/download/notnotnotswipez/MoreCompany/1.14.0/"
                .to_owned()
        }
    );
    assert_eq!(
        resolved.dependencies,
        vec![Dependency {
            source: "thunderstore".to_owned(),
            id: "BepInEx-BepInExPack".to_owned(),
            req: VersionReq::AtLeast("5.4.2100".to_owned()),
        }]
    );
}

#[test]
fn decodes_dependency_strings() {
    let ModNameVersion { name, version } =
        decode_mod_version_name("BepInEx-BepInExPack-5.4.2100").unwrap();
    assert_eq!(name, "BepInEx-BepInExPack");
    assert_eq!(version, "5.4.2100");
    assert!(decode_mod_version_name("nohyphens").is_none());
}

#[test]
fn urls_round_trip() {
    let game = lethal_company();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("thunderstore"),
    };
    let url = Thunderstore
        .mod_url(&ctx, "notnotnotswipez-MoreCompany", Some("1.11.0"))
        .unwrap();
    assert_eq!(
        url,
        "https://thunderstore.io/c/lethal-company/p/notnotnotswipez/MoreCompany/v/1.11.0/"
    );
    let spec = Thunderstore.parse_url(&ctx, &url).unwrap().unwrap();
    assert_eq!(spec.id, "notnotnotswipez-MoreCompany");
    assert_eq!(spec.version.as_deref(), Some("1.11.0"));

    let spec = Thunderstore
        .parse_url(
            &ctx,
            "https://thunderstore.io/package/download/BepInEx/BepInExPack/5.4.2100/",
        )
        .unwrap()
        .unwrap();
    assert_eq!(spec.id, "BepInEx-BepInExPack");
    assert_eq!(spec.version.as_deref(), Some("5.4.2100"));

    let spec = Thunderstore
        .parse_url(
            &ctx,
            "https://thunderstore.io/c/lethal-company/p/notnotnotswipez/MoreCompany/?tab=wiki",
        )
        .unwrap()
        .unwrap();
    assert_eq!(spec.version, None);

    assert!(
        Thunderstore
            .parse_url(&ctx, "https://example.com/c/x/p/a/b/")
            .unwrap()
            .is_none()
    );
    let err = Thunderstore
        .parse_url(&ctx, "https://thunderstore.io/c/riskofrain2/p/a/b/")
        .unwrap_err();
    assert!(err.to_string().contains("riskofrain2"), "{err}");
}

// a trimmed /api/cyberstorm/listing/lethal-company/?q=morecompany
const LISTING: &str = r#"{
    "count": 145,
    "next": "https://thunderstore.io/api/cyberstorm/listing/lethal-company/?page=2&q=morecompany",
    "previous": null,
    "results": [{
        "categories": [
            {"id": "658", "name": "Mods", "slug": "mods"},
            {"id": "872", "name": "Tweaks & Quality Of Life", "slug": "tweaks-and-quality-of-life"}
        ],
        "community_identifier": "lethal-company",
        "description": "A stable lobby player count expansion mod. With cosmetics!",
        "download_count": 26998136,
        "icon_url": "https://ccdn.thunderstore.io/live/repository/icons/notnotnotswipez-MoreCompany-1.14.0.png",
        "is_deprecated": false,
        "is_nsfw": false,
        "is_pinned": false,
        "last_updated": "2026-04-25T01:28:42.076106Z",
        "name": "MoreCompany",
        "namespace": "notnotnotswipez",
        "rating_count": 641,
        "size": 10530391,
        "datetime_created": "2023-11-16T04:46:28.298342Z"
    }]
}"#;

// a trimmed /api/cyberstorm/listing/lethal-company/notnotnotswipez/MoreCompany/
const DETAIL: &str = r#"{
    "categories": [{"id": "658", "name": "Mods", "slug": "mods"}],
    "community_identifier": "lethal-company",
    "description": "A stable lobby player count expansion mod. With cosmetics!",
    "download_count": 27016598,
    "icon_url": "https://ccdn.thunderstore.io/live/repository/icons/notnotnotswipez-MoreCompany-1.14.0.png",
    "is_deprecated": false,
    "latest_version_number": "1.14.0",
    "name": "MoreCompany",
    "namespace": "notnotnotswipez",
    "rating_count": 641,
    "version_created": "2026-04-25T01:28:40.943994Z"
}"#;

#[test]
fn converts_package_details() {
    let detail: ListedPackageDetail = serde_json::from_str(DETAIL).unwrap();
    let summary = detail_to_summary("lethal-company", detail);
    assert_eq!(summary.id, "notnotnotswipez-MoreCompany");
    assert_eq!(summary.latest_version.as_deref(), Some("1.14.0"));
    assert_eq!(summary.downloads, Some(27016598));
    assert_eq!(
        summary.updated,
        Some("2026-04-25T01:28:40.943994Z".parse().unwrap())
    );
    assert_eq!(summary.categories[0].id, "mods");
}

#[test]
fn converts_listings() {
    let listing: Listing = serde_json::from_str(LISTING).unwrap();
    assert_eq!(listing.count, 145);
    assert!(listing.next.is_some());
    let summary = listed_to_summary(
        "lethal-company",
        listing.results.into_iter().next().unwrap(),
    );
    assert_eq!(summary.id, "notnotnotswipez-MoreCompany");
    assert_eq!(
        summary.url.as_deref(),
        Some("https://thunderstore.io/c/lethal-company/p/notnotnotswipez/MoreCompany/")
    );
    assert_eq!(
        summary.author_url.as_deref(),
        Some("https://thunderstore.io/c/lethal-company/p/notnotnotswipez/")
    );
    assert_eq!(summary.name, "MoreCompany");
    assert_eq!(summary.author.as_deref(), Some("notnotnotswipez"));
    assert_eq!(summary.downloads, Some(26998136));
    assert_eq!(summary.rating, Some(641));
    assert_eq!(
        summary.categories[1],
        Category {
            id: "tweaks-and-quality-of-life".to_owned(),
            name: "Tweaks & Quality Of Life".to_owned(),
        }
    );
    assert_eq!(
        summary.spec(),
        ModSpec {
            source: "thunderstore".to_owned(),
            id: "notnotnotswipez-MoreCompany".to_owned(),
            version: None,
        }
    );
}

#[test]
fn sorts_versions_newest_first() {
    let versions: Vec<ListedVersion> = serde_json::from_str(
        r#"[
            {"version_number": "5.4.2304", "datetime_created": "2025-11-05T17:14:09Z"},
            {"version_number": "5.4.2305", "datetime_created": "2026-03-17T05:05:21Z"},
            {"version_number": "5.4.2100", "datetime_created": "2023-01-01T00:00:00Z"}
        ]"#,
    )
    .unwrap();
    assert_eq!(
        sort_versions(versions),
        ["5.4.2305", "5.4.2304", "5.4.2100"]
    );
}

#[tokio::test]
async fn needs_community_config() {
    let mut game = lethal_company();
    game.sources.clear();
    let ctx = SourceContext {
        game: &game,
        config: None,
    };
    let err = Thunderstore
        .resolve(&ctx, "notnotnotswipez-MoreCompany", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("isn't on Thunderstore"), "{err}");
}

// These talk to thunderstore.io; run them with `cargo test -- --ignored`.

#[tokio::test]
#[ignore = "needs network"]
async fn test_get_mod() {
    let game = lethal_company();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("thunderstore"),
    };

    let latest = Thunderstore
        .resolve(&ctx, "notnotnotswipez-MoreCompany", None)
        .await
        .unwrap();
    assert_eq!(latest.id, "notnotnotswipez-MoreCompany");
    assert!(latest.version.is_some());

    let pinned = Thunderstore
        .resolve(&ctx, "notnotnotswipez-MoreCompany", Some("1.11.0"))
        .await
        .unwrap();
    assert_eq!(pinned.version.as_deref(), Some("1.11.0"));
    assert_eq!(
        pinned.artifact,
        ArtifactLocation::Url {
            url: "https://thunderstore.io/package/download/notnotnotswipez/MoreCompany/1.11.0/"
                .to_owned()
        }
    );
    assert!(
        pinned
            .dependencies
            .iter()
            .any(|d| d.id == "BepInEx-BepInExPack")
    );

    let versions = Thunderstore
        .versions(&ctx, "notnotnotswipez-MoreCompany")
        .await
        .unwrap();
    assert_eq!(versions.first(), latest.version.as_ref());
    assert!(versions.iter().any(|v| v == "1.11.0"));
}

#[tokio::test]
#[ignore = "needs network"]
async fn summarizes_one_mod() {
    let game = lethal_company();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("thunderstore"),
    };
    let summary = Thunderstore
        .summary(&ctx, "notnotnotswipez-MoreCompany")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(summary.name, "MoreCompany");
    assert!(summary.downloads.unwrap() > 0);
    assert!(summary.icon_url.is_some());
}

#[tokio::test]
#[ignore = "needs network"]
async fn searches() {
    let game = lethal_company();
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("thunderstore"),
    };

    let home = Thunderstore
        .search(&ctx, &SearchQuery::default())
        .await
        .unwrap();
    assert_eq!(home.sort, SortBy::Downloads);
    assert!(home.has_more);
    let bepinex = home
        .items
        .iter()
        .find(|m| m.id == "BepInEx-BepInExPack")
        .unwrap();
    assert_eq!(
        bepinex.url.as_deref(),
        Some("https://thunderstore.io/c/lethal-company/p/BepInEx/BepInExPack/")
    );
    assert_eq!(
        bepinex.author_url.as_deref(),
        Some("https://thunderstore.io/c/lethal-company/p/BepInEx/")
    );

    let query = SearchQuery {
        text: Some("morecompany".to_owned()),
        categories: vec!["cosmetics".to_owned()],
        ..Default::default()
    };
    let found = Thunderstore.search(&ctx, &query).await.unwrap();
    assert!(!found.items.is_empty());
    assert!(
        found
            .items
            .iter()
            .all(|m| { m.categories.iter().any(|c| c.id == "cosmetics") })
    );

    let without = SearchQuery {
        excluded_categories: vec!["libraries".to_owned()],
        ..Default::default()
    };
    let found = Thunderstore.search(&ctx, &without).await.unwrap();
    assert!(found.total < home.total);
    assert!(
        found
            .items
            .iter()
            .all(|m| { m.categories.iter().all(|c| c.id != "libraries") })
    );

    let past_end = SearchQuery {
        page: 10_000,
        ..query
    };
    let empty = Thunderstore.search(&ctx, &past_end).await.unwrap();
    assert!(empty.items.is_empty() && !empty.has_more);

    let categories = Thunderstore.search_categories(&ctx).await.unwrap();
    assert!(categories.iter().any(|c| c.id == "moons"));
}

#[tokio::test]
#[ignore = "needs network"]
async fn rejects_other_communities() {
    let mut game = lethal_company();
    game.sources.get_mut("thunderstore").unwrap().insert(
        "community".to_owned(),
        toml::Value::String("riskofrain2".to_owned()),
    );
    let ctx = SourceContext {
        game: &game,
        config: game.sources.get("thunderstore"),
    };
    let err = Thunderstore
        .resolve(&ctx, "notnotnotswipez-MoreCompany", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("isn't listed"), "{err}");
}
