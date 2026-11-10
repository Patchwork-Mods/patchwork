//! `patchwork game`: setting up games, their paths, settings and runners.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use patchwork_core::Patchwork;
use patchwork_core::deploy::{DeployTarget, RootId, RootOrigin};
use patchwork_core::launch::{self};
use patchwork_core::locate::Locator;

use crate::common::{game_install, print_setting};

#[derive(Subcommand)]
pub(crate) enum GameCommand {
    /// List known games.
    List,
    /// Show a game's definition.
    Show { id: String },
    /// Tell patchwork where a game is installed.
    SetPath { id: String, path: PathBuf },
    /// Look for where games are installed: in every Steam library, and by
    /// GOG Galaxy, the Epic Games Launcher, Heroic and Legendary. Give a
    /// game to look for only that one.
    Detect {
        id: Option<String>,
        /// Use what's found, if it's in one place. Without a game, this sets
        /// up every game that's found and isn't set up yet.
        #[arg(long)]
        set: bool,
    },
    /// List a game's settings: how mods are linked into it, its deployer's
    /// settings, how it's started, and where its other folders are.
    Settings { id: String },
    /// List what can run a game, and what each one's settings are.
    Runners { id: String },
    /// Change one of a game's settings (see `game settings`). `prefix` is
    /// the Wine prefix a Windows game runs in, `root.<id>` moves one of its
    /// other folders, `launch` is the way to start it by default (see
    /// `launch --list`), `runner` is what to run it with, and
    /// `runner.<runner>.<setting>` changes a runner's setting (see `game
    /// runners`).
    Set {
        id: String,
        key: String,
        value: String,
    },
    /// Go back to what the game's definition says for a setting.
    Unset { id: String, key: String },
}

pub(crate) fn run(pw: &Patchwork, command: GameCommand) -> Result<()> {
    match command {
        GameCommand::List => {
            for entry in pw.registry().games() {
                let game = &entry.value;
                let installed = match pw.game_install(&game.id)? {
                    Some(install) => install.path.display().to_string(),
                    None => "not set up".to_owned(),
                };
                let hidden = if game.hidden { " (hidden)" } else { "" };
                println!(
                    "{}  {}{hidden} [{}]  {installed}",
                    game.id,
                    game.name,
                    entry.origins()
                );
            }
        }
        GameCommand::Show { id } => {
            let registry = pw.registry();
            let entry = registry.game(&id)?;
            println!(
                "# origin: {}, hash: {}",
                entry.origins(),
                entry.hash.short()
            );
            print!("{}", toml::to_string_pretty(&entry.value)?);
            if let Some(env) = pw.game_env(&DeployTarget::Game(id))? {
                println!("# installed: {env}");
            }
        }
        GameCommand::SetPath { id, path } => {
            let install = pw.set_game_path(&id, &path)?;
            println!("{id} is installed at {}", install.path.display());
        }
        GameCommand::Detect { id: Some(id), set } => {
            let registry = pw.registry();
            let def = &registry.game(&id)?.value;
            let found = Locator::scan().find(def);
            if found.is_empty() {
                bail!(
                    "couldn't find where {} is installed; run `patchwork game set-path {id} <dir>` to say",
                    def.name
                );
            }
            for found in &found {
                println!("{}  ({})", found.path.display(), found.origin);
            }
            if set {
                let [found] = found.as_slice() else {
                    bail!(
                        "{} is in more than one place; pick one with `patchwork game set-path {id} <dir>`",
                        def.name
                    );
                };
                let install = pw.set_game_path(&id, &found.path)?;
                println!("{id} is installed at {}", install.path.display());
            }
        }
        GameCommand::Detect { id: None, set } => {
            let locator = Locator::scan();
            let mut any = false;
            for entry in pw.registry().games().filter(|e| !e.value.hidden) {
                let game = &entry.value;
                let found = locator.find(game);
                if found.is_empty() {
                    continue;
                }
                any = true;
                let current = pw.game_install(&game.id)?.map(|i| i.path);
                for found in &found {
                    let mark = if current.as_ref() == Some(&found.path) {
                        "  (set up)"
                    } else {
                        ""
                    };
                    println!(
                        "{}  {}  ({}){mark}",
                        game.id,
                        found.path.display(),
                        found.origin
                    );
                }
                // never moves a game that's already set up
                if set
                    && current.is_none()
                    && let [found] = found.as_slice()
                {
                    pw.set_game_path(&game.id, &found.path)?;
                    println!("  set up {}", game.name);
                }
            }
            if !any {
                println!("no known games were found");
            }
        }
        GameCommand::Settings { id } => {
            let registry = pw.registry();
            let def = &registry.game(&id)?.value;
            let install = pw.game_install(&id)?;
            let link = install.as_ref().and_then(|i| i.link);
            let set = if link.is_some() { "" } else { " (default)" };
            let link = toml::Value::try_from(link.unwrap_or(def.link))?;
            println!("link = {link}{set}");
            for setting in pw.deployer_settings(&id)? {
                print_setting(&setting);
            }
            if let Some(launch) = install.as_ref().and_then(|i| i.launch.as_ref()) {
                println!("launch = {}", toml::Value::from(launch.as_str()));
            }
            if let Some(choices) = pw.runner_choices(&id)? {
                print_picked_runner(&choices);
                for runner in &choices.runners {
                    for setting in runner.settings.iter().filter(|s| s.origin.is_some()) {
                        print!("runner.{}.", runner.id);
                        print_setting(setting);
                    }
                }
            }
            let Some(roots) = pw.resolve_roots(&DeployTarget::Game(id.clone()))? else {
                return Ok(());
            };
            let set_prefix = install.as_ref().and_then(|i| i.prefix.as_ref());
            if let Some(prefix) = roots.prefix() {
                let how = if set_prefix.is_some() { "" } else { " (found)" };
                println!(
                    "prefix = {}{how}",
                    toml::Value::from(prefix.display().to_string())
                );
            }
            for (root, resolved) in roots.iter() {
                let allow: Vec<_> = resolved.allow.iter().map(|a| a.as_str()).collect();
                let allow = allow.join(", ");
                match &resolved.path {
                    Ok(path) => {
                        let how = match resolved.origin {
                            RootOrigin::Set => "",
                            RootOrigin::Definition => " (default)",
                            RootOrigin::Prefix => " (in the prefix)",
                        };
                        let path = toml::Value::from(path.display().to_string());
                        println!("root.{root} = {path}{how}  # for {allow}");
                    }
                    Err(why) => println!("# root.{root} isn't set: {why}"),
                }
            }
        }
        GameCommand::Runners { id } => {
            let choices = pw.runner_choices(&id)?.with_context(|| {
                format!("patchwork doesn't know where {id} is installed; run `patchwork game set-path {id} <dir>` first")
            })?;
            print_picked_runner(&choices);
            for runner in &choices.runners {
                match &runner.unavailable {
                    Some(why) => println!("{}  {}  [unavailable: {why}]", runner.id, runner.name),
                    None => println!("{}  {}", runner.id, runner.name),
                }
                for setting in &runner.settings {
                    print!("  ");
                    print_setting(setting);
                }
            }
        }
        GameCommand::Set { id, key, value } => {
            let mut install = game_install(pw, &id)?;
            if key == "link" {
                install.link = Some(toml::Value::String(value).try_into()?);
            } else if key == "prefix" {
                install.prefix = Some(PathBuf::from(value));
            } else if key == "launch" {
                install.launch = Some(value);
            } else if key == "runner" {
                install.runner = Some(value);
            } else if let Some(rest) = key.strip_prefix("runner.") {
                let (runner, setting) = runner_setting_key(rest)?;
                let choices = pw
                    .runner_choices(&id)?
                    .unwrap_or_else(|| unreachable!("game_install found that it's set up"));
                let Some(found) = choices.runners.iter().find(|r| r.id == runner) else {
                    bail!("no runner named `{runner}`; see `patchwork game runners {id}`");
                };
                let Some(spec) = found.settings.iter().find(|s| s.spec.key == setting) else {
                    let keys: Vec<_> = found.settings.iter().map(|s| s.spec.key).collect();
                    if keys.is_empty() {
                        bail!("the runner `{runner}` doesn't have any settings");
                    }
                    bail!(
                        "the runner `{runner}` has no setting `{setting}`; it has {}",
                        keys.join(", ")
                    );
                };
                install
                    .runners
                    .entry(runner.to_owned())
                    .or_default()
                    .insert(setting.to_owned(), spec.spec.parse(&value)?);
            } else if let Some(root) = key.strip_prefix("root.") {
                let root = RootId::new(root).map_err(anyhow::Error::msg)?;
                install.roots.insert(root, PathBuf::from(value));
            } else {
                let settings = pw.deployer_settings(&id)?;
                let Some(setting) = settings.iter().find(|s| s.spec.key == key) else {
                    let keys: Vec<_> = [
                        "link",
                        "prefix",
                        "root.<id>",
                        "launch",
                        "runner",
                        "runner.<runner>.<setting>",
                    ]
                    .into_iter()
                    .chain(settings.iter().map(|s| s.spec.key))
                    .collect();
                    bail!("{id} has no setting `{key}`; it has {}", keys.join(", "));
                };
                install
                    .deployer
                    .insert(key.clone(), setting.spec.parse(&value)?);
            }
            pw.set_game_install(&id, install)?;
            println!("set {key} for {id}");
        }
        GameCommand::Unset { id, key } => {
            let mut install = game_install(pw, &id)?;
            let root = key.strip_prefix("root.").and_then(|r| RootId::new(r).ok());
            if key == "link" {
                install.link = None;
            } else if key == "prefix" {
                if install.prefix.take().is_none() {
                    bail!("prefix isn't set for {id}");
                }
            } else if key == "launch" {
                if install.launch.take().is_none() {
                    bail!("launch isn't set for {id}");
                }
            } else if key == "runner" {
                if install.runner.take().is_none() {
                    bail!("runner isn't set for {id}");
                }
            } else if let Some(rest) = key.strip_prefix("runner.") {
                let (runner, setting) = runner_setting_key(rest)?;
                let set = install.runners.get_mut(runner);
                if set.and_then(|s| s.remove(setting)).is_none() {
                    bail!("{key} isn't set for {id}");
                }
                if install.runners[runner].is_empty() {
                    install.runners.remove(runner);
                }
            } else if let Some(root) = root {
                if install.roots.remove(&root).is_none() {
                    bail!("{key} isn't set for {id}");
                }
            } else if install.deployer.remove(&key).is_none() {
                bail!("{key} isn't set for {id}");
            }
            pw.set_game_install(&id, install)?;
            println!("{key} is back to {id}'s default");
        }
    }
    Ok(())
}

/// The runner and setting in what follows `runner.` in a setting's key.
fn runner_setting_key(key: &str) -> Result<(&str, &str)> {
    key.split_once('.').with_context(|| {
        format!("`runner.{key}` isn't a setting; expected `runner.<runner>.<setting>`")
    })
}

fn print_picked_runner(choices: &launch::RunnerChoices) {
    let name = |id: &str| {
        choices
            .runners
            .iter()
            .find(|r| r.id == id)
            .map_or_else(|| id.to_owned(), |r| r.name.clone())
    };
    match (&choices.picked, &choices.automatic) {
        (Some(picked), _) => println!("runner = {}", toml::Value::from(picked.as_str())),
        (None, Some(id)) => println!(
            "runner = {} (automatic: {})",
            toml::Value::from(id.as_str()),
            name(id)
        ),
        (None, None) => println!("runner = (automatic, but nothing can run it right now)"),
    }
}
