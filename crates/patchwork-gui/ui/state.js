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
  /** The source being searched on the Add mods tab, or null on other tabs. */
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
    return await invoke(command, args);
  } catch (e) {
    status(String(e), true);
    throw e;
  }
}

export const gameInfo = (id) => state.overview.games.find((g) => g.id === id);
export const profilesOf = (game) => state.overview.profiles.filter((p) => p.game === game);
