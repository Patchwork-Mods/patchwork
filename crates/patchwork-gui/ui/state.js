// What's open, and talking to main.rs.

import { $ } from "./dom.js";

/**
 * A command's error that has a blocker (see blockers.rs). It reads as its
 * message, like every other command's error, wherever it's shown.
 */
class BlockedError extends Error {
  constructor({ message, blocker }) {
    super(message);
    this.blocker = blocker;
  }

  toString() {
    return this.message;
  }
}

/** Runs a command in main.rs. Errors with blockers come as BlockedErrors. */
export async function invoke(command, args) {
  try {
    return await window.__TAURI__.core.invoke(command, args);
  } catch (e) {
    throw e?.blocker ? new BlockedError(e) : e;
  }
}
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
 * blocker; see blockers.rs), asks the user whether to apply that fix and
 * tries again if they do. Fails with an Error saying what was in the way if
 * they cancel, with the command's error if the fix didn't clear it, or with
 * the command's error as it is otherwise.
 */
export async function invokeResolving(command, args) {
  const tried = new Set();
  for (;;) {
    try {
      return await invoke(command, args);
    } catch (e) {
      const blocker = e?.blocker;
      if (!blocker) throw e;
      // The same thing in the way again means the fix didn't clear it.
      const key = JSON.stringify(blocker.fix);
      if (tried.has(key)) throw new Error(e.message);
      tried.add(key);
      if (!(await confirmBlocker(blocker))) throw new Error(`Cancelled: ${blocker.title}`);
      status(blocker.fixing);
      const retry = await applyFix(blocker.fix, args?.id ?? state.profile?.id);
      if (!retry) throw new Error(`Cancelled: ${blocker.title}`);
    }
  }
}

/**
 * How the window applies the fixes main.rs can't, by the fix's type (see
 * core's fix.rs): functions taking the fix and the profile id, resolving to
 * whether what it was for can be tried again. The modules with the UI each
 * one needs put them here.
 */
export const windowFixes = {};

/**
 * Applies `fix` (see core's fix.rs) for profile `id`, in the window or
 * through apply_fix. Resolves to whether to try what it was for again.
 */
export async function applyFix(fix, id) {
  const inWindow = windowFixes[fix.type];
  if (inWindow) return inWindow(fix, id);
  await invokeResolving("apply_fix", { id, fix });
  return true;
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
