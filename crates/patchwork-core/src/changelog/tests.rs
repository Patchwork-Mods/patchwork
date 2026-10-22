use super::*;
use crate::source::compare_dotted;

fn entry(version: Option<&str>, notes: &str) -> ChangelogEntry {
    ChangelogEntry {
        version: version.map(str::to_owned),
        notes: notes.to_owned(),
    }
}

fn versions(entries: &[ChangelogEntry]) -> Vec<Option<&str>> {
    entries.iter().map(|e| e.version.as_deref()).collect()
}

#[test]
fn splits_keep_a_changelog() {
    let markdown = "# Changelog\n\
        \n\
        All notable changes to this project will be documented in this file.\n\
        \n\
        ## [Unreleased]\n\
        - Nothing yet\n\
        \n\
        ## LethalLib [1.2.0] - 2024-05-01\n\
        \n\
        ### Added\n\
        - A thing\n\
        \n\
        ## [v1.1.0]\n\
        ### Fixed\n\
        - Another thing\n";
    let entries = split_markdown(markdown);
    // the Unreleased section comes before the first version, so goes with
    // the preamble
    assert_eq!(versions(&entries), [Some("1.2.0"), Some("1.1.0")]);
    assert_eq!(entries[0].notes, "### Added\n- A thing");
    assert_eq!(entries[1].notes, "### Fixed\n- Another thing");
}

#[test]
fn deeper_headings_stay_in_their_entry() {
    let markdown = "### 2.0.0\n#### Changed\n- x\n### 1.0.0\n- y";
    let entries = split_markdown(markdown);
    assert_eq!(versions(&entries), [Some("2.0.0"), Some("1.0.0")]);
    assert_eq!(entries[0].notes, "#### Changed\n- x");
}

#[test]
fn ignores_headings_in_code_blocks() {
    let markdown = "## 1.1.0\n```\n## 9.9.9\n```\n## 1.0.0\n- first";
    let entries = split_markdown(markdown);
    assert_eq!(versions(&entries), [Some("1.1.0"), Some("1.0.0")]);
    assert_eq!(entries[0].notes, "```\n## 9.9.9\n```");
}

#[test]
fn splits_on_lines_starting_with_versions() {
    let markdown =
        "**3.1.2**\n* Fixed minions\n\n**3.1.1**\n* Fixed saving\n\nVersion 3.0: big one\n- x";
    let entries = split_markdown(markdown);
    assert_eq!(
        versions(&entries),
        [Some("3.1.2"), Some("3.1.1"), Some("3.0")]
    );
    assert_eq!(entries[0].notes, "* Fixed minions");
}

#[test]
fn splits_on_versions_opening_code_blocks() {
    // WolfFixes writes each version as a code block, then switches to
    // headings for older ones
    let markdown = "```\r\nv1.4.4\r\nFixed Lantern.\r\n```\r\n```\r\nv.1.4.0 - DLC4\r\n* a\r\n```\r\n\
        ## 1.2.0\r\nOld notes\r\n```\r\nconfig = 1.0.0\r\n```";
    let entries = split_markdown(markdown);
    assert_eq!(
        versions(&entries),
        [Some("1.4.4"), Some("1.4.0"), Some("1.2.0")]
    );
    assert_eq!(entries[0].notes, "```\nFixed Lantern.\n```");
    assert_eq!(entries[1].notes, "```\n* a\n```");
}

#[test]
fn notes_mentioning_versions_stay_notes() {
    let markdown =
        "## 2.0.0\n- 1.9 configs are migrated\n  1.5 too\nNeeds R2API 5.0 or newer\n1.5x faster";
    let entries = split_markdown(markdown);
    assert_eq!(versions(&entries), [Some("2.0.0")]);
}

#[test]
fn unsplittable_changelog_comes_back_whole() {
    let markdown = "\n- 1.0.1: fixed a crash\n- 1.0.0: first release\n";
    assert_eq!(
        split_markdown(markdown),
        [entry(
            None,
            "- 1.0.1: fixed a crash\n- 1.0.0: first release"
        )]
    );
    assert_eq!(split_markdown("  \n"), []);
}

#[test]
fn finds_versions() {
    assert_eq!(find_version("v1.5.4"), Some("1.5.4"));
    assert_eq!(find_version("Version 2.10 (hotfix)"), Some("2.10"));
    assert_eq!(
        find_version("[1.0.0-beta.2] - 2024-01-01"),
        Some("1.0.0-beta.2")
    );
    assert_eq!(find_version("x64.2 build"), None);
    assert_eq!(find_version("Release 3"), None);
    assert_eq!(find_version("Added"), None);
}

#[test]
fn between_takes_versions_after_from_up_to_to() {
    let entries = [
        entry(Some("1.3.0"), "c"),
        entry(Some("1.2.0"), "b"),
        entry(Some("1.1.0"), "a"),
        entry(None, "unreleased-ish"),
        entry(Some("1.0.0"), "first"),
    ];
    let picked = between(&entries, Some("1.0.0"), Some("1.2.0"), compare_dotted);
    assert_eq!(versions(&picked), [Some("1.2.0"), Some("1.1.0")]);
    // a new mod gets everything up to its version
    let picked = between(&entries, None, Some("1.1.0"), compare_dotted);
    assert_eq!(versions(&picked), [Some("1.1.0"), Some("1.0.0")]);
    let picked = between(&entries, Some("v1.2.0"), None, compare_dotted);
    assert_eq!(versions(&picked), [Some("1.3.0")]);
}

#[test]
fn between_falls_back_to_order_for_free_text_versions() {
    let entries = [
        entry(Some("5.3SE"), "c"),
        entry(Some("5.2SE"), "b"),
        entry(Some("5.1SE"), "a"),
    ];
    let picked = between(&entries, Some("5.1SE"), Some("5.2SE"), compare_dotted);
    assert_eq!(versions(&picked), [Some("5.2SE")]);
    // a version that isn't in the changelog doesn't cut anything off
    let picked = between(&entries, Some("4.0SE"), None, compare_dotted);
    assert_eq!(picked.len(), 3);
}

#[test]
fn between_keeps_unversioned_changelogs() {
    let entries = [entry(None, "everything")];
    assert_eq!(
        between(&entries, Some("1.0"), Some("2.0"), compare_dotted),
        entries
    );
}
