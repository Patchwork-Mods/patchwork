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

/** The other mods in `list` by name, or just how many once there are more than two. */
const mods = (list) =>
  list.length <= 2 ? list.map((m) => m.mod).join(" and ") : `${list.length} mods`;

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
  // red only when deploying it changes nothing; losing some files is normal
  mark.classList.toggle("lost", Boolean(c?.lost));
  // one short line each way, e.g. "Overwritten by SMIM Quality Addon (48 files)"
  const line = (verb, list, n) =>
    list.length > 0 && `${verb} ${mods(list)} (${plural(n, "file", "files")})`;
  mark.title = [
    line("Overwritten by", c?.overriddenBy ?? [], loses),
    line("Overwrites", c?.overrides ?? [], wins),
  ]
    .filter(Boolean)
    .join("\n");
  mark.setAttribute("aria-label", mark.title.replace("\n", "; "));
  mark.textContent = !wins ? "⇣" : loses ? "⇅" : "⇡";
}

/** Opens #conflicts-dialog on `name`'s conflicts. */
function showConflicts(name) {
  const c = conflictsOf(name);
  if (!c) return;
  $("#conflicts-title").textContent = `${name}: shared files`;
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
        "Overwritten by",
        `Their copies are used. Move ${name} below them to use its own.`,
        c.overriddenBy,
      ),
      section(
        "Overwrites",
        `${name}'s copies are used, since it's below them.`,
        c.overrides,
      ),
      c.identical > 0 &&
        h(
          "p",
          { class: "muted" },
          `${plural(c.identical, "other shared file is", "other shared files are")} identical in every mod, so it doesn't matter which copy is used.`,
        ),
    ].filter(Boolean),
  );
  $("#conflicts-dialog").showModal();
}
