# Built-in registry

This directory holds the games that patchwork knows about out of the box. It's
compiled into patchwork: `build.rs` in `patchwork-core` picks up every
`<kind>/<id>.toml` here, so you can add a new game by just adding a file

- `games/<id>.toml`: game definitions. `id` must match the file name and use
  only lowercase letters, digits and `-`.
- `rulesets/<id>.toml`: shared install rules for the `rules` deployer.
- `runners/<id>.toml`: ways to start games that are just a command to fill
  in (see below).

While you work on definitions, set `PATCHWORK_REGISTRY_PATH` to this
directory so they're loaded at runtime without rebuilding:

```sh
PATCHWORK_REGISTRY_PATH=$PWD/registry patchwork game show my-game
```

## Layers

Definitions are loaded in layers, and later layers win:

1. this built-in registry
2. installed registries, in `<data>/registries/<name>/`
3. directories in `$PATCHWORK_REGISTRY_PATH`
4. the user's own files, in `<config>/games/` and `<config>/rulesets/`

All of them use the same layout. A file for an id that's already defined is
merged over the earlier one: tables merge key by key, and any other value
(arrays like `rules` included) replaces what was there. So an override only
needs the fields it changes:

```toml
# <config>/games/skyrim-se.toml
link = "symlink"
```

## Writing a game definition

A typical Thunderstore game looks like this:

```toml
schema = 1
id = "atlyss"
name = "ATLYSS"
image = "https://..."            # a URL, or a path relative to this file

[sources.thunderstore]
community = "atlyss"

[deployer]
kind = "rules"
rulesets = ["bepinex"]

[detect]
steam = [2768430]                # Steam app ids
folder = "ATLYSS"                # folder name under steamapps/common
exe = ["ATLYSS.exe"]
```

The tables a definition can have:

| Table | What it's for |
|---|---|
| `[sources.<id>]` | where mods come from, and each source's settings (Thunderstore community, Nexus `game_domain`, Modrinth project types, mod.io `game` and `id`) |
| `[deployer]` | how mods' files are placed: `kind` (`rules`, `bethesda`, `smf`), plus `rulesets` and `rules` for the `rules` deployer |
| `link` | `copy` (the default), `hardlink` or `symlink`. Hardlinks fall back to copying across drives |
| `[detect]` | how to find the game: Steam app ids, its folder name, and exes that should be there |
| `[versions]` | how to tell which version of the game is installed, so mods can say which ones they support |
| `[env.<axis>]` | choices each profile makes about the game, such as Minecraft's version and mod loader |
| `[roots.<id>]` | folders outside the game's own that mods may write to, such as `%LOCALAPPDATA%` for Skyrim's `plugins.txt` |
| `[launch.<id>]` | ways to start the game besides the usual one, like SKSE, and which `runners` can run them |
| `[capture]` | where tools write output that should be kept as a mod |
| `[instances]` | per-profile game folders, for pointing a launcher's instance at (Minecraft) |
| `[ext.<id>]` | extensions the game turns on, such as `[ext.fomod.installer]` or `[ext.bethesda.plugins]`. See [extensions.md](../docs/design/extensions.md) |

`games/skyrim-se.toml`, `games/hitman-woa.toml` and `games/minecraft.toml`
are written by hand and commented throughout. They're the best examples of
the less common tables. `rulesets/bepinex.toml` explains the rule format.

## Generated files

Every BepInEx and MelonLoader game on Thunderstore, and the
`rulesets/*-loader.toml` files, are generated from Thunderstore's ecosystem
schema (the data r2modman uses for its game list). Regenerate them with:

```sh
cargo run -p registry-gen
```

Generated files start with a header that says so. Don't edit them by hand,
since the next run overwrites them. Change the generator instead. For a
one-off, remove the header: the generator leaves files without it alone.
It also deletes generated games that are no longer in the schema.

Two rulesets are hand-written:

- `rulesets/bepinex.toml`. A test checks that it matches what the
  generator would produce for the usual BepInEx layout.
- `rulesets/recursive-melonloader.toml`. Every game on MelonLoader 0.7 or
  later uses it, because r2modman puts each mod in a folder of its own for
  those games, whatever the schema's install rules say.

## Runners

A game opts in to a runner with `runners = ["umu"]` in its `[launch.<id>]`,
and users can pick any runner for a game in its settings. The built-in
runners (Steam, native, Proton, Wine) are code. Simpler ones are data files
in `runners/`, and `runners/umu.toml` explains the format, including the
settings users can change.

## Other data

Any other directory here is loaded as raw TOML, for sources and deployers
that need shared data of their own.
