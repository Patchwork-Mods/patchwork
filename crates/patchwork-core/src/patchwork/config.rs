//! Changing settings in config files; see [`crate::config`].

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use super::{Patchwork, deploy_order, not_installed};
use crate::config::{
    self, ConfigContext, ConfigDefs, ConfigFormat, Contents, Edits, answers, checked_edits,
};
use crate::deploy::{
    self, DeployContext, DeployState, DeployTarget, Dest, GameFiles, Resolved, ResolvedRoots,
    Roots, changed_since_deployed,
};
use crate::error::IoContext;
use crate::event::Event;
use crate::ext::Hook;
use crate::game::GameDef;
use crate::hash::ContentHash;
use crate::options::{Answer, Form, GroupKind, Page};
use crate::profile::{Profile, ProfileRef};
use crate::relpath::RelPath;
use crate::{Error, Result, fsutil};

/// A config file whose settings can be changed, from
/// [`Patchwork::config_files`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFileInfo {
    /// Where it's deployed, which is how it's named.
    pub path: Dest,
    /// The [`ConfigFormat`] that reads it, as `<extension>.<name>`.
    pub format: String,
    /// What it says it's for, if it says.
    pub title: Option<String>,
    /// The mod whose copy it is, or `None` for one in the game's own
    /// folders (usually made by the game, or by a mod as it runs).
    #[serde(rename = "mod")]
    pub from: Option<String>,
    /// Whether the profile changes any of its settings.
    pub edited: bool,
    /// The profile changes its settings, but it isn't there (yet), so
    /// they aren't applied.
    pub missing: bool,
}

/// A config file's settings as a frontend shows them, from
/// [`Patchwork::config_form`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigView {
    pub path: Dest,
    pub format: String,
    #[serde(rename = "mod")]
    pub from: Option<String>,
    /// Whether the profile changes any of its settings.
    pub saved: bool,
    /// One page of groups of fields. Its `answers` are only the settings
    /// that are changed.
    pub form: Form,
}

/// Where a config file's settings are read from before the profile's
/// changes are made to it.
struct Base {
    src: PathBuf,
    /// The mod it's from, if any.
    from: Option<String>,
}

/// What the config files of a profile's game are worked out with.
struct Setup<'a> {
    defs: ConfigDefs,
    game: &'a GameDef,
}

impl Patchwork {
    /// Whether `profile`'s game has config files, or the profile changes
    /// settings in some; cheap enough for whenever a profile is shown,
    /// since it doesn't look for them.
    pub fn has_config_files(&self, profile: &ProfileRef) -> Result<bool> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        Ok(!loaded.config.is_empty() || !self.config_setup(game)?.defs.is_empty())
    }

    /// The config files `profile` can change settings in: those that its
    /// enabled, locked mods deploy, those in the game's folders, and any it
    /// changes settings in that aren't there now. Errors if the game isn't
    /// set up.
    pub fn config_files(&self, profile: &ProfileRef) -> Result<Vec<ConfigFileInfo>> {
        self.read_back_for(profile)?;
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let setup = self.config_setup(game)?;
        if setup.defs.is_empty() {
            return Ok(Vec::new());
        }
        let bases = self.config_bases(&loaded, &setup)?;
        let mut found = Vec::new();
        for (path, base) in &bases {
            let Some(def) = setup.defs.find(path) else {
                continue;
            };
            // Only to find its title, so a file that can't be read is
            // still listed; opening it says what's wrong.
            let title = self.config_format(&def.format).ok().and_then(|hook| {
                let bytes = fs::read(&base.src).ok()?;
                let contents = read(hook, &setup, path, &bytes).ok()?;
                contents.title
            });
            found.push(ConfigFileInfo {
                path: path.clone(),
                format: def.format.clone(),
                title,
                from: base.from.clone(),
                edited: edits_for(&loaded, path).is_some(),
                missing: false,
            });
        }
        for path in loaded.config.keys() {
            let listed = found.iter().any(|f| same_dest(&f.path, path));
            if !listed {
                found.push(ConfigFileInfo {
                    path: path.clone(),
                    format: setup
                        .defs
                        .find(path)
                        .map(|d| d.format.clone())
                        .unwrap_or_default(),
                    title: None,
                    from: None,
                    edited: true,
                    missing: true,
                });
            }
        }
        found.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(found)
    }

    /// The settings in the config file deployed to `path` for `profile`,
    /// with `answers` changing them, or with what the profile has saved if
    /// `None`. Values that don't fit their settings are left as the file
    /// has them, with a warning.
    pub fn config_form(
        &self,
        profile: &ProfileRef,
        path: &Dest,
        answers: Option<&[Answer]>,
    ) -> Result<ConfigView> {
        self.read_back_for(profile)?;
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let setup = self.config_setup(game)?;
        let (path, format, base, contents) = self.config_contents(&loaded, &setup, path)?;
        let saved = edits_for(&loaded, &path).cloned();
        let wanted = match answers {
            Some(answers) => edits_of(answers),
            None => saved.clone().unwrap_or_default(),
        };
        let (kept, problems) = checked_edits(&contents, &wanted);
        let file_name = path.path.as_str().rsplit('/').next().unwrap_or_default();
        let mut warnings = contents.warnings.clone();
        warnings.extend(
            problems
                .into_iter()
                .map(|p| format!("{p}, so it's left as it is")),
        );
        let groups = contents
            .groups
            .into_iter()
            .map(|mut group| {
                let changes = kept.get(&group.id);
                group.defaulted = changes.is_none_or(|c| c.is_empty());
                if let GroupKind::Fields { fields } = &mut group.kind {
                    for field in fields {
                        if let Some(value) = changes.and_then(|c| c.get(&field.id)) {
                            field.value = value.clone();
                        }
                    }
                }
                group
            })
            .collect();
        let changed = kept.values().map(BTreeMap::len).sum::<usize>();
        Ok(ConfigView {
            path: path.clone(),
            format,
            from: base.from,
            saved: saved.is_some(),
            form: Form {
                title: contents.title.unwrap_or_else(|| file_name.to_owned()),
                pages: vec![Page {
                    title: path.to_string(),
                    groups,
                }],
                summary: Some(match changed {
                    0 => "No settings changed".to_owned(),
                    1 => "1 setting changed".to_owned(),
                    n => format!("{n} settings changed"),
                }),
                warnings,
                answers: config::answers(&wanted),
            },
        })
    }

    /// Saves `answers` as the settings `profile` changes in the config file
    /// deployed to `path`, or with `None` forgets them, so it's deployed as
    /// it is. Only settings that differ from what the file has are kept.
    /// Values are checked against their settings first.
    pub fn set_config(
        &self,
        profile: &ProfileRef,
        path: &Dest,
        answers: Option<&[Answer]>,
    ) -> Result<()> {
        // So that settings changed in game don't come back afterwards.
        self.read_back_for(profile)?;
        let mut loaded = self.load_profile(profile)?;
        let Some(answers) = answers else {
            let before = loaded.config.len();
            loaded.config.retain(|p, _| !same_dest(p, path));
            if loaded.config.len() != before {
                self.save_profile(profile, &loaded)?;
            }
            return Ok(());
        };
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let setup = self.config_setup(game)?;
        let (path, _, _, contents) = self.config_contents(&loaded, &setup, path)?;
        let (kept, problems) = checked_edits(&contents, &edits_of(answers));
        if !problems.is_empty() {
            return Err(Error::invalid(problems.join("; ")));
        }
        let edits = config::changed(&contents.values(), &kept);
        loaded.config.retain(|p, _| !same_dest(p, &path));
        if !edits.is_empty() {
            loaded.config.insert(path, edits);
        }
        self.save_profile(profile, &loaded)
    }

    /// Reads settings the game changed in config files deployed to
    /// `target` back into the profile that's deployed there, before
    /// they're replaced or removed. Problems are reported as warnings.
    pub(super) fn read_back_config(&self, target: &DeployTarget) -> Result<()> {
        let mut state = self.deploy_state(target)?;
        let Some(profile) = state.profile.as_deref().and_then(|p| p.parse().ok()) else {
            return Ok(());
        };
        // A profile that's gone has nowhere to keep them.
        let Ok(mut loaded) = self.load_profile(&profile) else {
            return Ok(());
        };
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let setup = self.config_setup(game)?;
        if setup.defs.is_empty() {
            return Ok(());
        }
        let roots = state.roots();
        let snapshots = self.paths.generated().join(target.key()).join("config");
        let warn = |message: String| self.reporter.report(Event::Warning(message));
        let mut changed_profile = false;
        let mut changed_state = false;
        for (dest, entry) in &mut state.files {
            let Some(def) = setup.defs.find(dest).filter(|_| entry.rewritten) else {
                continue;
            };
            let path = roots.path(dest)?;
            let Ok(hash) = ContentHash::of_file(&path) else {
                continue;
            };
            if hash == entry.hash {
                continue;
            }
            let read_both = || -> Result<Edits> {
                let hook = self.config_format(&def.format)?;
                let before = read(hook, &setup, dest, &fs::read(&entry.src).at(&entry.src)?)?;
                let now = read(hook, &setup, dest, &fs::read(&path).at(&path)?)?;
                let (now_edits, _) = checked_edits(&now, &now.values());
                Ok(config::changed(&before.values(), &now_edits))
            };
            let changes = match read_both() {
                Ok(changes) => changes,
                Err(e) => {
                    warn(format!(
                        "settings changed in game in {dest} couldn't be kept, since {e}"
                    ));
                    continue;
                }
            };
            if !changes.is_empty() {
                let key = loaded
                    .config
                    .keys()
                    .find(|p| same_dest(p, dest))
                    .cloned()
                    .unwrap_or_else(|| dest.clone());
                let edits = loaded.config.entry(key).or_default();
                for (group, values) in changes {
                    edits.entry(group).or_default().extend(values);
                }
                changed_profile = true;
            }
            // What the game made of it is now what's deployed, so the same
            // changes aren't read back again, and deploying puts the
            // profile's version back.
            let snapshot = dest.path.under(
                &snapshots
                    .join(format!("read-back-{hash}"))
                    .join(dest.root.as_str()),
            );
            if let Some(dir) = snapshot.parent() {
                fs::create_dir_all(dir).at(dir)?;
            }
            fs::copy(&path, &snapshot).at(&snapshot)?;
            entry.src = snapshot;
            entry.hash = hash;
            changed_state = true;
        }
        if changed_profile {
            self.save_profile(&profile, &loaded)?;
        }
        if changed_state {
            fsutil::write_json(&self.deploy_state_file(target), &state)?;
        }
        Ok(())
    }

    /// Makes deploying apply the settings `loaded` changes in config files:
    /// marks placements of config files as ones the game rewrites (so
    /// they're copied, and read back before they're replaced), and adds a
    /// placement of each changed file, written into patchwork's own folder,
    /// over the mod's or the game's own copy.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn place_config(
        &self,
        loaded: &Profile,
        ctx: &DeployContext<'_>,
        target: &DeployTarget,
        inputs: ContentHash,
        prev: &DeployState,
        roots: &ResolvedRoots,
        placements: &mut Vec<Resolved>,
        warnings: &mut Vec<String>,
    ) -> Result<()> {
        let setup = self.config_setup_with(ctx)?;
        if setup.defs.is_empty() {
            if !loaded.config.is_empty() {
                warnings.push(format!(
                    "{} has no config files, so the settings the profile changes in them aren't applied",
                    ctx.game.name
                ));
            }
            return Ok(());
        }
        for p in placements.iter_mut() {
            if setup.defs.find(&p.dst).is_some() {
                p.rewritten = true;
            }
        }
        if loaded.config.is_empty() {
            return Ok(());
        }
        let out = self
            .paths
            .generated()
            .join(target.key())
            .join("config")
            .join(inputs.to_string());
        let all_roots = roots.roots();
        let mut added = Vec::new();
        for (path, edits) in &loaded.config {
            let Some(def) = setup.defs.find(path) else {
                warnings.push(format!(
                    "{path} isn't a config file for {} any more, so the settings the profile changes in it aren't applied",
                    ctx.game.name
                ));
                continue;
            };
            let winner = placements.iter().rev().find(|p| same_dest(&p.dst, path));
            let (dst, src, mod_name) = match winner {
                Some(p) => (p.dst.clone(), p.src.clone(), p.mod_name.clone()),
                None => match original(&all_roots, prev, path)? {
                    Some(src) => (path.clone(), src, String::new()),
                    None => {
                        warnings.push(format!(
                            "{path} isn't there yet, so the settings the profile changes in it \
                             aren't applied; start the game once so it's made, then deploy again"
                        ));
                        continue;
                    }
                },
            };
            let made = (|| -> Result<(Option<Vec<u8>>, Vec<String>)> {
                let hook = self.config_format(&def.format)?;
                let bytes = fs::read(&src).at(&src)?;
                let contents = read(hook, &setup, path, &bytes)?;
                let (kept, problems) = checked_edits(&contents, edits);
                if kept.is_empty() {
                    return Ok((None, problems));
                }
                let config = setup.config_for(&def.format);
                let ctx = ConfigContext::new(path, config);
                Ok((
                    Some(hook.hook.write(&ctx, &bytes, &answers(&kept))?),
                    problems,
                ))
            })();
            let bytes = match made {
                Ok((bytes, problems)) => {
                    warnings.extend(
                        problems
                            .into_iter()
                            .map(|p| format!("{path}: {p}, so it isn't changed")),
                    );
                    match bytes {
                        Some(bytes) => bytes,
                        None => continue,
                    }
                }
                Err(e) => {
                    warnings.push(format!(
                        "the settings the profile changes in {path} aren't applied, since {e}"
                    ));
                    continue;
                }
            };
            let file = dst.path.under(&out.join(dst.root.as_str()));
            // Written again each time, since the game's own copy can
            // change without the deploy's inputs changing; but only if it
            // has, since it might be deployed already.
            if fs::read(&file).ok().as_deref() != Some(&bytes[..]) {
                if let Some(dir) = file.parent() {
                    fs::create_dir_all(dir).at(dir)?;
                }
                fs::write(&file, &bytes).at(&file)?;
            }
            if let Err((_, message)) = roots.check(&dst) {
                warnings.push(message);
                continue;
            }
            added.push(Resolved {
                dst,
                src: file,
                mod_name,
                rewritten: true,
            });
        }
        placements.extend(added);
        Ok(())
    }

    /// Reads settings changed in game back into `profile`, if it's the one
    /// deployed to its target.
    fn read_back_for(&self, profile: &ProfileRef) -> Result<()> {
        let loaded = self.load_profile(profile)?;
        let target = self.deploy_target(&loaded)?;
        let deployed = self.deploy_state(&target)?.profile;
        if deployed.as_deref() == Some(&profile.to_string()) {
            self.read_back_config(&target)?;
        }
        Ok(())
    }

    /// The config files of `game`, and what's needed to read them.
    fn config_setup<'a>(&self, game: &'a GameDef) -> Result<Setup<'a>> {
        let registry = self.registry();
        let (config, _, _) = self.deploy_config(game)?;
        let ctx = DeployContext {
            game,
            config: &config,
            registry: &registry,
        };
        let mut defs = game.config.clone();
        defs.extend(self.deployer(&game.deployer.kind)?.config_files(&ctx)?);
        Ok(Setup {
            defs: ConfigDefs::new(defs)?,
            game,
        })
    }

    fn config_setup_with<'a>(&self, ctx: &DeployContext<'a>) -> Result<Setup<'a>> {
        let mut defs = ctx.game.config.clone();
        defs.extend(self.deployer(&ctx.game.deployer.kind)?.config_files(ctx)?);
        Ok(Setup {
            defs: ConfigDefs::new(defs)?,
            game: ctx.game,
        })
    }

    fn config_format(&self, id: &str) -> Result<&Hook<dyn ConfigFormat>> {
        self.config_formats
            .get(id)
            .ok_or_else(|| Error::invalid(format!("this build can't read `{id}` config files")))
    }

    /// The file deployed to `path` (in the spelling it has), its format's
    /// id, where it's read from, and what it has.
    fn config_contents(
        &self,
        loaded: &Profile,
        setup: &Setup<'_>,
        path: &Dest,
    ) -> Result<(Dest, String, Base, Contents)> {
        let def = setup.defs.find(path).ok_or_else(|| {
            Error::invalid(format!(
                "{path} isn't a config file for {}",
                setup.game.name
            ))
        })?;
        let bases = self.config_bases(loaded, setup)?;
        let (path, base) = bases
            .into_iter()
            .find(|(p, _)| same_dest(p, path))
            .ok_or_else(|| {
                Error::invalid(format!(
                    "{path} isn't there; if the game makes it, start the game once first"
                ))
            })?;
        let hook = self.config_format(&def.format)?;
        let bytes = fs::read(&base.src).at(&base.src)?;
        let contents = read(hook, setup, &path, &bytes)?;
        Ok((path, def.format.clone(), base, contents))
    }

    /// Every config file `loaded` could deploy, and where its settings
    /// are read from: the copy of the last of its enabled, locked mods to
    /// deploy it, or else the game's own.
    fn config_bases(&self, loaded: &Profile, setup: &Setup<'_>) -> Result<BTreeMap<Dest, Base>> {
        let target = self.deploy_target(loaded)?;
        let Some(resolved) = self.resolve_roots(&target)? else {
            return Err(not_installed(setup.game));
        };
        let roots = resolved.roots();
        let state = self.deploy_state(&target)?;
        let mut bases: BTreeMap<Dest, Base> = BTreeMap::new();

        if let Some(lock) = self.load_lock(&loaded.reference())? {
            let game = setup.game;
            let deployer = self.deployer(&game.deployer.kind)?;
            let (config, _, _) = self.deploy_config(game)?;
            let registry = self.registry();
            let ctx = DeployContext {
                game,
                config: &config,
                registry: &registry,
            };
            let files = GameFiles::new(roots.game_path(), &state);
            let order = deploy_order(loaded, &lock);
            let (mods, _) = self.installed(loaded, game, Some(&files), &order)?;
            let desired = deployer.plan(&ctx, &mods)?;
            let placements =
                deploy::resolve(deployer.kind(), desired.placements, &mods, None, &resolved)?;
            for p in placements {
                if setup.defs.find(&p.dst).is_none() {
                    continue;
                }
                let key = bases
                    .keys()
                    .find(|k| same_dest(k, &p.dst))
                    .cloned()
                    .unwrap_or(p.dst);
                let from = (!p.mod_name.is_empty()).then_some(p.mod_name);
                bases.insert(key, Base { src: p.src, from });
            }
        }

        for (root, folder) in setup.defs.folders() {
            let Some(root_path) = roots.get(&root) else {
                continue;
            };
            let dir = if folder.is_empty() {
                root_path.to_owned()
            } else {
                root_path.join(&folder)
            };
            for entry in walkdir::WalkDir::new(&dir)
                .sort_by_file_name()
                .into_iter()
                .flatten()
            {
                if !entry.file_type().is_file() {
                    continue;
                }
                let Some(rel) = entry
                    .path()
                    .strip_prefix(root_path)
                    .ok()
                    .and_then(|rel| RelPath::new(&rel.to_string_lossy()).ok())
                else {
                    continue;
                };
                let dest = Dest::new(root.clone(), rel);
                if setup.defs.find(&dest).is_none() || bases.keys().any(|k| same_dest(k, &dest)) {
                    continue;
                }
                if let Some(src) = original(&roots, &state, &dest)? {
                    bases.insert(dest, Base { src, from: None });
                }
            }
        }
        Ok(bases)
    }
}

impl Setup<'_> {
    /// Format `id`'s `[ext.<extension>.<name>]` table, or an empty one.
    fn config_for(&self, id: &str) -> &toml::Table {
        static EMPTY: std::sync::LazyLock<toml::Table> = std::sync::LazyLock::new(toml::Table::new);
        id.split_once('.')
            .and_then(|(ext, name)| self.game.hook_config(ext, name))
            .unwrap_or(&EMPTY)
    }
}

/// `bytes`, the file deployed to `path`, as `hook` reads it.
fn read(
    hook: &Hook<dyn ConfigFormat>,
    setup: &Setup<'_>,
    path: &Dest,
    bytes: &[u8],
) -> Result<Contents> {
    let config = setup.config_for(&crate::ext::hook_id(hook.ext, hook.name));
    hook.hook.read(&ConfigContext::new(path, config), bytes)
}

/// The game's own copy of `dest`, as it is without what patchwork
/// deployed, like [`GameFiles::original`] for any root.
fn original(roots: &Roots, state: &DeployState, dest: &Dest) -> Result<Option<PathBuf>> {
    let Ok(path) = roots.path(dest) else {
        return Ok(None);
    };
    Ok(match state.files.get(dest) {
        Some(deployed) if !changed_since_deployed(&path, deployed) => deployed.backup.clone(),
        _ => path.is_file().then_some(path),
    })
}

/// What `loaded` changes in the file deployed to `path`, matched ignoring
/// case, since mods rarely agree on it.
fn edits_for<'a>(loaded: &'a Profile, path: &Dest) -> Option<&'a Edits> {
    loaded
        .config
        .iter()
        .find(|(p, _)| same_dest(p, path))
        .map(|(_, edits)| edits)
}

fn same_dest(a: &Dest, b: &Dest) -> bool {
    a.root == b.root && a.path.as_str().eq_ignore_ascii_case(b.path.as_str())
}

/// `answers`' values as edits; their picks mean nothing for config files.
fn edits_of(answers: &[Answer]) -> Edits {
    let mut edits = Edits::new();
    for answer in answers {
        if !answer.values.is_empty() {
            edits
                .entry(answer.group.clone())
                .or_default()
                .extend(answer.values.clone());
        }
    }
    edits
}
