# Built-in registry

These definitions are compiled into patchwork. Installed registries
(`<data>/registries/<name>/`) and the user's own files (`<config>/`) use the
same layout and override anything here with the same id.

- `games/<id>.toml`: game definitions. `id` must match the file name.
- `rulesets/<id>.toml`: shared install rules for the `rules` deployer.

Any other directory is loaded as raw TOML, for sources and deployers that need
their own shared data.
