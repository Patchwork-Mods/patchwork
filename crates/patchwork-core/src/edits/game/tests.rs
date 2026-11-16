use super::*;

fn file(text: &str) -> (tempfile::TempDir, Paths, UserGameFile) {
    let tmp = tempfile::tempdir().unwrap();
    let paths = Paths::rooted(tmp.path());
    let games = paths.user_registry().join("games");
    fs::create_dir_all(&games).unwrap();
    fs::write(games.join("g.toml"), text).unwrap();
    let file = UserGameFile::load(&paths, "g").unwrap();
    (tmp, paths, file)
}

#[test]
fn changes_keep_what_else_the_file_has() {
    let (_tmp, paths, mut file) =
        file("name = \"Old\"\n\n# mine\n[sources.thunderstore]\ncommunity = \"g\"\n");
    let name = game_field("name").unwrap();
    let color = game_field("color").unwrap();
    file.set(name, None);
    file.set(color, Some(FieldValue::Text("#abc".into())));
    file.save().unwrap();

    let text = fs::read_to_string(paths.user_registry().join("games/g.toml")).unwrap();
    assert!(text.contains("# mine"), "{text}");
    assert!(!text.contains("Old"), "{text}");
    assert!(text.contains("community = \"g\""), "{text}");
    let table = UserGameFile::load(&paths, "g").unwrap().table().unwrap();
    assert_eq!(table["accent"].as_str(), Some("#abc"));
    // the key goes at the top, not into the last table
    assert!(table["sources"]["thunderstore"].get("accent").is_none());
}

#[test]
fn a_file_left_empty_is_removed() {
    let (_tmp, paths, mut file) = file("name = \"Old\"\n");
    file.set(game_field("name").unwrap(), None);
    file.save().unwrap();
    assert!(!paths.user_registry().join("games/g.toml").exists());
    // and loading it again is fine
    assert!(
        UserGameFile::load(&paths, "g")
            .unwrap()
            .table()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn fields_tell_the_definition_from_the_users_changes() {
    let (_tmp, paths, file) = file("image = \"art/g.png\"\naccent = \"nope\"\n");
    let before: toml::Table = toml::from_str("name = \"G\"\naccent = \"#123456\"").unwrap();
    let fields = file.fields(Some(&before));
    let by_id = |id: &str| fields.iter().find(|f| f.id == id).unwrap();

    assert_eq!(by_id("name").source, Some(FieldValue::Text("G".into())));
    assert_eq!(by_id("name").edited, None);
    // relative to the file, as the registry reads it
    let art = paths.user_registry().join("games/art/g.png");
    assert_eq!(
        by_id("picture").edited,
        Some(FieldValue::Text(art.to_string_lossy().into_owned()))
    );
    // one it can't take is left out
    assert_eq!(by_id("color").edited, None);
    assert_eq!(
        by_id("color").source,
        Some(FieldValue::Text("#123456".into()))
    );
}

#[test]
fn colors_are_checked() {
    let color = game_field("color").unwrap();
    assert_eq!(
        color
            .kind
            .check(color.id, FieldValue::Text(" #ABCDEF ".into()))
            .unwrap(),
        Some(FieldValue::Text("#abcdef".into()))
    );
    assert_eq!(
        color
            .kind
            .check(color.id, FieldValue::Text("".into()))
            .unwrap(),
        None
    );
    assert!(
        color
            .kind
            .check(color.id, FieldValue::Text("red".into()))
            .is_err()
    );
}
