use super::*;

struct Test;

impl OrderedList for Test {
    fn title(&self) -> &str {
        "Plugins"
    }

    fn items(&self, _: &ListContext<'_>, _: &[InstalledMod]) -> Result<ListItems> {
        unreachable!("these tests make the items themselves")
    }
}

fn item(name: &str, mod_name: &str) -> ListItem {
    ListItem {
        name: name.to_owned(),
        mod_name: mod_name.to_owned(),
        tier: if name.ends_with(".esm") { 0 } else { 1 },
        badges: Vec::new(),
        after: Vec::new(),
        detail: None,
        can_disable: true,
    }
}

fn needing(name: &str, after: &[&str]) -> ListItem {
    ListItem {
        after: after.iter().map(|m| m.to_string()).collect(),
        ..item(name, "m")
    }
}

fn found(items: Vec<ListItem>) -> ListItems {
    ListItems {
        items,
        fixed: vec![item("Skyrim.esm", ""), item("Update.esm", "")],
        tiers: Vec::new(),
        warnings: Vec::new(),
    }
}

fn entry(name: &str, enabled: bool) -> ListEntry {
    ListEntry {
        name: name.to_owned(),
        enabled,
    }
}

fn list(saved: &[ListEntry], found: ListItems) -> List {
    List::new("test.plugins".to_owned(), &Test, saved, found)
}

fn names(list: &List) -> Vec<&str> {
    list.items.iter().map(|i| i.item.name.as_str()).collect()
}

#[test]
fn unsaved_items_follow_their_mods() {
    let list = list(&[], found(vec![item("A.esp", "a"), item("B.esp", "b")]));
    assert_eq!(names(&list), ["A.esp", "B.esp"]);
    assert!(list.items.iter().all(|i| i.enabled));
}

#[test]
fn saved_order_wins_and_new_items_go_last() {
    let saved = [
        entry("b.ESP", false),
        entry("Gone.esp", true),
        entry("A.esp", true),
    ];
    let list = list(
        &saved,
        found(vec![
            item("A.esp", "a"),
            item("B.esp", "b"),
            item("New.esp", "c"),
        ]),
    );
    assert_eq!(names(&list), ["B.esp", "A.esp", "New.esp"]);
    assert!(!list.items[0].enabled);
    assert_eq!(
        list.entries(),
        [
            entry("B.esp", false),
            entry("A.esp", true),
            entry("New.esp", true)
        ]
    );
}

#[test]
fn lower_tiers_go_first_and_fixed_items_are_left_out() {
    let saved = [entry("A.esp", true), entry("M.esm", true)];
    let list = list(
        &saved,
        found(vec![
            item("A.esp", "a"),
            item("update.esm", "cleaned"),
            item("M.esm", "m"),
            item("L.esl", "l"),
        ]),
    );
    assert_eq!(names(&list), ["M.esm", "A.esp", "L.esl"]);
}

#[test]
fn the_later_mod_provides_an_item_two_have() {
    let list = list(&[], found(vec![item("A.esp", "a"), item("a.esp", "b")]));
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].item.mod_name, "b");
}

#[test]
fn items_that_cant_be_disabled_stay_enabled() {
    let mut always = item("A.esp", "a");
    always.can_disable = false;
    let mut list = list(&[entry("A.esp", false)], found(vec![always]));
    assert!(list.items[0].enabled);
    assert!(list.set_enabled("A.esp", false).is_err());
}

#[test]
fn missing_and_late_needs_are_warned_about() {
    let saved = [
        entry("Patch.esp", true),
        entry("Base.esp", true),
        entry("Off.esp", false),
    ];
    let list = list(
        &saved,
        found(vec![
            needing("Base.esp", &["Skyrim.esm"]),
            needing(
                "Patch.esp",
                &["Skyrim.esm", "Base.esp", "Off.esp", "Nowhere.esp"],
            ),
            item("Off.esp", "o"),
        ]),
    );
    assert_eq!(list.warnings.len(), 3, "{:?}", list.warnings);
    assert!(list.warnings[0].contains("Base.esp, which comes after it"));
    assert!(list.warnings[1].contains("Off.esp, which is disabled"));
    assert!(list.warnings[2].contains("Nowhere.esp, which isn't in any of the mods"));
}

#[test]
fn saving_keeps_the_places_of_items_that_are_gone_for_now() {
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
fn moving_keeps_items_in_their_tier() {
    let mut list = list(
        &[],
        found(vec![
            item("M1.esm", "a"),
            item("M2.esm", "a"),
            item("A.esp", "a"),
            item("B.esp", "a"),
        ]),
    );
    assert_eq!(list.move_item("B.esp", &Position::Top).unwrap(), 2);
    assert_eq!(names(&list), ["M1.esm", "M2.esm", "B.esp", "A.esp"]);
    assert_eq!(list.move_item("M1.esm", &Position::Bottom).unwrap(), 1);
    assert_eq!(
        list.move_item("a.esp", &Position::Before("B.esp".into()))
            .unwrap(),
        2
    );
    assert_eq!(names(&list), ["M2.esm", "M1.esm", "A.esp", "B.esp"]);
    assert_eq!(
        list.move_item("M2.esm", &Position::After("M1.esm".into()))
            .unwrap(),
        1
    );
    assert!(list.move_item("Nope.esp", &Position::Top).is_err());
    list.set_enabled("b.esp", false).unwrap();
    assert!(!list.items[3].enabled);
}
