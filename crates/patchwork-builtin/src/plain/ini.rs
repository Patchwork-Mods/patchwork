//! INI files: `[Section]` lines and `key = value` settings, with `;` or
//! `#` comments. Sections and keys are matched ignoring case, as Windows'
//! own INI functions (which most mods use) match them.
//!
//! Besides guessing from values, keys named the Bethesda way say their
//! type: `bEnabled` is a toggle (written `1` or `0` if it was), `iCount` a
//! whole number, `fScale` a number and `sName` text.

use std::ops::Range;

use patchwork_core::config::{ConfigContext, ConfigFormat, Contents};
use patchwork_core::options::{Answer, Field, FieldKind, GroupKind, Value};
use patchwork_core::{Error, Result};

use super::{Notes, field, float, float_text, group, guess, int};

/// Reads and writes INI files, as the `generic.ini` config format.
pub struct IniConfig;

/// How a setting's value is written, so changing it keeps the style.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Style {
    /// As it is.
    Plain,
    /// In double quotes.
    Quoted,
    /// A toggle as `1` or `0`.
    Digit,
    /// A toggle as `true` or `false`, in this case: lower, Capital, UPPER.
    Word(Case),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Case {
    Lower,
    Capital,
    Upper,
}

#[derive(Debug)]
struct Entry {
    line: usize,
    key: String,
    /// Where the value is in its line, without quotes' or comments.
    value: Range<usize>,
    raw: String,
    comments: Vec<String>,
}

#[derive(Debug, Default)]
struct Section {
    name: String,
    line: Option<usize>,
    entries: Vec<Entry>,
}

struct Parsed<'a> {
    bom: bool,
    lines: Vec<&'a str>,
    newline: &'static str,
    /// What goes between a key and its value in the file, for new ones.
    equals: &'static str,
    sections: Vec<Section>,
}

fn parse(file: &[u8]) -> Result<Parsed<'_>> {
    let text = std::str::from_utf8(file).map_err(|_| Error::invalid("it isn't text (UTF-8)"))?;
    let (bom, text) = match text.strip_prefix('\u{feff}') {
        Some(text) => (true, text),
        None => (false, text),
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut sections = vec![Section::default()];
    let mut comments: Vec<String> = Vec::new();
    let mut spaced = 0;
    let mut tight = 0;
    for (at, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            // Comments a blank line away from a setting are about
            // something else, like the whole file.
            comments.clear();
        } else if let Some(comment) = trimmed
            .strip_prefix(';')
            .or_else(|| trimmed.strip_prefix('#'))
        {
            comments.push(comment.trim_start_matches([';', '#']).trim().to_owned());
        } else if let Some(name) = trimmed.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push(Section {
                name: name.trim().to_owned(),
                line: Some(at),
                entries: Vec::new(),
            });
            comments.clear();
        } else if let Some(eq) = line.find('=') {
            let key = line[..eq].trim();
            if line[..eq].ends_with(' ') {
                spaced += 1;
            } else {
                tight += 1;
            }
            let value = value_span(line, eq + 1);
            let raw = line[value.clone()].to_owned();
            sections
                .last_mut()
                .expect("never empty")
                .entries
                .push(Entry {
                    line: at,
                    key: key.to_owned(),
                    value,
                    raw,
                    comments: std::mem::take(&mut comments),
                });
        }
    }
    Ok(Parsed {
        bom,
        newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
        lines,
        equals: if spaced >= tight { " = " } else { "=" },
        sections,
    })
}

/// Where the value is in `line`, starting the search at `from`: trimmed,
/// and without a comment after it (a `;` or `#` after a space, outside
/// quotes).
fn value_span(line: &str, from: usize) -> Range<usize> {
    let rest = &line[from..];
    let mut end = rest.len();
    let mut quoted = false;
    let mut prev = ' ';
    for (i, c) in rest.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ';' | '#' if !quoted && prev.is_whitespace() && i > 0 => {
                end = i;
                break;
            }
            _ => {}
        }
        prev = c;
    }
    let value = &rest[..end];
    let start = from + (value.len() - value.trim_start().len());
    let end = from + value.trim_end().len();
    start..end.max(start)
}

/// What `entry` is, from its key's prefix (the Bethesda way) and its
/// value, and how it's written.
fn typed(entry: &Entry) -> (FieldKind, Value, Style) {
    let raw = entry.raw.as_str();
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return (
            FieldKind::Text { multiline: false },
            Value::Text(raw[1..raw.len() - 1].to_owned()),
            Style::Quoted,
        );
    }
    let mut chars = entry.key.chars();
    let prefix = match (chars.next(), chars.next()) {
        (Some(p), Some(next)) if next.is_ascii_uppercase() => Some(p),
        _ => None,
    };
    match prefix {
        Some('b') if raw == "0" || raw == "1" => {
            return (FieldKind::Toggle, Value::Bool(raw == "1"), Style::Digit);
        }
        Some('i' | 'u') => {
            if let Ok(n) = raw.parse::<i64>() {
                return (int(), Value::Int(n), Style::Plain);
            }
        }
        Some('f') => {
            if let Some(n) = super::number(raw) {
                return (float(), Value::Float(n), Style::Plain);
            }
        }
        Some('s') => {
            return (
                FieldKind::Text { multiline: false },
                Value::Text(raw.to_owned()),
                Style::Plain,
            );
        }
        _ => {}
    }
    let (kind, value) = guess(raw);
    let style = match kind {
        FieldKind::Toggle => Style::Word(if raw.chars().all(|c| c.is_ascii_uppercase()) {
            Case::Upper
        } else if raw.starts_with(|c: char| c.is_ascii_uppercase()) {
            Case::Capital
        } else {
            Case::Lower
        }),
        _ => Style::Plain,
    };
    (kind, value, style)
}

/// `value` as a setting written in `style` is.
fn written(value: &Value, style: Style) -> String {
    match (value, style) {
        (Value::Bool(b), Style::Digit) => if *b { "1" } else { "0" }.to_owned(),
        (Value::Bool(b), Style::Word(case)) => {
            let word = b.to_string();
            match case {
                Case::Lower => word,
                Case::Upper => word.to_ascii_uppercase(),
                Case::Capital => {
                    let (first, rest) = word.split_at(1);
                    format!("{}{rest}", first.to_ascii_uppercase())
                }
            }
        }
        (Value::Bool(b), _) => b.to_string(),
        (Value::Int(n), _) => n.to_string(),
        (Value::Float(n), _) => float_text(*n),
        (Value::Text(text), Style::Quoted) => format!("\"{text}\""),
        (Value::Text(text), _) => text.clone(),
    }
}

impl ConfigFormat for IniConfig {
    fn read(&self, _ctx: &ConfigContext<'_>, file: &[u8]) -> Result<Contents> {
        let parsed = parse(file)?;
        let mut contents = Contents::default();
        let named_general = parsed
            .sections
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case("General"));
        for section in &parsed.sections {
            let mut fields: Vec<Field> = Vec::new();
            for entry in &section.entries {
                if fields.iter().any(|f| f.id.eq_ignore_ascii_case(&entry.key)) {
                    contents.warnings.push(format!(
                        "`{}` is in [{}] more than once; only the first is shown",
                        entry.key, section.name
                    ));
                    continue;
                }
                let (kind, value, _) = typed(entry);
                let notes = Notes::new(&entry.comments);
                fields.push(field(&entry.key, kind, value, &notes));
            }
            let existing = contents
                .groups
                .iter_mut()
                .find(|g| g.id.eq_ignore_ascii_case(&section.name));
            match existing {
                Some(group) => {
                    if let GroupKind::Fields { fields: have } = &mut group.kind {
                        for field in fields {
                            if !have.iter().any(|f| f.id.eq_ignore_ascii_case(&field.id)) {
                                have.push(field);
                            }
                        }
                    }
                }
                None if fields.is_empty() => {}
                None => {
                    let mut g = group(section.name.clone(), fields);
                    if section.name.is_empty() && named_general {
                        g.title = "Top of the file".to_owned();
                    }
                    contents.groups.push(g);
                }
            }
        }
        Ok(contents)
    }

    fn write(&self, _ctx: &ConfigContext<'_>, file: &[u8], values: &[Answer]) -> Result<Vec<u8>> {
        let parsed = parse(file)?;
        let mut lines: Vec<String> = parsed.lines.iter().map(|l| (*l).to_owned()).collect();
        let mut after: Vec<(usize, String)> = Vec::new();
        let mut added: Vec<(&str, Vec<String>)> = Vec::new();
        for answer in values {
            let sections: Vec<&Section> = parsed
                .sections
                .iter()
                .filter(|s| s.name.eq_ignore_ascii_case(&answer.group))
                .collect();
            for (key, value) in &answer.values {
                let entry = sections
                    .iter()
                    .flat_map(|s| &s.entries)
                    .find(|e| e.key.eq_ignore_ascii_case(key));
                if let Some(entry) = entry {
                    let (_, _, style) = typed(entry);
                    let line = &mut lines[entry.line];
                    line.replace_range(entry.value.clone(), &written(value, style));
                    continue;
                }
                let text = format!("{key}{}{}", parsed.equals, written(value, Style::Plain));
                let section = sections
                    .last()
                    .filter(|s| s.line.is_some() || !s.entries.is_empty());
                match section {
                    Some(s) => {
                        let last = s.entries.last().map(|e| e.line).or(s.line);
                        after.push((last.map_or(0, |l| l + 1), text));
                    }
                    None if answer.group.is_empty() => after.push((0, text)),
                    None => match added.iter_mut().find(|(name, _)| *name == answer.group) {
                        Some((_, settings)) => settings.push(text),
                        None => added.push((&answer.group, vec![text])),
                    },
                }
            }
        }
        // From the bottom, so earlier lines stay where they are; a stable
        // sort keeps settings added at one line in order.
        after.sort_by_key(|(line, _)| std::cmp::Reverse(*line));
        let mut at = usize::MAX;
        let mut offset = 0;
        for (line, text) in after {
            if line != at {
                at = line;
                offset = 0;
            }
            lines.insert(line + offset, text);
            offset += 1;
        }
        for (name, settings) in added {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("[{name}]"));
            lines.extend(settings);
        }
        let mut out = if parsed.bom {
            "\u{feff}".to_owned()
        } else {
            String::new()
        };
        out.push_str(&lines.join(parsed.newline));
        out.push_str(parsed.newline);
        Ok(out.into_bytes())
    }
}
