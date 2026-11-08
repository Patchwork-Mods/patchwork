# Extensions: game-specific behaviour without game-specific core

Status (2026-11-06): steps 1 (the skeleton), 2 (`OrderedList`),
3 (`ModTransform` + `ModOptions`), 4 (`Check`), 6 (`Action`, with `uses`)
and 7 (crates per game) of "Order of work" are built, and so is step 5's
`Runner`. Step 8 (mod settings) has value fields and config files, with
readers for BepInEx and plain INI, JSON and TOML. `AccountKind` and `Provisioner` wait for Minecraft's launcher,
which was taken out of `main` (2026-11-05) and kept on the
`minecraft-launcher` branch. The sections below are the original proposal
(2026-10-29). Where what was built differs from it, the notes under each
step in "Order of work" say so, and those notes win.

## The problem

Before this work, core, the CLI and the GUI knew about particular games.
Most of these rows are gone now (see "Order of work"); the Minecraft
accounts row is still there:

| Where | What | Whose |
|---|---|---|
| `Profile::plugins`, `ProfileMod::fomod` | saved state | Bethesda, FOMOD |
| `GameDef::fomod` | game config | FOMOD |
| `Patchwork::installed` | a FOMOD stage hard-wired into every deploy | FOMOD |
| `Deployer::plugins`, `BuildContext::load_order` | a trait method only one deployer uses | Bethesda |
| `deploy_inputs` | hashes `plugins` and `fomod` by name | both |
| `sync.rs` `PLUGINS` entry | syncs the load order specially | Bethesda |
| `account.rs`, `Launch::Java`, `Via::Java` (on the `minecraft-launcher` branch now) | accounts and launching | Minecraft |
| CLI `installer`, `plugins`, `account`; GUI `installer.js`, `plugins.js` | commands and screens | all of the above |

Every new game that needed something "special" added another row. This
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
| the game itself downloaded and set up | `Provisioner` (not on `main`; see step 5) | Minecraft |
| a new way to start a game | `Runner` | Steam, Proton, Wine, umu |
| the user to sign in as someone | `AccountKind` (not built; see step 5) | Minecraft offline/Microsoft |
| a button or command that runs once | `Action` | "Sort with LOOT" |
| settings in a kind of config file shown as a form | `ConfigFormat` | BepInEx `.cfg` |
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
[ext.fomod.installer]
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

#### Mod settings

Picks cover install-time choices, but most settings are values a mod reads
from a config file at runtime. Formats with enough schema to build a form:
BepInEx `.cfg`, MCM Helper (`MCM/Config/<mod>/config.json`), BG3's
`MCM_blueprint.json`, Forge TOML and MelonLoader `.cfg`; plain
JSON/INI/TOML/YAML only by guessing types. In-game-only menus (SkyUI's MCM)
are left alone.

1. **Value fields.** A `Group` holds items to pick or fields to set
   (toggle, integer, number, text, choice, colour), and `Answer` gains
   `values`. Core checks values before `save`.
2. **Config files.** Reader extensions turn a mod's config into a form;
   game or ruleset data says which files are config. The host writes the
   changed keys into `out`, reading in-game changes back before redeploys.
3. **Readers**, by reach: BepInEx, generic JSON/INI/TOML, MCM Helper, BG3
   MCM, Forge. Keybinds become a field kind when a reader needs one.
4. **Overlays** for mods without a schema: a `patchwork.options.toml` in
   the mod or registry data describing fields or pick groups.

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

### `Runner` and `AccountKind`: starting games

A game's `[launch.<id>]` says *what* to run. Runners are *how*, and work
like the `rules` deployer and its rulesets: the involved ones are code, and
simple ones are data in `registry/runners/<id>.toml`. A launch can list
`runners`, tried in order; without a list it's `steam`, `native`, `proton`,
`wine`. See `launch/runner.rs`.

```rust
pub trait Runner: Send + Sync {
    fn id(&self) -> &str;
    fn name(&self) -> &str;
    /// What the user can change, for frontends to show.
    fn settings(&self) -> Vec<SettingState>;
    /// `Ready`, `Unsuited(why)` (wrong kind of runner for this) or
    /// `Missing(why)` (right kind, but something it needs isn't there).
    fn check(&self, ctx: &RunContext<'_>) -> Fit;
    /// Describes the command; the host runs it.
    fn plan(&self, ctx: &RunContext<'_>) -> Result<Launch>;
}
```

Java launching isn't on `main`: it went to the `minecraft-launcher` branch
with accounts and provisioning (see step 5). When it comes back it should
be a runner of its own rather than `Launch::Java` and `Via::Java`, since
Java's placeholders come from the account. Java is a reasonable built-in extension, since games other than
Minecraft (Starsector, Project Zomboid) run on Java too.

```rust
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
    fn check(&self, ctx: &CheckContext<'_>, mods: &[InstalledMod]) -> Vec<Problem>;
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
| `patchwork action [<id>] [--dry-run]` | `actions` (in `profile`), `run_action` | (new) |
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
let t = TestGame::builtin("skyrim-se")    // or from_toml / from_def
    .extension(Bethesda)
    .extension(Fomod)
    .mod_from_dir("SkyUI", "tests/fixtures/skyui")
    .mod_files("Patch", [("Patch.esp", bytes)])
    .game_file("Data/Skyrim.esm", b"...");

let order = t.order("bethesda.plugins")?;
t.answer_options("SkyUI", &answers)?;
let plan = t.deploy()?;
assert!(plan.places("Data/SkyUI_SE.esp"));
assert!(t.read("appdata:plugins.txt")?.contains("*SkyUI_SE.esp"));
```

Built (2026-11-05):
- It's behind core's `testing` feature; extension crates turn it on in
  their dev-dependencies. `patchwork-bethesda/tests/harness.rs` runs the
  Bethesda load order and FOMOD installers through it.
- Everything is under one temporary directory: patchwork's state, the
  game's folder, a Wine prefix and every other root the game defines, so
  nothing is looked for or deployed outside it. Paths are written as plans
  show them (`Data/x.esp`, `appdata:plugins.txt`).
- Mods come from its own `test` source (a mod's id is its name), from a
  folder, archive or file, or from files given inline. They're added
  enabled and locked when the game is set up.
- Nothing is set up until the game is first used, so the builder methods
  come first. Setting up panics if it fails; the rest return `Result`s.
- It wraps lists (`list`, `order`, `set_order`, `set_list_enabled`),
  options (`options`, `options_for`, `answer_options`), `issues`,
  `ext_state`/`set_ext_state`, `set_enabled`, and `plan`/`deploy`, which
  return a `TestPlan` (`places`, `placed_by`, `placed`, `removed`), plus
  `deployed`, `read` and `path`. `patchwork()`, `profile()` and
  `block_on` are there for anything else.

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
     which step 7 split up.
   - Not done yet: `reads_state`. (`off` and the schema bumps came with
     step 2, `[fomod]` under `ext` with step 3, `uses` with step 6, and the
     test harness later; see "Test harness".)
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

   Built (2026-10-29), with these differences:
   - Both hooks live in `crate::options` and share one context,
     `ModContext`: `game()`, `config()` (the hook's
     `[ext.<extension>.<hook>]`), `state::<T>()` and `saved()` (the
     extension's state for that mod), `mods()` (every enabled mod as packed,
     listed only if a hook asks, since the options dialog calls `form` on
     every pick) and `files()` (the game's own, if it's set up).
   - `applies` takes a `ModPeek` rather than an `InstalledMod`, whose
     `has_near_top(path)` looks for a file at the mod's top or one folder
     down, without listing the mod's files. That keeps the "which mods have
     options" check cheap enough for every profile render. FOMOD only finds
     installers that deep now; before, deploys found them at any depth.
   - The FOMOD hooks are both called `installer`, so they share one config
     table: `[ext.fomod.installer] folder = "Data"`. The registry moves an
     old top-level `[fomod]` there.
   - `Answers` is `Vec<Answer { group, items }>` rather than a map, so two
     groups with the same id get answers in order, as FOMOD's same-named
     groups did. `Form` has `answers` (everything picked, including for
     pages that aren't shown now) for frontends to change and send back,
     and no `defaulted`: core reports `saved` from whether the mod has
     state, and each `Group` has its own `defaulted`. `Item::image` is a
     path in the mod; core resolves it to a store path in the
     `ModOptionsView` it returns.
   - `save` gets the mod and returns the extension's whole state for it.
     Forgetting (answers `None`) clears that state; there's no hook for it.
   - FOMOD group ids are `step/group`, with `\` escaping `/` and `\`.
   - A transform's error becomes a warning and leaves the mod's files as
     packed. Output files the mod hasn't got are dropped with a warning.
   - Schema 3 (unreleased) also moves each mod's `[[mod.fomod]]` to
     `[[mod.ext.fomod.picks]]`, on any load, so sync entries and archive
     imports from older builds are moved too. `PROFILE_SCHEMA` stays 3.
   - The FOMOD parser moved from core to `patchwork-builtin` (`fomod.rs`,
     now in `patchwork-bethesda` since step 7,
     with the hooks in `fomod/hooks.rs`). Core's `fomod` module, `GameDef::fomod`,
     `ProfileMod::fomod` and `Patchwork::installer`/`set_installer`/`installers`
     are gone, replaced by `mods_with_options`, `mod_options` and
     `set_mod_options`.
   - The CLI command is `patchwork options <mod> [--for <id>] …`, with
     `installer` kept as an alias. In the GUI, `installer.js` became
     `form.js`; ids and classes were renamed from `installer-*` to
     `options-*` (`.installer-option` is `.options-item`, `.mod-installer`
     is `.mod-options`), so user themes that styled them need updating.
   - Picking options isn't recorded as a `Change` in history yet.
   - Later (2026-11-06), for SMF mods' options, which change what the
     deployer builds rather than which files a mod installs: a deployer
     sees what the extensions on for the game keep for each mod in
     `InstalledMod::ext`, and `ModPeek::read_near_top(path)` lets
     `applies` look inside a small file (SMF only offers options for
     manifests that have some to pick).
4. **`Check`.** Move the plugin limit, missing masters and env mismatches
   into checks.

   Built (2026-10-29), with these differences:
   - Checks return `Vec<Problem>` (`severity`, `mods`, `list`, `message`)
     rather than `Issue`s, so an extension can't make up another kind of
     issue. Core wraps each in an `Issue` with `IssueKind::Check { id,
     list }`. `Problem::about_list(name)` says it's about one of the
     extension's own lists. `Patchwork::list` then shows it with the list,
     so it stays live while the user rearranges.
   - `CheckContext` has `game()`, `config()`, `files()` and `list(name)`
     (the extension's own lists only, until `uses` exists). In
     `Patchwork::list` it only sees the list being shown.
   - `OrderedList::check` is gone. The plugin limit is the `bethesda.limit`
     check, so `off = ["limit"]` turns it off. Its number stays as `limit`
     in `[ext.bethesda.plugins]`, since it's about the load order.
   - Missing masters stay in core's generic `after` check, not in a
     Bethesda check. Every list's problems (that check's, and the
     `ListItems::warnings` an `OrderedList` finds) now become issues with
     `IssueKind::List { id }`, naming the mod they're about. `List::warnings`
     became `List::problems` (`Vec<Problem>`); `list.js` and the CLI were
     updated.
   - Env mismatches stay in core. They come from the game's `[env]` data
     and the mods' own metadata, not from any extension, so a hook would
     add nothing.
   - `Patchwork::issues` runs lists and checks only for games that have
     some on, and only when every enabled mod is already unpacked, so it
     never unpacks. It still lists every mod's files and, for Bethesda,
     reads plugin headers each time a profile is shown. If that gets slow
     on big profiles, cache the result by deploy inputs.
   - Deploy plans get the same issues' messages as warnings.
5. **`Runner` + `AccountKind`.** This should be done *before* Microsoft
   sign-in is built, so that sign-in is built on the new hook rather than
   ported afterwards.
   - Done: `Runner`, with Steam, native, Proton and Wine ported to it,
     `Registrar::runner`, `runners` in `[launch.<id>]`, and runners as data
     (`registry/runners/`, starting with `umu`).
   - Done: picking a runner and a launch option for a game, and runner
     settings (`Runner::settings`, `[settings.<key>]` in data), in both
     frontends. They're kept in `GameInstall`.
   - Deferred (2026-11-05): `AccountKind`, a Java runner and `Provisioner`.
     A design for accounts with only offline ones to test it against would
     likely be wrong about Microsoft's (tokens, refreshing, a browser
     sign-in). So Minecraft's launcher was taken out of `main` and kept on
     the `minecraft-launcher` branch: `account.rs`, `launch/java.rs`,
     `provision.rs`, `patchwork/provision.rs`, the `patchwork-provisioners`
     crate, the CLI's `provision` and `account`, the GUI's Playing tab and
     "Set up now", `GameDef::provision` and `Paths::runtime`. Minecraft
     stays as a game for managing an instance from a launcher like Prism,
     with Modrinth, `[env]` and instances. Profiles that use an instance
     can't be launched now. It comes back as an extension, with Microsoft
     sign-in, once that's possible.
   - Tools (`plan_tool`) use the runner picked for the game if it can run
     them, then the usual ones; they don't use a launch's `runners` list.
6. **`Action`.** Add it when the first real action (LOOT sorting) is
   wanted.

   Built (2026-11-05), with LOOT sorting as the `loot` extension's `sort`
   action (in `patchwork-bethesda`, `loot.rs`), with these differences:
   - `run(&self, ctx)` takes no inputs: there's no `inputs()` form yet,
     since nothing needs one. `scope()` is `shown_with()`, the id of a list
     the action is about (frontends put it on that list's tab), or `None`
     for the profile. Nothing has a mod or game scope yet.
   - `Outcome` has `message`, `changes` and `warnings`. `StateChange` is
     `#[non_exhaustive]` and only has `List { id, order }` so far. Items
     `order` leaves out go after the ones it names, in their old order;
     tiers and switches are kept.
   - `Patchwork::run_action(profile, id, apply)` returns an `ActionReport`
     with each list's `before`, `after` and `moved` (the fewest items that
     moved, from a longest increasing run), so frontends can preview
     (`--dry-run`) or undo. Changes are only saved once the action is
     done, through `set_list`.
   - `ActionContext` has `game()`, `config()`, `files()`, `mods()`,
     `list(id)` (by full id: its own lists and those it `uses`) and
     `fetch(url)`. Fetching is the host's: it keeps the last copy and its
     ETag under `cache/actions/<extension>/`, and uses that copy, with a
     warning, if fetching fails. That keeps network and file I/O out of
     extensions for WASM.
   - `Registrar::uses(id)` is built for lists only. `Builder::open` fails
     if a used list isn't in the build; every hook of an extension is off
     for games where something it uses is off (`Patchwork::hook_on`); and
     reading or changing an undeclared list is an `Error::Extension`
     saying so, rather than a separate `NotDeclared` error. `reads_state`
     isn't built.
   - The CLI command is `patchwork action`, not `patchwork run`, which
     would be easy to confuse with `launch` and `tool`. In the GUI, a list
     tab's actions are buttons in `#list-actions` (`.list-action`), with
     undo. Profile-wide actions aren't shown in the GUI yet.
   - The LOOT sort reads the masterlist with `serde_norway` (merge keys
     applied), not libloot (GPL). Masters and unconditional `after`/`req`
     rules always hold, then groups, then the user's order; each tier is
     sorted on its own, and loops are broken with a warning. Not done:
     `condition`s (those rules are skipped), the userlist, and LOOT's
     sorting by overlapping records. Skyrim SE turns it on with
     `[ext.loot.sort] masterlist = "…/v0.29/masterlist.yaml"`, and so do
     Skyrim VR, Enderal SE, Fallout 4 (and VR) and Starfield (added
     2026-11-06). For those, `[ext.bethesda.plugins]` grew `shipped` (the
     game's own plugins that only load from plugins.txt, like Enderal's)
     and `light_flag`, `medium_flag` and `medium_limit` (Starfield's
     header flags and medium plugins). Oblivion, Fallout 3 and New Vegas
     (added 2026-11-07) load plugins by their files' dates, so they set
     `order = "timestamps"`: plugins.txt lists only the enabled plugins,
     without `*`s, and the build deploys copies of mods' plugins dated in
     the load order's order (copies, so the store's files, which other
     profiles share, keep theirs). Core's copies now keep the source's
     modification time for this. The game's own plugins keep their dates,
     so one put after a mod's is warned about. Oblivion also sets
     `header_size = 20`. Not done: Oblivion Remastered (plugins.txt in its
     Data folder, plus UE pak mods), New Vegas's `.nam` files, the Epic
     New Vegas's `FalloutNV_Epic` folder, and Oblivion's
     `bUseMyGamesDirectory=0`.
7. **Reorganize crates** into per-game extensions (step 1 of the original
   plan). By this point it is mostly moving files.

   Built (2026-11-05), with these differences:
   - Only games' code got crates of its own, not every extension:
     `patchwork-bethesda` (the `bethesda` and `fomod` extensions),
     `patchwork-helldivers` (`helldivers`, added 2026-11-05) and
     `patchwork-smf` (`smf`). Each crate's `lib.rs` has its `Extension`s.
     FOMOD stays its own extension, so other games can turn it on, but
     lives with Bethesda's code since that's who uses it now.
   - Sources, formats and the `rules` deployer stay in `patchwork-sources`,
     `patchwork-formats` and `patchwork-deployers`: they aren't about one
     game, share their dependencies, and splitting them would only add
     crates. `generic`, `thunderstore`, `nexus`, `modrinth`, `github` and
     `modio` (added 2026-11-07) stay in `patchwork-builtin`.
   - `patchwork-builtin` has a feature per game crate (`bethesda`,
     `helldivers`, `smf`, all on by default) and `register(builder)`, which adds every extension the
     build has. The frontends call that instead of naming each extension,
     so they need no `cfg`s. They don't pass the features through yet; a
     build without one sets `default-features = false` on
     `patchwork-builtin` itself.
   - The `opt-level = 2` dev profile override moved from
     `patchwork-deployers` to `patchwork-smf`, since it was for SMF.
8. **Mod settings**; see "Mod settings" under `ModOptions`.
   - Step 1 (value fields) built (2026-11-06). `Group::kind` is
     `GroupKind::Pick` or `GroupKind::Fields`, flattened in JSON (pick
     groups gain `"kind": "pick"`). Values the extension's `form` can't use
     become form warnings.
   - CLI: `patchwork options <mod> --set [[PAGE/]GROUP/]SETTING=VALUE`.
   - GUI classes: `options-fields`, `options-field`, `options-number`,
     `options-text`, `options-color`, `options-alpha`.
   - No real extension offers fields yet; only core's test one does.
   - Step 2 (config files) built (2026-11-06), with BepInEx's `.cfg` as
     the first reader (pulled forward from step 3), with these
     differences:
     - Readers are `ConfigFormat` hooks (`crate::config`), registered
       with `Registrar::config_format` as `<extension>.<name>`. They're pure:
       `read(bytes)` gives `Contents` (a title and groups of fields set to
       what the file has) and `write(bytes, answers)` gives the new bytes.
       Unlike other hooks they're used wherever data names one, whether or
       not their extension is on for the game.
     - Data: `[[config]] files = "…" format = "<ext>.<name>"` (with an
       optional `root`) in a game definition, and in rulesets, which the
       `rules` deployer hands over through `Deployer::config_files`
       (following `ruleset` includes). `bepinex.toml` says
       `BepInEx/config/**/*.cfg` is `bepinex.cfg`, from a new `bepinex`
       extension in `patchwork-builtin`.
     - Config files can be ones mods ship or ones in the game's own
       folders (usually made by the game or a mod as it runs). Settings
       are kept per profile, not per mod: `Profile::config`, by where the
       file is deployed, holds only the settings that differ from the
       file. They sync as `config/<path>` entries and go into archive
       exports. Paths are matched ignoring case.
     - Deploying applies them over the winning mod's copy, or the game's
       own (its backup, if patchwork replaced it), writes the result under
       `generated/<target>/config/<inputs>/`, and places it after the
       deployer's (and build's) placements. Every placement of a config
       file is `rewritten` and copied rather than linked, so games can't
       write into the store. Edits are in the deploy inputs.
     - Read-back: before planning a deploy or undeploy, and before showing
       or saving the deployed profile's settings, a deployed config file
       that's changed is diffed against what was deployed, and the
       settings that changed are saved into the deployed profile. The
       changed file is then kept as what's deployed
       (`config/read-back-<hash>/`), so the same changes aren't read
       twice and forgetting a file's settings sticks.
     - Edits for a file that isn't there (a game that hasn't been started
       yet) are kept, and listed as missing, with a deploy warning.
     - Core: `Patchwork::config_files`, `config_form`, `set_config` and
       `has_config_files`. CLI: `patchwork settings [FILE] [--set
       [GROUP/]SETTING=VALUE]… [--reset] [--mod NAME]` (`patchwork config`
       was already taken). GUI: a Config tab (`#tab-config`,
       `#config-filter`, `#config-files`, `.config-file`, `.config-open`,
       `.config-edited`) after the lists, opening `#options-dialog`.
     - Not done: attributing generated files to the mod that made them
       (BepInEx names them by plugin GUID), a per-mod entry point, keeping
       keys a game adds after the first deploy when a file's settings are
       rewritten (BepInEx adds missing keys back on its next start), and
       recording settings changes in history.
   - Step 3 (readers): plain INI, JSON and TOML built (2026-11-06) as
     `generic.ini`, `generic.json` and `generic.toml`, in the `generic`
     extension (`patchwork-builtin`, `plain.rs`), with these differences:
     - Types are guessed from values (`true`/`false`, whole numbers,
       numbers, text). INI also goes by Bethesda-style key names (`bX` as
       `1`/`0`, `iX`, `fX`, `sX`), and keeps how a toggle was written.
     - Comments just above a setting are its description. `Range: a ~ b`
       (or `> a`), `Allowed Values: …` and `Default: …` lines in them
       become limits, choices and defaults, so Forge and NeoForge's TOML
       needs no reader of its own. Hints the value doesn't fit are dropped.
     - Groups are INI sections, JSON objects and TOML tables, by their
       path joined with `.` (empty, titled "General", for the top). Lists,
       `null`s, inline tables and dates are skipped, with one warning.
     - Writing changes only the values (JSON with its own span-keeping
       parser, which takes comments and trailing commas; TOML with
       `toml_edit`), and adds settings, sections or objects that aren't
       there.
     - Data: both MelonLoader loader rulesets (from `registry-gen`) say
       `UserData/MelonPreferences.cfg` is `generic.toml`; the Bethesda
       games say `Data/<SKSE|F4SE|SFSE>/Plugins/*.ini` and `*.toml` are
       `generic.ini` and `generic.toml`; Minecraft says `config/**/*.toml`
       and `*.json` are `generic.toml` and `generic.json`.
     - Left: MCM Helper, BG3's MCM, JSON5 (unquoted keys), and overlays
       (step 4).
9. Remove the version-2 compatibility code once enough releases have
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
