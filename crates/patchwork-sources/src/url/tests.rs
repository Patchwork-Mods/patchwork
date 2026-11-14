use super::*;
use patchwork_core::game::GameDef;

fn game() -> GameDef {
    toml::from_str(
        r#"
        schema = 1
        id = "g"
        name = "G"
        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap()
}

#[tokio::test]
async fn resolves_without_fetching() {
    let game = game();
    let ctx = SourceContext {
        game: &game,
        config: None,
        settings: None,
        env: None,
        links: patchwork_core::links::Links::none(),
    };
    let zip = UrlSource
        .resolve(&ctx, "https://example.com/files/Cool.Mod.zip?dl=1", None)
        .await
        .unwrap();
    assert_eq!(zip.name, "Cool.Mod");
    assert_eq!(zip.format, ArtifactFormat::Archive);
    let pak = UrlSource
        .resolve(&ctx, "https://example.com/thing_P.pak", None)
        .await
        .unwrap();
    assert_eq!(pak.format, ArtifactFormat::File);
    for archive in ["https://example.com/a.7z", "https://example.com/b.RAR"] {
        let resolved = UrlSource.resolve(&ctx, archive, None).await.unwrap();
        assert_eq!(resolved.format, ArtifactFormat::Archive, "{archive}");
    }

    for bad in ["ftp://example.com/a.zip", "https://example.com/"] {
        assert!(UrlSource.resolve(&ctx, bad, None).await.is_err(), "{bad}");
    }
    assert!(
        UrlSource
            .resolve(&ctx, "https://example.com/a.zip", Some("1.0"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn hosted_links_are_left_to_the_host() {
    let game = game();
    let links = patchwork_core::links::Links::parse(&[
        (
            "files",
            r#"
            name = "Files"
            links = '^https://files\.example/f/(?<key>\w+)(?:/(?<file>[^/?#]+))?'
            link = ["https://files.example/f/${key}/${file}", "https://files.example/f/${key}"]
            file = "${file}"
            "#,
        ),
        (
            "browser",
            r#"
            name = "Browser"
            links = '^https://browser\.example/'
            browser = true
            format = "archive"
            "#,
        ),
        (
            "unknown",
            "name = 'Unknown'\nlinks = '^https://unknown\\.example/'\nbrowser = true",
        ),
    ])
    .unwrap();
    let ctx = SourceContext {
        game: &game,
        config: None,
        settings: None,
        env: None,
        links: &links,
    };

    let named = UrlSource
        .resolve(&ctx, "https://files.example/f/k1/My%20Mod.ftl?junk", None)
        .await
        .unwrap();
    assert_eq!(named.id, "https://files.example/f/k1/My%20Mod.ftl");
    assert_eq!(named.name, "My Mod");
    assert_eq!(named.format, ArtifactFormat::File);
    assert_eq!(
        named.artifact,
        ArtifactLocation::Source {
            file: "https://files.example/f/k1/My%20Mod.ftl".to_owned(),
            name: "My Mod.ftl".to_owned(),
        }
    );
    assert_eq!(
        UrlSource
            .download(&ctx, &named.id, "https://files.example/f/k1/My%20Mod.ftl")
            .await
            .unwrap(),
        Download::Url("https://files.example/f/k1/My%20Mod.ftl".to_owned())
    );

    let page = "https://browser.example/mods/x/downloads/cool-mod-1-2";
    let browser = UrlSource.resolve(&ctx, page, None).await.unwrap();
    assert_eq!(browser.name, "cool-mod-1-2");
    assert_eq!(browser.format, ArtifactFormat::Archive);
    assert_eq!(
        UrlSource.download(&ctx, page, page).await.unwrap(),
        Download::NeedsUser {
            page: page.to_owned()
        }
    );

    let err = UrlSource
        .resolve(&ctx, "https://unknown.example/thing", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("yourself"), "{err}");
    assert!(
        UrlSource
            .download(&ctx, "x", "https://nowhere.example/a.zip")
            .await
            .is_err()
    );
}
