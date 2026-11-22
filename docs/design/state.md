# State layout

All of patchwork's state is plain files under two directories. `<config>`
and `<data>` are the platform's usual config and data directories, such as
`~/.config/patchwork` and `~/.local/share/patchwork` on Linux. If
`PATCHWORK_HOME` is set, everything goes in that one directory instead.

## Config

- `<config>/games/`, `<config>/rulesets/`, `<config>/themes/`: the user's
  own definitions and themes
- `<config>/daemon.toml`: see Daemon below
- `<config>/follows.toml`: the mods followed for update notifications, as
  `source:id` (`mods`); the same on linked computers
- `<config>/gui.toml`: the app's own preferences, such as which theme is
  picked, for every game and for single games (`[theme.games]`), and the
  games starred as favorites (`favorites`)

## Registries

- `<data>/registries/<name>/`: installed registries, laid out like
  `registry/` (`games/`, `rulesets/`, `themes/` and so on).
  `$PATCHWORK_REGISTRY_PATH` adds more directories (`:`-separated) on top

## Profiles

- `<data>/profiles/<id>/profile.toml`: the mods the user asked for
- `<data>/profiles/<id>/profile.lock`: exactly what they resolved to.
  These two files are the same on every machine; everything specific to one
  machine is kept in `profile-state/`
- `<data>/profile-state/<id>/history/`: the profile and lockfile at each of
  the last 50 deploys that changed something, for `rollback`
- `<data>/profile-state/<id>/synced.json`: the profile as it was at the
  last sync, so local changes can be told apart from remote ones
- `<data>/local-mods.toml`: where each local mod was added from. A local
  mod's id is a hash of its file, so it means the same thing on any machine

## Store

- `<data>/store/`: mod files, content-addressed by BLAKE3 hash
- `<data>/source-files/`: which stored file each download is

## Deploying

- `<data>/deployments/<game>.json`: every file patchwork put in a game folder
- `<data>/backups/<game>/`: game files that mods replaced
- `<data>/generated/<game>/`: files deployers built (`<inputs>/`), and
  config files with the profile's settings applied (`config/<inputs>/`)
  or as the game rewrote them (`config/read-back-<hash>/`); kept while
  anything deployed comes from them

## Sync

- `<data>/sync/`: this machine's iroh key, which profiles it syncs, the
  nicknames the user gave other machines (`names.toml`, never synced),
  the followed mods as of the last sync with linked computers
  (`follows.toml`, to tell changes made here from ones that arrived),
  iroh's data, and what the daemon synced while the app was closed
  (`missed.json`)

## Daemon

- `<config>/daemon.toml`: which of the daemon's tasks are on, and how often
  periodic ones run (`[tasks.<id>]`, `enabled`, `every`), and whether a
  task's desktop notifications are on (`notify`)
- `<data>/daemon/`: where the running daemon can be reached
  (`daemon.json`), its log, and each task's own files under `tasks/<id>/`,
  such as a periodic task's past runs (`runs.json`) and what the update
  check last found (`tasks/updates/found.json`)
