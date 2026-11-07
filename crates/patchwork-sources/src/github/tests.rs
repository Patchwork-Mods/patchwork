use super::*;

#[test]
fn infers_patterns() {
    let cases = [
        // the tag's version, with or without a `v`
        ("ModName-0.1.2.zip", "v0.1.2", "ModName-*.zip"),
        ("ModName-v0.1.2.zip", "v0.1.2", "ModName-v*.zip"),
        ("ModName_1.4.dll", "release-1.4", "ModName_*.dll"),
        ("mod-1.2.3.tar.gz", "1.2.3", "mod-*.tar.gz"),
        // case doesn't matter, but the file's own case is kept
        ("MyMod-1.0-BETA.zip", "v1.0-beta", "MyMod-*.zip"),
        // the version's last appearance, which is usually the mod's own
        ("Mod2-for-2.zip", "v2", "Mod2-for-*.zip"),
        // more parts than the tag has
        ("BepInEx_x64_5.4.22.0.zip", "v5.4.22", "BepInEx_x64_*.zip"),
        // not part of a longer number
        ("Mod-12-2.zip", "v2", "Mod-12-*.zip"),
        // a version at the end of the name isn't taken for an extension
        ("Mod-1.2.7z", "v1.2", "Mod-*.7z"),
        // no tag version in the name: dotted numbers, but not lone ones
        ("Mod-x64-1.4.2.zip", "release-7", "Mod-x64-*.zip"),
        ("Mod-x64.zip", "release-7", "Mod-x64.zip"),
        // nothing that changes
        ("ModName.zip", "v0.1.2", "ModName.zip"),
        ("ModName.zip", "nightly", "ModName.zip"),
    ];
    for (name, tag, expected) in cases {
        assert_eq!(pattern::infer(name, tag), expected, "{name} from {tag}");
    }
}

#[test]
fn matches_patterns() {
    let yes = [
        ("ModName-*.zip", "ModName-0.1.3.zip"),
        ("ModName-*.zip", "modname-1.0.0-beta.ZIP"),
        ("ModName.zip", "ModName.zip"),
        ("*-x64-*.zip", "Mod-x64-1.2.zip"),
        ("a*a", "aa"),
    ];
    let no = [
        ("ModName-*.zip", "ModName-0.1.3.7z"),
        ("ModName-*.zip", "OtherMod-0.1.3.zip"),
        ("ModName.zip", "ModName-1.zip"),
        ("a*a", "a"),
        ("*-x64-*.zip", "Mod-arm64-1.2.zip"),
    ];
    for (p, name) in yes {
        assert!(pattern::matches(p, name), "{p} should match {name}");
    }
    for (p, name) in no {
        assert!(!pattern::matches(p, name), "{p} shouldn't match {name}");
    }
}

#[test]
fn parses_ids() {
    assert_eq!(
        ModId::parse("someone/SomeMod/SomeMod-*.zip").unwrap(),
        ModId {
            owner: "someone",
            repo: "SomeMod",
            pattern: Some("SomeMod-*.zip"),
        }
    );
    assert_eq!(ModId::parse("someone/SomeMod").unwrap().pattern, None);
    assert_eq!(ModId::parse("someone/SomeMod/").unwrap().pattern, None);
    for bad in ["someone", "some one/mod", "../x", "a//b"] {
        assert!(ModId::parse(bad).is_err(), "{bad}");
    }
}

#[test]
fn parses_links() {
    let cases = [
        (
            "https://github.com/someone/SomeMod",
            Some("someone/SomeMod"),
        ),
        (
            "https://github.com/someone/SomeMod.git",
            Some("someone/SomeMod"),
        ),
        (
            "https://github.com/someone/SomeMod/releases/tag/v1.2",
            Some("someone/SomeMod"),
        ),
        (
            "https://github.com/someone/SomeMod/tree/main/src?x=1",
            Some("someone/SomeMod"),
        ),
        (
            "https://github.com/someone/SomeMod/releases/download/v1.2/SomeMod-1.2.zip",
            Some("someone/SomeMod/SomeMod-*.zip"),
        ),
        (
            "https://github.com/someone/SomeMod/releases/tag/v1.2#file=SomeMod-*.zip",
            Some("someone/SomeMod/SomeMod-*.zip"),
        ),
        ("https://github.com/someone", None),
        ("https://github.com/settings/profile", None),
        ("https://gitlab.com/someone/SomeMod", None),
    ];
    for (url, expected) in cases {
        assert_eq!(parse_link(url).as_deref(), expected, "{url}");
    }
}

#[test]
fn links_round_trip() {
    let game: patchwork_core::game::GameDef = toml::from_str(
        r#"
        schema = 1
        id = "some-game"
        name = "Some Game"
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
    };
    let id = "someone/SomeMod/SomeMod-*.zip";
    let url = GitHub.mod_url(&ctx, id, Some("v1.2")).unwrap();
    assert_eq!(
        url,
        "https://github.com/someone/SomeMod/releases/tag/v1.2#file=SomeMod-*.zip"
    );
    let spec = GitHub.parse_url(&ctx, &url).unwrap().unwrap();
    assert_eq!(spec.id, id);
    assert_eq!(spec.version, None);
}

const RELEASES: &str = r#"[
    {
        "tag_name": "v0.3.0-rc1",
        "prerelease": true,
        "body": "testing",
        "assets": [
            { "name": "SomeMod-0.3.0-rc1.zip", "size": 3, "browser_download_url": "https://dl/SomeMod-0.3.0-rc1.zip" }
        ]
    },
    {
        "tag_name": "v0.2.0",
        "body": "",
        "assets": [
            { "name": "SomeMod-0.2.0-debug.zip", "size": 2, "browser_download_url": "https://dl/SomeMod-0.2.0-debug.zip" },
            { "name": "SomeMod-0.2.0.zip", "size": 2, "browser_download_url": "https://dl/SomeMod-0.2.0.zip" },
            { "name": "Extras.7z", "size": 9, "browser_download_url": "https://dl/Extras.7z" }
        ]
    },
    {
        "tag_name": "docs",
        "body": "no files",
        "assets": []
    },
    {
        "tag_name": "v0.1.0",
        "body": "first",
        "assets": [
            { "name": "SomeMod-0.1.0.zip", "size": 1, "browser_download_url": "https://dl/SomeMod-0.1.0.zip" }
        ]
    },
    {
        "tag_name": "v9",
        "draft": true,
        "assets": []
    }
]"#;

fn releases() -> Vec<Release> {
    serde_json::from_str(RELEASES).unwrap()
}

#[test]
fn picks_releases() {
    let releases = releases();
    let tag = |pattern| newest(&releases, pattern).map(|r| r.tag_name.as_str());
    // the prerelease is newer, but a full release has the file too
    assert_eq!(tag(Some("SomeMod-*.zip")), Some("v0.2.0"));
    assert_eq!(tag(None), Some("v0.2.0"));
    // only the prerelease has a file named like this
    assert_eq!(tag(Some("SomeMod-*-rc1.zip")), Some("v0.3.0-rc1"));
    assert_eq!(tag(Some("Nothing.zip")), None);
}

#[test]
fn picks_files() {
    let releases = releases();
    let file = |tag: &str, pattern| {
        let release = releases.iter().find(|r| r.tag_name == tag).unwrap();
        pick_asset(release, pattern).map(|a| a.name.as_str())
    };
    // `*` matches `0.2.0-debug` too, but the name with the release's version wins
    assert_eq!(file("v0.2.0", "SomeMod-*.zip"), Some("SomeMod-0.2.0.zip"));
    assert_eq!(file("v0.2.0", "Extras.7z"), Some("Extras.7z"));
    assert_eq!(file("v0.1.0", "Extras.7z"), None);
}

#[test]
fn offers_the_newest_releases_files() {
    let releases = releases();
    let id = ModId::parse("someone/SomeMod").unwrap();
    let offered = choices(&id, release(&releases, &id, None).unwrap());
    let ids: Vec<&str> = offered.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "someone/SomeMod/SomeMod-*-debug.zip",
            "someone/SomeMod/SomeMod-*.zip",
            "someone/SomeMod/Extras.7z",
        ]
    );
    assert_eq!(offered[1].name, "SomeMod-0.2.0.zip");
}

#[test]
fn resolves_from_the_picked_file() {
    let releases = releases();
    let id = ModId::parse("someone/SomeMod/SomeMod-*.zip").unwrap();
    let release = release(&releases, &id, Some("v0.1.0")).unwrap();
    let asset = pick_asset(release, id.pattern.unwrap()).unwrap();
    let resolved = to_resolved(&id, id.pattern.unwrap(), release, asset);
    assert_eq!(resolved.id, "someone/SomeMod/SomeMod-*.zip");
    assert_eq!(resolved.name, "SomeMod");
    assert_eq!(resolved.version.as_deref(), Some("v0.1.0"));
    assert_eq!(resolved.format, ArtifactFormat::Archive);
}

#[test]
fn orders_tags() {
    assert_eq!(
        GitHub.compare_versions("v1.10", "v1.9"),
        Some(Ordering::Greater)
    );
    assert_eq!(
        GitHub.compare_versions("release-2", "v2.0"),
        Some(Ordering::Equal)
    );
    assert_eq!(GitHub.compare_versions("nightly", "v1"), None);
}
