// The source tabs, where a source's mods can be searched for and added.

import { run, state, status } from "./state.js";
import { $, h, ms, nextFrame } from "./dom.js";
import { renderProfile } from "./profile.js";
import { cardMain } from "./mods.js";
import { refreshDetails } from "./details.js";
import { go } from "./nav.js";

const SOURCE_NAMES = { thunderstore: "Thunderstore", nexus: "Nexus Mods" };
const sourceName = (s) => SOURCE_NAMES[s] ?? s.charAt(0).toUpperCase() + s.slice(1);

/** The search waiting for the user to stop typing. */
let typing;

/** Opens the Mods tab ("mods") or a source's tab; source tabs all share #tab-browse. */
export function selectTab(key) {
  state.source = key === "mods" ? null : key;
  for (const tab of document.querySelectorAll("#tabs [role=tab]")) {
    tab.setAttribute("aria-selected", String((tab.dataset.source ?? "mods") === key));
  }
  $("#tab-mods").hidden = key !== "mods";
  $("#tab-browse").hidden = key === "mods";
  clearTimeout(typing);
  if (state.source) {
    // the search form is shared, so put back what this tab last searched for
    const last = state.searches[key];
    const form = $("#search");
    form.elements.text.placeholder = `Search ${sourceName(key)}`;
    form.elements.text.value = last?.query.text ?? "";
    form.elements.sort.value = last?.query.sort ?? "";
    renderResults(last);
    // a source opens on its home page, which is a search with no text
    if (!last) search().catch(() => {});
  }
}

/** Replaces the source tabs, staying on the open one if it's still there. */
export function renderSourceTabs(sources) {
  $("#tabs").replaceChildren(
    h("button", { role: "tab", onclick: () => go(() => selectTab("mods")) }, "Mods"),
    ...sources.map((s) =>
      h(
        "button",
        { role: "tab", "data-source": s, onclick: () => go(() => selectTab(s)) },
        sourceName(s),
      ),
    ),
  );
  selectTab(sources.includes(state.source) ? state.source : "mods");
}

/** Each source's latest search, so an older one finishing late is ignored. */
const searchSeq = {};

/** Starts a new search from the form, replacing the source's results. */
async function search() {
  const p = state.profile;
  const source = state.source;
  if (!p || !source) return;
  const form = new FormData($("#search"));
  const query = {
    game: p.game,
    source,
    text: form.get("text"),
    sort: form.get("sort") || null,
    page: 0,
  };
  const seq = (searchSeq[source] = (searchSeq[source] ?? 0) + 1);
  status("Searching…");
  const started = performance.now();
  const results = await run("search", query);
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
    results = await run("search", query);
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
