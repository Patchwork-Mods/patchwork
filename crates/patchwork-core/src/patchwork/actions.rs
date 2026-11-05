//! The actions that are on for a profile's game, and running them; see
//! [`crate::action`].

use std::fs;
use std::path::PathBuf;

use async_trait::async_trait;

use super::{Patchwork, deploy_order};
use crate::action::{
    self, Action, ActionContext, ActionInfo, ActionReport, Fetch, ListChange, StateChange,
};
use crate::deploy::{DeployContext, GameFiles};
use crate::error::IoContext;
use crate::ext::Hook;
use crate::fsutil;
use crate::game::GameDef;
use crate::list::List;
use crate::profile::ProfileRef;
use crate::{Error, Result};

impl Patchwork {
    /// The actions that are on for `def`, in id order.
    fn actions_on(&self, def: &GameDef) -> Vec<(&str, &Hook<dyn Action>)> {
        self.actions
            .iter()
            .filter(|(_, hook)| self.hook_on(def, hook.ext, hook.name))
            .map(|(id, hook)| (id.as_str(), hook))
            .collect()
    }

    /// The names of the actions extension `ext` has.
    pub(super) fn action_names(&self, ext: &str) -> impl Iterator<Item = &'static str> {
        self.actions
            .values()
            .filter(move |h| h.ext == ext)
            .map(|h| h.name)
    }

    /// The actions that are on for `profile`'s game.
    pub fn actions(&self, profile: &ProfileRef) -> Result<Vec<ActionInfo>> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let def = &registry.game(&loaded.game)?.value;
        Ok(self
            .actions_on(def)
            .into_iter()
            .map(|(id, hook)| ActionInfo {
                id: id.to_owned(),
                title: hook.hook.title().to_owned(),
                description: hook.hook.description().map(str::to_owned),
                shown_with: hook.hook.shown_with().map(str::to_owned),
            })
            .collect())
    }

    /// Runs action `id` on `profile`, and with `apply` saves what it wants
    /// changed; otherwise only says what that would be. Like
    /// [`Patchwork::list`], only mods that have been locked count. The
    /// action may fetch things, but its changes are only saved once it's
    /// done, so a failed run changes nothing.
    pub async fn run_action(
        &self,
        profile: &ProfileRef,
        id: &str,
        apply: bool,
    ) -> Result<ActionReport> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let Some(hook) = self.actions.get(id) else {
            return Err(Error::invalid(format!("there's no action `{id}`")));
        };
        if !self.hook_on(game, hook.ext, hook.name) {
            return Err(Error::invalid(format!(
                "{} doesn't have the `{id}` action",
                game.name
            )));
        }

        let target = self.deploy_target(&loaded)?;
        let (config, _, _) = self.deploy_config(game)?;
        let dctx = DeployContext {
            game,
            config: &config,
            registry: &registry,
        };
        let lock = self.load_lock(profile)?;
        let order = lock
            .as_ref()
            .map(|lock| deploy_order(&loaded, lock))
            .unwrap_or_default();
        let root = self.target_root(&target)?.unwrap_or_default();
        let state = self.deploy_state(&target)?;
        let files = GameFiles::new(&root, &state);
        let (mods, _) = self.installed(&loaded, game, Some(&files), &order)?;
        let lists = self.make_lists(&loaded, &dctx, &files, &mods)?;

        let empty = toml::Table::new();
        let hook_config = game.hook_config(hook.ext, hook.name).unwrap_or(&empty);
        let uses = self.uses_of(hook.ext);
        let fetcher = CachedFetch {
            dir: self.paths.cache.join("actions").join(hook.ext),
        };
        let ctx = ActionContext::new(
            game,
            hook_config,
            hook.ext,
            uses,
            &files,
            &lists,
            &mods,
            &fetcher,
        );
        let outcome = hook.hook.run(&ctx).await?;
        let mut warnings = ctx.take_warnings();
        warnings.extend(outcome.warnings);

        let mut changes = Vec::new();
        for change in outcome.changes {
            match change {
                StateChange::List { id: list_id, order } => {
                    if !action::may_use(hook.ext, uses, &list_id) {
                        return Err(action::not_declared(hook.ext, &list_id));
                    }
                    let list = find(&lists, &list_id, game)?;
                    let (after, unknown) = action::reorder(list, &order);
                    if !unknown.is_empty() {
                        warnings.push(format!(
                            "{} named what isn't in {}: {}",
                            hook.hook.title(),
                            list.title.to_lowercase(),
                            unknown.join(", ")
                        ));
                    }
                    let before = list.entries();
                    let moved = action::moved(&before, &after);
                    changes.push(ListChange {
                        id: list_id,
                        title: list.title.clone(),
                        before,
                        after,
                        moved,
                    });
                }
            }
        }

        if apply {
            for change in changes.iter().filter(|c| !c.moved.is_empty()) {
                self.set_list(profile, &change.id, change.after.clone())?;
            }
        }
        Ok(ActionReport {
            id: id.to_owned(),
            title: hook.hook.title().to_owned(),
            message: outcome.message,
            warnings,
            lists: changes,
            applied: apply,
        })
    }
}

fn find<'a>(lists: &'a [List], id: &str, game: &GameDef) -> Result<&'a List> {
    lists
        .iter()
        .find(|l| l.id == id)
        .ok_or_else(|| Error::invalid(format!("{} doesn't have the `{id}` list on", game.name)))
}

/// Fetches for actions, keeping the last copy of each URL (and its ETag,
/// so an unchanged one isn't downloaded again) in `dir`.
struct CachedFetch {
    dir: PathBuf,
}

#[async_trait]
impl Fetch for CachedFetch {
    async fn fetch(&self, url: &str) -> Result<(Vec<u8>, Option<String>)> {
        let key = blake3::hash(url.as_bytes()).to_hex();
        let body = self.dir.join(key.as_str());
        let etag = self.dir.join(format!("{key}.etag"));
        let cached = fs::read(&body).ok();
        let mut request = crate::http().get(url);
        if cached.is_some()
            && let Ok(tag) = fs::read_to_string(&etag)
        {
            request = request.header(reqwest::header::IF_NONE_MATCH, tag.trim());
        }
        let fetched = async {
            let response = request.send().await?.error_for_status()?;
            if response.status() == reqwest::StatusCode::NOT_MODIFIED {
                return Ok(None);
            }
            let tag = response
                .headers()
                .get(reqwest::header::ETAG)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            Ok(Some((response.bytes().await?.to_vec(), tag)))
        }
        .await;
        match (fetched, cached) {
            (Ok(None), Some(cached)) => Ok((cached, None)),
            (Ok(Some((bytes, tag))), _) => {
                fs::create_dir_all(&self.dir).at(&self.dir)?;
                fsutil::atomic_write(&body, &bytes)?;
                match tag {
                    Some(tag) => fsutil::atomic_write(&etag, tag.as_bytes())?,
                    None => {
                        let _ = fs::remove_file(&etag);
                    }
                }
                Ok((bytes, None))
            }
            (Err(e), Some(cached)) => Ok((
                cached,
                Some(format!(
                    "couldn't fetch {url} ({e}), so the copy from last time was used"
                )),
            )),
            (Ok(None), None) => Err(Error::invalid(format!(
                "{url} said nothing had changed, but there's no copy of it"
            ))),
            (Err(result), None) => Err(Error::Reqwest {
                url: url.to_owned(),
                result,
            }),
        }
    }
}
