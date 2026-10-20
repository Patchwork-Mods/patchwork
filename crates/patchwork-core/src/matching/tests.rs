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
fn every_word_has_to_match_with_close_misspellings_allowed() {
    let mods = [
        named("LethalThings"),
        named("MoreCompany"),
        named("LethalConfig"),
    ];
    assert_eq!(found(&mods, "lethal thngs"), ["LethalThings"]);
    assert_eq!(found(&mods, "lethalthings"), ["LethalThings"]);
    assert_eq!(found(&mods, "lethal"), ["LethalThings", "LethalConfig"]);
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
