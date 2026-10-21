//! Simple Mod Framework's `manifest.json`, parsed as JSON5 like SMF does.
//! Only what deploying needs is modelled; anything else is ignored.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
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

    /// The manifest's data and that of the options in effect. Until options
    /// can be chosen, that's the ones enabled by default. Returns warnings
    /// for options that couldn't be decided.
    pub fn active(&self) -> (Vec<&ModData>, Vec<String>) {
        let mut data = vec![&self.data];
        let mut warnings = Vec::new();
        for option in &self.options {
            match option.kind {
                OptionKind::Checkbox | OptionKind::Select if option.enabled_by_default => {
                    data.push(&option.data);
                }
                OptionKind::Checkbox | OptionKind::Select => {}
                OptionKind::Conditional => warnings.push(format!(
                    "option \"{}\" depends on a condition, which patchwork doesn't support yet; it's left off",
                    option.name
                )),
                OptionKind::Unknown => warnings.push(format!(
                    "option \"{}\" is of a kind patchwork doesn't know; it's left off",
                    option.name
                )),
            }
        }
        (data, warnings)
    }
}

impl ModData {
    /// Features this data uses that patchwork can't deploy yet.
    pub fn unsupported(&self) -> Vec<&'static str> {
        let mut found = Vec::new();
        let used = |v: &Option<Value>| match v {
            None | Some(Value::Null) => false,
            Some(Value::Object(o)) => o.values().any(|v| match v {
                Value::Object(inner) => !inner.is_empty(),
                _ => true,
            }),
            Some(_) => true,
        };
        if used(&self.localisation) {
            found.push("localisation");
        }
        if used(&self.localisation_overrides) {
            found.push("localisationOverrides");
        }
        if used(&self.localised_lines) {
            found.push("localisedLines");
        }
        if !self.scripts.is_empty() {
            found.push("scripts");
        }
        found
    }
}
