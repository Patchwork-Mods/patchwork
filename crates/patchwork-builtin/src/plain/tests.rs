use std::collections::BTreeMap;

use patchwork_core::config::{ConfigContext, ConfigFormat, Contents};
use patchwork_core::deploy::Dest;
use patchwork_core::options::{Answer, Field, FieldKind, Value};

use super::*;

fn read(format: &dyn ConfigFormat, file: &str) -> Contents {
    let path: Dest = "config/example".parse().unwrap();
    let config = toml::Table::new();
    format
        .read(&ConfigContext::new(&path, &config), file.as_bytes())
        .unwrap()
}

fn write(format: &dyn ConfigFormat, file: &str, values: &[Answer]) -> String {
    let path: Dest = "config/example".parse().unwrap();
    let config = toml::Table::new();
    let bytes = format
        .write(&ConfigContext::new(&path, &config), file.as_bytes(), values)
        .unwrap();
    String::from_utf8(bytes).unwrap()
}

fn set(group: &str, values: &[(&str, Value)]) -> Answer {
    let values: BTreeMap<String, Value> = values
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    Answer::values(group, values)
}

fn field<'a>(contents: &'a Contents, group: &str, id: &str) -> &'a Field {
    contents
        .groups
        .iter()
        .find(|g| g.id == group)
        .unwrap_or_else(|| panic!("no group {group:?}"))
        .fields()
        .iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("no field {id:?}"))
}

fn ids(contents: &Contents) -> Vec<&str> {
    contents.groups.iter().map(|g| g.id.as_str()).collect()
}

// INI

const INI: &str = "; po3's Tweaks
; made by someone

top = 5

[Fixes]
; Fixes the thing.
; Default: true
bFixThing = 1
iCount = 3
fScale = 1
sName = 42
Ratio = 0.5 ; inline comment
Mode = Fast
Quoted = \"a ; b\"
Loud = TRUE

[Fixes]
Later = yes
";

#[test]
fn ini_settings_are_guessed() {
    let contents = read(&IniConfig, INI);
    assert_eq!(ids(&contents), ["", "Fixes"]);
    assert_eq!(contents.groups[0].title, "General");
    assert_eq!(field(&contents, "", "top").value, Value::Int(5));
    // The file's header isn't the first setting's description.
    assert_eq!(field(&contents, "", "top").description, None);

    let fix = field(&contents, "Fixes", "bFixThing");
    assert!(matches!(fix.kind, FieldKind::Toggle));
    assert_eq!(fix.value, Value::Bool(true));
    assert_eq!(fix.default, Some(Value::Bool(true)));
    assert_eq!(fix.description.as_deref(), Some("Fixes the thing."));

    assert_eq!(field(&contents, "Fixes", "iCount").value, Value::Int(3));
    let scale = field(&contents, "Fixes", "fScale");
    assert!(matches!(scale.kind, FieldKind::Float { .. }));
    assert_eq!(scale.value, Value::Float(1.0));
    assert_eq!(
        field(&contents, "Fixes", "sName").value,
        Value::Text("42".into())
    );
    assert_eq!(field(&contents, "Fixes", "Ratio").value, Value::Float(0.5));
    assert_eq!(
        field(&contents, "Fixes", "Quoted").value,
        Value::Text("a ; b".into())
    );
    assert_eq!(field(&contents, "Fixes", "Loud").value, Value::Bool(true));
    // A section that's in the file twice is one group.
    assert_eq!(
        field(&contents, "Fixes", "Later").value,
        Value::Text("yes".into())
    );
}

#[test]
fn ini_writes_keep_the_file_as_it_was() {
    let out = write(
        &IniConfig,
        INI,
        &[
            set(
                "fixes",
                &[
                    ("bFixThing", Value::Bool(false)),
                    ("fScale", Value::Float(2.0)),
                    ("Ratio", Value::Float(0.25)),
                    ("Quoted", Value::Text("c".into())),
                    ("Loud", Value::Bool(false)),
                    ("Later", Value::Text("no".into())),
                    ("New", Value::Int(1)),
                ],
            ),
            set("", &[("added", Value::Bool(true))]),
            set("Other", &[("x", Value::Int(2))]),
        ],
    );
    assert_eq!(
        out,
        "; po3's Tweaks
; made by someone

top = 5
added = true

[Fixes]
; Fixes the thing.
; Default: true
bFixThing = 0
iCount = 3
fScale = 2.0
sName = 42
Ratio = 0.25 ; inline comment
Mode = Fast
Quoted = \"c\"
Loud = FALSE

[Fixes]
Later = no
New = 1

[Other]
x = 2
"
    );
    // What's written reads back as what was set.
    let contents = read(&IniConfig, &out);
    assert_eq!(field(&contents, "Fixes", "fScale").value, Value::Float(2.0));
    assert_eq!(
        field(&contents, "Fixes", "bFixThing").value,
        Value::Bool(false)
    );
}

#[test]
fn ini_keeps_crlf_and_bom() {
    let file = "\u{feff}[A]\r\nx=1\r\n";
    let out = write(
        &IniConfig,
        file,
        &[set("A", &[("x", Value::Int(2)), ("y", Value::Int(3))])],
    );
    assert_eq!(out, "\u{feff}[A]\r\nx=2\r\ny=3\r\n");
}

// JSON

const JSON: &str = r#"{
  // Turns it on.
  "enabled": true,
  "count": 3,
  "scale": 1.5,
  "name": "Bob \"B\" é",
  "list": [1, 2, {"a": 1}],
  "nothing": null,
  "graphics": {
    /* How far.
     * Range: 1 ~ 32 */
    "distance": 8,
    "deeper": { "x": 1 },
  },
}
"#;

#[test]
fn json_settings_are_read() {
    let contents = read(&JsonConfig, JSON);
    assert_eq!(ids(&contents), ["", "graphics", "graphics.deeper"]);
    let enabled = field(&contents, "", "enabled");
    assert_eq!(enabled.value, Value::Bool(true));
    assert_eq!(enabled.description.as_deref(), Some("Turns it on."));
    assert_eq!(field(&contents, "", "count").value, Value::Int(3));
    assert_eq!(field(&contents, "", "scale").value, Value::Float(1.5));
    assert_eq!(
        field(&contents, "", "name").value,
        Value::Text("Bob \"B\" é".into())
    );
    let distance = field(&contents, "graphics", "distance");
    assert!(matches!(
        distance.kind,
        FieldKind::Int {
            min: Some(1),
            max: Some(32),
            ..
        }
    ));
    assert_eq!(distance.description.as_deref(), Some("How far."));
    assert_eq!(contents.warnings.len(), 1, "{:?}", contents.warnings);
}

#[test]
fn json_writes_only_change_values() {
    let out = write(
        &JsonConfig,
        JSON,
        &[
            set(
                "",
                &[
                    ("enabled", Value::Bool(false)),
                    ("name", Value::Text("a\nb".into())),
                    ("added", Value::Int(7)),
                ],
            ),
            set("graphics.deeper", &[("x", Value::Float(2.0))]),
            set("graphics.new.inner", &[("y", Value::Bool(true))]),
        ],
    );
    assert_eq!(
        out,
        r#"{
  // Turns it on.
  "enabled": false,
  "count": 3,
  "scale": 1.5,
  "name": "a\nb",
  "list": [1, 2, {"a": 1}],
  "nothing": null,
  "graphics": {
    /* How far.
     * Range: 1 ~ 32 */
    "distance": 8,
    "deeper": { "x": 2.0 },
    "new": {"inner": {"y": true}},
  },
  "added": 7,
}
"#
    );
    let contents = read(&JsonConfig, &out);
    assert_eq!(
        field(&contents, "graphics.new.inner", "y").value,
        Value::Bool(true)
    );
}

#[test]
fn json_new_members_follow_the_layout() {
    let out = write(
        &JsonConfig,
        "{\"a\": 1}",
        &[set("", &[("b", Value::Int(2))])],
    );
    assert_eq!(out, "{\"a\": 1, \"b\": 2}");
    let out = write(&JsonConfig, "{}", &[set("", &[("b", Value::Int(2))])]);
    assert_eq!(out, "{\"b\": 2}");
    let out = write(
        &JsonConfig,
        "{\r\n\t\"a\": 1\r\n}",
        &[set("", &[("b", Value::Int(2))])],
    );
    assert_eq!(out, "{\r\n\t\"a\": 1,\r\n\t\"b\": 2\r\n}");
}

#[test]
fn bad_json_says_where() {
    let path: Dest = "config/example".parse().unwrap();
    let config = toml::Table::new();
    let err = JsonConfig
        .read(&ConfigContext::new(&path, &config), b"{\n\"a\": tru\n}")
        .unwrap_err();
    assert!(err.to_string().contains("line 2"), "{err}");
    assert!(
        JsonConfig
            .read(&ConfigContext::new(&path, &config), b"[1, 2]")
            .is_err()
    );
}

// TOML

/// Like Forge writes them.
const FORGE: &str = "
#Client settings
[client]
\t#How far to look.
\t#Range: 1 ~ 32
\tdistance = 8
\t#Allowed Values: NEAR, FAR
\tmode = \"FAR\"
\t#Range: 0.0 ~ 1.0
\topacity = 0.5
\tshow = true # a comment after
\tcolors = [\"red\"]

\t[client.sub]
\t\t#Default: 3
\t\tlevel = 2
";

#[test]
fn toml_settings_are_read_with_forge_hints() {
    let contents = read(&TomlConfig, FORGE);
    assert_eq!(ids(&contents), ["client", "client.sub"]);
    let distance = field(&contents, "client", "distance");
    assert!(matches!(
        distance.kind,
        FieldKind::Int {
            min: Some(1),
            max: Some(32),
            ..
        }
    ));
    assert_eq!(distance.description.as_deref(), Some("How far to look."));
    let mode = field(&contents, "client", "mode");
    assert!(matches!(&mode.kind, FieldKind::Choice { choices } if choices.len() == 2));
    let opacity = field(&contents, "client", "opacity");
    assert!(matches!(
        opacity.kind,
        FieldKind::Float {
            min: Some(0.0),
            max: Some(1.0),
            ..
        }
    ));
    assert_eq!(field(&contents, "client", "show").value, Value::Bool(true));
    assert_eq!(
        field(&contents, "client.sub", "level").default,
        Some(Value::Int(3))
    );
    assert_eq!(contents.warnings.len(), 1, "{:?}", contents.warnings);
}

#[test]
fn toml_writes_keep_comments_and_layout() {
    let out = write(
        &TomlConfig,
        FORGE,
        &[
            set(
                "client",
                &[
                    ("distance", Value::Int(16)),
                    ("show", Value::Bool(false)),
                    ("mode", Value::Text("NEAR".into())),
                ],
            ),
            set("client.sub", &[("level", Value::Int(5))]),
            set("server", &[("port", Value::Int(1))]),
        ],
    );
    assert!(
        out.contains("\t#How far to look.\n\t#Range: 1 ~ 32\n\tdistance = 16\n"),
        "{out}"
    );
    assert!(out.contains("\tshow = false # a comment after\n"), "{out}");
    assert!(out.contains("\tmode = \"NEAR\"\n"), "{out}");
    assert!(out.contains("\t\tlevel = 5\n"), "{out}");
    let contents = read(&TomlConfig, &out);
    assert_eq!(field(&contents, "server", "port").value, Value::Int(1));
}

#[test]
fn hints_that_dont_fit_the_value_are_dropped() {
    let contents = read(&TomlConfig, "#Range: 1 ~ 5\nx = 9\n");
    assert!(matches!(
        field(&contents, "", "x").kind,
        FieldKind::Int {
            min: None,
            max: None,
            ..
        }
    ));
}
