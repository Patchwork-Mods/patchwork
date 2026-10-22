// A game's profiles: the sidebar, and the header and Mods tab of the open one.

import { gameInfo, profilesOf, run, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { showGames } from "./games.js";
import { renderMods } from "./mods.js";
import { refreshDetails } from "./details.js";
import { modSearch } from "./toolbar.js";
import { renderSourceTabs } from "./browse.js";
import { markUpdates } from "./deploy.js";
import { go } from "./nav.js";

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
        {
          "aria-current": String(p.id === current),
          onclick: () => go(() => switchProfile(p.id)),
          oncontextmenu: (e) => openProfileMenu(e, p),
        },
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

/**
 * Shows a profile, as loaded or as returned by a command that changed it.
 * With `keepMods`, the mods on screen are left as they are, for a change
 * that's already showing on them.
 */
export function showProfile(profile, { keepMods = false } = {}) {
  state.profile = profile;
  state.searches = {};
  renderSidebar();
  renderProfile({ keepMods });
  refreshDetails();
}

/**
 * Enables the deploy button if there's somewhere to deploy to, and makes it
 * stand out when deploying would change anything. Undeploy, in its menu, is
 * only enabled while something's deployed.
 */
export function renderDeploy(p) {
  $("#deploy").disabled = !p?.install;
  $("#deploy").classList.toggle("primary", !!p?.install && p.deployStatus !== "deployed");
  $("#deploy-menu").hidden = !p;
  const files = p?.gameFiles ?? 0;
  $("#undeploy").disabled = !files;
  $("#undeploy-note").textContent = !files
    ? "nothing's deployed"
    : p.deployStatus === "otherProfile"
      ? "another profile's mods"
      : `${files} file${files === 1 ? "" : "s"}`;
}

export function renderProfile({ keepMods = false } = {}) {
  const p = state.profile;
  renderDeploy(p);
  $("#revert").hidden = !p?.hasChanges;
  $("#update").disabled = !p?.mods.length;
  markUpdates();
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

  // The game is in the sidebar, its folder is in its settings, and whether
  // it's deployed is on the Deploy button, so this is just the profile.
  $("#profile-title").textContent = p.name;
  const enabled = p.mods.filter((m) => m.enabled).length;
  $("#profile-meta").textContent = `${enabled} of ${p.mods.length} mods enabled`;
  $("#game-path").hidden = !!p.install;

  if (!keepMods) renderMods(p.mods);
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
  await go(() => openProfile(id));
});

// Right-clicking a profile in the sidebar. Its only item, Delete, asks again
// in a dialog before anything is deleted.

const profileMenu = $("#profile-menu");
/** The profile the menu was opened on. */
let menuProfile = null;

function openProfileMenu(e, profile) {
  e.preventDefault();
  menuProfile = profile;
  profileMenu.hidden = false;
  // Kept inside the window when opened near its right or bottom edge.
  const x = Math.min(e.clientX, innerWidth - profileMenu.offsetWidth - 4);
  const y = Math.min(e.clientY, innerHeight - profileMenu.offsetHeight - 4);
  profileMenu.style.left = `${Math.max(x, 0)}px`;
  profileMenu.style.top = `${Math.max(y, 0)}px`;
  profileMenu.querySelector("button").focus();
}

function closeProfileMenu() {
  profileMenu.hidden = true;
}

document.addEventListener("pointerdown", (e) => {
  if (!profileMenu.contains(e.target)) closeProfileMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeProfileMenu();
});
addEventListener("blur", closeProfileMenu);
addEventListener("resize", closeProfileMenu);
$("#profiles").addEventListener("scroll", closeProfileMenu);

$("#delete-profile").addEventListener("click", async () => {
  closeProfileMenu();
  const profile = menuProfile;
  if (!(await confirmDelete(profile))) return;
  await run("remove_profile", { id: profile.id });
  state.overview = await run("overview");
  status(`Deleted ${profile.label}`);
  if (state.profile?.id !== profile.id) {
    renderSidebar();
    return;
  }
  const next = profilesOf(state.game)[0];
  if (next) {
    await switchProfile(next.id);
  } else {
    state.profile = null;
    renderSidebar();
    renderProfile();
  }
});

/** Asks before deleting a profile; resolves to whether to go ahead. */
function confirmDelete(profile) {
  const dialog = $("#delete-profile-dialog");
  const mods = `${profile.mods} mod${profile.mods === 1 ? "" : "s"}`;
  $("#delete-profile-summary").textContent =
    `Delete ${profile.label} and its ${mods}? This can't be undone.`;
  dialog.returnValue = "";
  dialog.showModal();
  return new Promise((resolve) =>
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "delete"), {
      once: true,
    }),
  );
}

$("#game-path").addEventListener("submit", async (e) => {
  e.preventDefault();
  const path = new FormData(e.target).get("path");
  await run("set_game_path", { game: state.game, path });
  await refresh();
});
