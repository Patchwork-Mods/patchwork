// The Add mods tab: one box that searches every ticked source of the game
// at once as you type, merging what they find, or adds the mod from a link
// pasted into it (see addlink.js). The Mods tab's search box searches the
// same sources (see sourcesearch.js), and "See all" there opens its search
// here.

import { invoke, run, sourceName, state, status } from "./state.js";
import { $, h, ms, nextFrame } from "./dom.js";
import { renderProfile } from "./profile.js";
import { cardMain } from "./mods.js";
import { refreshDetails } from "./details.js";
import { go } from "./nav.js";
import { listOf, listTab, openList } from "./list.js";
import { CONFIG_TAB, openConfig } from "./config.js";
import { asLink } from "./addlink.js";
import { sourcesChanged } from "./sourcesearch.js";

/** The Add mods tab's key in state.tab. */
export const ADD_TAB = "add";

const box = $("#browse-search");
const form = $("#search");

/** The search waiting for the user to stop typing. */
let typing;

/** The sources the open profile's game can be searched on. */
export const sources = () => state.profile?.searchSources ?? [];

/** The orders `source` can sort its results in. */
const sortsOf = (source) => state.profile?.searchSorts?.[source] ?? [];

// Sources can be unticked on the Add mods tab, to leave them out of
// searches; that's remembered for each game, on this computer only.

/** The sources unticked for the open game. */
let unticked = new Set();
/** The game `unticked` is for. */
let untickedGame = null;

const untickedKey = (game) => `pw-unticked-sources:${game}`;

function loadUnticked() {
  const game = state.profile?.game ?? null;
  if (game === untickedGame) return;
  untickedGame = game;
  unticked = new Set();
  try {
    const saved = JSON.parse(localStorage.getItem(untickedKey(game)) ?? "[]");
    if (Array.isArray(saved)) unticked = new Set(saved);
  } catch {
    // nothing saved, or storage is off: every source is ticked
  }
}

function saveUnticked() {
  try {
    localStorage.setItem(untickedKey(untickedGame), JSON.stringify([...unticked]));
  } catch {
    // only lasts until patchwork closes, then
  }
}

/** The sources searches go to: the ticked ones, or all if none are. */
export function ticked() {
  loadUnticked();
  const on = sources().filter((s) => !unticked.has(s));
  return on.length ? on : sources();
}

/** Names `list` of sources, like "Thunderstore, GitHub and mod.io". */
export function sourceNames(list) {
  const names = list.map(sourceName);
  return names.length < 2 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names.at(-1)}`;
}

/**
 * Opens the Mods tab ("mods"), a list's tab (see listTab), the Config tab
 * or the Add mods tab; list tabs all share #tab-list.
 */
export function selectTab(key) {
  const list = listOf(key);
  const config = key === CONFIG_TAB;
  const add = key === ADD_TAB;
  state.tab = key;
  for (const tab of document.querySelectorAll("#tabs [role=tab]")) {
    tab.setAttribute("aria-selected", String(tab.dataset.tab === key));
  }
  $("#tab-mods").hidden = key !== "mods";
  $("#tab-list").hidden = !list;
  $("#tab-config").hidden = !config;
  $("#tab-browse").hidden = !add;
  clearTimeout(typing);
  if (list) openList(list);
  if (config) openConfig();
  if (add) showSearch();
}

/** The text to search for: what's in the box, unless it's a link. */
const boxText = () => (asLink(box.value) ? "" : box.value.trim());

/** Whether `found` is a search of what the tab would search for now. */
const current = (found) =>
  !!found &&
  found.text === boxText() &&
  found.sort === (form.elements.sort.value || null) &&
  found.sources.join() === ticked().join();

/** Shows the Add mods tab's search, searching again if it's out of date. */
function showSearch() {
  const list = sources();
  form.hidden = !list.length;
  box.placeholder = list.length ? "Search mods or paste a link" : "Paste a link to a mod";
  renderSourcePicker();
  renderSorts();
  renderFilterCount();
  if (!list.length) return renderResults(null);
  if (state.search) renderResults(state.search);
  // the tab opens on the sources' home pages, which is a search with no text
  if (!current(state.search) && !asLink(box.value)) searchNow();
}

/** A toggle per source, when there's more than one to search. */
function renderSourcePicker() {
  const list = sources();
  const on = ticked();
  const picker = $("#source-picker");
  picker.hidden = list.length < 2;
  picker.replaceChildren(
    ...list.map((s) =>
      h(
        "button",
        {
          type: "button",
          class: "source-toggle",
          "aria-pressed": String(on.includes(s)),
          title: on.includes(s)
            ? on.length > 1
              ? `Leave ${sourceName(s)} out of searches`
              : `${sourceName(s)} is the only source being searched`
            : `Search ${sourceName(s)} too`,
          onclick: () => toggleSource(s),
        },
        sourceName(s),
      ),
    ),
  );
}

/** Ticks or unticks `source`, and searches again; the last one stays ticked. */
function toggleSource(source) {
  const on = ticked();
  if (on.includes(source)) {
    if (on.length < 2) return;
    unticked.add(source);
  } else {
    unticked.delete(source);
  }
  saveUnticked();
  renderSourcePicker();
  renderSorts();
  renderFilterCount();
  sourcesChanged();
  if (!asLink(box.value)) searchNow();
}

/** Offers only the orders some ticked source can sort by. */
function renderSorts() {
  const on = ticked();
  const select = form.elements.sort;
  for (const option of select.options) {
    option.hidden = !!option.value && !on.some((s) => sortsOf(s).includes(option.value));
  }
  if (select.selectedOptions[0]?.hidden) select.value = "";
}

/**
 * Replaces the tabs, staying on the open one if it's still there: Mods, one
 * for each of the game's ordered lists, Config if the game has config files,
 * then Add mods while there's a profile to add to.
 */
export function renderTabs() {
  const p = state.profile;
  const lists = p?.lists ?? [];
  const config = !!p?.hasConfig;
  const tab = (key, label) =>
    h("button", { role: "tab", "data-tab": key, onclick: () => go(() => selectTab(key)) }, label);
  $("#tabs").replaceChildren(
    tab("mods", "Mods"),
    ...lists.map((l) => tab(listTab(l.id), l.title)),
    ...(config ? [tab(CONFIG_TAB, "Config")] : []),
    ...(p ? [tab(ADD_TAB, "Add mods")] : []),
  );
  const open = listOf(state.tab);
  const still = open
    ? lists.some((l) => l.id === open)
    : state.tab === CONFIG_TAB
      ? config
      : state.tab === ADD_TAB && !!p;
  selectTab(still ? state.tab : "mods");
}

// A search of several sources: `{ text, sort, sources, parts, items }`,
// where `parts` has each source's own search by id, as `{ query, items,
// total, hasMore, sort, taken }` or `{ error }` if it failed, and `items`
// is what's been shown of them, merged. `taken` counts the source's items
// merged so far.

/** Which field of a mod each sort orders by, highest first. */
const SORT_KEYS = {
  downloads: (item) => item.downloads ?? -1,
  rating: (item) => item.rating ?? -1,
  updated: (item) => (item.updated ? Date.parse(item.updated) : -1),
};

/**
 * Takes the items of `found`'s sources that haven't been merged yet,
 * merging them: by the sort's field, when every source sorted by it,
 * otherwise taking turns, so each source's best matches come first.
 */
function merge(found) {
  const parts = found.sources.map((s) => found.parts[s]).filter((part) => part && !part.error);
  const lists = parts.map((part) => {
    const fresh = part.items.slice(part.taken);
    part.taken = part.items.length;
    return fresh;
  });
  const by = SORT_KEYS[found.sort];
  if (by && parts.every((part) => part.sort === found.sort)) {
    // a stable sort keeps each source's own order between ties
    return lists.flat().sort((a, b) => by(b) - by(a));
  }
  const merged = [];
  for (let i = 0; lists.some((l) => i < l.length); i++) {
    for (const l of lists) if (i < l.length) merged.push(l[i]);
  }
  return merged;
}

const hasMore = (found) => found.sources.some((s) => found.parts[s]?.hasMore);

/** How many mods match across every source, if they all say. */
function totalOf(found) {
  const parts = found.sources.map((s) => found.parts[s]).filter((part) => !part.error);
  return parts.every((part) => part.total != null)
    ? parts.reduce((n, part) => n + part.total, 0)
    : null;
}

/** Sources `found` couldn't search, as `[source, error]`. */
export const failures = (found) =>
  found.sources.filter((s) => found.parts[s]?.error).map((s) => [s, found.parts[s].error]);

/** Searches one source, for searchAll(). */
async function searchPart(source, text, sort) {
  try {
    const filter = await filterOf(source);
    const query = {
      game: state.profile.game,
      source,
      text,
      // one that can't sort this way sticks to its own order
      sort: sort && sortsOf(source).includes(sort) ? sort : null,
      categories: filter.only,
      excludedCategories: filter.hide,
      page: 0,
    };
    const page = await invoke("search", { query });
    return {
      query,
      items: page.items,
      total: page.total,
      hasMore: page.has_more,
      sort: page.sort,
      taken: 0,
    };
  } catch (e) {
    return { error: String(e), items: [], hasMore: false, taken: 0 };
  }
}

/**
 * Searches every one of `list` of sources at once for `text`, returning
 * what they found (see above), or null if another profile was opened
 * meanwhile. A source that fails doesn't stop the others.
 */
export async function searchAll(list, text, sort = null) {
  const p = state.profile;
  const results = await Promise.all(list.map((s) => searchPart(s, text, sort)));
  if (state.profile?.id !== p.id) return null;
  const found = { text, sort, sources: list, parts: {}, items: [], loading: false };
  list.forEach((s, i) => (found.parts[s] = results[i]));
  found.items = merge(found);
  return found;
}

/** Opens the Add mods tab showing `found` (from searchAll()), to see more of it. */
export function openSearch(found) {
  box.value = found.text;
  form.elements.sort.value = found.sort ?? "";
  state.search = found;
  // what was in the box before, like a link's files, is forgotten
  box.dispatchEvent(new Event("input"));
  return go(() => selectTab(ADD_TAB));
}

/** Counts the tab's searches, so an older one finishing late is ignored. */
let searchSeq = 0;

/** Starts a new search of what's in the box, replacing the results. */
async function search() {
  const p = state.profile;
  const list = ticked();
  if (!p || !list.length) return;
  const seq = ++searchSeq;
  const sort = form.elements.sort.value || null;
  status(`Searching ${sourceNames(list)}…`);
  const started = performance.now();
  const found = await searchAll(list, boxText(), sort);
  const fetched = performance.now();
  if (!found || seq !== searchSeq) return;
  state.search = found;
  if (state.tab === ADD_TAB) renderResults(found);
  await nextFrame();
  const failed = failures(found);
  if (failed.length === list.length) {
    return status(`Couldn't search ${sourceNames(list)}: ${failed[0][1]}`, true);
  }
  const total = totalOf(found);
  status(
    `${total ?? found.items.length} mods · fetched in ${ms(fetched - started)}, ` +
      `rendered in ${ms(performance.now() - fetched)}`,
  );
}

/** Adds the next page of each source that has more to the bottom. */
async function loadMore() {
  const found = state.search;
  if (!found || found.loading || !hasMore(found)) return;
  found.loading = true;
  status("Loading more…");
  try {
    await Promise.all(
      found.sources.map(async (s) => {
        const part = found.parts[s];
        if (!part?.hasMore) return;
        const query = { ...part.query, page: part.query.page + 1 };
        try {
          const page = await invoke("search", { query });
          // Mods can shift between pages while scrolling, so skip ones already had.
          const had = new Set(part.items.map(key));
          part.items.push(...page.items.filter((item) => !had.has(key(item))));
          part.query = query;
          part.total = page.total;
          part.hasMore = page.has_more;
        } catch {
          // what it found so far stays; it just stops there
          part.hasMore = false;
        }
      }),
    );
  } finally {
    found.loading = false;
  }
  if (state.search !== found) return;
  const items = merge(found);
  found.items.push(...items);
  if (state.tab === ADD_TAB) {
    const added = addedMods();
    const labelled = found.sources.length > 1;
    $("#results").append(...items.map((item) => resultCard(item, added, { labelled })));
    showMore(found);
  }
  status(`${totalOf(found) ?? found.items.length} mods · showing ${found.items.length}`);
}

/** Shows a search's results, or nothing without one. */
function renderResults(found) {
  const errors = $("#search-errors");
  if (!found) {
    $("#results").replaceChildren();
    $("#more").hidden = true;
    errors.hidden = true;
    return;
  }
  const added = addedMods();
  // with several sources, each card says which it's from
  const labelled = found.sources.length > 1;
  $("#results").replaceChildren(...found.items.map((item) => resultCard(item, added, { labelled })));
  if (!found.items.length) {
    $("#results").append(h("p", { class: "empty" }, "No mods found."));
  }
  const failed = failures(found);
  errors.hidden = !failed.length;
  errors.replaceChildren(
    ...failed.flatMap(([s, err], i) => [
      i ? h("br") : null,
      `Couldn't search ${sourceName(s)}: ${err}`,
    ]),
  );
  $("#tab-browse").scrollTop = 0;
  showMore(found);
}

const key = (item) => `${item.source}:${item.id}`;
export const addedMods = () => new Set(state.profile.mods.map(key));

/**
 * Whether one of the files of search result `item` is in `added`, like
 * `nexus:17230/Engine Fixes - SKSE64 Preloader` for `nexus:17230`: ids that
 * pick one of a mod's files have it after a slash.
 */
const fileAdded = (item, added) => [...added].some((k) => k.startsWith(`${key(item)}/`));

/**
 * A search result, with a button to add it unless it's in `added` (see
 * addedMods()). With `labelled`, it says which source it's from, for
 * results from several.
 */
export function resultCard(item, added, { labelled = false } = {}) {
  const isAdded = added.has(key(item));
  const card = h(
    "li",
    { class: "result" },
    cardMain(item),
    labelled && h("span", { class: "result-source" }, sourceName(item.source)),
    h(
      "button",
      {
        class: "add",
        disabled: isAdded,
        title: !isAdded && fileAdded(item, added) ? "Add another of its files" : null,
        onclick: (e) => addResult(item, card, e.target),
      },
      isAdded ? "Added" : "Add",
    ),
  );
  return card;
}

/** Adds mod `modId` of search result `item`, returning whether it was. */
async function addFile(item, modId) {
  const p = state.profile;
  status(`Adding ${item.name}…`);
  const added = await run("add_mod", { id: p.id, source: item.source, modId }).catch(() => null);
  if (!added) return false;
  status(`Added ${added.name}`);
  if (state.profile?.id === p.id) {
    state.profile = added.profile;
    renderProfile();
    refreshDetails();
  }
  return true;
}

/**
 * The Add button of `item`'s card: adds the mod, or when it has several
 * files to choose between (a Nexus mod with a plugin and a preloader, say)
 * lists them in the card, where each can be added in turn.
 */
async function addResult(item, card, button) {
  button.disabled = true;
  status(`Looking up ${item.name}…`);
  const choices = await run("file_choices", {
    id: state.profile.id,
    source: item.source,
    modId: item.id,
  }).catch(() => null);
  if (!choices) {
    button.disabled = false;
    return;
  }
  if (choices.length < 2) {
    if (await addFile(item, choices[0]?.id ?? item.id)) button.textContent = "Added";
    else button.disabled = false;
    return;
  }
  status("Pick the files to add");
  const added = addedMods();
  const files = h(
    "div",
    { class: "result-files stack" },
    h("small", { class: "muted" }, "Which files?"),
    ...choices.map((c) => {
      const isAdded = added.has(`${item.source}:${c.id}`);
      return h(
        "button",
        {
          type: "button",
          disabled: isAdded,
          title: c.description ?? null,
          onclick: async (e) => {
            const b = e.currentTarget;
            b.disabled = true;
            if (await addFile(item, c.id)) b.classList.add("added");
            else b.disabled = false;
          },
        },
        c.name,
      );
    }),
  );
  files.querySelectorAll("button:disabled").forEach((b) => b.classList.add("added"));
  button.replaceWith(files);
  files.querySelector("button:enabled")?.focus();
}

// #more sits under the results while there are more pages, and loads the
// next one once it's within a screen of coming into view.
const moreObserver = new IntersectionObserver(
  (entries) => {
    if (entries.some((e) => e.isIntersecting)) loadMore().catch(() => {});
  },
  { root: $("#tab-browse"), rootMargin: "0px 0px 100% 0px" },
);
function showMore(found) {
  const more = $("#more");
  const any = hasMore(found);
  more.hidden = !any;
  // Observing again re-checks it, in case a page didn't push it out of reach.
  moreObserver.unobserve(more);
  if (any) moreObserver.observe(more);
}

// Filtering by category. Each source does its own filtering, so paging and
// the number of mods found stay right, and has its own categories. A
// source's filter starts out hiding the categories the Mods tab's filter
// hides, matched by name; changing it here only lasts until the profile is
// opened again.

const filterMenu = $("#source-filter");

/** Each source's categories for each game, by "game:source", once asked for. */
const categoryLists = new Map();

/** The categories `source` can filter by, sorted by name. */
function categoriesOf(source) {
  const k = `${state.profile.game}:${source}`;
  if (!categoryLists.has(k)) {
    const list = invoke("search_categories", { game: state.profile.game, source })
      .then((categories) => categories.sort((a, b) => a.name.localeCompare(b.name)))
      // a failed fetch is tried again next time
      .catch((e) => (categoryLists.delete(k), Promise.reject(e)));
    categoryLists.set(k, list);
  }
  return categoryLists.get(k);
}

/** `source`'s filter, starting it from the profile's hidden tags if it's new. */
async function filterOf(source) {
  const p = state.profile;
  if (state.filters[source]) return state.filters[source];
  let hide = [];
  if (p.hiddenTags.length) {
    const hidden = p.hiddenTags.map((t) => t.toLowerCase());
    // not knowing the categories just means searching without hiding any
    const categories = await categoriesOf(source).catch(() => []);
    hide = categories.filter((c) => hidden.includes(c.name.toLowerCase())).map((c) => c.id);
  }
  // another search may have started it while this was waiting
  if (state.profile === p) state.filters[source] ??= { only: [], hide };
  return state.filters[source] ?? { only: [], hide };
}

/** How many categories the ticked sources' filters show alone or hide. */
function filterCount() {
  return ticked().reduce((n, s) => {
    const filter = state.filters[s];
    return n + (filter ? filter.only.length + filter.hide.length : 0);
  }, 0);
}

function renderFilterCount() {
  const n = filterCount();
  filterMenu.querySelector("summary").textContent = n ? `Filter (${n})` : "Filter";
}

/**
 * Lists each ticked source's categories, under its name when there are
 * several; each category can be shown on its own (along with any others
 * shown on their own) or hidden.
 */
async function renderSourceFilter() {
  const list = ticked();
  const items = filterMenu.querySelector(".menu-items");
  items.replaceChildren(h("p", { class: "muted menu-note" }, "Loading categories…"));
  const loaded = await Promise.all(
    list.map(async (s) => {
      try {
        const [categories] = await Promise.all([categoriesOf(s), filterOf(s)]);
        return categories;
      } catch {
        return null;
      }
    }),
  );
  if (!filterMenu.open || ticked().join() !== list.join()) return;
  const named = list.length > 1;
  const sections = list.map((source, i) => {
    const categories = loaded[i];
    const head = named && h("div", { class: "muted menu-note category-source" }, sourceName(source));
    if (!categories) return [head, h("p", { class: "muted menu-note" }, "Couldn't load the categories.")];
    if (!categories.length) {
      return named ? [] : [h("p", { class: "muted menu-note" }, "This source has no categories.")];
    }
    const filter = state.filters[source];
    // Buttons are updated in place rather than redrawn, since a click on one
    // that's gone counts as a click outside, which closes the menu.
    const rows = categories.map((c) => {
      const toggle = (kind, label, title) =>
        h(
          "button",
          {
            type: "button",
            class: `toggle ${kind}`,
            title,
            "aria-pressed": String(filter[kind].includes(c.id)),
            onclick: () => setFilter(source, c.id, kind),
          },
          label,
        );
      return h(
        "div",
        { class: "category", "data-source": source, "data-id": c.id },
        h("span", {}, c.name),
        toggle("only", "Only", `Only show mods in ${c.name}, or in other categories shown only`),
        toggle("hide", "Hide", `Leave out mods in ${c.name}`),
      );
    });
    return [head, ...rows];
  });
  const clear = h("button", { type: "button", class: "clear", onclick: clearFilter }, "Clear");
  const all = sections.flat().filter(Boolean);
  items.replaceChildren(
    h("div", { class: "menu-head" }, h("span", { class: "muted menu-note" }, "Categories"), clear),
    all.length
      ? h("div", { class: "categories" }, all)
      : h("p", { class: "muted menu-note" }, "These sources have no categories."),
  );
  updateSourceFilter();
}

/** Brings the open menu's buttons and the summary's count up to date. */
function updateSourceFilter() {
  for (const row of filterMenu.querySelectorAll(".category")) {
    const filter = state.filters[row.dataset.source];
    if (!filter) continue;
    for (const kind of ["only", "hide"]) {
      const on = filter[kind].includes(row.dataset.id);
      row.querySelector(`.${kind}`).setAttribute("aria-pressed", String(on));
    }
  }
  const clear = filterMenu.querySelector(".clear");
  if (clear) clear.disabled = !filterCount();
  renderFilterCount();
}

/** Turns `kind` ("only" or "hide") on or off for a category, and searches again. */
function setFilter(source, id, kind) {
  const filter = state.filters[source];
  const other = kind === "only" ? "hide" : "only";
  const on = !filter[kind].includes(id);
  filter[kind] = filter[kind].filter((c) => c !== id);
  if (on) filter[kind].push(id);
  // a category can't be both shown alone and hidden
  filter[other] = filter[other].filter((c) => c !== id);
  updateSourceFilter();
  searchNow();
}

function clearFilter() {
  for (const s of ticked()) state.filters[s] = { only: [], hide: [] };
  updateSourceFilter();
  searchNow();
}

filterMenu.addEventListener("toggle", () => {
  if (filterMenu.open && sources().length) renderSourceFilter().catch(() => {});
});

// Searches as you type, waiting for a short pause so each keystroke doesn't
// hit every source; Enter, or changing the sort, searches straight away. A
// link in the box isn't searched for: Enter adds it (see addlink.js).
function searchNow() {
  clearTimeout(typing);
  search().catch(() => {});
}

/** What's in the box changed; called by addlink.js. */
export function browseChanged() {
  clearTimeout(typing);
  if (state.tab !== ADD_TAB || !sources().length || asLink(box.value)) return;
  if (current(state.search)) return;
  typing = setTimeout(searchNow, 300);
}

$("#browse-link").addEventListener("submit", (e) => {
  e.preventDefault();
  if (!asLink(box.value)) searchNow();
});
form.addEventListener("submit", (e) => {
  e.preventDefault();
  searchNow();
});
form.elements.sort.addEventListener("change", searchNow);
