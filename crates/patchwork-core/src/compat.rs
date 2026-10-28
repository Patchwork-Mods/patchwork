//! Which versions of a game (and values of its other `[env]` axes) a mod
//! works with, and working that out from what sources say about their files
//! when they have nothing better.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::environment::GameEnv;
use crate::game::GameDef;
use crate::relpath::RelPath;
use crate::source::compare_dotted;

/// The versions of a game a mod works with: any version that one of its
/// ranges allows. Written as a string or a list of them, e.g.
/// `["1.5.97", "1.6.*", ">=1.6.1130 <1.7"]`, where each range is one or
/// more of `=v` (or just `v`), `>=v`, `>v`, `<=v`, `<v`, `v.*` and `*`,
/// all of which have to hold.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "OneOrMany", into = "Vec<String>")]
pub struct GameVersions(Vec<Range>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Range(Vec<Bound>);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Bound {
    Any,
    Exact(String),
    AtLeast(String),
    Above(String),
    AtMost(String),
    Below(String),
    /// `1.6.*`: starts with these numbers.
    Prefix(String),
}

impl GameVersions {
    /// Whether `version` is one of these. `None` means it can't tell,
    /// because the versions aren't ones it knows how to compare.
    pub fn matches(&self, version: &str) -> Option<bool> {
        let mut unknown = false;
        for range in &self.0 {
            match range.matches(version) {
                Some(true) => return Some(true),
                Some(false) => {}
                None => unknown = true,
            }
        }
        if unknown { None } else { Some(false) }
    }

    /// Versions that any of `all` allows.
    pub fn union<'a>(all: impl IntoIterator<Item = &'a GameVersions>) -> Option<GameVersions> {
        let mut ranges: Vec<Range> = Vec::new();
        for versions in all {
            for range in &versions.0 {
                if !ranges.contains(range) {
                    ranges.push(range.clone());
                }
            }
        }
        (!ranges.is_empty()).then_some(GameVersions(ranges))
    }
}

impl Range {
    fn matches(&self, version: &str) -> Option<bool> {
        let mut all = Some(true);
        for bound in &self.0 {
            match bound.matches(version) {
                Some(true) => {}
                Some(false) => return Some(false),
                None => all = None,
            }
        }
        all
    }
}

impl Bound {
    fn matches(&self, version: &str) -> Option<bool> {
        let cmp = |v: &str| compare_dotted(version, v);
        match self {
            Bound::Any => Some(true),
            Bound::Exact(v) => cmp(v).map(Ordering::is_eq),
            Bound::AtLeast(v) => cmp(v).map(Ordering::is_ge),
            Bound::Above(v) => cmp(v).map(Ordering::is_gt),
            Bound::AtMost(v) => cmp(v).map(Ordering::is_le),
            Bound::Below(v) => cmp(v).map(Ordering::is_lt),
            Bound::Prefix(prefix) => {
                let numbers = |v: &str| {
                    v.split('.')
                        .map(|part| part.parse::<u64>().ok())
                        .collect::<Option<Vec<_>>>()
                };
                let (have, want) = (numbers(version)?, numbers(prefix)?);
                Some(
                    want.iter()
                        .enumerate()
                        .all(|(i, n)| have.get(i).unwrap_or(&0) == n),
                )
            }
        }
    }
}

impl FromStr for Range {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        let bound = |word: &str| -> Result<Bound, String> {
            let (make, v): (fn(String) -> Bound, &str) = if let Some(v) = word.strip_prefix(">=") {
                (Bound::AtLeast, v)
            } else if let Some(v) = word.strip_prefix("<=") {
                (Bound::AtMost, v)
            } else if let Some(v) = word.strip_prefix('>') {
                (Bound::Above, v)
            } else if let Some(v) = word.strip_prefix('<') {
                (Bound::Below, v)
            } else if word == "*" {
                return Ok(Bound::Any);
            } else if let Some(v) = word.strip_suffix(".*") {
                (Bound::Prefix, v)
            } else {
                (Bound::Exact, word.strip_prefix('=').unwrap_or(word))
            };
            if v.is_empty() || v.contains('*') {
                return Err(format!("`{word}` isn't a version or range of versions"));
            }
            Ok(make(v.to_owned()))
        };
        let bounds = s
            .split_whitespace()
            .map(bound)
            .collect::<Result<Vec<_>, _>>()?;
        if bounds.is_empty() {
            return Err("a game version range can't be empty".to_owned());
        }
        Ok(Range(bounds))
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let words: Vec<String> = self
            .0
            .iter()
            .map(|b| match b {
                Bound::Any => "*".to_owned(),
                Bound::Exact(v) => v.clone(),
                Bound::AtLeast(v) => format!(">={v}"),
                Bound::Above(v) => format!(">{v}"),
                Bound::AtMost(v) => format!("<={v}"),
                Bound::Below(v) => format!("<{v}"),
                Bound::Prefix(v) => format!("{v}.*"),
            })
            .collect();
        f.write_str(&words.join(" "))
    }
}

/// `1.5.97 or 1.6.*`
impl fmt::Display for GameVersions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ranges: Vec<String> = self.0.iter().map(Range::to_string).collect();
        f.write_str(&ranges.join(" or "))
    }
}

impl FromStr for GameVersions {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Ok(GameVersions(vec![s.parse()?]))
    }
}

impl<S: AsRef<str>> TryFrom<Vec<S>> for GameVersions {
    type Error = String;

    fn try_from(ranges: Vec<S>) -> Result<Self, String> {
        let ranges = ranges
            .iter()
            .map(|r| r.as_ref().parse())
            .collect::<Result<Vec<Range>, _>>()?;
        if ranges.is_empty() {
            return Err("give at least one game version".to_owned());
        }
        Ok(GameVersions(ranges))
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl TryFrom<OneOrMany> for GameVersions {
    type Error = String;

    fn try_from(value: OneOrMany) -> Result<Self, String> {
        match value {
            OneOrMany::One(s) => s.parse(),
            OneOrMany::Many(v) => v.try_into(),
        }
    }
}

impl From<GameVersions> for Vec<String> {
    fn from(versions: GameVersions) -> Self {
        versions.0.iter().map(Range::to_string).collect()
    }
}

/// Which values of a game's `[env]` axes other than `version` a mod works
/// with, by axis, like `loader = ["fabric", "quilt"]`. It works with any
/// value of axes that are left out. Values are matched ignoring case.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvSupport(BTreeMap<String, Vec<String>>);

impl EnvSupport {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Says the mod works with these values of `axis`.
    pub fn with(mut self, axis: &str, values: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.0.insert(
            axis.to_owned(),
            values.into_iter().map(Into::into).collect(),
        );
        self
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &[String])> {
        self.0.iter().map(|(axis, v)| (axis.as_str(), v.as_slice()))
    }

    /// The axes `env` has a value for that isn't one of these, with the
    /// values that would do. Axes `env` doesn't know aren't counted.
    pub fn unmet<'a>(&'a self, env: &GameEnv) -> Vec<(&'a str, &'a [String])> {
        self.iter()
            .filter(|(axis, values)| {
                env.get(axis)
                    .is_some_and(|have| !values.iter().any(|v| v.eq_ignore_ascii_case(have)))
            })
            .collect()
    }
}

/// Guesses which versions a file is for from what its source says about it
/// (its name, description and so on), using the words the game's definition
/// says mark versions (`[versions] labels`). `None` if none of them appear.
/// Words are matched whole and ignoring case, so `AE` matches `Foo AE-1.2`
/// but not `MAE`.
pub fn guess<'a>(game: &GameDef, texts: impl IntoIterator<Item = &'a str>) -> Option<GameVersions> {
    let labels = &game.versions.labels;
    if labels.is_empty() {
        return None;
    }
    let mut found = Vec::new();
    for text in texts {
        // Dots stay part of words, so version labels like `1.6.1170` are
        // matched whole too.
        let words = text
            .split(|c: char| !(c.is_alphanumeric() || c == '.'))
            .map(|w| w.trim_matches('.'));
        for word in words.filter(|w| !w.is_empty()) {
            if let Some((_, versions)) = labels
                .iter()
                .find(|(label, _)| label.eq_ignore_ascii_case(word))
            {
                found.push(versions);
            }
        }
    }
    GameVersions::union(found)
}

/// Whether a mod with these files cares which version of the game it's on,
/// going by the game definition's `[versions] files`. Every mod does if
/// that's empty.
pub fn is_version_specific<'a>(
    game: &GameDef,
    files: impl IntoIterator<Item = &'a RelPath>,
) -> bool {
    let patterns = &game.versions.files;
    if patterns.is_empty() {
        return true;
    }
    let mut set = GlobSetBuilder::new();
    for pattern in patterns {
        // Checked when the definition is loaded.
        if let Ok(glob) = GlobBuilder::new(pattern.as_str())
            .case_insensitive(true)
            .build()
        {
            set.add(glob);
        }
    }
    let set: GlobSet = match set.build() {
        Ok(set) => set,
        Err(_) => return true,
    };
    files.into_iter().any(|f| set.is_match(f.as_str()))
}

#[cfg(test)]
mod tests;
