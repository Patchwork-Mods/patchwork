//! Errors the user can do something about. Most commands that fail just
//! have a message for the status bar, but some fail on something with a fix,
//! like a mod whose file changed since it was locked. Core says what that
//! is with a [`patchwork_core::fix::Blocker`], which reaches the window as
//! a [`Blocker`]. `invokeResolving` in state.js asks the user whether to
//! apply its fix, does (through [`apply_fix`], or in the window for fixes
//! only it can do), and tries the command that failed again.

use std::any::Any;
use std::fmt;
use std::path::Path;

use patchwork_core::deploy::DeployTarget;
use patchwork_core::fix::Fix;
use serde::{Serialize, Serializer};
use serde_json::json;
use tauri::{AppHandle, State};

use crate::{App, CmdResult, Deployed, Job, apply_cancellable, lock_msg, profile_ref};

/// Something in the way of a command, and how to clear it, as the window
/// shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocker {
    /// What's in the way, short enough for a dialog's heading.
    pub title: String,
    /// Why it's in the way, and what the fix does.
    pub detail: String,
    /// What the button that applies `fix` says.
    pub fix_label: String,
    pub fix: Fix,
    /// What the status bar says while `fix` is applied.
    pub fixing: String,
}

impl From<patchwork_core::fix::Blocker> for Blocker {
    fn from(b: patchwork_core::fix::Blocker) -> Self {
        Self {
            fix_label: b.fix.label(),
            fixing: b.fix.doing(),
            title: b.title,
            detail: b.detail,
            fix: b.fix,
        }
    }
}

/// Why a command failed: a message, or a [`Blocker`] the user can clear.
#[derive(Debug)]
pub struct CmdError {
    pub message: String,
    pub blocker: Option<Box<Blocker>>,
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

impl From<patchwork_core::Error> for CmdError {
    fn from(e: patchwork_core::Error) -> Self {
        Self::from_core(&e)
    }
}

impl CmdError {
    /// `e`'s message, with its blocker if it has one.
    pub fn from_core(e: &patchwork_core::Error) -> Self {
        Self {
            message: e.to_string(),
            blocker: e.blocker().map(|b| Box::new(b.into())),
        }
    }

    /// `e`'s message, with its blocker if it's (or wraps) a core error that
    /// has one.
    pub fn from_display(e: &(impl fmt::Display + 'static)) -> Self {
        let any = e as &dyn Any;
        let core = any.downcast_ref::<patchwork_core::Error>().or_else(|| {
            any.downcast_ref::<anyhow::Error>()
                .and_then(|e| e.downcast_ref())
        });
        match core {
            Some(core) => Self::from_core(core),
            None => e.to_string().into(),
        }
    }
}

/// Applies `fix` for the profile `id` (what the command it's for was
/// working on), for the fixes that don't need the window. Fails on the
/// rest, which state.js applies itself.
#[tauri::command]
pub async fn apply_fix(
    handle: AppHandle,
    app: State<'_, App>,
    id: Option<String>,
    fix: Fix,
) -> CmdResult<()> {
    let profile = || match &id {
        Some(id) => profile_ref(id),
        None => Err("that fix is for a profile, but none was given".into()),
    };
    match fix {
        Fix::AcceptChanged { keys, .. } => {
            let _work = app.work.start()?;
            app.pw.accept_changed(&profile()?, &keys).await?;
        }
        Fix::Undeploy { game, instance } => {
            let target = match instance {
                Some(p) => DeployTarget::Instance {
                    game,
                    profile: profile_ref(&p.id)?,
                },
                None => DeployTarget::Game(game),
            };
            let _work = app.work.start()?;
            let job = Job::start(&app);
            let plan = app.pw.plan_undeploy(&target)?;
            let cancel = job.0.clone();
            if let Deployed::Cancelled { .. } = apply_cancellable(handle, plan, cancel).await? {
                return Err("undeploy stopped partway; undeploy again to finish it".into());
            }
        }
        Fix::SetGamePath { game, path } => {
            app.pw.set_game_path(&game, Path::new(&path))?;
        }
        Fix::Lock => {
            let _work = app.work.start()?;
            app.pw
                .lock(&profile()?, false)
                .await
                .map_err(lock_msg(&handle))?;
        }
        Fix::Sync => crate::sync::wait_for_files(&handle, &profile()?).await?,
        Fix::Deploy | Fix::ChooseGamePath { .. } | Fix::UpdatePatchwork | Fix::ProfileSettings => {
            return Err(format!("`{}` is done by the window", fix.label()).into());
        }
    }
    Ok(())
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
    fn a_core_blocker_reaches_the_window_with_its_fix() {
        let e = patchwork_core::Error::invalid("other");
        assert!(CmdError::from_core(&e).blocker.is_none());
        let e = patchwork_core::Error::blocked(
            "m",
            "t",
            "d",
            Fix::ChooseGamePath {
                game: "g".to_owned(),
            },
        );
        let value = serde_json::to_value(CmdError::from_display(&e)).unwrap();
        assert_eq!(value["message"], "m");
        assert_eq!(value["blocker"]["title"], "t");
        assert_eq!(value["blocker"]["fix"]["type"], "chooseGamePath");
        assert_eq!(value["blocker"]["fixLabel"], "Choose folder…");
        let wrapped = anyhow::Error::from(e);
        assert!(CmdError::from_display(&wrapped).blocker.is_some());
    }
}
