use std::collections::BTreeMap;

use patchwork_core::deploy::Dest;

use super::*;

/// Like BepInEx 5 writes them, on Windows.
const FILE: &str = "## Settings file was created by plugin Better Jumps v1.2.0\r
## Plugin GUID: com.example.betterjumps\r
\r
[General]\r
\r
## Whether jumps are better.\r
## Much better.\r
# Setting type: Boolean\r
# Default value: true\r
Enabled = true\r
\r
## How many jumps in the air.\r
# Setting type: Int32\r
# Default value: 2\r
# Acceptable value range: From 1 to 5\r
Air Jumps = 3\r
\r
# Setting type: Single\r
# Default value: 1.5\r
# Acceptable value range: From 0.5 to 4\r
Height = 1.5\r
\r
[Looks]\r
\r
# Setting type: Color\r
# Default value: FF0000FF\r
Trail = 00FF0080\r
\r
# Setting type: Mode\r
# Default value: Fast\r
# Acceptable values: Fast, Slow, Off\r
Mode = Slow\r
\r
# Setting type: Layers\r
# Default value: A\r
# Acceptable values: A, B, C\r
# Multiple values can be set at the same time by separating them with , (e.g. Debug, Warning)\r
Layers = A, C\r
\r
# Setting type: String\r
# Default value: \r
Greeting = Hello\\nthere \\\"you\\\"\r
\r
# Setting type: String\r
# Default value: \r
Folder = C:\\Games\\Mods\r
\r
# Setting type: Int32\r
# Default value: 1\r
Broken = lots\r
";

fn ctx_read(file: &str) -> Contents {
    let path: Dest = "BepInEx/config/com.example.betterjumps.cfg"
        .parse()
        .unwrap();
    let config = toml::Table::new();
    BepInExConfig
        .read(&ConfigContext::new(&path, &config), file.as_bytes())
        .unwrap()
}

fn ctx_write(file: &str, values: &[Answer]) -> String {
    let path: Dest = "BepInEx/config/com.example.betterjumps.cfg"
        .parse()
        .unwrap();
    let config = toml::Table::new();
    let bytes = BepInExConfig
        .write(&ConfigContext::new(&path, &config), file.as_bytes(), values)
        .unwrap();
    String::from_utf8(bytes).unwrap()
}

fn field<'a>(contents: &'a Contents, group: &str, id: &str) -> &'a Field {
    contents
        .groups
        .iter()
        .find(|g| g.id == group)
        .unwrap()
        .fields()
        .iter()
        .find(|f| f.id == id)
        .unwrap()
}

#[test]
fn settings_are_read_with_their_types() {
    let contents = ctx_read(FILE);
    assert_eq!(contents.title.as_deref(), Some("Better Jumps"));
    let ids: Vec<&str> = contents.groups.iter().map(|g| g.id.as_str()).collect();
    assert_eq!(ids, ["General", "Looks"]);

    let enabled = field(&contents, "General", "Enabled");
    assert!(matches!(enabled.kind, FieldKind::Toggle));
    assert_eq!(enabled.value, Value::Bool(true));
    assert_eq!(enabled.default, Some(Value::Bool(true)));
    assert_eq!(
        enabled.description.as_deref(),
        Some("Whether jumps are better.\nMuch better.")
    );

    let jumps = field(&contents, "General", "Air Jumps");
    assert!(matches!(
        jumps.kind,
        FieldKind::Int {
            min: Some(1),
            max: Some(5),
            ..
        }
    ));
    assert_eq!(jumps.value, Value::Int(3));
    let height = field(&contents, "General", "Height");
    assert!(matches!(
        height.kind,
        FieldKind::Float {
            min: Some(0.5),
            max: Some(4.0),
            ..
        }
    ));
    assert_eq!(height.value, Value::Float(1.5));

    let trail = field(&contents, "Looks", "Trail");
    assert!(matches!(trail.kind, FieldKind::Color { alpha: true }));
    assert_eq!(trail.value, Value::Text("#00ff0080".to_owned()));
    assert_eq!(trail.default, Some(Value::Text("#ff0000ff".to_owned())));

    let mode = field(&contents, "Looks", "Mode");
    let FieldKind::Choice { choices } = &mode.kind else {
        panic!("{mode:?}");
    };
    assert_eq!(choices.len(), 3);
    assert_eq!(mode.value, Value::Text("Slow".to_owned()));
    // flags are a list, which is text
    let layers = field(&contents, "Looks", "Layers");
    assert!(matches!(layers.kind, FieldKind::Text { .. }));

    let greeting = field(&contents, "Looks", "Greeting");
    assert_eq!(
        greeting.value,
        Value::Text("Hello\nthere \"you\"".to_owned())
    );
    assert!(matches!(greeting.kind, FieldKind::Text { multiline: true }));
    // Windows paths that were never escaped are left as they are
    let folder = field(&contents, "Looks", "Folder");
    assert_eq!(folder.value, Value::Text("C:\\Games\\Mods".to_owned()));
    // a value that doesn't fit its type is text
    let broken = field(&contents, "Looks", "Broken");
    assert_eq!(broken.value, Value::Text("lots".to_owned()));
    assert!(matches!(broken.kind, FieldKind::Text { .. }));

    // every value it reads fits its field
    for group in &contents.groups {
        for f in group.fields() {
            assert_eq!(f.check(&f.value).as_ref(), Ok(&f.value), "{f:?}");
        }
    }
}

#[test]
fn writing_changes_only_the_settings_given() {
    let values = [
        Answer::values(
            "General",
            BTreeMap::from([
                ("Enabled".to_owned(), Value::Bool(false)),
                ("Height".to_owned(), Value::Float(2.25)),
            ]),
        ),
        Answer::values(
            "Looks",
            BTreeMap::from([
                ("Trail".to_owned(), Value::Text("#0000ffff".to_owned())),
                ("Greeting".to_owned(), Value::Text("a\\b\nc".to_owned())),
            ]),
        ),
    ];
    let written = ctx_write(FILE, &values);
    let expected = FILE
        .replace("Enabled = true", "Enabled = false")
        .replace("Height = 1.5", "Height = 2.25")
        .replace("Trail = 00FF0080", "Trail = 0000FFFF")
        .replace(
            "Greeting = Hello\\nthere \\\"you\\\"",
            "Greeting = a\\\\b\\nc",
        );
    assert_eq!(written, expected);

    // and reading it back gets what was written
    let contents = ctx_read(&written);
    assert_eq!(
        field(&contents, "Looks", "Greeting").value,
        Value::Text("a\\b\nc".to_owned())
    );
    assert_eq!(
        field(&contents, "Looks", "Trail").value,
        Value::Text("#0000ffff".to_owned())
    );
}

#[test]
fn settings_the_file_hasnt_got_are_added() {
    let file = "[A]\n\nOne = 1\n\n[B]\nTwo = 2\n";
    let values = [
        Answer::values(
            "A",
            BTreeMap::from([
                ("New".to_owned(), Value::Int(5)),
                ("Other".to_owned(), Value::Bool(true)),
            ]),
        ),
        Answer::values("C", BTreeMap::from([("Three".to_owned(), Value::Int(3))])),
    ];
    assert_eq!(
        ctx_write(file, &values),
        "[A]\n\nOne = 1\nNew = 5\nOther = true\n\n[B]\nTwo = 2\n\n[C]\n\nThree = 3\n"
    );
}
