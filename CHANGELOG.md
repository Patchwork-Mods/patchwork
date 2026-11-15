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

### Games

- Hitman no longer warns about settings you've already changed.
- Risk of Rain 2's GUI was updated, and settings were fixed for a few games.
