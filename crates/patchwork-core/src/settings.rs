//! Settings the user gives sources and deployers, as opposed to the ones game
//! definitions give them. A source's settings (an API key, say) apply to
//! every game and are kept in `<config>/settings.toml`; a deployer's are made
//! for each game and kept with its install (see [`crate::GameInstall`]).
//!
//! Sources and deployers list the settings they take as [`SettingSpec`]s, so
//! frontends can show a form for them without knowing anything about them.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::IoContext;
use crate::paths::Paths;
use crate::{Error, Result, fsutil};

/// A setting a source or deployer takes.
///
/// ```
/// # use patchwork_core::settings::{SettingKind, SettingSpec};
/// const API_KEY: SettingSpec = SettingSpec::new("api_key", "API key", SettingKind::Secret)
///     .help("Needed for anything but searching.")
///     .link("https://example.com/api-keys")
///     .env("EXAMPLE_API_KEY");
/// ```
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SettingSpec {
    pub key: &'static str,
    pub label: &'static str,
    #[serde(flatten)]
    pub kind: SettingKind,
    /// A sentence or two on what it's for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<&'static str>,
    /// A page that helps with it, such as where to get an API key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<&'static str>,
    /// An environment variable that takes the setting's place when it's set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SettingKind {
    Text,
    /// Text that's kept in the system keychain when there is one, and never
    /// shown again once it's set.
    Secret,
    Bool,
    /// One of a few strings.
    Choice {
        choices: &'static [Choice],
    },
    /// A file or folder.
    Path,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
}

impl SettingSpec {
    pub const fn new(key: &'static str, label: &'static str, kind: SettingKind) -> Self {
        SettingSpec {
            key,
            label,
            kind,
            help: None,
            link: None,
            env: None,
        }
    }

    pub const fn help(mut self, help: &'static str) -> Self {
        self.help = Some(help);
        self
    }

    pub const fn link(mut self, link: &'static str) -> Self {
        self.link = Some(link);
        self
    }

    pub const fn env(mut self, var: &'static str) -> Self {
        self.env = Some(var);
        self
    }

    /// Checks that `value` is the kind of value this setting takes.
    pub fn check(&self, value: &toml::Value) -> Result<()> {
        let ok = match (&self.kind, value) {
            (SettingKind::Bool, toml::Value::Boolean(_)) => true,
            (SettingKind::Choice { choices }, toml::Value::String(s)) => {
                choices.iter().any(|c| c.value == s)
            }
            (SettingKind::Choice { .. } | SettingKind::Bool, _) => false,
            (_, toml::Value::String(s)) => !s.trim().is_empty(),
            _ => false,
        };
        if ok {
            return Ok(());
        }
        let wanted = match &self.kind {
            SettingKind::Bool => "true or false".to_owned(),
            SettingKind::Choice { choices } => {
                let values: Vec<_> = choices.iter().map(|c| c.value).collect();
                format!("one of {}", values.join(", "))
            }
            _ => "some text".to_owned(),
        };
        let shown = value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned);
        Err(Error::invalid(format!(
            "`{shown}` won't do for “{}”; it should be {wanted}",
            self.label
        )))
    }

    /// Turns text a user typed into the kind of value this setting takes.
    pub fn parse(&self, text: &str) -> Result<toml::Value> {
        let value = match self.kind {
            SettingKind::Bool => match text.trim() {
                "true" | "yes" | "on" => toml::Value::Boolean(true),
                "false" | "no" | "off" => toml::Value::Boolean(false),
                _ => toml::Value::String(text.to_owned()),
            },
            _ => toml::Value::String(text.trim().to_owned()),
        };
        self.check(&value)?;
        Ok(value)
    }

    /// The spec in `specs` for `key`.
    pub fn find<'a>(specs: &'a [SettingSpec], owner: &str, key: &str) -> Result<&'a SettingSpec> {
        specs.iter().find(|s| s.key == key).ok_or_else(|| {
            let keys: Vec<_> = specs.iter().map(|s| s.key).collect();
            Error::invalid(if keys.is_empty() {
                format!("{owner} doesn't have any settings")
            } else {
                format!("{owner} has no setting `{key}`; it has {}", keys.join(", "))
            })
        })
    }
}

/// Where a setting's value came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "from", content = "var", rename_all = "lowercase")]
pub enum Origin {
    /// `settings.toml`, or for deployers, the game's install settings.
    File,
    Keychain,
    /// An environment variable, which can't be changed from patchwork.
    Env(&'static str),
}

/// A setting and its value, for frontends to show.
#[derive(Debug, Clone, Serialize)]
pub struct SettingState {
    #[serde(flatten)]
    pub spec: SettingSpec,
    /// Always `None` for secrets, which are never handed back out.
    pub value: Option<toml::Value>,
    /// What it is when it isn't set: for deployers, what the game's
    /// definition says.
    pub default: Option<toml::Value>,
    /// `None` if it isn't set.
    pub origin: Option<Origin>,
}

/// `<config>/settings.toml`.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsFile {
    /// Secrets kept in the system keychain instead of here, as
    /// `sources.<id>.<key>`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    keychain: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sources: BTreeMap<String, toml::Table>,
}

/// Every source's settings, from wherever each one is set.
#[derive(Debug, Default)]
pub struct Settings {
    sources: BTreeMap<String, Resolved>,
    /// Settings that are set but couldn't be read, as messages for the user.
    pub problems: Vec<String>,
}

#[derive(Debug, Default)]
struct Resolved {
    values: toml::Table,
    origins: BTreeMap<String, Origin>,
}

/// The keychain service everything is saved under.
const KEYCHAIN_SERVICE: &str = "patchwork";

fn keychain_error(e: keyring::Error) -> Error {
    Error::invalid(format!("system keychain: {e}"))
}

fn keychain_get(name: &str) -> Result<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, name)
        .and_then(|e| e.get_password())
        .map_err(keychain_error)
}

fn keychain_set(name: &str, value: &str) -> Result<()> {
    keyring::Entry::new(KEYCHAIN_SERVICE, name)
        .and_then(|e| e.set_password(value))
        .map_err(keychain_error)
}

fn keychain_delete(name: &str) -> Result<()> {
    match keyring::Entry::new(KEYCHAIN_SERVICE, name).and_then(|e| e.delete_credential()) {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(keychain_error(e)),
    }
}

fn file_path(paths: &Paths) -> PathBuf {
    paths.config.join("settings.toml")
}

impl Settings {
    /// Reads the settings of the sources with `specs`, by source id. Env
    /// vars come first, then the keychain, then `settings.toml`.
    pub(crate) fn load(paths: &Paths, specs: &BTreeMap<&str, &[SettingSpec]>) -> Result<Self> {
        let file: SettingsFile = fsutil::read_toml(&file_path(paths))?.unwrap_or_default();
        let mut settings = Settings::default();
        for (&source, &specs) in specs {
            let mut resolved = Resolved::default();
            let saved = file.sources.get(source);
            for spec in specs {
                let name = format!("sources.{source}.{}", spec.key);
                let from_env = spec
                    .env
                    .and_then(|var| Some((var, std::env::var(var).ok()?)))
                    .filter(|(_, v)| !v.is_empty());
                let found = if let Some((var, value)) = from_env {
                    Some((toml::Value::String(value), Origin::Env(var)))
                } else if paths.keychain && file.keychain.contains(&name) {
                    match keychain_get(&name) {
                        Ok(value) => Some((toml::Value::String(value), Origin::Keychain)),
                        Err(e) => {
                            settings
                                .problems
                                .push(format!("couldn't read {source}'s {}: {e}", spec.label));
                            None
                        }
                    }
                } else if let Some(value) = saved.and_then(|t| t.get(spec.key)) {
                    match spec.check(value) {
                        Ok(()) => Some((value.clone(), Origin::File)),
                        Err(e) => {
                            settings.problems.push(format!("{source}: {e}"));
                            None
                        }
                    }
                } else {
                    None
                };
                if let Some((value, origin)) = found {
                    resolved.values.insert(spec.key.to_owned(), value);
                    resolved.origins.insert(spec.key.to_owned(), origin);
                }
            }
            settings.sources.insert(source.to_owned(), resolved);
        }
        Ok(settings)
    }

    /// A source's settings, for its [`crate::source::SourceContext`].
    /// `None` if it has none set.
    pub fn source(&self, id: &str) -> Option<&toml::Table> {
        self.sources
            .get(id)
            .map(|r| &r.values)
            .filter(|v| !v.is_empty())
    }

    pub(crate) fn states(&self, id: &str, specs: &[SettingSpec]) -> Vec<SettingState> {
        let resolved = self.sources.get(id);
        specs
            .iter()
            .map(|spec| {
                let value = resolved.and_then(|r| r.values.get(spec.key));
                SettingState {
                    spec: *spec,
                    value: value
                        .filter(|_| !matches!(spec.kind, SettingKind::Secret))
                        .cloned(),
                    default: None,
                    origin: resolved.and_then(|r| r.origins.get(spec.key)).copied(),
                }
            })
            .collect()
    }

    /// Saves a source's setting, or clears it if `value` is `None`. Secrets
    /// go in the keychain if there is one. Returns where it was saved.
    pub(crate) fn save(
        paths: &Paths,
        source: &str,
        spec: &SettingSpec,
        value: Option<toml::Value>,
    ) -> Result<Option<Origin>> {
        let path = file_path(paths);
        let mut file: SettingsFile = fsutil::read_toml(&path)?.unwrap_or_default();
        let name = format!("sources.{source}.{}", spec.key);
        if let Some(table) = file.sources.get_mut(source) {
            table.remove(spec.key);
            if table.is_empty() {
                file.sources.remove(source);
            }
        }
        if let Some(i) = file.keychain.iter().position(|n| *n == name) {
            keychain_delete(&name)?;
            file.keychain.remove(i);
        }
        let origin = match value {
            None => None,
            Some(value) => {
                spec.check(&value)?;
                // with no keychain to be had, secrets go in the file
                let in_keychain = matches!(spec.kind, SettingKind::Secret)
                    && paths.keychain
                    && value
                        .as_str()
                        .is_some_and(|v| keychain_set(&name, v).is_ok());
                if in_keychain {
                    file.keychain.push(name);
                    Some(Origin::Keychain)
                } else {
                    file.sources
                        .entry(source.to_owned())
                        .or_default()
                        .insert(spec.key.to_owned(), value);
                    Some(Origin::File)
                }
            }
        };
        write_private(&path, &file)?;
        Ok(origin)
    }
}

/// Writes `value` where only the user can read it, since it can have
/// secrets in it when there's no keychain.
fn write_private<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let text = toml::to_string_pretty(value).map_err(|e| Error::parse(path, e))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).at(dir)?;
    // temp files are only readable by their owner
    let mut tmp = tempfile::NamedTempFile::new_in(dir).at(dir)?;
    tmp.write_all(text.as_bytes()).at(tmp.path())?;
    tmp.persist(path).map_err(|e| e.error).at(path)?;
    Ok(())
}
