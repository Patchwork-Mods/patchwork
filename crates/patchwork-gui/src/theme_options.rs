//! Theme options: settings a theme offers in the app, so its look can be
//! changed without touching files.
//!
//! Each `[[option]]` table in a theme's `@theme` header is one setting. It
//! sets a CSS variable on `:root` (`var`), and toggles and choices also set
//! `data-opt-<id>` on `:root`, for rules a variable can't switch on or off:
//!
//! ```toml
//! [[option]]
//! id = "sparks"
//! label = "Sparks"
//! type = "choice"
//! var = "--sk-sparks"
//! choices = [
//!   { value = "moving", label = "Moving", css = "running" },
//!   { value = "still", label = "Still", css = "paused" },
//!   { value = "hidden", label = "Hidden", css = "paused" },
//! ]
//!
//! [[option]]
//! id = "glow"
//! label = "Glow"
//! type = "range"
//! var = "--sk-glow"
//! min = 0
//! max = 1
//! step = 0.05
//! default = 1
//! ```
//!
//! The types are `toggle` (`on`/`off` are the variable's values, `default`
//! true or false), `choice` (`css` is the variable's value, `value` if left
//! out, and `default` a choice's value, the first if left out), `color` (a
//! hex `default`) and `range` (`min`, `max`, `step`, `default`, and a
//! `unit` put after the number). The values picked are kept in
//! `gui.toml`'s `[theme.options.<theme id>]`, by option id. The frontend
//! (`ui/theme.js`) turns them into CSS.

use std::collections::BTreeMap;

use patchwork_core::game::Accent;
use serde::{Deserialize, Serialize};

/// One `[[option]]` from a theme's header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeOption {
    /// Its key in `gui.toml`, and the name of its `data-opt-<id>` attribute.
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The CSS variable it sets on `:root`, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub var: Option<String>,
    #[serde(flatten)]
    pub kind: OptionKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OptionKind {
    Toggle {
        #[serde(default = "yes")]
        default: bool,
        /// The variable's value when it's on, and when it's off.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        off: Option<String>,
    },
    Choice {
        choices: Vec<Choice>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        default: Option<String>,
    },
    Color {
        default: String,
    },
    Range {
        min: f64,
        max: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<f64>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        unit: String,
        default: f64,
    },
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// What's kept in `gui.toml` and put in `data-opt-<id>`.
    pub value: String,
    pub label: String,
    /// The variable's value, if not `value`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
}

/// An option's value, as kept in `gui.toml` and sent to the frontend.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum OptionValue {
    Bool(bool),
    Number(f64),
    Text(String),
}

/// Whether `s` can be an id: lower-case letters, digits and dashes.
fn is_id(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Checks a value that goes into the generated `:root { ... }` rule as it
/// is: it can't end the declaration or the rule.
fn check_css(what: &str, s: &str) -> Result<(), String> {
    if s.contains([';', '{', '}']) {
        return Err(format!("{what} {s:?} can't have ;, {{ or }} in it"));
    }
    Ok(())
}

impl ThemeOption {
    /// Whether the option makes sense, so the frontend can trust it.
    fn check(&self) -> Result<(), String> {
        let id = &self.id;
        if !is_id(id) {
            return Err(format!(
                "option id {id:?} should be lower-case letters, digits and dashes"
            ));
        }
        let what = |s: &str| format!("option {id:?}: {s}");
        if let Some(var) = &self.var {
            let name = var.strip_prefix("--").unwrap_or("");
            let ok = !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
            if !ok {
                return Err(what(&format!(
                    "{var:?} isn't a CSS variable like \"--my-color\""
                )));
            }
        }
        match &self.kind {
            OptionKind::Toggle { on, off, .. } => {
                if self.var.is_some() && (on.is_none() || off.is_none()) {
                    return Err(what("a toggle with a var needs `on` and `off`"));
                }
                for v in [on, off].into_iter().flatten() {
                    check_css(&what("value"), v)?;
                }
            }
            OptionKind::Choice { choices, default } => {
                if choices.is_empty() {
                    return Err(what("a choice needs `choices`"));
                }
                for (i, c) in choices.iter().enumerate() {
                    if !is_id(&c.value) {
                        return Err(what(&format!(
                            "choice {:?} should be lower-case letters, digits and dashes",
                            c.value
                        )));
                    }
                    if choices[..i].iter().any(|o| o.value == c.value) {
                        return Err(what(&format!("{:?} is there twice", c.value)));
                    }
                    check_css(&what("value"), c.css.as_deref().unwrap_or(&c.value))?;
                }
                if let Some(d) = default
                    && !choices.iter().any(|c| &c.value == d)
                {
                    return Err(what(&format!("the default {d:?} isn't one of the choices")));
                }
            }
            OptionKind::Color { default } => {
                Accent::try_from(default.clone()).map_err(|e| what(&e))?;
            }
            OptionKind::Range {
                min,
                max,
                step,
                unit,
                default,
            } => {
                if !min.is_finite() || !max.is_finite() || min >= max {
                    return Err(what("`min` should be less than `max`"));
                }
                if !(min..=max).contains(&default) {
                    return Err(what("`default` should be between `min` and `max`"));
                }
                if step.is_some_and(|s| s.is_nan() || s <= 0.0) {
                    return Err(what("`step` should be more than 0"));
                }
                if !unit.bytes().all(|b| b.is_ascii_alphabetic() || b == b'%') {
                    return Err(what(&format!("{unit:?} isn't a CSS unit like \"px\"")));
                }
            }
        }
        Ok(())
    }

    /// The value to use: `saved`, when it's one this option can have,
    /// otherwise the default.
    pub fn value(&self, saved: Option<&toml::Value>) -> OptionValue {
        match &self.kind {
            OptionKind::Toggle { default, .. } => {
                OptionValue::Bool(saved.and_then(toml::Value::as_bool).unwrap_or(*default))
            }
            OptionKind::Choice { choices, default } => {
                let saved = saved
                    .and_then(toml::Value::as_str)
                    .filter(|s| choices.iter().any(|c| c.value == *s));
                let value = saved.or(default.as_deref()).unwrap_or(&choices[0].value);
                OptionValue::Text(value.to_owned())
            }
            OptionKind::Color { default } => {
                let saved = saved
                    .and_then(toml::Value::as_str)
                    .and_then(|s| Accent::try_from(s.to_owned()).ok());
                OptionValue::Text(saved.map_or_else(|| default.to_ascii_lowercase(), String::from))
            }
            OptionKind::Range {
                min, max, default, ..
            } => {
                let saved = saved.and_then(|v| match v {
                    toml::Value::Float(f) => Some(*f),
                    #[allow(clippy::cast_precision_loss)]
                    toml::Value::Integer(i) => Some(*i as f64),
                    _ => None,
                });
                let n = saved.filter(|n| n.is_finite()).unwrap_or(*default);
                OptionValue::Number(n.clamp(*min, *max))
            }
        }
    }
}

/// Checks a theme's options: each one, and that no two share an id.
pub fn check(options: &[ThemeOption]) -> Result<(), String> {
    for (i, option) in options.iter().enumerate() {
        option.check()?;
        if options[..i].iter().any(|o| o.id == option.id) {
            return Err(format!("option {:?} is there twice", option.id));
        }
    }
    Ok(())
}

/// The values of `options`, from `saved` (a theme's table in
/// `[theme.options]`) or their defaults.
pub fn values(
    options: &[ThemeOption],
    saved: Option<&toml::Table>,
) -> BTreeMap<String, OptionValue> {
    options
        .iter()
        .map(|o| (o.id.clone(), o.value(saved.and_then(|t| t.get(&o.id)))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> Result<Vec<ThemeOption>, String> {
        #[derive(Deserialize)]
        struct Header {
            option: Vec<ThemeOption>,
        }
        let header: Header = toml::from_str(toml).map_err(|e| e.to_string())?;
        check(&header.option)?;
        Ok(header.option)
    }

    const OPTIONS: &str = r##"
        [[option]]
        id = "sparks"
        label = "Sparks"
        type = "toggle"
        var = "--sk-sparks"
        on = "running"
        off = "paused"

        [[option]]
        id = "density"
        label = "Density"
        type = "choice"
        choices = [{ value = "few", label = "Few" }, { value = "many", label = "Many", css = "2" }]
        default = "many"

        [[option]]
        id = "tint"
        label = "Tint"
        type = "color"
        var = "--tint"
        default = "#A8303A"

        [[option]]
        id = "glow"
        label = "Glow"
        type = "range"
        var = "--glow"
        min = 0
        max = 100
        unit = "%"
        default = 80
    "##;

    #[test]
    fn reads_options() {
        let options = parse(OPTIONS).unwrap();
        assert_eq!(options.len(), 4);
        assert_eq!(
            options[0].kind,
            OptionKind::Toggle {
                default: true,
                on: Some("running".into()),
                off: Some("paused".into())
            }
        );
        let defaults = values(&options, None);
        assert_eq!(defaults["sparks"], OptionValue::Bool(true));
        assert_eq!(defaults["density"], OptionValue::Text("many".into()));
        assert_eq!(defaults["tint"], OptionValue::Text("#a8303a".into()));
        assert_eq!(defaults["glow"], OptionValue::Number(80.0));
    }

    #[test]
    fn saved_values_are_checked() {
        let options = parse(OPTIONS).unwrap();
        let saved: toml::Table = toml::from_str(
            r##"
            sparks = false
            density = "lots"
            tint = "#fff"
            glow = 250
            "##,
        )
        .unwrap();
        let v = values(&options, Some(&saved));
        assert_eq!(v["sparks"], OptionValue::Bool(false));
        // Not one of the choices: the default.
        assert_eq!(v["density"], OptionValue::Text("many".into()));
        assert_eq!(v["tint"], OptionValue::Text("#fff".into()));
        assert_eq!(v["glow"], OptionValue::Number(100.0));

        let saved: toml::Table =
            toml::from_str("tint = \"red; color: blue\"\nglow = 12.5").unwrap();
        let v = values(&options, Some(&saved));
        assert_eq!(v["tint"], OptionValue::Text("#a8303a".into()));
        assert_eq!(v["glow"], OptionValue::Number(12.5));
    }

    #[test]
    fn bad_options() {
        let bad = [
            "id = \"Sparks\"\nlabel = \"x\"\ntype = \"toggle\"",
            "id = \"a\"\nlabel = \"x\"\ntype = \"toggle\"\nvar = \"--a\"",
            "id = \"a\"\nlabel = \"x\"\ntype = \"toggle\"\nvar = \"a\"\non = \"1\"\noff = \"0\"",
            "id = \"a\"\nlabel = \"x\"\ntype = \"toggle\"\nvar = \"--a\"\non = \"1; }\"\noff = \"0\"",
            "id = \"a\"\nlabel = \"x\"\ntype = \"choice\"\nchoices = []",
            "id = \"a\"\nlabel = \"x\"\ntype = \"choice\"\nchoices = [{ value = \"b\", label = \"B\" }]\ndefault = \"c\"",
            "id = \"a\"\nlabel = \"x\"\ntype = \"color\"\ndefault = \"red\"",
            "id = \"a\"\nlabel = \"x\"\ntype = \"range\"\nmin = 1\nmax = 0\ndefault = 0",
            "id = \"a\"\nlabel = \"x\"\ntype = \"range\"\nmin = 0\nmax = 1\ndefault = 2",
            "id = \"a\"\nlabel = \"x\"\ntype = \"range\"\nmin = 0\nmax = 1\ndefault = 0\nunit = \"px;\"",
            "id = \"a\"\nlabel = \"x\"\ntype = \"wobble\"",
        ];
        for toml in bad {
            assert!(parse(&format!("[[option]]\n{toml}")).is_err(), "{toml}");
        }
        let twice = "[[option]]\nid = \"a\"\nlabel = \"x\"\ntype = \"toggle\"\n\
                     [[option]]\nid = \"a\"\nlabel = \"y\"\ntype = \"toggle\"";
        assert!(parse(twice).is_err());
    }
}
