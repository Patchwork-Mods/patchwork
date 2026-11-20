use super::*;

use patchwork_core::environment::GameEnv;
use patchwork_core::game::GameDef;

/// Cut down from the real catalog, with its quirks: CDATA with whitespace
/// around it, a mod with a file per system, and one that's a lone `.dll`.
const MODS: &str = r#"<?xml version="1.0"?>
<ModLinks xmlns="https://github.com/HollowKnight-Modding/HollowKnight.ModLinks/HollowKnight.ModManager">
    <Manifest>
        <Name>ItemReveal</Name>
        <Description>Reveals items.</Description>
        <Version>0.1.0.0</Version>
        <Link SHA256="8b2a"><![CDATA[https://github.com/x/ItemReveal/releases/download/v0.1.0.0/ItemReveal.zip]]></Link>
        <Dependencies>
            <Dependency>Randomizer 4</Dependency>
            <Dependency>MenuChanger</Dependency>
        </Dependencies>
        <Repository>
            <![CDATA[https://github.com/x/ItemReveal]]>
        </Repository>
        <Tags>
            <Tag>Gameplay</Tag>
        </Tags>
        <Authors>
            <Author>someone</Author>
            <Author>someone else</Author>
        </Authors>
    </Manifest>
    <Manifest>
        <Name>Pale Court</Name>
        <DisplayName>Pale Court</DisplayName>
        <Description>Challenge Hallownest's mightiest champions!</Description>
        <Version>1.1.1.7</Version>
        <Links>
            <Linux SHA256="3232"><![CDATA[https://github.com/PaleCourt/PaleCourt/releases/download/1.1.1.7/PaleCourt-Lin.zip]]></Linux>
            <Mac SHA256="57B8"><![CDATA[https://github.com/PaleCourt/PaleCourt/releases/download/1.1.1.7/PaleCourt-Mac.zip]]></Mac>
            <Windows SHA256="1646">
                <![CDATA[https://github.com/PaleCourt/PaleCourt/releases/download/1.1.1.7/PaleCourt-Win.zip]]>
            </Windows>
        </Links>
        <Dependencies />
        <Repository><![CDATA[https://github.com/PaleCourt/PaleCourt]]></Repository>
        <Tags>
            <Tag>Boss</Tag>
            <Tag>Expansion</Tag>
        </Tags>
    </Manifest>
    <Manifest>
        <Name>MenuChanger</Name>
        <DisplayName>Menu Changer</DisplayName>
        <Description>A library for menus.</Description>
        <Version>1.0.0.10</Version>
        <Link SHA256="aaaa"><![CDATA[https://github.com/x/MenuChanger/releases/download/v1/MenuChanger.dll]]></Link>
        <Dependencies></Dependencies>
        <Repository><![CDATA[https://github.com/x/MenuChanger]]></Repository>
        <Tags>
            <Tag>Library</Tag>
        </Tags>
    </Manifest>
</ModLinks>
"#;

const API: &str = r#"<?xml version="1.0"?>
<ApiLinks xmlns="https://github.com/HollowKnight-Modding/HollowKnight.ModLinks/HollowKnight.ModManager">
    <Manifest>
        <Version>77</Version>
        <Links>
            <Linux SHA256="5d23">
                <![CDATA[https://github.com/hk-modding/api/releases/download/1.5.78.11833-77/moddingapi.v77.linux.zip]]>
            </Linux>
            <Windows SHA256="bc9f">
                <![CDATA[https://github.com/hk-modding/api/releases/download/1.5.78.11833-77/moddingapi.v77.windows.zip]]>
            </Windows>
        </Links>
        <Files>
            <File>Assembly-CSharp.dll</File>
        </Files>
    </Manifest>
</ApiLinks>
"#;

fn mods() -> Vec<Manifest> {
    parse(MODS).unwrap()
}

#[test]
fn parses_catalogs() {
    let mods = mods();
    assert_eq!(mods.len(), 3);
    let reveal = &mods[0];
    assert_eq!(reveal.name, "ItemReveal");
    assert_eq!(reveal.display_name, None);
    assert_eq!(reveal.version, "0.1.0.0");
    assert_eq!(
        reveal.links,
        Links::One(
            "https://github.com/x/ItemReveal/releases/download/v0.1.0.0/ItemReveal.zip".into()
        )
    );
    assert_eq!(reveal.dependencies, ["Randomizer 4", "MenuChanger"]);
    assert_eq!(
        reveal.repository.as_deref(),
        Some("https://github.com/x/ItemReveal")
    );
    assert_eq!(reveal.authors, ["someone", "someone else"]);

    let Links::PerSystem(links) = &mods[1].links else {
        panic!("Pale Court has a file per system");
    };
    let systems: Vec<&str> = links.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(systems, ["Linux", "Mac", "Windows"]);
    assert_eq!(
        links[2].1,
        "https://github.com/PaleCourt/PaleCourt/releases/download/1.1.1.7/PaleCourt-Win.zip"
    );
    assert!(mods[1].dependencies.is_empty());

    let api = parse(API).unwrap();
    assert_eq!(api.len(), 1);
    assert_eq!(api[0].name, "");
    assert_eq!(api[0].version, "77");
}

#[test]
fn rejects_broken_xml() {
    assert!(parse("<ModLinks><Manifest>").is_err());
}

#[test]
fn resolves_mods() {
    let mods = mods();
    let reveal = to_resolved(&mods[0], "linux", true).unwrap();
    assert_eq!(reveal.id, "ItemReveal");
    assert_eq!(reveal.version.as_deref(), Some("0.1.0.0"));
    assert_eq!(reveal.format, ArtifactFormat::Archive);
    assert_eq!(reveal.kind, None);
    let deps: Vec<&str> = reveal.dependencies.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(deps, [API_ID, "Randomizer 4", "MenuChanger"]);
    assert!(reveal.dependencies.iter().all(|d| d.source == ID));

    // No API catalog, no API to depend on.
    let reveal = to_resolved(&mods[0], "linux", false).unwrap();
    assert_eq!(reveal.dependencies.len(), 2);

    let menu = to_resolved(&mods[2], "linux", true).unwrap();
    assert_eq!(menu.name, "Menu Changer");
    assert_eq!(menu.format, ArtifactFormat::File);
}

#[test]
fn picks_each_systems_file() {
    let court = &mods()[1];
    let url = |platform| match to_resolved(court, platform, true).unwrap().artifact {
        ArtifactLocation::Url { url } => url,
        other => panic!("{other:?}"),
    };
    assert!(url("windows").ends_with("PaleCourt-Win.zip"));
    assert!(url("linux").ends_with("PaleCourt-Lin.zip"));
    assert!(url("mac").ends_with("PaleCourt-Mac.zip"));

    let mut api = parse(API).unwrap().remove(0);
    api.name = API_ID.into();
    let resolved = to_resolved(&api, "windows", true).unwrap();
    assert_eq!(resolved.kind.as_deref(), Some(API_ID));
    assert!(resolved.dependencies.is_empty(), "the API needs nothing");
    let err = to_resolved(&api, "mac", true).unwrap_err().to_string();
    assert!(err.contains("Linux, Windows"), "{err}");
}

#[test]
fn platform_comes_from_the_profile() {
    let game: GameDef = toml::from_str(
        r#"
        schema = 1
        id = "hollow-knight"
        name = "Hollow Knight"
        [env.platform]
        name = "Game build"
        choices = ["windows", "linux"]
        [deployer]
        kind = "rules"
        "#,
    )
    .unwrap();
    let env = GameEnv::from_picks(&[("platform".into(), "windows".into())].into());
    let ctx = |env| SourceContext {
        game: &game,
        config: None,
        settings: None,
        env,
        links: patchwork_core::links::Links::none(),
    };
    assert_eq!(platform(&ctx(Some(&env))), "windows");
    let own = platform(&ctx(None));
    assert!(["windows", "linux", "mac"].contains(&own));
}

#[test]
fn searches() {
    let mods = mods();
    let query = |text: &str| SearchQuery {
        text: Some(text.into()),
        sort: None,
        categories: Vec::new(),
        excluded_categories: Vec::new(),
        page: 0,
    };
    let names =
        |page: SearchPage| -> Vec<String> { page.items.into_iter().map(|m| m.id).collect() };

    // Everything, by name.
    assert_eq!(
        names(search_page(&mods, &query(""))),
        ["ItemReveal", "MenuChanger", "Pale Court"]
    );
    // Words anywhere, ignoring case: names, descriptions, authors.
    assert_eq!(names(search_page(&mods, &query("menu"))), ["MenuChanger"]);
    assert_eq!(
        names(search_page(&mods, &query("HALLOWNEST"))),
        ["Pale Court"]
    );
    assert_eq!(
        names(search_page(&mods, &query("someone reveals"))),
        ["ItemReveal"]
    );
    assert!(names(search_page(&mods, &query("nothing like it"))).is_empty());

    // Names first: Pale Court's description says "champions", but
    // MenuChanger's name has "changer" in it.
    assert_eq!(rank(&mods[2], "menu"), 0);
    assert_eq!(rank(&mods[2], "changer"), 1);
    assert_eq!(rank(&mods[1], "champions"), 2);
    let mut both = mods.clone();
    both[1].description = Some("A changer of courts.".into());
    assert_eq!(
        names(search_page(&both, &query("changer"))),
        ["MenuChanger", "Pale Court"]
    );

    // Tags.
    let mut tagged = query("");
    tagged.categories = vec!["library".into()];
    assert_eq!(names(search_page(&mods, &tagged)), ["MenuChanger"]);
    tagged.categories.clear();
    tagged.excluded_categories = vec!["Boss".into()];
    assert_eq!(
        names(search_page(&mods, &tagged)),
        ["ItemReveal", "MenuChanger"]
    );

    let summary = to_summary(&mods[0]);
    assert_eq!(summary.author.as_deref(), Some("someone, someone else"));
    assert_eq!(summary.latest_version.as_deref(), Some("0.1.0.0"));
    assert_eq!(summary.categories[0].id, "Gameplay");
}

#[test]
fn pages_searches() {
    let many: Vec<Manifest> = (0..PAGE_SIZE + 3)
        .map(|i| Manifest {
            name: format!("Mod{i:03}"),
            ..mods().remove(2)
        })
        .collect();
    let mut query = SearchQuery {
        text: None,
        sort: None,
        categories: Vec::new(),
        excluded_categories: Vec::new(),
        page: 0,
    };
    let first = search_page(&many, &query);
    assert_eq!(first.items.len(), PAGE_SIZE);
    assert!(first.has_more);
    assert_eq!(first.total, Some(PAGE_SIZE as u64 + 3));
    query.page = 1;
    let second = search_page(&many, &query);
    assert_eq!(second.items.len(), 3);
    assert!(!second.has_more);
}
