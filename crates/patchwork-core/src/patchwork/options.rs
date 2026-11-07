//! What mods install once extensions' transforms have had their say, and
//! the choices the user makes about mods; see [`crate::options`].

use std::cell::OnceCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use serde::Serialize;

use super::{Patchwork, deploy_order};
use crate::deploy::{GameFiles, InstalledMod, ModFile};
use crate::ext::{self, Hook};
use crate::game::GameDef;
use crate::lock::LockedMod;
use crate::options::{Answer, Form, ModContext, ModOptions, ModPeek, ModTransform, checked};
use crate::profile::{Profile, ProfileRef, no_such_mod};
use crate::relpath::RelPath;
use crate::{Error, Result};

/// Choices that can be made about a mod, from
/// [`Patchwork::mods_with_options`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModOptionsInfo {
    /// `<extension>.<options>`.
    pub id: String,
    pub title: String,
    /// Whether anything's been picked, rather than the mod getting the
    /// defaults.
    pub saved: bool,
}

/// A mod's choices as a frontend shows them, from
/// [`Patchwork::mod_options`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModOptionsView {
    /// `<extension>.<options>`.
    pub id: String,
    /// What the choices are called, like "Installer".
    pub title: String,
    /// Whether the profile has anything saved for them.
    pub saved: bool,
    pub form: Form,
    /// Where items' pictures are in the store, by their path in the mod.
    pub images: BTreeMap<String, PathBuf>,
}

impl Patchwork {
    /// Every hook name extension `ext` has, of every kind.
    pub(super) fn hook_names(&self, ext: &str) -> Vec<&'static str> {
        let mut names = self.list_names(ext);
        names.extend(
            self.transforms
                .iter()
                .filter(|h| h.ext == ext)
                .map(|h| h.name),
        );
        names.extend(
            self.options
                .values()
                .filter(|h| h.ext == ext)
                .map(|h| h.name),
        );
        names.extend(self.check_names(ext));
        names.extend(
            self.config_formats
                .values()
                .filter(|h| h.ext == ext)
                .map(|h| h.name),
        );
        names.extend(self.action_names(ext));
        names
    }

    /// `order`'s files, from the store, as `loaded`'s game installs them:
    /// changed by whichever transforms are on for it and apply to each mod,
    /// and with what the extensions that are on keep for each.
    /// `files` are the game's own, if it's been set up. Also returns
    /// problems the transforms found.
    pub(super) fn installed(
        &self,
        loaded: &Profile,
        game: &GameDef,
        files: Option<&GameFiles<'_>>,
        order: &[&LockedMod],
    ) -> Result<(Vec<InstalledMod>, Vec<String>)> {
        let mut mods = order
            .iter()
            .map(|locked| self.packed(locked))
            .collect::<Result<Vec<_>>>()?;
        for m in &mut mods {
            for ext in self.extensions_on(game) {
                let state = self.mod_state(loaded, ext, &m.name)?;
                if !state.is_empty() {
                    m.ext.insert(ext.to_owned(), state);
                }
            }
        }
        let mut warnings = Vec::new();
        let transforms: Vec<&Hook<dyn ModTransform>> = self
            .transforms
            .iter()
            .filter(|h| self.hook_on(game, h.ext, h.name))
            .collect();
        if transforms.is_empty() {
            return Ok((mods, warnings));
        }
        // Worked out from the mods as they're packed, and only then put in
        // place, so each transform sees the same mods whatever came before.
        let packed = mods.clone();
        let empty = toml::Table::new();
        for m in &mut mods {
            for hook in &transforms {
                let state = self.mod_state(loaded, hook.ext, &m.name)?;
                let config = game.hook_config(hook.ext, hook.name).unwrap_or(&empty);
                let ctx = ModContext::new(game, config, hook.ext, &state, &packed, files);
                if !hook.hook.applies(&ctx, &ModPeek::of(m)) {
                    continue;
                }
                match hook.hook.transform(&ctx, m) {
                    Ok(done) => {
                        warnings.extend(done.warnings);
                        m.files = transformed(m, done.files, &mut warnings);
                    }
                    Err(e) => warnings.push(format!(
                        "`{}`'s files are deployed as they're packed, since {e}",
                        m.name
                    )),
                }
            }
        }
        Ok((mods, warnings))
    }

    /// The names of `profile`'s mods that have choices to make, with what
    /// they are, among those that are locked and unpacked. Cheap enough to
    /// call whenever the profile is shown, since it doesn't unpack or list
    /// anything.
    pub fn mods_with_options(
        &self,
        profile: &ProfileRef,
    ) -> Result<BTreeMap<String, Vec<ModOptionsInfo>>> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let hooks = self.options_on(game);
        let mut found: BTreeMap<String, Vec<ModOptionsInfo>> = BTreeMap::new();
        if hooks.is_empty() {
            return Ok(found);
        }
        let Some(lock) = self.load_lock(profile)? else {
            return Ok(found);
        };
        let empty = toml::Table::new();
        for locked in &lock.mods {
            let Some(dir) = self.store.unpacked(&locked.hash, locked.format) else {
                continue;
            };
            let peek = ModPeek::dir(&locked.name, locked.kind.as_deref(), &dir);
            for (id, hook) in &hooks {
                let state = self.mod_state(&loaded, hook.ext, &locked.name)?;
                let config = game.hook_config(hook.ext, hook.name).unwrap_or(&empty);
                let ctx = ModContext::new(game, config, hook.ext, &state, &[], None);
                if hook.hook.applies(&ctx, &peek) {
                    found
                        .entry(locked.name.clone())
                        .or_default()
                        .push(ModOptionsInfo {
                            id: (*id).to_owned(),
                            title: hook.hook.title().to_owned(),
                            saved: ctx.saved(),
                        });
                }
            }
        }
        Ok(found)
    }

    /// `name`'s choices `id` in `profile` (or its only ones, if `None`),
    /// worked out for `answers`, or for what's saved if `None`. Only locked
    /// mods count, since the choices come from the mod's files.
    pub fn mod_options(
        &self,
        profile: &ProfileRef,
        name: &str,
        id: Option<&str>,
        answers: Option<&[Answer]>,
    ) -> Result<ModOptionsView> {
        self.with_options(profile, name, id, |id, hook, ctx, m| {
            let mut form = hook.hook.form(ctx, m, answers)?;
            // They're only checked when saved, so the user sees what's
            // wrong as they go.
            if let Some(Err(e)) = answers.map(|answers| checked(&form, answers)) {
                form.warnings.push(e.to_string());
            }
            let images = form
                .pages
                .iter()
                .flat_map(|p| &p.groups)
                .flat_map(|g| g.items())
                .filter_map(|i| i.image.as_ref())
                .filter_map(|rel| {
                    let file = m.files.iter().find(|f| f.rel == *rel)?;
                    Some((rel.to_string(), file.src.clone()))
                })
                .collect();
            Ok(ModOptionsView {
                id: id.to_owned(),
                title: hook.hook.title().to_owned(),
                saved: ctx.saved(),
                form,
                images,
            })
        })
    }

    /// Saves `answers` as what's picked in `name`'s choices `id` (or its
    /// only ones), or with `None` forgets what was, so it gets the
    /// defaults. Forgetting clears everything the choices' extension keeps
    /// for the mod. Values are checked against the fields they're for
    /// first, so the extension only gets ones they can have.
    pub fn set_mod_options(
        &self,
        profile: &ProfileRef,
        name: &str,
        id: Option<&str>,
        answers: Option<&[Answer]>,
    ) -> Result<()> {
        let (ext, table) = self.with_options(profile, name, id, |_, hook, ctx, m| {
            let table = match answers {
                None => toml::Table::new(),
                Some(answers) if answers.iter().all(|a| a.values.is_empty()) => {
                    hook.hook.save(ctx, m, answers)?
                }
                Some(answers) => {
                    let form = hook.hook.form(ctx, m, Some(answers))?;
                    hook.hook.save(ctx, m, &checked(&form, answers)?)?
                }
            };
            Ok((hook.ext, table))
        })?;
        let mut loaded = self.load_profile(profile)?;
        let label = loaded.label();
        let tables = &mut loaded
            .find_mut(name)
            .ok_or_else(|| no_such_mod(&label, name))?
            .ext;
        ext::write_state(self.extension(ext), tables, &table, &[])?;
        self.save_profile(profile, &loaded)
    }

    /// Calls `f` with `name`'s choices `id` in `profile` (or its only
    /// ones): their id and hook, the context to call it with, and the mod.
    fn with_options<T>(
        &self,
        profile: &ProfileRef,
        name: &str,
        id: Option<&str>,
        f: impl FnOnce(&str, &Hook<dyn ModOptions>, &ModContext<'_>, &InstalledMod) -> Result<T>,
    ) -> Result<T> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let lock = self.load_lock(profile)?;
        let m = self.locked_mod(&loaded, lock.as_ref(), name)?;
        let (id, hook) = self.options_for(game, &m, id)?;
        let state = self.mod_state(&loaded, hook.ext, name)?;
        let target = self.deploy_target(&loaded)?;
        let root = self.target_root(&target)?;
        let deployed = self.deploy_state(&target)?;
        let files = root.as_deref().map(|root| GameFiles::new(root, &deployed));
        // Listing every mod's files takes a while, and this is called each
        // time a pick changes, so it's only done if the hook asks.
        let mods = OnceCell::new();
        let load = || {
            lock.iter()
                .flat_map(|lock| deploy_order(&loaded, lock))
                .filter_map(|l| self.packed(l).ok())
                .collect()
        };
        let empty = toml::Table::new();
        let config = game.hook_config(hook.ext, hook.name).unwrap_or(&empty);
        let ctx = ModContext::lazy(
            game,
            config,
            hook.ext,
            &state,
            (&mods, &load),
            files.as_ref(),
        );
        f(id, hook, &ctx, &m)
    }

    /// The options that are on for `def`, in id order.
    fn options_on(&self, def: &GameDef) -> Vec<(&str, &Hook<dyn ModOptions>)> {
        self.options
            .iter()
            .filter(|(_, hook)| self.hook_on(def, hook.ext, hook.name))
            .map(|(id, hook)| (id.as_str(), hook))
            .collect()
    }

    /// Options `id` for `m`, or its only ones if `None`.
    fn options_for<'a>(
        &'a self,
        def: &GameDef,
        m: &InstalledMod,
        id: Option<&str>,
    ) -> Result<(&'a str, &'a Hook<dyn ModOptions>)> {
        let empty = toml::Table::new();
        let state = toml::Table::new();
        let applies = |hook: &Hook<dyn ModOptions>| {
            let config = def.hook_config(hook.ext, hook.name).unwrap_or(&empty);
            let ctx = ModContext::new(def, config, hook.ext, &state, &[], None);
            hook.hook.applies(&ctx, &ModPeek::of(m))
        };
        let mut found: Vec<_> = self
            .options_on(def)
            .into_iter()
            .filter(|(hook_id, _)| id.is_none_or(|id| id == *hook_id))
            .filter(|(_, hook)| applies(hook))
            .collect();
        match (found.len(), id) {
            (1, _) => Ok(found.remove(0)),
            (0, Some(id)) => Err(Error::invalid(format!(
                "`{}` hasn't any `{id}` choices for {}",
                m.name, def.name
            ))),
            (0, None) => Err(Error::invalid(format!(
                "`{}` hasn't any choices to make for {}",
                m.name, def.name
            ))),
            _ => {
                let ids: Vec<&str> = found.iter().map(|(id, _)| *id).collect();
                Err(Error::invalid(format!(
                    "`{}` has more than one set of choices; say which of {}",
                    m.name,
                    ids.join(", ")
                )))
            }
        }
    }

    /// `name`'s files as they're packed, if it's in `loaded` and locked.
    fn locked_mod(
        &self,
        loaded: &Profile,
        lock: Option<&crate::lock::Lockfile>,
        name: &str,
    ) -> Result<InstalledMod> {
        let label = loaded.label();
        if !loaded.mods.iter().any(|m| m.name == name) {
            return Err(no_such_mod(&label, name));
        }
        let Some(locked) = lock.iter().flat_map(|l| &l.mods).find(|l| l.name == name) else {
            return Err(Error::invalid(format!(
                "`{name}` hasn't been downloaded yet; lock {label} first"
            )));
        };
        self.packed(locked)
    }

    /// What extension `ext` keeps for `loaded`'s mod `name`, at its current
    /// version. Nothing, for a mod that's only there as a dependency.
    fn mod_state(&self, loaded: &Profile, ext: &str, name: &str) -> Result<toml::Table> {
        match loaded.mods.iter().find(|m| m.name == name) {
            Some(m) => ext::read_state(self.extension(ext), &m.ext, &[]),
            None => Ok(toml::Table::new()),
        }
    }
}

/// `m`'s files as a transform says to install them, leaving out ones that
/// aren't in it.
fn transformed(
    m: &InstalledMod,
    files: Vec<(RelPath, RelPath)>,
    warnings: &mut Vec<String>,
) -> Vec<ModFile> {
    let src: HashMap<&RelPath, &PathBuf> = m.files.iter().map(|f| (&f.rel, &f.src)).collect();
    files
        .into_iter()
        .filter_map(|(rel, from)| match src.get(&from) {
            Some(src) => Some(ModFile {
                rel,
                src: src.to_path_buf(),
            }),
            None => {
                warnings.push(format!(
                    "`{}` would install {from} to {rel}, but it hasn't got {from}",
                    m.name
                ));
                None
            }
        })
        .collect()
}
