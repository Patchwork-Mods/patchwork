//! Setting the game itself up in profiles' instances, for games whose
//! definitions have a [`ProvisionSpec`](crate::game::ProvisionSpec), and
//! starting it there; see [`crate::provision`].

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};

use futures_util::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};

use super::{Patchwork, download};
use crate::account::Account;
use crate::deploy::Os;
use crate::environment::GameEnv;
use crate::error::IoContext;
use crate::event::Event;
use crate::fsutil;
use crate::game::LaunchDefs;
use crate::launch::{self, Launch as LaunchCommand, LaunchOption};
use crate::profile::{Profile, ProfileRef};
use crate::provision::{Launch, ProvisionContext, Provisioner, RuntimeFile, Setup, Unpack};
use crate::relpath::RelPath;
use crate::{Error, Result};

/// How many files are downloaded at once. Setups can have thousands of
/// small ones (all of Minecraft's sounds, say).
const DOWNLOADS_AT_ONCE: usize = 16;

/// What [`Patchwork::provision`] did.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Provisioned {
    /// The setup's id.
    pub setup: String,
    /// How many files it has.
    pub files: usize,
    /// How many of them weren't there yet and were downloaded.
    pub downloaded: usize,
    pub bytes: u64,
}

/// Which setup a profile was last set up with on this machine.
#[derive(Debug, Serialize, Deserialize)]
struct ProvisionState {
    kind: String,
    setup: String,
    /// What the profile picked for the game's `[env]` then, so picking
    /// something else means setting it up again.
    env: BTreeMap<String, String>,
}

impl Patchwork {
    /// Sets up what `profile`'s game needs to run as the profile picks for
    /// its `[env]`, downloading whatever isn't already there. Only for
    /// profiles that use an instance of their own: the game's folder is
    /// whatever installed it's to set up.
    pub async fn provision(&self, profile: &ProfileRef) -> Result<Provisioned> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let Some(spec) = &game.provision else {
            return Err(Error::invalid(format!(
                "patchwork can't set {} up itself; whatever installed it does",
                game.name
            )));
        };
        if !self.uses_instance(&loaded)? {
            return Err(Error::invalid(format!(
                "{0} is deployed to the game's folder, which is set up by whatever installed it; \
                 run `patchwork --profile {0} instance on` to give it a folder of its own",
                loaded.label()
            )));
        }
        let provisioner = self.provisioner(&spec.kind)?;
        let picks = game.env.picks(&loaded.env);
        let env = GameEnv::from_picks(&picks);
        let ctx = ProvisionContext {
            game,
            config: &spec.config,
            env: &env,
            os: Os::current(),
            arch: std::env::consts::ARCH,
        };
        let setup = provisioner.plan(&ctx).await?;
        let setup_file = self.setup_file(&spec.kind, &setup.id)?;
        let runtime = self.paths.runtime(&spec.kind);
        let (downloaded, bytes) = self.fetch_runtime(&runtime, &setup).await?;
        fsutil::write_json(&setup_file, &setup)?;
        let state = ProvisionState {
            kind: spec.kind.clone(),
            setup: setup.id.clone(),
            env: picks,
        };
        fsutil::write_toml(&self.provision_state_file(profile), &state)?;
        Ok(Provisioned {
            setup: setup.id,
            files: setup.files.len(),
            downloaded,
            bytes,
        })
    }

    /// The setup `profile` was last set up with on this machine, and the
    /// folder its files are in, unless it's picked something else for the
    /// game's `[env]` since.
    pub fn provisioned(&self, profile: &ProfileRef) -> Result<Option<(Setup, PathBuf)>> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        let Some(spec) = &game.provision else {
            return Ok(None);
        };
        let Some(state) = fsutil::read_toml::<ProvisionState>(&self.provision_state_file(profile))?
        else {
            return Ok(None);
        };
        if state.kind != spec.kind || state.env != game.env.picks(&loaded.env) {
            return Ok(None);
        }
        let setup = fsutil::read_json(&self.setup_file(&state.kind, &state.setup)?)?;
        Ok(setup.map(|setup| (setup, self.paths.runtime(&state.kind))))
    }

    /// Whether `profile` is to be started in its instance and hasn't been
    /// set up for that as it picks now, so [`Patchwork::provision`] has to
    /// run before it can be.
    pub fn needs_provisioning(&self, profile: &ProfileRef) -> Result<bool> {
        let loaded = self.load_profile(profile)?;
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        Ok(game.provision.is_some()
            && self.uses_instance(&loaded)?
            && self.provisioned(profile)?.is_none())
    }

    /// The ways `profile`'s game can be started, the default one marked:
    /// in its instance if it uses one, otherwise as for the game (see
    /// [`Patchwork::launch_options`]). `None` if the game hasn't been set
    /// up.
    pub fn profile_launch_options(&self, profile: &Profile) -> Result<Option<Vec<LaunchOption>>> {
        if !self.uses_instance(profile)? {
            return self.launch_options(&profile.game);
        }
        let registry = self.registry();
        let game = &registry.game(&profile.game)?.value;
        Ok(Some(vec![LaunchOption {
            id: LaunchDefs::GAME.to_owned(),
            name: game.name.clone(),
            default: true,
            unavailable: game
                .provision
                .is_none()
                .then(|| "patchwork can't start it in an instance of its own".to_owned()),
        }]))
    }

    /// How to start `profile`'s game: in its instance, as it was last set
    /// up, if it uses one, otherwise as [`Patchwork::plan_launch`] does.
    /// Deploying and setting up first are up to the caller.
    pub fn plan_profile_launch(
        &self,
        profile: &ProfileRef,
        option: Option<&str>,
    ) -> Result<LaunchCommand> {
        let loaded = self.load_profile(profile)?;
        if !self.uses_instance(&loaded)? {
            return self.plan_launch(&loaded.game, option);
        }
        let registry = self.registry();
        let game = &registry.game(&loaded.game)?.value;
        if let Some(option) = option.filter(|o| *o != LaunchDefs::GAME) {
            return Err(Error::invalid(format!(
                "{} can't be launched as `{option}` in an instance of its own",
                game.name
            )));
        }
        if game.provision.is_none() {
            return Err(Error::invalid(format!(
                "patchwork can't start {} in an instance of its own; run `patchwork --profile {} instance off` to deploy it to the game's folder",
                game.name,
                loaded.label()
            )));
        }
        let Some((setup, runtime)) = self.provisioned(profile)? else {
            return Err(Error::invalid(format!(
                "{} isn't set up for what {} picks; run `patchwork --profile {} provision` first",
                game.name,
                loaded.label(),
                loaded.label()
            )));
        };
        let account = self.account()?.ok_or_else(|| {
            Error::invalid(
                "patchwork doesn't know who to play as; pick a name with `patchwork account offline <name>`",
            )
        })?;
        let os = Os::current();
        let instance = self.instance_dir(profile);
        let crate::provision::Launch::Java(java) = &setup.launch;
        let ctx = launch::java::JavaContext {
            os,
            runtime: &runtime,
            instance: &instance,
            account: &account,
            system_java: launch::java::system_java(os),
            fhs: launch::fhs_wrapper(os),
        };
        launch::java::plan(&game.name, java, &ctx)
    }

    /// Who the user plays as in games patchwork starts itself, if they've
    /// said.
    pub fn account(&self) -> Result<Option<Account>> {
        fsutil::read_toml(&self.paths.account_file())
    }

    /// Sets who the user plays as, or forgets it with `None`.
    pub fn set_account(&self, account: Option<&Account>) -> Result<()> {
        let path = self.paths.account_file();
        match account {
            Some(account) => fsutil::write_toml(&path, account),
            None => match fs::remove_file(&path) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                done => done.at(&path),
            },
        }
    }

    fn provisioner(&self, kind: &str) -> Result<&dyn Provisioner> {
        self.provisioners
            .get(kind)
            .map(|p| p.as_ref())
            .ok_or_else(|| Error::UnknownProvisioner(kind.to_owned()))
    }

    fn setup_file(&self, kind: &str, id: &str) -> Result<PathBuf> {
        let name = RelPath::new(id)
            .ok()
            .filter(|rel| rel.components().count() == 1)
            .ok_or_else(|| Error::Provisioner {
                kind: kind.to_owned(),
                message: format!("`{id}` can't be the name of a setup"),
            })?;
        Ok(self
            .paths
            .runtime(kind)
            .join("setups")
            .join(format!("{name}.json")))
    }

    fn provision_state_file(&self, profile: &ProfileRef) -> PathBuf {
        self.profile_state_dir(profile).join("provisioned.toml")
    }

    /// Downloads the files of `setup` that aren't in `runtime` yet, and
    /// unpacks those that need it. Returns how many it downloaded, and
    /// how much.
    async fn fetch_runtime(&self, runtime: &Path, setup: &Setup) -> Result<(usize, u64)> {
        let missing: Vec<&RuntimeFile> = setup
            .files
            .iter()
            .filter(|f| !is_present(&f.path.under(runtime), f.size))
            .collect();
        if !missing.is_empty() {
            let size: u64 = missing.iter().filter_map(|f| f.size).sum();
            self.reporter.report(Event::Fetching {
                name: format!(
                    "{} files ({:.1} MB) for {}",
                    missing.len(),
                    size as f64 / 1e6,
                    setup.id
                ),
            });
        }
        let downloaded = missing.len();
        // Futures that own what they need, so the one this is in can be
        // sent between threads (by Tauri, say).
        let bytes: Vec<u64> = stream::iter(missing.into_iter().cloned())
            .map(|file| {
                let runtime = runtime.to_owned();
                async move { fetch_file(&runtime, &file).await }
            })
            .buffer_unordered(DOWNLOADS_AT_ONCE)
            .try_collect()
            .await?;
        let mut unpacks: BTreeMap<&RelPath, Vec<(&RuntimeFile, &Unpack)>> = BTreeMap::new();
        for file in &setup.files {
            if let Some(unpack) = &file.unpack {
                unpacks.entry(&unpack.to).or_default().push((file, unpack));
            }
        }
        for (to, zips) in unpacks {
            let to = to.under(runtime);
            if !to.exists() {
                unpack_zips(runtime, &to, &zips)?;
            }
        }
        let Launch::Java(java) = &setup.launch;
        for dir in java.dirs.values() {
            let dir = dir.under(runtime);
            fs::create_dir_all(&dir).at(&dir)?;
        }
        Ok((downloaded, bytes.iter().sum()))
    }
}

fn is_present(path: &Path, size: Option<u64>) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && size.is_none_or(|s| s == m.len()))
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path).at(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs::set_permissions(path, permissions).at(path)
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<()> {
    Ok(())
}

/// Downloads `file` into `runtime`, checking it on the way, and returns
/// its size. It's only put in place once it's checked out, so a download
/// that's cut short isn't taken for the file next time.
async fn fetch_file(runtime: &Path, file: &RuntimeFile) -> Result<u64> {
    let dest = file.path.under(runtime);
    let dir = dest.parent().unwrap_or(runtime);
    fs::create_dir_all(dir).at(dir)?;
    let tmp = tempfile::Builder::new()
        .suffix(".tmp")
        .tempfile_in(dir)
        .at(dir)?
        .into_temp_path();
    download(&file.url, &tmp).await?;
    let size = fs::metadata(&tmp).at(&tmp)?.len();
    let wrong = |what: String| {
        Error::invalid(format!(
            "{} was {what}, so it's been left out in case it's corrupt; try again",
            file.url
        ))
    };
    if let Some(expected) = file.size
        && expected != size
    {
        return Err(wrong(format!("{size} bytes rather than {expected}")));
    }
    if let Some(expected) = &file.sha1 {
        let found = sha1_of(&tmp)?;
        if !found.eq_ignore_ascii_case(expected) {
            return Err(wrong(format!("not the file expected (sha1 {found})")));
        }
    }
    if file.executable {
        make_executable(&tmp)?;
    }
    tmp.persist(&dest).map_err(|e| e.error).at(&dest)?;
    Ok(size)
}

fn sha1_of(path: &Path) -> Result<String> {
    let mut file = File::open(path).at(path)?;
    let mut hasher = Sha1::new();
    io::copy(&mut file, &mut hasher).at(path)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Unpacks each of `zips` (files in `runtime`) into `to`, as they say.
/// They're unpacked beside `to` first and moved into place, so `to`
/// existing means they're all there.
fn unpack_zips(runtime: &Path, to: &Path, zips: &[(&RuntimeFile, &Unpack)]) -> Result<()> {
    let parent = to.parent().unwrap_or(to);
    fs::create_dir_all(parent).at(parent)?;
    let tmp = tempfile::Builder::new()
        .prefix(".unpacking")
        .tempdir_in(parent)
        .at(parent)?;
    for (file, unpack) in zips {
        let archive = file.path.under(runtime);
        let bad = |e: zip::result::ZipError| Error::parse(&archive, e);
        let mut zip = zip::ZipArchive::new(File::open(&archive).at(&archive)?).map_err(bad)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i).map_err(bad)?;
            let excluded = unpack.exclude.iter().any(|e| entry.name().starts_with(e));
            let Some(rel) = entry
                .enclosed_name()
                .filter(|_| entry.is_file() && !excluded)
            else {
                continue;
            };
            let dest = tmp.path().join(rel);
            if let Some(dir) = dest.parent() {
                fs::create_dir_all(dir).at(dir)?;
            }
            let mut out = File::create(&dest).at(&dest)?;
            io::copy(&mut entry, &mut out).at(&dest)?;
        }
    }
    let tmp = tmp.keep();
    fs::rename(&tmp, to).at(to)
}
