//! Helpers that most commands share: the selected profile, game installs, and
//! arguments used by more than one command.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use patchwork_core::edits::{FieldInfo, FieldKind, FieldValue};
use patchwork_core::issue::Severity;
use patchwork_core::order::Position;
use patchwork_core::profile::{Profile, ProfileRef};
use patchwork_core::settings::{Origin, SettingState};
use patchwork_core::{GameInstall, Patchwork};

#[derive(clap::Args)]
#[group(required = true, multiple = false)]
pub(crate) struct MoveTo {
    #[arg(long)]
    top: bool,
    #[arg(long)]
    bottom: bool,
    /// The position to move it to, counting from 1 as `list` does.
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    to: Option<u32>,
    #[arg(long)]
    before: Option<String>,
    #[arg(long)]
    after: Option<String>,
}

impl MoveTo {
    pub(crate) fn position(self) -> Position {
        if self.top {
            Position::Top
        } else if self.bottom {
            Position::Bottom
        } else if let Some(n) = self.to {
            Position::Index(n as usize - 1)
        } else if let Some(name) = self.before {
            Position::Before(name)
        } else if let Some(name) = self.after {
            Position::After(name)
        } else {
            unreachable!("clap requires one of them")
        }
    }
}

pub(crate) fn game_install(pw: &Patchwork, id: &str) -> Result<GameInstall> {
    pw.game_install(id)?.with_context(|| {
        format!("patchwork doesn't know where {id} is installed; run `patchwork game set-path {id} <dir>` first")
    })
}

pub(crate) fn print_setting(setting: &SettingState) {
    let key = setting.spec.key;
    let shown = match (&setting.value, setting.origin) {
        (_, None) => match &setting.default {
            Some(default) => format!("{default} (default)"),
            None => "(not set)".to_owned(),
        },
        (Some(value), _) => value.to_string(),
        (None, _) => "(set)".to_owned(),
    };
    let from = match setting.origin {
        Some(Origin::Env(var)) => format!(" from {var}"),
        Some(Origin::Keychain) => " in the system keychain".to_owned(),
        _ => String::new(),
    };
    println!("{key} = {shown}{from}");
}

pub(crate) fn severity(severity: Severity) -> &'static str {
    match severity {
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

pub(crate) fn selected(pw: &Patchwork, explicit: &Option<String>) -> Result<(ProfileRef, Profile)> {
    let profile = match explicit {
        Some(profile) => pw.find_profile(profile)?,
        None => match pw.current_profile()? {
            Some(profile) => profile,
            None => {
                bail!("no profile selected; create one with `patchwork profile new <game>/<name>`")
            }
        },
    };
    let loaded = pw.load_profile(&profile)?;
    Ok((profile, loaded))
}

/// What `--set FIELD=VALUE` and `--reset FIELD` say to change, for
/// `patchwork edit` and `patchwork game edit`; `kind` gives a field's kind
/// by id, so lists can be split at commas.
pub(crate) fn field_changes(
    set: Vec<String>,
    reset: Vec<String>,
    kind: impl Fn(&str) -> Option<FieldKind>,
) -> Result<BTreeMap<String, Option<FieldValue>>> {
    let mut changes = BTreeMap::new();
    for field in reset {
        changes.insert(field, None);
    }
    for change in set {
        let (field, value) = change
            .split_once('=')
            .with_context(|| format!("`--set {change}` should be FIELD=VALUE"))?;
        let value = match kind(field) {
            Some(FieldKind::List) => {
                FieldValue::List(value.split(',').map(str::to_owned).collect())
            }
            _ => FieldValue::Text(value.to_owned()),
        };
        changes.insert(field.to_owned(), Some(value));
    }
    Ok(changes)
}

/// Prints editable fields as they are, saying which have been changed. With
/// nothing `changed`, also says how to change them, which `ids` can be.
pub(crate) fn print_fields(fields: &[FieldInfo], changed: bool, ids: &[&str]) {
    let show = |v: &FieldValue| match v {
        FieldValue::Text(t) => t.clone(),
        FieldValue::List(items) => items.join(", "),
    };
    for f in fields {
        match (&f.edited, &f.source) {
            (Some(edited), Some(source)) if edited != source => {
                println!("{}: {}  (source: {})", f.id, show(edited), show(source));
            }
            (Some(edited), _) => println!("{}: {}  (changed)", f.id, show(edited)),
            (None, Some(source)) => println!("{}: {}", f.id, show(source)),
            // with nothing to show, only listing what can be set
            (None, None) if !changed => println!("{}: -", f.id),
            (None, None) => {}
        }
    }
    if !changed {
        println!(
            "\nchange one with --set FIELD=VALUE; fields: {}",
            ids.join(", ")
        );
    }
}
