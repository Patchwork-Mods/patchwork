// What's open, and talking to main.rs.

import { $ } from "./dom.js";

export const { invoke } = window.__TAURI__.core;
export const { listen } = window.__TAURI__.event;

export const state = {
  overview: null,
  /** The game being looked at, or null on the game picker. */
  game: null,
  profile: null,
  /** The open tab: "mods", a list's (see listTab in list.js), "config" or a
   * source's id. */
  tab: "mods",
  /** The source whose tab is open, or null on the Mods, lists' and Config tabs. */
  source: null,
  /** What the open list's tab is showing, from list in main.rs. */
  list: null,
  /** Each source's last search, as `{ page, query, results }`. */
  searches: {},
  /**
   * Each source's category filter, as `{ only, hide }` lists of category ids.
   * One that hasn't been set yet starts out hiding what the Mods tab hides.
   */
  filters: {},
};

export function status(text, isError = false) {
  const el = $("#status");
  el.textContent = text;
  el.classList.toggle("error", isError);
  // An undo only makes sense next to the message about what it undoes.
  $("#undo").hidden = true;
}

/** Runs a command, showing its error in the status bar if it fails. */
export async function run(command, args) {
  try {
    return await invoke(command, args);
  } catch (e) {
    status(String(e), true);
    throw e;
  }
}

export const gameInfo = (id) => state.overview.games.find((g) => g.id === id);
export const profilesOf = (game) => state.overview.profiles.filter((p) => p.game === game);
