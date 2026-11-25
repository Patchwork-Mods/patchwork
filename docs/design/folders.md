# Folders, shared sets and layered profiles

Status (2026-11-24): steps 1 (folders inside a profile) and 2 (shared
sets) are done, and step 3 (the personal layer) is under way; the rest is
a proposal. Where what gets built differs from this, the notes under
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
   so there's one idea to learn: it's a folder, and this one is in more
   profiles than one. Users never make a set as such: they use another
   profile's folder in this one too, and the set is how that's kept.
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
- **A set only exists while it's shared.** A folder is kept in its
  profile's own file until a second profile uses it; then it becomes a set
  that both include. Once only one profile includes a set, it's that
  profile's own folder again, and once none does it's gone (see "Folders
  in more profiles than one" below).

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
   *Done.* As built:
   - **Loading expands, saving splits.** `load_profile` gives the profile
     with each set's mods in its block, as a linked folder (the mods'
     `folder` is the set's id), the profile's `disabled` applied as their
     `enabled`, and a set's mod left out where the profile has the same
     mod. `save_profile` splits it back: the profile's own mods, each
     include's place and `disabled`, and the mods of any set that changed,
     saved to the set. So the code that reads a profile sees everything
     it installs, and editing a mod in a linked folder edits the set,
     with no code of its own. A set's mod whose name the profile already
     uses is shown renamed ("Minimap (Client QOL)") and saved by its own
     name.
   - **Where an include goes**: `[[include]] after = "<mod>"`, the
     profile's own mod it comes after (none: first). One that names a mod
     the profile no longer has goes last.
   - **Schema**: `PROFILE_SCHEMA` is 4, but a profile is only saved as 4
     when it includes a set, so older builds still open the rest (and
     zips, which never include sets). A set's file has its own
     `SET_SCHEMA`.
   - **Going back** (revert, rollback) to a deploy whose sets have
     changed since is a blocker: put the set back as it was (changing the
     other profiles that include it), copy the old one in as a folder, or
     keep the set as it is (`Fix::GoBack`, `rollback --sets`). Each deploy
     keeps its sets as they were. A fix that picks one goes back itself,
     so the GUI's window fix resolves to `{ done }` rather than retrying.
   - **A missing set** (removed, or the profile came from another
     computer) is a `MissingSet` issue, whose fix stops including it. The
     include is kept until then, in case the set comes back.
   - **A set that doesn't suit the profile** (a Fabric set in a Forge
     profile) needs nothing of its own: its mods get the usual
     `IncompatibleEnv` issues, and turning them off is the profile's own.
   - **Synced profiles** can't include a set yet (a blocker whose fix,
     `CopySet`, copies it in as a folder), and one that includes a set
     can't sync (`UnlinkSets`). Including sets in synced profiles is step 3.
   - **Exports** turn each linked folder into an ordinary folder.
   - **GUI**: `ProfileDetail::folders` has the linked folders too, after
     the profile's own, each with `linked` and `alsoIn` (the other
     profiles that include its set, by name). A linked folder is marked
     "Shared with N profiles" and its menu has Rename…, Stop sharing
     (keeping its mods) and Remove from this profile. Removing a linked
     folder's mod, or taking it out of the folder, says it leaves every
     profile that has it. (Superseded: this step first had a separate
     Add shared folder button listing the game's sets, and Share with
     other profiles on each folder; see "Folders in more profiles than
     one".)
   - **A set's own config edits** (`[config."<file>"]` in `set.toml`,
     saved as `SET_SCHEMA` 2 only when there are any, so older builds
     don't open and drop them). Loading merges them into the profile's
     `config` like the mods: a later include's over an earlier one's, the
     profile's own over all of them, setting by setting. Saving keeps as
     the profile's own only settings that aren't what its sets give
     (`set::own_config`), so saving a profile never changes a set's config.
     A set's config changes only by saving to it: `set_config` takes where
     to (`SaveTo`), by default the set whose linked folder the file's mod
     is in (or the set that alone changes it), else the profile. Saving
     to a set saves the whole form there and drops the profile's own
     changes to that file, since the form showed them. Saving to the
     profile keeps a value even when it's the file's own, if a set has it
     otherwise. Forgetting the profile's own goes back to the sets'.
     - Settings changed in game are read back as the profile's own.
     - Deploying doesn't warn about a missing file that only sets change
       (its mod can be off in this profile).
     - Sharing a folder moves the profile's edits to its mods' files into
       the set (if the game's set up to tell which those are). Unlinking
       keeping the mods, copying a set in and going back with "copy"
       make the set's edits the profile's own, under its own. Unlinking
       without them drops them.
     - Deploy history doesn't keep config edits, so going back leaves them
       as they are and they don't make going back a blocker.
     - The CLI's `settings` has `--to profile|<folder>` and says where each
       setting comes from. The GUI's settings dialog has a "Save changes
       in" choice and marks settings "From <set>" or "Changed here, over
       <set>". The Config tab badges files a set changes.
   - Order: core model and expanding/splitting; editing (sets, turning a
     folder into a set, include, unlink) and the CLI; lock, deploy and
     history with their blockers; exports and the sync blocker; GUI;
     sets' config edits.
   - **Folders in more profiles than one** (reworked after the rest of
     step 2, so sets aren't a second idea next to folders):
     - **One way in.** The Mods tab's Add folder button has New folder…
       and From another profile…, which lists the folders of the game's
       other profiles (`other_folders`): their own folders, and the sets
       they include that this profile doesn't, each once with every
       profile that has it. Each can be added as the same folder (linked)
       or copied in as this profile's own (`use_folder` with a
       `FolderSource` and `link`). The list says which of its mods the
       profile has already (its own entries win) and, for synced
       profiles, why a folder can only be copied in.
     - **Sets are made and removed by themselves.** Using another
       profile's own folder linked shares it there (`share_folder`) and
       includes it here. Whenever a profile stops including a set
       (`save_profile` compares what it stored before, and
       `remove_profile`), `settle_set` turns a set only one profile still
       includes into that profile's own folder (`unlink_set` keeping its
       mods and config edits) and deletes one none includes. Going back
       with "restore" can still put back a set only one profile includes;
       it settles the next time one stops including it.
     - **Synced profiles**: using a folder linked when either profile is
       synced is the `CopyFolder` blocker, whose fix copies it in (a
       window fix that's done, like `CopySet`).
     - **CLI**: `patchwork folder others` lists them, `patchwork folder
       use <folder> [--from <profile>] [--copy]` uses one. `set new` and
       `set share` are gone; `set remove` is for sets left over from
       before sets went by themselves, which the list shows as in no
       profile (adding one linked settles it into the profile's own).

3. **Personal layer** on synced profiles, with the split lock. *Under
   way.* As built so far:
   - **It's a private set.** The personal layer is kept like a shared set
     that only its profile includes, whose id is the profile's own, in
     `profile-state/<id>/personal.toml` (its mods, and the include's
     `after` and `disabled`). Loading includes it, so its mods show as a
     linked folder, "Personal", and everything that reads a profile sees
     them; saving writes it back there, never to `profile.toml`, and
     removes the file once it has no mods. `load_set`/`save_set` take a
     profile's id to mean its personal layer, so going back and the
     expanding and splitting of step 2 work on it unchanged.
   - **Only synced profiles get one** (`set_personal`, `folder personal
     <mods>` / `folder publish <mods>`, and the GUI's folder button: Keep
     on this computer only / Share with everyone). Moving a mod in
     publishes its removal for everyone else; moving it out publishes
     it. A shared set's mod can't be made personal. The Personal folder
     can't be renamed or removed; it goes when its last mod does.
     Duplicating a profile turns it into an ordinary folder.
   - **Syncing leaves it out**: `sync_entries` gives the profile without
     it (`personal::without`), so a follower's personal mods aren't
     `unshared_mods` either, and what arrives never touches it.
   - **The split lock**: what only personal mods lead to is kept in
     `personal.lock` beside it (`personal::split_lock` on `save_lock`,
     merged back by `load_lock`), so `profile.lock` stays what's synced.
     Locking does the shared mods first, then the personal ones on top
     (`lock_layer`), which can't relock anything the shared lock has. A
     personal mod needing a newer version of one of those is a blocker
     whose fix removes it.
   - **Linked computers** keep it in step through the devices document
     (patchwork-sync's `personal`): each profile's layer and what only it
     needs locked is one entry, `personal/<profile id>` (a
     `personal::Layer` as JSON, or removed), and the newest wins, like
     `follows.toml`. What it was when last in step is kept in
     `<data>/sync/personal/<id>`, so a change here is told from one that
     arrived. The first time, a computer that already had personal mods
     for the profile adds them to what's there (`Layer::merge`) rather
     than losing them. Taking it (`apply_personal`) replaces this
     computer's layer and lockfile and downloads what's new, without
     locking. Its local mods' files go in the devices document too, and
     a profile's document now only gets its shared mods' files
     (`load_shared_lock`), so friends never get personal ones. A
     profile that arrives later gets its layer as soon as it does. The
     GUI says "Only on your computers" once linked ("Only on this
     computer" until then), and so does the CLI.
   - **Personal includes of shared sets.** A synced profile includes a
     set in its personal layer (`include_set` and `use_folder` do it
     themselves when the profile's synced; the `CopySet` blocker is
     gone). The includes are kept in `personal.toml` (`[[include]]`,
     `PERSONAL_SCHEMA` 2, saved as 1 while there are none), with the
     layer's own place among them as an include of the profile's id so
     ties keep their order (`Personal::includes`/`set_includes`). As
     loaded, every include that's part of the layer has
     `Include::personal`, which `is_personal`, the split lock,
     `without` (which leaves out their config edits too) and the
     save's split go by; deploy history keeps it. `includers` and
     `stored_includes` read them too, so a set isn't settled away from
     under one. A set only a synced profile still includes, personally,
     stays a set rather than becoming an ordinary folder (which would go
     to everyone). Stopping sharing it keeping its mods puts them in the
     personal layer; going back with "copy" past a change to it does the
     same, and so does it for the layer itself. Its config edits apply
     but can't be saved to (as for the layer), and are dropped when it's
     unlinked. A synced profile's own folders still can't become sets
     (`CopyFolder`), and a profile that includes sets in its file still
     can't start syncing as it is (`UnlinkSets`, or `IncludePersonally`;
     see below). The sets themselves aren't
     synced between linked computers yet (step 5): another computer gets
     the include, and a `MissingSet` issue until it has the set. GUI:
     the folder's marked "Only on your computers" next to "Shared with
     N profiles", and "From another profile…" says so before adding.
   - **Personal config edits.** The personal layer keeps settings of its
     own (`[config."<file>"]` in `personal.toml`, `PERSONAL_SCHEMA` 3,
     saved as that only when there are any), which go to linked
     computers with the rest of the `Layer`. They, and those of sets
     included personally, go **over** the profile's own settings, not
     under them like a shared set's: they're the person's own choice on
     top of what everyone has. So the loaded profile keeps what the
     personal ones gave and what of the profile's own they hide
     (`Profile::personal_config`), and `own_config` gives the hidden
     values back, so saving never publishes a personal value or drops
     the profile's own. A layer with only settings is still included
     (its empty folder isn't shown: `Profile::shown_folders`).
     - `set_config` saves to the layer with `SaveTo::Set(<profile id>)`
       (making it if need be; a synced profile's `ConfigView::sets`
       always offers it) or to a set included personally. Saving there
       leaves the profile's own as they are. Saving anywhere else keeps
       settings the personal layer changes as they were there, since the
       form showed the personal values. By default a file whose mod is
       personal, or whose settings the layer already changes, saves to
       it.
     - Settings changed in game that the personal layer gives go back to
       it (or the set included personally that gives them); the rest are
       the profile's own, as before.
     - Unlinking a set included personally keeping its mods, and going
       back with "copy" past a change to one, put its settings in the
       layer, under the layer's own.
     - `ConfigSet` and `SharedSetting` say whether they're `personal`.
       GUI: "Save changes in" offers "Just for you (only on your
       computers)", and a setting is marked "Yours" or "Yours, over the
       profile's". CLI: `settings --to personal`.
   - **Followed profiles' additions go in it.** What was last synced
     records whether this computer only follows the profile
     (`is_followed`), and `save_profile` moves the profile's own mods
     whoever shares it doesn't have into the personal layer
     (`keep_added`), splitting the lockfile again. So whatever adds a
     mod puts it there, and it goes to the person's other computers
     rather than staying on this one. Mods a follower added before
     this, kept in the profile itself, move the next time it syncs.
     Nothing can be moved in or out of a followed profile's layer by
     hand (`set_personal` refuses, and the GUI doesn't offer it): the
     rest is whoever shares it's. `unshared_mods` and the GUI's "only
     here" mark are gone, since those mods are in the Personal folder.
   - **Going back** puts the personal layer back as it was, like the
     profile's own mods, without asking: it's only this profile's (and
     goes to the person's other computers as it was). Sets included
     personally that changed since are asked about like any shared set,
     and the blocker says "copy" puts their mods in the Personal folder.
   - **Starting to sync a profile that includes sets**: the blocker
     (`UnlinkSets`, copy them in, which everyone then gets) has a second
     fix, `IncludePersonally`, which makes its includes personal
     (`include_personally`, `patchwork set personal <set>`), splitting
     the lockfile so what only they need stays out of what's synced.
   - Still to do: the client/server-side issue.
4. **Optional folders** in synced profiles.
5. **Sets synced between linked computers**, and exporting a set on its
   own.
6. Who changed what, and maintainers (section 4).
