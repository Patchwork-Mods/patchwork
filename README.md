<p align="center"><img src="docs/images/patchwork_logo.svg" alt="patchwork logo" width="160"></p>
<h1 align="center">Patchwork</h1>
<p align="center">A mod manager for 300+ games and every major mod site, with endless customizability and
modpacks that sync between friends</p>
<hr>

### Goals:

- Any game that can be modded can be easily added to framework
- It should be easy to download mods from any mod provider
- Users should be able to change anything easily
- Sharing modpacks should be easy and handled for you

### Current Features:

- An extensive library of games, including:
  - Almost all thunderstore games
  - Hitman 3,
  - Helldivers 2
- And some games which are supported but currently untested:
  - Skyrim (Special Edition and VR), Enderal, Fallout 4 (and VR), Starfield,
    Oblivion, Fallout 3 and Fallout: New Vegas.
- You can easily add support for additional games, or customize how mods are managed
for existing games.
- Patchwork can get mods from thunderstore, nexus, modrinth, github, and mod.io.
- Fully customizable CSS, so you can style the gui however you like.
- Full modpack support, so importing from other mod managers should be easy
- Uses [iroh](https://www.iroh.computer/) to share files with friends via p2p, letting
your modpacks auto-sync whenever you both are online.

> [!WARNING]
> Patchwork is in alpha. back up any saves and game folders you care about
> before you use it, and please report problems on the
> [issue tracker](https://github.com/patchwork-mods/patchwork/issues).

> [!Note]
> Most of patchwork's code was written with claude code.
> The design, architecture and review are done by a maintainer with a CS background
> and experience building mod managers, and every change is tested before release.
>
> See [CONTRIBUTING.md](CONTRIBUTING.md#ai-use) for the policy on
> AI-assisted contributions.

## Installing

Download the file for your OS from the
[latest release](https://github.com/patchwork-mods/patchwork/releases/latest).
The app comes with the `patchwork` CLI, and updates itself from the button at
the top of the game list.

### Windows

Download and run the `.exe` installer. SmartScreen will warn you, since the installer isn't signed yet. Choose
   **More info** → **Run anyway**.

### macOS

Download the `.dmg`, open it, and drag patchwork into Applications. The first time you open it, macOS blocks it,
since it isn't signed yet. Allow it under System Settings → Privacy & Security → **Open Anyway**.

To use the CLI, click **Install command-line tool** at the top of the game list.

### Linux

| Distro | Download |
| --- | --- |
| Debian, Ubuntu, Mint, ... | `.deb` |
| Fedora, openSUSE, ... | `.rpm` |
| Anything else | `.AppImage` |

The `.deb` and `.rpm` put the CLI on your PATH. With the AppImage, click
**Install command-line tool** at the top of the game list.

### Just the CLI

If you don't want the app, install only the CLI:

```sh
# Linux / macOS
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/patchwork-mods/patchwork/releases/latest/download/patchwork-cli-installer.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/patchwork-mods/patchwork/releases/latest/download/patchwork-cli-installer.ps1 | iex"
```

Update it with `patchwork self-update`.

## Getting started

Patchwork is fundamentally based on `Suppliers` and `Deployers`. Each supported
game will have a custom deployer set up for it. You can install mods through the
`Add Mod` tab, or by typing a link into the search bar.

Mods released on GitHub are added from a link to their repo: pick the file
you want from the newest release (`ModName-0.1.2.zip`, say), and updates
find the same file in later releases (`ModName-*.zip`).

We also have a CLI to control patchwork:

```sh
patchwork game detect                       # find Steam, GOG and Epic games
patchwork profile new lethal-company/main   # a profile is <game>/<name>
patchwork search more company               # or just `patchwork search` for the most popular
patchwork add thunderstore:notnotnotswipez-MoreCompany
patchwork add ~/Downloads/SomeMod.zip       # a local file works too
patchwork add https://github.com/owner/repo # asks which of its releases' files
patchwork deploy --dry-run                  # see what would change
patchwork deploy
patchwork launch
```
Run `patchwork help`, or `patchwork <command> --help`, for more commands

## Load order

When two mods provide the same file, the one further down the mod list
wins. You can also manually set priorities for each mod in the GUI

```sh
patchwork priority "Better Textures" 10   # win over every mod below 10
patchwork list --load-order               # the order they deploy in
patchwork conflicts                       # which mod wins each shared file
```

## Dependencies

The mod list only has the mods you added; what they need is downloaded
for them when the profile is locked (deploying locks it). The app lists
these under **Dependencies** below the mods. To use your own copy of
one instead, like a fork, add that to the list and have it replace the
dependency:

```sh
patchwork deps                                         # what got pulled in, and why
patchwork add thunderstore:SomeoneElse-R2API_Core
patchwork replace SomeoneElse-R2API_Core RiskofThunder-R2API_Core
patchwork replace SomeoneElse-R2API_Core --undo        # back to the original
```

Adding a dependency to the list yourself (to pin its version, say) works
too; mods that need it then use that one.

Only dependencies a source marks as required are installed. Optional ones
(Modrinth) and ones whose source doesn't say (all of Nexus Mods'
requirements) are listed in the mod's details instead. A mod missing any
of the second kind gets a "maybe needed" mark, and locking it the first
time tells you about them. Add the ones you want and ignore the rest:

```sh
patchwork soft-deps --missing                          # what mods list that you don't have
patchwork ignore SkyUI "Skyrim Script Extender (SKSE64)"
patchwork ignore SkyUI nexus:30379 --undo
```

## Sharing profiles

- **Export and import**: `patchwork profile export` and `import` read and
  write patchwork zips, r2modman profiles (`.r2z`), Thunderstore profile
  codes, and plain lists of mod links.
- **Sync** (peer-to-peer, using [iroh](https://iroh.computer)):
  - `patchwork sync link` keeps every profile the same on all your
    computers.
  - `patchwork sync share` gives friends a ticket to a profile that everyone
    can edit.
  - `patchwork sync share --read-only` lets others follow your profile:
    they get your changes, and anything they add stays on their computer.
    Add `--hide-addresses` to keep your IP addresses out of a ticket you
    post publicly.
  - `patchwork sync remove <id>` removes a friend's computer from a shared
    profile (`patchwork sync peers` lists them). Everyone else moves to a
    new document and gets told about it the next time they're online.
    Followers need a new ticket.
  - `patchwork sync name <id> <name>` gives another computer a nickname,
    shown wherever it's listed. Names stay on your computer. In the app,
    name people in a profile's **Share…** dialog, or your own computers
    under **Settings → My computers**.
  - `patchwork sync reset-id` gives this computer a new id. Profiles stay
    synced, and the others stop trying to reach the old one.

  Changes from elsewhere are downloaded but never deployed without you.
  The background daemon (`patchwork daemon install`, or **Settings →
  Background sync**) keeps syncing while the app is closed.

## Customising

- **Themes.** A theme is a CSS file in `<config>/themes/` that starts with
  a `@theme` comment giving its name, and `game = "<id>"` if it's for one
  game. Pick one for every game in **Settings → Appearance** (hover one
  to try it on the whole window), or in `<config>/gui.toml`
  (`[theme] theme = "sorbet"`); a game's own theme is used while that game
  is open and replaces it. A game without one can still have its colour,
  `accent = "#rrggbb"` in its definition, as the accent. **Game themes**
  in Appearance (`game_themes = "on"`, `"accent"` or `"off"`) picks how
  much of that you get, and a game's **Game settings** can pick a theme
  for just that game (`[theme.games] <game> = "<theme>"`).
  `<config>/themes/user.css` loads last, on top of whichever theme is in
  use, for your own changes. Appearance also picks light, dark or the
  system's mode; CSS for dark mode goes under
  `:root[data-mode="dark"]`. The `--pw-*` CSS variables and the ids and
  classes in the app's `index.html` are kept stable for themes to use.
  Changes to theme files show as soon as they're saved. Games' own themes
  come with the registry, in [registry/themes](registry/themes); see
  [examples/themes](examples/themes) for how to write one.
- **Game definitions.** Drop a TOML file into `<config>/games/` to change a
  built-in game. It only needs the fields you want to change, for example
  `hidden = true` to hide a game you don't own. A new file there adds a new
  game. See [registry/README.md](registry/README.md) for the format.

`<config>` and `<data>` are your platform's usual config and data
directories, such as `~/.config/patchwork` and `~/.local/share/patchwork`
on Linux. Set `PATCHWORK_HOME` to keep everything in one directory instead.

## Where things are kept

Everything is plain files under `<config>` and `<data>` (see
[Customising](#customising)):

- `<config>/`: your own game definitions, rulesets and themes
- `<data>/profiles/<id>/`: `profile.toml` (the mods you asked for) and
  `profile.lock` (exactly what they resolved to). These are the same on every
  machine, so you can commit or share them.
- `<data>/store/`: downloaded mod files, shared between profiles. This is
  where the disk space goes.
- `<data>/backups/<game>/`: game files that mods replaced, put back by
  `patchwork undeploy`

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for building from source, how the
code is organised, and adding games.

## License

[MIT](LICENSE)
