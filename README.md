# patchwork

<img src="docs/images/patchwork_logo.svg" alt="patchwork" width="160">

A mod manager built to be flexible and convenient, with support for hundreds of games

### Goals:

- Any game that can be modded can be easily added to framework
- It should be easy to download mods from any mod provider
- Users should be able to change anything easily
- Sharing modpacks should be easy and handled for you

### Current Features:

- An extensive library of games, including:
  - Almost all thunderstore games
  - Hitman 3,
- You can easily add support for additional games, or customize how mods are managed
for existing games.
- Patchwork can get mods from thunderstore, nexus, and just local files.
- Fully customizable CSS, so you can style the gui however you like
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

Each [release](https://github.com/patchwork-mods/patchwork/releases/latest)
comes in two forms:

- **The app**, which includes the `patchwork` CLI. It comes as an installer
  for Windows, a `.dmg` for macOS, and an AppImage, `.deb` or `.rpm` for
  Linux. The app updates itself from the button at the top of the game list.
  The Windows installer, `.deb` and `.rpm` put `patchwork` on your PATH. On
  macOS or with the AppImage, use **Install command-line tool** at the top of
  the game list.
- **Just the CLI**, from its shell or PowerShell installer. Update it with
  `patchwork self-update`.

Neither is signed yet. The first time you open the app, macOS blocks it:
allow it under System Settings → Privacy & Security → **Open Anyway**. On
Windows, SmartScreen warns about the installer: choose **More info** →
**Run anyway**.

## Getting started

In the app, pick a game from the list. patchwork finds games installed
through Steam on its own; you can also point it at a game folder yourself.
Then make a profile, add mods from the tab for a source (Thunderstore,
Nexus Mods, ...), and press **Deploy**.

The CLI does the same:

```sh
patchwork game detect                       # find Steam games
patchwork profile new lethal-company/main   # a profile is <game>/<name>
patchwork search more company               # or just `patchwork search` for the most popular
patchwork add thunderstore:notnotnotswipez-MoreCompany
patchwork add ~/Downloads/SomeMod.zip       # a local file works too
patchwork deploy --dry-run                  # see what would change
patchwork deploy
patchwork launch
```
Run `patchwork help`, or `patchwork <command> --help`, for more commands

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

  Changes from elsewhere are downloaded but never deployed without you.
  The background daemon (`patchwork daemon install`, or **Settings →
  Background sync**) keeps syncing while the app is closed.

## Customising

- **Themes.** The app writes `<config>/themes/user.css`, which applies to
  every game, and you can add `<config>/themes/<game>.css` for a single game.
  The `--pw-*` CSS variables and the ids and classes in the app's
  `index.html` are kept stable for themes to use.
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
