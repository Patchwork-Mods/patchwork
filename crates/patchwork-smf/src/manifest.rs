//! Simple Mod Framework's `manifest.json`, parsed as JSON5 like SMF does.
//! Only what deploying needs is modelled; anything else is ignored.

use std::collections::HashSet;

use serde::Deserialize;
use serde_json::Value;

use crate::condition;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub options: Vec<ModOption>,
    #[serde(flatten)]
    pub data: ModData,
}

/// What a mod contributes. Both the manifest itself and each of its options
/// can have these, and an enabled option's add to the manifest's.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModData {
    pub content_folders: Vec<String>,
    pub blobs_folders: Vec<String>,
    pub localisation: Option<Value>,
    pub localisation_overrides: Option<Value>,
    pub localised_lines: Option<Value>,
    pub packagedefinition: Vec<PackageDefinitionEntry>,
    pub thumbs: Vec<String>,
    pub dependencies: Vec<Dependency>,
    pub requirements: Vec<ModRef>,
    pub incompatibilities: Vec<ModRef>,
    pub peacock_plugins: Vec<String>,
    pub scripts: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModOption {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: OptionKind,
    #[serde(default)]
    pub enabled_by_default: bool,
    /// The group a `select` option is one of.
    #[serde(default)]
    pub group: Option<String>,
    /// When a `conditional` option is on.
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub tooltip: Option<String>,
    /// A picture of it, relative to the manifest.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(flatten)]
    pub data: ModData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionKind {
    Checkbox,
    Select,
    Conditional,
    /// Kinds from newer SMF versions.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PackageDefinitionEntry {
    #[serde(rename_all = "camelCase")]
    Partition {
        name: String,
        parent: String,
        partition_type: String,
    },
    Entity {
        partition: String,
        path: String,
    },
}

/// A resource whose dependencies are copied into a chunk, so a mod can use
/// something from one part of the game in another.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    Hash(String),
    #[serde(rename_all = "camelCase")]
    Full {
        #[serde(rename = "runtimeID")]
        runtime_id: String,
        #[serde(default)]
        to_chunk: usize,
        #[serde(default)]
        port_from_chunk1: bool,
    },
}

impl Dependency {
    pub fn runtime_id(&self) -> &str {
        match self {
            Dependency::Hash(id) | Dependency::Full { runtime_id: id, .. } => id,
        }
    }

    pub fn to_chunk(&self) -> usize {
        match self {
            Dependency::Hash(_) => 0,
            Dependency::Full { to_chunk, .. } => *to_chunk,
        }
    }

    /// Whether resources in chunk1 are copied too, not just those outside
    /// chunks 0 and 1.
    pub fn includes_chunk1(&self) -> bool {
        match self {
            Dependency::Hash(_) => false,
            Dependency::Full {
                port_from_chunk1, ..
            } => *port_from_chunk1,
        }
    }
}

/// Another mod, by manifest id, optionally with a semver range.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ModRef {
    Id(String),
    WithVersion(String, String),
}

impl ModRef {
    pub fn id(&self) -> &str {
        match self {
            ModRef::Id(id) | ModRef::WithVersion(id, _) => id,
        }
    }
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        json5::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())
    }

    /// The manifest's data and that of the options in effect: the
    /// checkboxes and selects `picked` (as [`ModOption::key`]s), or those
    /// enabled by default if nothing's been picked, and the conditionals
    /// whose condition holds for mods `load_order`. Returns warnings for
    /// options that couldn't be decided.
    pub fn active(
        &self,
        picked: Option<&[String]>,
        load_order: &HashSet<String>,
    ) -> (Vec<&ModData>, Vec<String>) {
        let mut data = vec![&self.data];
        let mut warnings = Vec::new();
        for option in &self.options {
            match option.on(picked, load_order) {
                Ok(true) => data.push(&option.data),
                Ok(false) => {}
                Err(e) => {
                    warnings.push(format!("option \"{}\" is left off, since {e}", option.name))
                }
            }
        }
        (data, warnings)
    }

    /// Whether any options are for the user to pick.
    pub fn has_choices(&self) -> bool {
        self.options.iter().any(ModOption::pickable)
    }
}

impl ModOption {
    /// What SMF saves to say the option's picked: its name, or for a
    /// select, `group:name`.
    pub fn key(&self) -> String {
        match self.kind {
            OptionKind::Select => format!(
                "{}:{}",
                self.group.as_deref().unwrap_or_default(),
                self.name
            ),
            _ => self.name.clone(),
        }
    }

    /// Whether it's for the user to pick.
    pub fn pickable(&self) -> bool {
        matches!(self.kind, OptionKind::Checkbox | OptionKind::Select)
    }

    /// Whether it's on, as [`Manifest::active`] says.
    pub fn on(
        &self,
        picked: Option<&[String]>,
        load_order: &HashSet<String>,
    ) -> Result<bool, String> {
        match self.kind {
            OptionKind::Checkbox | OptionKind::Select => Ok(match picked {
                Some(picked) => picked.contains(&self.key()),
                None => self.enabled_by_default,
            }),
            OptionKind::Conditional => {
                let condition = self
                    .condition
                    .as_deref()
                    .ok_or_else(|| "it has no condition".to_owned())?;
                condition::evaluate(condition, load_order)
                    .map_err(|e| format!("its condition `{condition}` can't be worked out: {e}"))
            }
            OptionKind::Unknown => Err("it's of a kind patchwork doesn't know".to_owned()),
        }
    }
}

impl ModData {
    /// Features this data uses that patchwork can't deploy yet.
    pub fn unsupported(&self) -> Vec<&'static str> {
        let mut found = Vec::new();
        if !self.scripts.is_empty() {
            found.push("scripts");
        }
        found
    }
}

/// A new string from a mod's `localisation`, named `key`, for `language`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalisedString {
    pub language: String,
    pub key: String,
    pub text: String,
}

/// A value that should be an object, with null as an empty one.
fn object(
    value: &Value,
    feature: &str,
    what: &str,
) -> Result<serde_json::Map<String, Value>, String> {
    match value {
        Value::Object(o) => Ok(o.clone()),
        Value::Null => Ok(Default::default()),
        _ => Err(format!("`{feature}`: {what} isn't an object")),
    }
}

/// A string a mod's `localisationOverrides` changes: in the LOCR `locr`
/// (a runtime ID or path), for `language`, the string named `key`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalisationOverride {
    pub locr: String,
    pub language: String,
    pub key: String,
    pub text: String,
}

impl ModData {
    /// The strings `localisation` adds, in order. It's an object of
    /// languages, each an object of strings.
    pub fn localisation(&self) -> Result<Vec<LocalisedString>, String> {
        const FEATURE: &str = "localisation";
        let mut found = Vec::new();
        let Some(localisation) = &self.localisation else {
            return Ok(found);
        };
        for (language, strings) in object(localisation, FEATURE, "it")? {
            for (key, text) in object(&strings, FEATURE, &format!("`{language}`"))? {
                let Value::String(text) = text else {
                    return Err(format!("`{FEATURE}`: `{language}`.`{key}` isn't a string"));
                };
                found.push(LocalisedString {
                    language: language.clone(),
                    key,
                    text,
                });
            }
        }
        Ok(found)
    }

    /// `localisedLines`: runtime IDs (or paths), and the name of the string
    /// each is a line for.
    pub fn localised_lines(&self) -> Result<Vec<(String, String)>, String> {
        const FEATURE: &str = "localisedLines";
        let mut found = Vec::new();
        let Some(lines) = &self.localised_lines else {
            return Ok(found);
        };
        for (line, name) in object(lines, FEATURE, "it")? {
            let Value::String(name) = name else {
                return Err(format!("`{FEATURE}`: `{line}` isn't a string"));
            };
            found.push((line, name));
        }
        Ok(found)
    }

    /// The strings `localisationOverrides` changes, in order. It's an object
    /// of LOCRs, each an object of languages, each an object of strings.
    pub fn localisation_overrides(&self) -> Result<Vec<LocalisationOverride>, String> {
        let mut found = Vec::new();
        let Some(overrides) = &self.localisation_overrides else {
            return Ok(found);
        };
        const FEATURE: &str = "localisationOverrides";
        for (locr, languages) in object(overrides, FEATURE, "it")? {
            for (language, strings) in object(&languages, FEATURE, &format!("`{locr}`"))? {
                for (key, text) in object(&strings, FEATURE, &format!("`{locr}`.`{language}`"))? {
                    let Value::String(text) = text else {
                        return Err(format!(
                            "`localisationOverrides`: `{locr}`.`{language}`.`{key}` isn't a string"
                        ));
                    };
                    found.push(LocalisationOverride {
                        locr: locr.clone(),
                        language: language.clone(),
                        key,
                        text,
                    });
                }
            }
        }
        Ok(found)
    }
}
