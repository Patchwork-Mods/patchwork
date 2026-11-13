// The open profile's mods, as cards or list rows.

import { run, state, status } from "./state.js";
import {
  $,
  ago,
  chevronIcon,
  compact,
  h,
  initials,
  link,
  linkIcon,
  openExternal,
  tagIcon,
  trashIcon,
} from "./dom.js";
import { openProfile, refresh, showProfile } from "./profile.js";
import { isHidden, modQuery, modTags } from "./toolbar.js";
import { updateMod } from "./deploy.js";
import { showChangelog } from "./changelog.js";
import { showOptions } from "./form.js";
import { dragHandle, dragging } from "./reorder.js";
import { conflictMark } from "./conflicts.js";
import { describeVersions, gameVersionsOf, showGameVersions } from "./gameversions.js";
import { requirementsList, requirementsMark } from "./requirements.js";
import { settingsButton } from "./config.js";
import { ADD_TAB, selectTab } from "./browse.js";
import { go } from "./nav.js";

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

/**
 * "arranged" (the profile's own order, which the user rearranges) or "load"
 * (the order mods deploy in, with priorities applied); remembered between
 * runs. The two only differ once a mod has a priority.
 */
let modsOrder = "arranged";
try {
  if (localStorage.getItem("modsOrder") === "load") modsOrder = "load";
} catch {}

function setModsOrder(order) {
  modsOrder = order;
  try {
    localStorage.setItem("modsOrder", order);
  } catch {}
  if (state.profile) renderMods(state.profile.mods);
}

for (const button of document.querySelectorAll("#mods-order button")) {
  button.addEventListener("click", () => setModsOrder(button.dataset.order));
}

/** Whether the load order is showing: picked, and different from the arrangement. */
function showingLoadOrder(mods) {
  const differs = mods.some((m) => m.priority);
  $("#mods-order").hidden = !differs;
  const load = differs && modsOrder === "load";
  for (const button of document.querySelectorAll("#mods-order button")) {
    button.setAttribute("aria-pressed", String((button.dataset.order === "load") === load));
  }
  return load;
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

/** Marks a mod with choices nothing's been picked in yet, like an
 * installer's options, so it's deployed with the defaults. */
function optionsMark(m) {
  const unpicked = (m.options ?? []).filter((o) => !o.saved);
  if (!unpicked.length) return null;
  const what = unpicked.map((o) => o.title.toLowerCase()).join(" or ");
  const why = `Nothing's been picked in its ${what} yet, so it's deployed with the defaults`;
  return h("span", { class: "mod-options muted", title: why }, "default options");
}

/** A button for each set of choices the mod has, like its installer. */
function optionsButtons(m) {
  return (m.options ?? []).map((o) =>
    h(
      "button",
      {
        type: "button",
        title: `Pick the options in its ${o.title.toLowerCase()}`,
        onclick: () => showOptions(m, o),
      },
      o.title,
    ),
  );
}

const PRIORITY_TITLE =
  "Priority: mods with a higher one load later, so they win when two change the same file, wherever they are in the list. Mods with the same priority go by their place in it. Click to change it, or use the arrow keys.";

/** A priority with its sign, like "+2" or "−1". */
const signed = (n) => (n > 0 ? `+${n}` : n < 0 ? `−${-n}` : "0");

/** Reads a typed priority, allowing a "−" for a minus; null if it isn't a whole number. */
function parsePriority(text) {
  const t = text.trim().replace("−", "-");
  return /^[-+]?\d{1,9}$/.test(t) ? Number(t) : null;
}

/** Fills each priority chip on screen in again, from its mod. */
const chipFills = new WeakMap();
function refreshChips() {
  for (const chip of document.querySelectorAll(".mod-priority")) chipFills.get(chip)?.();
}

/**
 * Saves a mod's priority. It stays where it is, unless the load order is
 * showing, when the mods are drawn again (keeping the focus on its chip).
 */
async function savePriority(m, priority) {
  if ((m.priority ?? 0) === priority) return;
  const { id } = state.profile;
  let profile;
  try {
    profile = await run("set_priority", { id, name: m.name, priority });
  } catch {
    refreshChips();
    return;
  }
  if (state.profile?.id !== id) return;
  status(priority ? `${m.name} has priority ${signed(priority)}` : `${m.name}'s priority is back to 0`);
  const mods = state.profile.mods;
  const same =
    profile.mods.length === mods.length && profile.mods.every((n, i) => n.key === mods[i].key);
  if (!same || modsOrder === "load") {
    const focused = document.activeElement?.classList.contains("mod-priority");
    showProfile(profile);
    if (focused) {
      document
        .querySelector(`#mods [data-name="${CSS.escape(m.name)}"] .mod-priority`)
        ?.focus({ preventScroll: true });
    }
    return;
  }
  // the cards and rows on screen were made from these, so keep them up to date
  // (a priority of 0 isn't sent at all, so it's cleared first)
  profile.mods = profile.mods.map((n, i) => Object.assign(mods[i], { priority: 0 }, n));
  showProfile(profile, { keepMods: true });
  showingLoadOrder(profile.mods);
  refreshChips();
}

/** The priority that puts a mod after every other one, or before them. */
function extremePriority(m, last) {
  const others = (state.profile?.mods ?? []).filter((o) => o !== m).map((o) => o.priority ?? 0);
  const own = m.priority ?? 0;
  if (last) {
    const top = Math.max(0, ...others);
    return own > top ? own : top + 1;
  }
  const bottom = Math.min(0, ...others);
  return own < bottom ? own : bottom - 1;
}

/** The open priority popover, if any, and what closes it. */
let priorityPopover = null;

function closePriority({ save }) {
  const open = priorityPopover;
  if (!open) return;
  priorityPopover = null;
  open.el.remove();
  open.chip.classList.remove("open");
  open.chip.setAttribute("aria-expanded", "false");
  if (save) {
    const n = parsePriority(open.field.value);
    if (n != null) savePriority(open.m, n);
  }
}

/**
 * Opens the popover for changing a mod's priority under its chip: a field
 * with − and + either side, and buttons that load it last, first, or
 * reset it. It's put on the page itself, so a card's edges don't cut it off.
 * `typed` starts the field off with what was typed on the chip.
 */
function openPriority(chip, m, typed) {
  closePriority({ save: true });
  const field = h("input", {
    type: "text",
    inputmode: "numeric",
    class: "priority-field",
    value: typed ?? String(m.priority ?? 0),
    "aria-label": `Priority of ${m.name}`,
  });
  const step = (by) => {
    field.value = String((parsePriority(field.value) ?? m.priority ?? 0) + by);
    field.focus();
  };
  const choose = (n) => {
    field.value = String(n);
    closePriority({ save: true });
    chip.focus();
  };
  const button = (text, attrs, onclick) => h("button", { type: "button", ...attrs, onclick }, text);
  const el = h(
    "div",
    { class: "priority-popover", role: "dialog", "aria-label": `Priority of ${m.name}` },
    h(
      "div",
      { class: "priority-stepper" },
      button("−", { title: "Load it earlier", "aria-label": "Lower" }, () => step(-1)),
      field,
      button("+", { title: "Load it later", "aria-label": "Higher" }, () => step(1)),
    ),
    h("p", { class: "muted" }, "Higher loads later, and wins when two mods change the same file."),
    h(
      "div",
      { class: "priority-actions" },
      button("Load last", { title: "Higher than every other mod" }, () =>
        choose(extremePriority(m, true)),
      ),
      button("Load first", { title: "Lower than every other mod" }, () =>
        choose(extremePriority(m, false)),
      ),
      button("Reset", { title: "Back to 0, going by its place in the list" }, () => choose(0)),
    ),
  );
  field.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      closePriority({ save: true });
      chip.focus();
    } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      e.preventDefault();
      step(e.key === "ArrowUp" ? 1 : -1);
    }
  });
  el.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      closePriority({ save: false });
      chip.focus();
    }
  });
  // a click anywhere else saves it, as does leaving it by tabbing
  el.addEventListener("focusout", (e) => {
    if (priorityPopover?.el === el && e.relatedTarget && !el.contains(e.relatedTarget)) {
      closePriority({ save: true });
    }
  });
  priorityPopover = { el, chip, m, field };
  chip.classList.add("open");
  chip.setAttribute("aria-expanded", "true");
  document.body.append(el);
  // below the chip, or above it if there isn't room, kept on screen
  const at = chip.getBoundingClientRect();
  const { offsetWidth: w, offsetHeight: ht } = el;
  const left = Math.max(4, Math.min(at.right - w, innerWidth - w - 4));
  const top = at.bottom + 4 + ht > innerHeight ? Math.max(4, at.top - 4 - ht) : at.bottom + 4;
  el.style.left = `${left}px`;
  el.style.top = `${top}px`;
  field.focus();
  if (typed == null) field.select();
  else field.setSelectionRange(typed.length, typed.length);
}

/** The chip whose popover a press on it just closed, so the click that
 * follows doesn't open it again. */
let closedByChip = null;

document.addEventListener("mousedown", (e) => {
  closedByChip = null;
  if (priorityPopover && !priorityPopover.el.contains(e.target)) {
    if (priorityPopover.chip.contains(e.target)) closedByChip = priorityPopover.chip;
    closePriority({ save: true });
  }
});
// it'd be left behind on scrolling
document.addEventListener(
  "scroll",
  (e) => {
    if (priorityPopover && !priorityPopover.el.contains(e.target)) closePriority({ save: true });
  },
  true,
);

/** Two arrows, one up and one down, for a priority that's still 0. */
function priorityIcon() {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(ns, "path");
  path.setAttribute("d", "M8 4v16M4 8l4-4 4 4M16 20V4M12 16l4 4 4-4");
  svg.append(path);
  return svg;
}

/**
 * A mod's priority, as a small chip: a faint pair of arrows while it's 0
 * (only showing on hovering its row or card), and a pill saying "+2" or
 * "−1" once it's set. Clicking it opens a popover to change it; the arrow
 * keys change it directly (Page Up and Down by 10), Delete resets it, and
 * typing a number opens the popover with it.
 */
function priorityInput(m) {
  const chip = h("span", {
    class: "mod-priority",
    role: "spinbutton",
    tabindex: 0,
    title: PRIORITY_TITLE,
    "aria-label": `Priority of ${m.name}`,
    "aria-haspopup": "dialog",
    "aria-expanded": "false",
  });
  // what the arrow keys have it at, saved once they stop for a moment
  let pending = null;
  let timer = null;
  const show = (n) => {
    chip.classList.toggle("set", n !== 0);
    chip.replaceChildren(n ? signed(n) : priorityIcon());
    chip.setAttribute("aria-valuenow", String(n));
    chip.setAttribute(
      "aria-valuetext",
      n ? `${signed(n)}, loads ${n > 0 ? "later" : "earlier"}` : "0, by its place in the list",
    );
  };
  const fill = () => {
    if (pending == null) show(m.priority ?? 0);
  };
  const flush = () => {
    clearTimeout(timer);
    if (pending == null) return;
    const n = pending;
    pending = null;
    savePriority(m, n);
  };
  chipFills.set(chip, fill);
  fill();
  chip.addEventListener("click", (e) => {
    // it sits on a list row, which expands when clicked
    e.stopPropagation();
    if (closedByChip === chip) {
      closedByChip = null;
      return;
    }
    flush();
    openPriority(chip, m);
  });
  chip.addEventListener("keydown", (e) => {
    const steps = { ArrowUp: 1, ArrowDown: -1, PageUp: 10, PageDown: -10 };
    if (e.key in steps) {
      e.preventDefault();
      pending = (pending ?? m.priority ?? 0) + steps[e.key];
      show(pending);
      clearTimeout(timer);
      timer = setTimeout(flush, 600);
    } else if (e.key === "Delete" || e.key === "Backspace" || e.key === "Home") {
      e.preventDefault();
      pending = 0;
      show(0);
      flush();
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      flush();
      openPriority(chip, m);
    } else if (/^[-−\d]$/.test(e.key)) {
      e.preventDefault();
      flush();
      openPriority(chip, m, e.key);
    }
  });
  chip.addEventListener("blur", flush);
  return chip;
}

function modRow(m, { movable, onToggle, onRemove, onVersion }) {
  const { key } = m;
  const update = h("div", { class: "card-update" });
  fillUpdate(update, m, onVersion);
  const details = h("div", { class: "row-details" });
  // Most rows stay closed, so what they show when opened is only made then.
  details.hidden = true;
  const head = h("div", { class: "row-head" });
  let item;
  // unlike h(), replaceChildren() would show false and undefined as text
  const fillDetails = () => details.replaceChildren(...rowDetails(m, item, onRemove).filter(Boolean));
  const fill = () => {
    item = cardItem(m);
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
          optionsMark(m),
          requirementsMark(m),
        ),
        item.author && h("span", { class: "author" }, item.author),
      ),
    );
    if (!details.hidden) fillDetails();
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
    if (open && details.hidden) fillDetails();
    details.hidden = !open;
  };
  // clicking anywhere on the picture or name expands it too
  head.addEventListener("click", () => setExpanded(!expandedRows.has(key)));
  setExpanded(expandedRows.has(key));

  row.append(
    movable && dragHandle(m),
    head,
    update,
    // the chip is left of the cog, so while it's hidden its space runs into
    // the name's rather than parting the cog from the switch
    priorityInput(m),
    settingsButton(m),
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
  const count = (n, noun) =>
    n != null && h("span", { title: n.toLocaleString() }, `${compact(n)} ${noun}`);
  const facts = [
    item.updated &&
      h("span", { title: new Date(item.updated).toLocaleString() }, `Updated ${ago(item.updated)}`),
    count(item.downloads, "downloads"),
    count(item.rating, "likes"),
    h("span", {}, `${m.source}:${m.shownId}`),
    worksWith(m),
  ].filter(Boolean);
  const urlButton = (url, text) =>
    url && h("button", { type: "button", title: url, onclick: () => openExternal(url) }, text);
  return [
    summary && h("p", {}, summary),
    h(
      "div",
      { class: "facts muted" },
      tagsMark(m),
      tiesMark(m),
      facts.flatMap((f, i) => (i ? [" · ", f] : [f])),
    ),
    m.softDependencies?.length > 0 && h("h4", { class: "requirements-title" }, "It also lists"),
    requirementsList(m),
    h(
      "div",
      { class: "row-actions" },
      urlButton(item.url, "Website"),
      urlButton(item.author_url, "Author's mods"),
      m.versioned && h("button", { type: "button", onclick: () => showChangelog(m) }, "Changelog"),
      ...optionsButtons(m),
      h(
        "button",
        {
          type: "button",
          title: "Say which versions of the game it works with",
          onclick: () => showGameVersions(m),
        },
        "Game versions",
      ),
      h("button", { type: "button", class: "danger", onclick: onRemove }, "Remove"),
    ),
  ];
}

/** The list popped up by hovering or focusing a mod's tags or ties icon, if one is up. */
let markPopup = null;
const hidePopup = () => {
  markPopup?.remove();
  markPopup = null;
};
// it's placed once, so it goes rather than float off when anything scrolls
document.addEventListener("scroll", hidePopup, true);
// nor does it outlive its icon, say when the mods are drawn again under the pointer
document.addEventListener("pointerover", (e) => {
  if (markPopup && !e.target.closest?.(".tags-mark, .ties-mark")) hidePopup();
});

/**
 * A small icon (with class `name`-mark) that pops up `content()` (in a
 * `name`-popup) while hovered or focused. The popup is put on the page
 * itself, since a card clips what spills out of it.
 */
function hoverMark(name, label, icon, content) {
  const show = () => {
    hidePopup();
    markPopup = h("div", { class: `${name}-popup`, role: "tooltip" }, content());
    document.body.append(markPopup);
    const at = mark.getBoundingClientRect();
    const pop = markPopup.getBoundingClientRect();
    const centred = at.left + at.width / 2 - pop.width / 2;
    const left = Math.min(Math.max(8, centred), innerWidth - pop.width - 8);
    const below = at.bottom + 6;
    const top = below + pop.height > innerHeight - 8 ? at.top - pop.height - 6 : below;
    markPopup.style.left = `${left}px`;
    markPopup.style.top = `${Math.max(8, top)}px`;
  };
  const mark = h(
    "span",
    {
      class: `${name}-mark`,
      tabindex: "0",
      "aria-label": label,
      onpointerenter: show,
      onpointerleave: hidePopup,
      onfocus: show,
      onblur: hidePopup,
    },
    icon,
  );
  return mark;
}

/** A small tag icon that shows a mod's tags while hovered or focused, or nothing when it hasn't any. */
function tagsMark(m) {
  const tags = modTags(m);
  if (tags.length === 0) return null;
  return hoverMark("tags", `Tags: ${tags.join(", ")}`, tagIcon(), () =>
    h("div", { class: "tags" }, tags.map((t) => h("span", { class: "tag" }, t))),
  );
}

/** Which versions of the game a mod works with, if anything says. */
function worksWith(m) {
  const known = gameVersionsOf(m);
  if (!known) return null;
  const title = known.set ? "Set in the profile" : "Going by its source";
  return h("span", { title }, `Works with ${describeVersions(known.versions)}`);
}

/**
 * A small link icon that lists what a profile's mod needs, what needs it and
 * what it stands in for (from the lockfile) while hovered or focused, or
 * nothing when it has no ties.
 */
function tiesMark(m) {
  const deps = state.profile?.dependencies ?? [];
  const groups = [
    ["Needs", deps.filter((d) => d.neededBy.includes(m.name) && !d.replacedBy).map((d) => d.name)],
    ["Needed by", deps.find((d) => d.key === m.key)?.neededBy ?? []],
    ["Stands in for", deps.filter((d) => d.replacedBy === m.name).map((d) => d.name)],
  ].filter(([, names]) => names.length > 0);
  if (groups.length === 0) return null;
  const label = groups.map(([title, names]) => `${title} ${names.join(", ")}`).join("; ");
  return hoverMark("ties", label, linkIcon(), () =>
    groups.flatMap(([title, names]) => [
      h("h4", {}, `${title} (${names.length})`),
      h("ul", {}, [...names].sort((a, b) => a.localeCompare(b)).map((n) => h("li", {}, n))),
    ]),
  );
}

/** A mod's picture, or its initials when it hasn't got one. */
function modIcon(item) {
  return item.icon_url
    ? h("img", { src: item.icon_url, alt: "", loading: "lazy", decoding: "async" })
    : h("div", { class: "no-icon", "aria-hidden": "true" }, initials(item.name));
}

/** A mod's name, author, summary and counts, from a search result (a ModSummary). */
function cardBody(item) {
  // Hovering a count shows it in full.
  const count = (n, noun) =>
    n != null && h("span", { title: `${n.toLocaleString()} ${noun}` }, `${compact(n)} ${noun}`);
  const stats = [count(item.downloads, "downloads"), count(item.rating, "likes")].filter(Boolean);
  const summary = item.summary?.trim();
  return h(
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
        h("div", { title: new Date(item.updated).toLocaleString() }, `Updated ${ago(item.updated)}`),
    ),
  );
}

/** A mod's picture and details, from a search result (a ModSummary). */
export function cardMain(item) {
  return h("div", { class: "card-main" }, modIcon(item), cardBody(item));
}

/** What a profile's mod shows on its card: its summary, once that's been fetched. */
const cardItem = (m) => m.summary ?? { name: m.name, summary: `${m.source}:${m.shownId}` };

/** How much of a card's picture still shows while its sheet is raised, in pixels. */
function cardPeek(card) {
  const peek = parseFloat(getComputedStyle(card).getPropertyValue("--pw-card-peek"));
  return Number.isFinite(peek) ? peek : 40;
}

// An added mod's card: its picture, with a sheet under it holding its
// details, the on/off switch, priority and version. Hovering the card (or
// tabbing into it) slides the sheet up over the picture, leaving a strip of
// it showing, to bring up what hangs below: buttons for its settings,
// options, changelog and pages, and what it needs. The remove button and
// the handle to drag it by sit on the strip that still shows.
function modCard(m, { movable, onToggle, onRemove, onVersion }) {
  const cover = h("div", { class: "card-cover" });
  const body = h("div", { class: "card-body" });
  const more = h("div", { class: "card-more" });
  // made along with the rest of what's below the sheet: config.js only
  // keeps buttons that are on the page up to date
  let settings = null;
  // the tags come with the summary, so this fills in along with it
  const tags = h("span", { class: "card-tags" });
  let item;
  // Only made once the sheet is first raised (see measure below), since
  // most cards never are.
  let raised = false;
  const fillMore = () => {
    settings ??= settingsButton(m, { label: true });
    more.replaceChildren(...cardMore(m, item, settings).filter(Boolean));
  };
  const fill = () => {
    item = cardItem(m);
    tags.replaceChildren(...[tagsMark(m), tiesMark(m)].filter(Boolean));
    cover.replaceChildren(modIcon(item));
    body.replaceChildren(cardBody(item));
    if (raised) fillMore();
  };
  fill();
  summaryViews.set(m.key, fill);
  const update = h("div", { class: "card-update" });
  updateNotes.set(m.key, { el: update, onVersion });
  fillUpdate(update, m, onVersion);
  const sheet = h(
    "div",
    { class: "card-sheet" },
    body,
    h(
      "div",
      { class: "card-marks" },
      issueMark(m),
      conflictMark(m),
      unsharedMark(m),
      optionsMark(m),
      requirementsMark(m),
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
      h("div", { class: "card-footer-end" }, tags, priorityInput(m)),
      m.versioned
        ? versionSelect(m, onVersion)
        : h("span", { class: "mod-version muted" }, m.version ?? m.locked ?? "No versions"),
    ),
    more,
  );
  const card = h(
    "li",
    { class: m.enabled ? "result mod-card" : "result mod-card disabled", "data-name": m.name },
    cover,
    sheet,
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
  );
  // The sheet rises as far as what hangs below it needs, up to the strip
  // of picture left showing; CSS raises it while hovered or focused.
  const measure = () => {
    if (!raised) {
      raised = true;
      fillMore();
    }
    const room = Math.max(0, cover.offsetHeight - cardPeek(card));
    more.style.maxHeight = `${room}px`;
    card.style.setProperty("--raise", `${Math.min(more.scrollHeight, room)}px`);
  };
  card.addEventListener("pointerenter", measure);
  card.addEventListener("focusin", measure);
  return card;
}

/** What hangs below a card's sheet, brought up by raising it. */
function cardMore(m, item, settings) {
  const urlButton = (url, text, title) =>
    url &&
    h("button", { type: "button", title: `${title}: ${url}`, onclick: () => openExternal(url) }, text);
  const facts = [h("span", {}, `${m.source}:${m.shownId}`), worksWith(m)].filter(Boolean);
  return [
    h(
      "div",
      { class: "card-actions" },
      settings,
      m.versioned && h("button", { type: "button", onclick: () => showChangelog(m) }, "Changelog"),
      h(
        "button",
        {
          type: "button",
          title: "Say which versions of the game it works with",
          onclick: () => showGameVersions(m),
        },
        "Supports",
      ),
      urlButton(item.url, "Website", "Its page"),
      urlButton(item.author_url, "Author", "The author's mods"),
    ),
    h("div", { class: "facts muted" }, facts.map((f) => h("div", {}, f))),
  ];
}

/**
 * Says when a newer version than the one in use is out (`m.update`, which
 * the backend works out). A mod that follows the latest version gets a
 * button that updates it, as Update would. A pinned mod is pinned for a
 * reason, so it only gets a quiet note: clicking it pins the mod to the new
 * version, and × dismisses it until an even newer one is out (the backend
 * leaves out versions the user has seen; see `seen` in profile.rs).
 */
export function fillUpdate(el, m, onVersion) {
  const newest = m.update;
  const current = m.version ?? m.locked;
  el.classList.toggle("pinned", !!m.version);
  if (!newest) {
    el.replaceChildren();
    return;
  }
  if (m.version) {
    const dismiss = h(
      "button",
      {
        type: "button",
        class: "dismiss",
        "aria-label": `Dismiss ${newest}`,
        title: `Stop pointing out ${newest}; only a newer version will be`,
        onclick: async () => {
          dismiss.disabled = true;
          try {
            const profile = await run("dismiss_update", { id: state.profile.id, name: m.name, version: newest });
            Object.assign(m, profile.mods.find((n) => n.key === m.key));
            fillUpdate(el, m, onVersion);
          } catch {
            dismiss.disabled = false;
          }
        },
      },
      "×",
    );
    el.replaceChildren(
      h(
        "button",
        {
          type: "button",
          title: `Pinned to ${current}; click to pin to ${newest} instead`,
          onclick: () => onVersion?.(newest),
        },
        `${newest} available`,
      ),
      dismiss,
    );
    return;
  }
  const button = h(
    "button",
    {
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
  // Until it's used it only has the version showing: every version of
  // every mod was most of the elements on the page.
  const fillAll = () => {
    if (filledSelects.has(select)) return;
    filledSelects.add(select);
    fillVersions(select, m);
  };
  select.addEventListener("focus", fillAll);
  select.addEventListener("pointerdown", fillAll);
  fillVersions(select, m);
  versionSelects.set(m.key, select);
  return select;
}

/** Version dropdowns with every version in; see versionSelect(). */
const filledSelects = new WeakSet();

export function fillVersions(select, m) {
  // keep whatever's showing, which may be a choice that's still being saved
  const current = select.options.length ? select.value : (m.version ?? "");
  const versions = m.versions ?? [];
  const auto = !m.version && m.locked ? `Auto (${m.locked})` : "Auto";
  const listed = filledSelects.has(select) ? versions : versions.filter((v) => v === current);
  const options = [h("option", { value: "" }, auto), ...listed.map((v) => h("option", { value: v }, v))];
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
  // (a priority of 0 isn't sent at all, so it's cleared first)
  profile.mods = profile.mods.map((n, i) => Object.assign(mods[i], { priority: 0 }, n));
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
  const loadOrder = showingLoadOrder(mods);
  const shown = query
    ? await findMods(id, mods, query).catch(() => null)
    : loadOrder
      ? [...mods].sort((a, b) => a.loadIndex - b.loadIndex)
      : mods;
  if (!shown || render !== renders) return;
  versionSelects.clear();
  summaryViews.clear();
  updateNotes.clear();
  const view = modsView === "cards" ? modCard : modRow;
  const item = (m) =>
    view(m, {
      // search results come best match first, and the load order isn't
      // what's rearranged
      movable: !query && !loadOrder,
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
  $("#mods-empty").replaceChildren(
    ...(mods.length
      ? [`No mods match “${query}”.`]
      : [
          "No mods yet. ",
          h("button", { type: "button", class: "link", onclick: () => go(() => selectTab(ADD_TAB)) }, "Add some"),
        ]),
  );
  $("#filter-menu > summary").textContent = state.profile.hiddenTags.length
    ? `Filter (${state.profile.hiddenTags.length})`
    : "Filter";
  renderDependencies(query);
}

// The Dependencies section: mods the profile's mods brought in, which are
// only in the lockfile, and the ones replaced by a mod in the list.

try {
  $("#dependencies").open = localStorage.getItem("dependenciesOpen") === "true";
} catch {}
$("#dependencies").addEventListener("toggle", () => {
  try {
    localStorage.setItem("dependenciesOpen", String($("#dependencies").open));
  } catch {}
});

// a row's Replace menu closes when you click elsewhere
document.addEventListener("click", (e) => {
  for (const menu of document.querySelectorAll("#dependencies-list details.menu[open]")) {
    if (!menu.contains(e.target)) menu.open = false;
  }
});

/**
 * Lists the dependencies, the ones whose names match `query` if there is
 * one, counting only those; with none matching, the section is hidden.
 */
function renderDependencies(query) {
  const all = (state.profile.dependencies ?? []).filter((d) => !d.inProfile);
  const words = (query ?? "").toLowerCase();
  const shown = words ? all.filter((d) => d.name.toLowerCase().includes(words)) : all;
  const replaced = shown.filter((d) => d.replacedBy).length;
  $("#dependencies").hidden = !shown.length;
  $("#dependencies > summary").textContent =
    `Dependencies (${shown.length - replaced})` + (replaced ? ` · ${replaced} replaced` : "");
  $("#dependencies-list").replaceChildren(...shown.map(dependencyRow));
}

function dependencyRow(d) {
  const head = h("div", { class: "row-head" });
  const fill = () => {
    const item = d.summary ?? { name: d.name };
    head.replaceChildren(
      modIcon(item),
      h(
        "div",
        { class: "row-title" },
        h("span", { class: "mod-name", title: d.key }, item.name),
        h(
          "span",
          { class: "needed-by" },
          d.neededBy.length ? `Needed by ${d.neededBy.join(", ")}` : "Nothing needs it any more",
        ),
      ),
    );
  };
  fill();
  summaryViews.set(d.key, fill);
  const actions = d.replacedBy
    ? [
        h("span", { class: "muted" }, `Replaced by ${d.replacedBy}`),
        h(
          "button",
          {
            type: "button",
            title: `Have the mods that need it use ${d.name} again`,
            onclick: (e) => stopReplacing(d, e.target),
          },
          "Stop replacing",
        ),
      ]
    : [
        h("span", { class: "mod-version muted" }, d.version ?? ""),
        h(
          "button",
          {
            type: "button",
            title: "Add it to the list, so you can pick its version or turn it off",
            onclick: (e) => addDependency(d, e.target),
          },
          "Add to list",
        ),
        replaceMenu(d),
      ];
  return h(
    "li",
    { class: d.replacedBy ? "mod-row dependency-row replaced" : "mod-row dependency-row", "data-key": d.key },
    head,
    ...actions,
  );
}

/** A menu of the profile's mods, to pick one to stand in for `d`. */
function replaceMenu(d) {
  const menu = h(
    "details",
    { class: "menu" },
    h("summary", { title: "Use a mod from the list in its place, like a fork of it" }, "Replace with"),
  );
  const items = h("div", { class: "menu-items" });
  for (const m of state.profile.mods) {
    items.append(
      h(
        "button",
        {
          type: "button",
          onclick: () => {
            menu.open = false;
            replaceWith(d, m).catch(() => {});
          },
        },
        m.name,
      ),
    );
  }
  menu.append(items);
  return menu;
}

/** Sets what mod `m` stands in for, and shows the profile it's locked with. */
async function setReplaces(m, replaces, doing, done) {
  const { id } = state.profile;
  status(doing);
  const profile = await run("set_replaces", { id, name: m.name, replaces });
  status(done);
  if (state.profile?.id === id) showProfile(profile);
}

function replaceWith(d, m) {
  return setReplaces(
    m,
    [...(m.replaces ?? []), d.key],
    `Replacing ${d.name} with ${m.name}…`,
    `${m.name} stands in for ${d.name}`,
  );
}

async function stopReplacing(d, button) {
  const m = state.profile.mods.find((m) => m.name === d.replacedBy);
  if (!m) return;
  button.disabled = true;
  try {
    await setReplaces(
      m,
      (m.replaces ?? []).filter((k) => k !== d.key),
      `Bringing back ${d.name}…`,
      `Mods that need ${d.name} use it again`,
    );
  } catch {
    button.disabled = false;
  }
}

/** Adds a dependency to the profile's own mods. */
async function addDependency(d, button) {
  button.disabled = true;
  const { id } = state.profile;
  status(`Adding ${d.name}…`);
  try {
    const added = await run("add_mod", { id, source: d.source, modId: d.id });
    status(`Added ${added.name} to the list`);
    if (state.profile?.id === id) showProfile(added.profile);
  } catch {
    button.disabled = false;
  }
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
