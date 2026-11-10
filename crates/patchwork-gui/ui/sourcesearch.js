// Searching one of the game's sources from the Mods tab's search box, for
// mods that aren't in the profile yet. When the game has just one source,
// it's searched as you type, after a pause; with more, only on Enter or the
// button under the mods (#mods-more), since which to search is a choice
// and each keystroke shouldn't hit several. The source is the one the Add
// mods tab would open on, and what's found counts as that source's latest
// search there (see searchFor() in browse.js), so "See more" carries on
// from it. The hint under the box (.link-hint) says what Enter will do.

import { invoke, sourceName, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { modQuery, modSearch } from "./toolbar.js";
import { asLink, site } from "./addlink.js";
import { addedMods, lastSource, openSearch, resultCard, searchFor, sources } from "./browse.js";

const hint = $("#mods-link .link-hint");
const more = $("#mods-more");

/**
 * The search of a source for what's in the box, once one's been asked for:
 * `{ text, source, loading, failed, search, previous }`, where `search` is
 * what it found. Null once the box says something else, except while
 * searching as you type, when it stays up until the next search starts,
 * and then shows as `previous` until that one's done.
 */
let found = null;

/** Whether the source is searched as you type: when it's the only one. */
const asYouType = () => sources().length === 1;

/** The search waiting for the user to stop typing. */
let typing;

/** Counts link lookups, so one finishing late doesn't replace a newer hint. */
let lookups = 0;

/** Brings the hint and #mods-more up to date with what's in the box. */
export function boxChanged() {
  const text = modQuery();
  const auto = asYouType() && !!text;
  clearTimeout(typing);
  if (found && found.text !== text && !auto) found = null;
  if (auto && found?.text !== text) {
    typing = setTimeout(() => searchSource(lastSource(), { scroll: false }), 300);
  }
  renderHint().catch(() => {});
  render();
}

function showHint(text) {
  hint.textContent = text ?? "";
  hint.hidden = !text;
}

async function renderHint() {
  const p = state.profile;
  const lookup = ++lookups;
  const link = asLink(modSearch.value);
  if (!p) return showHint(null);
  if (!link) {
    const source = lastSource();
    const ask = modQuery() && source && !asYouType();
    return showHint(ask ? `Search ${sourceName(source)} for more mods` : null);
  }
  const source = await invokeQuietly("link_source", { game: p.game, link });
  if (lookup !== lookups) return;
  const from = source === "url" ? site(link) : source && sourceName(source);
  showHint(
    source === null
      ? "Not a link patchwork recognizes"
      : from
        ? `Add the mod from ${from}`
        : "Add the mod from this link",
  );
}

/** Like run(), without putting a failure in the status bar; null if it fails. */
async function invokeQuietly(command, args) {
  try {
    return await invoke(command, args);
  } catch {
    return null;
  }
}

/**
 * Searches `source` for what's in the box, showing what it finds under the
 * mods, and scrolling to it with `scroll`.
 */
async function searchSource(source, { scroll = true } = {}) {
  clearTimeout(typing);
  const text = modQuery();
  if (!state.profile || !text || !source) return;
  const name = sourceName(source);
  // while typing, what the source last found stays up meanwhile
  const previous = found?.source === source ? (found.search ?? found.previous) : null;
  const mine = (found = { text, source, loading: true, failed: false, search: null, previous });
  render();
  status(`Searching ${name}…`);
  let search;
  try {
    search = await searchFor(source, text);
  } catch {
    if (found !== mine) return;
    mine.loading = false;
    mine.failed = true;
    render();
    return;
  }
  if (found !== mine) return;
  mine.loading = false;
  mine.search = search;
  // null means another profile was opened, or the source searched again
  if (!search) found = null;
  render();
  if (!search) return;
  status(`${search.total ?? search.items.length} mods on ${name}`);
  if (scroll) more.scrollIntoView({ block: "nearest" });
}

/** The source buttons, when there's more than one to search. */
function sourcePicker(current) {
  const list = sources();
  if (list.length < 2) return null;
  return h(
    "div",
    { class: "segmented", role: "group", "aria-label": "Source" },
    list.map((s) =>
      h(
        "button",
        {
          type: "button",
          "aria-pressed": String(s === current),
          onclick: () => s !== current && searchSource(s),
        },
        sourceName(s),
      ),
    ),
  );
}

function render() {
  const text = modQuery();
  const source = found?.source ?? lastSource();
  more.hidden = !state.profile || !text || !source;
  if (more.hidden) return more.replaceChildren();
  const name = sourceName(source);
  if (!found && asYouType()) {
    return more.replaceChildren(h("p", { class: "muted" }, `Searching ${name}…`));
  }
  if (!found) {
    return more.replaceChildren(
      h(
        "button",
        { type: "button", class: "mods-more-search", onclick: () => searchSource(source) },
        `Search ${name} for “${text}”`,
      ),
    );
  }
  const head = (...rest) =>
    h(
      "div",
      { class: "row mods-more-head" },
      h("strong", {}, `From ${name}`),
      sourcePicker(source),
      ...rest,
    );
  const search = found.search ?? found.previous;
  if (found.loading && !search) {
    return more.replaceChildren(head(), h("p", { class: "muted" }, `Searching ${name}…`));
  }
  if (found.failed) {
    return more.replaceChildren(
      head(),
      h(
        "p",
        { class: "muted" },
        `Couldn't search ${name}. `,
        h("button", { type: "button", onclick: () => searchSource(source) }, "Try again"),
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
  more.replaceChildren(
    head(
      h(
        "button",
        {
          type: "button",
          class: "mods-more-all",
          title: "Open this search on the Add mods tab, to sort, filter and see every result",
          onclick: () => openSearch(source, search),
        },
        !search.hasMore
          ? "Open in Add mods"
          : search.total != null
            ? `See all ${search.total} on Add mods`
            : "See more on Add mods",
      ),
    ),
    items.length
      ? h(
          "ul",
          { class: "result-grid" },
          items.map((item) => resultCard(item, added)),
        )
      : h(
          "p",
          { class: "empty" },
          search.items.length
            ? `You already have everything ${name} found for “${search.query.text}”.`
            : `Nothing on ${name} matches “${search.query.text}”.`,
        ),
  );
}

// Enter in the box adds the mod for a link (see addlink.js), and otherwise
// searches the source.
$("#mods-link").addEventListener("submit", (e) => {
  e.preventDefault();
  if (asLink(modSearch.value)) return;
  searchSource(found?.source ?? lastSource());
});
