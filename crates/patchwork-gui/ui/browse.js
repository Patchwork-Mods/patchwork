// The Add mods tab: a link box (see addlink.js), then a search of one of the
// game's sources, where its mods can be found and added. The Mods tab's
// search box can search a source too (see sourcesearch.js), which counts as
// the source's latest search here.

import { run, sourceName, state, status } from "./state.js";
import { $, h, ms, nextFrame } from "./dom.js";
import { renderProfile } from "./profile.js";
import { cardMain } from "./mods.js";
import { refreshDetails } from "./details.js";
import { go } from "./nav.js";
import { listOf, listTab, openList } from "./list.js";
import { CONFIG_TAB, openConfig } from "./config.js";

/** The Add mods tab's key in state.tab. */
export const ADD_TAB = "add";

/** The search waiting for the user to stop typing. */
let typing;

/** The source last picked on the Add mods tab, kept while switching tabs. */
let picked = null;

/** The sources the open profile's game can be searched on. */
export const sources = () => state.profile?.searchSources ?? [];

/** The source the Add mods tab would open on: the one last picked or searched. */
export const lastSource = () => (sources().includes(picked) ? picked : (sources()[0] ?? null));

/**
 * Opens the Mods tab ("mods"), a list's tab (see listTab), the Config tab
 * or the Add mods tab; list tabs all share #tab-list.
 */
export function selectTab(key) {
  const list = listOf(key);
  const config = key === CONFIG_TAB;
  const add = key === ADD_TAB;
  state.tab = key;
  state.source = add ? lastSource() : null;
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
  if (add) showSource();
}

/** Shows the open source's search on the Add mods tab, or hides it if there's none. */
function showSource() {
  const source = state.source;
  renderSourcePicker();
  $("#search").hidden = !source;
  if (!source) return renderResults(null);
  // the search form is shared, so put back what this source last searched for
  const last = state.searches[source];
  const form = $("#search");
  form.elements.text.placeholder = `Search ${sourceName(source)}`;
  form.elements.text.value = last?.query.text ?? "";
  form.elements.sort.value = last?.query.sort ?? "";
  renderResults(last);
  renderFilterCount();
  // a source opens on its home page, which is a search with no text
  if (!last) search().catch(() => {});
}

/** Switches the Add mods tab to searching `source`. */
function pickSource(source) {
  if (source === state.source) return;
  picked = source;
  clearTimeout(typing);
  state.source = source;
  showSource();
}

/** One button per source, when there's more than one to pick between. */
function renderSourcePicker() {
  const list = sources();
  const picker = $("#source-picker");
  picker.hidden = list.length < 2;
  picker.replaceChildren(
    ...list.map((s) =>
      h(
        "button",
        { type: "button", "aria-pressed": String(s === state.source), onclick: () => pickSource(s) },
        sourceName(s),
      ),
    ),
  );
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

/** Each source's latest search, so an older one finishing late is ignored. */
const searchSeq = {};

/**
 * Searches `source` for `text`, keeping it as the source's latest search,
 * and returns it, or null if a newer search of the source started first or
 * another profile was opened.
 */
async function searchSource(source, text, sort = null) {
  const p = state.profile;
  const seq = (searchSeq[source] = (searchSeq[source] ?? 0) + 1);
  const filter = await filterOf(source);
  // the first search on a tab is what starts its filter off
  if (state.source === source) renderFilterCount();
  const query = {
    game: p.game,
    source,
    text,
    sort,
    categories: filter.only,
    excludedCategories: filter.hide,
    page: 0,
  };
  const results = await run("search", { query });
  if (seq !== searchSeq[source] || state.profile?.id !== p.id) return null;
  return (state.searches[source] = {
    query,
    items: results.items,
    total: results.total,
    hasMore: results.has_more,
    loading: false,
  });
}

/**
 * Searches `source` for `text` from somewhere other than the Add mods tab,
 * which then opens on it with this search showing; see openSearch().
 */
export function searchFor(source, text) {
  picked = source;
  return searchSource(source, text);
}

/**
 * Opens the Add mods tab on `source`, showing `found` (from searchFor()) if
 * nothing has replaced it as the source's latest search, to see more of it.
 */
export function openSearch(source, found) {
  picked = source;
  if (found && !state.searches[source]) state.searches[source] = found;
  return go(() => selectTab(ADD_TAB));
}

/** Starts a new search from the form, replacing the source's results. */
async function search() {
  const p = state.profile;
  const source = state.source;
  if (!p || !source) return;
  const form = new FormData($("#search"));
  status("Searching…");
  const started = performance.now();
  const found = await searchSource(source, form.get("text"), form.get("sort") || null);
  const fetched = performance.now();
  if (!found) return;
  // The user may have switched tabs while this was in flight.
  if (state.source === source) renderResults(found);
  await nextFrame();
  status(
    `${found.total ?? found.items.length} mods · fetched in ${ms(fetched - started)}, ` +
      `rendered in ${ms(performance.now() - fetched)}`,
  );
}

/** Adds the next page of the open source's search to the bottom. */
async function loadMore() {
  const source = state.source;
  const last = state.searches[source];
  if (!last?.hasMore || last.loading) return;
  // Doesn't bump searchSeq: a new search started meanwhile wins over this.
  const seq = searchSeq[source];
  last.loading = true;
  const query = { ...last.query, page: last.query.page + 1 };
  status("Loading more…");
  let results;
  try {
    results = await run("search", { query });
  } finally {
    last.loading = false;
  }
  if (seq !== searchSeq[source] || state.searches[source] !== last) return;
  // Mods can shift between pages while scrolling, so skip ones already shown.
  const shown = new Set(last.items.map(key));
  const items = results.items.filter((item) => !shown.has(key(item)));
  last.query = query;
  last.items.push(...items);
  last.total = results.total;
  last.hasMore = results.has_more;
  if (state.source === source) {
    const added = addedMods();
    $("#results").append(...items.map((item) => resultCard(item, added)));
    showMore(last);
  }
  status(`${last.total ?? last.items.length} mods · showing ${last.items.length}`);
}

const key = (item) => `${item.source}:${item.id}`;
export const addedMods = () => new Set(state.profile.mods.map(key));

/**
 * Whether one of the files of search result `item` is in `added`, like
 * `nexus:17230/Engine Fixes - SKSE64 Preloader` for `nexus:17230`: ids that
 * pick one of a mod's files have it after a slash.
 */
const fileAdded = (item, added) => [...added].some((k) => k.startsWith(`${key(item)}/`));

/** Shows a source's last search, or nothing if it hasn't been searched. */
function renderResults(search) {
  if (!search) {
    $("#results").replaceChildren();
    $("#more").hidden = true;
    return;
  }
  const added = addedMods();
  $("#results").replaceChildren(...search.items.map((item) => resultCard(item, added)));
  if (!search.items.length) {
    $("#results").append(h("p", { class: "empty" }, "No mods found."));
  }
  $("#tab-browse").scrollTop = 0;
  showMore(search);
}

/** A search result, with a button to add it unless it's in `added` (see addedMods()). */
export function resultCard(item, added) {
  const isAdded = added.has(key(item));
  const card = h(
    "li",
    { class: "result" },
    cardMain(item),
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
function showMore(search) {
  const more = $("#more");
  more.hidden = !search.hasMore;
  // Observing again re-checks it, in case a page didn't push it out of reach.
  moreObserver.unobserve(more);
  if (search.hasMore) moreObserver.observe(more);
}

// Filtering by category. The source does the filtering, so paging and the
// number of mods found stay right. A source's filter starts out hiding the
// categories the Mods tab's filter hides, matched by name; changing it here
// only lasts until the profile is opened again.

const filterMenu = $("#source-filter");

/** Each source's categories for each game, by "game:source", once asked for. */
const categoryLists = new Map();

/** The categories `source` can filter by, sorted by name. */
function categoriesOf(source) {
  const k = `${state.profile.game}:${source}`;
  if (!categoryLists.has(k)) {
    const list = run("search_categories", { game: state.profile.game, source })
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

function renderFilterCount() {
  const filter = state.filters[state.source];
  const n = filter ? filter.only.length + filter.hide.length : 0;
  filterMenu.querySelector("summary").textContent = n ? `Filter (${n})` : "Filter";
}

/**
 * Lists the open source's categories, each of which can be shown on its own
 * (along with any others shown on their own) or hidden.
 */
async function renderSourceFilter() {
  const source = state.source;
  const items = filterMenu.querySelector(".menu-items");
  items.replaceChildren(h("p", { class: "muted menu-note" }, "Loading categories…"));
  let categories;
  try {
    [categories] = await Promise.all([categoriesOf(source), filterOf(source)]);
  } catch {
    items.replaceChildren(h("p", { class: "muted menu-note" }, "Couldn't load the categories."));
    return;
  }
  if (state.source !== source) return;
  if (!categories.length) {
    items.replaceChildren(h("p", { class: "muted menu-note" }, "This source has no categories."));
    return;
  }
  const filter = state.filters[source];
  // Buttons are updated in place rather than redrawn, since a click on one
  // that's gone counts as a click outside, which closes the menu.
  const rows = categories.map((c) => {
    const toggle = (list, label, title) =>
      h(
        "button",
        {
          type: "button",
          class: `toggle ${list}`,
          title,
          "aria-pressed": String(filter[list].includes(c.id)),
          onclick: () => setFilter(source, c.id, list),
        },
        label,
      );
    return h(
      "div",
      { class: "category", "data-id": c.id },
      h("span", {}, c.name),
      toggle("only", "Only", `Only show mods in ${c.name}, or in other categories shown only`),
      toggle("hide", "Hide", `Leave out mods in ${c.name}`),
    );
  });
  const clear = h(
    "button",
    { type: "button", class: "clear", onclick: () => clearFilter(source) },
    "Clear",
  );
  items.replaceChildren(
    h("div", { class: "menu-head" }, h("span", { class: "muted menu-note" }, "Categories"), clear),
    h("div", { class: "categories" }, rows),
  );
  updateSourceFilter();
}

/** Brings the open menu's buttons and the summary's count up to date. */
function updateSourceFilter() {
  const filter = state.filters[state.source];
  if (!filter) return;
  for (const row of filterMenu.querySelectorAll(".category")) {
    for (const list of ["only", "hide"]) {
      const on = filter[list].includes(row.dataset.id);
      row.querySelector(`.${list}`).setAttribute("aria-pressed", String(on));
    }
  }
  const clear = filterMenu.querySelector(".clear");
  if (clear) clear.disabled = !filter.only.length && !filter.hide.length;
  renderFilterCount();
}

/** Turns `list` ("only" or "hide") on or off for a category, and searches again. */
function setFilter(source, id, list) {
  const filter = state.filters[source];
  const other = list === "only" ? "hide" : "only";
  const on = !filter[list].includes(id);
  filter[list] = filter[list].filter((c) => c !== id);
  if (on) filter[list].push(id);
  // a category can't be both shown alone and hidden
  filter[other] = filter[other].filter((c) => c !== id);
  updateSourceFilter();
  searchNow();
}

function clearFilter(source) {
  state.filters[source] = { only: [], hide: [] };
  updateSourceFilter();
  searchNow();
}

filterMenu.addEventListener("toggle", () => {
  if (filterMenu.open && state.source) renderSourceFilter().catch(() => {});
});

// Searches as you type, waiting for a short pause so each keystroke doesn't
// hit the source; changing the sort searches straight away.
function searchNow() {
  clearTimeout(typing);
  search().catch(() => {});
}
$("#search").addEventListener("submit", (e) => {
  e.preventDefault();
  searchNow();
});
$("#search [name=text]").addEventListener("input", () => {
  clearTimeout(typing);
  typing = setTimeout(searchNow, 150);
});
$("#search [name=sort]").addEventListener("change", searchNow);
