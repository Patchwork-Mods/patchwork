use super::*;
use crate::list::{ListItem, OrderedItem};

fn item(name: &str, tier: u8, enabled: bool) -> OrderedItem {
    OrderedItem {
        item: ListItem {
            name: name.to_owned(),
            mod_name: "m".to_owned(),
            tier,
            badges: Vec::new(),
            after: Vec::new(),
            detail: None,
            can_disable: true,
        },
        enabled,
    }
}

fn list(items: Vec<OrderedItem>) -> List {
    List {
        id: "a.l".to_owned(),
        title: "L".to_owned(),
        description: None,
        tiers: Vec::new(),
        items,
        fixed: Vec::new(),
        problems: Vec::new(),
    }
}

fn names(entries: &[ListEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

fn entries(names: &[&str]) -> Vec<ListEntry> {
    names
        .iter()
        .map(|n| ListEntry {
            name: (*n).to_owned(),
            enabled: true,
        })
        .collect()
}

#[test]
fn reorder_puts_named_first_and_keeps_tiers_and_switches() {
    let l = list(vec![
        item("M.esm", 0, true),
        item("a.esp", 1, true),
        item("b.esp", 1, false),
        item("c.esp", 1, true),
    ]);
    let order = ["C.ESP", "a.esp", "M.esm", "c.esp", "gone.esp"].map(String::from);
    let (got, unknown) = reorder(&l, &order);
    assert_eq!(names(&got), ["M.esm", "c.esp", "a.esp", "b.esp"]);
    assert!(!got[3].enabled);
    assert_eq!(unknown, ["gone.esp"]);
}

#[test]
fn moved_is_the_fewest_items() {
    let before = entries(&["a", "b", "c", "d", "e"]);
    let after = entries(&["a", "d", "b", "c", "e"]);
    let m = moved(&before, &after);
    assert_eq!(
        m,
        [Moved {
            name: "d".to_owned(),
            from: 3,
            to: 1
        }]
    );
    assert!(moved(&before, &before).is_empty());
    let reversed = entries(&["e", "d", "c", "b", "a"]);
    assert_eq!(moved(&before, &reversed).len(), 4);
}

#[test]
fn own_and_declared_hooks_may_be_used() {
    assert!(may_use("loot", &[], "loot.sort"));
    assert!(may_use("loot", &["bethesda.plugins"], "bethesda.plugins"));
    assert!(!may_use("loot", &[], "bethesda.plugins"));
    assert!(!may_use("loot", &[], "lootx.sort"));
}
