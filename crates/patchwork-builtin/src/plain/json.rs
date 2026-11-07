//! JSON files, including ones with comments and trailing commas (JSONC),
//! which many mods' configs have. The file has to be an object. Each
//! object in it that has settings is a group, with an id of the keys
//! leading to it joined by `.` (empty for the top). Lists and `null`s are
//! skipped.
//!
//! This has its own small parser, which notes where each value is, so
//! writing replaces just the values and keeps the rest of the file as it
//! is, comments included.

use std::ops::Range;

use patchwork_core::config::{ConfigContext, ConfigFormat, Contents};
use patchwork_core::options::{Answer, Field, FieldKind, Value};
use patchwork_core::{Error, Result};

use super::{Notes, field, float, float_text, group, int, skipped};

/// Reads and writes JSON files, as the `generic.json` config format.
pub struct JsonConfig;

/// How deep objects and lists can go, so a bad file can't overflow the
/// stack.
const MAX_DEPTH: usize = 128;

#[derive(Debug)]
enum Node {
    Object(Object),
    Array,
    Null,
    Bool(bool),
    /// As written.
    Number(String),
    Str(String),
}

#[derive(Debug)]
struct Object {
    /// Where its `{` is.
    open: usize,
    members: Vec<Member>,
}

#[derive(Debug)]
struct Member {
    key: String,
    /// Where the key's opening quote is.
    key_at: usize,
    comments: Vec<String>,
    node: Node,
    span: Range<usize>,
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn fail<T>(&self, what: &str) -> Result<T> {
        let line = self.text[..self.at.min(self.text.len())]
            .lines()
            .count()
            .max(1);
        Err(Error::invalid(format!(
            "it isn't valid JSON: {what} on line {line}"
        )))
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    /// Skips whitespace and comments, giving the comments' text.
    fn space(&mut self) -> Result<Vec<String>> {
        let mut comments = Vec::new();
        loop {
            let rest = &self.text[self.at..];
            let trimmed = rest.trim_start();
            // A blank line between a comment and what follows it means
            // the comment isn't about that.
            let gap = &rest[..rest.len() - trimmed.len()];
            if gap.matches('\n').count() > 1 {
                comments.clear();
            }
            self.at += gap.len();
            if let Some(body) = trimmed.strip_prefix("//") {
                let end = body.find('\n').unwrap_or(body.len());
                comments.push(body[..end].trim().to_owned());
                self.at += 2 + end;
            } else if let Some(body) = trimmed.strip_prefix("/*") {
                let Some(end) = body.find("*/") else {
                    return self.fail("a comment that doesn't end");
                };
                comments.extend(
                    body[..end]
                        .lines()
                        .map(|l| l.trim().trim_start_matches('*').trim().to_owned()),
                );
                self.at += 2 + end + 2;
            } else {
                return Ok(comments);
            }
        }
    }

    fn value(&mut self, depth: usize) -> Result<(Node, Range<usize>)> {
        if depth > MAX_DEPTH {
            return self.fail("too much nesting");
        }
        let start = self.at;
        let node = match self.peek() {
            Some(b'{') => Node::Object(self.object(depth)?),
            Some(b'[') => {
                self.at += 1;
                loop {
                    self.space()?;
                    if self.peek() == Some(b']') {
                        break;
                    }
                    self.value(depth + 1)?;
                    self.space()?;
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => break,
                        _ => return self.fail("a list without a `,` or `]`"),
                    }
                }
                self.at += 1;
                Node::Array
            }
            Some(b'"') => Node::Str(self.string()?),
            Some(b't') => self.word("true", Node::Bool(true))?,
            Some(b'f') => self.word("false", Node::Bool(false))?,
            Some(b'n') => self.word("null", Node::Null)?,
            Some(c) if c == b'-' || c.is_ascii_digit() => {
                let len = self.text[self.at..]
                    .find(|c: char| {
                        !(c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | 'e' | 'E'))
                    })
                    .unwrap_or(self.text.len() - self.at);
                let raw = &self.text[self.at..self.at + len];
                if raw.parse::<f64>().is_err() {
                    return self.fail("a number that isn't one");
                }
                self.at += len;
                Node::Number(raw.to_owned())
            }
            _ => return self.fail("something that isn't a value"),
        };
        Ok((node, start..self.at))
    }

    fn word(&mut self, word: &str, node: Node) -> Result<Node> {
        if self.text[self.at..].starts_with(word) {
            self.at += word.len();
            Ok(node)
        } else {
            self.fail("something that isn't a value")
        }
    }

    fn object(&mut self, depth: usize) -> Result<Object> {
        let open = self.at;
        self.at += 1;
        let mut members = Vec::new();
        loop {
            let comments = self.space()?;
            match self.peek() {
                Some(b'}') => break,
                Some(b'"') => {}
                _ => return self.fail("an object key that isn't in quotes"),
            }
            let key_at = self.at;
            let key = self.string()?;
            self.space()?;
            if self.peek() != Some(b':') {
                return self.fail("a key without a `:`");
            }
            self.at += 1;
            self.space()?;
            let (node, span) = self.value(depth + 1)?;
            members.push(Member {
                key,
                key_at,
                comments,
                node,
                span,
            });
            self.space()?;
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => break,
                _ => return self.fail("an object without a `,` or `}`"),
            }
        }
        self.at += 1;
        Ok(Object { open, members })
    }

    fn string(&mut self) -> Result<String> {
        self.at += 1;
        let mut out = String::new();
        let mut chars = self.text[self.at..].char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => {
                    self.at += i + 1;
                    return Ok(out);
                }
                '\\' => match chars.next().map(|(_, c)| c) {
                    Some('n') => out.push('\n'),
                    Some('r') => out.push('\r'),
                    Some('t') => out.push('\t'),
                    Some('b') => out.push('\u{8}'),
                    Some('f') => out.push('\u{c}'),
                    Some('u') => {
                        let Some(high) = hex(&mut chars) else {
                            return self.fail("a bad `\\u` escape");
                        };
                        let code = if (0xd800..0xdc00).contains(&high) {
                            let low = match (chars.next(), chars.next()) {
                                (Some((_, '\\')), Some((_, 'u'))) => hex(&mut chars),
                                _ => None,
                            };
                            match low {
                                Some(low) if (0xdc00..0xe000).contains(&low) => {
                                    0x10000 + ((high - 0xd800) << 10) + (low - 0xdc00)
                                }
                                _ => return self.fail("a bad `\\u` escape"),
                            }
                        } else {
                            high
                        };
                        out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                    }
                    Some(c) => out.push(c),
                    None => break,
                },
                c => out.push(c),
            }
        }
        self.at = self.text.len();
        self.fail("text that doesn't end")
    }
}

/// Four hex digits, as in a `\\u` escape.
fn hex(chars: &mut std::str::CharIndices<'_>) -> Option<u32> {
    let digits: String = chars.take(4).map(|(_, c)| c).collect();
    (digits.len() == 4).then(|| u32::from_str_radix(&digits, 16).ok())?
}

fn parse(file: &[u8]) -> Result<(&str, Object)> {
    let text = std::str::from_utf8(file).map_err(|_| Error::invalid("it isn't text (UTF-8)"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut parser = Parser { text, at: 0 };
    parser.space()?;
    if parser.peek() != Some(b'{') {
        return parser.fail("something other than an object at the top");
    }
    let (Node::Object(root), _) = parser.value(0)? else {
        unreachable!("starts with `{{`");
    };
    parser.space()?;
    if parser.at < text.len() {
        return parser.fail("more after the end");
    }
    // Offsets are from after any byte order mark.
    Ok((text, root))
}

/// Each object with settings in `object`, with its path, depth first.
fn objects<'a>(path: &[&'a str], object: &'a Object, out: &mut Vec<(Vec<&'a str>, &'a Object)>) {
    out.push((path.to_vec(), object));
    for member in &object.members {
        if let Node::Object(inner) = &member.node {
            let mut path = path.to_vec();
            path.push(&member.key);
            objects(&path, inner, out);
        }
    }
}

/// `text` as a JSON string.
fn quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn written(value: &Value) -> String {
    match value {
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(n) => float_text(*n),
        Value::Text(text) => quoted(text),
    }
}

impl ConfigFormat for JsonConfig {
    fn read(&self, _ctx: &ConfigContext<'_>, file: &[u8]) -> Result<Contents> {
        let (_, root) = parse(file)?;
        let mut all = Vec::new();
        objects(&[], &root, &mut all);
        let mut contents = Contents::default();
        let mut skips = 0;
        for (path, object) in all {
            let mut fields: Vec<Field> = Vec::new();
            for member in &object.members {
                let (kind, value) = match &member.node {
                    Node::Object(_) => continue,
                    Node::Array | Node::Null => {
                        skips += 1;
                        continue;
                    }
                    Node::Bool(b) => (FieldKind::Toggle, Value::Bool(*b)),
                    Node::Number(raw) => match raw.parse::<i64>() {
                        Ok(n) => (int(), Value::Int(n)),
                        Err(_) => match super::number(raw) {
                            Some(n) => (float(), Value::Float(n)),
                            None => {
                                skips += 1;
                                continue;
                            }
                        },
                    },
                    Node::Str(text) => (
                        FieldKind::Text {
                            multiline: text.contains('\n'),
                        },
                        Value::Text(text.clone()),
                    ),
                };
                if fields.iter().any(|f| f.id == member.key) {
                    continue;
                }
                fields.push(field(
                    &member.key,
                    kind,
                    value,
                    &Notes::new(&member.comments),
                ));
            }
            let id = path.join(".");
            if fields.is_empty() || contents.groups.iter().any(|g| g.id == id) {
                continue;
            }
            contents.groups.push(group(id, fields));
        }
        contents.warnings.extend(skipped(skips));
        Ok(contents)
    }

    fn write(&self, _ctx: &ConfigContext<'_>, file: &[u8], values: &[Answer]) -> Result<Vec<u8>> {
        let (text, root) = parse(file)?;
        let mut all = Vec::new();
        objects(&[], &root, &mut all);
        let find = |id: &str| all.iter().find(|(path, _)| path.join(".") == id);

        // Replacements, as (span, new text); insertions have empty spans.
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        // New members for objects, by where the object's `{` is.
        let mut new: Vec<(&Object, Vec<String>)> = Vec::new();
        for answer in values {
            let (object, missing_path): (&Object, Vec<String>) = match find(&answer.group) {
                Some((_, object)) => (object, Vec::new()),
                None => {
                    // The deepest object there is on the way to it.
                    let parts: Vec<&str> = answer.group.split('.').collect();
                    let (at, object) = (0..parts.len())
                        .rev()
                        .find_map(|n| find(&parts[..n].join(".")).map(|(_, o)| (n, *o)))
                        .expect("the top is always there");
                    (
                        object,
                        parts[at..].iter().map(|p| (*p).to_owned()).collect(),
                    )
                }
            };
            let mut members = Vec::new();
            for (key, value) in &answer.values {
                let member = missing_path
                    .is_empty()
                    .then(|| object.members.iter().find(|m| m.key == *key))
                    .flatten()
                    .filter(|m| !matches!(m.node, Node::Object(_)));
                match member {
                    Some(m) => edits.push((m.span.clone(), written(value))),
                    None => members.push(format!("{}: {}", quoted(key), written(value))),
                }
            }
            if members.is_empty() {
                continue;
            }
            let member = match missing_path.split_first() {
                None => members,
                Some((first, rest)) => {
                    let mut inner = format!("{{{}}}", members.join(", "));
                    for key in rest.iter().rev() {
                        inner = format!("{{{}: {inner}}}", quoted(key));
                    }
                    vec![format!("{}: {inner}", quoted(first))]
                }
            };
            match new.iter_mut().find(|(o, _)| o.open == object.open) {
                Some((_, have)) => have.extend(member),
                None => new.push((object, member)),
            }
        }
        for (object, members) in new {
            match object.members.last() {
                Some(last) => {
                    // Laid out like the last member: on a line of its own
                    // with the same indent, or after it on the same line.
                    let before = &text[..last.key_at];
                    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
                    let indent = &before[line_start..];
                    let sep = if indent.trim().is_empty() && line_start > object.open {
                        let newline = if before.ends_with(&format!("\r\n{indent}")) {
                            "\r\n"
                        } else {
                            "\n"
                        };
                        format!(",{newline}{indent}")
                    } else {
                        ", ".to_owned()
                    };
                    let added: String = members.iter().map(|m| format!("{sep}{m}")).collect();
                    edits.push((last.span.end..last.span.end, added));
                }
                None => {
                    let at = object.open + 1;
                    edits.push((at..at, members.join(", ")));
                }
            }
        }
        edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        let mut out = text.to_owned();
        for (span, new) in edits {
            out.replace_range(span, &new);
        }
        if file.starts_with("\u{feff}".as_bytes()) {
            out.insert(0, '\u{feff}');
        }
        Ok(out.into_bytes())
    }
}
