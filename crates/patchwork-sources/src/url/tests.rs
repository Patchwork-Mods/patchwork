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
    };
    let zip = UrlSource
        .resolve(&ctx, "https://example.com/files/Cool.Mod.zip?dl=1", None)
        .await
        .unwrap();
    assert_eq!(zip.name, "Cool.Mod");
    assert_eq!(zip.format, ArtifactFormat::Zip);
    let pak = UrlSource
        .resolve(&ctx, "https://example.com/thing_P.pak", None)
        .await
        .unwrap();
    assert_eq!(pak.format, ArtifactFormat::File);

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
