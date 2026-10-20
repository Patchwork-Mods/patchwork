//! Finding a profile's own mods by what's typed, as a search box over them
//! does. Unlike [`crate::search`], nothing goes to a source: it only narrows
//! down and reorders mods that are already known, by their names and (once
//! they've been fetched) their summaries.
//!
//! A word typed matches one of a mod's fields, and counts for more in its
//! name than its author, and more in its author than its description, so
//! searching an author puts their own mods above ones that only mention them.
//! Close misspellings still match, for less.
//!
//! Every word but one has to match, so a mistake too small to count as a
//! misspelling, like "risk on rain", or a word still being typed, like "risk
//! of re", doesn't hide everything. Mods matching more of the words go first,
//! as search engines do (Meilisearch's matching strategies, say).

use unicode_normalization::UnicodeNormalization;
use unicode_normalization::char::is_combining_mark;

use crate::profile::ProfileMod;
use crate::search::ModSummary;

/// What a mod can be found by.
#[derive(Debug, Clone, Copy)]
pub struct ModText<'a> {
    pub name: &'a str,
    pub author: Option<&'a str>,
    pub summary: Option<&'a str>,
}

impl<'a> ModText<'a> {
    /// A profile's mod, by its summary's name, author and description once
    /// that's been fetched, or else just its own name.
    pub fn of(m: &'a ProfileMod, summary: Option<&'a ModSummary>) -> Self {
        ModText {
            name: summary.map_or(&m.name, |s| &s.name),
            author: summary.and_then(|s| s.author.as_deref()),
            summary: summary.and_then(|s| s.summary.as_deref()),
        }
    }
}

/// The indices of the mods matching all of `query`'s words, or all but one,
/// best first, with ties kept in the order given. A blank query matches every
/// mod.
pub fn rank(mods: &[ModText], query: &str) -> Vec<usize> {
    let terms = words(query);
    if terms.is_empty() {
        return (0..mods.len()).collect();
    }
    let phrase = terms.concat();
    let needed = terms.len().saturating_sub(1).max(1);
    let mut scored = Vec::new();
    for (i, m) in mods.iter().enumerate() {
        let fields = [
            Field::new(3.0, Some(m.name)),
            Field::new(2.0, m.author),
            Field::new(1.0, m.summary),
        ];
        let mut score = 0.0;
        let mut matched = 0;
        for term in &terms {
            let best = fields
                .iter()
                .map(|f| f.weight * word_match(term, f))
                .fold(0.0, f64::max);
            if best > 0.0 {
                matched += 1;
                score += best;
            }
        }
        if matched < needed {
            continue;
        }
        // all of it together in the name, as in "lethalthings" for LethalThings
        if terms.len() > 1 && fields[0].joined.contains(&phrase) {
            score += 2.0;
        }
        if fields[0].joined == phrase {
            score += 2.0;
        }
        scored.push((i, matched, score));
    }
    // stable, so ties keep their order
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.total_cmp(&a.2)));
    scored.into_iter().map(|(i, _, _)| i).collect()
}

/// One of a mod's fields, split up for matching.
struct Field {
    weight: f64,
    words: Vec<String>,
    /// The words run together, for matching across the gaps between them.
    joined: String,
}

impl Field {
    fn new(weight: f64, text: Option<&str>) -> Self {
        let words = words(text.unwrap_or(""));
        let joined = words.concat();
        Field {
            weight,
            words,
            joined,
        }
    }
}

/// Lowercase words without accents, also splitting camelCase
/// ("RiskOfOptions").
fn words(text: &str) -> Vec<String> {
    let mut split = String::with_capacity(text.len());
    let mut prev_lower = false;
    for c in text.chars() {
        if prev_lower && c.is_uppercase() {
            split.push(' ');
        }
        prev_lower = c.is_lowercase();
        split.push(c);
    }
    let plain: String = split
        .nfkd()
        .filter(|&c| !is_combining_mark(c))
        .collect::<String>()
        .to_lowercase();
    plain
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

/// How well one typed word matches a field, from 0 to 1.
fn word_match(term: &str, field: &Field) -> f64 {
    let len = term.chars().count();
    let mut best: f64 = 0.0;
    // short words only match the start of a word, or "a" would match everything
    if len >= 3 && field.joined.contains(term) {
        best = 0.6;
    }
    let typos = match len {
        0..4 => 0,
        4..8 => 1,
        _ => 2,
    };
    for word in &field.words {
        if word == term {
            return 1.0;
        }
        if word.starts_with(term) {
            best = best.max(0.8);
        } else if typos > 0 && best < 0.5 {
            let whole = edit_distance(term, word);
            // or the start of the word, for one that's still being typed
            let start = if word.chars().count() > len {
                let start: String = word.chars().take(len).collect();
                edit_distance(term, &start)
            } else {
                usize::MAX
            };
            if whole <= typos {
                best = best.max(0.5 - 0.15 * (whole as f64 - 1.0));
            } else if start <= typos {
                best = best.max(0.4 - 0.15 * (start as f64 - 1.0));
            }
        }
    }
    best
}

/// Edits (including swapping two letters) to turn `a` into `b`.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev2: Vec<usize> = Vec::new();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut row = vec![i; b.len() + 1];
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            row[j] = (prev[j] + 1).min(row[j - 1] + 1).min(prev[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                row[j] = row[j].min(prev2[j - 2] + 1);
            }
        }
        prev2 = std::mem::replace(&mut prev, row);
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests;
