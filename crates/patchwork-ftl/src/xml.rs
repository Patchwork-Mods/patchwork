//! A forgiving XML parser, for FTL's files and mods' patches to them.
//!
//! FTL reads its XML with RapidXML, which lets a lot through: files with
//! several root elements, `--` inside comments, stray `&`s and the like,
//! and mods are written against what the game accepts rather than what
//! the XML spec does. So this parser never fails: it keeps text, attribute
//! values and comments exactly as written (entities and all), closes
//! elements left open at the end, and ignores closing tags that don't
//! match anything open. Writing a parsed document back out gives the same
//! text, apart from the spacing inside tags.

use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    /// Text as written, entities and all.
    Text(String),
    /// What's between `<!--` and `-->`.
    Comment(String),
    /// What's between `<![CDATA[` and `]]>`.
    CData(String),
    /// Declarations, processing instructions and doctypes, whole.
    Other(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    /// Values as written, entities and all.
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Element {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }

    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_attr(&mut self, name: &str, value: &str) {
        match self.attrs.iter_mut().find(|(n, _)| n == name) {
            Some((_, v)) => value.clone_into(v),
            None => self.attrs.push((name.to_owned(), value.to_owned())),
        }
    }

    pub fn remove_attr(&mut self, name: &str) {
        self.attrs.retain(|(n, _)| n != name);
    }

    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    /// The element's own text (not its children's), with entities as
    /// written.
    pub fn text(&self) -> String {
        let mut text = String::new();
        for child in &self.children {
            match child {
                Node::Text(t) | Node::CData(t) => text.push_str(t),
                _ => {}
            }
        }
        text
    }
}

/// Parses `text` into its top-level nodes.
pub fn parse(text: &str) -> Vec<Node> {
    Parser { s: text, pos: 0 }.parse()
}

struct Parser<'a> {
    s: &'a str,
    pos: usize,
}

fn is_name_end(c: char) -> bool {
    c.is_whitespace() || matches!(c, '/' | '>' | '=' | '<' | '?')
}

impl<'a> Parser<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.pos..]
    }

    /// Everything up to `end` (exclusive), moving past it; or the rest of
    /// the text if `end` never comes.
    fn until(&mut self, end: &str) -> String {
        let rest = self.rest();
        let (found, skip) = match rest.find(end) {
            Some(i) => (&rest[..i], i + end.len()),
            None => (rest, rest.len()),
        };
        let found = found.to_owned();
        self.pos += skip;
        found
    }

    fn skip_whitespace(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start().len();
    }

    fn name(&mut self) -> String {
        let rest = self.rest();
        let end = rest.find(is_name_end).unwrap_or(rest.len());
        self.pos += end;
        rest[..end].to_owned()
    }

    fn parse(mut self) -> Vec<Node> {
        let mut top = Vec::new();
        let mut open: Vec<Element> = Vec::new();
        let push = |open: &mut Vec<Element>, top: &mut Vec<Node>, node: Node| match open.last_mut()
        {
            Some(parent) => parent.children.push(node),
            None => top.push(node),
        };
        let close = |open: &mut Vec<Element>, top: &mut Vec<Node>| {
            let element = open.pop().unwrap();
            push(open, top, Node::Element(element));
        };
        while self.pos < self.s.len() {
            let rest = self.rest();
            let lt = rest.find('<').unwrap_or(rest.len());
            if lt > 0 {
                let text = rest[..lt].to_owned();
                self.pos += lt;
                push(&mut open, &mut top, Node::Text(text));
                continue;
            }
            let node = if rest.starts_with("<!--") {
                self.pos += 4;
                Node::Comment(self.until("-->"))
            } else if rest.starts_with("<![CDATA[") {
                self.pos += 9;
                Node::CData(self.until("]]>"))
            } else if rest.starts_with("<?") {
                self.pos += 2;
                Node::Other(format!("<?{}?>", self.until("?>")))
            } else if rest.starts_with("<!") {
                self.pos += 2;
                Node::Other(format!("<!{}>", self.until(">")))
            } else if rest.starts_with("</") {
                self.pos += 2;
                let name = self.name();
                self.until(">");
                if let Some(i) = open.iter().rposition(|e| e.name == name) {
                    while open.len() > i {
                        close(&mut open, &mut top);
                    }
                }
                continue;
            } else if rest[1..].starts_with(|c: char| !is_name_end(c)) {
                self.pos += 1;
                let (element, closed) = self.start_tag();
                if closed {
                    Node::Element(element)
                } else {
                    open.push(element);
                    continue;
                }
            } else {
                self.pos += 1;
                Node::Text("<".to_owned())
            };
            push(&mut open, &mut top, node);
        }
        while !open.is_empty() {
            close(&mut open, &mut top);
        }
        top
    }

    /// An element's start tag, after the `<`: the element, and whether it
    /// closed itself.
    fn start_tag(&mut self) -> (Element, bool) {
        let mut element = Element::new(&self.name());
        loop {
            self.skip_whitespace();
            let rest = self.rest();
            if rest.is_empty() {
                return (element, true);
            } else if rest.starts_with("/>") {
                self.pos += 2;
                return (element, true);
            } else if rest.starts_with('>') {
                self.pos += 1;
                return (element, false);
            } else if rest.starts_with('<') {
                // A tag that never ended; the `<` starts the next one.
                return (element, false);
            }
            let name = self.name();
            if name.is_empty() {
                // Junk such as a stray `=` or `/`.
                self.pos += self.rest().chars().next().map_or(0, char::len_utf8);
                continue;
            }
            self.skip_whitespace();
            let value = if self.rest().starts_with('=') {
                self.pos += 1;
                self.skip_whitespace();
                match self.rest().chars().next() {
                    Some(q @ ('"' | '\'')) => {
                        self.pos += 1;
                        self.until(&q.to_string())
                    }
                    _ => {
                        let rest = self.rest();
                        let end = rest
                            .find(|c: char| c.is_whitespace() || c == '>')
                            .unwrap_or(rest.len());
                        let end = if rest[..end].ends_with('/') && rest[end..].starts_with('>') {
                            end - 1
                        } else {
                            end
                        };
                        self.pos += end;
                        rest[..end].to_owned()
                    }
                }
            } else {
                String::new()
            };
            element.attrs.push((name, value));
        }
    }
}

/// Writes `nodes` back out as text.
pub fn write(nodes: &[Node]) -> String {
    let mut out = String::new();
    for node in nodes {
        write_node(&mut out, node);
    }
    out
}

fn write_node(out: &mut String, node: &Node) {
    match node {
        Node::Text(t) => out.push_str(t),
        Node::Comment(c) => {
            let _ = write!(out, "<!--{c}-->");
        }
        Node::CData(c) => {
            let _ = write!(out, "<![CDATA[{c}]]>");
        }
        Node::Other(o) => out.push_str(o),
        Node::Element(e) => {
            out.push('<');
            out.push_str(&e.name);
            for (name, value) in &e.attrs {
                let q = if value.contains('"') { '\'' } else { '"' };
                let _ = write!(out, " {name}={q}{value}{q}");
            }
            if e.children.is_empty() {
                out.push_str(" />");
            } else {
                out.push('>');
                for child in &e.children {
                    write_node(out, child);
                }
                let _ = write!(out, "</{}>", e.name);
            }
        }
    }
}

/// Text from a file: UTF-8 (with or without a byte order mark), UTF-16
/// with one, or otherwise Windows-1252, which is what the docs for
/// Slipstream, the original mod manager, say it accepts.
pub fn decode(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        return decode(rest);
    }
    let utf16 = |be: bool, b: &[u8]| {
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| {
                if be {
                    u16::from_be_bytes(c)
                } else {
                    u16::from_le_bytes(c)
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    if let Some(rest) = bytes.strip_prefix(b"\xff\xfe") {
        return utf16(false, rest);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xfe\xff") {
        return utf16(true, rest);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| windows_1252(b)).collect(),
    }
}

fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9f => HIGH[usize::from(b - 0x80)],
        _ => char::from(b),
    }
}
