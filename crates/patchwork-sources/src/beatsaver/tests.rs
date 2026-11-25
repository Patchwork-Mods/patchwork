use super::*;

use patchwork_core::game::GameDef;

/// Cut down from `/maps/id/25f`: a map with one difficulty, using nothing.
const PLAIN: &str = r#"{
  "id": "25f",
  "name": "DM DOKURO - Reality Check Through The Skull",
  "description": "\r\n(Expert+++ Only) (Epilepsy Warning)\r\n\r\nA Megalovania remix.",
  "uploader": {"id": 52609, "name": "rickput", "verifiedMapper": true},
  "metadata": {"bpm": 260.0, "duration": 334, "songName": "Reality Check Through The Skull",
    "songSubName": "", "songAuthorName": "Mapped by Rickput", "levelAuthorName": "DM DOKURO"},
  "stats": {"plays": 0, "downloads": 0, "upvotes": 11945, "downvotes": 959, "score": 0.924},
  "uploaded": "2018-05-26T22:49:59Z",
  "versions": [{
    "hash": "ad6c9f88d63259a95e39397c31be2981c4beb744", "key": "25f", "state": "Published",
    "createdAt": "2018-05-26T22:49:59Z", "sageScore": 6,
    "diffs": [{"njs": 17.0, "characteristic": "Standard", "difficulty": "ExpertPlus",
      "chroma": false, "me": false, "ne": false, "cinema": false}],
    "downloadURL": "https://r2cdn.beatsaver.com/ad6c9f88d63259a95e39397c31be2981c4beb744.zip",
    "coverURL": "https://cfcdn.beatsaver.com/ad6c9f88d63259a95e39397c31be2981c4beb744.jpg"
  }],
  "lastPublishedAt": "2018-05-26T22:49:59Z",
  "tags": ["electronic"]
}"#;

/// Cut down from `/maps/id/43a1f`, with a third difficulty added that uses
/// nothing: two use Noodle Extensions, Chroma and Vivify. It has a newer
/// version that's only being testplayed, and no tags or description.
const NOODLE: &str = r#"{
  "id": "43a1f",
  "name": "[Extra Sensory II] Simplifi - you",
  "uploader": {"id": 4284246, "name": "Swifter"},
  "metadata": {"songAuthorName": "Simplifi", "levelAuthorName": "Swifter"},
  "stats": {"upvotes": 6546},
  "versions": [
    {
      "hash": "d265d40935983502ad6f05ddeb5417f3e37c2290", "state": "Published",
      "createdAt": "2025-01-26T23:35:01.797228Z",
      "diffs": [
        {"difficulty": "Normal", "chroma": true, "me": false, "ne": true, "cinema": false, "vivify": true},
        {"difficulty": "Hard", "chroma": true, "me": false, "ne": true, "cinema": false, "vivify": true},
        {"difficulty": "Expert", "chroma": false, "me": false, "ne": false, "cinema": false}
      ],
      "downloadURL": "https://r2cdn.beatsaver.com/d265d40935983502ad6f05ddeb5417f3e37c2290.zip"
    },
    {
      "hash": "0000000000000000000000000000000000000000", "state": "Testplay",
      "createdAt": "2025-02-01T00:00:00Z",
      "diffs": [],
      "downloadURL": "https://r2cdn.beatsaver.com/0000000000000000000000000000000000000000.zip"
    }
  ],
  "lastPublishedAt": "2025-01-26T23:35:01.797228Z"
}"#;

fn map(json: &str) -> Map {
    serde_json::from_str(json).unwrap()
}

fn config() -> Config<'static> {
    Config {
        needs: Some("beatmods:SongCore"),
        uses: vec![
            ("ne", Some("beatmods:NoodleExtensions")),
            ("chroma", Some("beatmods:Chroma")),
            ("cinema", None),
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

fn query(text: Option<&str>, sort: Option<SortBy>) -> SearchQuery {
    SearchQuery {
        text: text.map(str::to_owned),
        sort,
        categories: Vec::new(),
        excluded_categories: Vec::new(),
        page: 0,
    }
}

#[test]
fn parses_maps() {
    let plain = map(PLAIN);
    assert_eq!(plain.page(), "https://beatsaver.com/maps/25f");
    // The first line with something on it...
    assert_eq!(
        plain.summary().as_deref(),
        Some("(Expert+++ Only) (Epilepsy Warning)")
    );
    // ...or who the song's by, with no description.
    assert_eq!(map(NOODLE).summary().as_deref(), Some("Simplifi"));
    // The version being testplayed is newer, but not published.
    assert_eq!(
        map(NOODLE).published().unwrap().download_url,
        "https://r2cdn.beatsaver.com/d265d40935983502ad6f05ddeb5417f3e37c2290.zip"
    );
}

#[test]
fn resolves_to_the_published_zip() {
    let resolved = to_resolved(&config(), &map(PLAIN)).unwrap();
    assert_eq!(resolved.id, "25f");
    assert_eq!(resolved.version, None);
    assert_eq!(resolved.kind.as_deref(), Some("map"));
    assert_eq!(resolved.format, ArtifactFormat::Archive);
    assert_eq!(
        resolved.artifact,
        ArtifactLocation::Url {
            url: "https://r2cdn.beatsaver.com/ad6c9f88d63259a95e39397c31be2981c4beb744.zip"
                .to_owned()
        }
    );
    // Every map needs SongCore, and this one nothing else.
    assert_eq!(
        resolved.dependencies,
        vec![Dependency {
            source: "beatmods".to_owned(),
            id: "SongCore".to_owned(),
            req: VersionReq::Any,
        }]
    );
    assert!(resolved.soft_dependencies.is_empty());

    let unpublished = map(&PLAIN.replace("\"Published\"", "\"Testplay\""));
    let err = to_resolved(&config(), &unpublished)
        .unwrap_err()
        .to_string();
    assert!(err.contains("isn't published"), "{err}");
}

#[test]
fn suggests_the_features_it_uses() {
    let resolved = to_resolved(&config(), &map(NOODLE)).unwrap();
    let suggested: Vec<_> = resolved
        .soft_dependencies
        .iter()
        .map(|s| (s.key.as_deref(), s.label(), s.need, s.notes.as_deref()))
        .collect();
    // Vivify isn't in the game's `uses`, so it isn't mentioned.
    assert_eq!(
        suggested,
        [
            (
                Some("beatmods:NoodleExtensions"),
                "Noodle Extensions",
                Need::Unknown,
                Some("2 of its 3 difficulties use Noodle Extensions, which moves notes and walls."),
            ),
            (
                Some("beatmods:Chroma"),
                "Chroma",
                Need::Optional,
                Some(
                    "2 of its 3 difficulties use Chroma, which colors its lights, notes and walls."
                ),
            ),
        ]
    );
    // One patchwork can't get is named without a key.
    let cinema = map(&PLAIN.replace("\"cinema\": false", "\"cinema\": true"));
    let resolved = to_resolved(&config(), &cinema).unwrap();
    let [only] = &resolved.soft_dependencies[..] else {
        panic!("{:?}", resolved.soft_dependencies)
    };
    assert_eq!(only.key, None);
    assert_eq!(
        only.notes.as_deref(),
        Some("Its difficulty uses Cinema, which plays a video behind it.")
    );
}

#[test]
fn reads_the_game_config() {
    let game = game();
    let table: toml::Table = toml::from_str(
        r#"
        needs = "beatmods:SongCore"
        uses = { ne = "beatmods:NoodleExtensions", cinema = "" }
        "#,
    )
    .unwrap();
    let config = super::config(&ctx(&game, Some(&table))).unwrap();
    assert_eq!(config.needs, Some("beatmods:SongCore"));
    assert_eq!(
        config.uses,
        [("cinema", None), ("ne", Some("beatmods:NoodleExtensions"))]
    );
    // None at all is fine: maps then need and suggest nothing.
    let config = super::config(&ctx(&game, None)).unwrap();
    assert_eq!((config.needs, config.uses.len()), (None, 0));

    let bad: toml::Table = toml::from_str("uses = { ne = 1 }").unwrap();
    assert!(super::config(&ctx(&game, Some(&bad))).is_err());
}

#[test]
fn keys() {
    assert_eq!(key("25f").as_deref(), Some("25f"));
    assert_eq!(key(" 43A1F ").as_deref(), Some("43a1f"));
    assert_eq!(key(""), None);
    assert_eq!(key("crab rave"), None);
    assert_eq!(key("123456789"), None);
}

#[test]
fn parses_links() {
    let game = game();
    let ctx = ctx(&game, None);
    let parse = |url| {
        BeatSaver
            .parse_url(&ctx, url)
            .unwrap()
            .map(|spec| (spec.source, spec.id))
    };
    let want = Some(("beatsaver".to_owned(), "25f".to_owned()));
    assert_eq!(parse("https://beatsaver.com/maps/25f"), want);
    assert_eq!(parse("https://www.beatsaver.com/maps/25F?tab=x"), want);
    assert_eq!(parse("https://beatsaver.com/beatmap/25f"), want);
    assert_eq!(parse("beatsaver://25f"), want);
    assert_eq!(parse("beatsaver://25f/"), want);
    assert_eq!(parse("https://beatsaver.com/profile/52609"), None);
    assert_eq!(parse("https://beatsaver.com/maps/not-a-map"), None);
    assert_eq!(parse("https://beatmods.com/maps/25f"), None);
    assert_eq!(
        BeatSaver.mod_url(&ctx, "25f", None).as_deref(),
        Some("https://beatsaver.com/maps/25f")
    );
    assert_eq!(BeatSaver.mod_url(&ctx, "nope", None), None);
}

#[test]
fn summarizes_maps() {
    let summary = to_summary(&map(PLAIN));
    assert_eq!(summary.spec().id, "25f");
    assert_eq!(summary.author.as_deref(), Some("rickput"));
    assert_eq!(
        summary.author_url.as_deref(),
        Some("https://beatsaver.com/profile/52609")
    );
    assert_eq!(summary.rating, Some(11945));
    assert_eq!(
        summary.icon_url.as_deref(),
        Some("https://cfcdn.beatsaver.com/ad6c9f88d63259a95e39397c31be2981c4beb744.jpg")
    );
    assert_eq!(
        summary.categories,
        [Category {
            id: "electronic".to_owned(),
            name: "Electronic".to_owned()
        }]
    );
}

#[test]
fn search_links() {
    let url = |q: &SearchQuery| search_url(q).unwrap();
    // Newest first with no text, as BeatSaver shows them...
    assert_eq!(
        url(&query(None, None)),
        (
            "https://api.beatsaver.com/search/text/0?order=Latest&pageSize=20".to_owned(),
            SortBy::Newest
        )
    );
    // ...and by relevance with some.
    assert_eq!(
        url(&query(Some(" crab rave "), None)),
        (
            "https://api.beatsaver.com/search/text/0?order=Relevance&pageSize=20&q=crab+rave"
                .to_owned(),
            SortBy::Relevance
        )
    );
    // A sort it doesn't offer is by relevance.
    assert_eq!(
        url(&query(None, Some(SortBy::Downloads))).1,
        SortBy::Relevance
    );
    let mut tagged = query(Some("x"), Some(SortBy::Rating));
    tagged.categories = vec!["anime".to_owned(), "K-Pop".to_owned()];
    tagged.excluded_categories = vec!["speed".to_owned()];
    tagged.page = 2;
    assert_eq!(
        url(&tagged).0,
        "https://api.beatsaver.com/search/text/2?order=Rating&pageSize=20&q=x&tags=anime%7Ck-pop%2C%21speed"
    );
    let mut excluded = query(None, None);
    excluded.excluded_categories = vec!["ai".to_owned(), "poodle".to_owned()];
    assert_eq!(tag_query(&excluded).as_deref(), Some("!ai,!poodle"));
}

#[test]
fn search_pages() {
    let response: SearchResponse = serde_json::from_str(&format!(
        r#"{{"docs": [{PLAIN}, {NOODLE}], "info": {{"total": 444, "pages": 23, "duration": 0.002}}}}"#
    ))
    .unwrap();
    let mut q = query(Some("x"), None);
    let page = to_page(&response, &q, SortBy::Relevance);
    assert_eq!(page.total, Some(444));
    assert!(page.has_more);
    let ids: Vec<_> = page.items.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["25f", "43a1f"]);
    q.page = 22;
    assert!(!to_page(&response, &q, SortBy::Relevance).has_more);
}
