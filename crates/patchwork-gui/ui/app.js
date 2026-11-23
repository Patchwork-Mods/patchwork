// Patchwork's UI. Plain JS modules with no build step: everything goes
// through invoke() to the commands in main.rs.
//
// Mod names, summaries and so on come from remote sources, so they're only
// ever put in the page as text (see h() in dom.js), never as HTML. The one
// exception is changelogs, which main.rs turns from markdown into HTML with
// anything unsafe taken out (see changelog.rs).

import { modsView, setModsView } from "./mods.js";
import { refresh } from "./profile.js";
import { showMissed } from "./daemon.js";
import { invoke } from "./state.js";
import { detectGames } from "./games.js";
// these only wire up their parts of the page
import "./browse.js";
import "./list.js";
import "./config.js";
import "./toolbar.js";
import "./deploy.js";
import "./history.js";
import "./play.js";
import "./downloads.js";
import "./progress.js";
import "./selfupdate.js";
import "./cli.js";
import "./settings.js";
import "./sync.js";
import "./setup.js";
import "./capture.js";
import "./addlink.js";
import "./sourcesearch.js";
import "./wheel.js";
import "./editmod.js";

// Whether the last thing pressed was the pointer rather than a key, which
// default.css uses to show focus rings only to people moving by keyboard.
// Shortcuts (with Ctrl, Alt or Meta held) don't count as moving by keyboard.
const root = document.documentElement;
addEventListener("pointerdown", () => root.setAttribute("data-pointer", ""), true);
addEventListener(
  "keydown",
  (e) => {
    if (e.ctrlKey || e.altKey || e.metaKey) return;
    if (["Shift", "Control", "Alt", "Meta", "CapsLock"].includes(e.key)) return;
    root.removeAttribute("data-pointer");
  },
  true,
);

// Clicking outside a dialog cancels it, as its Cancel (or Close) button
// would, or as Escape does if it has none. A dialog that's busy, with its
// Cancel disabled or hidden or turning Escape away, stays open. Only a click that
// starts and ends outside counts, so selecting text in a dialog and letting
// go outside it doesn't close it.
const outside = (dialog, e) => {
  if (e.target !== dialog) return false;
  const r = dialog.getBoundingClientRect();
  return e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom;
};
let pressedOutside = null;
document.addEventListener("pointerdown", (e) => {
  pressedOutside = e.target instanceof HTMLDialogElement && e.target.open && outside(e.target, e) ? e.target : null;
});
document.addEventListener("click", (e) => {
  const dialog = pressedOutside;
  pressedOutside = null;
  if (!dialog || !outside(dialog, e)) return;
  const cancel = [...dialog.querySelectorAll("button[value=cancel]")].find((b) => b.closest("dialog") === dialog);
  if (cancel) {
    if (!cancel.disabled && !cancel.hidden) cancel.click();
  } else if (dialog.dispatchEvent(new Event("cancel", { cancelable: true }))) {
    dialog.close();
  }
});

// Whether the window is in the background (not focused, or hidden), so
// themes can stop anything that moves, which otherwise keeps the GPU busy
// redrawing a window nobody is looking at.
const syncInactive = () =>
  root.toggleAttribute("data-inactive", document.hidden || !document.hasFocus());
addEventListener("focus", syncInactive);
addEventListener("blur", syncInactive);
document.addEventListener("visibilitychange", syncInactive);
syncInactive();

setModsView(modsView);

// The window starts hidden (see tauri.conf.json) and is shown once the
// first screen has its theme and content and has been drawn, rather than
// blank and unstyled while that loads. Shown even if loading failed, so the
// error can be seen. A hidden window may not draw frames at all, hence the
// timer as well as waiting for the next frame.
const drawn = () =>
  new Promise((done) => {
    requestAnimationFrame(() => requestAnimationFrame(done));
    setTimeout(done, 50);
  });
const shown = refresh()
  .catch(() => {})
  .then(drawn)
  .then(() => invoke("ready"))
  .catch(() => {});

shown.then(showMissed).catch(() => {});
// Games installed or moved since last time; quiet unless it finds any.
shown.then(() => detectGames({ quiet: true })).catch(() => {});
