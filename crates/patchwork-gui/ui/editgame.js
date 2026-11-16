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

/** Saves the tab's fields, if any were changed. Says whether they were. */
export async function saveGameAppearance() {
  if (!shown) return false;
  const changes = changesOf(shown.views);
  const before = Object.fromEntries(
    shown.views.map((v) => [v.field.id, v.field.edited ?? null]),
  );
  if (JSON.stringify(changes) === JSON.stringify(before)) return false;
  await invoke("set_game_edits", { game: shown.id, changes });
  return true;
}
