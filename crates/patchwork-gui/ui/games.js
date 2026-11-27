// The game picker, adding a game, and a game's settings.

import { gameInfo, invoke, profilesOf, run, state, status } from "./state.js";
import { $, copyString, h, initials, pickFolder } from "./dom.js";
import { onContext } from "./menu.js";
import { refresh, renderProfile, renderSidebar, switchProfile } from "./profile.js";
import { selectTab } from "./browse.js";
import { go } from "./nav.js";
import { openSettings, readSetting, settingField } from "./settings.js";
import { applyTheme, renderGameTheme } from "./theme.js";
import { renderGameAppearance, saveGameAppearance, setGameAppearance } from "./editgame.js";
import { autosave } from "./autosave.js";

function showScreen(screen) {
  document.body.dataset.screen = screen;
  $("#games-screen").hidden = screen !== "games";
  $("#sidebar").hidden = screen !== "game";
  $("#main").hidden = screen !== "game";
}

export async function showGames() {
  state.game = null;
  state.profile = null;
  // Theme first, so the menu doesn't show for a frame in the game's look.
  // It's cached after the first time, so this doesn't wait on main.rs.
  await applyTheme(null);
  renderGames();
  showScreen("games");
}

const gameSearch = $("#game-search");
const gameQuery = () => gameSearch.value.trim();
let renders = 0;

/**
 * Lays out the games matching the search, best first: the favorites, then
 * the ones with an install folder set, then the rest, with adding a game at
 * the very end.
 */
async function renderGames() {
  const render = ++renders;
  const { games } = state.overview;
  const query = gameQuery();
  const shown = query ? await findGames(games, query).catch(() => null) : games;
  if (!shown || render !== renders) return;
  const favorites = new Set(state.overview.favorites);
  const rest = shown.filter((g) => !favorites.has(g.id));
  // a game whose folder has gone shows as not installed, but keeps its settings
  const installed = rest.filter((g) => g.install && !g.missing);
  const others = rest.filter((g) => !g.install || g.missing);
  fillSection($("#favorite-games"), shown.filter((g) => favorites.has(g.id)));
  // While searching, adding a game is only offered when nothing matches.
  fillSection($("#installed-games"), installed);
  fillSection($("#other-games"), others, query ? null : addGameCard());
  $("#games-empty").hidden = shown.length > 0 || !query;
  $("#games-empty").replaceChildren(
    `No games match “${query}”. `,
    h("button", { type: "button", class: "link", onclick: openGameDialog }, "Add a game"),
  );
}

/** Fills in a section's cards and count, hiding it when it'd be empty. */
function fillSection(section, games, extra = null) {
  section.querySelector(".game-grid").replaceChildren(...games.map(gameCard), ...(extra ? [extra] : []));
  section.querySelector(".count").textContent = games.length;
  section.hidden = !games.length && !extra;
}

/**
 * The games matching `query`, best first; see matching.rs. Only names are
 * searched.
 */
async function findGames(games, query) {
  const ids = await run("find_games", { query });
  const byId = new Map(games.map((g) => [g.id, g]));
  return ids.map((id) => byId.get(id)).filter(Boolean);
}

function gameCard(g) {
  const count = profilesOf(g.id).length;
  const card = h(
    "li",
    {},
    favoriteButton(g),
    editButton(g),
    h(
      "button",
      { class: "game-card", "data-game": g.id, onclick: () => go(() => openGame(g.id)) },
      gameArt(g),
      h("span", { class: "game-name" }, g.name),
      h("span", { class: "muted" }, `${count} profile${count === 1 ? "" : "s"}`),
      g.missing ? h("span", { class: "badge", title: `Not found at ${g.install}` }, "Folder missing") : null,
      g.origin === "user"
        ? h("span", { class: "badge" }, "Added by you")
        : g.origin.endsWith("+ user")
          ? h("span", { class: "badge" }, "Customized")
          : null,
    ),
  );
  onContext(card, () => gameMenu(g));
  return card;
}

/** What right-clicking a game on the picker offers. */
function gameMenu(g) {
  const on = state.overview.favorites.includes(g.id);
  const profiles = profilesOf(g.id);
  return [
    { label: "Open", run: () => go(() => openGame(g.id)) },
    profiles.length > 1 && {
      label: "Open profile",
      items: profiles.map((p) => ({ label: p.name, run: () => go(() => openGame(g.id, p.id)) })),
    },
    "-",
    { label: on ? "Unfavorite" : "Favorite", run: () => setFavorite(g, !on) },
    { label: "Game settings…", run: () => openGameSettings(g.id, "general") },
    { label: "Edit appearance…", run: () => openGameSettings(g.id, "appearance") },
    g.install && "-",
    g.install && {
      label: "Open install folder",
      title: g.install,
      disabled: g.missing,
      run: () =>
        invoke("open_game_folder", { game: g.id, folder: "install" }).catch((err) => status(String(err), true)),
    },
    g.install && { label: "Copy install folder path", title: g.install, run: () => copyString(g.install, "Copied the path") },
  ];
}

const starPath = "M12 2.5l2.9 6 6.6.8-4.9 4.6 1.3 6.5L12 17.2l-5.9 3.2 1.3-6.5-4.9-4.6 6.6-.8z";

/** The star in a card's corner, which pins the game to the top. */
function favoriteButton(g) {
  const on = state.overview.favorites.includes(g.id);
  const label = on ? `Unfavorite ${g.name}` : `Favorite ${g.name}`;
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
  path.setAttribute("d", starPath);
  svg.append(path);
  return h(
    "button",
    {
      type: "button",
      class: "favorite",
      title: on ? "Unfavorite" : "Favorite: show it above the rest",
      "aria-label": label,
      "aria-pressed": String(on),
      onclick: () => setFavorite(g, !on),
    },
    svg,
  );
}

/** The pencil in a card's other corner, which opens the game's settings on Appearance. */
function editButton(g) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  for (const d of ["M12 20h9", "M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z"]) {
    const path = document.createElementNS("http://www.w3.org/2000/svg", "path");
    path.setAttribute("d", d);
    svg.append(path);
  }
  return h(
    "button",
    {
      type: "button",
      class: "edit-game",
      title: "Change its name, picture or color",
      "aria-label": `Change how ${g.name} looks`,
      onclick: () => openGameSettings(g.id, "appearance"),
    },
    svg,
  );
}

async function setFavorite(g, favorite) {
  try {
    await invoke("set_favorite", { game: g.id, favorite });
  } catch (err) {
    status(String(err), true);
    return;
  }
  const favorites = state.overview.favorites.filter((id) => id !== g.id);
  if (favorite) favorites.push(g.id);
  state.overview.favorites = favorites;
  await renderGames();
  // The card moved sections, so keep focus on its star for the keyboard.
  document.querySelector(`.game-card[data-game="${CSS.escape(g.id)}"]`)?.parentElement
    .querySelector(".favorite")
    ?.focus();
}

/** The game's picture, or its initials if it has none or it won't load. */
function gameArt(g) {
  const art = h("div", { class: "game-art", "aria-hidden": "true" }, initials(g.name));
  if (g.image) {
    const img = h("img", {
      src: g.image,
      alt: "",
      loading: "lazy",
      decoding: "async",
      onerror: () => img.remove(),
    });
    art.append(img);
  }
  return art;
}

function addGameCard() {
  return h(
    "li",
    {},
    h(
      "button",
      { class: "game-card add", onclick: openGameDialog },
      h("div", { class: "game-art", "aria-hidden": "true" }, "+"),
      h("span", { class: "game-name" }, "Add a game"),
      h("span", { class: "muted" }, "Set up one patchwork doesn't know yet"),
    ),
  );
}

/**
 * Looks for every game (see detect_games in main.rs), setting up the ones
 * found and moving the ones whose folder has gone, then shows the result.
 * `quiet` says nothing unless something changed, for when it's done on its
 * own as Patchwork opens.
 */
export async function detectGames({ quiet = false } = {}) {
  const button = $("#detect-games");
  button.disabled = true;
  if (!quiet) status("Looking for games…");
  let detected;
  try {
    detected = await invoke("detect_games");
  } catch (err) {
    if (!quiet) status(String(err), true);
    else console.warn(err);
    return;
  } finally {
    button.disabled = false;
  }
  for (const [game, err] of detected.failed) console.warn(`couldn't set up ${game}: ${err}`);
  const changed = detected.added.length + detected.moved.length;
  if (changed || !quiet) {
    state.overview = await run("overview");
    // only redraw the picker; an open game's page is left as it is
    if (!state.game) await renderGames();
  }
  if (changed || !quiet) status(detectedSummary(detected));
}

function detectedSummary({ added, moved, missing, ambiguous, failed }) {
  const names = (ids) => {
    const shown = ids.slice(0, 3).map((id) => gameInfo(id)?.name ?? id);
    return ids.length > 3 ? `${shown.join(", ")} and ${ids.length - 3} more` : shown.join(", ");
  };
  const games = (n) => `${n} game${n === 1 ? "" : "s"}`;
  const parts = [];
  if (added.length) parts.push(`Found ${games(added.length)}: ${names(added)}`);
  if (moved.length) parts.push(`found ${names(moved)} in a new folder`);
  if (ambiguous.length) parts.push(`${names(ambiguous)} found in more than one place; open it to pick`);
  if (missing.length) parts.push(`couldn't find ${games(missing.length)} where they were set up`);
  if (failed.length) parts.push(`couldn't set up ${names(failed.map(([id]) => id))}`);
  if (!parts.length) return "No new games found";
  const text = parts.join("; ");
  return text.charAt(0).toUpperCase() + text.slice(1);
}

$("#detect-games").addEventListener("click", () => detectGames());

gameSearch.addEventListener("input", () => {
  if (state.overview) renderGames();
});

gameSearch.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && gameSearch.value) {
    e.stopPropagation();
    gameSearch.value = "";
    gameSearch.dispatchEvent(new Event("input"));
  }
});

/**
 * Shows a game's profiles, opening `profile` if given, or else the one last
 * used for this game, or else its first one.
 */
export async function openGame(id, profile = null) {
  state.game = id;
  state.profile = null;
  // a profile opening forgets these too, but there may not be one
  state.search = null;
  state.filters = {};
  selectTab("mods");
  const last = state.overview.lastProfiles[id];
  const ps = profilesOf(id);
  const pick = ps.find((p) => p.id === profile) ?? ps.find((p) => p.id === last) ?? ps[0];
  // Load before showing the screen, so the last game's mods don't flash up
  // while this one's profile is fetched.
  try {
    if (pick) await switchProfile(pick.id);
    else renderProfile();
  } finally {
    await applyTheme(id);
    renderSidebar();
    showScreen("game");
  }
}

$("#back").addEventListener("click", () => go(showGames));

// Adding a game. Writes <config>/games/<id>.toml through create_game in
// main.rs; anything the form doesn't cover can be added to that file later.

const gameForm = $("#new-game");
/** Whether the id has been typed in, rather than filled in from the name. */
let idEdited = false;

const slug = (name) =>
  name.toLowerCase().normalize("NFKD").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

function openGameDialog() {
  gameForm.reset();
  idEdited = false;
  $("#new-game-error").hidden = true;
  $("#new-game-rulesets").replaceChildren(
    h("span", {}, "Install rules"),
    ...state.overview.rulesets.map((r) =>
      h(
        "label",
        { class: "row" },
        h("input", { type: "checkbox", name: "rulesets", value: r }),
        h("span", {}, r),
        r === "bepinex"
          ? h("small", { class: "muted" }, "most Thunderstore games")
          : r === "recursive-melonloader"
            ? h("small", { class: "muted" }, "Thunderstore games on MelonLoader 0.7+")
            : null,
      ),
    ),
  );
  if (!state.overview.rulesets.length) {
    $("#new-game-rulesets").append(
      h("p", { class: "muted hint" }, "No shared rules are installed; files go in as they are."),
    );
  }
  $("#game-dialog").showModal();
}

gameForm.elements.name.addEventListener("input", (e) => {
  if (!idEdited) gameForm.elements.id.value = slug(e.target.value);
});
gameForm.elements.id.addEventListener("input", () => (idEdited = true));
gameForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#game-dialog").close();
});

gameForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(gameForm);
  const game = {
    id: form.get("id"),
    name: form.get("name"),
    install: form.get("install") || null,
    thunderstore: form.get("thunderstore") || null,
    nexus: form.get("nexus") || null,
    rulesets: form.getAll("rulesets"),
    link: form.get("link"),
  };
  const submit = gameForm.querySelector("[type=submit]");
  const existed = !!gameInfo(game.id);
  submit.disabled = true;
  let error = null;
  try {
    await invoke("create_game", { game });
  } catch (err) {
    error = String(err);
  } finally {
    submit.disabled = false;
  }
  state.overview = await run("overview");
  // The game can be saved even if its install folder couldn't be set, so
  // carry on to it with the error showing rather than leaving the dialog up.
  if (error && (existed || !gameInfo(game.id))) {
    $("#new-game-error").textContent = error;
    $("#new-game-error").hidden = false;
    $("#new-game-error").scrollIntoView({ block: "nearest" });
    return;
  }
  $("#game-dialog").close();
  await go(() => openGame(game.id));
  status(error ?? `Added ${game.name}`, !!error);
});

// "Browse…" next to a folder field fills in the input it names, in the same form.
document.addEventListener("click", (e) => {
  const button = e.target.closest("button[data-browse]");
  if (button) pickFolder(button.form.elements[button.dataset.browse]);
});

// Game settings. General is sent back to set_game_settings as a
// GameInstall (see patchwork-core): the folder, how files are linked, the
// deployer's settings, how it's started, and where the game's other
// folders are. Appearance is editgame.js's and theme.js's. Each change is
// saved as it's made, and can be undone (see autosave.js).

const settingsDialog = $("#game-settings-dialog");
const settingsForm = $("#game-settings-form");
const saving = autosave(settingsDialog);
/** The deployer's settings as last shown. */
let deployerSettings = [];
/** The game's other roots as last shown. */
let rootSettings = [];

/** What can run the game as last shown, if anything can be picked. */
let runnerChoices = null;
/** The game's settings as last shown, for what the form doesn't show. */
let shownInstall = {};
/** The game the dialog is about, which needn't be the open one. */
let settingsGame = null;
/** General as last saved, as sent to set_game_settings (null for a game
 *  that isn't set up), to tell whether it's changed. */
let savedGeneral = null;
/** Whether anything's been saved since the dialog opened, so the window
 *  catches up once it's closed. */
let gameChanged = false;

/**
 * The runner's settings under names of their own, since runners can share keys
 * with each other and with the rest of the form.
 */
function runnerSetting(runner, setting) {
  return { ...setting, key: `runner-${runner.id}-${setting.key}` };
}

/** Shows the settings and any problem of the runner the form says will be used. */
function showRunner() {
  const picked = settingsForm.elements.runner.value;
  const id = picked || runnerChoices?.automatic;
  for (const group of $("#game-settings-runner-fields").children) {
    group.hidden = group.dataset.runner !== id;
  }
  const runner = runnerChoices?.runners.find((r) => r.id === id);
  $("#game-settings-runner-note").textContent = picked
    ? (runner?.unavailable ?? "")
    : runner
      ? "Patchwork uses the first runner that can start the game."
      : "Nothing can start the game right now.";
}

function renderLaunching(settings, install) {
  const options = settings.launchOptions;
  runnerChoices = settings.runners;
  $("#game-settings-launching").hidden = !runnerChoices;
  if (!runnerChoices) return;

  const launch = settingsForm.elements.launch;
  $("#game-settings-launch").hidden = options.length < 2;
  launch.replaceChildren(
    h("option", { value: "" }, "The game's default"),
    ...options.map((o) =>
      h("option", { value: o.id, disabled: !!o.unavailable, title: o.unavailable }, o.name),
    ),
  );
  launch.value = install.launch ?? "";

  const name = (id) => runnerChoices.runners.find((r) => r.id === id)?.name ?? id;
  const automatic = runnerChoices.automatic;
  const runner = settingsForm.elements.runner;
  runner.replaceChildren(
    h("option", { value: "" }, automatic ? `Automatic (${name(automatic)})` : "Automatic"),
    ...runnerChoices.runners.map((r) =>
      h("option", { value: r.id, title: r.unavailable }, r.unavailable ? `${r.name} (unavailable)` : r.name),
    ),
  );
  runner.value = install.runner ?? "";
  $("#game-settings-runner-fields").replaceChildren(
    ...runnerChoices.runners
      .filter((r) => r.settings.length)
      .map((r) =>
        h(
          "div",
          { class: "stack", "data-runner": r.id },
          ...r.settings.map((s) => settingField(runnerSetting(r, s), { idPrefix: "game-setting" })),
        ),
      ),
  );
  showRunner();
}

settingsForm.elements.runner.addEventListener("change", showRunner);

const rootOrigins = { set: "", definition: "", prefix: " in the Wine prefix" };

function rootField(root, set) {
  const id = `game-settings-root-${root.id}`;
  const name = `root-${root.id}`;
  const found = root.path ? `Found${rootOrigins[root.origin] ?? ""}: ${root.path}` : root.problem;
  return h(
    "div",
    { class: "field" },
    h("label", { for: id }, root.id, " ", h("small", { class: "muted" }, `for ${root.allow.join(", ")}`)),
    h(
      "div",
      { class: "row" },
      h("input", { id, name, value: set ?? "", placeholder: root.path ?? "Not found", autocomplete: "off" }),
      h("button", { type: "button", "data-browse": name }, "Browse…"),
      h("button", { type: "button", "data-open-folder": root.id, title: `Open the ${root.id} folder` }, "Open"),
    ),
    h("small", { class: "muted" }, root.origin === "set" ? "" : found),
  );
}

$("#game-settings").addEventListener("click", () => openGameSettings(state.game, "general"));

// "Open" next to a folder opens it as saved, wherever patchwork finds it.
settingsForm.addEventListener("click", (e) => {
  const button = e.target.closest("button[data-open-folder]");
  if (!button) return;
  invoke("open_game_folder", { game: settingsGame, folder: button.dataset.openFolder }).catch((err) =>
    status(String(err), true),
  );
});

/** Fills in General for game `id`, from what's saved. */
async function fillGeneral(id) {
  const game = gameInfo(id);
  const [settings, env] = await Promise.all([
    invoke("game_settings", { game: game.id }),
    invoke("game_env", { game: game.id }),
  ]);
  settingsGame = game.id;
  // what's saved goes in as each box's defaultValue (see autosave.js)
  settingsForm.elements.path.defaultValue = settings.install?.path ?? "";
  settingsForm.elements.path.value = settingsForm.elements.path.defaultValue;
  const link = settingsForm.elements.link;
  const defaultLabel = [...link.options].find((o) => o.value === settings.defaultLink)?.text;
  link.options[0].text = `Default (${defaultLabel ?? settings.defaultLink})`;
  link.value = settings.install?.link ?? "";
  deployerSettings = settings.deployer;
  $("#game-settings-deployer").replaceChildren(
    ...deployerSettings.map((s) => settingField(s, { idPrefix: "game-setting" })),
  );
  rootSettings = settings.roots;
  const install = settings.install ?? {};
  $("#game-settings-roots").hidden = rootSettings.length === 0;
  shownInstall = install;
  renderLaunching(settings, install);
  $("#game-settings-prefix").hidden = !settings.usesPrefix;
  settingsForm.elements.prefix.defaultValue = install.prefix ?? "";
  settingsForm.elements.prefix.value = settingsForm.elements.prefix.defaultValue;
  settingsForm.elements.prefix.placeholder = settings.prefix ?? "Not found";
  $("#game-settings-root-list").replaceChildren(
    ...rootSettings.map((r) => rootField(r, install.roots?.[r.id])),
  );
  $("#game-settings-title").textContent = `${game.name} settings`;
  $("#game-settings-env").textContent = env?.summary ? `Installed: ${env.summary}` : "";
  $("#game-settings-env").hidden = !env?.summary;
  savedGeneral = readGeneral();
}

/** Opens game `id`'s settings on `tab`: "general" or "appearance". */
async function openGameSettings(id, tab) {
  try {
    await Promise.all([fillGeneral(id), renderGameAppearance(id), renderGameTheme(id)]);
  } catch (err) {
    status(String(err), true);
    return;
  }
  gameChanged = false;
  saving.open({ key: id, tab });
}

$("#game-settings-app").addEventListener("click", () => {
  settingsDialog.close();
  openSettings("accounts").catch(() => {});
});

/**
 * General as the form has it, as set_game_settings takes it, or null for a
 * game that isn't set up and still has no folder. Throws if the folder's
 * been emptied.
 */
function readGeneral() {
  const deployer = {};
  for (const setting of deployerSettings) {
    const value = readSetting(settingsForm, setting);
    if (value != null) deployer[setting.key] = value;
  }
  const roots = {};
  for (const root of rootSettings) {
    const value = settingsForm.elements[`root-${root.id}`].value.trim();
    if (value) roots[root.id] = value;
  }
  // what isn't shown stays as it is, like settings for a runner that's gone
  const runners = structuredClone(shownInstall.runners ?? {});
  for (const runner of runnerChoices?.runners ?? []) {
    const set = {};
    for (const setting of runner.settings) {
      const value = readSetting(settingsForm, runnerSetting(runner, setting));
      if (value != null) set[setting.key] = value;
    }
    if (Object.keys(set).length) runners[runner.id] = set;
    else delete runners[runner.id];
  }
  const path = settingsForm.elements.path.value.trim();
  // a game that isn't set up can still have its appearance changed
  if (!path && !shownInstall.path) return null;
  if (!path) throw new Error("The install folder can't be empty.");
  return {
    path,
    link: settingsForm.elements.link.value || null,
    deployer,
    roots,
    prefix: settingsForm.elements.prefix.value.trim() || null,
    launch: runnerChoices ? settingsForm.elements.launch.value || null : (shownInstall.launch ?? null),
    runner: runnerChoices ? settingsForm.elements.runner.value || null : (shownInstall.runner ?? null),
    runners,
  };
}

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/** Saves game `id`'s General as `settings`, for undo and redo, and shows it
 *  if the dialog's on that game. */
async function setGeneral(id, settings) {
  await invoke("set_game_settings", { game: id, settings });
  gameChanged = true;
  if (settingsGame === id && settingsDialog.open) await saving.refill(() => fillGeneral(id));
}

/** Saves game `id`'s Appearance edits as `changes`, for undo and redo. */
async function setAppearance(id, changes) {
  await saving.refill(() => setGameAppearance(id, changes));
  gameChanged = true;
  // its color is in the theme
  if (state.game === id) await applyTheme(id);
}

// Each field saves as it's changed: General's together, as one GameInstall,
// and Appearance's together, as the game's edits. The Theme saves itself
// (see theme.js).
saving.watch(settingsForm, (field) => {
  const id = settingsGame;
  if (field.closest("#game-settings-appearance")) {
    return saving.change(
      saving.labelOf(field),
      async () => {
        const saved = await saveGameAppearance();
        if (!saved) return null;
        gameChanged = true;
        if (state.game === id) await applyTheme(id);
        return { undo: () => setAppearance(id, saved.before), redo: () => setAppearance(id, saved.after) };
      },
      { tab: "appearance" },
    );
  }
  return saving.change(
    saving.labelOf(field),
    async () => {
      const settings = readGeneral();
      const before = savedGeneral;
      if (!settings || same(settings, before)) return null;
      await invoke("set_game_settings", { game: id, settings });
      savedGeneral = settings;
      gameChanged = true;
      // a game just set up has nothing to go back to
      if (!before) return {};
      return { undo: () => setGeneral(id, before), redo: () => setGeneral(id, settings) };
    },
    { tab: "general" },
  );
});

// Once it's closed (and anything typed is saved), the window catches up:
// the game's name in the picker and sidebar, its folder, and so on.
settingsDialog.addEventListener("close", () =>
  saving.queue(async () => {
    if (!gameChanged) return;
    gameChanged = false;
    await refresh();
  }),
);
