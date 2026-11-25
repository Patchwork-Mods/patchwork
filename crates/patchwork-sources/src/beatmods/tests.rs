use super::*;

use patchwork_core::game::GameDef;

/// Cut down from `/mods/1?status=verified`: BSIPA, with a version for an
/// older game version carrying build metadata, a newer one only on Oculus
/// and an unverified one.
const BSIPA: &str = r#"{
  "project": {
    "id": 1, "name": "BSIPA", "nameId": "BSIPA",
    "summary": "A mod loader for Beat Saber.",
    "category": "Core",
    "authors": [{"id": 999, "username": "bsmgmodders", "displayName": "BSMG"}],
    "iconFileUrl": "https://beatmods.com/files/default-beatsaber.png",
    "gitUrl": "https://github.com/nike4613/BeatSaber-IPA-Reloaded",
    "updatedAt": "2026-11-13T14:15:14.068Z"
  },
  "versions": [
    {
      "id": 1119, "projectId": 1, "semver": "4.3.7",
      "supportedGameVersions": [
        {"id": 1106, "gameName": "beatsaber", "version": "1.44.1"},
        {"id": 1099, "gameName": "beatsaber", "version": "1.41.1"}
      ],
      "status": "verified", "dependencies": [], "platform": "universal",
      "zipHash": "947774ef1010ff809ae05e345e269a90",
      "downloadUrl": "https://beatmods.com/files/1/1119/BSIPA_universal_v4.3.7.zip"
    },
    {
      "semver": "4.3.6+1.29.1",
      "supportedGameVersions": [{"version": "1.29.1"}],
      "status": "verified", "dependencies": [], "platform": "universal",
      "zipHash": "aaaa"
    },
    {
      "semver": "4.3.6",
      "supportedGameVersions": [{"version": "1.40.8"}, {"version": "1.39.1"}],
      "status": "verified", "dependencies": [], "platform": "steam",
      "zipHash": "bbbb", "downloadUrl": "https://beatmods.com/files/1/1000/BSIPA_steam_v4.3.6.zip"
    },
    {
      "semver": "4.4.0",
      "supportedGameVersions": [{"version": "1.44.1"}],
      "status": "verified", "dependencies": [], "platform": "oculus",
      "zipHash": "cccc"
    },
    {
      "semver": "4.3.8",
      "supportedGameVersions": [{"version": "1.44.1"}],
      "status": "unverified", "dependencies": [], "platform": "universal",
      "zipHash": "dddd"
    }
  ]
}"#;

/// Cut down from `/mods?gameName=beatsaber&gameVersion=1.40.8&status=verified`.
const LIST: &str = r#"[
  {
    "project": {
      "id": 58, "name": "SiraUtil", "nameId": "SiraUtil",
      "summary": "A Zenject extension for Beat Saber.", "category": "Library",
      "authors": [{"username": "auros", "displayName": "Auros"}],
      "updatedAt": "2026-10-01T00:00:00Z"
    },
    "version": {
      "semver": "3.1.14",
      "supportedGameVersions": [{"version": "1.40.8"}],
      "status": "verified",
      "dependencies": [{"pId": 1, "sv": "^4.3.6"}, {"pId": 96, "sv": "^1.12.5"}],
      "platform": "universal", "zipHash": "eeee"
    }
  },
  {
    "project": {
      "id": 232, "name": "yt-dlp", "nameId": "yt-dlp",
      "summary": "A youtube-dl fork with additional features and fixes",
      "category": "Library", "authors": [{"username": "bsmgmodders"}],
      "updatedAt": "2026-11-13T01:30:43.644Z"
    },
    "version": {
      "semver": "2024.10.7", "supportedGameVersions": [{"version": "1.40.8"}],
      "status": "verified", "dependencies": [], "platform": "universal",
      "zipHash": "1af2114d5d966bbad8de89db8abe9a7a",
      "downloadUrl": "https://beatmods.com/files/232/3202/yt-dlp_universal_v2024.10.7.zip"
    }
  },
  {
    "project": {
      "id": 300, "name": "Camera2", "nameId": "Camera2", "summary": "Better cameras",
      "category": "Tweaks & Tools", "authors": []
    },
    "version": {
      "semver": "1.0.0", "supportedGameVersions": [{"version": "1.40.8"}],
      "status": "verified", "dependencies": [], "platform": "universal", "zipHash": "ffff"
    }
  },
  {
    "project": {
      "id": 300, "name": "Camera2", "nameId": "Camera2", "summary": "Better cameras",
      "category": "Tweaks & Tools", "authors": []
    },
    "version": {
      "semver": "1.0.0", "supportedGameVersions": [{"version": "1.40.8"}],
      "status": "verified", "dependencies": [], "platform": "oculus", "zipHash": "0000"
    }
  },
  {
    "project": {
      "id": 400, "name": "Oculus Only", "nameId": "OculusOnly", "category": "Other",
      "authors": []
    },
    "version": {
      "semver": "1.0.0", "supportedGameVersions": [{"version": "1.40.8"}],
      "status": "verified", "dependencies": [], "platform": "oculus", "zipHash": "1111"
    }
  }
]"#;

fn bsipa() -> ProjectPage {
    serde_json::from_str(BSIPA).unwrap()
}

fn list() -> Vec<Entry> {
    serde_json::from_str(LIST).unwrap()
}

fn config() -> Config<'static> {
    Config {
        game: "beatsaber",
        platforms: PLATFORMS.to_vec(),
    }
}

fn picked(wanted: Option<&str>, game_version: Option<&str>) -> Result<String> {
    let page = bsipa();
    pick(&config(), &page, wanted, game_version, "Beat Saber").map(|v| v.semver.clone())
}

#[test]
fn parses_responses() {
    let page = bsipa();
    assert_eq!(page.project.name_id, "BSIPA");
    assert_eq!(page.project.authors().as_deref(), Some("BSMG"));
    assert_eq!(page.versions.len(), 5);
    assert_eq!(page.versions[1].download_url, None);

    let list = list();
    assert_eq!(list[0].version.dependencies[1].project, 96);
    assert_eq!(list[0].version.dependencies[1].sv, "^1.12.5");
    assert_eq!(list[1].project.authors().as_deref(), Some("bsmgmodders"));
    assert_eq!(list[2].project.authors(), None);
}

#[test]
fn orders_versions() {
    let cmp = |a, b| compare_semver(a, b);
    assert_eq!(cmp("4.3.7", "4.3.6"), Some(Ordering::Greater));
    assert_eq!(cmp("4.10.0", "4.9.9"), Some(Ordering::Greater));
    // Build metadata doesn't count.
    assert_eq!(cmp("4.3.6+1.29.1", "4.3.6"), Some(Ordering::Equal));
    assert_eq!(cmp("4.3.6+1.29.1", "4.3.7"), Some(Ordering::Less));
    // Pre-releases come before their release.
    assert_eq!(cmp("1.0.0-beta.2", "1.0.0"), Some(Ordering::Less));
    assert_eq!(cmp("1.0.0-beta.2", "1.0.0-beta.10"), Some(Ordering::Less));
    assert_eq!(cmp("1.0.0-alpha", "1.0.0-beta"), Some(Ordering::Less));
    assert_eq!(cmp("2024.10.7", "2024.9.30"), Some(Ordering::Greater));
    assert_eq!(cmp("v1.2", "1.2.0"), Some(Ordering::Equal));
    assert_eq!(cmp("one", "1.0.0"), None);
}

#[test]
fn reads_dependency_ranges() {
    let at_least = |v: &str| VersionReq::AtLeast(v.into());
    assert_eq!(version_req("^4.3.6"), at_least("4.3.6"));
    assert_eq!(version_req("~1.2.0"), at_least("1.2.0"));
    assert_eq!(version_req(">=1.0.0"), at_least("1.0.0"));
    assert_eq!(version_req(" ^ 1.0.0"), at_least("1.0.0"));
    assert_eq!(version_req("1.2.3"), VersionReq::Exact("1.2.3".into()));
    assert_eq!(version_req("=1.2.3"), VersionReq::Exact("1.2.3".into()));
    assert_eq!(version_req("*"), VersionReq::Any);
    assert_eq!(version_req(""), VersionReq::Any);
    assert_eq!(version_req("1.x"), VersionReq::Any);
    assert_eq!(version_req("^1.x"), VersionReq::Any);
    assert_eq!(version_req("1.0.0 - 2.0.0"), VersionReq::Any);
    assert_eq!(version_req(">1.0.0 <2.0.0"), VersionReq::Any);
}

#[test]
fn picks_the_version_for_the_game() {
    // The newest verified one for that game version, on Steam or for any
    // platform: not the Oculus 4.4.0 or the unverified 4.3.8.
    assert_eq!(picked(None, Some("1.44.1")).unwrap(), "4.3.7");
    assert_eq!(picked(None, Some("1.40.8")).unwrap(), "4.3.6");
    assert_eq!(picked(None, Some("1.29.1")).unwrap(), "4.3.6+1.29.1");
    // With the game version unknown, the newest there is.
    assert_eq!(picked(None, None).unwrap(), "4.3.7");

    let err = picked(None, Some("1.46.0")).unwrap_err().to_string();
    assert!(
        err.contains("BSIPA has no version for Beat Saber 1.46.0 yet"),
        "{err}"
    );
    assert!(err.contains("4.3.7, is for 1.41.1, 1.44.1)"), "{err}");
}

#[test]
fn picks_pinned_versions() {
    // Whatever the game version is.
    assert_eq!(picked(Some("4.3.6"), Some("1.44.1")).unwrap(), "4.3.6");
    assert_eq!(picked(Some("4.3.6+1.29.1"), None).unwrap(), "4.3.6+1.29.1");
    // Only ones that are allowed.
    assert!(picked(Some("4.4.0"), None).is_err());
    assert!(picked(Some("4.3.8"), None).is_err());

    let oculus = Config {
        game: "beatsaber",
        platforms: vec!["oculus"],
    };
    let page = bsipa();
    let v = pick(&oculus, &page, None, Some("1.44.1"), "Beat Saber").unwrap();
    assert_eq!(v.semver, "4.4.0");
}

#[test]
fn resolves_mods() {
    let page = bsipa();
    let resolved = to_resolved(&page.project, &page.versions[0], &HashMap::new()).unwrap();
    assert_eq!(resolved.id, "BSIPA");
    assert_eq!(resolved.name, "BSIPA");
    assert_eq!(resolved.version.as_deref(), Some("4.3.7"));
    assert_eq!(resolved.format, ArtifactFormat::Archive);
    assert_eq!(
        resolved.artifact,
        ArtifactLocation::Url {
            url: "https://beatmods.com/files/1/1119/BSIPA_universal_v4.3.7.zip".into()
        }
    );
    let versions = resolved.game_versions.unwrap();
    assert_eq!(versions.matches("1.44.1"), Some(true));
    assert_eq!(versions.matches("1.41.1"), Some(true));
    assert_eq!(versions.matches("1.42.0"), Some(false));
    assert!(resolved.dependencies.is_empty());

    // Without a download link, the one by hash (which redirects to it).
    let old = to_resolved(&page.project, &page.versions[1], &HashMap::new()).unwrap();
    assert_eq!(
        old.artifact,
        ArtifactLocation::Url {
            url: "https://beatmods.com/cdn/mod/aaaa.zip".into()
        }
    );
}

#[test]
fn names_dependencies() {
    let list = list();
    let sira = &list[0];
    let names: HashMap<u64, String> = [(1, "BSIPA".into()), (96, "BSML".into())].into();
    let resolved = to_resolved(&sira.project, &sira.version, &names).unwrap();
    let deps: Vec<(&str, String)> = resolved
        .dependencies
        .iter()
        .map(|d| (d.id.as_str(), d.req.to_string()))
        .collect();
    assert_eq!(
        deps,
        [("BSIPA", ">=4.3.6".into()), ("BSML", ">=1.12.5".into())]
    );
    assert!(resolved.dependencies.iter().all(|d| d.source == ID));

    let missing: HashMap<u64, String> = [(1, "BSIPA".into())].into();
    let err = to_resolved(&sira.project, &sira.version, &missing)
        .unwrap_err()
        .to_string();
    assert!(err.contains("number 96"), "{err}");
}

#[test]
fn finds_projects_by_name_or_number() {
    let list = list();
    assert_eq!(find_number(&list, "SiraUtil"), Some(58));
    assert_eq!(find_number(&list, "sirautil"), Some(58));
    assert_eq!(find_number(&list, "232"), Some(232));
    assert_eq!(find_number(&list, "999"), Some(999));
    assert_eq!(find_number(&list, "Nothing"), None);
}

#[test]
fn reads_the_game_config() {
    let game: GameDef = toml::from_str(
        r#"
        schema = 1
        id = "beat-saber"
        name = "Beat Saber"
        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap();
    let ctx = |config| SourceContext {
        game: &game,
        config,
        settings: None,
        env: None,
        links: patchwork_core::links::Links::none(),
    };
    let err = super::config(&ctx(None)).unwrap_err().to_string();
    assert!(err.contains("[sources.beatmods] game"), "{err}");

    let table: toml::Table = toml::from_str(r#"game = "beatsaber""#).unwrap();
    let c = super::config(&ctx(Some(&table))).unwrap();
    assert_eq!(c.game, "beatsaber");
    assert_eq!(c.platforms, PLATFORMS);

    let table: toml::Table = toml::from_str(
        r#"game = "beatsaber"
platforms = ["oculus"]"#,
    )
    .unwrap();
    assert_eq!(
        super::config(&ctx(Some(&table))).unwrap().platforms,
        ["oculus"]
    );
}

#[test]
fn links_to_pages() {
    let game: GameDef = toml::from_str(
        r#"
        schema = 1
        id = "beat-saber"
        name = "Beat Saber"
        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap();
    let ctx = SourceContext {
        game: &game,
        config: None,
        settings: None,
        env: None,
        links: patchwork_core::links::Links::none(),
    };
    let spec = BeatMods
        .parse_url(&ctx, "https://beatmods.com/mods/58")
        .unwrap()
        .unwrap();
    assert_eq!((spec.source.as_str(), spec.id.as_str()), (ID, "58"));
    for other in [
        "https://beatmods.com/mods",
        "https://beatmods.com/mods/SiraUtil",
        "https://beatmods.com/",
        "https://example.com/mods/58",
    ] {
        assert_eq!(BeatMods.parse_url(&ctx, other).unwrap(), None, "{other}");
    }

    assert_eq!(
        BeatMods.mod_url(&ctx, "58", None).as_deref(),
        Some("https://beatmods.com/mods/58")
    );
    NUMBERS.lock().unwrap().insert("SiraUtil".into(), 58);
    assert_eq!(
        BeatMods.mod_url(&ctx, "SiraUtil", None).as_deref(),
        Some("https://beatmods.com/mods/58")
    );
    assert_eq!(BeatMods.mod_url(&ctx, "NeverListed", None), None);
}

#[test]
fn searches() {
    let list = list();
    let query = |text: &str| SearchQuery {
        text: Some(text.into()),
        sort: None,
        categories: Vec::new(),
        excluded_categories: Vec::new(),
        page: 0,
    };
    let ids = |page: SearchPage| -> Vec<String> { page.items.into_iter().map(|m| m.id).collect() };
    let search = |q: &SearchQuery| search_page(&config(), &list, q);

    // Everything usable, by name, once each: Camera2's Oculus file and
    // the Oculus-only mod are left out.
    let all = search(&query(""));
    assert_eq!(all.total, Some(3));
    assert_eq!(ids(all), ["Camera2", "SiraUtil", "yt-dlp"]);
    // Words in names, summaries and authors, ignoring case.
    assert_eq!(ids(search(&query("ZENJECT"))), ["SiraUtil"]);
    assert_eq!(ids(search(&query("auros"))), ["SiraUtil"]);
    assert_eq!(ids(search(&query("bsmg"))), ["yt-dlp"]);
    assert!(ids(search(&query("oculus"))).is_empty());
    // Names first, then by name.
    assert_eq!(ids(search(&query("a"))), ["Camera2", "SiraUtil", "yt-dlp"]);
    assert_eq!(ids(search(&query("util"))), ["SiraUtil"]);
    assert_eq!(ids(search(&query("s"))), ["SiraUtil", "Camera2", "yt-dlp"]);

    // Categories.
    let mut q = query("");
    q.categories = vec!["library".into()];
    assert_eq!(ids(search(&q)), ["SiraUtil", "yt-dlp"]);
    q.categories.clear();
    q.excluded_categories = vec!["Library".into()];
    assert_eq!(ids(search(&q)), ["Camera2"]);

    // Most recently updated first; ones that don't say last.
    let mut q = query("");
    q.sort = Some(SortBy::Updated);
    let page = search(&q);
    assert_eq!(page.sort, SortBy::Updated);
    assert_eq!(ids(page), ["yt-dlp", "SiraUtil", "Camera2"]);

    let summary = to_summary(&list[0].project, Some(&list[0].version));
    assert_eq!(summary.url.as_deref(), Some("https://beatmods.com/mods/58"));
    assert_eq!(summary.author.as_deref(), Some("Auros"));
    assert_eq!(summary.latest_version.as_deref(), Some("3.1.14"));
    assert_eq!(summary.categories[0].id, "Library");
}

#[test]
fn ranks_names_first() {
    let list = list();
    let sira = &list[0].project;
    assert_eq!(rank(sira, "sira"), 0);
    assert_eq!(rank(sira, "util"), 1);
    assert_eq!(rank(sira, "zenject"), 2);
}

/// A mod numbered `id` whose newest version is for `game_versions`, on
/// `platform`.
fn entry(id: u64, platform: &str, game_versions: &[&str]) -> Entry {
    let versions: Vec<_> = game_versions
        .iter()
        .map(|v| serde_json::json!({ "version": v }))
        .collect();
    serde_json::from_value(serde_json::json!({
        "project": { "id": id, "name": format!("Mod{id}"), "nameId": format!("Mod{id}") },
        "version": {
            "semver": "1.0.0", "supportedGameVersions": versions, "status": "verified",
            "platform": platform, "zipHash": "0000"
        }
    }))
    .unwrap()
}

#[test]
fn finds_the_version_most_mods_are_for() {
    let mut entries: Vec<Entry> = (0..6)
        .map(|id| entry(id, "universal", &["1.42.0", "1.44.1"]))
        .collect();
    entries.extend((6..10).map(|id| entry(id, "universal", &["1.40.8"])));
    // A couple out early for a newer version don't count, nor do ones on
    // another platform.
    entries.push(entry(10, "universal", &["1.46.0"]));
    entries.push(entry(11, "steam", &["1.46.0", "1.44.1"]));
    entries.extend((12..30).map(|id| entry(id, "oculus", &["1.46.0"])));
    assert_eq!(
        moddable(&config(), &entries),
        Some(("1.44.1".to_owned(), 7))
    );
    assert_eq!(moddable(&config(), &[]), None);
}

#[test]
fn explains_game_versions_with_no_mods() {
    let err = no_mods_for("Beat Saber", "1.46.0", Some(("1.44.1", 115)));
    let blocker = err.blocker().unwrap();
    assert_eq!(blocker.title, "No mods for Beat Saber 1.46.0 yet");
    assert!(blocker.fixes.is_empty(), "{blocker:?}");
    assert!(
        blocker.detail.contains("pick 1.44.1 under Betas"),
        "{blocker:?}"
    );
    assert!(blocker.detail.contains("(115 mods)"), "{blocker:?}");
    // Where only the message is shown, as in the CLI, it says what to do.
    let message = err.to_string();
    assert!(message.contains("Beat Saber 1.46.0 has no mods on BeatMods yet"));
    assert!(message.contains("switch to 1.44.1"), "{message}");

    let blocker = no_mods_for("Beat Saber", "1.46.0", None).blocker().unwrap();
    assert!(blocker.detail.contains("Wait for them"), "{blocker:?}");
}
