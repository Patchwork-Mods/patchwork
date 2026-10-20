use super::*;

fn named(name: &str) -> ModText<'_> {
    ModText {
        name,
        author: None,
        summary: None,
    }
}

fn found<'a>(mods: &[ModText<'a>], query: &str) -> Vec<&'a str> {
    rank(mods, query)
        .into_iter()
        .map(|i| mods[i].name)
        .collect()
}

#[test]
fn splits_camel_case_and_drops_accents() {
    assert_eq!(words("RiskOfOptions"), ["risk", "of", "options"]);
    assert_eq!(words("Café-Mod v2"), ["cafe", "mod", "v2"]);
}

#[test]
fn counts_swapped_letters_as_one_edit() {
    assert_eq!(edit_distance("thnigs", "things"), 1);
    assert_eq!(edit_distance("thngs", "things"), 1);
    assert_eq!(edit_distance("", "abc"), 3);
}

#[test]
fn a_blank_query_keeps_everything_in_order() {
    let mods = [named("B"), named("A")];
    assert_eq!(found(&mods, "  "), ["B", "A"]);
}

#[test]
fn close_misspellings_match() {
    let mods = [
        named("LethalThings"),
        named("MoreCompany"),
        named("LethalConfig"),
    ];
    assert_eq!(
        found(&mods, "lethal thngs"),
        ["LethalThings", "LethalConfig"]
    );
    assert_eq!(found(&mods, "lethalthings"), ["LethalThings"]);
    assert_eq!(found(&mods, "lethal"), ["LethalThings", "LethalConfig"]);
}

#[test]
fn every_word_but_one_has_to_match_and_more_matching_goes_first() {
    let mods = [
        named("Risk of Options"),
        named("Risk of Rain 2"),
        named("Lethal Company"),
    ];
    // Risk of Options only has "risk"
    assert_eq!(found(&mods, "risk on rain"), ["Risk of Rain 2"]);
    // "re" matches neither, so they tie on "risk of"
    assert_eq!(
        found(&mods, "risk of re"),
        ["Risk of Options", "Risk of Rain 2"]
    );
    assert_eq!(
        found(&mods, "rain risk"),
        ["Risk of Rain 2", "Risk of Options"]
    );
    assert!(found(&mods, "lethal of rain").contains(&"Risk of Rain 2"));
    assert!(found(&mods, "lethal xyz rain").is_empty());
    // a single word still has to match
    assert!(found(&mods, "xyz").is_empty());
}

#[test]
fn short_words_only_match_the_start_of_a_word() {
    let mods = [named("LethalThings"), named("Alpha")];
    assert_eq!(found(&mods, "a"), ["Alpha"]);
}

#[test]
fn names_count_for_more_than_authors_and_authors_than_descriptions() {
    let mods = [
        ModText {
            name: "Tweaks",
            author: None,
            summary: Some("Works well with Evaisa's mods"),
        },
        ModText {
            name: "LethalLib",
            author: Some("Evaisa"),
            summary: None,
        },
        named("Evaisa Tools"),
    ];
    assert_eq!(
        found(&mods, "evaisa"),
        ["Evaisa Tools", "LethalLib", "Tweaks"]
    );
}
