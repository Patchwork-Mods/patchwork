//! Finding mods on a source. A search with no text is how a front end shows a
//! source's home screen (its most popular or recently updated mods).
use std::fmt;
use std::str::FromStr;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::source::ModSpec;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortBy {
    /// How well a mod matches the search text.
    Relevance,
    /// Most downloaded first.
    Downloads,
    /// Most recently updated first.
    Updated,
    /// Most recently created first.
    Newest,
    /// Best rated (likes, endorsements, ...) first.
    Rating,
}

impl SortBy {
    pub const ALL: [SortBy; 5] = [
        SortBy::Relevance,
        SortBy::Downloads,
        SortBy::Updated,
        SortBy::Newest,
        SortBy::Rating,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SortBy::Relevance => "relevance",
            SortBy::Downloads => "downloads",
            SortBy::Updated => "updated",
            SortBy::Newest => "newest",
            SortBy::Rating => "rating",
        }
    }
}

impl fmt::Display for SortBy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SortBy {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        SortBy::ALL
            .into_iter()
            .find(|sort| sort.as_str() == s)
            .ok_or_else(|| {
                let all: Vec<_> = SortBy::ALL.iter().map(|s| s.as_str()).collect();
                Error::invalid(format!(
                    "unknown sort `{s}`; expected one of {}",
                    all.join(", ")
                ))
            })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// What to search for. `None` (or blank) lists everything, which is the
    /// home screen.
    pub text: Option<String>,
    /// `None` lets the source pick; [`SearchPage::sort`] says what it picked.
    pub sort: Option<SortBy>,
    /// [`Category::id`]s; mods must be in at least one of them.
    pub categories: Vec<String>,
    /// [`Category::id`]s; mods in any of them are left out.
    pub excluded_categories: Vec<String>,
    /// Zero-based. Sources choose their own page size.
    pub page: u32,
}

impl SearchQuery {
    /// The search text, if there's any to search for.
    pub fn text(&self) -> Option<&str> {
        self.text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    /// What goes in [`SearchQuery::categories`].
    pub id: String,
    pub name: String,
}

/// Enough about a mod to show it in a list. Turn it into a [`ModSpec`] with
/// [`ModSummary::spec`] to add it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModSummary {
    pub source: String,
    /// The same id [`crate::source::Source::resolve`] takes.
    pub id: String,
    pub name: String,
    /// Which of the mod's files the id picks, for ids that pick one of
    /// several, like `Preloader` for `nexus:17230/Engine Fixes * - Preloader`.
    /// It's shown with the name, so mods added for two files of one mod
    /// can be told apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The mod's page on the source's site.
    pub url: Option<String>,
    pub author: Option<String>,
    /// The author's page on the source's site, listing their mods.
    pub author_url: Option<String>,
    pub summary: Option<String>,
    pub icon_url: Option<String>,
    pub downloads: Option<u64>,
    /// Likes, endorsements or whatever the source counts.
    pub rating: Option<u64>,
    pub updated: Option<Timestamp>,
    pub latest_version: Option<String>,
    pub categories: Vec<Category>,
}

impl ModSummary {
    pub fn spec(&self) -> ModSpec {
        ModSpec {
            source: self.source.clone(),
            id: self.id.clone(),
            version: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchPage {
    pub items: Vec<ModSummary>,
    /// How many mods match across every page, if the source says.
    pub total: Option<u64>,
    pub has_more: bool,
    /// The order the items are in.
    pub sort: SortBy,
}

impl SearchPage {
    pub fn empty(sort: SortBy) -> Self {
        SearchPage {
            items: Vec::new(),
            total: Some(0),
            has_more: false,
            sort,
        }
    }
}

#[cfg(test)]
mod tests;
