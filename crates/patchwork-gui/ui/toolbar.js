// The Mods tab's toolbar and search box: filtering by tag, searching and
// sorting.

import { run, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { openProfile } from "./profile.js";
import { renderMods } from "./mods.js";
import { asLink } from "./addlink.js";

// Filtering. Mods with any of the profile's hiddenTags go in a collapsed
// section under the rest, which keeps things like libraries out of the way
// without changing the load order. Tags come from each mod's summary, so a
// mod whose summary hasn't been fetched yet stays in the main list until it
// has.

const filterMenu = $("#filter-menu");

const lower = (tags) => tags.map((t) => t.toLowerCase());
export const modTags = (m) => m.summary?.categories?.map((c) => c.name) ?? [];

export function isHidden(m) {
  const hidden = lower(state.profile.hiddenTags);
  return hidden.length > 0 && lower(modTags(m)).some((t) => hidden.includes(t));
}

/**
 * Lists every tag the profile's mods have, with how many have it, plus any
 * hidden tag none of them have any more so it can still be unticked.
 */
function renderFilterMenu() {
  const p = state.profile;
  const counts = new Map();
  for (const m of p.mods) {
    // the same tag can be spelled differently by different sources
    for (const tag of new Set(lower(modTags(m)))) {
      const entry = counts.get(tag) ?? {
        name: modTags(m).find((t) => t.toLowerCase() === tag),
        n: 0,
      };
      entry.n++;
      counts.set(tag, entry);
    }
  }
  for (const tag of p.hiddenTags) {
    if (!counts.has(tag.toLowerCase())) counts.set(tag.toLowerCase(), { name: tag, n: 0 });
  }
  const tags = [...counts.values()].sort((a, b) => a.name.localeCompare(b.name));
  const hidden = lower(p.hiddenTags);
  const items = filterMenu.querySelector(".menu-items");
  if (!tags.length) {
    items.replaceChildren(
      h("p", { class: "muted menu-note" }, "No tags yet. They show up once the mods' details have loaded."),
    );
    return;
  }
  items.replaceChildren(
    h("p", { class: "muted menu-note" }, "Hide mods tagged:"),
    ...tags.map((tag) =>
      h(
        "label",
        {},
        h("input", {
          type: "checkbox",
          checked: hidden.includes(tag.name.toLowerCase()),
          onchange: (e) => setHidden(tag.name, e.target.checked).catch(() => {}),
        }),
        h("span", {}, tag.name),
        h("span", { class: "muted" }, String(tag.n)),
      ),
    ),
  );
}

async function setHidden(tag, hide) {
  const p = state.profile;
  const tags = p.hiddenTags.filter((t) => t.toLowerCase() !== tag.toLowerCase());
  if (hide) tags.push(tag);
  await run("set_hidden_tags", { id: p.id, tags });
  if (state.profile !== p) return;
  p.hiddenTags = tags;
  renderMods(p.mods);
}

filterMenu.addEventListener("toggle", () => {
  if (filterMenu.open && state.profile) renderFilterMenu();
});

// The search box is also where links are pasted to add a mod (see
// addlink.js); while it holds one, the mods aren't narrowed down.
export const modSearch = $("#mod-search");
export const modQuery = () => (asLink(modSearch.value) ? "" : modSearch.value.trim());

/** Whether the box held a link last time, when the mods shown were all of them. */
let wasLink = false;

modSearch.addEventListener("input", () => {
  if (!state.profile) return;
  const isLink = !!asLink(modSearch.value);
  // editing a link doesn't change which mods are shown
  if (isLink && wasLink) return;
  wasLink = isLink;
  renderMods(state.profile.mods);
});

modSearch.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && modSearch.value) {
    e.stopPropagation();
    modSearch.value = "";
    modSearch.dispatchEvent(new Event("input"));
  }
});

// Ctrl+F or / goes to the search box of the game picker or whichever tab is
// open. / only does while nothing else is being typed in, so it can still be
// typed itself.
document.addEventListener("keydown", (e) => {
  const find = (e.ctrlKey || e.metaKey) && !e.altKey && e.key.toLowerCase() === "f";
  const slash = e.key === "/" && !e.ctrlKey && !e.metaKey && !e.altKey;
  if (!find && !slash) return;
  if (slash && e.target.closest?.("input, textarea, select, [contenteditable]")) return;
  if (document.querySelector("dialog[open]")) return;
  const box = !$("#games-screen").hidden
    ? $("#game-search")
    : $("#main").hidden
      ? null
      : !$("#tab-mods").hidden
        ? !$("#mods-link").hidden && modSearch
        : !$("#tab-browse").hidden
          ? $("#search").hidden
            ? $("#browse-link .link-url")
            : $("#search").elements.text
          : null;
  if (!box) return;
  e.preventDefault();
  box.focus();
  box.select();
});

// On the Mods tab, typing while no other field has focus types into the
// search box, as nothing else there takes text. Space is left alone (it
// presses buttons) until the box holds something, and keys another handler
// already took (e.g. digits on a priority chip) are left with it.
document.addEventListener("keydown", (e) => {
  if (e.defaultPrevented || e.isComposing) return;
  if (e.key.length !== 1 || e.ctrlKey || e.metaKey || e.altKey) return;
  if (e.target.closest?.('input, textarea, select, [contenteditable], [role="menu"]')) return;
  if (document.querySelector("dialog[open]") || !$("#profile-menu").hidden) return;
  if (!$("#games-screen").hidden || $("#main").hidden || $("#tab-mods").hidden) return;
  if ($("#mods-link").hidden || !state.profile) return;
  if (e.key === " " && !modSearch.value) return;
  e.preventDefault();
  modSearch.focus();
  const end = modSearch.value.length;
  modSearch.setRangeText(e.key, end, end, "end");
  modSearch.dispatchEvent(new Event("input", { bubbles: true }));
});

// Sorting. It rearranges the mods once rather than keeping them sorted, so
// they can be moved around by hand afterwards. Since that throws away the
// old order, the status bar offers to put it back.

const sortMenu = $("#sort-menu");

async function sortMods(keys, label) {
  const id = state.profile.id;
  sortMenu.open = false;
  sortMenu.setAttribute("aria-busy", "true");
  status(`Sorting by ${label}…`);
  try {
    const before = await run("sort_mods", { id, keys });
    await openProfile(id);
    if (!before) {
      status(`Already sorted by ${label}`);
      return;
    }
    status(`Sorted by ${label}`);
    const undo = $("#undo");
    undo.hidden = false;
    undo.onclick = async () => {
      undo.hidden = true;
      await run("reorder_mods", { id, names: before });
      status("Put the mods back in their old order");
      if (state.profile?.id === id) await openProfile(id);
    };
  } finally {
    sortMenu.removeAttribute("aria-busy");
  }
}

for (const item of sortMenu.querySelectorAll("[data-keys]")) {
  const label = item.firstChild.textContent.trim().toLowerCase();
  item.addEventListener("click", () =>
    sortMods(item.dataset.keys.split(" "), label).catch(() => {}),
  );
}

// <details> doesn't close by itself when you click elsewhere or press Escape.
for (const menu of document.querySelectorAll("details.menu")) {
  document.addEventListener("click", (e) => {
    if (!menu.contains(e.target)) menu.open = false;
  });
  menu.addEventListener("keydown", (e) => {
    if (e.key === "Escape" && menu.open) {
      menu.open = false;
      menu.querySelector("summary").focus();
    }
  });
}
