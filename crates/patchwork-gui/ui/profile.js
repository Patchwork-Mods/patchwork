// A game's profiles: the sidebar, and the header and Mods tab of the open one.

import { gameInfo, profilesOf, run, state } from "./state.js";
import { $, h } from "./dom.js";
import { showGames } from "./games.js";
import { renderMods } from "./mods.js";
import { refreshDetails } from "./details.js";
import { modSearch } from "./toolbar.js";
import { renderSourceTabs } from "./browse.js";

/** Reloads everything, staying on whichever screen and profile is open. */
export async function refresh() {
  state.overview = await run("overview");
  for (const warning of state.overview.warnings) console.warn(warning);
  if (state.game && gameInfo(state.game)) {
    renderSidebar();
    if (state.profile) await openProfile(state.profile.id);
    else renderProfile();
  } else {
    await showGames();
  }
}

export function renderSidebar() {
  const game = gameInfo(state.game);
  const current = state.profile?.id;
  $("#game-title").textContent = game.name;
  const ps = profilesOf(game.id);
  $("#profiles").replaceChildren(
    ...ps.map((p) =>
      h(
        "button",
        { "aria-current": String(p.id === current), onclick: () => switchProfile(p.id) },
        h("span", {}, p.name),
        h("span", { class: "muted" }, String(p.mods)),
      ),
    ),
  );
  if (!ps.length) {
    $("#profiles").append(h("p", { class: "muted" }, "No profiles yet."));
  }
}

export async function switchProfile(id) {
  modSearch.value = "";
  await run("use_profile", { id });
  state.overview.current = id;
  await openProfile(id);
}

export async function openProfile(id) {
  showProfile(await run("profile", { id }));
}

/** Shows a profile, as loaded or as returned by a command that changed it. */
export function showProfile(profile) {
  state.profile = profile;
  state.searches = {};
  renderSidebar();
  renderProfile();
  refreshDetails();
}

export function renderProfile() {
  const p = state.profile;
  $("#deploy").disabled = !p?.install;
  $("#update").disabled = !p?.mods.length;
  if (!p) {
    const game = gameInfo(state.game);
    $("#profile-title").textContent = `No ${game.name} profiles yet`;
    $("#profile-meta").textContent = "Create one in the sidebar to start adding mods.";
    $("#mods").replaceChildren();
    $("#hidden-mods").hidden = true;
    $("#mod-problems").replaceChildren();
    $("#mods-empty").hidden = true;
    $("#mods-toolbar").hidden = true;
    $("#game-path").hidden = !!game.install;
    renderSourceTabs([]);
    return;
  }

  $("#profile-title").textContent = p.label;
  const enabled = p.mods.filter((m) => m.enabled).length;
  const deployed =
    p.deployedFiles != null ? `deployed (${p.deployedFiles} files)` : "not deployed";
  $("#profile-meta").textContent =
    `${enabled}/${p.mods.length} mods enabled · ${p.install ?? "game not set up"} · ${deployed}`;
  $("#game-path").hidden = !!p.install;

  renderMods(p.mods);
  $("#mods-toolbar").hidden = !p.mods.length;
  $("#mod-problems").replaceChildren(...p.problems.map((w) => h("li", {}, w)));
  renderSourceTabs(p.searchSources);
}

$("#new-profile").addEventListener("submit", async (e) => {
  e.preventDefault();
  const name = new FormData(e.target).get("name");
  const id = await run("create_profile", { game: state.game, name });
  e.target.reset();
  state.overview = await run("overview");
  await openProfile(id);
});

$("#game-path").addEventListener("submit", async (e) => {
  e.preventDefault();
  const path = new FormData(e.target).get("path");
  await run("set_game_path", { game: state.game, path });
  await refresh();
});
