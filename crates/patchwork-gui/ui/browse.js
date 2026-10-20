// The source tabs, where a source's mods can be searched for and added.

import { run, state, status } from "./state.js";
import { $, h, ms, nextFrame } from "./dom.js";
import { renderProfile } from "./profile.js";
import { cardMain } from "./mods.js";
import { refreshDetails } from "./details.js";

const SOURCE_NAMES = { thunderstore: "Thunderstore", nexus: "Nexus Mods" };
const sourceName = (s) => SOURCE_NAMES[s] ?? s.charAt(0).toUpperCase() + s.slice(1);

/** Opens the Mods tab ("mods") or a source's tab; source tabs all share #tab-browse. */
export function selectTab(key) {
  state.source = key === "mods" ? null : key;
  for (const tab of document.querySelectorAll("#tabs [role=tab]")) {
    tab.setAttribute("aria-selected", String((tab.dataset.source ?? "mods") === key));
  }
  $("#tab-mods").hidden = key !== "mods";
  $("#tab-browse").hidden = key === "mods";
  if (state.source) {
    // the search form is shared, so put back what this tab last searched for
    const last = state.searches[key];
    const form = $("#search");
    form.elements.text.placeholder = `Search ${sourceName(key)}`;
    form.elements.text.value = last?.query.text ?? "";
    form.elements.sort.value = last?.query.sort ?? "";
    renderResults(last);
    // a source opens on its home page, which is a search with no text
    if (!last) search(0).catch(() => {});
  }
}

/** Replaces the source tabs, staying on the open one if it's still there. */
export function renderSourceTabs(sources) {
  $("#tabs").replaceChildren(
    h("button", { role: "tab", onclick: () => selectTab("mods") }, "Mods"),
    ...sources.map((s) =>
      h("button", { role: "tab", "data-source": s, onclick: () => selectTab(s) }, sourceName(s)),
    ),
  );
  selectTab(sources.includes(state.source) ? state.source : "mods");
}

/** Each source's latest search, so an older one finishing late is ignored. */
const searchSeq = {};

async function search(page) {
  const p = state.profile;
  const source = state.source;
  if (!p || !source) return;
  const form = new FormData($("#search"));
  const query = {
    game: p.game,
    source,
    text: form.get("text"),
    sort: form.get("sort") || null,
    page,
  };
  const seq = (searchSeq[source] = (searchSeq[source] ?? 0) + 1);
  status("Searching…");
  const started = performance.now();
  const results = await run("search", query);
  const fetched = performance.now();
  if (seq !== searchSeq[source] || state.profile?.id !== p.id) return;
  state.searches[source] = { page, query, results };
  // The user may have switched tabs while this was in flight.
  if (state.source === source) renderResults(state.searches[source]);
  await nextFrame();
  status(
    `${results.total ?? results.items.length} mods · fetched in ${ms(fetched - started)}, ` +
      `rendered in ${ms(performance.now() - fetched)}`,
  );
}

/** Shows a source's last search, or nothing if it hasn't been searched. */
function renderResults(search) {
  if (!search) {
    $("#results").replaceChildren();
    $("#pager").hidden = true;
    return;
  }
  const { page, results } = search;
  const added = new Set(state.profile.mods.map((m) => `${m.source}:${m.id}`));
  $("#results").replaceChildren(
    ...results.items.map((item) => {
      const isAdded = added.has(`${item.source}:${item.id}`);
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
    }),
  );
  if (!results.items.length) {
    $("#results").append(h("p", { class: "empty" }, "No mods found."));
  }
  $("#pager").hidden = page === 0 && !results.has_more;
  $("#prev").disabled = page === 0;
  $("#next").disabled = !results.has_more;
  $("#page-info").textContent = `Page ${page + 1}`;
  $("#tab-browse").scrollTop = 0;
}

$("#search").addEventListener("submit", (e) => {
  e.preventDefault();
  search(0);
});
// Emptying the search bar goes back to the source's home page.
$("#search [name=text]").addEventListener("input", (e) => {
  if (!e.target.value.trim() && state.searches[state.source]?.query.text?.trim()) {
    search(0).catch(() => {});
  }
});
$("#prev").addEventListener("click", () => search(state.searches[state.source].page - 1));
$("#next").addEventListener("click", () => search(state.searches[state.source].page + 1));
