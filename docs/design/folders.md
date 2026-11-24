# Folders, shared sets and layered profiles

Status (2026-11-24): step 1 (folders inside a profile) is done; the rest
is a proposal. Where what gets built differs from this, the notes under
each step in "Order of work" say so, and those notes win.

## The problem

A profile is one flat list of mods. That's fine until someone plays one
game several ways:

- Their **client-side QOL** mods (a minimap, a better HUD, performance
  tweaks) belong in every profile for the game, and today have to be added
  to each one and kept in step by hand.
- With **friends**, the gameplay mods have to match exactly, so they sync.
  But each person's QOL mods shouldn't go to everyone, while they should
  still follow the person to their own other computers.
- A pack maker wants to say "these are **recommended but optional**", like
  shaders, that each person picks for themselves.
- A long list is hard to read. Mods belong together (a library and its
  addons, "UI", "audio"), and today the only grouping is hidden tags.

## Ideas

Three different things get called "folders" here, and they're kept apart:

1. **Folders**: groups of mods inside one profile. They're about how the
   list is shown and about turning a group on or off at once. A mod is in
   at most one folder.
2. **Shared sets**: a list of mods that lives outside any profile and that
   several profiles **include**. Changing the set changes every profile that
   includes it. In a profile, an included set shows as a **linked folder**,
   so there's one idea to learn: it's a folder, and this one is linked.
3. **Layers** of a synced profile: **required** (everyone has them and
   they must match), **optional** (published to everyone, and each person
   opts in) and **personal** (only the person and their linked computers).

## 1. Folders

```toml
[[folder]]
id = "0b6d3c4e-..."
name = "Client QOL"

[[mod]]
name = "Minimap"
source = "thunderstore"
id = "..."
folder = "0b6d3c4e-..."
```

- `Profile::folders` is `[[folder]]`, each with an `id` (a UUID, so renames
  and syncs agree on which folder is which) and a `name`, unique in the
  profile ignoring case, so the CLI can name it.
- `ProfileMod::folder` is the id of the folder the mod is in, if any.
- **A folder is one block in the order.** Its mods are kept next to each
  other in the profile, where its first mod is, so the folder has a place in
  the load order like a mod does. Priorities still apply on top, as they do
  now. Anything that could leave them apart (a hand-edited file, two
  machines' syncs, a frontend's reorder) is put back together on load, on
  save and after syncing, by moving each folder's mods up to its first one
  (`Profile::group_folders`). Moving a mod into a folder puts it at the end
  of the folder.
- An **empty folder** has no place in the order. It's shown after
  everything else until a mod goes in. The GUI can make a folder from a
  mod, so it starts out where that mod is.
- A mod whose `folder` names a folder the profile doesn't have (one
  removed on another machine while this one moved a mod into it, say) is
  shown as not in a folder. Its `folder` is kept, since the folder's
  entry may still be on its way. Removing a folder takes its mods out of it.
  They stay in the profile, where they were.
- **On/off** for a folder turns all its mods on or off: one save, one undo.
  There's no separate `enabled` for the folder, so nothing else (lock,
  deploy, export formats) needs to know folders exist. The folder's switch
  shows on, off, or mixed.
- **Sync**: each folder is a `folder/<id>` entry holding its name, `null`
  once removed. Which folder a mod is in is part of the mod's own entry, so
  moving different mods at once all survives, as with priorities.
- **Older builds** read profiles with folders, drop the `[[folder]]`s and
  `folder`s when they save, and ignore `folder/` entries. Folders are only
  about how the list is shown, so this loses nothing that matters, and
  neither `PROFILE_SCHEMA` nor `SYNC_FORMAT` changes for them.
- **History**: each deploy's snapshot keeps the folders. Which folder a
  mod is in doesn't count as a change since the deploy (like hidden tags),
  so filing mods alone doesn't light up Revert. Going back (revert or
  rollback) keeps the folders there are now and how mods have been filed
  since, except where that would change the deploy's order: those mods are
  filed as they were then. A mod that comes back is in its old folder,
  which comes back too if it was removed (renamed if its name has been
  taken).
- **Sharing**: the patchwork zip carries folders. Formats that can't carry
  them (r2modman codes, URL lists) leave the mods unfiled.
- **GUI**: in the Mods tab's own order, a folder is a collapsible section
  that sits among the mods and is dragged as one. Its header has the name,
  how many mods and how many are off, the on/off switch and a menu (Rename,
  Remove folder). Mods are dragged within their folder, and each mod's
  menu has "Move to folder" (with "New folder…"). Whether a folder is open
  is the viewer's own preference, kept in `localStorage`. The load order
  view and search results stay flat. Hidden mods stay in the Hidden
  section.
- **CLI**: `patchwork folder list|add|rename|remove|on|off`, and
  `patchwork folder put <mod> [<folder>]` (no folder takes it out).
  `patchwork list` shows each mod's folder.

## 2. Shared sets (linked folders)

```toml
# <data>/sets/<id>/set.toml
schema = 1
id = "..."
game = "lethal-company"
name = "Client QOL"

[[mod]]
# ProfileMods, with their extension state (FOMOD picks and so on)

[config."BepInEx/config/minimap.cfg"]
# the set's own config edits
```

```toml
# in a profile, in the mods' order like a folder
[[include]]
set = "<set id>"
disabled = ["SomeMod"]   # this profile's only override
```

- A set has mods and config edits, but **no lockfile of its own**. The
  profile's lock resolves its own mods and its sets' mods together, so
  shared dependencies are resolved once. Changing a set while working in
  profile A relocks A. Profile B picks the change up the next time it's
  locked, as a change synced from elsewhere would be.
- **Editing a linked folder from inside a profile changes the set**,
  everywhere. The only thing a profile changes for itself is which of the
  set's mods are off in it (`disabled`), so working in one profile can't
  quietly change another.
- **The same mod in both** (directly in the profile and in a set it
  includes): the profile's own entry wins, and the set's is left out.
  Config edits work the same way: the profile's win over the set's, setting
  by setting.
- A set that doesn't suit the profile (a Fabric set in a Forge profile)
  raises a blocker when locking, whose fixes include "turn these off in
  this profile".
- Where it goes in the order: the include is a block, the same as a folder.
- A set is for one game. Exporting a set on its own (as a zip like a
  profile's) is a later addition.

## 3. Layers on synced profiles

| Layer | Who has it | Synced through |
|---|---|---|
| Required | everyone | the profile's document, as now |
| Optional | published to everyone, each person opts in | the profile's document (`folder/<id>` with `optional = true`) |
| Personal | this person and their linked computers | not the profile's document |

- **Optional** is a property of a folder. Each person's opt-ins are kept
  out of the shared document. They go wherever personal includes go (below).
- **Personal** additions are the person's own includes and mods on top of
  a synced profile. They're kept in profile-state on this machine, and
  synced between the person's linked computers the way `follows.toml` is.
- **Locking** is the hard part. Today whoever relocks publishes the
  `lock/` entries for everyone. With layers, the published lock covers the
  required layer and every optional folder (so whoever opts in gets the
  same versions). Personal mods are locked on top of it, **constrained to
  its versions**. If a personal mod needs a library at a version the shared
  lock doesn't have, that's a blocker for this person ("X needs Y ≥ 2, the
  pack has 1.4"). It never relocks the shared layer.
- **Catching mistakes**: where a source says which side a mod runs on
  (Modrinth's `client_side`/`server_side`), a personal mod that both sides
  need is an issue: "friends won't have this, so you'll desync".
- **Putting a personal set into a shared layer** copies it in as an
  ordinary folder of the profile. Keeping it linked for other people would
  make sets documents of their own that friends sync, which waits until
  there's a need for it.

## 4. Working on a shared profile together

No permissions to start with. Anyone who can write to an iroh document can
write anything, and real ownership would need signed entries every peer
checks. That's a lot of machinery for groups of friends. In order of cost:

1. **Who changed what**: "Alex added X to Required", from the changelog and
   the entries' authors.
2. **Folder maintainers**, by convention: editing someone else's folder
   asks "this is Sam's folder, change it anyway?".
3. **Suggestions**, only if 1 and 2 aren't enough: a change to a folder
   you don't maintain is proposed, and its maintainer accepts or declines
   it.

## Order of work

1. **Folders inside a profile**: core, sync, zip export, CLI, GUI.
   Replaces nothing yet; hidden tags stay as they are. *Done.* As built:
   the GUI files a mod from the folder button on each mod (not its menu),
   and deploy history keeps folders (see "History" above).
2. **Shared sets as linked folders**: local only, no syncing of sets.
3. **Personal layer** on synced profiles, with the split lock.
4. **Optional folders** in synced profiles.
5. **Sets synced between linked computers**, and exporting a set on its
   own.
6. Who changed what, and maintainers (section 4).
