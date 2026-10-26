use super::*;

fn route(route: &str, tracking: &str, exts: &[&str], default: bool) -> serde_json::Value {
    serde_json::json!({
        "route": route,
        "defaultFileExtensions": exts,
        "trackingMethod": tracking,
        "subRoutes": [],
        "isDefaultLocation": default,
    })
}

/// Routes like the MelonLoader games' that have an in-game mod manager.
fn mod_manager_routes() -> Vec<serde_json::Value> {
    let mut loader = route("MelonLoader", "state", &[], false);
    loader["subRoutes"] = serde_json::json!([
        route("Managed", "state", &[".managed.dll"], false),
        route("Libs", "state", &[], false),
    ]);
    vec![
        route("Mods", "state", &[".dll"], false),
        route("UserData/ModManager", "subdir-no-flatten", &[], true),
        route("UserLibs", "state", &[".lib.dll"], false),
        loader,
    ]
}

fn melon(name: &str, loader: &str, routes: Vec<serde_json::Value>) -> serde_json::Value {
    let mut game = r2(name, "game", routes);
    game["packageLoader"] = loader.into();
    game["relativeFileExclusions"] =
        serde_json::json!(["manifest.json", "icon.png", "README.md", "LICENCE"]);
    game
}

fn standard_routes() -> Vec<serde_json::Value> {
    vec![
        route("BepInEx/plugins", "subdir", &[".dll"], true),
        route("BepInEx/core", "subdir", &[], false),
        route("BepInEx/patchers", "subdir", &[], false),
        route("BepInEx/monomod", "subdir", &[".mm.dll"], false),
        route("BepInEx/config", "none", &[], false),
    ]
}

fn r2(name: &str, kind: &str, routes: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "meta": { "displayName": name },
        "distributions": [
            { "platform": "steam", "identifier": "632360" },
            { "platform": "epic-games-store", "identifier": "abc" },
        ],
        "steamFolderName": name,
        "exeNames": [format!("{name}.exe")],
        "gameInstanceType": kind,
        "packageLoader": "bepinex",
        "installRules": routes,
        "relativeFileExclusions": null,
    })
}

fn schema() -> Schema {
    let mut sideloader = standard_routes();
    sideloader.push(route(
        "BepInEx/Sideloader",
        "state",
        &[".hotmod", ".h3mod"],
        false,
    ));
    serde_json::from_value(serde_json::json!({
        "games": {
            "riskofrain2": { "r2modman": [
                r2("Risk of Rain 2", "game", standard_routes()),
                r2("Risk of Rain 2 Dedicated Server", "server", standard_routes()),
            ]},
            "outward": { "r2modman": [
                r2("Outward", "game", standard_routes()),
                r2("Outward Definitive", "game", standard_routes()),
            ]},
            "h3vr": { "r2modman": [r2("H3VR", "game", sideloader)] },
            "backpack-hero": { "r2modman": [melon("Backpack Hero", "melonloader", mod_manager_routes())] },
            "schedule-i": { "r2modman": [melon("Schedule I", "recursive-melonloader", Vec::new())] },
            "unsupported": { "r2modman": [{
                "meta": { "displayName": "Other" },
                "distributions": [],
                "gameInstanceType": "game",
                "packageLoader": "northstar",
                "installRules": [],
            }]},
            "no-r2modman": { "r2modman": null },
        },
        "modloaderPackages": [
            { "packageId": "bbepis-BepInExPack", "rootFolder": "BepInExPack", "loader": "bepinex" },
            { "packageId": "denikson-BepInExPack_Valheim", "rootFolder": "BepInExPack_Valheim", "loader": "bepinex" },
            { "packageId": "1F31A-BepInEx_Valheim_Full", "rootFolder": "BepInEx_Valheim_Full", "loader": "bepinex" },
            { "packageId": "bbepis-BepInEx_Rogue_Tower", "rootFolder": "", "loader": "bepinex" },
            { "packageId": "LavaGang-MelonLoader", "rootFolder": "", "loader": "melonloader" },
            { "packageId": "LavaGang-MelonLoader", "rootFolder": "", "loader": "recursive-melonloader" },
            { "packageId": "SHMCD_Modding-MelonLoader_SHMCD", "rootFolder": "", "loader": "recursive-melonloader" },
        ],
    }))
    .unwrap()
}

fn communities() -> BTreeMap<String, Community> {
    let ror2 = Community {
        identifier: "riskofrain2".to_owned(),
        short_description: Some("Escape a chaotic alien planet.".to_owned()),
        cover_image_url: Some("https://example.com/ror2.webp".to_owned()),
    };
    BTreeMap::from([(ror2.identifier.clone(), ror2)])
}

fn games() -> BTreeMap<String, GameDef> {
    generate_games(&schema(), &communities())
        .unwrap()
        .into_iter()
        .map(|(id, text)| {
            assert!(text.starts_with(HEADER));
            (id, toml::from_str(&text).unwrap())
        })
        .collect()
}

#[test]
fn ids_are_stable_and_variants_get_their_own() {
    let ids: Vec<_> = games().into_keys().collect();
    assert_eq!(
        ids,
        [
            "backpack-hero",
            "h3vr",
            "outward",
            "outward-definitive",
            "risk-of-rain-2",
            "risk-of-rain-2-server",
            "schedule-i",
        ]
    );
}

#[test]
fn games_carry_their_listing_and_detection_hints() {
    let games = games();
    let ror2 = &games["risk-of-rain-2"];
    assert_eq!(ror2.name, "Risk of Rain 2");
    assert_eq!(
        ror2.description.as_deref(),
        Some("Escape a chaotic alien planet.")
    );
    assert_eq!(ror2.image.as_deref(), Some("https://example.com/ror2.webp"));
    assert_eq!(
        ror2.sources["thunderstore"]["community"].as_str(),
        Some("riskofrain2")
    );
    assert_eq!(ror2.detect.steam, [632360]);
    assert_eq!(ror2.detect.exe, ["Risk of Rain 2.exe"]);
    assert_eq!(
        ror2.deployer.config["rulesets"][0].as_str(),
        Some("bepinex")
    );
    // Servers share the game's community.
    let server = &games["risk-of-rain-2-server"];
    assert_eq!(server.sources, ror2.sources);
    assert!(games["outward"].description.is_none());
}

#[test]
fn unusual_games_get_their_own_rules() {
    let games = games();
    let h3vr = &games["h3vr"].deployer.config;
    assert!(h3vr.get("rulesets").is_none());
    let rules: Vec<Rule> = h3vr["rules"].clone().try_into().unwrap();
    assert_eq!(rules[0].ruleset.as_deref(), Some("bepinex-loader"));
    let sideloader = Rule {
        folder: Some("Sideloader".to_owned()),
        to: Some("BepInEx/Sideloader/{path}".to_owned()),
        ..Rule::default()
    };
    assert!(rules.contains(&sideloader), "{rules:#?}");
    // Extension rules go after every folder rule, longest extension first,
    // and before the catch-all.
    let files: Vec<_> = rules.iter().filter_map(|r| r.files.as_deref()).collect();
    assert_eq!(files, ["**/*.mm.dll", "**/*.hotmod", "**/*.h3mod", "**"]);
}

/// The generator's rules for the usual routes are exactly the hand-written
/// `bepinex` ruleset, so the many games that just use it behave the same as
/// they would with their own generated rules.
#[test]
fn standard_rules_match_the_bepinex_ruleset() {
    #[derive(Deserialize)]
    struct Ruleset {
        rules: Vec<Rule>,
    }
    let text = include_str!("../../../registry/rulesets/bepinex.toml");
    let ruleset: Ruleset = toml::from_str(text).unwrap();
    let mut expected = ruleset.rules;
    assert_eq!(
        expected.remove(0).ruleset.as_deref(),
        Some("bepinex-loader")
    );

    let game: R2Game = serde_json::from_value(r2("Standard", "game", standard_routes())).unwrap();
    assert!(is_standard(&game));
    assert_eq!(mod_rules(&game).unwrap(), expected);
}

#[test]
fn loader_ruleset_covers_every_pack() {
    #[derive(Deserialize)]
    struct Ruleset {
        rules: Vec<toml::Table>,
    }
    let text = generate_loader_ruleset(&schema()).unwrap();
    assert!(text.starts_with(HEADER));
    let rules = toml::from_str::<Ruleset>(&text).unwrap().rules;
    assert_eq!(
        rules[0]["files"].as_str(),
        Some("{BepInExPack*,BepInEx_Valheim_Full}/**")
    );
    assert_eq!(
        rules[1]["mods"].as_str(),
        Some("bbepis-BepInEx_Rogue_Tower")
    );
    let skipped: Vec<_> = rules[3]["mods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    assert_eq!(
        skipped,
        [
            "1F31A-BepInEx_Valheim_Full",
            "bbepis-BepInEx_Rogue_Tower",
            "bbepis-BepInExPack",
            "denikson-BepInExPack_Valheim",
        ]
    );
}

#[test]
fn melonloader_games() {
    let games = games();
    let schedule = &games["schedule-i"].deployer.config;
    assert_eq!(
        schedule["rulesets"].as_array().unwrap(),
        &[toml::Value::from("recursive-melonloader")]
    );
    assert!(schedule.get("rules").is_none());

    let rules: Vec<Rule> = games["backpack-hero"].deployer.config["rules"]
        .clone()
        .try_into()
        .unwrap();
    let rule = |folder: Option<&str>, files: Option<&str>, to: &str| Rule {
        folder: folder.map(str::to_owned),
        files: files.map(str::to_owned),
        to: Some(to.to_owned()),
        ..Rule::default()
    };
    assert_eq!(
        rules,
        [
            Rule {
                ruleset: Some("melonloader-loader".to_owned()),
                ..Rule::default()
            },
            // The package's metadata isn't left out: it goes to the default
            // route, which isn't tracked by `state`.
            rule(Some("Mods"), None, "Mods/{path}"),
            rule(Some("ModManager"), None, "UserData/ModManager/{mod}/{path}"),
            rule(Some("UserLibs"), None, "UserLibs/{path}"),
            rule(Some("MelonLoader"), None, "MelonLoader/{path}"),
            // Sub-routes are under their parent.
            rule(Some("Managed"), None, "MelonLoader/Managed/{path}"),
            rule(Some("Libs"), None, "MelonLoader/Libs/{path}"),
            rule(None, Some("**/*.managed.dll"), "MelonLoader/Managed/{file}"),
            rule(None, Some("**/*.lib.dll"), "UserLibs/{file}"),
            rule(None, Some("**/*.dll"), "Mods/{file}"),
            // Not flattened.
            rule(None, Some("**"), "UserData/ModManager/{mod}/{path}"),
        ]
    );
}

#[test]
fn exclusions_only_apply_to_state_routes() {
    let mut routes = mod_manager_routes();
    routes[0]["isDefaultLocation"] = true.into();
    routes[1]["isDefaultLocation"] = false.into();
    routes.push(route("UserData/Skins", "state", &[".png"], false));
    let game: R2Game = serde_json::from_value(melon("Game", "melonloader", routes)).unwrap();
    let rules = mod_rules(&game).unwrap();
    assert_eq!(
        rules[0],
        Rule {
            files: Some("{manifest.json,icon.png,README.md,LICENCE}".to_owned()),
            skip: true,
            ..Rule::default()
        }
    );
}

#[test]
fn melonloader_rulesets_list_their_packs() {
    #[derive(Deserialize)]
    struct Ruleset {
        rules: Vec<toml::Table>,
    }
    let rulesets: BTreeMap<_, _> = generate_loader_rulesets(&schema())
        .unwrap()
        .into_iter()
        .collect();
    let rules = |id: &str| toml::from_str::<Ruleset>(&rulesets[id]).unwrap().rules;

    let old = rules("melonloader-loader");
    assert_eq!(old[0]["mods"].as_str(), Some("LavaGang-MelonLoader"));
    assert_eq!(old[0]["skip"].as_bool(), Some(true));
    assert_eq!(old[1]["files"].as_str(), Some("**"));

    let recursive = rules("recursive-melonloader-loader");
    let mods: Vec<_> = recursive[0]["mods"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m.as_str().unwrap())
        .collect();
    assert_eq!(
        mods,
        ["LavaGang-MelonLoader", "SHMCD_Modding-MelonLoader_SHMCD"]
    );
    assert!(recursive[0].get("skip").is_none());
    assert_eq!(recursive[1]["skip"].as_bool(), Some(true));
}

#[test]
fn unsupported_tracking_is_an_error() {
    let mut routes = standard_routes();
    routes.push(route("BepInEx/Zips", "package-zip", &[], false));
    let game: R2Game = serde_json::from_value(r2("Zippy", "game", routes)).unwrap();
    assert!(mod_rules(&game).is_err());
}

#[test]
fn ambiguous_folders_are_an_error() {
    let mut routes = standard_routes();
    routes.push(route("Other/plugins", "state", &[], false));
    let game: R2Game = serde_json::from_value(r2("Twice", "game", routes)).unwrap();
    assert!(mod_rules(&game).is_err());
}

#[test]
fn slugs() {
    assert_eq!(slug("Outward Definitive"), "outward-definitive");
    assert_eq!(slug("  ULTRAKILL: Revamped! "), "ultrakill-revamped");
}
