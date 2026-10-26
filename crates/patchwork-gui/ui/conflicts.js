// File conflicts: when two mods provide the same file, only the copy from the
// one further down the load order is deployed. Each mod that shares files
// with others gets a mark saying how, which opens #conflicts-dialog listing
// the files. They come from `conflicts` in main.rs, which only counts mods
// that have been locked, and are loaded in the background after the profile
// is, since working them out can mean reading every mod's files.

import { $, h } from "./dom.js";
import { run, state } from "./state.js";

/** Each mod's conflicts by name, for the profile they were loaded for. */
let loaded = { id: null, byName: new Map() };

/** The mark on each mod on screen, by name, to fill once conflicts load. */
const marks = new Map();

/** The latest load, so an older one finishing late is ignored. */
let loads = 0;

/** Loads the open profile's conflicts and marks its mods with them. */
export async function loadConflicts() {
  const id = state.profile?.id;
  if (!id) return;
  const seq = ++loads;
  let list;
  try {
    list = await run("conflicts", { id });
  } catch {
    return;
  }
  if (seq !== loads || state.profile?.id !== id) return;
  loaded = { id, byName: new Map(list.map((c) => [c.name, c])) };
  for (const [name, mark] of marks) {
    if (mark.isConnected) fillMark(mark, name);
    else marks.delete(name);
  }
}

/** The conflicts loaded for `name` in the open profile, if it has any. */
function conflictsOf(name) {
  return loaded.id === state.profile?.id ? loaded.byName.get(name) : undefined;
}

const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;
const fileCount = (list) => list.reduce((n, m) => n + m.files.length, 0);

/**
 * A mod's conflict mark, empty until there are conflicts to show. Only files
 * that differ count: ones that are the same in every mod don't matter.
 */
export function conflictMark(m) {
  const mark = h("button", { type: "button", class: "mod-conflict", hidden: true });
  mark.addEventListener("click", (e) => {
    // it sits on a list row's head, which expands the row when clicked
    e.stopPropagation();
    showConflicts(m.name);
  });
  marks.set(m.name, mark);
  fillMark(mark, m.name);
  return mark;
}

function fillMark(mark, name) {
  const c = conflictsOf(name);
  const wins = fileCount(c?.overrides ?? []);
  const loses = fileCount(c?.overriddenBy ?? []);
  mark.hidden = !wins && !loses;
  if (mark.hidden) return;
  const lost = !wins;
  mark.classList.toggle("lost", lost);
  const parts = [
    wins && `Its copies of ${plural(wins, "file", "files")} replace other mods'`,
    loses && `other mods' copies of ${plural(loses, "file", "files")} replace its own`,
  ].filter(Boolean);
  const text = parts.join("; ") + ". Click to see which.";
  mark.title = text[0].toUpperCase() + text.slice(1);
  mark.setAttribute("aria-label", `Conflicts: ${mark.title}`);
  // overriding more than it's overridden reads as winning, and vice versa
  mark.textContent = lost ? "⇣" : loses ? "⇅" : "⇡";
}

/** Opens #conflicts-dialog on `name`'s conflicts. */
function showConflicts(name) {
  const c = conflictsOf(name);
  if (!c) return;
  $("#conflicts-title").textContent = `${name}'s conflicts`;
  const section = (title, note, list) =>
    list.length > 0 &&
    h(
      "section",
      {},
      h("h3", {}, title),
      h("p", { class: "muted" }, note),
      list.map((other) =>
        h(
          "details",
          { class: "conflict-mod" },
          h(
            "summary",
            {},
            h("span", { class: "mod-name" }, other.mod),
            h("span", { class: "muted" }, plural(other.files.length, "file", "files")),
          ),
          h("ul", { class: "conflict-files" }, other.files.map((f) => h("li", {}, f))),
        ),
      ),
    );
  $("#conflicts-body").replaceChildren(
    ...[
      section(
        "Overrides",
        `These mods come before ${name} in the load order, so its copies of these files are deployed instead of theirs.`,
        c.overrides,
      ),
      section(
        "Overridden by",
        `These mods come after ${name}, so their copies of these files are deployed instead of its own. Move ${name} below them to change that.`,
        c.overriddenBy,
      ),
      c.identical > 0 &&
        h(
          "p",
          { class: "muted" },
          `${plural(c.identical, "more file is", "more files are")} the same in every mod that has ${c.identical === 1 ? "it" : "them"}, so it doesn't matter which is deployed.`,
        ),
    ].filter(Boolean),
  );
  $("#conflicts-dialog").showModal();
}
