//! The checks that are on for a profile's game, and the issues they and
//! the game's lists find; see [`crate::check`].

use super::{Patchwork, deploy_order};
use crate::Result;
use crate::check::{Check, CheckContext, Problem};
use crate::deploy::{DeployContext, GameFiles, InstalledMod};
use crate::ext::{Hook, hook_id};
use crate::game::GameDef;
use crate::issue::Issue;
use crate::list::List;
use crate::lock::Lockfile;
use crate::profile::Profile;
use crate::source::ArtifactFormat;

impl Patchwork {
    /// The checks that are on for `def`, in id order.
    pub(super) fn checks_on(&self, def: &GameDef) -> Vec<(&str, &Hook<dyn Check>)> {
        self.checks
            .iter()
            .filter(|(_, hook)| def.hook_on(hook.ext, hook.name))
            .map(|(id, hook)| (id.as_str(), hook))
            .collect()
    }

    /// The names of the checks extension `ext` has.
    pub(super) fn check_names(&self, ext: &str) -> impl Iterator<Item = &'static str> {
        self.checks
            .values()
            .filter(move |h| h.ext == ext)
            .map(|h| h.name)
    }

    /// What's wrong with installing `mods` for `game`: the problems with
    /// `lists` (the lists that are on for it), and what its checks find.
    pub(super) fn found_issues(
        &self,
        game: &GameDef,
        files: &GameFiles<'_>,
        lists: &[List],
        mods: &[InstalledMod],
    ) -> Vec<Issue> {
        let mut issues: Vec<Issue> = lists
            .iter()
            .flat_map(|list| {
                list.problems
                    .iter()
                    .map(|p| Issue::list(&list.id, p.clone()))
            })
            .collect();
        let empty = toml::Table::new();
        for (id, hook) in self.checks_on(game) {
            let config = game.hook_config(hook.ext, hook.name).unwrap_or(&empty);
            let ctx = CheckContext::new(game, config, hook.ext, files, lists);
            issues.extend(
                hook.hook
                    .check(&ctx, mods)
                    .into_iter()
                    .map(|p| Issue::check(hook.ext, id, p)),
            );
        }
        issues
    }

    /// What [`Patchwork::found_issues`] finds for `loaded`, from what's
    /// already in the store: if any of its mods would have to be unpacked
    /// first, nothing. Lists that can't be worked out are issues too, so
    /// a broken one doesn't stop the profile being shown.
    pub(super) fn known_found_issues(
        &self,
        loaded: &Profile,
        lock: &Lockfile,
    ) -> Result<Vec<Issue>> {
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let lists = self.lists_on(game);
        if lists.is_empty() && self.checks_on(game).is_empty() {
            return Ok(Vec::new());
        }
        let order = deploy_order(loaded, lock);
        let ready = order.iter().all(|m| match m.format {
            ArtifactFormat::File => self.store.contains(&m.hash, m.format),
            _ => self.store.unpacked(&m.hash, m.format).is_some(),
        });
        if !ready {
            return Ok(Vec::new());
        }
        let target = self.deploy_target(loaded)?;
        let root = self.target_root(&target)?.unwrap_or_default();
        let state = self.deploy_state(&target)?;
        let files = GameFiles::new(&root, &state);
        let (config, _, _) = self.deploy_config(game)?;
        let ctx = DeployContext {
            game,
            config: &config,
            registry: &registry,
        };
        let (mods, _) = self.installed(loaded, game, Some(&files), &order)?;
        let mut made = Vec::new();
        let mut issues = Vec::new();
        for (id, hook) in lists {
            match self.make_list(loaded, id, hook, &ctx, &files, &mods) {
                Ok(list) => made.push(list),
                Err(e) => issues.push(Issue::list(
                    id,
                    Problem::warning(format!("{} couldn't be worked out: {e}", hook.hook.title())),
                )),
            }
        }
        issues.extend(self.found_issues(game, &files, &made, &mods));
        Ok(issues)
    }

    /// What `ext`'s checks say about its list `list`, for showing with
    /// it. They only see that list.
    pub(super) fn list_problems(
        &self,
        game: &GameDef,
        ext: &str,
        files: &GameFiles<'_>,
        list: &List,
        mods: &[InstalledMod],
    ) -> Vec<Problem> {
        let empty = toml::Table::new();
        let lists = std::slice::from_ref(list);
        self.checks_on(game)
            .into_iter()
            .filter(|(_, hook)| hook.ext == ext)
            .flat_map(|(_, hook)| {
                let config = game.hook_config(hook.ext, hook.name).unwrap_or(&empty);
                let ctx = CheckContext::new(game, config, hook.ext, files, lists);
                hook.hook.check(&ctx, mods)
            })
            .filter(|p| {
                p.list
                    .as_deref()
                    .is_some_and(|name| hook_id(ext, name) == list.id)
            })
            .collect()
    }
}
