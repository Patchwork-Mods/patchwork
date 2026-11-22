//! Errors the user can do something about. Most commands that fail just
//! have a message for the status bar, but some fail on something with a fix,
//! like a mod whose file changed since it was locked. Those fail with a
//! [`Blocker`]: what's in the way, and the command that clears it. `run` in
//! state.js asks the user whether to run that command, and tries the one
//! that failed again if they do.

use std::fmt;

use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use tauri::State;

use crate::{App, CmdResult, msg, profile_ref};

/// Something in the way of a command, and how to clear it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocker {
    /// What's in the way, short enough for a dialog's heading.
    pub title: String,
    /// Why it's in the way, and what the fix does.
    pub detail: String,
    /// What the button that runs `command` says.
    pub fix_label: String,
    /// The command that clears it, and its arguments.
    pub command: &'static str,
    pub args: Value,
    /// What the status bar says while `command` runs.
    pub fixing: String,
}

/// Why a command failed: a message, or a [`Blocker`] the user can clear.
#[derive(Debug)]
pub struct CmdError {
    pub message: String,
    pub blocker: Option<Box<Blocker>>,
}

impl CmdError {
    pub fn blocked(message: impl Into<String>, blocker: Blocker) -> Self {
        Self {
            message: message.into(),
            blocker: Some(Box::new(blocker)),
        }
    }
}

impl fmt::Display for CmdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<String> for CmdError {
    fn from(message: String) -> Self {
        Self {
            message,
            blocker: None,
        }
    }
}

impl From<&str> for CmdError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

/// A plain message goes to the window as just its text, as every command's
/// errors did before blockers, so only `run` has to know about them.
impl Serialize for CmdError {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.blocker {
            None => s.serialize_str(&self.message),
            Some(blocker) => json!({ "message": self.message, "blocker": blocker }).serialize(s),
        }
    }
}

/// The blocker for `e` while working on the profile `id`, if it has a fix.
pub fn blocker_for(id: &str, e: &patchwork_core::Error) -> Option<Blocker> {
    match e {
        patchwork_core::Error::ArtifactChanged(changed) => {
            let patchwork_core::error::ArtifactChanged {
                key, name, version, ..
            } = &**changed;
            let at = version
                .as_deref()
                .map(|v| format!(" {v}"))
                .unwrap_or_default();
            Some(Blocker {
                title: format!("{name} changed since it was locked"),
                detail: format!(
                    "{name}{at} downloads as a different file now than the one this profile \
                     was locked with, here or on a PC it syncs with. That usually means its \
                     author or the site it's from repackaged it. Using the new file keeps \
                     {name} at the same version, and PCs this profile syncs with will use it \
                     too."
                ),
                fix_label: "Use the new file".to_owned(),
                command: "accept_changed",
                args: json!({ "id": id, "keys": [key] }),
                fixing: format!("Using the new file for {name}…"),
            })
        }
        _ => None,
    }
}

/// Locks the mods in `keys` (as `<source>:<id>`) to the files they're
/// downloaded as now, keeping their versions. Clears the blocker for a mod
/// whose file changed since it was locked.
#[tauri::command]
pub async fn accept_changed(
    app: State<'_, App>,
    id: String,
    keys: Vec<String>,
) -> CmdResult<Vec<String>> {
    let _work = app.work.start()?;
    let r = profile_ref(&id)?;
    app.pw.accept_changed(&r, &keys).await.map_err(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_error_reaches_the_window_as_its_message() {
        let e = CmdError::from("nope");
        assert_eq!(serde_json::to_value(&e).unwrap(), json!("nope"));
    }

    #[test]
    fn a_blocker_carries_its_fix() {
        let e = patchwork_core::Error::invalid("other");
        assert!(blocker_for("p", &e).is_none());
        let blocker = Blocker {
            title: "t".to_owned(),
            detail: "d".to_owned(),
            fix_label: "Fix".to_owned(),
            command: "accept_changed",
            args: json!({ "id": "p" }),
            fixing: "Fixing…".to_owned(),
        };
        let value = serde_json::to_value(CmdError::blocked("m", blocker)).unwrap();
        assert_eq!(value["message"], "m");
        assert_eq!(value["blocker"]["command"], "accept_changed");
        assert_eq!(value["blocker"]["fixLabel"], "Fix");
    }
}
