use serde::Deserialize;

use super::*;

/// Saves a list of names, at whatever version it's told to be.
struct Names(u32);

impl Extension for Names {
    fn id(&self) -> &'static str {
        "names"
    }

    fn state_version(&self) -> u32 {
        self.0
    }

    /// Version 1 kept them as one comma-separated string.
    fn migrate(&self, from: u32, mut state: toml::Table) -> Result<toml::Table> {
        assert_eq!(from, 1);
        let joined = state.remove("joined").unwrap();
        let names = joined
            .as_str()
            .unwrap()
            .split(',')
            .map(|n| toml::Value::String(n.to_owned()))
            .collect();
        state.insert("names".to_owned(), toml::Value::Array(names));
        Ok(state)
    }

    fn register(&self, _: &mut Registrar<'_>) {}
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Saved {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    names: Vec<String>,
}

fn saved(names: &[&str]) -> Saved {
    Saved {
        names: names.iter().map(|n| n.to_string()).collect(),
    }
}

#[test]
fn state_round_trips_with_its_version() {
    let mut tables = ExtTables::new();
    write_state(&Names(1), &mut tables, &saved(&["a", "b"]), &[]).unwrap();
    assert_eq!(tables["names"]["v"].as_integer(), Some(1));
    let back: Saved = read_state(&Names(1), &tables, &[]).unwrap();
    assert_eq!(back, saved(&["a", "b"]));
}

#[test]
fn missing_state_is_the_default() {
    let back: Saved = read_state(&Names(1), &ExtTables::new(), &[]).unwrap();
    assert_eq!(back, Saved::default());
}

#[test]
fn saving_the_default_leaves_no_table() {
    let mut tables = ExtTables::new();
    write_state(&Names(1), &mut tables, &saved(&["a"]), &[]).unwrap();
    write_state(&Names(1), &mut tables, &Saved::default(), &[]).unwrap();
    assert!(tables.is_empty());
}

#[test]
fn older_state_is_migrated() {
    let mut tables = ExtTables::new();
    tables.insert(
        "names".to_owned(),
        toml::from_str("v = 1\njoined = \"a,b\"").unwrap(),
    );
    let back: Saved = read_state(&Names(2), &tables, &[]).unwrap();
    assert_eq!(back, saved(&["a", "b"]));
}

#[test]
fn newer_state_is_neither_read_nor_overwritten() {
    let mut tables = ExtTables::new();
    write_state(&Names(2), &mut tables, &saved(&["a"]), &[]).unwrap();
    let read = read_state::<Saved>(&Names(1), &tables, &[]).unwrap_err();
    assert!(read.to_string().contains("newer patchwork"), "{read}");
    let written = write_state(&Names(1), &mut tables, &saved(&["b"]), &[]).unwrap_err();
    assert!(written.to_string().contains("newer patchwork"), "{written}");
    assert_eq!(tables["names"]["v"].as_integer(), Some(2));
}

#[test]
fn other_extensions_state_is_left_alone() {
    let mut tables = ExtTables::new();
    let other: toml::Table = toml::from_str("v = 7\nanything = true").unwrap();
    tables.insert("other".to_owned(), other.clone());
    write_state(&Names(1), &mut tables, &saved(&["a"]), &[]).unwrap();
    assert_eq!(tables["other"], other);
}

struct Nothing;

impl Deployer for Nothing {
    fn kind(&self) -> &str {
        "nothing"
    }

    fn plan(
        &self,
        _: &crate::deploy::DeployContext<'_>,
        _: &[crate::deploy::InstalledMod],
    ) -> Result<crate::deploy::DesiredState> {
        Ok(Default::default())
    }
}

struct Clashing(&'static str);

impl Extension for Clashing {
    fn id(&self) -> &'static str {
        self.0
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(Nothing);
    }
}

#[test]
fn clashes_and_bad_ids_are_problems() {
    let mut registered = Registered::default();
    registered.extension(Clashing("one"));
    registered.extension(Clashing("two"));
    registered.extension(Clashing("one"));
    registered.extension(Clashing("Bad Id"));
    assert_eq!(registered.problems.len(), 3, "{:?}", registered.problems);
    assert!(registered.problems[0].contains("`one` and extension `two`"));
    assert!(registered.problems[1].contains("registered twice"));
    assert!(registered.problems[2].contains("isn't a valid extension id"));
    assert_eq!(registered.extensions.len(), 2);
}

#[test]
fn keys_and_typed_state_share_a_table() {
    let mut tables = ExtTables::new();
    write_key(&Names(1), &mut tables, "order", &vec!["x", "y"]).unwrap();
    write_state(&Names(1), &mut tables, &saved(&["a"]), &["order"]).unwrap();
    let order: Vec<String> = read_key(&Names(1), &tables, "order").unwrap();
    assert_eq!(order, ["x", "y"]);
    let back: Saved = read_state(&Names(1), &tables, &["order"]).unwrap();
    assert_eq!(back, saved(&["a"]));

    // emptying both leaves nothing behind
    write_key(&Names(1), &mut tables, "order", &Vec::<String>::new()).unwrap();
    write_state(&Names(1), &mut tables, &Saved::default(), &["order"]).unwrap();
    assert!(tables.is_empty());
}

#[test]
fn writing_a_key_migrates_the_rest_first() {
    let mut tables = ExtTables::new();
    tables.insert(
        "names".to_owned(),
        toml::from_str("v = 1\njoined = \"a,b\"").unwrap(),
    );
    write_key(&Names(2), &mut tables, "order", &vec!["x"]).unwrap();
    assert_eq!(tables["names"]["v"].as_integer(), Some(2));
    let back: Saved = read_state(&Names(2), &tables, &["order"]).unwrap();
    assert_eq!(back, saved(&["a", "b"]));
}
