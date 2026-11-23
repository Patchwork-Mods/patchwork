// The Appearance tab of a game's settings: how the game is shown (its name,
// picture and color) in place of what its definition says. Changes go in
// the user's own file for the game, <config>/games/<id>.toml, which the
// registry merges over the definition, so anything else written there by
// hand is kept. The tab's Theme is theme.js's.
//
// The fields come from the backend (`game_fields`, see core's
// edits/game.rs); editfields.js builds them.

import { invoke } from "./state.js";
import { $ } from "./dom.js";
import { changesOf, fieldView } from "./editfields.js";

/** The game the tab is about, and its fields' views. */
let shown = null;

/**
 * What --pw-accent comes to without a game's own color, which is what a
 * game without one gets: the color field shows it when it isn't set.
 */
function themeAccent() {
  const layer = $("#theme-accent");
  layer.disabled = true;
  const accent = getComputedStyle(document.documentElement).getPropertyValue("--pw-accent").trim();
  layer.disabled = false;
  return accent;
}

/** Fills in the tab's fields for game `id`. */
export async function renderGameAppearance(id) {
  const info = await invoke("game_fields", { game: id });
  const defaultColor = themeAccent();
  shown = {
    id,
    views: info.fields.map((f) =>
      fieldView(f, "game-appearance", () => {}, {
        noSource: info.added,
        sourceName: "the game's definition",
        defaultColor,
      }),
    ),
  };
  $("#game-appearance-fields").replaceChildren(...shown.views.map((v) => v.el));
}

/** The edits the tab's fields were filled in from, as set_game_edits takes them. */
const savedEdits = () =>
  Object.fromEntries(shown.views.map((v) => [v.field.id, v.field.edited ?? null]));

/**
 * Saves the tab's fields, if any were changed, resolving to the edits as
 * they were and are, `{ before, after }`, or null if nothing was.
 */
export async function saveGameAppearance() {
  if (!shown) return null;
  const after = changesOf(shown.views);
  const before = savedEdits();
  if (JSON.stringify(after) === JSON.stringify(before)) return null;
  await invoke("set_game_edits", { game: shown.id, changes: after });
  for (const v of shown.views) v.field.edited = after[v.field.id];
  return { before, after };
}

/** Saves game `id`'s edits as `changes`, for undo and redo, and shows them
 *  if the tab's on that game. */
export async function setGameAppearance(id, changes) {
  await invoke("set_game_edits", { game: id, changes });
  if (shown?.id === id) await renderGameAppearance(id);
}
