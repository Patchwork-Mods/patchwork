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

## Layout

| Crate | |
|---|---|
| `patchwork-core` | registry, profiles, lockfiles, content store, deploy engine, the `Source` and `Deployer` traits |
| `patchwork-sources` | built-in sources (`local`) |
| `patchwork-deployers` | built-in deployers (`rules`) |
| `patchwork-cli` | the `patchwork` binary, a thin layer over core |

`registry/` holds the built-in game definitions and rulesets; see its README.

## State

All state is files (set `PATCHWORK_HOME` to put it all in one place):

- `<config>/games/*.toml`, `<config>/rulesets/*.toml`: your own definitions. A file for an id that's already defined is merged over it, so it only needs the fields you want to change (e.g. `description = "..."`, or `hidden = true` to hide a game)
- `<data>/registries/<name>/`: installed registries
- `$PATCHWORK_REGISTRY_PATH`: extra registry directories (`:`-separated, like `$PATH`), loaded above installed registries; set it to this repo's `registry/` to try definition changes without rebuilding
- `<data>/profiles/<id>/profile.toml`: what you want
- `<data>/profiles/<id>/profile.lock`: exactly what that resolved to
- `<data>/store/`: content-addressed (BLAKE3) mod artifacts
- `<data>/deployments/<game>.json`: every file patchwork put in a game directory
- `<data>/backups/<game>/`: game files that mods replaced
