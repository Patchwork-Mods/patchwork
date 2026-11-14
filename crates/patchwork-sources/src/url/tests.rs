use super::*;
use patchwork_core::ext::testing::serve;
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
async fn resolves_files() {
    let game = game();
    let ctx = SourceContext {
        game: &game,
        config: None,
        settings: None,
        env: None,
        links: patchwork_core::links::Links::none(),
    };
    let file = || "200 OK\r\nContent-Type: application/octet-stream";
    let base = serve(vec![
        ("/files/Cool.Mod.zip?dl=1", file(), "zip".to_owned()),
        ("/thing_P.pak", file(), "pak".to_owned()),
        // no Content-Type at all is taken as a file
        ("/a.7z", "200 OK", "7z".to_owned()),
        ("/b.RAR", file(), "rar".to_owned()),
        (
            "/drive/folders/1EYSBtvs",
            "200 OK\r\nContent-Type: text/html; charset=utf-8",
            "<html>Armory 1.0.zip</html>".to_owned(),
        ),
    ]);
    let url = |path: &str| format!("{base}{path}");

    let zip = UrlSource
        .resolve(&ctx, &url("/files/Cool.Mod.zip?dl=1"), None)
        .await
        .unwrap();
    assert_eq!(zip.name, "Cool.Mod");
    assert_eq!(zip.format, ArtifactFormat::Archive);
    let pak = UrlSource
        .resolve(&ctx, &url("/thing_P.pak"), None)
        .await
        .unwrap();
    assert_eq!(pak.format, ArtifactFormat::File);
    for archive in ["/a.7z", "/b.RAR"] {
        let resolved = UrlSource.resolve(&ctx, &url(archive), None).await.unwrap();
        assert_eq!(resolved.format, ArtifactFormat::Archive, "{archive}");
    }

    let folder = UrlSource
        .resolve(&ctx, &url("/drive/folders/1EYSBtvs"), None)
        .await
        .unwrap_err();
    assert!(folder.to_string().contains("web page"), "{folder}");
    let missing = UrlSource
        .resolve(&ctx, &url("/gone.zip"), None)
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("404"), "{missing}");

    for bad in [
        "ftp://example.com/a.zip".to_owned(),
        // no file name, so not fetched
        "https://example.invalid/".to_owned(),
    ] {
        assert!(UrlSource.resolve(&ctx, &bad, None).await.is_err(), "{bad}");
    }
    assert!(
        UrlSource
            .resolve(&ctx, &url("/a.7z"), Some("1.0"))
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

#[tokio::test]
async fn folder_links_list_their_files_to_pick_from() {
    let game = game();
    let base = serve(vec![(
        "/list/top",
        "200 OK",
        r#"<a href="{base}/folder/sub">Old</a><a href="{base}/f/abc">Mod 1.0.zip</a><a href="{base}/f/def">Manual.PDF</a>"#
            .to_owned(),
    )]);
    let links = patchwork_core::links::Links::parse(&[(
        "test",
        &r#"
        name = "Test"
        links = '^BASE_RE/f/(?<key>\w+)'
        [folder]
        links = '^BASE_RE/folder/(?<id>\w+)'
        fetch = "BASE/list/${id}"
        entries = '<a href="(?<link>[^"]+)">(?<name>[^<]*)</a>'
        "#
        // the dots in the base match themselves too
        .replace("BASE_RE", &base)
        .replace("BASE", &base),
    )])
    .unwrap();
    let ctx = SourceContext {
        game: &game,
        config: None,
        settings: None,
        env: None,
        links: &links,
    };

    let top = format!("{base}/folder/top");
    let choices = UrlSource.choices(&ctx, &top).await.unwrap();
    let summary: Vec<_> = choices
        .iter()
        .map(|c| (c.name.as_str(), c.id.clone(), c.folder, c.not_mod))
        .collect();
    assert_eq!(
        summary,
        [
            ("Old", format!("{base}/folder/sub"), true, false),
            ("Mod 1.0.zip", format!("{base}/f/abc"), false, false),
            ("Manual.PDF", format!("{base}/f/def"), false, true),
        ]
    );

    let err = UrlSource.resolve(&ctx, &top, None).await.unwrap_err();
    assert!(err.to_string().contains("is a folder on Test"), "{err}");
    // other links have nothing to choose between
    let file = format!("{base}/f/abc");
    assert!(UrlSource.choices(&ctx, &file).await.unwrap().is_empty());
}
