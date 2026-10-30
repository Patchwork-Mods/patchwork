# Built-in registry

These definitions are compiled into patchwork: `build.rs` in `patchwork-core`
picks up every `<kind>/<id>.toml` here, so adding a file is all it takes.
Installed registries (`<data>/registries/<name>/`) and the user's own files
(`<config>/`) use the same layout. A file whose id is already defined is merged
over the earlier one: tables merge key by key, and other values (including
arrays like `rules`) replace what was there.

- `games/<id>.toml`: game definitions. `id` must match the file name.
- `rulesets/<id>.toml`: shared install rules for the `rules` deployer.

## Generated files

Every BepInEx and MelonLoader game on Thunderstore, and the
`rulesets/*-loader.toml` files, are generated from Thunderstore's ecosystem
schema (the data r2modman uses for its game list). Regenerate them with:

```
cargo run -p registry-gen
```

Generated files start with a header saying so. Don't edit them by hand, since
the next run overwrites them; change the generator instead, or for a one-off,
add a file without the header (the generator leaves those alone, and deletes
generated games that have left the schema). `rulesets/bepinex.toml` is
hand-written, and a test checks that it matches what the generator would
produce for the usual BepInEx layout.
`rulesets/recursive-melonloader.toml` is hand-written too: games on
MelonLoader 0.7 and later all use it, as r2modman puts every mod in a folder
of its own for them whatever the schema's install rules say.

## Other data

Any other directory is loaded as raw TOML, for sources and deployers that need
their own shared data.

`runners/` holds ways of running games that are just a command to fill in. A
game opts in with `runners = ["umu"]` in its `[launch.<id>]`, and users can pick
any runner for a game; `umu.toml` explains the format, including settings users
can change.

While working on definitions, set `PATCHWORK_REGISTRY_PATH` to this directory
to load them at runtime without rebuilding.
