// The open profile's mods, as cards or list rows.

import { run, state, status } from "./state.js";
import { $, ago, chevronIcon, compact, h, initials, link, openExternal, trashIcon } from "./dom.js";
import { openProfile, refresh, showProfile } from "./profile.js";
import { isHidden, modQuery, modTags } from "./toolbar.js";
import { updateMod } from "./deploy.js";
import { showChangelog } from "./changelog.js";
import { showInstaller } from "./installer.js";
import { dragHandle, dragging } from "./reorder.js";
import { conflictMark } from "./conflicts.js";

// The Mods tab shows mods either as cards, like a source's search results,
// or as a denser list of rows that each expand to show the rest.

/** "cards" or "list"; remembered between runs. */
export let modsView = "cards";
try {
  if (localStorage.getItem("modsView") === "list") modsView = "list";
} catch {}

export function setModsView(view) {
  modsView = view;
  try {
    localStorage.setItem("modsView", view);
  } catch {}
  for (const button of document.querySelectorAll("#mods-view button")) {
    button.setAttribute("aria-pressed", String(button.dataset.view === view));
  }
  if (state.profile) renderMods(state.profile.mods);
}

for (const button of document.querySelectorAll("#mods-view button")) {
  button.addEventListener("click", () => setModsView(button.dataset.view));
}

/** The mods whose list rows are expanded, by key, so re-renders keep them open. */
const expandedRows = new Set();

// An added mod's list row: a handle to drag it by, its picture, name and
// author, an on/off switch and its version, and an arrow that expands it to
// show its description, when it was updated, links to its pages, its
// changelog and a remove button.
/** A warning sign on a mod with issues (see Patchwork::issues), saying what
 * they are when hovered. */
function issueMark(m) {
  const issues = (state.profile?.issues ?? []).filter((i) => i.mods.includes(m.name));
  if (!issues.length) return null;
  const messages = issues.map((i) => i.message).join("\n");
  return h("span", { class: "mod-issue", title: messages, "aria-label": messages }, "⚠");
}

/** Marks a mod added here to a profile this computer follows, which isn't
 * synced: whoever shares the profile doesn't get it. */
function unsharedMark(m) {
  if (!(state.profile?.unshared ?? []).includes(m.name)) return null;
  const why = "Added on this computer; it isn't synced with whoever shares this profile";
  return h("span", { class: "mod-unshared muted", title: why }, "only here");
}

/** Marks a mod whose installer's options haven't been picked, so it's
 * deployed with the ones it recommends. */
function installerMark(m) {
  if (!m.installer || m.fomod) return null;
  const why =
    "Its installer's options haven't been picked, so it's deployed with the ones it recommends";
  return h("span", { class: "mod-installer muted", title: why }, "recommended options");
}

/** Opens the mod's installer, for mods that have one. */
function installerButton(m) {
  return (
    m.installer &&
    h(
      "button",
      {
        type: "button",
        title: "Pick the options its installer offers",
        onclick: () => showInstaller(m),
      },
      "Installer",
    )
  );
}

function modRow(m, { movable, onToggle, onRemove, onVersion }) {
  const { key } = m;
  const update = h("div", { class: "card-update" });
  fillUpdate(update, m, onVersion);
  const details = h("div", { class: "row-details" });
  const head = h("div", { class: "row-head" });
  const fill = () => {
    const item = cardItem(m);
    head.replaceChildren(
      modIcon(item),
      h(
        "div",
        { class: "row-title" },
        h(
          "span",
          { class: "mod-name" },
          item.name,
          issueMark(m),
          conflictMark(m),
          unsharedMark(m),
          installerMark(m),
        ),
        item.author && h("span", { class: "author" }, item.author),
      ),
    );
    details.replaceChildren(...rowDetails(m, item, onRemove));
  };
  fill();
  summaryViews.set(key, fill);
  updateNotes.set(key, { el: update, onVersion });

  const row = h("li", { class: m.enabled ? "mod-row" : "mod-row disabled", "data-name": m.name });
  const arrow = h(
    "button",
    {
      class: "expand",
      title: "More about this mod",
      "aria-label": `More about ${m.name}`,
      onclick: () => setExpanded(!expandedRows.has(key)),
    },
    chevronIcon(),
  );
  const setExpanded = (open) => {
    if (open) expandedRows.add(key);
    else expandedRows.delete(key);
    row.classList.toggle("expanded", open);
    arrow.setAttribute("aria-expanded", String(open));
    details.hidden = !open;
  };
  // clicking anywhere on the picture or name expands it too
  head.addEventListener("click", () => setExpanded(!expandedRows.has(key)));
  setExpanded(expandedRows.has(key));

  row.append(
    movable && dragHandle(m),
    head,
    update,
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
    ),
    m.versioned
      ? versionSelect(m, onVersion)
      : h("span", { class: "mod-version muted" }, m.version ?? m.locked ?? "No versions"),
    arrow,
    details,
  );
  return row;
}

/** What an expanded list row shows below its first line. */
function rowDetails(m, item, onRemove) {
  const summary = m.summary?.summary?.trim();
  const tags = modTags(m);
  const count = (n, noun) =>
    n != null && h("span", { title: n.toLocaleString() }, `${compact(n)} ${noun}`);
  const facts = [
    item.updated &&
      h("span", { title: new Date(item.updated).toLocaleString() }, `Updated ${ago(item.updated)}`),
    count(item.downloads, "downloads"),
    count(item.rating, "likes"),
    h("span", {}, `${m.source}:${m.shownId}`),
  ].filter(Boolean);
  const urlButton = (url, text) =>
    url && h("button", { type: "button", title: url, onclick: () => openExternal(url) }, text);
  return [
    summary && h("p", {}, summary),
    h("div", { class: "facts muted" }, facts.flatMap((f, i) => (i ? [" · ", f] : [f]))),
    tags.length > 0 && h("div", { class: "tags" }, tags.map((t) => h("span", { class: "tag" }, t))),
    h(
      "div",
      { class: "row-actions" },
      urlButton(item.url, "Website"),
      urlButton(item.author_url, "Author's mods"),
      m.versioned && h("button", { type: "button", onclick: () => showChangelog(m) }, "Changelog"),
      installerButton(m),
      h("button", { type: "button", class: "danger", onclick: onRemove }, "Remove"),
    ),
  ];
}

/** A mod's picture, or its initials when it hasn't got one. */
function modIcon(item) {
  return item.icon_url
    ? h("img", { src: item.icon_url, alt: "", loading: "lazy", decoding: "async" })
    : h("div", { class: "no-icon", "aria-hidden": "true" }, initials(item.name));
}

/** A mod's picture and details, from a search result (a ModSummary). */
export function cardMain(item) {
  // Hovering a count shows it in full.
  const count = (n, noun) =>
    n != null && h("span", { title: `${n.toLocaleString()} ${noun}` }, `${compact(n)} ${noun}`);
  const stats = [count(item.downloads, "downloads"), count(item.rating, "likes")].filter(Boolean);
  const summary = item.summary?.trim();
  return h(
    "div",
    { class: "card-main" },
    modIcon(item),
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
const cardItem = (m) => m.summary ?? { name: m.name, summary: `${m.source}:${m.shownId}` };

// An added mod's card: an on/off switch and its version along the bottom,
// a note when there's a newer version, and a remove button and a handle to
// drag it by that only show while the card is hovered or focused.
function modCard(m, { movable, onToggle, onRemove, onVersion }) {
  let main = cardMain(cardItem(m));
  const update = h("div", { class: "card-update" });
  summaryViews.set(m.key, () => {
    const fresh = cardMain(cardItem(m));
    main.replaceWith(fresh);
    main = fresh;
  });
  updateNotes.set(m.key, { el: update, onVersion });
  fillUpdate(update, m, onVersion);
  return h(
    "li",
    { class: m.enabled ? "result" : "result disabled", "data-name": m.name },
    main,
    issueMark(m),
    conflictMark(m),
    unsharedMark(m),
    movable && dragHandle(m),
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
      installerButton(m),
      m.versioned
        ? versionSelect(m, onVersion)
        : h("span", { class: "mod-version muted" }, m.version ?? m.locked ?? "No versions"),
    ),
  );
}

/**
 * Says when a newer version than the one in use is out, as a button that
 * moves the mod to it: a pinned mod gets pinned to it, and one that follows
 * the latest version is updated on its own, as Update would.
 */
export function fillUpdate(el, m, onVersion) {
  const newest = m.versions?.[0];
  const current = m.version ?? m.locked;
  if (!m.versioned || !newest || !current || newest === current) {
    el.replaceChildren();
    return;
  }
  const button = h(
    "button",
    m.version
      ? {
          title: `Pinned to ${current}; click to pin to ${newest} instead`,
          onclick: () => onVersion?.(newest),
        }
      : {
          title: `Click to update to ${newest}`,
          onclick: async () => {
            button.disabled = true;
            try {
              await updateMod(m);
            } finally {
              button.disabled = false;
            }
          },
        },
    `↑ ${newest}`,
  );
  el.replaceChildren(button);
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

// Each mod on screen, by its key (`<source>:<id>`), so what details.js
// gets for it can be put straight in.

/** The version dropdown of each mod on screen. */
export const versionSelects = new Map();
/** Redraws the picture and details of each mod on screen from its summary. */
export const summaryViews = new Map();
/** The newer-version note of each mod on screen. */
export const updateNotes = new Map();

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
  versionSelects.set(m.key, select);
  return select;
}

export function fillVersions(select, m) {
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

/** The mods in the Hidden section, as last rendered. */
let hiddenMods = [];

function renderHiddenCount() {
  const off = hiddenMods.filter((m) => !m.enabled).length;
  $("#hidden-mods > summary").textContent =
    `Hidden (${hiddenMods.length})` + (off ? ` · ${off} disabled` : "");
}

/**
 * Turns a mod on or off from its switch. Only its card or row changes, so the
 * rest aren't rebuilt (which makes their pictures blink), and the page is
 * scrolled to keep the switch where it was, as a problem can show up above.
 */
async function toggleMod(m, input) {
  const { id } = state.profile;
  const enabled = input.checked;
  const item = input.closest("li");
  const top = item.getBoundingClientRect().top;
  let profile;
  try {
    profile = await run("set_enabled", { id, name: m.name, enabled });
  } catch {
    input.checked = !enabled;
    return;
  }
  if (state.profile?.id !== id) return;
  const mods = state.profile.mods;
  const same =
    profile.mods.length === mods.length && profile.mods.every((n, i) => n.key === mods[i].key);
  if (!same) {
    showProfile(profile);
    return;
  }
  // the cards and rows on screen were made from these, so keep them up to date
  profile.mods = profile.mods.map((n, i) => Object.assign(mods[i], n));
  item.classList.toggle("disabled", !enabled);
  const onOff = input.parentElement.querySelector("span");
  if (onOff) onOff.textContent = enabled ? "On" : "Off";
  showProfile(profile, { keepMods: true });
  renderHiddenCount();
  if (item.isConnected) $("#tab-mods").scrollTop += item.getBoundingClientRect().top - top;
}

/** Counts renderMods() calls, so one still waiting on a search gives way to a later one. */
let renders = 0;

export async function renderMods(mods) {
  const id = state.profile.id;
  const render = ++renders;
  const query = modQuery();
  const shown = query ? await findMods(id, mods, query).catch(() => null) : mods;
  if (!shown || render !== renders) return;
  versionSelects.clear();
  summaryViews.clear();
  updateNotes.clear();
  const view = modsView === "cards" ? modCard : modRow;
  const item = (m) =>
    view(m, {
      // search results come best match first, not in load order
      movable: !query,
      onToggle: (e) => toggleMod(m, e.target),
      onRemove: async () => {
        if (!(await confirmRemove(m))) return;
        await run("remove_mod", { id, name: m.name });
        status(`Removed ${m.name}`);
        await refresh();
      },
      onVersion: async (version) => {
        status(version ? `Pinning ${m.name} to ${version}…` : `Unpinning ${m.name}…`);
        let profile;
        try {
          profile = await run("set_version", { id, name: m.name, version });
          status(version ? `Pinned ${m.name} to ${version}` : `${m.name} follows the latest version`);
        } finally {
          // put the dropdown back if it couldn't be saved
          if (profile) showProfile(profile);
          else await openProfile(id);
        }
      },
    });
  hiddenMods = shown.filter(isHidden);
  const layout = modsView === "cards" ? "result-grid" : "mod-list";
  $("#mods").className = layout;
  $("#mods").replaceChildren(...shown.filter((m) => !isHidden(m)).map(item));
  $("#hidden-mods-list").className = layout;
  $("#hidden-mods-list").replaceChildren(...hiddenMods.map(item));
  $("#hidden-mods").hidden = !hiddenMods.length;
  renderHiddenCount();
  $("#mods-empty").hidden = shown.length > 0;
  $("#mods-empty").textContent = mods.length
    ? `No mods match “${query}”.`
    : "No mods yet. Find some in the tabs above.";
  $("#filter-menu > summary").textContent = state.profile.hiddenTags.length
    ? `Filter (${state.profile.hiddenTags.length})`
    : "Filter";
}

/**
 * The mods matching `query`, best first, with ties kept in load order; see
 * matching.rs. A mod's author and description are only searched once its
 * summary is in.
 */
async function findMods(id, mods, query) {
  const keys = await run("find_mods", { id, query });
  const byKey = new Map(mods.map((m) => [m.key, m]));
  return keys.map((key) => byKey.get(key)).filter(Boolean);
}

/**
 * Renders the mods again on the next frame, once for any number of calls,
 * or once a mod being dragged has been let go.
 */
let rerender = null;
export function renderModsSoon() {
  rerender ??= requestAnimationFrame(() => {
    rerender = null;
    if (dragging()) renderModsSoon();
    else if (state.profile) renderMods(state.profile.mods);
  });
}
