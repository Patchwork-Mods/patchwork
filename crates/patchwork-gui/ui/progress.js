// Download progress, from the core's `downloading` and `downloaded` events:
// a bar for each mod being downloaded, on its row or card, and one for all of
// them together in the status bar.
//
// Themes style these through CSS alone (see "Download progress" in
// default.css). A mod's row or card gets the `downloading` class while it
// downloads, `indeterminate` too when the server didn't say how big the file
// is, and `--pw-download-progress` from 0 to 1. #download-progress, the
// overall bar, gets the same variable and classes.

import { listen } from "./state.js";
import { $ } from "./dom.js";

/** Downloads seen since the bar last emptied, by mod name. */
const downloads = new Map();
/** Hides the overall bar a moment after the last download ends. */
let hideTimer = null;
/** Lets the downloads of one lock add up, though they run one after another. */
const LINGER_MS = 1500;

listen("patchwork://event", ({ payload: e }) => {
  if (e.kind === "downloading") {
    clearTimeout(hideTimer);
    downloads.set(e.name, { downloaded: e.downloaded, total: e.total ?? null, done: false });
    showRows(e.name);
    showOverall();
  } else if (e.kind === "downloaded") {
    const d = downloads.get(e.name);
    if (!d) return;
    d.done = true;
    clearRows(e.name);
    showOverall();
    if ([...downloads.values()].every((d) => d.done)) {
      clearTimeout(hideTimer);
      hideTimer = setTimeout(finish, LINGER_MS);
    }
  }
});

/** How far one download has got, from 0 to 1, or null if there's no telling. */
function fraction(d) {
  if (d.done) return 1;
  if (!d.total) return null;
  return Math.min(1, d.downloaded / d.total);
}

/** The rows and cards showing the mod named `name`, in any of its lists. */
const rowsOf = (name) =>
  document.querySelectorAll(
    `#tab-mods :is(.mod-row, .mod-card)[data-name="${CSS.escape(name)}"]`,
  );

function setBar(el, value) {
  el.classList.add("downloading");
  el.classList.toggle("indeterminate", value === null);
  el.style.setProperty("--pw-download-progress", String(value ?? 0));
}

function clearBar(el) {
  el.classList.remove("downloading", "indeterminate");
  el.style.removeProperty("--pw-download-progress");
}

// Rows can be drawn again mid-download, so they're found afresh on every
// event, which come often enough that a new row soon gets its bar back.
function showRows(name) {
  const value = fraction(downloads.get(name));
  for (const row of rowsOf(name)) setBar(row, value);
}

function clearRows(name) {
  for (const row of rowsOf(name)) clearBar(row);
}

const megabytes = (bytes) => `${(bytes / 1e6).toFixed(1)} MB`;

/**
 * Shows all the downloads together: by bytes when every size is known, or
 * else by how many there are, each counting as far as it's got.
 */
function showOverall() {
  const all = [...downloads.values()];
  const bar = $("#download-progress");
  const sized = all.every((d) => d.total);
  const downloaded = all.reduce((sum, d) => sum + (d.done && d.total ? d.total : d.downloaded), 0);
  let value;
  if (sized) {
    const total = all.reduce((sum, d) => sum + d.total, 0);
    value = total ? downloaded / total : 0;
    bar.title = `${megabytes(downloaded)} of ${megabytes(total)}`;
  } else {
    value = all.reduce((sum, d) => sum + (fraction(d) ?? 0), 0) / all.length;
    bar.title = `${megabytes(downloaded)} so far`;
  }
  const finished = all.filter((d) => d.done).length;
  const of = `${finished} of ${all.length} ${all.length === 1 ? "download" : "downloads"} done`;
  bar.title = `${of}, ${bar.title}`;
  bar.setAttribute("aria-valuenow", String(Math.round(value * 100)));
  bar.setAttribute("aria-valuetext", bar.title);
  // nothing's known about any of them yet
  setBar(bar, value === 0 && all.every((d) => fraction(d) === null) ? null : value);
  bar.hidden = false;
}

function finish() {
  downloads.clear();
  const bar = $("#download-progress");
  bar.hidden = true;
  clearBar(bar);
}
