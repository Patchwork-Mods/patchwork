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

Every BepInEx game on Thunderstore, and `rulesets/bepinex-loader.toml`, are
generated from Thunderstore's ecosystem schema (the data r2modman uses for its
game list). Regenerate them with:

```
cargo run -p registry-gen
```

Generated files start with a header saying so. Don't edit them by hand, since
the next run overwrites them; change the generator instead, or for a one-off,
add a file without the header (the generator leaves those alone, and deletes
generated games that have left the schema). `rulesets/bepinex.toml` is
hand-written, and a test checks that it matches what the generator would
produce for the usual BepInEx layout.

## Other data

Any other directory is loaded as raw TOML, for sources and deployers that need
their own shared data.

While working on definitions, set `PATCHWORK_REGISTRY_PATH` to this directory
to load them at runtime without rebuilding.
