// A game's profiles: the sidebar, and the header and Mods tab of the open one.

import { gameInfo, invoke, profilesOf, run, state, status } from "./state.js";
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
  state.filters = {};
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
    renderGamePath(!!game.install);
    renderSourceTabs([]);
    return;
  }

  // The game is in the sidebar, its folder is in its settings, and whether
  // it's deployed is on the Deploy button, so this is just the profile.
  $("#profile-title").textContent = p.name;
  const enabled = p.mods.filter((m) => m.enabled).length;
  $("#profile-meta").textContent = `${enabled} of ${p.mods.length} mods enabled`;
  renderGamePath(!!p.install);

  if (!keepMods) renderMods(p.mods);
  $("#mods-toolbar").hidden = !p.mods.length;
  $("#mod-problems").replaceChildren(
    ...p.issues.map((i) => h("li", { class: i.severity, "data-kind": i.kind.type }, i.message)),
  );
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

// Right-clicking a profile in the sidebar: rename, duplicate, export or
// delete it. Delete asks again in a dialog before anything is deleted.

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

$("#rename-profile").addEventListener("click", () => {
  closeProfileMenu();
  const profile = menuProfile;
  askName(`Rename ${profile.label}`, "Rename", profile.name, async (name) => {
    await invoke("rename_profile", { id: profile.id, name });
    state.overview = await run("overview");
    if (state.profile?.id === profile.id) {
      state.profile.name = name;
      state.profile.label = `${profile.game}/${name}`;
      $("#profile-title").textContent = name;
    }
    renderSidebar();
    status(`Renamed ${profile.label} to ${name}`);
  });
});

$("#duplicate-profile").addEventListener("click", () => {
  closeProfileMenu();
  const profile = menuProfile;
  askName(`Duplicate ${profile.label}`, "Duplicate", `${profile.name}-copy`, async (name) => {
    const id = await invoke("duplicate_profile", { id: profile.id, name });
    state.overview = await run("overview");
    await go(() => switchProfile(id));
    status(`Copied ${profile.label} to ${name}`);
  });
});

const nameForm = $("#profile-name-form");
/** What the name dialog does with the name; see askName(). */
let nameAction = null;

/**
 * Asks for a profile name, starting from `value`, and runs `action` with it.
 * If that fails, the dialog stays open with the error so the name can be
 * fixed.
 */
function askName(title, submit, value, action) {
  nameForm.reset();
  nameAction = action;
  $("#profile-name-title").textContent = title;
  $("#profile-name-submit").textContent = submit;
  $("#profile-name-error").hidden = true;
  nameForm.elements.name.value = value;
  $("#profile-name-dialog").showModal();
  nameForm.elements.name.select();
}

nameForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#profile-name-dialog").close();
});

nameForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const submit = $("#profile-name-submit");
  submit.disabled = true;
  try {
    await nameAction(nameForm.elements.name.value.trim());
  } catch (err) {
    $("#profile-name-error").textContent = String(err);
    $("#profile-name-error").hidden = false;
    return;
  } finally {
    submit.disabled = false;
  }
  $("#profile-name-dialog").close();
});

// Exporting. Formats that make files are saved where the user picks; the
// rest (codes, links) are text, shown in the dialog to copy.

const exportForm = $("#export-form");
/** The profile being exported, and the formats its game can be shared in. */
let exporting = null;
let exportFormats = [];

const exportFormat = () => exportFormats.find((f) => f.id === exportForm.elements.format.value);

$("#export-profile").addEventListener("click", async () => {
  closeProfileMenu();
  exporting = menuProfile;
  exportFormats = await run("profile_formats", { game: exporting.game });
  exportForm.reset();
  exportForm.elements.format.replaceChildren(
    ...exportFormats.map((f) => h("option", { value: f.id }, f.id)),
  );
  $("#export-title").textContent = `Export ${exporting.label}`;
  showExportFormat();
  $("#export-dialog").showModal();
});

/** Shows what the chosen format is, and clears any earlier export. */
function showExportFormat() {
  const format = exportFormat();
  $("#export-description").textContent = format?.description ?? "";
  $("#export-submit").textContent = format?.extension ? "Save…" : "Export";
  $("#export-text").hidden = true;
  $("#export-copy").hidden = true;
  $("#export-error").hidden = true;
}

exportForm.elements.format.addEventListener("change", showExportFormat);

exportForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#export-dialog").close();
});

exportForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const format = exportFormat();
  if (!format) return;
  const profile = exporting;
  let path = null;
  if (format.extension) {
    try {
      path = await window.__TAURI__.dialog.save({
        defaultPath: `${profile.name}.${format.extension}`,
        filters: [{ name: format.id, extensions: [format.extension] }],
      });
    } catch (err) {
      status(String(err), true);
      return;
    }
    if (!path) return;
  }
  const submit = $("#export-submit");
  submit.disabled = true;
  $("#export-error").hidden = true;
  // Formats that carry exact versions lock the profile first, which can
  // mean downloading its mods.
  status(`Exporting ${profile.label}…`);
  let text;
  try {
    text = await invoke("export_profile", { id: profile.id, format: format.id, path });
  } catch (err) {
    $("#export-error").textContent = String(err);
    $("#export-error").hidden = false;
    status(`Couldn't export ${profile.label}`, true);
    return;
  } finally {
    submit.disabled = false;
  }
  if (path) {
    $("#export-dialog").close();
    status(`Exported ${profile.label} to ${path}`);
  } else {
    exportForm.elements.text.value = text;
    $("#export-text").hidden = false;
    $("#export-copy").hidden = false;
    exportForm.elements.text.select();
    status(`Exported ${profile.label} as ${format.id}`);
  }
});

$("#export-copy").addEventListener("click", async () => {
  const text = exportForm.elements.text;
  try {
    await navigator.clipboard.writeText(text.value);
    status("Copied to the clipboard");
  } catch {
    // Some webviews don't allow the clipboard API; it's selected, at least.
    text.select();
    document.execCommand("copy");
    status("Copied to the clipboard");
  }
});

// Importing, into the game that's open: from pasted text, or from a file.

const importForm = $("#import-form");
/** The file chosen to import from, which wins over pasted text. */
let importFile = null;

function showImportFile(path) {
  importFile = path;
  $("#import-file-name").textContent = path ?? "or a profile exported to a file";
}

$("#import-profile").addEventListener("click", () => {
  importForm.reset();
  showImportFile(null);
  $("#import-title").textContent = `Import a ${gameInfo(state.game).name} profile`;
  $("#import-error").hidden = true;
  $("#import-dialog").showModal();
});

$("#import-file").addEventListener("click", async () => {
  try {
    const path = await window.__TAURI__.dialog.open({ directory: false, multiple: false });
    if (path) {
      showImportFile(path);
      importForm.elements.text.value = "";
    }
  } catch (err) {
    status(String(err), true);
  }
});

importForm.elements.text.addEventListener("input", () => showImportFile(null));

importForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#import-dialog").close();
});

importForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const text = importForm.elements.text.value.trim();
  const error = $("#import-error");
  if (!importFile && !text) {
    error.textContent = "Paste a code or links, or choose a file.";
    error.hidden = false;
    return;
  }
  const submit = importForm.querySelector("[type=submit]");
  submit.disabled = true;
  error.hidden = true;
  let id;
  try {
    id = await invoke("import_profile", {
      input: importFile ? { path: importFile } : { text },
      game: state.game,
      name: importForm.elements.name.value.trim() || null,
      replace: importForm.elements.replace.checked,
    });
  } catch (err) {
    error.textContent = String(err);
    error.hidden = false;
    return;
  } finally {
    submit.disabled = false;
  }
  $("#import-dialog").close();
  state.overview = await run("overview");
  await go(() => openProfile(id));
  status(`Imported ${state.profile.label}; deploy it to install its mods`);
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

/**
 * Asks where the game is installed if patchwork doesn't know, with the folder
 * filled in if it can be found. Looks once each time a game is opened, not
 * on every render.
 */
function renderGamePath(installed) {
  const form = $("#game-path");
  form.hidden = installed;
  if (installed || form.dataset.game === state.game) return;
  const game = (form.dataset.game = state.game);
  form.elements.path.value = "";
  $("#game-path-found").textContent = "";
  $("#game-path-found-list").replaceChildren();
  invoke("find_installs", { game }).then(
    (found) => {
      // not if the user has moved on or started typing
      if (state.game !== game || !found.length || form.elements.path.value) return;
      form.elements.path.value = found[0].path;
      $("#game-path-found").textContent =
        found.length === 1
          ? `Found in ${found[0].origin}`
          : `Found in ${found.length} places; this one's in ${found[0].origin}`;
      $("#game-path-found-list").replaceChildren(
        ...found.map((f) => h("option", { value: f.path }, f.origin)),
      );
    },
    (e) => console.warn(e),
  );
}

$("#game-path").addEventListener("submit", async (e) => {
  e.preventDefault();
  const path = new FormData(e.target).get("path");
  await run("set_game_path", { game: state.game, path });
  await refresh();
});
