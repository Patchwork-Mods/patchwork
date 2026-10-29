# Extensions: game-specific behaviour without game-specific core

Status: proposal (2026-10-29). Steps 1 (the skeleton) and 2 (`OrderedList`)
of "Order of work" are built; see the notes under each.

## The problem

Core, the CLI and the GUI know about particular games:

| Where | What | Whose |
|---|---|---|
| `Profile::plugins`, `ProfileMod::fomod` | saved state | Bethesda, FOMOD |
| `GameDef::fomod` | game config | FOMOD |
| `Patchwork::installed` | a FOMOD stage hard-wired into every deploy | FOMOD |
| `Deployer::plugins`, `BuildContext::load_order` | a trait method only one deployer uses | Bethesda |
| `deploy_inputs` | hashes `plugins` and `fomod` by name | both |
| `sync.rs` `PLUGINS` entry | syncs the load order specially | Bethesda |
| `account.rs`, `Launch::Java`, `Via::Java` | accounts and launching | Minecraft |
| CLI `installer`, `plugins`, `account`; GUI `installer.js`, `plugins.js` | commands and screens | all of the above |

Every new game that needs something "special" adds another row. This
proposal replaces the rows with a few **generic extension points**. Core
provides the machinery (saving, syncing, hashing, ordering, showing forms).
Extensions only describe what is specific to their game.

"Extension" (decided 2026-10-29) is used rather than "plugin" because Bethesda already uses
*plugin* for `.esp` files, and both words would show up in the same code.

## Goals

1. **Easy for someone adding a game.** Most games need no code at all. The
   ones that do should need one small trait impl, not changes across core,
   the CLI and the GUI.
2. **Easy for us to add new kinds of extension point** without breaking the
   extensions that already exist.
3. **Ready for sandboxed (WASM) extensions later.** Hooks take and return
   plain data, and the host does all filesystem I/O, as sources and
   deployers already do.
4. **Never lose user data.** A profile opened by a build without some
   extension keeps that extension's state untouched, including over sync.

## The ladder for game authors

Authors should stop at the lowest rung that works:

1. **Game TOML only.** Use the `rules` deployer, `[roots]`, `[launch]`,
   `[env]` and so on. Most Thunderstore-style games stop here.
2. **Turn on existing extensions from the TOML.** For example, `[ext.fomod]`
   for a game whose mods ship FOMOD installers, or `[ext.load-order]` for a
   new Bethesda-engine game.
3. **Write an extension.** Implement one or more hooks from the table
   below, in a crate that exports one `Extension`.

The docs for rung 3 are mostly a "you need X → use hook Y" table:

| You need… | Hook | Today's example |
|---|---|---|
| somewhere new to get mods from | `Source` | Modrinth, Nexus |
| mods' files put in place in a special way | `Deployer` | Bethesda, SMF |
| to change which files a mod installs, before deploying | `ModTransform` | FOMOD |
| the user to make choices about a mod | `ModOptions` | FOMOD wizard |
| an order the user arranges, apart from mod order | `OrderedList` | plugin load order |
| the game itself downloaded and set up | `Provisioner` | Minecraft |
| a new way to start a game | `Launcher` | Java |
| the user to sign in as someone | `AccountKind` | Minecraft offline/Microsoft |
| a button or command that runs once | `Action` | "sort with LOOT" (future) |
| problems reported before deploying | `Check` | plugin limit, missing masters |
| a new profile share format | `ProfileFormat` | r2modman codes |

## Shape of an extension

```rust
/// A bundle of things that add to what patchwork can do, usually for one
/// game or family of games.
pub trait Extension: Send + Sync {
    /// Unique and stable. It names the extension's tables in game
    /// definitions and profiles, so it can never change once released.
    fn id(&self) -> &'static str;

    /// Bumped when the shape of its saved state changes; see
    /// "Saved state" below.
    fn state_version(&self) -> u32 {
        1
    }

    fn register(&self, ext: &mut Registrar<'_>);
}
```

```rust
// crates/patchwork-bethesda/src/lib.rs
pub struct Bethesda;

impl Extension for Bethesda {
    fn id(&self) -> &'static str {
        "bethesda"
    }

    fn register(&self, ext: &mut Registrar<'_>) {
        ext.deployer(BethesdaDeployer);
        ext.ordered_list(LoadOrder);
        ext.check(PluginLimit);
    }
}
```

The frontends register extensions by calling `Builder::extension(Bethesda)`.
That call replaces the separate `.source()`, `.deployer()` and similar
calls in both `main.rs` files. It also makes per-game Cargo features
simple: one `#[cfg(feature = "bethesda")]` line each.

### Why a `Registrar` rather than one big trait

The other option is a single `Extension` trait with a default method for
every hook. That gets awkward as hooks are added: an extension that adds
two ordered lists needs some kind of collection anyway, and the trait ends
up holding every concern at once. With a registrar:

- **Adding a new kind of hook** means adding a new trait and a new
  `Registrar` method. Existing extensions don't change.
- One extension can register several hooks of the same kind.
- Each hook trait is small and self-contained. That makes it the unit to
  give a WASM interface later, one at a time, instead of all at once.

### IDs

Everything an extension registers gets an id of the form `<extension>.<name>`,
such as `bethesda.plugins` or `fomod.installer`. The registrar adds the
prefix automatically, so names can't clash between extensions. Ids are what
the profile, game definitions, sync keys and frontends use to refer to
these things.

## Turning extensions on per game

A game definition turns an extension on by having a table for it:

```toml
[ext.fomod]
folder = "Data"

[ext.bethesda.plugins]
folder = "Data"
file = "appdata:plugins.txt"
implicit = ["Skyrim.esm", "Update.esm"]
limit = 254
```

- Each hook gets **its own table** as its config (for example, the
  `bethesda.plugins` hook gets `[ext.bethesda.plugins]`), merged with the
  user's settings for it. This works the same way the deployer gets
  `[deployer]` today.
- Hooks can declare `SettingSpec`s, as deployers already do. This means
  user-editable extension settings appear in the GUI's existing settings
  form for free.
- **A table for an extension this build doesn't have** produces a warning in
  the game's status ("this game uses `fomod`, which this build of patchwork
  doesn't include"). It is not an error. The game still works, without that
  feature.
- `GameDef` gains `ext: BTreeMap<String, toml::Table>`, and `fomod` moves
  into it. The registry loader accepts the old top-level `[fomod]` and
  `[deployer.plugins]` and moves them under `ext`, so user and third-party
  registries keep working.

Hooks are **only consulted for games that turn them on**. That rule keeps a
Hitman profile from ever running Bethesda code, even though the code is in
the binary.

### Turning individual hooks off

Games in one family differ in small ways. For example, an older Bethesda
game might want the load order but not the plugin-limit check. Users may
also not want some warnings. So hooks can be turned off one by one, but
only by naming them:

```toml
[ext.bethesda]
off = ["limit"]
```

- `[ext.<id>]` turns on the **whole extension**: every hook it registers.
- `[ext.<id>.<hook>]` only **configures** a hook. Leaving it out never turns
  the hook off. If it did, a typo like `[ext.bethesda.plugin]` would
  silently drop the load order.
- Hook names in `off` that the extension doesn't have are an error, so
  typos get caught there too.
- A hook that needs config it didn't get (the load order without `file`)
  reports that in the game's status, rather than quietly doing nothing.
- Because registry layers merge key by key, a user can turn a hook off in
  their own overlay (`<config>/games/skyrim-se.toml` with just the `off`
  line) without copying the game's definition.

## Saved state

This is the most important piece, because it removes the per-game fields
from `Profile`.

```toml
# profile.toml
[ext.bethesda]
v = 1
plugins = [{ name = "Unofficial Patch.esp", enabled = true }]

[[mod]]
name = "SkyUI"
source = "nexus"
id = "3863"

[mod.ext.fomod]
v = 1
picks = [{ step = "Main", group = "Version", options = ["SE"] }]
```

```rust
pub struct Profile {
    // ...the generic fields as now...
    /// What extensions keep for the profile, by extension id. Kept as
    /// TOML so profiles round-trip through builds that lack an extension.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ext: BTreeMap<String, toml::Table>,
}

pub struct ProfileMod {
    // ...
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ext: BTreeMap<String, toml::Table>,
}
```

Hooks never touch these maps directly. They get a typed handle:

```rust
/// One extension's slot in a profile or mod, read and written as `T`.
pub struct State<'a, T> { /* ... */ }

impl<T: Serialize + DeserializeOwned + Default> State<'_, T> {
    pub fn get(&self) -> Result<T>;
    /// Only through `Patchwork`, which saves, syncs and records history.
    pub fn set(&mut self, value: T) -> Result<()>;
}
```

Core does all of the following for every extension, with no work needed
from extension authors:

- **Round-trips unknown state.** If a build lacks an extension, that
  extension's tables are kept byte-for-byte.
- **Versions it.** Core writes `v`. If the saved `v` is newer than
  `state_version()`, the hook gets a clear error, so an old build can't
  quietly overwrite state it doesn't understand. Migration from an older
  `v` is an optional `fn migrate(from: u32, table) -> table` on the
  extension.
- **Syncs it.** Profile-level state syncs as one entry per extension,
  `ext/<id>`, replacing the special `plugins` entry. Mod-level state syncs
  as part of the mod's own entry, as `fomod` already does. Conflicts follow
  the existing last-writer-wins-per-entry rule, at extension granularity.
- **Hashes it into `deploy_inputs`.** This happens generically, in id order,
  and only for extensions that are on for the game. Changing an extension's
  state makes the deploy stale without the extension doing anything.
- **Records it in history**, the same way the rest of the profile is.

Migration from today's format: `PROFILE_SCHEMA` goes to 3. Loading a
version-2 profile moves `plugin` to `ext.bethesda.plugins` and each mod's
`fomod` to `mod.ext.fomod.picks`. Sync accepts the old `plugins` key while
peers may be on older builds.

## The hooks

Every hook follows the same rules, so learning one teaches the rest:

- It takes a **context struct** (`#[non_exhaustive]`, with fields reached
  through methods). New information can be added to a context without
  breaking anyone.
- It returns **data**: serde types, with no callbacks or references into
  core.
- It **doesn't write files**. It may read the game's own files through
  `GameFiles` and mods' files through the context, as `Deployer::build`
  already can.
- Problems go in a `warnings` list rather than failing the deploy, unless
  nothing sensible can be done.
- Every context has `config()` (its `[ext…]` table merged with the user's
  settings) and `state::<T>()`.

### `ModTransform`: change what a mod installs

This replaces the FOMOD code in `Patchwork::installed`.

```rust
pub trait ModTransform: Send + Sync {
    /// Whether it has anything to do for this mod. Cheap: called for every
    /// mod in every deploy.
    fn applies(&self, ctx: &TransformContext<'_>, m: &InstalledMod) -> bool;

    /// Replaces `m`'s files with the ones it should install: a subset,
    /// renamed or moved, but always ones it had.
    fn transform(&self, ctx: &TransformContext<'_>, m: &InstalledMod) -> Result<Transformed>;
}

pub struct Transformed {
    pub files: Vec<(RelPath /* dst */, RelPath /* from */)>,
    pub warnings: Vec<String>,
}
```

Restricting output to "files the mod had" keeps the host-does-I/O boundary.
It also means the result can be explained in the conflicts view. Transforms
that need to *generate* files belong in `Deployer::build`.

If more than one transform applies to a mod, they run in registration
order. If ordering ever needs to be explicit, `after: &[&str]` can be added
to the trait as a default method without breaking anything.

### `ModOptions`: let the user make choices about a mod

This replaces the FOMOD-specific `InstallerView`, the `installer` command
and `installer.js`, using a **generic form** that frontends render without
knowing what it is for. It is the same idea as `SettingSpec`, scaled up to
multi-step choices.

```rust
pub trait ModOptions: Send + Sync {
    fn applies(&self, ctx: &OptionsContext<'_>, m: &InstalledMod) -> bool;

    /// The choices, as they stand with `answers` (or the saved ones).
    /// Called again after each change, so options can appear and
    /// disappear as the user picks.
    fn form(&self, ctx: &OptionsContext<'_>, m: &InstalledMod, answers: Option<&Answers>)
        -> Result<Form>;

    /// Turns the user's answers into the state to save.
    fn save(&self, ctx: &OptionsContext<'_>, answers: &Answers) -> Result<toml::Table>;
}

pub struct Form {
    pub title: String,
    pub pages: Vec<Page>,        // FOMOD steps
    pub summary: Option<String>, // "installs 214 files"
    pub warnings: Vec<String>,
    pub defaulted: bool,         // nothing saved yet
}
pub struct Page { pub title: String, pub groups: Vec<Group> }
pub struct Group {
    pub id: String,
    pub title: String,
    pub pick: Pick,              // ExactlyOne, AtMostOne, AtLeastOne, Any, All
    pub items: Vec<Item>,
}
pub struct Item {
    pub id: String,
    pub title: String,
    pub description: Option<String>, // Markdown
    pub image: Option<Asset>,        // a file in the mod; the host resolves it
    pub state: ItemState,            // Optional, Recommended, Required, NotUsable
    pub picked: bool,
}
pub type Answers = BTreeMap<String /* group id */, Vec<String /* item ids */>>;
```

The FOMOD extension implements both `ModTransform` and `ModOptions` and
shares its state between them. Other installer formats, such as BAIN
wizards or a Minecraft modpack's optional mods, then come almost for free
in the GUI and CLI.

### `OrderedList`: something the user puts in order

This replaces `Deployer::plugins`, `plugins.rs`'s Bethesda-only fields, the
`plugins` command and `plugins.js`. Most of `plugins.rs` (merging saved
order with what's found, `keep_missing`, moving and enabling) is already
generic, and becomes core's `order` machinery.

```rust
pub trait OrderedList: Send + Sync {
    /// Shown as the tab or command name, like "Plugins".
    fn title(&self) -> &str;

    /// What can go in the list for these mods. Unlike `Deployer::plan`,
    /// this may read mods' and the game's files.
    fn items(&self, ctx: &ListContext<'_>, mods: &[InstalledMod]) -> Result<ListItems>;

    /// Problems with an arrangement, like a plugin above its master or too
    /// many plugins. Shown live while the user rearranges.
    fn check(&self, ctx: &ListContext<'_>, order: &[OrderedItem]) -> Vec<String> {
        Vec::new()
    }
}

pub struct ListItems {
    pub items: Vec<ListItem>,
    /// Always first and not movable, like the game's own masters.
    pub fixed: Vec<String>,
    pub warnings: Vec<String>,
}

pub struct ListItem {
    pub id: String,
    pub mod_name: String,
    /// Items with a lower tier always sort above higher ones, whatever
    /// the user does: masters are tier 0, other plugins 1.
    pub tier: u8,
    /// Short labels a frontend shows next to it, like "master", "light".
    pub badges: Vec<String>,
    /// Ids that must come before it; `check` and auto-sort use these.
    pub after: Vec<String>,
    pub can_disable: bool,
}
```

Core saves the arrangement in the extension's state, merges it with what
`items` finds (as `LoadOrder::new` does now), and hands the result to the
deployer: `BuildContext::list("bethesda.plugins")` replaces
`BuildContext::load_order`.

Frontends get **one** generic reorderable-list screen with a tab per list
that's on for the game. Future lists, such as a Minecraft resource-pack
order or Cyberpunk REDmod load order, need no frontend work.

### `Launcher` and `AccountKind`: starting games

`Launch::Java` and `Via::Java` become one registered launcher out of
several. Java is a reasonable built-in extension of its own, since games
other than Minecraft (Starsector, Project Zomboid) run on Java too.

```rust
pub trait Launcher: Send + Sync {
    /// What a provisioner's `Setup::launch` is tagged with, like `java`.
    fn via(&self) -> &str;
    /// The command to run. Placeholders such as `${auth_name}` come from the
    /// account, if the game needs one.
    fn command(&self, ctx: &LaunchContext<'_>, launch: &toml::Table) -> Result<Command>;
}

pub trait AccountKind: Send + Sync {
    fn id(&self) -> &str;           // "offline", "microsoft"
    fn title(&self) -> &str;
    /// How to add one: a form (an offline name) or a sign-in flow
    /// (device code, browser redirect), described as data so every
    /// frontend can drive it.
    fn sign_in(&self) -> SignIn;
    /// The launch placeholders it fills, refreshing tokens if needed.
    async fn vars(&self, account: &toml::Table) -> Result<BTreeMap<String, String>>;
}
```

Accounts are stored per extension under `<config>/accounts/<ext>.toml`,
with secrets kept in the keychain as source secrets already are. Games say
which extension's accounts they use in `[provision]`.

### `Action`: things to run once

This is the escape hatch for features that don't fit any other hook: "Sort
plugins with LOOT", "Clean masters", "Regenerate Minecraft options.txt",
"Rebuild SMF cache".

```rust
pub trait Action: Send + Sync {
    fn title(&self) -> &str;
    /// Where frontends offer it: a game, a profile, or a mod.
    fn scope(&self) -> Scope;
    /// What it asks for before running, as a settings form.
    fn inputs(&self) -> &[SettingSpec] { &[] }
    fn applies(&self, ctx: &ActionContext<'_>) -> bool { true }
    /// Returns what it wants done: state changes, and a message. Changes go
    /// through `Patchwork` so they're saved, synced and kept in history
    /// like any other edit.
    async fn run(&self, ctx: &ActionContext<'_>, inputs: &toml::Table) -> Result<Outcome>;
}

pub struct Outcome {
    pub message: Option<String>,
    pub state: Vec<StateChange>, // set this extension's profile or mod state
    pub warnings: Vec<String>,
}
```

Frontends list actions generically: the CLI as `patchwork run <id>`, the
GUI as a menu on the game, profile or mod. Because actions can only return
state changes, they can't leave a profile half-edited. That restriction can
be loosened later by adding new `StateChange` variants.

### `Check`: problems to show before deploying

```rust
pub trait Check: Send + Sync {
    fn check(&self, ctx: &CheckContext<'_>, mods: &[InstalledMod]) -> Vec<Issue>;
}
```

These feed the existing `issue` machinery. Missing dependencies, the
plugin limit, and "this Minecraft mod is for Forge but the profile uses
Fabric" all become checks, instead of warnings tucked inside other hooks.

## Frontends become generic

After this change, the CLI and GUI talk to extensions through a handful of
generic commands:

| CLI | GUI command | Replaces |
|---|---|---|
| `patchwork options <mod>` | `mod_options`, `set_mod_options` | `installer`, `set_installer` |
| `patchwork order [<list>] list/move/enable/disable` | `lists`, `list`, `set_list` | `plugins …`, `load_order`, `set_load_order` |
| `patchwork account [add <kind>/list/use/forget]` | `accounts`, `sign_in` | `account offline/forget` |
| `patchwork run <action>` | `actions`, `run_action` | (new) |
| `patchwork game settings` (unchanged) | `game_settings` (now includes extension settings) | |

`Patchwork` gets a small discovery API so frontends can show only what is
on for the current game:

```rust
impl Patchwork {
    /// The ordered lists, actions, option-bearing mods and account kinds
    /// that are on for this profile's game.
    pub fn features(&self, profile: &ProfileRef) -> Result<Features>;
}
```

`installer.js` and `plugins.js` become `form.js` and `list.js`. They are
renderers for the `Form` and `ListItems` types, and know nothing about
FOMOD or Bethesda.

**Custom GUI code from extensions is out of scope.** Every extension point
above is rendered by the host from data. That gives a consistent look,
works in both the CLI and the GUI, and is what makes WASM extensions
practical later. If extensions eventually need richer UI, the next step
would be more building blocks (a table view, a tree view) rather than
arbitrary JS.

## Rules for us when adding a new extension point

1. Find at least two real uses, or one real use and one very likely one,
   before adding a hook. Otherwise use `Action`.
2. A new hook is a new trait, a new `Registrar` method, and a new entry in
   the author table above. Never add required methods to an existing hook.
   New methods get defaults.
3. Contexts are `#[non_exhaustive]` and use accessors. Outputs are plain
   serde data. Inputs never include `&Patchwork` or a writable path.
4. If a hook's results affect what gets deployed, it must go through the
   generic `deploy_inputs` hashing. Extensions should never need to remember
   to make deploys stale.
5. The frontends render the hook's output generically. If a hook needs a
   custom screen, the data model is probably wrong.
6. Write the hook against the test harness (below) before porting a real
   extension to it.

## Test harness

`patchwork_core::ext::testing` gives extension authors what our own tests
already build by hand:

```rust
let t = TestGame::new("skyrim-se.toml")   // or an inline TOML
    .extension(Bethesda)
    .extension(Fomod)
    .mod_from_dir("SkyUI", "tests/fixtures/skyui")
    .game_file("Data/Skyrim.esm", b"...");

let order = t.list("bethesda.plugins")?;
t.answer_options("SkyUI", answers)?;
let plan = t.deploy()?;
assert!(plan.places("Data/SkyUI_SE.esp"));
```

## Order of work

Each step leaves `main` working and shippable.

1. **Skeleton.** `Extension`, `Registrar`, `Builder::extension`, plus
   `ext` maps on `GameDef`, `Profile` and `ProfileMod`, with round-tripping,
   versioning, sync and hashing. The existing sources, deployers,
   provisioners and formats get registered through extensions. No behaviour
   changes yet.
   Built, with these differences from the sections above:
   - Sources, deployers, provisioners and formats keep their existing ids
     (`nexus`, `bethesda`, ...), not `<extension>.<name>`, since profiles
     and game TOML already name them. Only new kinds of hook get prefixed.
   - They aren't gated by `[ext.<id>]` either: games pick them as before
     (`[deployer] kind`, `[sources]`). Gating applies to the new hooks.
   - State is read and written through `Patchwork::ext_state` and
     `set_ext_state` (with `StateOf::Profile` or `StateOf::Mod(name)`).
     Contexts' `state::<T>()` will wrap the same functions. Saving the
     default (an empty table) removes the extension's table.
   - `migrate` is called once, from the saved version straight to the
     current one.
   - The extensions live in a new `patchwork-builtin` crate (`generic`,
     `thunderstore`, `nexus`, `modrinth`, `bethesda`, `smf`, `minecraft`),
     which step 7 splits up.
   - Not done yet: `uses`/`reads_state`, the test harness and moving
     `[fomod]` under `ext`. (`off` and the schema bumps came with step 2.)
2. **`OrderedList`.** Port the Bethesda load order, add the generic list
   command and screen, migrate `plugin` → `ext.bethesda`. This is the
   smallest port, and it proves state, sync, hashing and generic UI end to
   end.

   Built (2026-10-29), with these differences:
   - Items are keyed by `name` (not `id`), matched ignoring ASCII case.
   - `ListItems` also has `tiers` (names for frontends' headings) and
     `ListItem` a `detail` (hover text); `fixed` is `Vec<ListItem>` so it can
     carry badges (the plugin limit needs to know which are light).
   - `check(&self, ctx, list: &List)`; core does the generic `after`
     check itself, so Bethesda's `check` is only the plugin limit.
   - `ListContext` gives `game()`, `deploy()` (the deployer's context),
     `files()` and `config()`; no user settings for hooks yet.
   - The saved order lives under the list's name in the extension's state;
     typed state written with `set_ext_state` keeps those keys.
   - `off` is in, with unknown names reported as an `UnknownHook` issue.
   - `PROFILE_SCHEMA` is 3 and `SYNC_FORMAT` 2. Loading migrates schema 2's
     `[[plugin]]` (profiles, history, archive imports) and sync format 1's
     `plugins` entry; the registry moves `[deployer.plugins]` under
     `[ext.bethesda.plugins]`.
   - History has `Change::List { id, title }` for a list's order changing.
   - The CLI command is `patchwork order [--list <id>] …`, with `plugins`
     kept as an alias. GUI ids and classes were renamed from `plugin-*` to
     `list-*` (user themes that styled them need updating).

3. **`ModTransform` + `ModOptions`.** Port FOMOD into its own extension and
   add the generic form renderer.
4. **`Check`.** Move the plugin limit, missing masters and env mismatches
   into checks.
5. **`Launcher` + `AccountKind`.** Port Java and Minecraft accounts. This
   should be done *before* Microsoft sign-in is built, so that sign-in is
   built on the new hook rather than ported afterwards.
6. **`Action`.** Add it when the first real action (LOOT sorting) is
   wanted.
7. **Reorganize crates** into per-game extensions (step 1 of the original
   plan). By this point it is mostly moving files.
8. Remove the version-2 compatibility code once enough releases have
   passed.

## Using another extension's work

Extensions can build on each other. For example, a LOOT extension sorts
the Bethesda load order. The danger isn't security: compiled-in extensions
are trusted Rust code and could read the profile file anyway. The danger is
that one extension's saved format quietly becomes another's API. The rules
below are designed around that.

1. **Use results, not raw state.** An extension asks core for another
   extension's hook output by id, through the same API the frontends use,
   for example `ctx.list("bethesda.plugins")` for the merged load order.
   The owner can change how it stores things without breaking anyone.
2. **Changes go through the owner.** An action that wants a new order
   returns `StateChange::List { id: "bethesda.plugins", order }`. Core
   applies it with the generic list machinery, so the owner's rules (tiers,
   for example) still hold. Only an extension can write its own state.
3. **Declare what you use.** In `register`:

   ```rust
   ext.uses("bethesda.plugins");
   ```

   Core then:
   - refuses to register the extension with a clear message if the
     dependency isn't in the build;
   - turns the extension's hooks on only for games where everything it
     uses is on;
   - gives `NotDeclared` errors for undeclared access, so dependencies
     can't be added by accident.

   When WASM extensions arrive, this list is what the host enforces.
4. **Raw state is a declared last resort.**

   ```rust
   ext.reads_state("bethesda", 1..=1);
   let saved: Saved = ctx.foreign_state("bethesda")?;
   ```

   This is read-only. If the owner's `v` falls outside the declared range,
   the read fails instead of misreading the data. Owners can see who
   depends on their format by searching for `reads_state("<id>"`.

## Open questions

None at the moment. Decided so far:

- The name is "extension".
- Extensions may use each other's results and, as a declared last resort,
  read each other's state (see "Using another extension's work").
- Game TOML can turn individual hooks off, but only by naming them (see
  "Turning individual hooks off").
- `ModTransform` only ever installs files from its own mod. Anything that
  combines files from several mods belongs in `Deployer::build`.
