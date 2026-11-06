//! The conditions of SMF's `conditional` options, which SMF evaluates with
//! filtrex against its config: in practice things like
//! `"Some.Mod" not in config.loadOrder and "Other.Mod" in config.loadOrder`.
//!
//! This is the part of filtrex those use: `and`, `or`, `not`, `in`,
//! `not in`, comparisons, parentheses, strings, numbers, `(a, b)` lists and
//! dotted names. The only name there is is `config.loadOrder`, the manifest
//! ids of the mods being deployed (or the names of mods without a
//! manifest), as SMF's is.

use std::collections::HashSet;

/// Whether `condition` holds for mods `load_order`.
pub fn evaluate(condition: &str, load_order: &HashSet<String>) -> Result<bool, String> {
    let tokens = tokenize(condition)?;
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
        load_order,
    };
    let value = parser.or()?;
    match parser.tokens.get(parser.at) {
        None => Ok(value.truthy()),
        Some(token) => Err(format!("didn't expect {token:?} there")),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Str(String),
    Num(f64),
    Name(String),
    Symbol(&'static str),
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    const SYMBOLS: &[&str] = &["==", "!=", "<=", ">=", "<", ">", "(", ")", ",", "."];
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' || c == '\'' {
            // filtrex: double quotes for strings, single for names.
            chars.next();
            let mut s = String::new();
            loop {
                match chars.next() {
                    None => return Err("a quote isn't closed".to_owned()),
                    Some((_, '\\')) => match chars.next() {
                        Some((_, e)) => s.push(e),
                        None => return Err("a quote isn't closed".to_owned()),
                    },
                    Some((_, q)) if q == c => break,
                    Some((_, other)) => s.push(other),
                }
            }
            tokens.push(if c == '"' {
                Token::Str(s)
            } else {
                Token::Name(s)
            });
        } else if c.is_ascii_digit() {
            let mut end = i;
            while let Some(&(j, d)) = chars.peek() {
                if d.is_ascii_digit() || d == '.' {
                    end = j + d.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let n = text[i..end]
                .parse()
                .map_err(|_| format!("{} isn't a number", &text[i..end]))?;
            tokens.push(Token::Num(n));
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            let mut end = i;
            while let Some(&(j, d)) = chars.peek() {
                if d.is_alphanumeric() || d == '_' || d == '$' {
                    end = j + d.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(Token::Name(text[i..end].to_owned()));
        } else if let Some(symbol) = SYMBOLS.iter().find(|s| text[i..].starts_with(**s)) {
            for _ in 0..symbol.len() {
                chars.next();
            }
            tokens.push(Token::Symbol(symbol));
        } else {
            return Err(format!("didn't expect `{c}`"));
        }
    }
    Ok(tokens)
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<Value>),
}

impl Value {
    fn truthy(&self) -> bool {
        match self {
            Value::Str(s) => !s.is_empty(),
            Value::Num(n) => *n != 0.0,
            Value::Bool(b) => *b,
            Value::List(_) => true,
        }
    }

    /// filtrex's `in`: whether `self` is in `list`, or equal to it if it
    /// isn't a list.
    fn is_in(&self, list: &Value) -> bool {
        match list {
            Value::List(items) => items.contains(self),
            other => other == self,
        }
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
    load_order: &'a HashSet<String>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat_name(&mut self, name: &str) -> bool {
        if matches!(self.peek(), Some(Token::Name(n)) if n == name) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn eat_symbol(&mut self, symbol: &str) -> bool {
        if matches!(self.peek(), Some(Token::Symbol(s)) if *s == symbol) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn or(&mut self) -> Result<Value, String> {
        let mut value = self.and()?;
        while self.eat_name("or") {
            let right = self.and()?;
            value = Value::Bool(value.truthy() || right.truthy());
        }
        Ok(value)
    }

    fn and(&mut self) -> Result<Value, String> {
        let mut value = self.not()?;
        while self.eat_name("and") {
            let right = self.not()?;
            value = Value::Bool(value.truthy() && right.truthy());
        }
        Ok(value)
    }

    fn not(&mut self) -> Result<Value, String> {
        if self.eat_name("not") {
            return Ok(Value::Bool(!self.not()?.truthy()));
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Value, String> {
        let left = self.primary()?;
        if self.eat_name("in") {
            let right = self.primary()?;
            return Ok(Value::Bool(left.is_in(&right)));
        }
        // `not in`, but not a `not` that starts the next operand.
        if matches!(self.peek(), Some(Token::Name(n)) if n == "not")
            && matches!(self.tokens.get(self.at + 1), Some(Token::Name(n)) if n == "in")
        {
            self.at += 2;
            let right = self.primary()?;
            return Ok(Value::Bool(!left.is_in(&right)));
        }
        for op in ["==", "!=", "<=", ">=", "<", ">"] {
            if self.eat_symbol(op) {
                let right = self.primary()?;
                return compare(op, &left, &right);
            }
        }
        Ok(left)
    }

    fn primary(&mut self) -> Result<Value, String> {
        let token = self
            .peek()
            .cloned()
            .ok_or_else(|| "it ends too soon".to_owned())?;
        self.at += 1;
        match token {
            Token::Str(s) => Ok(Value::Str(s)),
            Token::Num(n) => Ok(Value::Num(n)),
            Token::Symbol("(") => {
                let first = self.or()?;
                if self.eat_symbol(")") {
                    return Ok(first);
                }
                let mut items = vec![first];
                while self.eat_symbol(",") {
                    items.push(self.or()?);
                }
                if !self.eat_symbol(")") {
                    return Err("a bracket isn't closed".to_owned());
                }
                Ok(Value::List(items))
            }
            Token::Name(name) => {
                let mut path = vec![name];
                while self.eat_symbol(".") {
                    match self.peek().cloned() {
                        Some(Token::Name(n)) => {
                            self.at += 1;
                            path.push(n);
                        }
                        _ => return Err("a `.` isn't followed by a name".to_owned()),
                    }
                }
                self.lookup(&path)
            }
            Token::Symbol(s) => Err(format!("didn't expect `{s}`")),
        }
    }

    fn lookup(&self, path: &[String]) -> Result<Value, String> {
        match path {
            [one] if one == "true" => Ok(Value::Bool(true)),
            [one] if one == "false" => Ok(Value::Bool(false)),
            [config, load_order] if config == "config" && load_order == "loadOrder" => {
                let mut ids: Vec<_> = self.load_order.iter().cloned().collect();
                ids.sort();
                Ok(Value::List(ids.into_iter().map(Value::Str).collect()))
            }
            _ => Err(format!(
                "patchwork doesn't know `{}` (only `config.loadOrder`)",
                path.join(".")
            )),
        }
    }
}

fn compare(op: &str, left: &Value, right: &Value) -> Result<Value, String> {
    let result = match op {
        "==" => left == right,
        "!=" => left != right,
        _ => match (left, right) {
            (Value::Num(a), Value::Num(b)) => match op {
                "<" => a < b,
                ">" => a > b,
                "<=" => a <= b,
                _ => a >= b,
            },
            _ => return Err(format!("`{op}` only compares numbers")),
        },
    };
    Ok(Value::Bool(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(ids: &[&str]) -> HashSet<String> {
        ids.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn load_order_membership() {
        let mods = order(&["A.One", "musicalmushr00m.UntouchableSigSuitFix"]);
        let eval = |c| evaluate(c, &mods).unwrap();
        assert!(eval(r#""A.One" in config.loadOrder"#));
        assert!(!eval(r#""B.Two" in config.loadOrder"#));
        assert!(eval(r#""B.Two" not in config.loadOrder"#));
        assert!(eval(
            r#""B.Two" not in config.loadOrder and "musicalmushr00m.UntouchableSigSuitFix" in config.loadOrder"#
        ));
        assert!(!eval(
            r#""A.One" not in config.loadOrder and "B.Two" not in config.loadOrder"#
        ));
        assert!(eval(
            r#""B.Two" in config.loadOrder or ("A.One" in config.loadOrder)"#
        ));
        assert!(eval(r#"not ("B.Two" in config.loadOrder)"#));
        assert!(eval(r#"not "B.Two" in config.loadOrder"#));
        assert!(eval(r#""x" in ("x", "y") and 2 > 1 and "a" == "a""#));
    }

    #[test]
    fn what_it_cant_evaluate_is_an_error() {
        let mods = order(&[]);
        assert!(evaluate(r#""A" in config.modOptions"#, &mods).is_err());
        assert!(evaluate(r#""A" in"#, &mods).is_err());
        assert!(evaluate(r#""A"#, &mods).is_err());
        assert!(evaluate(r#"("A" in config.loadOrder"#, &mods).is_err());
        assert!(evaluate(r#""A" "B""#, &mods).is_err());
    }
}
