//! TOML files. Each table with settings is a group, with an id of its
//! name (`client.graphics`; empty for the settings before any table).
//! Lists, inline tables, arrays of tables and dates are skipped.
//!
//! Forge's and NeoForge's configs are TOML with `Range:`, `Allowed
//! Values:` and `Default:` lines in their comments, which become each
//! field's limits, choices and default.

use patchwork_core::config::{ConfigContext, ConfigFormat, Contents};
use patchwork_core::options::{Answer, Field, FieldKind, Value};
use patchwork_core::{Error, Result};
use toml_edit::{DocumentMut, Item, Table};

use super::{Notes, field, float, group, int, skipped};

/// Reads and writes TOML files, as the `generic.toml` config format.
pub struct TomlConfig;

fn parse(file: &[u8]) -> Result<DocumentMut> {
    let text = std::str::from_utf8(file).map_err(|_| Error::invalid("it isn't text (UTF-8)"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    text.parse()
        .map_err(|e: toml_edit::TomlError| Error::invalid(format!("it isn't valid TOML: {e}")))
}

/// The comment lines just above `key` in `table`, without their `#`s.
fn comments(table: &Table, key: &str) -> Vec<String> {
    let Some(prefix) = table
        .key(key)
        .and_then(|k| k.leaf_decor().prefix())
        .and_then(|p| p.as_str())
    else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    // What's after the last newline is the indent on the key's own line.
    let whole = prefix.rsplit_once('\n').map_or("", |(lines, _)| lines);
    for line in whole.lines() {
        let line = line.trim();
        match line.strip_prefix('#') {
            Some(comment) => lines.push(comment.trim_start_matches('#').trim().to_owned()),
            // A blank line means the comments above it are about
            // something else.
            None if line.is_empty() => lines.clear(),
            None => {}
        }
    }
    lines
}

/// Each table in `table` with its path, depth first.
fn tables<'a>(path: &[&'a str], table: &'a Table, out: &mut Vec<(Vec<&'a str>, &'a Table)>) {
    out.push((path.to_vec(), table));
    for (key, item) in table.iter() {
        if let Item::Table(inner) = item {
            let mut path = path.to_vec();
            path.push(key);
            tables(&path, inner, out);
        }
    }
}

fn table_mut<'a>(mut table: &'a mut Table, path: &[String]) -> Option<&'a mut Table> {
    for key in path {
        table = table.get_mut(key)?.as_table_mut()?;
    }
    Some(table)
}

/// The table at `path`, made (with any above it) if it isn't there.
fn table_made<'a>(mut table: &'a mut Table, path: &[String]) -> Result<&'a mut Table> {
    for key in path {
        let item = table
            .entry(key)
            .or_insert_with(|| Item::Table(Table::new()));
        table = item
            .as_table_mut()
            .ok_or_else(|| Error::invalid(format!("`{key}` is a setting, not a table")))?;
    }
    Ok(table)
}

fn value_of(value: &Value) -> toml_edit::Value {
    match value {
        Value::Bool(b) => (*b).into(),
        Value::Int(n) => (*n).into(),
        Value::Float(n) => (*n).into(),
        Value::Text(text) => text.as_str().into(),
    }
}

impl ConfigFormat for TomlConfig {
    fn read(&self, _ctx: &ConfigContext<'_>, file: &[u8]) -> Result<Contents> {
        let doc = parse(file)?;
        let mut all = Vec::new();
        tables(&[], doc.as_table(), &mut all);
        let mut contents = Contents::default();
        let mut skips = 0;
        for (path, table) in all {
            let mut fields: Vec<Field> = Vec::new();
            for (key, item) in table.iter() {
                let (kind, value) = match item {
                    Item::Table(_) | Item::None => continue,
                    Item::ArrayOfTables(_) => {
                        skips += 1;
                        continue;
                    }
                    Item::Value(value) => match value {
                        toml_edit::Value::Boolean(b) => {
                            (FieldKind::Toggle, Value::Bool(*b.value()))
                        }
                        toml_edit::Value::Integer(n) => (int(), Value::Int(*n.value())),
                        toml_edit::Value::Float(n) if n.value().is_finite() => {
                            (float(), Value::Float(*n.value()))
                        }
                        toml_edit::Value::String(s) => (
                            FieldKind::Text {
                                multiline: s.value().contains('\n'),
                            },
                            Value::Text(s.value().clone()),
                        ),
                        _ => {
                            skips += 1;
                            continue;
                        }
                    },
                };
                fields.push(field(key, kind, value, &Notes::new(&comments(table, key))));
            }
            if !fields.is_empty() {
                contents.groups.push(group(path.join("."), fields));
            }
        }
        contents.warnings.extend(skipped(skips));
        Ok(contents)
    }

    fn write(&self, _ctx: &ConfigContext<'_>, file: &[u8], values: &[Answer]) -> Result<Vec<u8>> {
        let mut doc = parse(file)?;
        let paths: Vec<(String, Vec<String>)> = {
            let mut all = Vec::new();
            tables(&[], doc.as_table(), &mut all);
            all.into_iter()
                .map(|(path, _)| {
                    let id = path.join(".");
                    (id, path.into_iter().map(str::to_owned).collect())
                })
                .collect()
        };
        for answer in values {
            let known = paths.iter().find(|(id, _)| *id == answer.group);
            let table = match known {
                Some((_, path)) => table_mut(doc.as_table_mut(), path).expect("found above"),
                None => {
                    let path: Vec<String> = answer.group.split('.').map(str::to_owned).collect();
                    table_made(doc.as_table_mut(), &path)?
                }
            };
            for (key, value) in &answer.values {
                let new = value_of(value);
                match table.get_mut(key).and_then(Item::as_value_mut) {
                    Some(old) => {
                        // Keeps the spacing and any comment after it.
                        let decor = old.decor().clone();
                        *old = new;
                        *old.decor_mut() = decor;
                    }
                    None => {
                        table.insert(key, Item::Value(new));
                    }
                }
            }
        }
        let mut out = doc.to_string();
        if file.starts_with("\u{feff}".as_bytes()) {
            out.insert(0, '\u{feff}');
        }
        Ok(out.into_bytes())
    }
}
