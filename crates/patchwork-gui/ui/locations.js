// Settings → Folders: where patchwork keeps its own files, how big each
// is, opening them, and moving the data folder, or just the mods' files,
// somewhere else (a bigger drive, say), with what's in it brought over. The backend looks folders
// up by id, so the window can only open ones patchwork knows about (see
// locations.rs).

import { invoke, listen, status } from "./state.js";
import { $, copyString, h } from "./dom.js";

const FOLDERS = {
  config: {
    name: "Settings",
    about: "Your own games, rulesets and themes, and the app's preferences. It stays where it is.",
  },
  data: {
    name: "Data",
    about: "Profiles, mods' files, backups of game files mods replaced, and what's deployed where.",
    movable: "Where should Patchwork's data go?",
    title: "Move Patchwork's data",
    // a folder of its own, in a folder with other things in
    own: "patchwork",
  },
  store: {
    name: "Mod files",
    about: "Every mod's files, kept once however many profiles use them. Usually in Data; it's most of its size, so it can move on its own.",
    movedAbout: "Every mod's files, kept once however many profiles use them. Moved out of Data.",
    movable: "Where should the mods' files go?",
    title: "Move mods' files",
    own: "patchwork-mods",
  },
  profiles: { name: "Profiles", about: "Each profile's mod list and lockfile. In Data." },
  backups: { name: "Backups", about: "The game files mods replaced, put back when they're undeployed. In Data." },
  cache: { name: "Cache", about: "Downloads in progress and what sources last said. Safe to delete." },
};

export function size(bytes) {
  if (bytes < 1e3) return `${bytes} B`;
  if (bytes < 1e6) return `${(bytes / 1e3).toFixed(0)} KB`;
  if (bytes < 1e9) return `${(bytes / 1e6).toFixed(1)} MB`;
  return `${(bytes / 1e9).toFixed(2)} GB`;
}

/** Fills in the Folders tab, with sizes as they're worked out. */
export async function renderFolders() {
  const { folders, home } = await invoke("app_folders");
  const homeNote = $("#app-folders-home");
  homeNote.hidden = !home;
  homeNote.textContent = home ? `PATCHWORK_HOME puts everything in ${home}.` : "";
  $("#app-folders-list").replaceChildren(...folders.map(folderRow));
  for (const f of folders) {
    if (!f.exists) continue;
    invoke("folder_size", { id: f.id }).then(
      (bytes) => {
        const cell = document.querySelector(`#app-folders-list [data-size="${f.id}"]`);
        if (cell) cell.textContent = size(bytes);
      },
      () => {},
    );
  }
}

function folderRow(f) {
  const about = FOLDERS[f.id] ?? { name: f.id, about: "" };
  const open = () => invoke("open_app_folder", { id: f.id }).catch((err) => status(String(err), true));
  return h(
    "li",
    { class: "app-folder stack", "data-folder": f.id },
    h(
      "div",
      { class: "row" },
      h("b", { class: "grow" }, about.name, f.moved ? h("small", { class: "muted" }, " (moved)") : null),
      h("small", { class: "muted", "data-size": f.id }, f.exists ? "…" : "Not made yet"),
    ),
    h("code", { class: "app-folder-path", title: f.path }, f.path),
    h("small", { class: "muted" }, (f.moved && about.movedAbout) || about.about),
    h(
      "div",
      { class: "row end" },
      h("button", { type: "button", onclick: () => copyString(f.path, "Copied the path") }, "Copy path"),
      h("button", { type: "button", onclick: open }, "Open"),
      f.usual
        ? h(
            "button",
            { type: "button", title: `Back to ${f.usual}`, onclick: () => startMove(f.id, f.usual) },
            "Move back",
          )
        : null,
      about.movable ? h("button", { type: "button", onclick: () => startMove(f.id) }, "Move…") : null,
    ),
  );
}

// Moving the data folder, or the mods' files (the store).

const dialog = $("#data-move-dialog");
const sep = (path) => (path.includes("\\") && !path.includes("/") ? "\\" : "/");
const join = (dir, name) => (dir.endsWith(sep(dir)) ? dir + name : dir + sep(dir) + name);
const baseName = (path) => path.split(/[\\/]/).filter(Boolean).pop() ?? "";

/** What's being moved: `data` or `store`. */
let what = "data";
/** Where it usually is, which it can go back to as it is. */
let usual = null;
/** The plan shown, once one's been worked out. */
let plan = null;
let moving = false;

async function pick() {
  const picked = await window.__TAURI__.dialog.open({ directory: true, title: FOLDERS[what].movable });
  return picked || null;
}

/** Moves folder `id` to `to`, or to a folder picked first. */
async function startMove(id, to = null) {
  what = id;
  usual = to;
  const picked = to ?? (await pick());
  if (!picked) return;
  resetDialog();
  dialog.showModal();
  await showPlan(picked);
}

function resetDialog() {
  plan = null;
  moving = false;
  $("#data-move-title").textContent = FOLDERS[what].title;
  $("#data-move-summary").textContent = "Working out how much there is…";
  $("#data-move-to").value = "";
  $("#data-move-how").textContent = "";
  $("#data-move-conflicts").hidden = true;
  $("#data-move-progress").hidden = true;
  $("#data-move-error").hidden = true;
  $("#data-move-close").hidden = false;
  $("#data-move-close").disabled = false;
  $("#data-move-pick").disabled = false;
  $("#data-move-stop").hidden = true;
  $("#data-move-restart").hidden = true;
  $("#data-move-go").hidden = false;
  $("#data-move-go").disabled = true;
}

function showError(err) {
  const p = $("#data-move-error");
  p.textContent = String(err);
  p.hidden = false;
}

/**
 * Works out moving to `picked`. A folder with other things in it gets a
 * folder of its own in it (`patchwork`, or `patchwork-mods` for mods'
 * files), rather than what's moved spread among them, unless it's called
 * that already. Where it usually is is used as it is.
 */
async function showPlan(picked) {
  $("#data-move-error").hidden = true;
  $("#data-move-go").disabled = true;
  const own = FOLDERS[what].own;
  try {
    let next = await invoke("plan_data_move", { what, to: picked });
    if (!next.empty && picked !== usual && baseName(picked).toLowerCase() !== own) {
      next = await invoke("plan_data_move", { what, to: join(picked, own) });
    }
    plan = next;
  } catch (err) {
    plan = null;
    $("#data-move-summary").textContent = "";
    $("#data-move-to").value = picked;
    showError(err);
    return;
  }
  const files = `${plan.files.toLocaleString()} file${plan.files === 1 ? "" : "s"}`;
  $("#data-move-summary").textContent = `Moves ${size(plan.bytes)} (${files}) from ${plan.from}.`;
  $("#data-move-to").value = plan.to;
  $("#data-move-how").textContent = [
    plan.sameDrive === true
      ? "It's on the same drive, so this is quick."
      : plan.sameDrive === false
        ? "It's on another drive, so everything is copied, which can take a while. The old folder is removed once Patchwork has restarted."
        : "If it's on another drive, everything is copied, which can take a while.",
    what === "store" && plan.sameDrive !== true
      ? "Hard links can't reach another drive, so games set to deploy by hard link get copies from then on."
      : "",
    "Deployed mods stay deployed. Syncing and background work stop while it moves, and Patchwork restarts afterwards.",
  ]
    .filter(Boolean)
    .join(" ");
  const conflicts = plan.conflicts;
  $("#data-move-conflicts").hidden = !conflicts.length;
  $("#data-move-conflict-list").replaceChildren(...conflicts.map((c) => h("li", {}, h("code", {}, c))));
  $("#data-move-go").disabled = conflicts.length > 0;
}

$("#data-move-pick").addEventListener("click", async () => {
  const picked = await pick();
  if (picked) await showPlan(picked);
});

$("#data-move-go").addEventListener("click", async () => {
  if (!plan || moving) return;
  moving = true;
  $("#data-move-go").hidden = true;
  $("#data-move-pick").disabled = true;
  $("#data-move-close").hidden = true;
  $("#data-move-stop").hidden = false;
  $("#data-move-progress").hidden = false;
  $("#data-move-bar").value = 0;
  $("#data-move-progress-text").textContent = "Starting…";
  try {
    // restarts the app once it's done, so this only returns if it failed
    await invoke("move_data", { what, to: plan.to });
  } catch (err) {
    moving = false;
    $("#data-move-progress").hidden = true;
    $("#data-move-stop").hidden = true;
    const cancelled = String(err).toLowerCase().includes("cancel");
    showError(
      cancelled
        ? "Stopped. Everything is back where it was."
        : `${err}. Everything is back where it was.`,
    );
    // syncing and the daemon were stopped for it
    $("#data-move-restart").hidden = false;
    $("#data-move-close").hidden = false;
    $("#data-move-close").textContent = "Close";
  }
});

listen("patchwork://data-move", ({ payload: p }) => {
  if (!moving) return;
  $("#data-move-bar").value = p.total ? p.done / p.total : 0;
  $("#data-move-progress-text").textContent = `${size(p.done)} of ${size(p.total)}`;
});

$("#data-move-stop").addEventListener("click", () => {
  $("#data-move-progress-text").textContent = "Stopping and putting things back…";
  invoke("cancel_job").catch(() => {});
});

$("#data-move-restart").addEventListener("click", () => invoke("restart_app"));

// Escape mustn't close it while it moves.
dialog.addEventListener("cancel", (e) => {
  if (moving) e.preventDefault();
});
