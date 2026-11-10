// The game picker, adding a game, and a game's settings.

import { gameInfo, invoke, profilesOf, run, state, status } from "./state.js";
import { $, h, initials, pickFolder } from "./dom.js";
import { refresh, renderProfile, renderSidebar, switchProfile } from "./profile.js";
import { selectTab } from "./browse.js";
import { go } from "./nav.js";
import { openSettings, readSetting, settingField } from "./settings.js";
import { applyTheme, renderGameTheme, saveGameTheme } from "./theme.js";

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
 * Lays out the games matching the search, best first: the ones with an
 * install folder set, then the rest, with adding a game at the very end.
 */
async function renderGames() {
  const render = ++renders;
  const { games } = state.overview;
  const query = gameQuery();
  const shown = query ? await findGames(games, query).catch(() => null) : games;
  if (!shown || render !== renders) return;
  const installed = shown.filter((g) => g.install);
  const others = shown.filter((g) => !g.install);
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
  return h(
    "li",
    {},
    h(
      "button",
      { class: "game-card", "data-game": g.id, onclick: () => go(() => openGame(g.id)) },
      gameArt(g),
      h("span", { class: "game-name" }, g.name),
      h("span", { class: "muted" }, `${count} profile${count === 1 ? "" : "s"}`),
      g.origin === "user"
        ? h("span", { class: "badge" }, "Added by you")
        : g.origin.endsWith("+ user")
          ? h("span", { class: "badge" }, "Customized")
          : null,
    ),
  );
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

// Game settings, sent back to set_game_settings as a GameInstall (see
// patchwork-core): the folder, how files are linked, the deployer's
// settings, how it's started, and where the game's other folders are.

const settingsForm = $("#game-settings-form");
/** The deployer's settings as last shown. */
let deployerSettings = [];
/** The game's other roots as last shown. */
let rootSettings = [];

/** What can run the game as last shown, if anything can be picked. */
let runnerChoices = null;
/** The game's settings as last shown, for what the form doesn't show. */
let shownInstall = {};

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
    ),
    h("small", { class: "muted" }, root.origin === "set" ? "" : found),
  );
}

$("#game-settings").addEventListener("click", async () => {
  const game = gameInfo(state.game);
  const [settings, env] = await Promise.all([
    run("game_settings", { game: game.id }),
    run("game_env", { game: game.id }),
  ]);
  settingsForm.reset();
  settingsForm.elements.path.value = settings.install?.path ?? "";
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
  settingsForm.elements.prefix.value = install.prefix ?? "";
  settingsForm.elements.prefix.placeholder = settings.prefix ?? "Not found";
  $("#game-settings-root-list").replaceChildren(
    ...rootSettings.map((r) => rootField(r, install.roots?.[r.id])),
  );
  await renderGameTheme(game.id);
  $("#game-settings-title").textContent = `${game.name} settings`;
  $("#game-settings-env").textContent = env?.summary ? `Installed: ${env.summary}` : "";
  $("#game-settings-env").hidden = !env?.summary;
  $("#game-settings-error").hidden = true;
  $("#game-settings-dialog").showModal();
});

settingsForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#game-settings-dialog").close();
});

$("#game-settings-app").addEventListener("click", () => {
  $("#game-settings-dialog").close();
  openSettings("accounts").catch(() => {});
});

settingsForm.addEventListener("submit", async (e) => {
  e.preventDefault();
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
  const settings = {
    path: settingsForm.elements.path.value,
    link: settingsForm.elements.link.value || null,
    deployer,
    roots,
    prefix: settingsForm.elements.prefix.value.trim() || null,
    launch: runnerChoices ? settingsForm.elements.launch.value || null : (shownInstall.launch ?? null),
    runner: runnerChoices ? settingsForm.elements.runner.value || null : (shownInstall.runner ?? null),
    runners,
  };
  const submit = settingsForm.querySelector("[type=submit]");
  submit.disabled = true;
  try {
    await invoke("set_game_settings", { game: state.game, settings });
    await saveGameTheme();
  } catch (err) {
    $("#game-settings-error").textContent = String(err);
    $("#game-settings-error").hidden = false;
    return;
  } finally {
    submit.disabled = false;
  }
  $("#game-settings-dialog").close();
  await refresh();
  status(`Saved ${gameInfo(state.game).name} settings`);
});
