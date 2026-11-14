# Built-in registry

This directory holds the games that patchwork knows about out of the box. It's
compiled into patchwork: `build.rs` in `patchwork-core` picks up every
`<kind>/<id>.toml` here, so you can add a new game by just adding a file

- `games/<id>.toml`: game definitions. `id` must match the file name and use
  only lowercase letters, digits and `-`.
- `rulesets/<id>.toml`: shared install rules for the `rules` deployer.
- `hosts/<id>.toml`: file hosts like Dropbox and MediaFire, whose links go to
  a page about a file (see below).
- `runners/<id>.toml`: ways to start games that are just a command to fill
  in (see below).
- `themes/<id>.css`: games' own themes for the app, each with
  `game = "<id>"` in its `@theme` comment (see
  [examples/themes](../examples/themes/README.md)). `build.rs` in
  `patchwork-gui` builds these in.

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

Themes follow the same layers: a `themes/` folder in an installed registry
or a `$PATCHWORK_REGISTRY_PATH` directory adds themes, and one with the
same file name as an earlier one takes its place, up to the user's own in
`<config>/themes/`.

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
image = "https://..."            # a URL, or a path relative to this file;
                                 # if left out, Steam's library art for detect.steam
accent = "#e8a33d"               # optional: the app's accent while it's open

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

## File hosts

`hosts/<id>.toml` teaches patchwork a file host whose links go to a page
about a file rather than the file itself, like Dropbox, Google Drive,
MediaFire or ModDB. Any source can use them; the `url` source does, so
pasting such a link adds the file for every game. The mod keeps the page's
link, and each download follows it again, since hosts' download links
expire.

```toml
name = "MediaFire"
# Regexes for the host's links, tried in order. Named groups become
# `${name}` variables, along with `${url}` (the link as given).
links = ['^https?://(?:www\.)?mediafire\.com/file/(?<key>\w+)(?:/(?<file>[^/?#]+))?']
# The link to keep in profiles (`${link}`). A list means the first whose
# variables all have values; with none, the link as given is kept.
link = ["https://www.mediafire.com/file/${key}/${file}/file", "https://www.mediafire.com/?${key}"]
# The file's name, if the link has it. Without it, patchwork asks the host
# when the mod is added (a step's `name`, or what the download is called).
file = "${file}"

# Pages to read on the way to the file. Each fetches `fetch` (by default
# the link the step before found, or `${link}` for the first) and finds
# the next link with the first `find` regex that matches: its `url` group,
# its base64 `b64` group decoded, or its first group. Relative links and
# HTML entities are fine. `name` can find the file's name on the page.
[[steps]]
find = ['data-scrambled-url="(?<b64>[^"]+)"', 'href="(?<url>https://download\d+\.mediafire\.com/[^"]+)"']
```

Also:

- `download`: where the file is, if it can be worked out from the link
  without reading any pages (Google Drive), or from the last step's
  `${found}`. By default it's what the last step found, or `${link}`.
- `query`: parameters to set on the download link, like Dropbox's
  `{ dl = "1" }`.
- `browser = true`: the host turns away anything that isn't a browser
  (ModDB), so the user downloads the file from the page and hands it to
  patchwork, as with Nexus Mods without Premium. A host that answers 401,
  403 or 429 is treated the same way.
- `format`: `"archive"` or `"file"`, for when the file's name can't be
  found out, as with `browser` hosts.

A host that sends a web page instead of the file (Google Drive does when a
file's had too many downloads) is reported rather than installed.

### Folders

A `[folder]` table lets patchwork list a host's shared folders, so pasting
a folder's link asks which file in it to add (a folder link can't be added
itself). Each entry on the folder's page that links to one of the host's
files becomes a file to pick, and one that links to another folder opens
as a folder of its own; other entries (like Google Docs) are left out.

```toml
[folder]
# Like the host's own `links` and `link`, for folders.
links = ['^https?://drive\.google\.com/drive/(?:u/\d+/)?folders/(?<id>[\w-]+)']
link = "https://drive.google.com/drive/folders/${id}"
# The page that lists what's in it; by default `${link}`.
fetch = "https://drive.google.com/embeddedfolderview?id=${id}"
# Matched over and over on that page, once per entry: its `link` and
# `name` groups, and an optional `size` group in bytes.
entries = '(?s)<div class="flip-entry"[^>]*>.*?<a href="(?<link>[^"]+)".*?class="flip-entry-title">(?<name>[^<]*)<'
```

## Other data

Any other directory here is loaded as raw TOML, for sources and deployers
that need shared data of their own.
