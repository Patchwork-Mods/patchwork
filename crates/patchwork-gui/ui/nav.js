// Back and forward through the places the user has been: the game picker, or
// a game's profile and tab. Mouse back/forward buttons and Alt+←/→ move
// through them, like a browser.

import { gameInfo, profilesOf, state } from "./state.js";
import { openGame, showGames } from "./games.js";
import { switchProfile } from "./profile.js";
import { ADD_TAB, selectTab } from "./browse.js";
import { listTab } from "./list.js";

/** Where the user is now. */
const here = () => ({
  game: state.game,
  profile: state.game ? (state.profile?.id ?? null) : null,
  tab: state.game ? (state.tab ?? "mods") : null,
});

const same = (a, b) => a.game === b.game && a.profile === b.profile && a.tab === b.tab;

const backStack = [];
const forwardStack = [];
/** Navigations run one at a time, so quick presses don't trip over each other. */
let queue = Promise.resolve();

function enqueue(task) {
  queue = queue.then(task).catch(() => {});
  return queue;
}

/**
 * Runs a navigation the user asked for, remembering where they were so Back
 * can return there.
 */
export function go(navigate) {
  return enqueue(async () => {
    const from = here();
    try {
      await navigate();
    } finally {
      if (!same(from, here())) {
        backStack.push(from);
        forwardStack.length = 0;
      }
    }
  });
}

/** Whether a remembered place still exists. */
function exists(place) {
  if (!place.game) return true;
  if (!gameInfo(place.game)) return false;
  return !place.profile || profilesOf(place.game).some((p) => p.id === place.profile);
}

async function restore(place) {
  if (!place.game) return showGames();
  if (state.game !== place.game) await openGame(place.game, place.profile);
  else if (place.profile && state.profile?.id !== place.profile) await switchProfile(place.profile);
  const p = state.profile;
  if (
    place.tab === "mods" ||
    p?.lists.some((l) => listTab(l.id) === place.tab) ||
    (p && place.tab === ADD_TAB)
  ) {
    selectTab(place.tab);
  }
}

/** Moves one step through `from`, pushing where the user was onto `to`. */
function step(from, to) {
  return enqueue(async () => {
    // places that have since been deleted are skipped
    let place;
    while ((place = from.pop()) && !exists(place));
    if (!place) return;
    to.push(here());
    await restore(place);
  });
}

export const back = () => step(backStack, forwardStack);
export const forward = () => step(forwardStack, backStack);

/** A dialog or menu is in the way, so moving would happen behind it. */
const blocked = () => !!document.querySelector("dialog[open]");

// Mouse buttons 3 and 4 are back and forward. Their default is cancelled so
// the webview doesn't try to navigate the page itself.
const NAV_BUTTONS = { 3: back, 4: forward };
for (const type of ["mousedown", "auxclick"]) {
  document.addEventListener(type, (e) => {
    if (NAV_BUTTONS[e.button]) e.preventDefault();
  });
}
document.addEventListener("mouseup", (e) => {
  const move = NAV_BUTTONS[e.button];
  if (!move) return;
  e.preventDefault();
  if (!blocked()) move();
});

document.addEventListener("keydown", (e) => {
  if (!e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  const move = e.key === "ArrowLeft" ? back : e.key === "ArrowRight" ? forward : null;
  if (!move) return;
  e.preventDefault();
  if (!blocked()) move();
});
