# Changelog

Each release's section is its GitHub release notes and what the app's update
dialog shows, so the heading has to match the version being released.

## 0.3.0

### Mods

- The Mods tab's search box also takes links: paste one to add that mod, or
  press Enter to search the game's sources.
- A mod's page shows which versions of other mods it needs, and its
  dependencies are in a hover tab in the list view.
- Nexus mods with more than one main file (a plugin and its preloader, say)
  ask which to add, and each can be added as a mod of its own. A link to one
  file on a mod's Files tab adds that file.
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
