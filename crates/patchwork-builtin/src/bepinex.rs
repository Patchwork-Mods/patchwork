//! BepInEx's `.cfg` config files, which BepInEx writes for each plugin the
//! first time it runs, with each setting's type, default, description and
//! allowed values in the comments above it:
//!
//! ```text
//! ## Settings file was created by plugin Example v1.0.0
//! ## Plugin GUID: com.example.plugin
//!
//! [General]
//!
//! ## How fast to go.
//! # Setting type: Single
//! # Default value: 1
//! # Acceptable value range: From 0.5 to 4
//! Speed = 1
//! ```
//!
//! Sections are groups and keys are fields, by the names the file gives
//! them. Settings without a type BepInEx knows of (or whose value doesn't
//! fit it) are text.

use patchwork_core::config::{ConfigContext, ConfigFormat, Contents};
use patchwork_core::options::{Answer, Choice, Field, FieldKind, Group, GroupKind, Value};
use patchwork_core::{Error, Result};

/// Reads and writes BepInEx `.cfg` files, as the `bepinex.cfg` config
/// format.
pub struct BepInExConfig;

/// One `key = value` line, and what the comments above it said.
#[derive(Debug, Default)]
struct Entry {
    /// Which line of the file it's on.
    line: usize,
    key: String,
    raw: String,
    description: Vec<String>,
    kind: Option<String>,
    default: Option<String>,
    range: Option<(String, String)>,
    choices: Option<Vec<String>>,
    /// The value is a list of choices, separated by `, `.
    flags: bool,
}

#[derive(Debug, Default)]
struct Section {
    name: String,
    /// Where its `[name]` line is, or `None` for settings before any.
    line: Option<usize>,
    entries: Vec<Entry>,
}

#[derive(Debug, Default)]
struct Parsed<'a> {
    lines: Vec<&'a str>,
    newline: &'static str,
    /// The plugin's name, from the header.
    title: Option<String>,
    sections: Vec<Section>,
}

fn parse(file: &[u8]) -> Result<Parsed<'_>> {
    let text = std::str::from_utf8(file).map_err(|_| Error::invalid("it isn't text (UTF-8)"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut parsed = Parsed {
        lines: text.lines().collect(),
        newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
        ..Default::default()
    };
    let mut pending = Entry::default();
    for (at, line) in parsed.lines.iter().enumerate() {
        let line = line.trim();
        if let Some(comment) = line.strip_prefix("##") {
            let comment = comment.strip_prefix(' ').unwrap_or(comment);
            if parsed.sections.is_empty()
                && let Some(rest) = comment.strip_prefix("Settings file was created by plugin ")
            {
                // "Name v1.0.0"; the name can have spaces.
                let name = rest.rsplit_once(" v").map_or(rest, |(name, _)| name);
                parsed.title = Some(name.trim().to_owned());
            } else if !parsed.sections.is_empty() {
                pending.description.push(comment.to_owned());
            }
        } else if let Some(comment) = line.strip_prefix('#') {
            let comment = comment.trim();
            if let Some(kind) = comment.strip_prefix("Setting type:") {
                pending.kind = Some(kind.trim().to_owned());
            } else if let Some(default) = comment.strip_prefix("Default value:") {
                pending.default = Some(default.trim().to_owned());
            } else if let Some(range) = comment.strip_prefix("Acceptable value range:") {
                pending.range = range
                    .trim()
                    .strip_prefix("From ")
                    .and_then(|r| r.split_once(" to "))
                    .map(|(min, max)| (min.trim().to_owned(), max.trim().to_owned()));
            } else if let Some(choices) = comment.strip_prefix("Acceptable values:") {
                pending.choices = Some(choices.split(',').map(|c| c.trim().to_owned()).collect());
            } else if comment.starts_with("Multiple values can be set at the same time") {
                pending.flags = true;
            }
        } else if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            parsed.sections.push(Section {
                name: name.trim().to_owned(),
                line: Some(at),
                entries: Vec::new(),
            });
            pending = Entry::default();
        } else if let Some((key, value)) = line.split_once('=') {
            if parsed.sections.is_empty() {
                parsed.sections.push(Section::default());
            }
            let entry = Entry {
                line: at,
                key: key.trim().to_owned(),
                raw: value.trim().to_owned(),
                ..std::mem::take(&mut pending)
            };
            let section = parsed.sections.last_mut().expect("pushed above");
            section.entries.push(entry);
        } else if line.is_empty() {
            // A blank line between a description and its setting is fine;
            // BepInEx doesn't write one, but people editing by hand might.
        }
    }
    Ok(parsed)
}

/// What `entry` is, as a field kind, and its value and default as that
/// kind would have them. `None` if it doesn't fit, so it's shown as text.
fn typed(entry: &Entry) -> Option<(FieldKind, Value, Option<Value>)> {
    let kind = entry.kind.as_deref().unwrap_or("");
    let parse = |raw: &str| value_of(kind, entry, raw);
    let field = match kind {
        "Boolean" => FieldKind::Toggle,
        "SByte" | "Byte" | "Int16" | "UInt16" | "Int32" | "UInt32" | "Int64" | "UInt64" => {
            let (min, max) = match &entry.range {
                Some((min, max)) => (min.parse().ok(), max.parse().ok()),
                None => (None, None),
            };
            FieldKind::Int {
                min,
                max,
                step: None,
            }
        }
        "Single" | "Double" | "Decimal" => {
            let (min, max) = match &entry.range {
                Some((min, max)) => (number(min), number(max)),
                None => (None, None),
            };
            FieldKind::Float {
                min,
                max,
                step: None,
            }
        }
        "Color" => FieldKind::Color { alpha: true },
        _ => match &entry.choices {
            Some(choices) if !entry.flags => FieldKind::Choice {
                choices: choices
                    .iter()
                    .map(|c| Choice {
                        id: c.clone(),
                        title: c.clone(),
                    })
                    .collect(),
            },
            _ => FieldKind::Text { multiline: false },
        },
    };
    let value = parse(&entry.raw)?;
    let default = entry.default.as_deref().and_then(parse);
    let field = match (field, &value) {
        (FieldKind::Text { .. }, Value::Text(text)) => FieldKind::Text {
            multiline: text.contains('\n'),
        },
        (field, _) => field,
    };
    Some((field, value, default))
}

/// `raw` as a setting of .NET type `kind` holds it.
fn value_of(kind: &str, entry: &Entry, raw: &str) -> Option<Value> {
    Some(match kind {
        "Boolean" => Value::Bool(match raw.to_ascii_lowercase().as_str() {
            "true" => true,
            "false" => false,
            _ => return None,
        }),
        "SByte" | "Byte" | "Int16" | "UInt16" | "Int32" | "UInt32" | "Int64" | "UInt64" => {
            Value::Int(raw.parse().ok()?)
        }
        "Single" | "Double" | "Decimal" => Value::Float(number(raw)?),
        "Color" => {
            let hex = raw.trim_start_matches('#');
            let ok =
                (hex.len() == 6 || hex.len() == 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
            if !ok {
                return None;
            }
            let hex = if hex.len() == 6 {
                format!("{hex}ff")
            } else {
                hex.to_owned()
            };
            Value::Text(format!("#{}", hex.to_ascii_lowercase()))
        }
        "String" => Value::Text(unescape(raw)),
        _ => {
            if let Some(choices) = &entry.choices
                && !entry.flags
                && !choices.iter().any(|c| c == raw)
            {
                return None;
            }
            Value::Text(raw.to_owned())
        }
    })
}

/// A number as .NET writes one, which is always with a `.`.
fn number(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|n| n.is_finite())
}

/// `value` as BepInEx writes a setting of .NET type `kind`.
fn written(kind: &str, value: &Value) -> String {
    match (kind, value) {
        ("Color", Value::Text(hex)) => hex.trim_start_matches('#').to_ascii_uppercase(),
        ("String", Value::Text(text)) => escape(text),
        (_, Value::Text(text)) => text.clone(),
        (_, Value::Bool(b)) => b.to_string(),
        (_, Value::Int(n)) => n.to_string(),
        (_, Value::Float(n)) => n.to_string(),
    }
}

/// Like BepInEx's `TomlTypeConverter` escapes strings.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// Like BepInEx's `TomlTypeConverter` unescapes strings, which it doesn't
/// for what looks like a Windows path that was never escaped (`C:\Games`).
fn unescape(raw: &str) -> String {
    let unquoted = raw.strip_prefix('"').unwrap_or(raw);
    let bytes = unquoted.as_bytes();
    let path = bytes.len() >= 3
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1] == b':'
        && bytes[2] == b'\\'
        && bytes.get(3) != Some(&b'\\')
        && !unquoted[3..].contains("\\\\");
    if path {
        return raw.to_owned();
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('0') => out.push('\0'),
            Some(c) => out.push(c),
            None => out.push('\\'),
        }
    }
    out
}

impl ConfigFormat for BepInExConfig {
    fn read(&self, _ctx: &ConfigContext<'_>, file: &[u8]) -> Result<Contents> {
        let parsed = parse(file)?;
        let mut contents = Contents {
            title: parsed.title,
            ..Default::default()
        };
        for section in parsed.sections {
            let mut fields: Vec<Field> = Vec::new();
            for entry in &section.entries {
                if fields.iter().any(|f| f.id == entry.key) {
                    contents.warnings.push(format!(
                        "`{}` is in [{}] more than once; only the first is shown",
                        entry.key, section.name
                    ));
                    continue;
                }
                let description =
                    (!entry.description.is_empty()).then(|| entry.description.join("\n"));
                let (kind, value, default) = typed(entry).unwrap_or_else(|| {
                    let text = Value::Text(entry.raw.clone());
                    let default = entry.default.clone().map(Value::Text);
                    (FieldKind::Text { multiline: false }, text, default)
                });
                fields.push(Field {
                    id: entry.key.clone(),
                    title: entry.key.clone(),
                    description,
                    kind,
                    value,
                    default,
                });
            }
            match contents.groups.iter_mut().find(|g| g.id == section.name) {
                Some(group) => {
                    if let GroupKind::Fields { fields: have } = &mut group.kind {
                        have.extend(fields);
                    }
                }
                None => contents.groups.push(Group {
                    id: section.name.clone(),
                    title: section.name.clone(),
                    kind: GroupKind::Fields { fields },
                    defaulted: true,
                }),
            }
        }
        Ok(contents)
    }

    fn write(&self, _ctx: &ConfigContext<'_>, file: &[u8], values: &[Answer]) -> Result<Vec<u8>> {
        let parsed = parse(file)?;
        let mut lines: Vec<String> = parsed.lines.iter().map(|l| (*l).to_owned()).collect();
        // Lines to add after a line, for settings the file hasn't got, and
        // sections it hasn't got with their settings.
        let mut after: Vec<(usize, String)> = Vec::new();
        let mut added: Vec<(&str, Vec<String>)> = Vec::new();
        for answer in values {
            let section = parsed.sections.iter().find(|s| s.name == answer.group);
            for (key, value) in &answer.values {
                let entry = section.and_then(|s| s.entries.iter().find(|e| e.key == *key));
                if let Some(entry) = entry {
                    // Keeps the key as the file spells it, and what's
                    // around the `=`.
                    let line = &parsed.lines[entry.line];
                    let (left, _) = line.split_once('=').expect("parsed as a setting");
                    let kind = entry.kind.as_deref().unwrap_or("");
                    lines[entry.line] = format!("{left}= {}", written(kind, value));
                    continue;
                }
                let text = format!("{key} = {}", written("", value));
                match section {
                    Some(s) => {
                        let last = s.entries.last().map(|e| e.line).or(s.line).unwrap_or(0);
                        after.push((last, text));
                    }
                    None => match added.iter_mut().find(|(name, _)| *name == answer.group) {
                        Some((_, settings)) => settings.push(text),
                        None => added.push((&answer.group, vec![text])),
                    },
                }
            }
        }
        // From the bottom, so earlier lines stay where they are; a stable
        // sort keeps settings added after one line in order.
        after.sort_by_key(|(line, _)| std::cmp::Reverse(*line));
        let mut at = usize::MAX;
        let mut offset = 0;
        for (line, text) in after {
            if line != at {
                at = line;
                offset = 0;
            }
            offset += 1;
            lines.insert(line + offset, text);
        }
        for (name, settings) in added {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{name}]"));
            lines.push(String::new());
            lines.extend(settings);
        }
        let mut out = lines.join(parsed.newline);
        out.push_str(parsed.newline);
        Ok(out.into_bytes())
    }
}

#[cfg(test)]
mod tests;
