// The game picker, adding a game, and a game's settings.

import { gameInfo, invoke, profilesOf, run, state, status } from "./state.js";
import { $, h, initials, pickFolder } from "./dom.js";
import { refresh, renderProfile, renderSidebar, switchProfile } from "./profile.js";
import { selectTab } from "./browse.js";

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

function renderGames() {
  const { games } = state.overview;
  $("#game-grid").replaceChildren(
    ...games.map((g) => {
      const count = profilesOf(g.id).length;
      const facts = [
        `${count} profile${count === 1 ? "" : "s"}`,
        g.install ? "installed" : "not set up",
      ];
      return h(
        "li",
        {},
        h(
          "button",
          { class: "game-card", "data-game": g.id, onclick: () => openGame(g.id) },
          h("div", { class: "game-art", "aria-hidden": "true" }, initials(g.name)),
          h("span", { class: "game-name" }, g.name),
          h("span", { class: "muted" }, facts.join(" · ")),
          g.origin === "user"
            ? h("span", { class: "badge" }, "Added by you")
            : g.origin.endsWith("+ user")
              ? h("span", { class: "badge" }, "Customized")
              : null,
        ),
      );
    }),
    h(
      "li",
      {},
      h(
        "button",
        { class: "game-card add", onclick: openGameDialog },
        h("div", { class: "game-art", "aria-hidden": "true" }, "+"),
        h("span", { class: "game-name" }, "Add a game"),
        h("span", { class: "muted" }, "Set up one patchwork doesn't know yet"),
      ),
    ),
  );
}

/**
 * Shows a game's profiles, opening the current profile if it's for this
 * game, or else its first one.
 */
async function openGame(id) {
  state.game = id;
  state.profile = null;
  showScreen("game");
  selectTab("mods");
  await applyTheme(id);
  renderSidebar();
  const current = state.overview.current;
  const ps = profilesOf(id);
  const pick = ps.find((p) => p.id === current) ?? ps[0];
  if (pick) await switchProfile(pick.id);
  else renderProfile();
}

$("#back").addEventListener("click", () => showGames());

async function applyTheme(game) {
  document.body.dataset.game = game ?? "";
  const theme = await invoke("theme", { game });
  $("#theme-default").textContent = theme.default;
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
  await openGame(game.id);
  status(error ?? `Added ${game.name}`, !!error);
});

// "Browse…" next to a folder field fills in the input it names, in the same form.
document.addEventListener("click", (e) => {
  const button = e.target.closest("button[data-browse]");
  if (button) pickFolder(button.form.elements[button.dataset.browse]);
});

// Game settings. The form's fields are named after GameInstall's in core, so
// it's filled in from game_settings and sent back to set_game_settings as is.

const settingsForm = $("#game-settings-form");

$("#game-settings").addEventListener("click", async () => {
  const game = gameInfo(state.game);
  const settings = await run("game_settings", { game: game.id });
  settingsForm.reset();
  for (const [key, value] of Object.entries(settings ?? {})) {
    if (settingsForm.elements[key]) settingsForm.elements[key].value = value;
  }
  $("#game-settings-title").textContent = `${game.name} settings`;
  $("#game-settings-error").hidden = true;
  $("#game-settings-dialog").showModal();
});

settingsForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#game-settings-dialog").close();
});

settingsForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const settings = Object.fromEntries(new FormData(settingsForm));
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
