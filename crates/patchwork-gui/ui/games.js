// The game picker, adding a game, and a game's settings.

import { gameInfo, invoke, profilesOf, run, state, status } from "./state.js";
import { $, h, initials, pickFolder } from "./dom.js";
import { refresh, renderProfile, renderSidebar, switchProfile } from "./profile.js";
import { selectTab } from "./browse.js";
import { go } from "./nav.js";
import { openSettings, readSetting, settingField } from "./settings.js";

function showScreen(screen) {
  document.body.dataset.screen = screen;
  $("#games-screen").hidden = screen !== "games";
  $("#sidebar").hidden = screen !== "game";
  $("#main").hidden = screen !== "game";
}

export async function showGames() {
  state.game = null;
  state.profile = null;
  showScreen("games");
  await applyTheme(null);
  renderGames();
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
 * Shows a game's profiles, opening `profile` if given, or else the current
 * profile if it's for this game, or else its first one.
 */
export async function openGame(id, profile = null) {
  state.game = id;
  state.profile = null;
  showScreen("game");
  selectTab("mods");
  await applyTheme(id);
  renderSidebar();
  const current = state.overview.current;
  const ps = profilesOf(id);
  const pick = ps.find((p) => p.id === profile) ?? ps.find((p) => p.id === current) ?? ps[0];
  if (pick) await switchProfile(pick.id);
  else renderProfile();
}

$("#back").addEventListener("click", () => go(showGames));

async function applyTheme(game) {
  document.body.dataset.game = game ?? "";
  const theme = await invoke("theme", { game });
  $("#theme-builtin").textContent = theme.builtin;
  $("#theme-user").textContent = theme.user ?? "";
  $("#theme-game").textContent = theme.game ?? "";
}

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
// patchwork-core): the folder, how files are linked, and the deployer's
// settings.

const settingsForm = $("#game-settings-form");
/** The deployer's settings as last shown. */
let deployerSettings = [];

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
  openSettings().catch(() => {});
});

settingsForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const deployer = {};
  for (const setting of deployerSettings) {
    const value = readSetting(settingsForm, setting);
    if (value != null) deployer[setting.key] = value;
  }
  const settings = {
    path: settingsForm.elements.path.value,
    link: settingsForm.elements.link.value || null,
    deployer,
  };
  const submit = settingsForm.querySelector("[type=submit]");
  submit.disabled = true;
  try {
    await invoke("set_game_settings", { game: state.game, settings });
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
