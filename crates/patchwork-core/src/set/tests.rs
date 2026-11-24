use super::*;
use crate::profile::Include;

fn m(name: &str) -> ProfileMod {
    toml::from_str(&format!("name = {name:?}\nsource = \"ts\"\nid = {name:?}")).unwrap()
}

fn set(name: &str, mods: &[&str]) -> Set {
    let mut set = Set::new("game", name);
    set.mods = mods.iter().map(|n| m(n)).collect();
    set
}

fn profile(mods: &[&str], includes: &[(&Set, Option<&str>)]) -> Profile {
    let mut p = Profile::new("game", "default");
    p.mods = mods.iter().map(|n| m(n)).collect();
    p.includes = includes
        .iter()
        .map(|(s, after)| Include {
            after: after.map(str::to_owned),
            ..Include::new(s.id)
        })
        .collect();
    p
}

fn expanded(mut p: Profile, sets: &[&Set]) -> Profile {
    expand(&mut p, |id| {
        sets.iter().find(|s| s.id == id).map(|s| (*s).clone())
    });
    p
}

fn names(p: &Profile) -> Vec<&str> {
    p.mods.iter().map(|m| m.name.as_str()).collect()
}

#[test]
fn a_sets_mods_go_where_its_include_is() {
    let qol = set("QOL", &["x", "y"]);
    let ui = set("UI", &["u"]);
    let p = expanded(
        profile(&["a", "b"], &[(&qol, Some("a")), (&ui, None)]),
        &[&qol, &ui],
    );
    assert_eq!(names(&p), ["u", "a", "x", "y", "b"]);
    assert_eq!(p.folder_of(&p.mods[2]).unwrap().name, "QOL");
    assert!(p.is_linked(&p.mods[0]));
    assert!(!p.is_linked(&p.mods[1]));
}

#[test]
fn an_include_after_a_mod_that_is_gone_goes_last() {
    let qol = set("QOL", &["x"]);
    let p = expanded(profile(&["a", "b"], &[(&qol, Some("gone"))]), &[&qol]);
    assert_eq!(names(&p), ["a", "b", "x"]);
}

#[test]
fn the_profiles_own_mod_wins_and_the_sets_is_kept() {
    let qol = set("QOL", &["x", "a", "y"]);
    let stored = profile(&["a"], &[(&qol, Some("a"))]);
    let p = expanded(stored.clone(), &[&qol]);
    assert_eq!(names(&p), ["a", "x", "y"]);
    assert!(!p.is_linked(&p.mods[0]));
    let (back, changed) = collapse(&p);
    assert!(changed.is_empty(), "{changed:?}");
    assert_eq!(back.mods, stored.mods);
    assert_eq!(back.includes[0].after.as_deref(), Some("a"));
}

#[test]
fn a_taken_name_is_shown_renamed_and_saved_as_it_was() {
    let qol = set("QOL", &["x"]);
    let mut stored = profile(&[], &[(&qol, None)]);
    let mut other = m("x");
    other.id = "other".into();
    stored.mods.push(other);
    let mut p = expanded(stored, &[&qol]);
    assert_eq!(names(&p), ["x (QOL)", "x"]);
    p.mods[0].enabled = false;
    let (back, changed) = collapse(&p);
    assert!(changed.is_empty(), "{changed:?}");
    assert_eq!(back.includes[0].disabled, ["x"]);
}

#[test]
fn turning_a_sets_mod_off_only_changes_the_profile() {
    let qol = set("QOL", &["x", "y"]);
    let mut p = expanded(profile(&["a"], &[(&qol, Some("a"))]), &[&qol]);
    p.mods[2].enabled = false;
    let (back, changed) = collapse(&p);
    assert!(changed.is_empty(), "{changed:?}");
    assert_eq!(back.includes[0].disabled, ["y"]);
    assert_eq!(names(&back), ["a"]);
    let again = expanded(back, &[&qol]);
    assert!(!again.mods[2].enabled);
}

#[test]
fn editing_a_sets_mods_changes_the_set() {
    let qol = set("QOL", &["x", "y"]);
    let mut p = expanded(profile(&["a"], &[(&qol, Some("a"))]), &[&qol]);
    p.mods[1].priority = 5;
    // moving `y` out of the linked folder takes it out of the set
    p.mods[2].folder = None;
    // and moving `a` in puts it in the set
    p.mods[0].folder = Some(qol.id);
    let (back, changed) = collapse(&p);
    assert_eq!(names(&back), ["y"]);
    assert_eq!(back.includes[0].after, None);
    let (id, mods) = &changed[0];
    assert_eq!(*id, qol.id);
    let set_names: Vec<&str> = mods.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(set_names, ["a", "x"]);
    assert_eq!(mods[1].priority, 5);
    assert!(mods.iter().all(|m| m.enabled && m.folder.is_none()));
}

#[test]
fn moving_a_linked_folder_moves_its_include() {
    let qol = set("QOL", &["x"]);
    let ui = set("UI", &["u"]);
    let mut p = expanded(
        profile(&["a", "b"], &[(&qol, Some("a")), (&ui, Some("b"))]),
        &[&qol, &ui],
    );
    assert_eq!(names(&p), ["a", "x", "b", "u"]);
    let u = p.mods.remove(3);
    p.mods.insert(0, u);
    let (back, _) = collapse(&p);
    assert_eq!(back.includes[0].set, ui.id);
    assert_eq!(back.includes[0].after, None);
    assert_eq!(back.includes[1].after.as_deref(), Some("a"));
    assert_eq!(names(&expanded(back, &[&qol, &ui])), ["u", "a", "x", "b"]);
}

#[test]
fn a_missing_set_is_kept_as_it_was() {
    let qol = set("QOL", &["x"]);
    let stored = profile(&["a"], &[(&qol, Some("a"))]);
    let p = expanded(stored.clone(), &[]);
    assert_eq!(names(&p), ["a"]);
    assert!(p.includes[0].linked.is_none());
    let (back, changed) = collapse(&p);
    assert!(changed.is_empty());
    assert_eq!(back.includes, stored.includes);
}

#[test]
fn profiles_without_includes_keep_the_old_schema() {
    let qol = set("QOL", &[]);
    assert_eq!(profile(&["a"], &[]).saved_schema(), 3);
    assert_eq!(profile(&["a"], &[(&qol, None)]).saved_schema(), 4);
    let saved = toml::to_string(&profile(&["a"], &[(&qol, Some("a"))])).unwrap();
    assert!(saved.contains("[[include]]"), "{saved}");
    assert!(saved.contains("after = \"a\""), "{saved}");
}
