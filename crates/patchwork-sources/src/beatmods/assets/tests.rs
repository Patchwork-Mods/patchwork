use super::*;

use patchwork_core::game::GameDef;

/// Cut down from `/assets?status=verified`: a saber, notes whose file name
/// has a `%` in it, a saber whose has a `#`, a sound (a type Beat Saber
/// doesn't offer), one for another game and one waiting to be verified.
const LIST: &str = r##"{
  "assets": [
    {
      "id": 8078, "oldId": null, "gameName": "beatsaber", "linkedIds": [],
      "type": "saber_saber", "renderingMethod": "birp_sp",
      "uploader": {"id": 6244, "username": "ismellfood", "displayName": "iSmellFood"},
      "iconUrls": ["https://beatmods.com/files/8078/1.png"],
      "name": "iSF-HoloPoint",
      "description": "\nA lil simple gameplay-focused saber.\nDiscord: https://discord.gg/x",
      "license": "cc0-1.0",
      "fileHash": "a70f1b186444f83141f7cc61f31b08b6", "fileSize": 1437247,
      "downloadUrl": "https://beatmods.com/files/8078/iSF-HoloPoint.saber",
      "status": "verified",
      "tags": ["CustomColors", "CustomTrails", "Animations"],
      "createdAt": "2026-11-17T09:43:07.842Z", "updatedAt": "2026-11-17T09:55:06.021Z"
    },
    {
      "id": 5791, "gameName": "beatsaber", "type": "note_bloq",
      "uploader": {"username": "sede"},
      "iconUrls": [], "name": "60% SE_DE Note (Outline)",
      "fileHash": "4920091cd2e0757ad3ae12bbe95df782",
      "downloadUrl": "https://beatmods.com/files/5791/60% SE_DE Note (Outline).bloq",
      "status": "verified", "tags": [],
      "updatedAt": "2026-01-02T00:00:00Z"
    },
    {
      "id": 3987, "gameName": "beatsaber", "type": "saber_saber",
      "name": "#pc-modeling",
      "fileHash": "8899b1245d7252fb63f6f9307d7bc0e4",
      "downloadUrl": "https://beatmods.com/files/3987/#pc-modeling.saber",
      "status": "verified", "tags": ["Meme"],
      "updatedAt": "2025-06-01T00:00:00Z"
    },
    {
      "id": 7983, "gameName": "beatsaber", "type": "sound_ogg",
      "name": "St. Anger Snare",
      "fileHash": "23014016235c69645cfd09c73d424b26",
      "downloadUrl": "https://beatmods.com/files/7983/St. Anger Snare.ogg",
      "status": "verified", "tags": ["Hitsound"]
    },
    {
      "id": 9001, "gameName": "chromapper", "type": "saber_saber",
      "name": "Mapper Saber", "fileHash": "ffff",
      "downloadUrl": "https://beatmods.com/files/9001/Mapper Saber.saber",
      "status": "verified"
    },
    {
      "id": 9002, "gameName": "beatsaber", "type": "saber_saber",
      "name": "Queued Saber", "fileHash": "eeee",
      "downloadUrl": "https://beatmods.com/files/9002/Queued Saber.saber",
      "status": "queue"
    }
  ],
  "total": 6,
  "page": null
}"##;

fn assets() -> Vec<Asset> {
    serde_json::from_str::<AssetList>(LIST).unwrap().assets
}

fn asset(id: u64) -> Asset {
    assets().into_iter().find(|a| a.id == id).unwrap()
}

fn config() -> Config<'static> {
    Config {
        game: "beatsaber",
        types: vec![
            ("note_bloq", Some("beatmods:CustomNotes")),
            ("saber_saber", Some("beatmods:CustomSabersLite")),
            ("platform_plat", None),
        ],
    }
}

fn game() -> GameDef {
    toml::from_str(
        r#"
        schema = 1
        id = "beat-saber"
        name = "Beat Saber"
        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap()
}

fn ctx<'a>(game: &'a GameDef, config: Option<&'a toml::Table>) -> SourceContext<'a> {
    SourceContext {
        game,
        config,
        settings: None,
        env: None,
        links: patchwork_core::links::Links::none(),
    }
}

#[test]
fn parses_responses() {
    let saber = asset(8078);
    assert_eq!(saber.name, "iSF-HoloPoint");
    assert_eq!(saber.kind, "saber_saber");
    assert_eq!(saber.author().as_deref(), Some("iSmellFood"));
    assert_eq!(asset(5791).author().as_deref(), Some("sede"));
    assert_eq!(asset(3987).author(), None);
    // The first line with something on it.
    assert_eq!(
        saber.summary().as_deref(),
        Some("A lil simple gameplay-focused saber.")
    );
    // `/assets/{id}` is one on its own.
    let one: Asset = serde_json::from_str(
        r#"{"id": 1, "gameName": "beatsaber", "type": "avatar_avatar", "name": "A",
            "fileHash": "00", "downloadUrl": "https://beatmods.com/files/1/A.avatar",
            "status": "verified"}"#,
    )
    .unwrap();
    assert_eq!(one.file_name(), "A.avatar");
}

#[test]
fn links_to_files_by_hash() {
    let url = |id| asset(id).url().unwrap();
    assert_eq!(
        url(8078),
        "https://beatmods.com/files/8078/iSF-HoloPoint.saber?v=a70f1b186444f83141f7cc61f31b08b6"
    );
    // Escaped, so the `#` isn't taken as the end of the link, and the
    // file keeps its own name.
    assert_eq!(
        url(3987),
        "https://beatmods.com/files/3987/%23pc-modeling.saber?v=8899b1245d7252fb63f6f9307d7bc0e4"
    );
    assert_eq!(
        url(5791),
        "https://beatmods.com/files/5791/60%25%20SE_DE%20Note%20(Outline).bloq\
         ?v=4920091cd2e0757ad3ae12bbe95df782"
    );
    for id in [8078, 3987, 5791] {
        let location = ArtifactLocation::Url { url: url(id) };
        assert_eq!(location.file_name(), asset(id).file_name());
    }
}

#[test]
fn reads_the_game_config() {
    let game = game();
    let err = |text: Option<&str>| {
        let table: Option<toml::Table> = text.map(|t| toml::from_str(t).unwrap());
        super::config(&ctx(&game, table.as_ref()))
            .unwrap_err()
            .to_string()
    };
    assert!(err(None).contains("[sources.beatmods-assets] game"));
    assert!(err(Some(r#"game = "beatsaber""#)).contains("[sources.beatmods-assets] types"));
    assert!(err(Some("game = \"beatsaber\"\ntypes = { note_bloq = 1 }")).contains("note_bloq"));

    let table: toml::Table = toml::from_str(
        r#"game = "beatsaber"
types = { saber_saber = "beatmods:CustomSabersLite", platform_plat = "" }"#,
    )
    .unwrap();
    let c = super::config(&ctx(&game, Some(&table))).unwrap();
    assert_eq!(c.game, "beatsaber");
    assert_eq!(
        c.types,
        [
            ("platform_plat", None),
            ("saber_saber", Some("beatmods:CustomSabersLite"))
        ]
    );
}

#[test]
fn resolves_assets_with_their_loader() {
    let resolved = to_resolved(&config(), &asset(8078)).unwrap();
    assert_eq!(resolved.id, "8078");
    assert_eq!(resolved.name, "iSF-HoloPoint");
    assert_eq!(resolved.version, None);
    assert_eq!(resolved.format, ArtifactFormat::File);
    assert_eq!(resolved.kind.as_deref(), Some("saber_saber"));
    assert_eq!(resolved.artifact.file_name(), "iSF-HoloPoint.saber");
    assert_eq!(resolved.dependencies.len(), 1);
    let dep = &resolved.dependencies[0];
    assert_eq!(
        (dep.source.as_str(), dep.id.as_str()),
        ("beatmods", "CustomSabersLite")
    );
    assert_eq!(dep.req, VersionReq::Any);

    // A type that needs nothing.
    let mut plat = asset(8078);
    plat.kind = "platform_plat".into();
    assert!(
        to_resolved(&config(), &plat)
            .unwrap()
            .dependencies
            .is_empty()
    );

    let mut bad = config();
    bad.types = vec![("saber_saber", Some("CustomSabersLite"))];
    let err = to_resolved(&bad, &asset(8078)).unwrap_err().to_string();
    assert!(err.contains("source:id"), "{err}");
}

#[test]
fn turns_down_assets_it_cant_use() {
    let err = |id| {
        check(&config(), &asset(id), "Beat Saber")
            .unwrap_err()
            .to_string()
    };
    assert!(check(&config(), &asset(8078), "Beat Saber").is_ok());
    assert!(err(7983).contains("sounds (ogg), which patchwork can't install for Beat Saber"));
    assert!(err(9001).contains("another game (chromapper)"));
    assert!(err(9002).contains("isn't verified"));
}

#[test]
fn links_to_pages() {
    let game = game();
    let ctx = ctx(&game, None);
    let source = BeatModsAssets;
    assert_eq!(
        source.mod_url(&ctx, "8078", None).as_deref(),
        Some("https://beatmods.com/assets/8078")
    );
    assert_eq!(source.mod_url(&ctx, "iSF-HoloPoint", None), None);
    let parsed = |url| {
        source
            .parse_url(&ctx, url)
            .unwrap()
            .map(|spec| (spec.source, spec.id))
    };
    let found = Some((ID.to_owned(), "8078".to_owned()));
    assert_eq!(parsed("https://beatmods.com/assets/8078"), found);
    assert_eq!(parsed("https://www.beatmods.com/assets/8078?x=1"), found);
    assert_eq!(parsed("modelsaber://saber/8078/iSF-HoloPoint.saber"), found);
    assert_eq!(parsed("https://beatmods.com/mods/8078"), None);
    assert_eq!(parsed("https://beatmods.com/assets/new"), None);
    assert_eq!(parsed("modelsaber://saber/x/y.saber"), None);
    assert_eq!(parsed("https://example.com/assets/8078"), None);
}

#[test]
fn searches() {
    let assets = assets();
    let query = |text: &str| SearchQuery {
        text: Some(text.into()),
        sort: None,
        categories: Vec::new(),
        excluded_categories: Vec::new(),
        page: 0,
    };
    let ids = |page: SearchPage| -> Vec<String> { page.items.into_iter().map(|m| m.id).collect() };
    let search = |q: &SearchQuery| search_page(&config(), &assets, q);

    // Only what's usable, newest first: no sound, other game's or
    // unverified asset.
    let all = search(&query(""));
    assert_eq!(all.total, Some(3));
    assert_eq!(ids(all), ["8078", "5791", "3987"]);
    // Words in names, descriptions, uploaders and tags.
    assert_eq!(ids(search(&query("GAMEPLAY"))), ["8078"]);
    assert_eq!(ids(search(&query("sede"))), ["5791"]);
    assert_eq!(ids(search(&query("meme"))), ["3987"]);
    assert!(ids(search(&query("snare"))).is_empty());
    // Names first, then by name.
    assert_eq!(ids(search(&query("i"))), ["8078", "3987", "5791"]);
    assert_eq!(ids(search(&query("note"))), ["5791"]);

    // Types.
    let mut q = query("");
    q.categories = vec!["note_bloq".into()];
    assert_eq!(ids(search(&q)), ["5791"]);
    q.categories.clear();
    q.excluded_categories = vec!["note_bloq".into()];
    assert_eq!(ids(search(&q)), ["8078", "3987"]);

    let summary = to_summary(&assets[0]);
    assert_eq!(summary.source, ID);
    assert_eq!(
        summary.url.as_deref(),
        Some("https://beatmods.com/assets/8078")
    );
    assert_eq!(
        summary.icon_url.as_deref(),
        Some("https://beatmods.com/files/8078/1.png")
    );
    assert_eq!(summary.file.as_deref(), Some("iSF-HoloPoint.saber"));
    assert_eq!(summary.categories[0].name, "Sabers");
}

#[tokio::test]
async fn offers_the_games_types() {
    let game = game();
    let table: toml::Table = toml::from_str(
        r#"game = "beatsaber"
types = { saber_saber = "", note_bloq = "", made_up = "" }"#,
    )
    .unwrap();
    let names: Vec<String> = BeatModsAssets
        .search_categories(&ctx(&game, Some(&table)))
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["made_up", "Notes", "Sabers"]);
}
