use super::*;
use crate::options::{Field, FieldKind, GroupKind};

fn def(files: &[&str]) -> ConfigDef {
    ConfigDef {
        files: files
            .iter()
            .map(|f| (*f).to_owned().try_into().unwrap())
            .collect(),
        format: "test.kv".to_owned(),
        root: RootId::game(),
    }
}

fn dest(s: &str) -> Dest {
    s.parse().unwrap()
}

impl ConfigDef {
    fn with_format(mut self, format: &str) -> Self {
        self.format = format.to_owned();
        self
    }
}

#[test]
fn defs_take_one_pattern_or_many() {
    let one: ConfigDef = toml::from_str("files = \"a/*.cfg\"\nformat = \"x.y\"").unwrap();
    assert_eq!(one, def(&["a/*.cfg"]).with_format("x.y"));
    let many: ConfigDef =
        toml::from_str("files = [\"a/*.cfg\", \"b.ini\"]\nformat = \"x.y\"\nroot = \"appdata\"")
            .unwrap();
    assert_eq!(many.files.len(), 2);
    assert_eq!(many.root.as_str(), "appdata");
    assert!(toml::from_str::<ConfigDef>("files = \"a\"\nformat = \"x\"\nnope = 1").is_err());
}

#[test]
fn files_are_matched_ignoring_case_and_in_their_root() {
    let defs = ConfigDefs::new(vec![
        def(&["BepInEx/config/**/*.cfg"]),
        def(&["**/*.cfg"]).with_format("other.kv"),
    ])
    .unwrap();
    let format = |path: &str| defs.find(&dest(path)).map(|d| d.format.as_str());
    assert_eq!(format("bepinex/CONFIG/a.cfg"), Some("test.kv"));
    assert_eq!(format("BepInEx/config/sub/a.cfg"), Some("test.kv"));
    // the first that matches says what reads it
    assert_eq!(format("elsewhere/a.cfg"), Some("other.kv"));
    assert_eq!(format("a.ini"), None);
    assert_eq!(format("appdata:a.cfg"), None);
}

#[test]
fn only_the_folders_patterns_could_match_in_are_looked_in() {
    let folders = |files: &[&str]| -> Vec<String> {
        ConfigDefs::new(vec![def(files)])
            .unwrap()
            .folders()
            .into_iter()
            .map(|(_, f)| f)
            .collect()
    };
    assert_eq!(folders(&["BepInEx/config/**/*.cfg"]), ["BepInEx/config"]);
    assert_eq!(folders(&["UserData/MelonPreferences.cfg"]), ["UserData"]);
    assert_eq!(folders(&["a/b/*.ini", "a/*.cfg"]), ["a"]);
    assert_eq!(folders(&["a/*.cfg", "a/b/*.ini"]), ["a"]);
    assert_eq!(folders(&["**/*.cfg", "a/*.ini"]), [""]);
}

fn contents() -> Contents {
    let field = |id: &str, kind: FieldKind, value: Value| Field {
        id: id.to_owned(),
        title: id.to_owned(),
        description: None,
        kind,
        value,
        default: None,
    };
    Contents {
        title: None,
        groups: vec![Group {
            id: "g".to_owned(),
            title: "G".to_owned(),
            kind: GroupKind::Fields {
                fields: vec![
                    field("on", FieldKind::Toggle, Value::Bool(false)),
                    field(
                        "speed",
                        FieldKind::Float {
                            min: Some(0.0),
                            max: None,
                            step: None,
                        },
                        Value::Float(1.5),
                    ),
                ],
            },
            defaulted: true,
        }],
        warnings: Vec::new(),
    }
}

#[test]
fn edits_are_checked_against_the_file() {
    let edits = Edits::from([
        (
            "g".to_owned(),
            BTreeMap::from([
                ("on".to_owned(), Value::Bool(true)),
                ("speed".to_owned(), Value::Int(2)),
                ("gone".to_owned(), Value::Int(2)),
            ]),
        ),
        (
            "h".to_owned(),
            BTreeMap::from([("x".to_owned(), Value::Int(1))]),
        ),
    ]);
    let (kept, problems) = checked_edits(&contents(), &edits);
    assert_eq!(
        kept["g"],
        BTreeMap::from([
            ("on".to_owned(), Value::Bool(true)),
            ("speed".to_owned(), Value::Float(2.0)),
        ])
    );
    assert_eq!(kept.len(), 1);
    assert_eq!(problems.len(), 2, "{problems:?}");

    let bad = Edits::from([(
        "g".to_owned(),
        BTreeMap::from([("speed".to_owned(), Value::Int(-1))]),
    )]);
    let (kept, problems) = checked_edits(&contents(), &bad);
    assert!(kept.is_empty());
    assert!(problems[0].contains("less than 0"), "{problems:?}");
}

#[test]
fn only_changed_settings_count_as_changed() {
    let before = contents().values();
    let mut now = before.clone();
    now.get_mut("g")
        .unwrap()
        .insert("on".to_owned(), Value::Bool(true));
    now.entry("new".to_owned())
        .or_default()
        .insert("x".to_owned(), Value::Int(1));
    let found = changed(&before, &now);
    assert_eq!(
        found["g"],
        BTreeMap::from([("on".to_owned(), Value::Bool(true))])
    );
    assert_eq!(found["new"]["x"], Value::Int(1));
    assert!(changed(&before, &before).is_empty());
}
