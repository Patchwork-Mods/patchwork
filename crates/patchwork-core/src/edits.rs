//! What the user has changed about how one of a profile's mods is shown: its
//! name, picture, description and so on, in place of what its source says.
//!
//! Each thing that can be changed is a [`ModField`], listed in
//! [`MOD_FIELDS`]. Adding one is a matter of adding it there: frontends
//! build their editors from the list, and [`ModEdits::apply`] puts the
//! changes on the mod's [`ModSummary`], which is what everything that shows
//! a mod or searches them goes by.
//!
//! Changes are kept in the profile as a table keyed by field id
//! ([`ProfileMod::edits`]), so ones made by a newer patchwork, for fields
//! this one doesn't know, are kept as they are rather than lost.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::profile::ProfileMod;
use crate::search::{Category, ModSummary};

/// What sort of value a [`ModField`] takes, which says how to edit it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    /// One line of text.
    Text,
    /// Text that can run to several lines.
    LongText,
    /// A picture: a URL, or the path to a file on this computer.
    Picture,
    /// A web address.
    Link,
    /// A list of short names, like tags.
    List,
}

/// A value of a [`ModField`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    Text(String),
    List(Vec<String>),
}

/// Something about a mod the user can change.
pub struct ModField {
    /// What it's saved as in [`ProfileMod::edits`], and what frontends
    /// name it by.
    pub id: &'static str,
    pub label: &'static str,
    /// A word on what it's for, for under the field.
    pub help: &'static str,
    pub kind: FieldKind,
    /// What the mod's source says for it, if it says.
    from_source: fn(&ModSummary) -> Option<FieldValue>,
    /// Puts the user's value in place of the source's, for fields that show
    /// as part of the summary. Others (like notes) are only kept.
    apply: Option<fn(&mut ModSummary, FieldValue)>,
}

/// Everything about a mod that can be changed, in the order to show them.
pub static MOD_FIELDS: &[ModField] = &[
    ModField {
        id: "name",
        label: "Name",
        help: "What the mod is called here. The profile still knows it by its own name.",
        kind: FieldKind::Text,
        from_source: |s| Some(FieldValue::Text(s.name.clone())),
        apply: Some(|s, v| {
            if let FieldValue::Text(t) = v {
                s.name = t;
            }
        }),
    },
    ModField {
        id: "picture",
        label: "Picture",
        help: "A link to a picture, or a picture on this computer.",
        kind: FieldKind::Picture,
        from_source: |s| s.icon_url.clone().map(FieldValue::Text),
        apply: Some(|s, v| {
            if let FieldValue::Text(t) = v {
                s.icon_url = Some(t);
            }
        }),
    },
    ModField {
        id: "description",
        label: "Description",
        help: "",
        kind: FieldKind::LongText,
        from_source: |s| s.summary.clone().map(FieldValue::Text),
        apply: Some(|s, v| {
            if let FieldValue::Text(t) = v {
                s.summary = Some(t);
            }
        }),
    },
    ModField {
        id: "author",
        label: "Author",
        help: "",
        kind: FieldKind::Text,
        from_source: |s| s.author.clone().map(FieldValue::Text),
        apply: Some(|s, v| {
            if let FieldValue::Text(t) = v {
                s.author = Some(t);
                // the source's link would be to someone else
                s.author_url = None;
            }
        }),
    },
    ModField {
        id: "page",
        label: "Page",
        help: "Where the mod's name links to.",
        kind: FieldKind::Link,
        from_source: |s| s.url.clone().map(FieldValue::Text),
        apply: Some(|s, v| {
            if let FieldValue::Text(t) = v {
                s.url = Some(t);
            }
        }),
    },
    ModField {
        id: "tags",
        label: "Tags",
        help: "What the mod is sorted, filtered and hidden by.",
        kind: FieldKind::List,
        from_source: |s| {
            (!s.categories.is_empty())
                .then(|| FieldValue::List(s.categories.iter().map(|c| c.name.clone()).collect()))
        },
        apply: Some(|s, v| {
            if let FieldValue::List(tags) = v {
                s.categories = tags
                    .into_iter()
                    .map(|name| Category {
                        id: name.clone(),
                        name,
                    })
                    .collect();
            }
        }),
    },
    ModField {
        id: "notes",
        label: "Notes",
        help: "Anything to remember about the mod. Only shown here.",
        kind: FieldKind::LongText,
        from_source: |_| None,
        apply: None,
    },
];

/// The field with id `id`, if there is one.
pub fn mod_field(id: &str) -> Option<&'static ModField> {
    MOD_FIELDS.iter().find(|f| f.id == id)
}

impl ModField {
    /// What the mod's source says for it, if it says.
    pub fn from_source(&self, summary: &ModSummary) -> Option<FieldValue> {
        (self.from_source)(summary)
    }

    /// Whether it changes how the mod shows, rather than only being kept.
    pub fn shows(&self) -> bool {
        self.apply.is_some()
    }

    /// `value` tidied up (trimmed, blank list items dropped) if it suits the
    /// field, or `None` if that leaves it empty, which means the source's
    /// value is used.
    pub fn check(&self, value: FieldValue) -> Result<Option<FieldValue>> {
        let value = match (self.kind, value) {
            (FieldKind::List, FieldValue::List(items)) => {
                let mut tidy: Vec<String> = Vec::new();
                for item in items {
                    let item = item.trim();
                    if !item.is_empty() && !tidy.iter().any(|t| t == item) {
                        tidy.push(item.to_owned());
                    }
                }
                (!tidy.is_empty()).then_some(FieldValue::List(tidy))
            }
            (FieldKind::List, FieldValue::Text(_)) => {
                return Err(Error::invalid(format!("`{}` takes a list", self.id)));
            }
            (_, FieldValue::List(_)) => {
                return Err(Error::invalid(format!("`{}` takes text", self.id)));
            }
            (kind, FieldValue::Text(text)) => {
                // long text keeps its leading indents and blank lines
                // between paragraphs; anything else is one line
                let text = match kind {
                    FieldKind::LongText => text.trim_end().trim_start_matches(['\r', '\n']),
                    _ => text.trim(),
                };
                if kind != FieldKind::LongText && text.contains('\n') {
                    return Err(Error::invalid(format!("`{}` takes one line", self.id)));
                }
                (!text.is_empty()).then(|| FieldValue::Text(text.to_owned()))
            }
        };
        Ok(value)
    }
}

/// One of a mod's [`MOD_FIELDS`] as it is, for an editor.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldInfo {
    pub id: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    pub kind: FieldKind,
    /// Whether it changes how the mod shows, rather than only being kept.
    pub shows: bool,
    /// What the mod's source says, if it says.
    pub source: Option<FieldValue>,
    /// What the user's changed it to, if they have.
    pub edited: Option<FieldValue>,
}

/// What the user has changed about a mod, by [`ModField::id`]. Ids this
/// patchwork doesn't know are kept but otherwise left alone.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModEdits(BTreeMap<String, toml::Value>);

impl ModEdits {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The user's value for `field`, if they've set one it can take.
    pub fn get(&self, field: &ModField) -> Option<FieldValue> {
        let value = FieldValue::deserialize(self.0.get(field.id)?.clone()).ok()?;
        field.check(value).ok().flatten()
    }

    /// Sets the user's value for field `id`, or with `None` (or an empty
    /// value) goes back to the source's.
    pub fn set(&mut self, id: &str, value: Option<FieldValue>) -> Result<()> {
        let field = mod_field(id)
            .ok_or_else(|| Error::invalid(format!("mods have no field `{id}` to change")))?;
        match value.map(|v| field.check(v)).transpose()?.flatten() {
            Some(value) => {
                let value =
                    toml::Value::try_from(value).map_err(|e| Error::invalid(e.to_string()))?;
                self.0.insert(id.to_owned(), value);
            }
            None => {
                self.0.remove(id);
            }
        }
        Ok(())
    }

    /// Each of [`MOD_FIELDS`], with what `summary` (as fetched from `m`'s
    /// source) says for it and what it's been changed to. Without a
    /// summary, the mod's name in the profile stands in for its source's.
    pub fn fields(&self, m: &ProfileMod, summary: Option<&ModSummary>) -> Vec<FieldInfo> {
        let bare;
        let summary = match summary {
            Some(s) => s,
            None => {
                bare = bare_summary(m);
                &bare
            }
        };
        MOD_FIELDS
            .iter()
            .map(|f| FieldInfo {
                id: f.id,
                label: f.label,
                help: f.help,
                kind: f.kind,
                shows: f.shows(),
                source: f.from_source(summary),
                edited: self.get(f),
            })
            .collect()
    }

    /// Puts the changes on `m`'s summary, as fetched from its source. A mod
    /// with no summary gets one if it has changes that show, starting from
    /// its name in the profile.
    pub fn apply(&self, m: &ProfileMod, summary: Option<ModSummary>) -> Option<ModSummary> {
        let changes: Vec<_> = MOD_FIELDS
            .iter()
            .filter_map(|f| Some((f.apply?, self.get(f)?)))
            .collect();
        if changes.is_empty() {
            return summary;
        }
        let mut summary = summary.unwrap_or_else(|| bare_summary(m));
        for (apply, value) in changes {
            apply(&mut summary, value);
        }
        Some(summary)
    }
}

/// A summary of `m` with nothing but its name in the profile, for a mod
/// whose source has nothing to show for it.
fn bare_summary(m: &ProfileMod) -> ModSummary {
    ModSummary {
        source: m.source.clone(),
        id: m.id.clone(),
        name: m.name.clone(),
        file: None,
        url: None,
        author: None,
        author_url: None,
        summary: None,
        icon_url: None,
        downloads: None,
        rating: None,
        updated: None,
        latest_version: None,
        categories: Vec::new(),
    }
}

#[cfg(test)]
mod tests;
