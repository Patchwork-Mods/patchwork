// The source tabs, where a source's mods can be searched for and added.

import { run, state, status } from "./state.js";
import { $, h, ms, nextFrame } from "./dom.js";
import { renderProfile } from "./profile.js";
import { cardMain } from "./mods.js";
import { refreshDetails } from "./details.js";
import { go } from "./nav.js";
import { openPlugins } from "./plugins.js";

const SOURCE_NAMES = { thunderstore: "Thunderstore", nexus: "Nexus Mods" };
const sourceName = (s) => SOURCE_NAMES[s] ?? s.charAt(0).toUpperCase() + s.slice(1);

/** The search waiting for the user to stop typing. */
let typing;

/**
 * Opens the Mods tab ("mods"), the Plugins tab ("plugins") or a source's
 * tab; source tabs all share #tab-browse.
 */
export function selectTab(key) {
  state.tab = key;
  state.source = key === "mods" || key === "plugins" ? null : key;
  for (const tab of document.querySelectorAll("#tabs [role=tab]")) {
    tab.setAttribute("aria-selected", String(tab.dataset.tab === key));
  }
  $("#tab-mods").hidden = key !== "mods";
  $("#tab-plugins").hidden = key !== "plugins";
  $("#tab-browse").hidden = !state.source;
  clearTimeout(typing);
  if (key === "plugins") openPlugins();
  if (state.source) {
    // the search form is shared, so put back what this tab last searched for
    const last = state.searches[key];
    const form = $("#search");
    form.elements.text.placeholder = `Search ${sourceName(key)}`;
    form.elements.text.value = last?.query.text ?? "";
    form.elements.sort.value = last?.query.sort ?? "";
    renderResults(last);
    renderFilterCount();
    // a source opens on its home page, which is a search with no text
    if (!last) search().catch(() => {});
  }
}

/**
 * Replaces the tabs after Mods, staying on the open one if it's still there.
 * The Plugins tab is there for profiles whose game has plugins.
 */
export function renderSourceTabs(sources) {
  const plugins = !!state.profile?.hasPlugins;
  const tab = (key, label) =>
    h("button", { role: "tab", "data-tab": key, onclick: () => go(() => selectTab(key)) }, label);
  $("#tabs").replaceChildren(
    tab("mods", "Mods"),
    // replaceChildren would turn a null into a "null" text node
    ...(plugins ? [tab("plugins", "Plugins")] : []),
    ...sources.map((s) => tab(s, sourceName(s))),
  );
  const still = state.tab === "plugins" ? plugins : sources.includes(state.tab);
  selectTab(still ? state.tab : "mods");
}

/** Each source's latest search, so an older one finishing late is ignored. */
const searchSeq = {};

/** Starts a new search from the form, replacing the source's results. */
async function search() {
  const p = state.profile;
  const source = state.source;
  if (!p || !source) return;
  const form = new FormData($("#search"));
  const seq = (searchSeq[source] = (searchSeq[source] ?? 0) + 1);
  status("Searching…");
  const started = performance.now();
  const filter = await filterOf(source);
  // the first search on a tab is what starts its filter off
  if (state.source === source) renderFilterCount();
  const query = {
    game: p.game,
    source,
    text: form.get("text"),
    sort: form.get("sort") || null,
    categories: filter.only,
    excludedCategories: filter.hide,
    page: 0,
  };
  const results = await run("search", { query });
  const fetched = performance.now();
  if (seq !== searchSeq[source] || state.profile?.id !== p.id) return;
  state.searches[source] = {
    query,
    items: results.items,
    total: results.total,
    hasMore: results.has_more,
    loading: false,
  };
  // The user may have switched tabs while this was in flight.
  if (state.source === source) renderResults(state.searches[source]);
  await nextFrame();
  status(
    `${results.total ?? results.items.length} mods · fetched in ${ms(fetched - started)}, ` +
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
const addedMods = () => new Set(state.profile.mods.map(key));

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

function resultCard(item, added) {
  const isAdded = added.has(key(item));
  return h(
    "li",
    { class: "result" },
    cardMain(item),
    h(
      "button",
      {
        class: "add",
        disabled: isAdded,
        onclick: async (e) => {
          e.target.disabled = true;
          status(`Adding ${item.name}…`);
          const added = await run("add_mod", {
            id: state.profile.id,
            source: item.source,
            modId: item.id,
          }).catch(() => (e.target.disabled = false));
          if (added) {
            status(`Added ${added.name}`);
            e.target.textContent = "Added";
            state.profile = added.profile;
            renderProfile();
            refreshDetails();
          }
        },
      },
      isAdded ? "Added" : "Add",
    ),
  );
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
