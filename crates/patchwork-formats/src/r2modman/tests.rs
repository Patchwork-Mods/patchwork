use std::collections::BTreeMap;

use patchwork_core::event::Silent;
use patchwork_core::hash::ContentHash;
use patchwork_core::lock::{LOCK_SCHEMA, LockedGame, LockedMod};
use patchwork_core::source::{ArtifactFormat, ArtifactLocation};

use super::*;

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

fn profile_mod(source: &str, id: &str, enabled: bool) -> ProfileMod {
    ProfileMod {
        name: id.to_owned(),
        source: source.to_owned(),
        id: id.to_owned(),
        version: None,
        enabled,
        game_versions: None,
        fomod: None,
        ext: Default::default(),
    }
}

fn locked(source: &str, id: &str, version: &str, dependencies: &[&str]) -> LockedMod {
    LockedMod {
        name: id.to_owned(),
        source: source.to_owned(),
        id: id.to_owned(),
        requested: None,
        version: Some(version.to_owned()),
        hash: ContentHash::of(id.as_bytes()),
        format: ArtifactFormat::Archive,
        artifact: ArtifactLocation::Url {
            url: format!("https://example.com/{id}.zip"),
        },
        dependencies: dependencies.iter().map(|d| d.to_string()).collect(),
        requires: Default::default(),
        game_versions: None,
        env: Default::default(),
        kind: None,
    }
}

#[tokio::test]
async fn r2z_round_trips() {
    let game = lethal_company();
    let sources = BTreeMap::new();
    let scratch = tempfile::tempdir().unwrap();
    let ctx = FormatContext {
        game: Some(&game),
        sources: &sources,
        settings: Default::default(),
        reporter: &Silent,
        scratch: scratch.path(),
    };
    let shared = SharedProfile {
        name: Some("My Pack".to_owned()),
        mods: vec![
            profile_mod(SOURCE, "a-More", true),
            profile_mod(SOURCE, "b-Off", false),
            profile_mod("local", "/x/thing.zip", true),
        ],
        lock: Some(Lockfile {
            schema: LOCK_SCHEMA,
            game: LockedGame {
                id: "lethal-company".to_owned(),
                env: Default::default(),
            },
            mods: vec![
                locked(SOURCE, "a-More", "1.2.3", &["thunderstore:BepInEx-Pack"]),
                locked(SOURCE, "b-Off", "0.1.0", &["thunderstore:c-OnlyForOff"]),
                locked("local", "/x/thing.zip", "", &[]),
                locked(SOURCE, "BepInEx-Pack", "5.4.2100", &[]),
                locked(SOURCE, "c-OnlyForOff", "2.0.0", &[]),
            ],
        }),
        ..Default::default()
    };

    let r2z = R2modman.export(&ctx, &shared).await.unwrap();
    assert!(R2modman.detect(&r2z));
    let imported = R2modman.import(&ctx, &r2z).await.unwrap();
    assert_eq!(imported.name.as_deref(), Some("My Pack"));
    let mods: Vec<_> = imported
        .mods
        .iter()
        .map(|m| (m.id.as_str(), m.version.as_deref().unwrap(), m.enabled))
        .collect();
    assert_eq!(
        mods,
        [
            ("a-More", "1.2.3", true),
            ("b-Off", "0.1.0", false),
            ("BepInEx-Pack", "5.4.2100", true),
            ("c-OnlyForOff", "2.0.0", false),
        ]
    );
}

#[test]
fn reads_r2modman_manifests() {
    // As written by r2modman itself.
    let manifest: Manifest = serde_norway::from_str(
        "profileName: Friends\nmods:\n  - name: BepInEx-BepInExPack\n    version:\n      major: 5\n      minor: 4\n      patch: 2100\n    enabled: true\n",
    )
    .unwrap();
    assert_eq!(manifest.profile_name, "Friends");
    assert_eq!(manifest.mods[0].version.patch, 2100);
}

#[test]
fn code_payloads_round_trip() {
    let payload = encode_code_payload(b"zip bytes");
    assert!(payload.starts_with("#r2modman\n"));
    assert_eq!(decode_code_payload(&payload).unwrap(), b"zip bytes");
    assert!(decode_code_payload("<html>").is_err());
}

#[test]
fn detects_codes() {
    let code = ThunderstoreCode::default();
    assert!(code.detect(b"0191c1ee-8c95-0d38-c6ef-7f0d55b8e8b5\n"));
    assert!(!code.detect(b"https://thunderstore.io/"));
    assert!(!code.detect(b"short"));
}
