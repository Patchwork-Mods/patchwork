//! How a profile's game is set up to run: what it picks for the game's
//! `[env]` (its mod loader, say) and whether it's deployed to an instance
//! of its own. Also capturing what tools wrote into the game's folder as a
//! mod. These are the CLI's `env`, `instance` and `capture`.

use std::collections::BTreeMap;

use patchwork_core::capture::ToolOutput;
use patchwork_core::profile::Profile;
use patchwork_core::{Captured, Patchwork};
use serde::Serialize;
use tauri::{AppHandle, State};

use crate::{App, CmdResult, ProfileDetail, lock_msg, msg, profile_detail, profile_ref};

/// One of the game's `[env]` axes, and what the profile picks for it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiEnvAxis {
    id: String,
    name: String,
    /// What can be picked; empty if it's anything, like a version.
    choices: Vec<String>,
    default: Option<String>,
    picked: Option<String>,
}

/// Whether a profile is deployed to an instance of its own, for games that
/// can have them.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiInstance {
    on: bool,
    /// What the game's definition says, for "Default (…)".
    default: bool,
    /// Where it is, whether or not it's made yet.
    dir: String,
}

/// What [`ProfileDetail`] says about how the profile's game is set up.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSetup {
    env: Vec<UiEnvAxis>,
    instance: Option<UiInstance>,
    /// Whether the game has somewhere tools write files that can be
    /// captured.
    capture: bool,
}

pub fn profile_setup(pw: &Patchwork, profile: &Profile) -> CmdResult<ProfileSetup> {
    let registry = pw.registry();
    let game = &registry.game(&profile.game).map_err(msg)?.value;
    let env = game
        .env
        .iter()
        .map(|(id, axis)| UiEnvAxis {
            id: id.to_owned(),
            name: axis.name.clone(),
            choices: axis.choices.clone(),
            default: axis.default.clone(),
            picked: profile.env.get(id).cloned(),
        })
        .collect();
    let uses_instance = pw.uses_instance(profile).map_err(msg)?;
    let instance = game.instances.as_ref().map(|def| UiInstance {
        on: uses_instance,
        default: def.default,
        dir: pw.instance_dir(&profile.reference()).display().to_string(),
    });
    Ok(ProfileSetup {
        env,
        instance,
        capture: game.capture.is_some(),
    })
}

/// Picks values for the game's `[env]` axes (`None` goes back to the
/// game's default), then relocks every mod to suit, as `patchwork env`
/// does. The picks are kept even if relocking fails.
#[tauri::command]
pub async fn set_profile_env(
    handle: AppHandle,
    app: State<'_, App>,
    id: String,
    picks: BTreeMap<String, Option<String>>,
) -> CmdResult<ProfileDetail> {
    let _work = app.work.start()?;
    let r = profile_ref(&id)?;
    for (axis, value) in &picks {
        app.pw
            .set_env(
                &r,
                axis,
                value.as_deref().map(str::trim).filter(|v| !v.is_empty()),
            )
            .map_err(msg)?;
    }
    if !picks.is_empty() {
        app.pw.lock(&r, false).await.map_err(lock_msg(&handle))?;
    }
    profile_detail(&app.pw, id)
}

/// Sets whether the profile is deployed to an instance of its own on this
/// machine, or with `None` goes back to the game's default. It has to be
/// undeployed first if that moves where it's deployed.
#[tauri::command]
pub fn set_profile_instance(
    app: State<'_, App>,
    id: String,
    on: Option<bool>,
) -> CmdResult<ProfileDetail> {
    app.pw.set_instance(&profile_ref(&id)?, on).map_err(msg)?;
    profile_detail(&app.pw, id)
}

/// What tools have written into the game's folder since the profile was
/// deployed, which capturing would keep.
#[tauri::command]
pub fn tool_output(app: State<'_, App>, id: String) -> CmdResult<ToolOutput> {
    app.pw.tool_output(&profile_ref(&id)?).map_err(msg)
}

/// Moves what tools wrote into the game's folder into the mod called
/// `name`. It's taken out of the game's folder, so the profile has to be
/// deployed again to put it back.
#[tauri::command]
pub async fn capture(app: State<'_, App>, id: String, name: String) -> CmdResult<Captured> {
    let _work = app.work.start()?;
    app.pw
        .capture(&profile_ref(&id)?, name.trim())
        .await
        .map_err(msg)
}
