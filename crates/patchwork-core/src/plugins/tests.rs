use super::*;

fn plugin(name: &str, mod_name: &str) -> Plugin {
    Plugin {
        name: name.to_owned(),
        mod_name: mod_name.to_owned(),
        master: name.ends_with(".esm") || name.ends_with(".esl"),
        light: name.ends_with(".esl"),
        masters: Vec::new(),
    }
}

fn needing(name: &str, masters: &[&str]) -> Plugin {
    Plugin {
        masters: masters.iter().map(|m| m.to_string()).collect(),
        ..plugin(name, "m")
    }
}

fn found(plugins: Vec<Plugin>) -> Plugins {
    Plugins {
        found: plugins,
        implicit: vec!["Skyrim.esm".to_owned(), "Update.esm".to_owned()],
        light_implicit: Vec::new(),
        limit: None,
        warnings: Vec::new(),
    }
}

fn entry(name: &str, enabled: bool) -> PluginEntry {
    PluginEntry {
        name: name.to_owned(),
        enabled,
    }
}

fn names(order: &LoadOrder) -> Vec<&str> {
    order
        .plugins
        .iter()
        .map(|p| p.plugin.name.as_str())
        .collect()
}

#[test]
fn unsaved_plugins_follow_their_mods() {
    let order = LoadOrder::new(&[], found(vec![plugin("A.esp", "a"), plugin("B.esp", "b")]));
    assert_eq!(names(&order), ["A.esp", "B.esp"]);
    assert!(order.plugins.iter().all(|p| p.enabled));
}

#[test]
fn saved_order_wins_and_new_plugins_go_last() {
    let saved = [
        entry("b.ESP", false),
        entry("Gone.esp", true),
        entry("A.esp", true),
    ];
    let order = LoadOrder::new(
        &saved,
        found(vec![
            plugin("A.esp", "a"),
            plugin("B.esp", "b"),
            plugin("New.esp", "c"),
        ]),
    );
    assert_eq!(names(&order), ["B.esp", "A.esp", "New.esp"]);
    assert!(!order.plugins[0].enabled);
    assert_eq!(
        order.entries(),
        [
            entry("B.esp", false),
            entry("A.esp", true),
            entry("New.esp", true)
        ]
    );
}

#[test]
fn masters_go_first_and_implicit_plugins_are_left_out() {
    let saved = [entry("A.esp", true), entry("M.esm", true)];
    let order = LoadOrder::new(
        &saved,
        found(vec![
            plugin("A.esp", "a"),
            plugin("update.esm", "cleaned"),
            plugin("M.esm", "m"),
            plugin("L.esl", "l"),
        ]),
    );
    assert_eq!(names(&order), ["M.esm", "L.esl", "A.esp"]);
}

#[test]
fn the_later_mod_provides_a_plugin_two_have() {
    let order = LoadOrder::new(&[], found(vec![plugin("A.esp", "a"), plugin("a.esp", "b")]));
    assert_eq!(order.plugins.len(), 1);
    assert_eq!(order.plugins[0].plugin.mod_name, "b");
}

#[test]
fn missing_and_late_masters_are_warned_about() {
    let saved = [
        entry("Patch.esp", true),
        entry("Base.esp", true),
        entry("Off.esp", false),
    ];
    let order = LoadOrder::new(
        &saved,
        found(vec![
            needing("Base.esp", &["Skyrim.esm"]),
            needing(
                "Patch.esp",
                &["Skyrim.esm", "Base.esp", "Off.esp", "Nowhere.esp"],
            ),
            plugin("Off.esp", "o"),
        ]),
    );
    assert_eq!(order.warnings.len(), 3, "{:?}", order.warnings);
    assert!(order.warnings[0].contains("Base.esp, which loads after it"));
    assert!(order.warnings[1].contains("Off.esp, which is disabled"));
    assert!(order.warnings[2].contains("Nowhere.esp, which isn't in any of the mods"));
}

#[test]
fn too_many_full_plugins_are_warned_about() {
    let mut plugins = found(vec![
        plugin("A.esp", "a"),
        plugin("B.esp", "b"),
        plugin("L.esl", "l"),
    ]);
    // the game's two count, but light plugins don't
    plugins.limit = Some(4);
    assert!(LoadOrder::new(&[], plugins.clone()).warnings.is_empty());
    plugins.limit = Some(3);
    let warnings = LoadOrder::new(&[], plugins).warnings;
    assert!(warnings[0].starts_with("4 plugins"), "{warnings:?}");
}

#[test]
fn light_implicit_plugins_dont_count_towards_the_limit() {
    let mut plugins = found(vec![plugin("A.esp", "a")]);
    plugins.implicit.push("ccA.esl".to_owned());
    plugins.implicit.push("ccB.esm".to_owned());
    plugins.light_implicit = vec!["CCA.esl".to_owned(), "ccB.esm".to_owned()];
    plugins.limit = Some(3);
    assert!(LoadOrder::new(&[], plugins.clone()).warnings.is_empty());
    plugins.light_implicit.pop();
    let warnings = LoadOrder::new(&[], plugins).warnings;
    assert!(warnings[0].starts_with("4 plugins"), "{warnings:?}");
}

#[test]
fn saving_keeps_the_places_of_plugins_that_are_gone_for_now() {
    let old = [
        entry("First.esp", true),
        entry("A.esp", true),
        entry("Gone.esp", false),
        entry("B.esp", true),
    ];
    let new = vec![entry("B.esp", true), entry("A.esp", true)];
    assert_eq!(
        keep_missing(&old, new),
        [
            entry("First.esp", true),
            entry("B.esp", true),
            entry("A.esp", true),
            entry("Gone.esp", false),
        ]
    );
}

#[test]
fn moving_keeps_masters_above_the_rest() {
    let mut order = LoadOrder::new(
        &[],
        found(vec![
            plugin("M1.esm", "a"),
            plugin("M2.esm", "a"),
            plugin("A.esp", "a"),
            plugin("B.esp", "a"),
        ]),
    );
    assert_eq!(order.move_plugin("B.esp", &Position::Top).unwrap(), 2);
    assert_eq!(names(&order), ["M1.esm", "M2.esm", "B.esp", "A.esp"]);
    assert_eq!(order.move_plugin("M1.esm", &Position::Bottom).unwrap(), 1);
    assert_eq!(
        order
            .move_plugin("a.esp", &Position::Before("B.esp".into()))
            .unwrap(),
        2
    );
    assert_eq!(names(&order), ["M2.esm", "M1.esm", "A.esp", "B.esp"]);
    assert_eq!(
        order
            .move_plugin("M2.esm", &Position::After("M1.esm".into()))
            .unwrap(),
        1
    );
    assert!(order.move_plugin("Nope.esp", &Position::Top).is_err());
    order.set_enabled("b.esp", false).unwrap();
    assert!(!order.plugins[3].enabled);
}
