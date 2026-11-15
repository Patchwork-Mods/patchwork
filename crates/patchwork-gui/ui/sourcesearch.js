// Searching the game's sources from the Mods tab's search box, for mods
// that aren't in the profile yet. Every source ticked on the Add mods tab
// is searched at once, as you type after a pause, and what they find shows
// under the mods (#mods-more), merged, with "See all" opening the search on
// the Add mods tab (see openSearch() in browse.js). A link in the box is
// added instead (see addlink.js).

import { sourceName, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { modQuery } from "./toolbar.js";
import { addedMods, failures, openSearch, resultCard, searchAll, sourceNames, ticked } from "./browse.js";

const more = $("#mods-more");

/**
 * The search for what's in the box: `{ text, sources, loading, search,
 * previous }`, where `search` is what was found (see searchAll()). Null
 * while the box is empty or holds a link. What the last search found stays
 * up, as `previous`, until the next one's done.
 */
let found = null;

/** The search waiting for the user to stop typing. */
let typing;

/** Brings #mods-more up to date with what's in the box; called by addlink.js. */
export function boxChanged() {
  const text = modQuery();
  clearTimeout(typing);
  if (!text) found = null;
  else if (!sameSearch()) typing = setTimeout(() => searchSources({ scroll: false }), 300);
  render();
}

/** The sources to search changed, on the Add mods tab. */
export function sourcesChanged() {
  if (found && !sameSearch()) boxChanged();
}

/** Whether `found` is a search of what's in the box, of the ticked sources. */
const sameSearch = () =>
  !!found && found.text === modQuery() && found.sources.join() === ticked().join();

/**
 * Searches the ticked sources for what's in the box, showing what they find
 * under the mods, and scrolling to it with `scroll`.
 */
async function searchSources({ scroll = true } = {}) {
  clearTimeout(typing);
  const text = modQuery();
  const list = ticked();
  if (!state.profile || !text || !list.length) return;
  const names = sourceNames(list);
  // what was last found stays up meanwhile
  const previous = found?.search ?? found?.previous ?? null;
  const mine = (found = { text, sources: list, loading: true, search: null, previous });
  render();
  status(`Searching ${names}…`);
  const search = await searchAll(list, text);
  if (found !== mine) return;
  mine.loading = false;
  mine.search = search;
  // null means another profile was opened
  if (!search) found = null;
  render();
  if (!search) return;
  const failed = failures(search);
  if (failed.length === list.length) {
    status(`Couldn't search ${names}: ${failed[0][1]}`, true);
    return;
  }
  status(`${search.items.length} mods found on ${names}`);
  if (scroll) more.scrollIntoView({ block: "nearest" });
}

function render() {
  const text = modQuery();
  const list = found?.sources ?? ticked();
  more.hidden = !state.profile || !text || !list.length;
  if (more.hidden) return more.replaceChildren();
  const names = sourceNames(list);
  const search = found?.search ?? found?.previous;
  const head = (...rest) =>
    h("div", { class: "row mods-more-head" }, h("strong", {}, `From ${names}`), ...rest);
  if (!search) {
    return more.replaceChildren(head(), h("p", { class: "muted" }, `Searching ${names}…`));
  }
  const failed = failures(search);
  if (failed.length === search.sources.length) {
    return more.replaceChildren(
      head(),
      h(
        "p",
        { class: "muted" },
        `Couldn't search ${names}. `,
        h("button", { type: "button", onclick: () => searchSources() }, "Try again"),
      ),
    );
  }
  // Mods already in the profile, and its dependencies (including replaced
  // ones, which another mod stands in for), are in the lists above, so
  // they're left out here; the Add mods tab still shows them. One added
  // from here keeps its card, as "Added", until the results are redrawn.
  const added = addedMods();
  const listed = new Set([...added, ...(state.profile.dependencies ?? []).map((d) => d.key)]);
  const items = search.items.filter((item) => !listed.has(`${item.source}:${item.id}`));
  const labelled = search.sources.length > 1;
  const anyMore = search.sources.some((s) => search.parts[s]?.hasMore);
  more.replaceChildren(
    head(
      h(
        "button",
        {
          type: "button",
          class: "mods-more-all",
          title: "Open this search on the Add mods tab, to sort, filter and see every result",
          onclick: () => openSearch(search),
        },
        anyMore ? "See all on Add mods" : "Open in Add mods",
      ),
    ),
    failed.length
      ? h("p", { class: "muted" }, failed.map(([s]) => `Couldn't search ${sourceName(s)}.`).join(" "))
      : null,
    items.length
      ? h(
          "ul",
          { class: "result-grid" },
          items.map((item) => resultCard(item, added, { labelled })),
        )
      : h(
          "p",
          { class: "empty" },
          search.items.length
            ? `You already have everything ${names} found for “${search.text}”.`
            : `Nothing on ${names} matches “${search.text}”.`,
        ),
  );
}

// Enter in the box adds the mod for a link (see addlink.js), and otherwise
// searches straight away.
$("#mods-link").addEventListener("submit", (e) => {
  e.preventDefault();
  if (modQuery()) searchSources();
});
