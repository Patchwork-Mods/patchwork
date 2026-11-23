# Changelog

Each release's section is its GitHub release notes and what the app's update
dialog shows, so the heading has to match the version being released.

## 0.3.1

- Added Hollow Knight and ModLinks support
- Added ability for themes to declare options
- You can now follow mods, which will give you a notification when a new update comes out.
- Cleaned up the mod tab and improved the buttons
- Shows when a Steam game is on a beta branch beside the profile's name
- Finds installed games in Steam, GOG and Epic each time Patchwork opens, or with the new Find games button, and notices when a game set up has been moved or uninstalled
- When something has to be fixed before an action can continue, Patchwork now explains what and offers to fix it
- A mod whose file changed since it was locked (say, repackaged by its author) can now be accepted at the same version
- Reworked settings page to not require manual saving

### Themes

Added new themes for the following:
- FTL
- Hollow Knight
- Hollow Knight: Silksong
- Return of the Obra Dinn

## 0.3.0

This is a big update, mostly focused on cleaning up the ui a bit and expanding
functionality. Many small gui improvements, such as a theme selector, support
for steam's game art, and a progress bar for downloads

### Mods

- Added support for downloading from Dropbox, Google Drive, MediaFire and ModDB
- Dramatically expanded the mod search bar functionality to allow for adding mods
  too
- Mods are now smarter about what versions of dependencies they need
- Nexus mods with multiple main files ask which to add
- Reduced annoyance of a pinned mod's update notice

### App

- The update dialog shows what's new in the release
- Lots of usability improvements to the mod page
- A few places, such as the settings page, load faster and act snappier
- Added a theme selector to the GUI, and a few 
- Default game art pulls from Steam if possible
- Improved UI for games without profiles
- The Add mods tab works before there's a profile, to look around first;
  adding a mod then asks for a profile to add it to.
- Added checking for updates to the background daemon
- Added progress bar for downloads
- Patchwork doesn't self-update if it was installed via package manager
- The background daemon keeps starting at login after a package manager
  moves Patchwork somewhere new (Homebrew and Nix do on every update).

### Games

- Fixed settings for a few games
- Gave Risk of Rain 2 a custom GUI to match it's in-game UI
- FTL: Faster Than Light, with Slipstream mods, Hyperspace and Multiverse.
  - On Windows, add [Hyperspace's rollback](`https://github.com/FTL-Hyperspace/FTL-Version-Rollback/archive/refs/heads/main.zip`)
  as a mod to turn the game into 1.6.9, which Hyperspace needs
- mod.io takes an OAuth access token as well as an API key
- Hitman and DRG are no longer in the release downloads due to licensing issues.
  Building from source fixes this but we can't ship them in release just yet
