// What's open, and talking to main.rs.

import { $, h } from "./dom.js";

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

/** Says `message` in the status bar, with a button to read `warnings` if there are any. */
export function showWarnings(message, warnings) {
  if (!warnings.length) return status(message);
  const count = warnings.length;
  status(`${message}, with ${count} warning${count === 1 ? "" : "s"}`);
  $("#warnings-list").replaceChildren(...warnings.map((w) => h("li", {}, w)));
  $("#status-warnings").hidden = false;
}

$("#status-warnings").addEventListener("click", () => $("#warnings-dialog").showModal());

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
 * Runs a command, and when something's in the way of it (a blocker; see
 * blockers.rs), asks the user which of its fixes to apply, applies it and
 * tries again. A blocker with no fixes is one the user sees to themselves,
 * so they're just asked whether to try again. Fails with an Error saying
 * what was in the way if they cancel, with the command's error if a fix
 * didn't clear it, or with the command's error as it is otherwise.
 *
 * A fix can have something done once the command's gone through (like
 * deploying again after a game's folder moved). That's done before this
 * resolves, or, given an `after` list, put there for the caller to do.
 */
export async function invokeResolving(command, args, { after } = {}) {
  const tried = new Set();
  const key = (fix) => JSON.stringify(fix);
  const followUps = [];
  for (;;) {
    let result;
    try {
      result = await invoke(command, args);
    } catch (e) {
      const blocker = e?.blocker;
      if (!blocker) throw e;
      // Something already fixed being in the way again means the fix
      // didn't clear it.
      if (blocker.fixes.some((fix) => tried.has(key(fix)))) throw new Error(e.message);
      const id = args?.id ?? state.profile?.id;
      const choice = await chooseFix(blocker, id);
      if (choice === null) throw new Error(`Cancelled: ${blocker.title}`);
      if (choice === TRY_AGAIN) continue;
      tried.add(key(choice));
      status(choice.doing);
      const retry = await applyFix(choice, id);
      if (retry?.afterwards) {
        followUps.push(retry.afterwards);
        continue;
      }
      // the fix did what was asked itself
      if (retry && typeof retry === "object") return retry.done;
      if (!retry) throw new Error(`Cancelled: ${blocker.title}`);
      continue;
    }
    if (after) after.push(...followUps);
    else for (const followUp of followUps) await followUp();
    return result;
  }
}

/**
 * How the window applies the fixes main.rs can't, by the fix's type (see
 * core's fix.rs): functions taking the fix and the profile id, resolving to
 * whether what it was for can be tried again. The modules with the UI each
 * one needs put them here. One that needs something done once what it was
 * for has gone through resolves to `{ afterwards }`, a function doing it
 * (see `invokeResolving`). One that does what was asked itself (like
 * going back to a deploy with a choice the command didn't have) resolves
 * to `{ done }`, which is what the command gives instead.
 */
export const windowFixes = {};

/**
 * What #blocker-dialog shows under a fix's blocker before it's picked, by
 * the fix's type: functions taking the fix and the profile id, resolving to
 * a node (like the files an undeploy takes out). The modules that know how
 * put them here.
 */
export const fixPreviews = {};

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

/** What `chooseFix` resolves to for a blocker's Try again. */
const TRY_AGAIN = "tryAgain";

/**
 * Shows `blocker` (for profile `id`) in #blocker-dialog, with a button for
 * each of its fixes, the first one primary, or Try again if it has none.
 * Resolves to the fix picked, TRY_AGAIN, or null if cancelled.
 */
function chooseFix(blocker, id) {
  const dialog = $("#blocker-dialog");
  $("#blocker-title").textContent = blocker.title;
  $("#blocker-detail").textContent = blocker.detail;
  const buttons = blocker.fixes.length
    ? blocker.fixes.map((fix, i) =>
        h("button", { value: `fix-${i}`, class: i === 0 ? "primary" : "" }, fix.label),
      )
    : [h("button", { value: TRY_AGAIN, class: "primary" }, "Try again")];
  // the primary button goes last, by Cancel's opposite end
  $("#blocker-actions").replaceChildren(
    h("button", { value: "cancel" }, "Cancel"),
    ...buttons.reverse(),
  );
  showPreviews(blocker, id);
  dialog.returnValue = "";
  dialog.showModal();
  return new Promise((resolve) =>
    dialog.addEventListener(
      "close",
      () => {
        const value = dialog.returnValue;
        if (value === TRY_AGAIN) return resolve(TRY_AGAIN);
        resolve(value.startsWith("fix-") ? blocker.fixes[Number(value.slice(4))] : null);
      },
      { once: true },
    ),
  );
}

/** Counts showPreviews calls, so an old one doesn't fill a newer dialog. */
let previews = 0;

/** Fills #blocker-preview with what `fixPreviews` show for `blocker`'s fixes. */
async function showPreviews(blocker, id) {
  const mine = ++previews;
  const preview = $("#blocker-preview");
  const shown = blocker.fixes.filter((fix) => fixPreviews[fix.type]);
  preview.replaceChildren();
  preview.hidden = !shown.length;
  const nodes = await Promise.all(
    shown.map((fix) =>
      fixPreviews[fix.type](fix, id).catch((e) => h("p", { class: "error" }, errorText(e))),
    ),
  );
  // a dialog for another blocker might be open by now
  if (mine === previews) preview.replaceChildren(...nodes);
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
