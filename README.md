# patchwork

A universal mod manager. Mods come from **sources**, are organized into
**profiles**, and are installed by **deployers**. Game support is data
(TOML loaded at runtime), not code.

```
patchwork game set-path lethal-company ~/.steam/steam/steamapps/common/Lethal\ Company
patchwork profile new lethal-company/main
patchwork search more company        # or just `patchwork search` for the most popular
patchwork add ~/Downloads/BepInEx-BepInExPack-5.4.2100.zip --name BepInEx-BepInExPack
patchwork add local:~/Downloads/MoreCompany.zip
patchwork deploy --dry-run
patchwork deploy
patchwork disable MoreCompany && patchwork deploy
patchwork undeploy
```

## Installing

Each [release](https://github.com/patchwork-mods/patchwork/releases/latest) has:

- **The app**, with the `patchwork` CLI inside it: an installer for Windows,
  a `.dmg` for macOS, and an AppImage, `.deb` or `.rpm` for Linux. It handles
  Nexus's `nxm://` links and updates itself from the button at the top of the
  game list. The Windows installer, `.deb` and `.rpm` put `patchwork` on your
  PATH; on macOS or with the AppImage, use **Install command-line tool** on
  the same screen.
- **Just the CLI**, from its shell or PowerShell installer. It updates itself
  with `patchwork self-update`.

Neither is signed yet. macOS blocks the app the first time it's opened: allow
it under System Settings → Privacy & Security → **Open Anyway**. Windows
SmartScreen warns about the installer the same way (**More info** → **Run
anyway**).

## Releasing

Bump `version` in `Cargo.toml` and push it to main, then run the **Release**
workflow on main from GitHub's Actions tab with the tag to make (`v0.2.0`).
`release.yml` builds the CLI with dist, and in the same jobs the app's bundles,
then the `latest.json` the app's updater reads. The GitHub Release (and its
tag) is only made once all of them have built, so if any fail there's no
release: fix it and re-run the failed jobs.

Run it with the default tag, `dry-run`, to build everything without releasing
it, and fill in `only` (say `aarch64-apple-darwin`) to build just some targets
while debugging one. Nothing builds on pushes, since macOS runners bill at 10×.

`release.yml` was made by dist but has since been edited by hand, so don't
regenerate it with `dist generate`: the top of it lists what changed.

The app's updates are signed. Once, before the first release:

```
cargo tauri signer generate -w ~/.tauri/patchwork.key
```

Put the public key in `plugins.updater.pubkey` in
`crates/patchwork-gui/tauri.conf.json`, and the private key's contents and
password in the repository secrets `TAURI_SIGNING_PRIVATE_KEY` and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Keep the private key safe: without it,
existing installs can't be updated.

## Layout

| Crate | |
|---|---|
| `patchwork-core` | registry, profiles, lockfiles, content store, deploy engine, the `Source` and `Deployer` traits |
| `patchwork-sources` | built-in sources (`local`) |
| `patchwork-deployers` | built-in deployers (`rules`) |
| `patchwork-cli` | the `patchwork` binary, a thin layer over core |
| `patchwork-gui` | the Patchwork app (Tauri), also a thin layer over core |
| `registry-gen` | regenerates `registry/` from Thunderstore's ecosystem schema (a dev tool, not released) |

`registry/` holds the built-in game definitions and rulesets; see its README.

## State

All state is files (set `PATCHWORK_HOME` to put it all in one place):

- `<config>/games/*.toml`, `<config>/rulesets/*.toml`: your own definitions. A file for an id that's already defined is merged over it, so it only needs the fields you want to change (e.g. `description = "..."`, or `hidden = true` to hide a game)
- `<data>/registries/<name>/`: installed registries
- `$PATCHWORK_REGISTRY_PATH`: extra registry directories (`:`-separated, like `$PATH`), loaded above installed registries; set it to this repo's `registry/` to try definition changes without rebuilding
- `<data>/profiles/<id>/profile.toml`: what you want
- `<data>/profiles/<id>/profile.lock`: exactly what that resolved to
- `<data>/profiles/<id>/history/<n>.toml`: the profile and its lockfile at each of its last 50 deploys that changed something, for `patchwork rollback`
- `<data>/store/`: content-addressed (BLAKE3) mod artifacts
- `<data>/source-files/`: which store entry each file a source downloads (or you downloaded for it, with `patchwork provide` or `patchwork link`) is
- `<data>/deployments/<game>.json`: every file patchwork put in a game directory
- `<data>/backups/<game>/`: game files that mods replaced
