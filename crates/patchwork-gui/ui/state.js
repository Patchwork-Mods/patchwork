// What's open, and talking to main.rs.

import { $ } from "./dom.js";

export const { invoke } = window.__TAURI__.core;
export const { listen } = window.__TAURI__.event;

export const state = {
  overview: null,
  /** The game being looked at, or null on the game picker. */
  game: null,
  profile: null,
  /** The open tab: "mods", a list's (see listTab in list.js), "config" or
   * "add". */
  tab: "mods",
  /** What the open list's tab is showing, from list in main.rs. */
  list: null,
  /** The Add mods tab's last search, from searchAll() in browse.js. */
  search: null,
  /**
   * Each source's category filter, as `{ only, hide }` lists of category ids.
   * One that hasn't been set yet starts out hiding what the Mods tab hides.
   */
  filters: {},
  /**
   * The deploy or undeploy that's running, as `{ button, cancelled }`:
   * the selector of the button showing Cancel for it ("#deploy" or "#play"),
   * and whether that's been clicked. Null when there isn't one.
   */
  job: null,
};

export function status(text, isError = false) {
  const el = $("#status");
  el.textContent = text;
  el.classList.toggle("error", isError);
  // An undo, or warnings, only make sense next to the message they're about.
  $("#undo").hidden = true;
  $("#status-warnings").hidden = true;
}

/** Runs a command, showing its error in the status bar if it fails. */
export async function run(command, args) {
  try {
    return await invokeResolving(command, args);
  } catch (e) {
    status(errorText(e), true);
    throw e;
  }
}

/** What to show for an error from a command or `invokeResolving`. */
export const errorText = (e) => (e instanceof Error ? e.message : String(e));

/**
 * Runs a command, and when something's in the way of it that has a fix (a
 * blocker; see blockers.rs), asks the user whether to run that fix and
 * tries again if they do. Fails with an Error saying what was in the way if
 * they cancel, or with the command's error as it is otherwise.
 */
export async function invokeResolving(command, args) {
  for (;;) {
    try {
      return await invoke(command, args);
    } catch (e) {
      const blocker = e?.blocker;
      if (!blocker) throw e;
      if (!(await confirmBlocker(blocker))) throw new Error(`Cancelled: ${blocker.title}`);
      status(blocker.fixing);
      await invokeResolving(blocker.command, blocker.args);
    }
  }
}

/** Shows `blocker` in #blocker-dialog; resolves to whether to run its fix. */
function confirmBlocker(blocker) {
  const dialog = $("#blocker-dialog");
  $("#blocker-title").textContent = blocker.title;
  $("#blocker-detail").textContent = blocker.detail;
  $("#blocker-fix").textContent = blocker.fixLabel;
  dialog.returnValue = "";
  dialog.showModal();
  return new Promise((resolve) =>
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "fix"), { once: true }),
  );
}

const SOURCE_NAMES = {
  thunderstore: "Thunderstore",
  nexus: "Nexus Mods",
  github: "GitHub",
  modio: "mod.io",
};

/** What to call a source, like "Nexus Mods" for `nexus`. */
export const sourceName = (s) => SOURCE_NAMES[s] ?? s.charAt(0).toUpperCase() + s.slice(1);

export const gameInfo = (id) => state.overview.games.find((g) => g.id === id);
export const profilesOf = (game) => state.overview.profiles.filter((p) => p.game === game);
