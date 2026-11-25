//! Changing settings in config files; see [`crate::config`].

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::personal::personal_include;
use super::{Patchwork, deploy_order, not_installed};
use crate::config::{
    self, ConfigContext, ConfigDefs, ConfigFormat, Contents, Edits, FileEdits, answers,
    checked_edits, edits_in, overlay, same_dest,
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
use crate::personal::{PERSONAL_NAME, Personal};
use crate::profile::{Include, Profile, ProfileRef};
use crate::relpath::RelPath;
use crate::set::{merge_config, own_config, sets_config};
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
    /// The profile's mod its settings are most likely for, so frontends
    /// can offer them from the mod: the one whose copy it is, or for one
    /// in the game's folders, the only mod named like its title or file
    /// name (BepInEx names them by plugin GUID, like `com.author.name`).
    pub owner: Option<String>,
    /// Whether the profile changes any of its settings, itself or
    /// through a set it includes.
    pub edited: bool,
    /// The shared sets the profile includes that change its settings, by
    /// name.
    pub sets: Vec<String>,
    /// The profile changes its settings, but it isn't there (yet), so
    /// they aren't applied.
    pub missing: bool,
}

/// Where changes to a profile's config file settings are saved: in the
/// profile, or in a shared set it includes, for every profile that does.
/// A synced profile's personal layer is the set whose id is the profile's
/// (see [`crate::personal`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SaveTo {
    Profile,
    Set(Uuid),
}

/// A shared set a config file's settings can be saved in, from
/// [`ConfigView::sets`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigSet {
    pub id: Uuid,
    pub name: String,
    /// Whether it changes any of the file's settings.
    pub edited: bool,
    /// Whether it's the profile's personal layer or a set it includes
    /// personally, so its settings are only on this person's computers,
    /// over the profile's own.
    pub personal: bool,
}

/// A setting a shared set changes, from [`ConfigView::shared`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedSetting {
    pub group: String,
    pub field: String,
    /// The set's id and name; the one whose value is used, if more than
    /// one changes it.
    pub id: Uuid,
    pub set: String,
    /// Whether it's the personal layer or a set included personally,
    /// whose settings win over the profile's own.
    pub personal: bool,
    /// The profile changes it too. For a shared set, the profile's own
    /// value is used; for a personal one, the personal value is, over the
    /// profile's.
    pub overridden: bool,
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
    /// Whether the profile changes any of its settings, itself or through
    /// a set it includes.
    pub saved: bool,
    /// Whether the profile changes any of its settings itself.
    pub own: bool,
    /// The sets the profile includes, which its settings can be saved in
    /// instead of the profile, and a synced profile's personal layer.
    pub sets: Vec<ConfigSet>,
    /// Where saving puts them unless told otherwise: the set whose linked
    /// folder the file's mod is in, or else the profile.
    pub save_to: SaveTo,
    /// The settings that come from the sets.
    pub shared: Vec<SharedSetting>,
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
            let owner = base
                .from
                .clone()
                .or_else(|| owner_by_name(&loaded, path, title.as_deref()));
            found.push(ConfigFileInfo {
                path: path.clone(),
                format: def.format.clone(),
                title,
                from: base.from.clone(),
                owner,
                edited: edits_for(&loaded, path).is_some(),
                sets: sets_editing(&loaded, path),
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
                    owner: owner_by_name(&loaded, path, None),
                    edited: true,
                    sets: sets_editing(&loaded, path),
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
        let save_to = default_save_to(&loaded, &path, Some(&base), contents.title.as_deref());
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
        let own = own_config(&loaded);
        let own = edits_in(&own, &path);
        let personal = sets_config(&loaded, true);
        let has = |edits: Option<&Edits>, group: &str, field: &str| {
            edits.is_some_and(|o| o.get(group).is_some_and(|v| v.contains_key(field)))
        };
        let mut shared = Vec::new();
        for include in in_order(&loaded) {
            let Some(linked) = &include.linked else {
                continue;
            };
            for (group, values) in edits_in(&linked.config, &path).into_iter().flatten() {
                for field in values.keys() {
                    // a later set's wins
                    shared.retain(|s: &SharedSetting| !(s.group == *group && s.field == *field));
                    shared.push(SharedSetting {
                        group: group.clone(),
                        field: field.clone(),
                        id: linked.folder.id,
                        set: linked.folder.name.clone(),
                        personal: include.personal,
                        overridden: has(own, group, field)
                            || !include.personal && has(edits_in(&personal, &path), group, field),
                    });
                }
            }
        }
        Ok(ConfigView {
            path: path.clone(),
            format,
            from: base.from,
            saved: saved.is_some(),
            own: own.is_some(),
            sets: config_sets(&loaded, &path, self.is_synced(profile)),
            save_to,
            shared,
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
    /// its sets (if any) have it. Only settings that differ from what the
    /// file (and its sets) have are kept. Values are checked against their
    /// settings first.
    ///
    /// `to` says where they're kept (by default, see [`ConfigView::save_to`]).
    /// Saved to a shared set, they're the set's for every profile that
    /// includes it, and the profile stops changing the file's settings
    /// itself, since `answers` are all of them, as the profile has them.
    ///
    /// Saved to the personal layer of a synced profile (or a set it
    /// includes personally), they're only on this person's computers, over
    /// the profile's own, which stay as they are, so nothing changes for
    /// anyone else. Saved anywhere else, settings that the personal layer
    /// changes are kept there as they were, since the form has the
    /// personal values for them and those mustn't go to everyone.
    pub fn set_config(
        &self,
        profile: &ProfileRef,
        path: &Dest,
        answers: Option<&[Answer]>,
        to: Option<SaveTo>,
    ) -> Result<()> {
        // So that settings changed in game don't come back afterwards.
        self.read_back_for(profile)?;
        let mut loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let mut own = own_config(&loaded);
        if to == Some(SaveTo::Set(loaded.id)) && loaded.personal().is_none() {
            if !self.is_synced(profile) {
                return Err(not_synced(&loaded));
            }
            loaded.includes.push(personal_include(&loaded));
        }
        let Some(answers) = answers else {
            let to = match to {
                Some(to) => to,
                None => self
                    .config_setup(game)
                    .and_then(|setup| self.config_contents(&loaded, &setup, path))
                    .map(|(path, _, base, contents)| {
                        default_save_to(&loaded, &path, Some(&base), contents.title.as_deref())
                    })
                    // one that isn't there
                    .unwrap_or_else(|_| default_save_to(&loaded, path, None, None)),
            };
            match to {
                SaveTo::Profile => {
                    let before = own.len();
                    own.retain(|p, _| !same_dest(p, path));
                    if own.len() != before {
                        merge_config(&mut loaded, &own);
                        self.save_profile(profile, &loaded)?;
                    }
                }
                SaveTo::Set(id) => {
                    let mut set = self.included_set(&loaded, id)?;
                    let before = set.config.len();
                    set.config.retain(|p, _| !same_dest(p, path));
                    if set.config.len() != before {
                        self.save_set_config(&set)?;
                        if is_personal(&loaded, id) {
                            self.profile_changed(profile, false);
                        }
                    }
                }
            }
            return Ok(());
        };
        let setup = self.config_setup(game)?;
        let (path, _, base, contents) = self.config_contents(&loaded, &setup, path)?;
        let (kept, problems) = checked_edits(&contents, &edits_of(answers));
        if !problems.is_empty() {
            return Err(Error::invalid(problems.join("; ")));
        }
        let to = to.unwrap_or_else(|| {
            default_save_to(&loaded, &path, Some(&base), contents.title.as_deref())
        });
        let personal = match to {
            SaveTo::Set(id) => is_personal(&loaded, id),
            SaveTo::Profile => false,
        };
        // What it'd be without what's being saved: the file, with the sets'
        // changes (but the one saved to) over it, and the profile's own
        // under personal ones. The personal layer's are over everything
        // else, so where it changes a setting, nothing under it does.
        let mut under = FileEdits::from([(path.clone(), contents.values())]);
        for include in in_order(&loaded) {
            if include.personal && personal {
                overlay(&mut under, &own);
            }
            if let Some(linked) = &include.linked
                && to != SaveTo::Set(linked.folder.id)
            {
                overlay(&mut under, &linked.config);
            }
        }
        let changed = config::changed(&under[&path], &kept);
        // Where the personal layer changes a setting, the form has its
        // value, so anywhere else keeps what it had.
        let cover = match personal {
            true => Edits::new(),
            false => edits_in(&sets_config(&loaded, true), &path)
                .cloned()
                .unwrap_or_default(),
        };
        if !personal {
            let before = edits_in(&own, &path).cloned();
            own.retain(|p, _| !same_dest(p, &path));
            let edits = match to {
                SaveTo::Profile => keep_covered(&changed, before.as_ref(), &cover),
                // the profile's own that are hidden stay
                SaveTo::Set(_) => keep_covered(&Edits::new(), before.as_ref(), &cover),
            };
            if !edits.is_empty() {
                own.insert(path.clone(), edits);
            }
        }
        match to {
            SaveTo::Profile => {}
            SaveTo::Set(id) => {
                let mut set = self.included_set(&loaded, id)?;
                let edits = keep_covered(&changed, edits_in(&set.config, &path), &cover);
                set.config.retain(|p, _| !same_dest(p, &path));
                if !edits.is_empty() {
                    set.config.insert(path, edits);
                }
                self.save_set_config(&set)?;
                if let Some(linked) = loaded
                    .includes
                    .iter_mut()
                    .find(|i| i.set == id)
                    .and_then(|i| i.linked.as_mut())
                {
                    linked.config = set.config;
                }
            }
        }
        merge_config(&mut loaded, &own);
        self.save_profile(profile, &loaded)
    }

    /// The set `id`, if `loaded` includes it, or its personal layer
    /// (which may have nothing in it yet).
    fn included_set(&self, loaded: &Profile, id: Uuid) -> Result<crate::set::Set> {
        if id == loaded.id {
            if loaded.personal().is_none() {
                return Err(not_synced(loaded));
            }
            return Ok(self
                .load_personal_set(id)?
                .unwrap_or_else(|| Personal::new(&loaded.game).set(id)));
        }
        if loaded.include(id).is_none_or(|i| i.linked.is_none()) {
            return Err(Error::invalid(format!(
                "{} doesn't include the set {id}",
                loaded.label()
            )));
        }
        self.load_set(id)?
            .ok_or_else(|| Error::invalid(format!("there's no set with id {id}")))
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
                // what the personal layer gives goes back to it, since
                // it'd win over the profile's own anyway
                let (personal, own) = split_personal(&loaded, dest, changes);
                for (id, changes) in personal {
                    if let Some(mut set) = self.load_set(id)? {
                        add_edits(&mut set.config, dest, changes);
                        self.save_set_config(&set)?;
                        changed_profile = true;
                    }
                }
                if !own.is_empty() {
                    add_edits(&mut loaded.config, dest, own);
                    changed_profile = true;
                }
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
        let own = own_config(loaded);
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
                    // Only the profile's own: a set's can be for a mod
                    // that's off in this profile.
                    None if edits_in(&own, path).is_none() => continue,
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
    edits_in(&loaded.config, path)
}

/// The names of the sets `loaded` includes that change settings in `path`.
fn sets_editing(loaded: &Profile, path: &Dest) -> Vec<String> {
    loaded
        .includes
        .iter()
        .filter_map(|i| i.linked.as_ref())
        .filter(|l| edits_in(&l.config, path).is_some())
        .map(|l| l.folder.name.clone())
        .collect()
}

/// The sets `loaded` includes, for saving `path`'s settings in, in the
/// order their settings go over each other. If it's `synced`, its personal
/// layer is among them even while there's nothing in it.
fn config_sets(loaded: &Profile, path: &Dest, synced: bool) -> Vec<ConfigSet> {
    let mut sets: Vec<ConfigSet> = in_order(loaded)
        .filter_map(|i| Some((i, i.linked.as_ref()?)))
        .map(|(i, l)| ConfigSet {
            id: l.folder.id,
            name: l.folder.name.clone(),
            edited: edits_in(&l.config, path).is_some(),
            personal: i.personal,
        })
        .collect();
    if synced && loaded.personal().is_none() {
        sets.push(ConfigSet {
            id: loaded.id,
            name: PERSONAL_NAME.to_owned(),
            edited: false,
            personal: true,
        });
    }
    sets
}

/// Where changes to `path`'s settings go unless said otherwise: the set
/// whose linked folder its mod is in (the one whose copy it is, or that
/// it's named like), as editing the folder's mods changes the set, or
/// the personal layer if the mod's in that. Otherwise the personal layer
/// (or a set included personally) if it changes the file's settings now,
/// or the profile, unless only a shared set does.
fn default_save_to(
    loaded: &Profile,
    path: &Dest,
    base: Option<&Base>,
    title: Option<&str>,
) -> SaveTo {
    let owner = base
        .and_then(|b| b.from.clone())
        .or_else(|| owner_by_name(loaded, path, title));
    if let Some(m) = owner.as_deref().and_then(|name| loaded.find(name))
        && loaded.is_linked(m)
        && let Some(id) = m.folder
    {
        return SaveTo::Set(id);
    }
    let sets: Vec<&Include> = in_order(loaded).collect();
    let last = |personal: bool| {
        sets.iter()
            .rev()
            .filter(|i| i.personal == personal)
            .filter_map(|i| i.linked.as_ref())
            .find(|l| edits_in(&l.config, path).is_some())
            .map(|l| SaveTo::Set(l.folder.id))
    };
    // the personal layer's win over the profile's, which win over the
    // shared sets'
    if let Some(to) = last(true) {
        return to;
    }
    if edits_in(&own_config(loaded), path).is_none()
        && let Some(to) = last(false)
    {
        return to;
    }
    SaveTo::Profile
}

/// The one mod in `profile` named like the config file at `path` or its
/// `title`, ignoring case and anything but letters and digits: by the
/// file's name, the last `.`-separated part of it (for a name like
/// `com.author.name`), or its title. A mod goes by its name or the last
/// `-`-separated part of its id (for a Thunderstore-style `Author-Name`).
/// Names under three letters are too likely to match by chance.
fn owner_by_name(profile: &Profile, path: &Dest, title: Option<&str>) -> Option<String> {
    fn key(s: &str) -> Option<String> {
        let key: String = s
            .chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect();
        (key.chars().count() >= 3).then_some(key)
    }
    let file = path.path.as_str().rsplit('/').next().unwrap_or_default();
    let stem = file.rsplit_once('.').map_or(file, |(stem, _)| stem);
    let last = stem.rsplit('.').next().unwrap_or(stem);
    let names: Vec<String> = [Some(stem), Some(last), title]
        .into_iter()
        .flatten()
        .filter_map(key)
        .collect();
    let mut matching = profile.mods.iter().filter(|m| {
        let id = m.id.rsplit(['-', '/']).next().unwrap_or(&m.id);
        [m.name.as_str(), id]
            .into_iter()
            .filter_map(key)
            .any(|k| names.contains(&k))
    });
    let found = matching.next()?;
    matching.next().is_none().then(|| found.name.clone())
}

/// Splits settings changed in game in the file at `path` into those that
/// go back to the personal layer or a set included personally (the last
/// that changes each, by its id), and the rest, which are the profile's.
fn split_personal(loaded: &Profile, path: &Dest, changes: Edits) -> (Vec<(Uuid, Edits)>, Edits) {
    let mut personal: Vec<(Uuid, Edits)> = Vec::new();
    let mut own = Edits::new();
    for (group, values) in changes {
        for (field, value) in values {
            let owner = loaded
                .includes
                .iter()
                .rev()
                .filter(|i| i.personal)
                .filter_map(|i| i.linked.as_ref())
                .find(|l| {
                    edits_in(&l.config, path)
                        .is_some_and(|e| e.get(&group).is_some_and(|v| v.contains_key(&field)))
                })
                .map(|l| l.folder.id);
            let into = match owner {
                Some(id) => match personal.iter().position(|(p, _)| *p == id) {
                    Some(at) => &mut personal[at].1,
                    None => {
                        personal.push((id, Edits::new()));
                        &mut personal.last_mut().expect("just pushed").1
                    }
                },
                None => &mut own,
            };
            into.entry(group.clone()).or_default().insert(field, value);
        }
    }
    (personal, own)
}

/// Adds `changes` to what `config` changes in the file at `path`.
fn add_edits(config: &mut FileEdits, path: &Dest, changes: Edits) {
    let key = config
        .keys()
        .find(|p| same_dest(p, path))
        .cloned()
        .unwrap_or_else(|| path.clone());
    let edits = config.entry(key).or_default();
    for (group, values) in changes {
        edits.entry(group).or_default().extend(values);
    }
}

/// `edits` for a file, but with the settings `cover` changes as `before`
/// has them, if it has them.
fn keep_covered(edits: &Edits, before: Option<&Edits>, cover: &Edits) -> Edits {
    let covers = |group: &str, id: &str| cover.get(group).is_some_and(|v| v.contains_key(id));
    let mut kept = Edits::new();
    let changed = edits.iter().map(|e| (e, false));
    let had = before.into_iter().flatten().map(|e| (e, true));
    for ((group, values), from_before) in changed.chain(had) {
        for (id, value) in values {
            if covers(group, id) == from_before {
                kept.entry(group.clone())
                    .or_default()
                    .insert(id.clone(), value.clone());
            }
        }
    }
    kept
}

/// Whether the set `id` is `loaded`'s personal layer or one it includes
/// personally.
fn is_personal(loaded: &Profile, id: Uuid) -> bool {
    loaded.include(id).is_some_and(|i| i.personal)
}

/// `loaded`'s includes in the order their settings go over each other:
/// shared sets', then the personal ones', which go over the profile's own.
fn in_order(loaded: &Profile) -> impl Iterator<Item = &Include> {
    let (personal, shared): (Vec<&Include>, Vec<&Include>) =
        loaded.includes.iter().partition(|i| i.personal);
    shared.into_iter().chain(personal)
}

fn not_synced(loaded: &Profile) -> Error {
    Error::invalid(format!(
        "{} isn't synced, so it has no personal settings; its own are only on this computer \
         already",
        loaded.label()
    ))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(mods: &[(&str, &str)]) -> Profile {
        let mut p = Profile::new("game", "default");
        p.mods = mods
            .iter()
            .map(|(name, id)| {
                toml::from_str(&format!("name = {name:?}\nsource = \"ts\"\nid = {id:?}")).unwrap()
            })
            .collect();
        p
    }

    #[test]
    fn config_files_go_to_the_mod_named_like_them() {
        let p = profile(&[
            ("R2API", "RiskofThunder-R2API"),
            ("Better UI", "XoXFaby-BetterUI"),
            ("UI", "Someone-UI"),
            ("Fast", "One-Fast"),
            ("Fast Two", "Two-Fast"),
        ]);
        let owner = |path: &str, title| owner_by_name(&p, &path.parse().unwrap(), title);
        // by the last part of a GUID-like name, by title, and by id
        assert_eq!(
            owner("BepInEx/config/com.bepis.r2api.cfg", None).as_deref(),
            Some("R2API")
        );
        assert_eq!(
            owner("BepInEx/config/x.y.cfg", Some("Better UI")).as_deref(),
            Some("Better UI")
        );
        assert_eq!(
            owner("BepInEx/config/BetterUI.cfg", None).as_deref(),
            Some("Better UI")
        );
        // too short to tell, matching two mods, or none
        assert_eq!(owner("BepInEx/config/ui.cfg", None), None);
        assert_eq!(owner("BepInEx/config/fast.cfg", None), None);
        assert_eq!(owner("BepInEx/config/BepInEx.cfg", None), None);
    }
}
