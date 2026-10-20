// Patchwork's UI. Plain JS with no build step: everything goes through
// invoke() to the commands in main.rs.
//
// Mod names, summaries and so on come from remote sources, so they're only
// ever put in the page as text (see h()), never as HTML.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);
const nextFrame = () => new Promise((r) => requestAnimationFrame(() => r()));
const ms = (t) => `${Math.round(t)}ms`;

/** Makes an element. `attrs` keys starting with "on" are event listeners. */
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value == null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else el.setAttribute(key, value === true ? "" : value);
  }
  el.append(...children.flat().filter((c) => c != null));
  return el;
}

const state = {
  overview: null,
  /** The game being looked at, or null on the game picker. */
  game: null,
  profile: null,
  /** The source whose tab is open, or null on the Mods tab. */
  source: null,
  /** Each source's last search, as `{ page, query, results }`. */
  searches: {},
};

function status(text, isError = false) {
  const el = $("#status");
  el.textContent = text;
  el.classList.toggle("error", isError);
  // An undo only makes sense next to the message about what it undoes.
  $("#undo").hidden = true;
}

/** Runs a command, showing its error in the status bar if it fails. */
async function run(command, args) {
  try {
    return await invoke(command, args);
  } catch (e) {
    status(String(e), true);
    throw e;
  }
}

function compact(n) {
  if (n < 1e3) return String(n);
  if (n < 1e6) return (n / 1e3).toFixed(1) + "k";
  return (n / 1e6).toFixed(1) + "M";
}

/** A link that opens in the browser rather than the app, or plain text when
 * there's no URL. */
function link(url, text) {
  if (!url) return text;
  return h(
    "a",
    {
      href: url,
      title: url,
      onclick: (e) => {
        e.preventDefault();
        window.__TAURI__.opener.openUrl(url).catch((err) => status(String(err), true));
      },
    },
    text,
  );
}

const relativeTime = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

/** "3 days ago" and the like, for an ISO timestamp. */
function ago(timestamp) {
  let delta = (Date.parse(timestamp) - Date.now()) / 1000;
  for (const [unit, size] of [
    ["second", 60],
    ["minute", 60],
    ["hour", 24],
    ["day", 30],
    ["month", 12],
    ["year", Infinity],
  ]) {
    if (Math.abs(delta) < size) return relativeTime.format(Math.round(delta), unit);
    delta /= size;
  }
}

/** Reloads everything, staying on whichever screen and profile is open. */
async function refresh() {
  state.overview = await run("overview");
  for (const warning of state.overview.warnings) console.warn(warning);
  if (state.game && gameInfo(state.game)) {
    renderSidebar();
    if (state.profile) await openProfile(state.profile.id);
    else renderProfile();
  } else {
    await showGames();
  }
}

const gameInfo = (id) => state.overview.games.find((g) => g.id === id);
const profilesOf = (game) => state.overview.profiles.filter((p) => p.game === game);

// Games

function showScreen(screen) {
  document.body.dataset.screen = screen;
  $("#games-screen").hidden = screen !== "games";
  $("#sidebar").hidden = screen !== "game";
  $("#main").hidden = screen !== "game";
}

async function showGames() {
  state.game = null;
  state.profile = null;
  showScreen("games");
  await applyTheme(null);
  renderGames();
}

/** Up to two letters standing in for a game's picture. */
function initials(name) {
  const words = name
    .split(/[\s\-_:]+/)
    .filter((w) => w && !/^(a|an|and|of|the)$/i.test(w));
  return words.slice(0, 2).map((w) => w[0].toUpperCase()).join("") || "?";
}

function renderGames() {
  const { games } = state.overview;
  $("#game-grid").replaceChildren(
    ...games.map((g) => {
      const count = profilesOf(g.id).length;
      const facts = [
        `${count} profile${count === 1 ? "" : "s"}`,
        g.install ? "installed" : "not set up",
      ];
      return h(
        "li",
        {},
        h(
          "button",
          { class: "game-card", "data-game": g.id, onclick: () => openGame(g.id) },
          h("div", { class: "game-art", "aria-hidden": "true" }, initials(g.name)),
          h("span", { class: "game-name" }, g.name),
          h("span", { class: "muted" }, facts.join(" · ")),
          g.origin === "user" ? h("span", { class: "badge" }, "Added by you") : null,
        ),
      );
    }),
    h(
      "li",
      {},
      h(
        "button",
        { class: "game-card add", onclick: openGameDialog },
        h("div", { class: "game-art", "aria-hidden": "true" }, "+"),
        h("span", { class: "game-name" }, "Add a game"),
        h("span", { class: "muted" }, "Set up one patchwork doesn't know yet"),
      ),
    ),
  );
}

/**
 * Shows a game's profiles, opening the current profile if it's for this
 * game, or else its first one.
 */
async function openGame(id) {
  state.game = id;
  state.profile = null;
  showScreen("game");
  selectTab("mods");
  await applyTheme(id);
  renderSidebar();
  const current = state.overview.current;
  const ps = profilesOf(id);
  const pick = ps.find((p) => p.id === current) ?? ps[0];
  if (pick) await switchProfile(pick.id);
  else renderProfile();
}

$("#back").addEventListener("click", () => showGames());

// Profiles

function renderSidebar() {
  const game = gameInfo(state.game);
  const current = state.profile?.id;
  $("#game-title").textContent = game.name;
  const ps = profilesOf(game.id);
  $("#profiles").replaceChildren(
    ...ps.map((p) =>
      h(
        "button",
        { "aria-current": String(p.id === current), onclick: () => switchProfile(p.id) },
        h("span", {}, p.name),
        h("span", { class: "muted" }, String(p.mods)),
      ),
    ),
  );
  if (!ps.length) {
    $("#profiles").append(h("p", { class: "muted" }, "No profiles yet."));
  }
}

async function switchProfile(id) {
  await run("use_profile", { id });
  state.overview.current = id;
  await openProfile(id);
}

async function openProfile(id) {
  state.profile = await run("profile", { id });
  state.searches = {};
  renderSidebar();
  renderProfile();
  refreshVersions();
  refreshSummaries();
}

async function applyTheme(game) {
  document.body.dataset.game = game ?? "";
  const theme = await invoke("theme", { game });
  $("#theme-default").textContent = theme.default;
  $("#theme-game").textContent = theme.game ?? "";
}

function renderProfile() {
  const p = state.profile;
  $("#deploy").disabled = !p?.install;
  $("#update").disabled = !p?.mods.length;
  if (!p) {
    const game = gameInfo(state.game);
    $("#profile-title").textContent = `No ${game.name} profiles yet`;
    $("#profile-meta").textContent = "Create one in the sidebar to start adding mods.";
    $("#mods").replaceChildren();
    $("#hidden-mods").hidden = true;
    $("#mod-problems").replaceChildren();
    $("#mods-empty").hidden = true;
    $("#mods-toolbar").hidden = true;
    $("#game-path").hidden = !!game.install;
    renderSourceTabs([]);
    return;
  }

  $("#profile-title").textContent = p.label;
  const enabled = p.mods.filter((m) => m.enabled).length;
  const deployed =
    p.deployedFiles != null ? `deployed (${p.deployedFiles} files)` : "not deployed";
  $("#profile-meta").textContent =
    `${enabled}/${p.mods.length} mods enabled · ${p.install ?? "game not set up"} · ${deployed}`;
  $("#game-path").hidden = !!p.install;

  renderMods(p.mods);
  $("#mods-toolbar").hidden = !p.mods.length;
  $("#mod-problems").replaceChildren(...p.problems.map((w) => h("li", {}, w)));
  renderSourceTabs(p.searchSources);
}

// The Mods tab shows mods either as cards, like a source's search results,
// or as a denser list without pictures.

/** "cards" or "list"; remembered between runs. */
let modsView = "cards";
try {
  if (localStorage.getItem("modsView") === "list") modsView = "list";
} catch {}

function setModsView(view) {
  modsView = view;
  try {
    localStorage.setItem("modsView", view);
  } catch {}
  for (const button of document.querySelectorAll("#mods-view button")) {
    button.setAttribute("aria-pressed", String(button.dataset.view === view));
  }
  if (state.profile) {
    renderMods(state.profile.mods);
    refreshSummaries();
  }
}

for (const button of document.querySelectorAll("#mods-view button")) {
  button.addEventListener("click", () => setModsView(button.dataset.view));
}

function modRow(m, { onToggle, onRemove, onVersion }) {
  return h(
    "li",
    { class: m.enabled ? null : "disabled" },
    h("input", {
      type: "checkbox",
      checked: m.enabled,
      "aria-label": `Enable ${m.name}`,
      onchange: onToggle,
    }),
    h("span", { class: "mod-name" }, m.name),
    h("span", { class: "mod-source muted" }, `${m.source}:${m.id}`),
    m.versioned
      ? versionSelect(m, onVersion)
      : h("span", { class: "mod-source muted" }, m.version ?? m.locked ?? ""),
    h("button", { class: "remove", onclick: onRemove }, "Remove"),
  );
}

/** A mod's picture and details, from a search result (a ModSummary). */
function cardMain(item) {
  // Hovering a count shows it in full.
  const count = (n, noun) =>
    n != null && h("span", { title: `${n.toLocaleString()} ${noun}` }, `${compact(n)} ${noun}`);
  const stats = [count(item.downloads, "downloads"), count(item.rating, "likes")].filter(Boolean);
  const summary = item.summary?.trim();
  return h(
    "div",
    { class: "card-main" },
    item.icon_url
      ? h("img", { src: item.icon_url, alt: "", loading: "lazy", decoding: "async" })
      : h("div", { class: "no-icon", "aria-hidden": "true" }, initials(item.name)),
    h(
      "div",
      { class: "result-body" },
      h("h3", {}, link(item.url, item.name)),
      item.author && h("div", { class: "author" }, link(item.author_url, item.author)),
      summary && h("p", { class: "muted", title: summary }, summary),
      h(
        "div",
        { class: "facts muted" },
        stats.length > 0 ? h("div", {}, stats.flatMap((s, i) => (i ? [" · ", s] : [s]))) : null,
        item.updated &&
          h(
            "div",
            { title: new Date(item.updated).toLocaleString() },
            `Updated ${ago(item.updated)}`,
          ),
      ),
    ),
  );
}

/** What a profile's mod shows on its card: its summary, once that's been fetched. */
const cardItem = (m) => m.summary ?? { name: m.name, summary: `${m.source}:${m.id}` };

// An added mod's card: an on/off switch and its version along the bottom,
// a note when there's a newer version, and a remove button that only shows
// while the card is hovered or focused.
function modCard(m, { onToggle, onRemove, onVersion }) {
  const main = cardMain(cardItem(m));
  const update = h("div", { class: "card-update" });
  if (state.profile) {
    cardMains.set(modKey(m), main);
    updateNotes.set(modKey(m), { el: update, onVersion });
  }
  fillUpdate(update, m, onVersion);
  return h(
    "li",
    { class: m.enabled ? "result" : "result disabled" },
    main,
    h(
      "button",
      {
        class: "remove",
        title: `Remove ${m.name}`,
        "aria-label": `Remove ${m.name}`,
        onclick: onRemove,
      },
      trashIcon(),
    ),
    update,
    h(
      "div",
      { class: "card-footer" },
      h(
        "label",
        { class: "switch", title: "Disabled mods are left out when deploying" },
        h("input", {
          type: "checkbox",
          role: "switch",
          checked: m.enabled,
          "aria-label": `Enable ${m.name}`,
          onchange: onToggle,
        }),
        h("span", { "aria-hidden": "true" }, m.enabled ? "On" : "Off"),
      ),
      m.versioned
        ? versionSelect(m, onVersion)
        : h("span", { class: "mod-version muted" }, m.version ?? m.locked ?? "No versions"),
    ),
  );
}

/**
 * Says when a newer version than the one in use is out. A pinned mod can be
 * moved to it straight away; one that follows the latest version gets it
 * from Update.
 */
function fillUpdate(el, m, onVersion) {
  const newest = m.versions?.[0];
  const current = m.version ?? m.locked;
  if (!m.versioned || !newest || !current || newest === current) {
    el.replaceChildren();
    return;
  }
  el.replaceChildren(
    m.version
      ? h(
          "button",
          {
            title: `Pinned to ${current}; click to pin to ${newest} instead`,
            onclick: () => onVersion?.(newest),
          },
          `↑ ${newest}`,
        )
      : h("span", { title: `Update locks the newest versions of every mod` }, `↑ ${newest}`),
  );
}

function trashIcon() {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(ns, "path");
  path.setAttribute(
    "d",
    "M4 7h16M10 11v6M14 11v6M5 7l1 12a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2l1-12M9 7V4h6v3",
  );
  svg.append(path);
  return svg;
}

/** Asks before taking a mod out of the profile; resolves to whether to go ahead. */
function confirmRemove(m) {
  const dialog = $("#remove-dialog");
  $("#remove-summary").textContent =
    `Remove ${m.name} from ${state.profile.label}? Its files leave the game on the next deploy.`;
  dialog.returnValue = "";
  dialog.showModal();
  return new Promise((resolve) =>
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "remove"), {
      once: true,
    }),
  );
}

// Versions and summaries. The profile comes with each mod's versions and
// summary as last fetched, so the dropdowns and cards are complete straight
// away; ones older than VERSIONS_TTL or SUMMARY_TTL (see main.rs) are then
// refreshed in the background.

/** The version dropdown of each mod on screen, by modKey(). */
const versionSelects = new Map();
/** The picture and details of each mod card on screen, by modKey(). */
const cardMains = new Map();
/** The newer-version note of each mod card on screen, by modKey(). */
const updateNotes = new Map();
/** Fetches in flight, by modKey(), so re-renders don't repeat them. */
const versionFetches = new Map();
const summaryFetches = new Map();
/** Mods whose versions or summary couldn't be fetched; not retried until a restart. */
const versionFailures = new Set();
const summaryFailures = new Set();

const modKey = (m) => `${state.profile.game}/${m.source}:${m.id}`;

/**
 * Picks a mod's version: "Auto" follows the latest one, anything else
 * freezes the mod at it.
 */
function versionSelect(m, onVersion) {
  const select = h("select", {
    class: "mod-version",
    "aria-label": `Version of ${m.name}`,
    title: "Auto follows the latest version; pick one to freeze the mod at it",
    onchange: async () => {
      select.disabled = true;
      try {
        await onVersion?.(select.value || null);
      } finally {
        select.disabled = false;
      }
    },
  });
  fillVersions(select, m);
  if (state.profile) versionSelects.set(modKey(m), select);
  return select;
}

function fillVersions(select, m) {
  // keep whatever's showing, which may be a choice that's still being saved
  const current = select.options.length ? select.value : (m.version ?? "");
  const versions = m.versions ?? [];
  const auto = !m.version && m.locked ? `Auto (${m.locked})` : "Auto";
  const options = [
    h("option", { value: "" }, auto),
    ...versions.map((v) => h("option", { value: v }, v)),
  ];
  // a pinned version that's since been taken down still shows
  if (current && !versions.includes(current)) {
    options.push(h("option", { value: current }, m.versions ? `${current} (unavailable)` : current));
  }
  select.replaceChildren(...options);
  select.value = current;
}

/** Puts freshly fetched versions into a mod's dropdown. */
function showVersions(key, versions) {
  const m = state.profile?.mods.find((m) => modKey(m) === key);
  if (!m) return;
  m.versions = versions;
  m.versionsStale = false;
  // a newer version than the summary knows of means the card's details
  // (when it was updated, say) are out of date too
  const latest = m.summary?.latest_version;
  if (latest && versions[0] && latest !== versions[0] && !summaryFetches.has(key)) {
    m.summaryStale = true;
    if (wantsSummaries()) fetchSummaries([m]);
  }
  const note = updateNotes.get(key);
  if (note?.el.isConnected) fillUpdate(note.el, m, note.onVersion);
  const select = versionSelects.get(key);
  if (!select?.isConnected) return;
  // Replacing the options of an open dropdown closes it (or leaves the old
  // list showing), so wait until it's no longer in use.
  if (document.activeElement === select) {
    select.addEventListener("blur", () => fillVersions(select, m), { once: true });
  } else {
    fillVersions(select, m);
  }
}

/**
 * Puts a freshly fetched summary on a mod's card, or moves the mod to the
 * other section if its new tags change whether it's hidden.
 */
function showSummary(key, summary) {
  const m = state.profile?.mods.find((m) => modKey(m) === key);
  if (!m) return;
  const wasHidden = isHidden(m);
  m.summary = summary;
  m.summaryStale = false;
  if (isHidden(m) !== wasHidden) {
    renderMods(state.profile.mods);
    return;
  }
  const main = cardMains.get(key);
  if (!main?.isConnected) return;
  const fresh = cardMain(cardItem(m));
  main.replaceWith(fresh);
  cardMains.set(key, fresh);
}

/**
 * Fetches something for each of `mods`, a few at a time, sharing fetches
 * that are already in flight. Stops if another profile is opened, and says
 * in the status bar if any failed.
 */
async function refreshEach(what, mods, { fetches, failures, fetch, show }) {
  const p = state.profile;
  const queue = mods.filter((m) => !failures.has(modKey(m)));
  const failed = [];
  const worker = async () => {
    while (queue.length && state.profile?.id === p.id) {
      const m = queue.shift();
      const key = modKey(m);
      if (!fetches.has(key)) {
        const pending = fetch(p, m);
        fetches.set(key, pending);
        pending.finally(() => fetches.delete(key)).catch(() => {});
      }
      try {
        show(key, await fetches.get(key));
      } catch (e) {
        failures.add(key);
        failed.push(`${m.name}: ${e}`);
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(4, queue.length) }, worker));
  if (failed.length) {
    for (const f of failed) console.warn(f);
    status(`Couldn't ${what} ${failed.length} mod(s); ${failed[0]}`, true);
  }
}

/** Refreshes the profile's stale version lists. */
function refreshVersions() {
  return refreshEach(
    "check for new versions of",
    state.profile.mods.filter((m) => m.versioned && m.versionsStale),
    {
      fetches: versionFetches,
      failures: versionFailures,
      fetch: (p, m) => invoke("versions", { game: p.game, source: m.source, modId: m.id }),
      show: showVersions,
    },
  );
}

/**
 * Refreshes the profile's stale summaries, which only the cards show, and
 * which have the tags the filter goes by.
 */
function refreshSummaries() {
  if (!wantsSummaries()) return;
  return fetchSummaries(state.profile.mods.filter((m) => m.summaryStale));
}

const wantsSummaries = () => modsView === "cards" || state.profile.hiddenTags.length > 0;

function fetchSummaries(mods) {
  return refreshEach("get the details of", mods, {
    fetches: summaryFetches,
    failures: summaryFailures,
    fetch: (p, m) => invoke("summary", { game: p.game, source: m.source, modId: m.id }),
    show: showSummary,
  });
}

function renderMods(mods) {
  const id = state.profile.id;
  versionSelects.clear();
  cardMains.clear();
  updateNotes.clear();
  const render = modsView === "cards" ? modCard : modRow;
  const item = (m) =>
    render(m, {
      onToggle: async (e) => {
        await run("set_enabled", { id, name: m.name, enabled: e.target.checked });
        await openProfile(id);
      },
      onRemove: async () => {
        if (!(await confirmRemove(m))) return;
        await run("remove_mod", { id, name: m.name });
        status(`Removed ${m.name}`);
        await refresh();
      },
      onVersion: async (version) => {
        status(version ? `Pinning ${m.name} to ${version}…` : `Unpinning ${m.name}…`);
        try {
          await run("set_version", { id, name: m.name, version });
          status(version ? `Pinned ${m.name} to ${version}` : `${m.name} follows the latest version`);
        } finally {
          await openProfile(id);
        }
      },
    });
  const hidden = mods.filter(isHidden);
  const layout = modsView === "cards" ? "result-grid" : "mod-list";
  $("#mods").className = layout;
  $("#mods").replaceChildren(...mods.filter((m) => !isHidden(m)).map(item));
  $("#hidden-mods-list").className = layout;
  $("#hidden-mods-list").replaceChildren(...hidden.map(item));
  $("#hidden-mods").hidden = !hidden.length;
  const off = hidden.filter((m) => !m.enabled).length;
  $("#hidden-mods > summary").textContent =
    `Hidden (${hidden.length})` + (off ? ` · ${off} disabled` : "");
  $("#mods-empty").hidden = mods.length > 0;
  $("#filter-menu > summary").textContent = state.profile.hiddenTags.length
    ? `Filter (${state.profile.hiddenTags.length})`
    : "Filter";
}

// Filtering. Mods with any of the profile's hiddenTags go in a collapsed
// section under the rest, which keeps things like libraries out of the way
// without changing the load order. Tags come from each mod's summary, so a
// mod whose summary hasn't been fetched yet stays in the main list until it
// has.

const filterMenu = $("#filter-menu");

const lower = (tags) => tags.map((t) => t.toLowerCase());
const modTags = (m) => m.summary?.categories?.map((c) => c.name) ?? [];

function isHidden(m) {
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
  // list view only fetches summaries while there's something to filter by
  refreshSummaries();
}

filterMenu.addEventListener("toggle", () => {
  if (filterMenu.open && state.profile) renderFilterMenu();
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
for (const menu of [sortMenu, filterMenu]) {
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

$("#new-profile").addEventListener("submit", async (e) => {
  e.preventDefault();
  const name = new FormData(e.target).get("name");
  const id = await run("create_profile", { game: state.game, name });
  e.target.reset();
  state.overview = await run("overview");
  await openProfile(id);
});

$("#game-path").addEventListener("submit", async (e) => {
  e.preventDefault();
  const path = new FormData(e.target).get("path");
  await run("set_game_path", { game: state.game, path });
  await refresh();
});

// Adding a game. Writes <config>/games/<id>.toml through create_game in
// main.rs; anything the form doesn't cover can be added to that file later.

const gameForm = $("#new-game");
/** Whether the id has been typed in, rather than filled in from the name. */
let idEdited = false;

const slug = (name) =>
  name.toLowerCase().normalize("NFKD").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

function openGameDialog() {
  gameForm.reset();
  idEdited = false;
  $("#new-game-error").hidden = true;
  $("#new-game-rulesets").replaceChildren(
    h("span", {}, "Install rules"),
    ...state.overview.rulesets.map((r) =>
      h(
        "label",
        { class: "row" },
        h("input", { type: "checkbox", name: "rulesets", value: r }),
        h("span", {}, r),
        r === "bepinex"
          ? h("small", { class: "muted" }, "most Thunderstore games")
          : null,
      ),
    ),
  );
  if (!state.overview.rulesets.length) {
    $("#new-game-rulesets").append(
      h("p", { class: "muted hint" }, "No shared rules are installed; files go in as they are."),
    );
  }
  $("#game-dialog").showModal();
}

gameForm.elements.name.addEventListener("input", (e) => {
  if (!idEdited) gameForm.elements.id.value = slug(e.target.value);
});
gameForm.elements.id.addEventListener("input", () => (idEdited = true));
gameForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#game-dialog").close();
});

gameForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(gameForm);
  const game = {
    id: form.get("id"),
    name: form.get("name"),
    install: form.get("install") || null,
    thunderstore: form.get("thunderstore") || null,
    nexus: form.get("nexus") || null,
    rulesets: form.getAll("rulesets"),
    link: form.get("link"),
  };
  const submit = gameForm.querySelector("[type=submit]");
  const existed = !!gameInfo(game.id);
  submit.disabled = true;
  let error = null;
  try {
    await invoke("create_game", { game });
  } catch (err) {
    error = String(err);
  } finally {
    submit.disabled = false;
  }
  state.overview = await run("overview");
  // The game can be saved even if its install folder couldn't be set, so
  // carry on to it with the error showing rather than leaving the dialog up.
  if (error && (existed || !gameInfo(game.id))) {
    $("#new-game-error").textContent = error;
    $("#new-game-error").hidden = false;
    $("#new-game-error").scrollIntoView({ block: "nearest" });
    return;
  }
  $("#game-dialog").close();
  await openGame(game.id);
  status(error ?? `Added ${game.name}`, !!error);
});

// Tabs

const SOURCE_NAMES = { thunderstore: "Thunderstore", nexus: "Nexus Mods" };
const sourceName = (s) => SOURCE_NAMES[s] ?? s.charAt(0).toUpperCase() + s.slice(1);

/** Opens the Mods tab ("mods") or a source's tab; source tabs all share #tab-browse. */
function selectTab(key) {
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
function renderSourceTabs(sources) {
  $("#tabs").replaceChildren(
    h("button", { role: "tab", onclick: () => selectTab("mods") }, "Mods"),
    ...sources.map((s) =>
      h("button", { role: "tab", "data-source": s, onclick: () => selectTab(s) }, sourceName(s)),
    ),
  );
  selectTab(sources.includes(state.source) ? state.source : "mods");
}

// Browse

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
              const name = await run("add_mod", {
                id: state.profile.id,
                source: item.source,
                modId: item.id,
              }).catch(() => (e.target.disabled = false));
              if (typeof name === "string") {
                status(`Added ${name}`);
                e.target.textContent = "Added";
                state.profile = await run("profile", { id: state.profile.id });
                renderProfile();
                refreshVersions();
                refreshSummaries();
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

// Deploy

$("#deploy").addEventListener("click", async () => {
  const id = state.profile.id;
  $("#deploy").disabled = true;
  status("Working out what to deploy…");
  try {
    const plan = await run("plan_deploy", { id });
    const changes = plan.ops.length;
    $("#plan-summary").textContent = changes
      ? `${changes} change${changes === 1 ? "" : "s"}, ${plan.unchanged} files already in place.`
      : `Nothing to do; ${plan.unchanged} files already in place.`;
    $("#plan-warnings").replaceChildren(...plan.warnings.map((w) => h("li", {}, w)));
    $("#plan-ops").replaceChildren(...plan.ops.map(opRow));
    $("#plan-dialog [value=apply]").disabled = !changes;
    status("Ready");
    $("#plan-dialog").showModal();
  } finally {
    $("#deploy").disabled = false;
  }
});

$("#plan-dialog").addEventListener("close", async () => {
  if ($("#plan-dialog").returnValue !== "apply") return;
  status("Deploying…");
  const files = await run("apply_plan");
  status(`Deployed ${files} files`);
  await openProfile(state.profile.id);
});

function opRow(op) {
  switch (op.op) {
    case "remove":
      return h("li", { class: "remove" }, `− ${op.dst}`);
    case "backup":
      return h("li", {}, `~ ${op.dst} (backing up original)`);
    case "place":
      return h("li", {}, `+ ${op.dst}  [${op.modName}]`);
  }
}

// Update. Relocks the profile at the newest versions, like `patchwork lock
// --update`; pinned mods stay where they are. Nothing changes in the game
// until the next deploy.

$("#update").addEventListener("click", async () => {
  const id = state.profile.id;
  $("#update").disabled = true;
  status("Checking for updates…");
  let changes = [];
  try {
    changes = await run("update_lock", { id });
    if (!changes.length) {
      status("Everything's up to date");
      return;
    }
    const count = changes.length;
    const summary = `${count} mod${count === 1 ? "" : "s"} changed; deploy to apply`;
    $("#update-summary").textContent = summary;
    $("#update-list").replaceChildren(
      ...changes.map((c) =>
        h("li", {}, `${c.name}  ${c.from ?? "(new)"} → ${c.to ?? "(no version)"}`),
      ),
    );
    status(summary);
    $("#update-dialog").showModal();
  } finally {
    await openProfile(id);
  }
  // mods that moved to a new version have new details too; fetch them even
  // if the cards aren't showing, so the cache doesn't keep the old ones
  const changed = new Set(changes.map((c) => c.name));
  const mods = state.profile.mods.filter(
    (m) => changed.has(m.name) && !summaryFetches.has(modKey(m)),
  );
  for (const m of mods) m.summaryStale = true;
  fetchSummaries(mods);
});

// Progress from long-running commands (adding, locking, deploying).

listen("patchwork://event", ({ payload: e }) => {
  switch (e.kind) {
    case "resolving":
      return status(`Resolving ${e.name}…`);
    case "fetching":
      return status(`Downloading ${e.name}…`);
    case "applying":
      return status(`Deploying ${e.dst}`);
    case "warning":
      return status(e.message, true);
  }
});

setModsView(modsView);
refresh().catch(() => {});
