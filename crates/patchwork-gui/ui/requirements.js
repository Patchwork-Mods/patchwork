// Soft dependencies: what a mod lists that patchwork doesn't install with it,
// because its source says it's optional, or doesn't say whether it's needed
// (Nexus Mods' requirements are all like that). Required ones are installed
// with the mod and are in the Dependencies section instead.
//
// A mod with unsure ones the profile doesn't have gets a mark saying how
// many, which opens #requirements-dialog; an expanded list row lists them
// too. Each can be added, or ignored so it isn't pointed out any more.
// Optional ones are listed but never counted. They come from `softDependencies`
// on each mod in main.rs, which is empty until the mod's been locked.

import { $, h, link } from "./dom.js";
import { listen, run, state, status } from "./state.js";
import { showProfile } from "./profile.js";

/** What to call a soft dependency. */
const labelOf = (d) => d.name ?? d.summary?.name ?? d.key ?? d.url ?? "?";

/** The unsure ones the profile doesn't have and that aren't ignored. */
const unsure = (m) => (m.softDependencies ?? []).filter((d) => d.need === "unknown" && !d.installed && !d.ignored);

/** A mark saying how many unsure soft dependencies a mod is missing, or null. */
export function requirementsMark(m) {
  const missing = unsure(m);
  if (!missing.length) return null;
  const title =
    `Its page lists ${missing.map(labelOf).join(", ")} without saying whether ` +
    `${missing.length === 1 ? "it's" : "they're"} required. Click to add or ignore ${missing.length === 1 ? "it" : "them"}.`;
  return h(
    "button",
    {
      type: "button",
      class: "mod-requirements",
      title,
      "aria-label": title,
      onclick: (e) => {
        // it sits on a list row's head, which expands the row when clicked
        e.stopPropagation();
        showRequirements(m.name);
      },
    },
    `${missing.length} maybe needed`,
  );
}

/** A mod's soft dependencies as a list, for its expanded row and the dialog. */
export function requirementsList(m) {
  const soft = m.softDependencies ?? [];
  if (!soft.length) return null;
  return h("ul", { class: "requirements" }, soft.map((d) => requirementRow(m, d)));
}

function requirementRow(m, d) {
  const page = d.summary?.url ?? d.url;
  const need = d.need === "optional" ? "optional" : "may be required";
  const have = d.installed
    ? h("span", { class: "requirement-have" }, d.installed.toLowerCase() === labelOf(d).toLowerCase() ? "✓ in the profile" : `✓ as ${d.installed}`)
    : d.ignored
      ? h("span", { class: "muted" }, "ignored")
      : h("span", { class: d.need === "unknown" ? "requirement-missing" : "muted" }, "not in the profile");
  const actions = !d.installed && [
    !d.ignored &&
      d.key &&
      h(
        "button",
        { type: "button", title: `Add ${labelOf(d)} to the list`, onclick: (e) => addRequirement(d, e.target) },
        "Add",
      ),
    h(
      "button",
      {
        type: "button",
        title: d.ignored ? "Point it out again while it's missing" : "Stop pointing it out, because you've got it some other way or don't want it",
        onclick: (e) => setIgnored(m, d, !d.ignored, e.target),
      },
      d.ignored ? "Unignore" : "Ignore",
    ),
  ];
  return h(
    "li",
    { class: d.installed ? "have" : d.ignored ? "ignored" : "", "data-ident": d.ident },
    h(
      "div",
      { class: "requirement-head" },
      h("span", { class: "requirement-name" }, link(page, labelOf(d))),
      h("span", { class: `requirement-need ${d.need}` }, need),
      have,
      // the page's own note on it, kept to one short line
      d.notes && h("span", { class: "requirement-notes muted", title: d.notes }, d.notes),
      h("span", { class: "requirement-actions" }, actions),
    ),
  );
}

/** The mod named `name` in the open profile, as it is now. */
const modNamed = (name) => state.profile?.mods.find((m) => m.name === name);

let shownIn = null;

function showRequirements(name) {
  shownIn = name;
  fillDialog();
  const dialog = $("#requirements-dialog");
  if (!dialog.open) dialog.showModal();
}

function fillDialog() {
  const m = shownIn && modNamed(shownIn);
  if (!m) {
    $("#requirements-dialog").close();
    return;
  }
  $("#requirements-title").textContent = `What ${m.name} lists`;
  const soft = m.softDependencies ?? [];
  const unsureToo = soft.some((d) => d.need === "unknown");
  const optionalToo = soft.some((d) => d.need === "optional");
  const intro = [
    unsureToo &&
      "Its page lists the ones marked \u201cmay be required\u201d without saying whether they really are, so they weren't installed with it.",
    optionalToo && "Optional ones weren't installed either.",
    "Add the ones you want, and ignore the rest so they aren't pointed out.",
  ].filter(Boolean);
  $("#requirements-body").replaceChildren(h("p", { class: "muted" }, intro.join(" ")), requirementsList(m));
}

$("#requirements-dialog").addEventListener("close", () => {
  shownIn = null;
});

/** Shows `profile` once a change to it is done, and the dialog with it. */
function changed(profile) {
  if (state.profile?.id !== profile.id) return;
  showProfile(profile);
  if (shownIn) fillDialog();
}

async function addRequirement(d, button) {
  const [source, ...rest] = d.key.split(":");
  button.disabled = true;
  const { id } = state.profile;
  status(`Adding ${labelOf(d)}…`);
  try {
    const added = await run("add_mod", { id, source, modId: rest.join(":") });
    status(`Added ${added.name} to the list`);
    changed(added.profile);
  } catch {
    button.disabled = false;
  }
}

async function setIgnored(m, d, ignored, button) {
  button.disabled = true;
  try {
    const profile = await run("set_ignored", { id: state.profile.id, name: m.name, ident: d.ident, ignored });
    status(ignored ? `${m.name}: ignoring ${labelOf(d)}` : `${m.name}: pointing out ${labelOf(d)} again`);
    changed(profile);
  } catch {
    button.disabled = false;
  }
}

// Locking a mod for the first time says what it may need, once. It's kept
// above the mods, since the status bar moves on quickly, until dismissed or
// until the mod has nothing left to point out (each is added or ignored).

/** The mods that said what they may need, as { profile, name, missing }. */
let notices = [];

listen("patchwork://event", ({ payload: e }) => {
  if (e.kind !== "maybeNeeded") return;
  notices = notices.filter((n) => n.profile !== e.profile || n.name !== e.name);
  notices.push({ profile: e.profile, name: e.name, missing: e.missing });
  renderNotices();
});

/**
 * The names of what of `n.missing` is still to add or ignore. The event can
 * come before the profile shown has the mod or its soft dependencies, so
 * one it doesn't know of yet still counts.
 */
function stillMissing(n) {
  const soft = modNamed(n.name)?.softDependencies ?? [];
  return n.missing.flatMap((label) => {
    // the core's label, which doesn't look at the summary
    const d = soft.find((d) => (d.name ?? d.key ?? d.url) === label);
    if (!d) return [label];
    return d.installed || d.ignored ? [] : [labelOf(d)];
  });
}

/** Shows the open profile's notices, dropping the ones with nothing left. */
export function renderNotices() {
  const p = state.profile;
  const items = [];
  notices = notices.filter((n) => {
    if (n.profile !== p?.id) return true;
    const missing = stillMissing(n);
    if (!missing.length) return false;
    const close = h("button", { type: "button", class: "dismiss", "aria-label": "Dismiss", title: "Dismiss" }, "×");
    close.addEventListener("click", () => {
      notices = notices.filter((o) => o !== n);
      renderNotices();
    });
    const review = h(
      "button",
      { type: "button", class: "link", title: "Add or ignore them", onclick: () => showRequirements(n.name) },
      "Review",
    );
    items.push(h("li", {}, h("span", {}, h("strong", {}, n.name), ` may need ${missing.join(", ")}`), review, close));
    return true;
  });
  $("#mod-notices").replaceChildren(...items);
}
