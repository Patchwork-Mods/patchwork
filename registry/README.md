# Built-in registry

These definitions are compiled into patchwork: `build.rs` in `patchwork-core`
picks up every `<kind>/<id>.toml` here, so adding a file is all it takes.
Installed registries (`<data>/registries/<name>/`) and the user's own files
(`<config>/`) use the same layout. A file whose id is already defined is merged
over the earlier one: tables merge key by key, and other values (including
arrays like `rules`) replace what was there.

- `games/<id>.toml`: game definitions. `id` must match the file name.
- `rulesets/<id>.toml`: shared install rules for the `rules` deployer.

Any other directory is loaded as raw TOML, for sources and deployers that need
their own shared data.

While working on definitions, set `PATCHWORK_REGISTRY_PATH` to this directory
to load them at runtime without rebuilding.
