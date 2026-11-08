// Playing. Play starts the game the way last picked from its menu or the game
// settings, or the game's default way; picking one from the menu starts it
// that way and remembers it for the game. A profile that isn't deployed as it
// is now is deployed first, with Play showing Cancel meanwhile.

import { run, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { cancelJob, deployFirst } from "./deploy.js";

/** Whether the game's being started, which takes a few seconds. */
let starting = false;

/** The option Play uses for profile `p`: the game's default, if it can be used. */
function picked(p) {
  const usable = p.launchOptions.filter((o) => !o.unavailable);
  return usable.find((o) => o.default) ?? usable[0];
}

/** Makes `id` what Play starts for profile `p`, here and in the CLI. */
async function remember(p, id) {
  if (p.launchOptions.length < 2 || p.launchOptions.find((o) => o.id === id)?.default) return;
  await run("set_launch_option", { game: p.game, option: id });
  for (const o of p.launchOptions) o.default = o.id === id;
}

/** Shows Play, and its menu of launch options, once the game's folder is known. */
export function renderPlay(p) {
  const options = p?.launchOptions ?? [];
  $("#play-split").hidden = !options.length;
  if (!options.length) return;
  const pick = picked(p);
  const play = $("#play");
  // Play deploying first shows Cancel for it, like Deploy does
  const busy = state.job?.button === "#play";
  play.textContent = busy ? (state.job.cancelled ? "Cancelling…" : "Cancel") : "Play";
  play.disabled = busy ? state.job.cancelled : !pick || !!state.job || starting;
  play.title = busy
    ? "Stop deploying before the next file; the game won't start (Esc)"
    : pick
      ? `Play ${pick.name}`
      : "None of the game's launch options can be used";
  $("#play-menu").inert = !!state.job;
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
            remember(p, o.id)
              .then(() => play(o.id))
              .catch(() => {});
          },
        },
        h("span", { class: "tick", "aria-hidden": "true" }, o.id === pick?.id ? "✓" : ""),
        o.name,
        o.unavailable
          ? h("span", { class: "muted" }, "unavailable")
          : o.default && h("span", { class: "muted" }, "default"),
      ),
    ),
    h(
      "button",
      {
        type: "button",
        role: "menuitem",
        onclick: () => {
          $("#play-menu").open = false;
          $("#game-settings").click();
        },
      },
      h("span", { class: "tick", "aria-hidden": "true" }),
      "Launch settings…",
    ),
  );
}

/**
 * Starts the profile's game the way `option` (an id) says, deploying it
 * first if it isn't deployed as it is now. Cancelling that deploy doesn't
 * start the game.
 */
async function play(option) {
  $("#play-menu").open = false;
  const p = state.profile;
  if (p.deployStatus !== "deployed" && !(await deployFirst(p.id))) return;
  starting = true;
  renderPlay(state.profile);
  try {
    status("Starting the game…");
    status(await run("launch_game", { id: p.id, option }));
  } finally {
    starting = false;
    renderPlay(state.profile);
  }
}

$("#play").addEventListener("click", () => {
  if (state.job) return cancelJob();
  const pick = picked(state.profile);
  if (pick) play(pick.id).catch(() => {});
});
