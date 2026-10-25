//! Valve's KeyValues text format, which Steam keeps its library list and app
//! manifests in:
//!
//! ```text
//! "AppState"
//! {
//!     "appid"       "489830"
//!     "installdir"  "Skyrim Special Edition"
//! }
//! ```
//!
//! Only as much of it as reading those files needs: quoted and bare strings,
//! nested blocks, `\` escapes and `//` comments. Conditionals like `[$WIN32]`
//! are skipped.

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    Str(String),
    Block(Vec<(String, Value)>),
}

impl Value {
    /// The first entry named `key`, ignoring case as Steam does.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.get(key)?.as_str()
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Block(_) => None,
        }
    }

    /// A block's entries, or none for a string.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &Value)> {
        let entries = match self {
            Value::Block(entries) => entries.as_slice(),
            Value::Str(_) => &[],
        };
        entries.iter().map(|(k, v)| (k.as_str(), v))
    }
}

/// Parses a whole file as the entries of one block. `None` if it isn't
/// KeyValues.
pub(crate) fn parse(text: &str) -> Option<Value> {
    let mut tokens = Tokens {
        rest: text.strip_prefix('\u{feff}').unwrap_or(text),
        unterminated: false,
    };
    let entries = block(&mut tokens, false)?;
    (!tokens.unterminated).then_some(Value::Block(entries))
}

/// Entries up to the closing `}`, or the end of the file if this is the
/// top level.
fn block(tokens: &mut Tokens, nested: bool) -> Option<Vec<(String, Value)>> {
    let mut entries = Vec::new();
    loop {
        let key = match tokens.next() {
            None if !nested => return Some(entries),
            Some(Token::Close) if nested => return Some(entries),
            Some(Token::Str(key)) => key,
            _ => return None,
        };
        let value = match tokens.next()? {
            Token::Str(value) => Value::Str(value),
            Token::Open => Value::Block(block(tokens, true)?),
            Token::Close => return None,
        };
        entries.push((key, value));
    }
}

enum Token {
    Str(String),
    Open,
    Close,
}

struct Tokens<'a> {
    rest: &'a str,
    /// Whether the text ended in the middle of a string, which otherwise
    /// looks just like it ending.
    unterminated: bool,
}

impl Tokens<'_> {
    fn next(&mut self) -> Option<Token> {
        loop {
            self.rest = self.rest.trim_start();
            if let Some(comment) = self.rest.strip_prefix("//") {
                self.rest = comment.split_once('\n').map_or("", |(_, rest)| rest);
            } else if self.rest.starts_with('[') {
                // a conditional, which applies to the entry before it
                self.rest = self.rest.split_once(']').map_or("", |(_, rest)| rest);
            } else {
                break;
            }
        }
        let mut chars = self.rest.chars();
        let token = match chars.next()? {
            '{' => Token::Open,
            '}' => Token::Close,
            '"' => {
                let mut s = String::new();
                loop {
                    let Some(c) = chars.next() else {
                        self.unterminated = true;
                        return None;
                    };
                    match c {
                        '"' => break,
                        '\\' => match chars.next().unwrap_or('\\') {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            c => s.push(c),
                        },
                        c => s.push(c),
                    }
                }
                Token::Str(s)
            }
            _ => {
                let end = self
                    .rest
                    .find(|c: char| c.is_whitespace() || matches!(c, '"' | '{' | '}'))
                    .unwrap_or(self.rest.len());
                let (word, rest) = self.rest.split_at(end);
                self.rest = rest;
                return Some(Token::Str(word.to_owned()));
            }
        };
        self.rest = chars.as_str();
        Some(token)
    }
}

#[cfg(test)]
mod tests;
