//! Config files in plain INI, JSON and TOML (the `generic.ini`,
//! `generic.json` and `generic.toml` config formats), which don't say what
//! type each setting is. Types are guessed from the values: `true` and
//! `false` are toggles, whole numbers and other numbers are numbers, and
//! the rest is text.
//!
//! Comments just above a setting are its description. A few lines in them
//! are read as hints, as Forge's and NeoForge's TOML configs write them:
//!
//! ```text
//! # How far to look.
//! # Default: 8
//! # Range: 1 ~ 32
//! # Allowed Values: NEAR, FAR
//! ```
//!
//! Writing changes only the values, so comments and layout are kept.

mod ini;
mod json;
mod toml_file;

pub use ini::IniConfig;
pub use json::JsonConfig;
pub use toml_file::TomlConfig;

use patchwork_core::options::{Choice, Field, FieldKind, Group, GroupKind, Value};

/// What comment lines above a setting say.
#[derive(Debug, Default, Clone)]
struct Notes {
    description: Option<String>,
    /// From a `Range: a ~ b` (or `> a`, `< b`) line, as written.
    range: (Option<String>, Option<String>),
    /// From an `Allowed Values: a, b` line.
    choices: Option<Vec<String>>,
    /// From a `Default: a` line.
    default: Option<String>,
}

impl Notes {
    /// Comment lines, with what marks them as comments taken off.
    fn new<S: AsRef<str>>(lines: &[S]) -> Self {
        let mut notes = Notes::default();
        let mut description = Vec::new();
        for line in lines {
            let line = line.as_ref().trim();
            if let Some(range) = hint(line, "range:") {
                notes.range = if let Some((min, max)) = range.split_once('~') {
                    (Some(min.trim().to_owned()), Some(max.trim().to_owned()))
                } else if let Some(min) = range.strip_prefix('>') {
                    (Some(min.trim_start_matches('=').trim().to_owned()), None)
                } else if let Some(max) = range.strip_prefix('<') {
                    (None, Some(max.trim_start_matches('=').trim().to_owned()))
                } else {
                    (None, None)
                };
            } else if let Some(choices) = hint(line, "allowed values:") {
                notes.choices = Some(
                    choices
                        .split(',')
                        .map(|c| c.trim().to_owned())
                        .filter(|c| !c.is_empty())
                        .collect(),
                );
            } else if let Some(default) = hint(line, "default:") {
                notes.default = Some(default.to_owned());
            } else {
                description.push(line);
            }
        }
        // Blank comment lines at either end are only spacing.
        while description.first().is_some_and(|l| l.is_empty()) {
            description.remove(0);
        }
        while description.last().is_some_and(|l| l.is_empty()) {
            description.pop();
        }
        if !description.is_empty() {
            notes.description = Some(description.join("\n"));
        }
        notes
    }
}

/// What follows `name` (lowercase, with its `:`) at the start of `line`,
/// ignoring case.
fn hint<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let head = line.get(..name.len())?;
    head.eq_ignore_ascii_case(name)
        .then(|| line[name.len()..].trim())
}

/// A field for a setting of `kind` set to `value`, with what `notes` say.
fn field(id: &str, kind: FieldKind, value: Value, notes: &Notes) -> Field {
    let kind = match kind {
        FieldKind::Int { step, .. } => {
            let bound = |b: &Option<String>| b.as_deref().and_then(|b| b.parse::<i64>().ok());
            FieldKind::Int {
                min: bound(&notes.range.0),
                max: bound(&notes.range.1),
                step,
            }
        }
        FieldKind::Float { step, .. } => {
            let bound = |b: &Option<String>| b.as_deref().and_then(number);
            FieldKind::Float {
                min: bound(&notes.range.0),
                max: bound(&notes.range.1),
                step,
            }
        }
        FieldKind::Text { multiline } => match (&notes.choices, &value) {
            (Some(choices), Value::Text(text)) if choices.iter().any(|c| c == text) => {
                FieldKind::Choice {
                    choices: choices
                        .iter()
                        .map(|c| Choice {
                            id: c.clone(),
                            title: c.clone(),
                        })
                        .collect(),
                }
            }
            _ => FieldKind::Text { multiline },
        },
        kind => kind,
    };
    let default = notes
        .default
        .as_deref()
        .and_then(|raw| value_as(&kind, unquoted(raw)));
    let field = Field {
        id: id.to_owned(),
        title: id.to_owned(),
        description: notes.description.clone(),
        kind,
        value: value.clone(),
        default,
    };
    // A value outside the hinted range would make the field unsavable;
    // the hints are only guesses, so they give way.
    match field.check(&value) {
        Ok(_) => field,
        Err(_) => Field {
            kind: match field.kind {
                FieldKind::Int { step, .. } => FieldKind::Int {
                    min: None,
                    max: None,
                    step,
                },
                FieldKind::Float { step, .. } => FieldKind::Float {
                    min: None,
                    max: None,
                    step,
                },
                kind => kind,
            },
            ..field
        },
    }
}

/// `raw` as a setting of `kind` would hold it.
fn value_as(kind: &FieldKind, raw: &str) -> Option<Value> {
    match kind {
        FieldKind::Toggle => match raw.to_ascii_lowercase().as_str() {
            "true" | "1" => Some(Value::Bool(true)),
            "false" | "0" => Some(Value::Bool(false)),
            _ => None,
        },
        FieldKind::Int { .. } => raw.parse().ok().map(Value::Int),
        FieldKind::Float { .. } => number(raw).map(Value::Float),
        FieldKind::Choice { choices } => choices
            .iter()
            .any(|c| c.id == raw)
            .then(|| Value::Text(raw.to_owned())),
        FieldKind::Text { .. } | FieldKind::Color { .. } => Some(Value::Text(raw.to_owned())),
    }
}

/// `raw` without one pair of double quotes around it.
fn unquoted(raw: &str) -> &str {
    raw.strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(raw)
}

/// A finite number, as config files write them.
fn number(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// What a value written as `raw` (unquoted) most likely is.
fn guess(raw: &str) -> (FieldKind, Value) {
    if raw.eq_ignore_ascii_case("true") || raw.eq_ignore_ascii_case("false") {
        return (
            FieldKind::Toggle,
            Value::Bool(raw.eq_ignore_ascii_case("true")),
        );
    }
    if let Ok(n) = raw.parse::<i64>() {
        return (int(), Value::Int(n));
    }
    if raw.contains(['.', 'e', 'E'])
        && raw.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
        && let Some(n) = number(raw)
    {
        return (float(), Value::Float(n));
    }
    (
        FieldKind::Text { multiline: false },
        Value::Text(raw.to_owned()),
    )
}

fn int() -> FieldKind {
    FieldKind::Int {
        min: None,
        max: None,
        step: None,
    }
}

fn float() -> FieldKind {
    FieldKind::Float {
        min: None,
        max: None,
        step: None,
    }
}

/// A number as text that still reads as one with a fractional part, so
/// that reading it back guesses the same type: `1.0`, not `1`.
fn float_text(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.1}")
    } else {
        n.to_string()
    }
}

/// A group of `fields`, titled by its id, or "General" for the settings
/// outside any section or table (whose id is empty).
fn group(id: String, fields: Vec<Field>) -> Group {
    Group {
        title: if id.is_empty() {
            "General".to_owned()
        } else {
            id.clone()
        },
        id,
        kind: GroupKind::Fields { fields },
        defaulted: true,
    }
}

/// A warning about `count` settings a format skips: lists, tables
/// inside settings, and settings with no value.
fn skipped(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some(
            "1 setting isn't shown, since it's a list or has no value, which can't be \
             changed here yet"
                .to_owned(),
        ),
        n => Some(format!(
            "{n} settings aren't shown, since they're lists or have no value, which can't \
             be changed here yet"
        )),
    }
}

#[cfg(test)]
mod tests;
