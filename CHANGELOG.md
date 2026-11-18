# Changelog

Each release's section is its GitHub release notes and what the app's update
dialog shows, so the heading has to match the version being released.

## 0.3.0

### Mods

- Links to files on Dropbox, Google Drive, MediaFire and ModDB can be added
  like any other download link, for every game. ModDB only lets browsers
  download, so it asks you to download the file and hand it over. More
  hosts can be added as registry data (`hosts/`).
- A link to a shared Google Drive folder lists what's in it to pick a file
  from, with its folders opening in place and files that don't look like
  mods (PDFs, pictures) dimmed.
- One box for adding mods: the Add mods tab's search box searches every
  source the game has at once as you type, with each result marked with its
  source, and takes links too: paste one to add that mod. Untick a source
  under the box to leave it out. The Mods tab's search box does the same,
  showing what the sources found under your mods.
- A mod's page shows which versions of other mods it needs, and its
  dependencies are in a hover tab in the list view.
- Nexus mods with more than one main file (a plugin and its preloader, say)
  ask which to add, and each can be added as a mod of its own. A link to one
  file on a mod's Files tab adds that file.
- A pinned mod's newer version is a quiet note instead of an update button,
  and goes away once dismissed (or when you pin) until an even newer one is
  out.
- Priority is set from a chip and popover instead of a number field.
- Lots of small improvements to the mod page.

### App

- Remembers which profile you had selected for each game.
- Themes: a theme selector, a few example themes, and more `--pw-*` CSS
  variables to style.
- Deploying can be cancelled.
- Game art from Steam, a faster settings page, and a snappier UI in places.
- The update dialog shows what's new in the release.
- A game with no profiles says so where its mods would be, with one box to
  start one: type a name, or paste an r2modman code, a list of mod links or
  a sync ticket, and the button says which it'll do. The sidebar's New
  profile box works the same way.
- The Add mods tab works before there's a profile, to look around first;
  adding a mod then asks for a profile to add it to.
- The background daemon (Settings → Background) does more than sync: it
  also checks for mod updates every 6 hours while Patchwork is closed, so
  update marks are ready when you open it. Each thing it does can be
  turned on or off there, or with `patchwork daemon enable`/`disable`;
  `patchwork daemon every updates 2h` changes how often it checks.

### Games

- Hitman no longer warns about settings you've already changed.
- Risk of Rain 2's GUI was updated, and settings were fixed for a few games.
- FTL: Faster Than Light, with Slipstream mods, Hyperspace and Multiverse.
  On Windows, add Hyperspace's rollback
  (`https://github.com/FTL-Hyperspace/FTL-Version-Rollback/archive/refs/heads/main.zip`)
  as a mod to turn the game into 1.6.9, which Hyperspace needs, while it's
  deployed; taking it out puts your own version back.
- Deep Rock Galactic, with .pak mods from mod.io, links or files. Like mint,
  deploying packs them all into one `mods_P.pak` that starts them itself,
  without the game's own mod.io integration. mint's in-game mod list and
  server-list labels aren't there yet.
- mod.io takes an OAuth access token as well as an API key, for games like
  Deep Rock Galactic that only let signed-in players download.
