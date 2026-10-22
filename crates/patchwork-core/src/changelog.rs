//! Mods' changelogs, so front ends can say what an update changes without
//! sending users to the mod's page.
//!
//! Sources hand changelogs over as [`ChangelogEntry`]s, newest first. Ones
//! that only have a single markdown document (Thunderstore's `CHANGELOG.md`)
//! can split it into entries with [`split_markdown`].
use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

/// One version's notes in a mod's changelog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangelogEntry {
    /// The version the notes are for. `None` when they couldn't be tied to
    /// one, like a changelog that isn't split up by version.
    pub version: Option<String>,
    /// The notes, as markdown.
    pub notes: String,
}

/// Splits a markdown changelog into one entry per version, newest first as
/// changelogs are written. Changelogs are written all sorts of ways, so an
/// entry starts at any of:
///
/// - a heading with a version in it (`## 1.2.0`, `### [v1.2.0] - 2024-01-01`).
///   The first one sets which level of heading starts an entry, so later
///   ones at that level without a version (`## Unreleased`) do too.
/// - a line that starts with a version (`v1.2.0`, `**1.2.0**`, `Version 1.2`).
/// - a version on the first line of a code block, for changelogs that put
///   each version's notes in one. The code block stays in the notes.
///
/// Whatever comes before the first entry, usually a title and some
/// boilerplate, is dropped. A changelog with no versions marked comes back
/// whole, as a single entry without a version. An empty one gives no
/// entries.
pub fn split_markdown(markdown: &str) -> Vec<ChangelogEntry> {
    let mut level = None;
    let mut entries: Vec<ChangelogEntry> = Vec::new();
    let mut fence: Option<&str> = None;
    // a code block's opening line, held back until the next line says
    // whether it starts an entry
    let mut opened: Option<&str> = None;
    let start = |entries: &mut Vec<ChangelogEntry>, version: Option<&str>, notes: &str| {
        entries.push(ChangelogEntry {
            version: version.map(str::to_owned),
            notes: notes.to_owned(),
        })
    };
    let push = |entries: &mut Vec<ChangelogEntry>, line: &str| {
        if let Some(entry) = entries.last_mut() {
            entry.notes.push_str(line);
            entry.notes.push('\n');
        }
    };
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if let Some(open) = opened.take() {
            if let Some(version) = version_line(line) {
                start(&mut entries, Some(version), &format!("{open}\n"));
                continue;
            }
            push(&mut entries, open);
        }
        if let Some(open) = fence {
            // nothing else in a code block marks a version
            if trimmed.starts_with(open) {
                fence = None;
            }
        } else if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            opened = Some(line);
            continue;
        } else if let Some((depth, text)) = heading(line) {
            let version = find_version(text);
            if version.is_some() || level.is_some_and(|level| depth <= level) {
                level.get_or_insert(depth);
                start(&mut entries, version, "");
                continue;
            }
        } else if let Some(version) = version_line(line) {
            start(&mut entries, Some(version), "");
            continue;
        }
        push(&mut entries, line);
    }
    if let Some(open) = opened {
        push(&mut entries, open);
    }
    if entries.is_empty() {
        let notes = markdown.trim();
        return if notes.is_empty() {
            Vec::new()
        } else {
            vec![ChangelogEntry {
                version: None,
                notes: notes.to_owned(),
            }]
        };
    }
    for entry in &mut entries {
        entry.notes = entry.notes.trim().to_owned();
    }
    entries
}

/// The version a line starts with, if it's one that marks where a version's
/// notes start: `v1.2.0`, `v.1.4.0 - DLC4`, `**1.2.0**`, `Version 1.2:` and
/// the like. Indented lines and list items don't count, since they're
/// usually notes that mention a version.
fn version_line(line: &str) -> Option<&str> {
    if line.starts_with([' ', '\t']) {
        return None;
    }
    let text = line.trim_start_matches(['*', '_', '`', '[']);
    let lower = text.to_ascii_lowercase();
    let prefix = ["version ", "ver. ", "ver ", "v.", "v"]
        .iter()
        .find(|p| lower.starts_with(*p))
        .map_or(0, |p| p.len());
    let text = &text[prefix..];
    let len = version_len(text.as_bytes())?;
    let ends = text[len..]
        .chars()
        .next()
        .is_none_or(|c| c.is_whitespace() || ":)]*_`,-".contains(c));
    ends.then(|| &text[..len])
}

/// An ATX heading's depth and text, e.g. `(2, "1.2.0")` for `## 1.2.0 ##`.
fn heading(line: &str) -> Option<(usize, &str)> {
    // more than three spaces in makes it a code block
    let indent = line.len() - line.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let line = &line[indent..];
    let depth = line.bytes().take_while(|&b| b == b'#').count();
    if !(1..=6).contains(&depth) {
        return None;
    }
    let rest = &line[depth..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((depth, rest.trim().trim_end_matches('#').trim_end()))
}

/// The first version-looking word in `text`: digits and dots with at least
/// one dot (`1.2`, `5.4.2100`), maybe followed by a pre-release or build
/// suffix (`1.0.0-beta.2`). A leading `v` is left off.
fn find_version(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        // a version has to start a word, so `x64.2` isn't one
        let at_word = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        let at_v = at_word && matches!(bytes[start], b'v' | b'V');
        let digits_from = if at_v { start + 1 } else { start };
        if at_word && let Some(len) = version_len(&bytes[digits_from..]) {
            return Some(&text[digits_from..digits_from + len]);
        }
        start += 1;
    }
    None
}

/// How long the version at the start of `bytes` is, if there is one.
fn version_len(bytes: &[u8]) -> Option<usize> {
    let number = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count()
    };
    let mut end = number(0);
    if end == 0 {
        return None;
    }
    let mut parts = 1;
    while bytes.get(end) == Some(&b'.') && number(end + 1) > 0 {
        end += 1 + number(end + 1);
        parts += 1;
    }
    if parts < 2 {
        return None;
    }
    // a suffix like `-beta.2` or `+build5`
    if matches!(bytes.get(end), Some(b'-' | b'+'))
        && bytes.get(end + 1).is_some_and(u8::is_ascii_alphanumeric)
    {
        end += 1;
        end += bytes[end..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric() || **b == b'.' || **b == b'-')
            .count();
    }
    Some(end)
}

/// Whether two versions are the same, ignoring a leading `v` and case.
pub fn same_version(a: &str, b: &str) -> bool {
    let strip = |v: &str| v.trim().trim_start_matches(['v', 'V']).to_ascii_lowercase();
    strip(a) == strip(b)
}

/// The entries for what changed after `from` up to and including `to`, which
/// is what moving a mod from `from` to `to` brings in. `None` for `from` means
/// the mod is new, so everything up to `to` counts, and `None` for `to`
/// means up to the newest.
///
/// Versions are ordered with `compare` where it can, or else by where they
/// are in `entries` (newest first). Entries without a version are left out,
/// unless no entry has one, in which case they all come back, since there's
/// no telling which are new.
pub fn between(
    entries: &[ChangelogEntry],
    from: Option<&str>,
    to: Option<&str>,
    compare: impl Fn(&str, &str) -> Option<Ordering>,
) -> Vec<ChangelogEntry> {
    if entries.iter().all(|e| e.version.is_none()) {
        return entries.to_vec();
    }
    let position = |wanted: &str| {
        entries.iter().position(|e| {
            e.version
                .as_deref()
                .is_some_and(|v| same_version(v, wanted))
        })
    };
    let newest = to.and_then(position).unwrap_or(0);
    let oldest = from.and_then(position).unwrap_or(entries.len());
    entries
        .iter()
        .enumerate()
        .filter(|(i, e)| {
            let Some(version) = e.version.as_deref() else {
                return false;
            };
            let after_from = match from {
                None => true,
                Some(from) => match compare(version, from) {
                    Some(ordering) => ordering.is_gt(),
                    None => *i < oldest,
                },
            };
            let up_to = match to {
                None => true,
                Some(to) => match compare(version, to) {
                    Some(ordering) => ordering.is_le(),
                    None => *i >= newest,
                },
            };
            after_from && up_to
        })
        .map(|(_, e)| e.clone())
        .collect()
}

#[cfg(test)]
mod tests;
