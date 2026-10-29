// Playing. Play starts the game the way last picked from its menu, or the
// game's default way; picking one from the menu starts it that way and
// remembers it for the game. A profile that isn't deployed as it is now is
// deployed first, once the user has seen what that changes.

import { run, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { deployFirst } from "./deploy.js";
import { openSettings } from "./settings.js";

const storageKey = (game) => `launchOption.${game}`;

function remembered(game) {
  try {
    return localStorage.getItem(storageKey(game));
  } catch {
    return null;
  }
}

function remember(game, id) {
  try {
    localStorage.setItem(storageKey(game), id);
  } catch {
    // only a convenience
  }
}

/** The option Play uses for profile `p`: the one last picked, if it can still be used. */
function picked(p) {
  const usable = p.launchOptions.filter((o) => !o.unavailable);
  const last = remembered(p.game);
  return usable.find((o) => o.id === last) ?? usable.find((o) => o.default) ?? usable[0];
}

/** Shows Play, and its menu of launch options, once the game's folder is known. */
export function renderPlay(p) {
  const options = p?.launchOptions ?? [];
  $("#play-split").hidden = !options.length;
  if (!options.length) return;
  const pick = picked(p);
  $("#play").disabled = !pick;
  $("#play").title = pick ? `Play ${pick.name}` : "None of the game's launch options can be used";
  $("#play-options").replaceChildren(
    ...options.map((o) =>
      h(
        "button",
        {
          type: "button",
          role: "menuitemradio",
          "aria-checked": String(o.id === pick?.id),
          disabled: !!o.unavailable,
          title: o.unavailable,
          onclick: () => {
            remember(p.game, o.id);
            play(o.id).catch(() => {});
          },
        },
        h("span", { class: "tick", "aria-hidden": "true" }, o.id === pick?.id ? "✓" : ""),
        o.name,
        o.unavailable
          ? h("span", { class: "muted" }, "unavailable")
          : o.default && h("span", { class: "muted" }, "default"),
      ),
    ),
  );
}

/** Starts the profile's game the way `option` (an id) says. */
async function play(option) {
  $("#play-menu").open = false;
  const p = state.profile;
  if (p.setup.needsAccount) {
    status("Pick a name to play as first");
    await openSettings("playing");
    $("#account-name").focus();
    return;
  }
  $("#play").disabled = true;
  try {
    if (p.deployStatus !== "deployed") {
      if (!(await deployFirst(p.id))) return status("Ready");
    }
    status("Starting the game…");
    status(await run("launch_game", { id: p.id, option }));
  } finally {
    renderPlay(state.profile);
  }
}

$("#play").addEventListener("click", () => {
  const pick = picked(state.profile);
  if (pick) play(pick.id).catch(() => {});
});
