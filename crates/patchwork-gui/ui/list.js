// A tab for each of the game's ordered lists, like Skyrim's plugins: things
// in the profile's mods that it puts in an order of their own, apart from the
// mods' order. Items of a lower tier always come before higher ones, so each
// tier is a list of its own, and items can be switched off without disabling
// their mod. Nothing here knows what a list is for; extensions say that.

import { $, h } from "./dom.js";
import { run, state, status } from "./state.js";
import { dragHandle } from "./reorder.js";
import { showProfile } from "./profile.js";

/** The tab key for list `id`. */
export const listTab = (id) => `list:${id}`;

/** The id of the list `tab` is for, or null if it isn't a list's. */
export const listOf = (tab) => (tab?.startsWith("list:") ? tab.slice("list:".length) : null);

/** The latest load, so an older one finishing late is ignored. */
let loads = 0;

/** Loads and shows the open profile's list `id`. */
export async function openList(id) {
  const p = state.profile;
  if (!p?.lists.some((l) => l.id === id)) return;
  const seq = ++loads;
  let list;
  try {
    list = await run("list", { id: p.id, list: id });
  } catch {
    return;
  }
  if (seq === loads && state.profile?.id === p.id && listOf(state.tab) === id) renderList(list);
}

/** Loads the open list again, if a list's tab is open. */
export function refreshList() {
  const id = listOf(state.tab);
  if (id) openList(id);
}

const plural = (n, one, many) => (n === 1 ? one : many);
const capitalized = (s) => s.charAt(0).toUpperCase() + s.slice(1);

function renderList(list) {
  state.list = list;
  $("#list-description").textContent = list.description ?? "";
  $("#list-description").hidden = !list.description;
  const actions = (state.profile?.actions ?? []).filter((a) => a.shownWith === list.id);
  $("#list-actions").hidden = !actions.length;
  $("#list-actions").replaceChildren(
    ...actions.map((a) =>
      h(
        "button",
        {
          type: "button",
          class: "list-action",
          "data-action": a.id,
          title: a.description ?? "",
          onclick: (e) => runAction(a, e.target),
        },
        a.title,
      ),
    ),
  );
  $("#list-problems").replaceChildren(
    ...list.problems.map((p) => h("li", { class: p.severity }, p.message)),
  );
  const unlocked = list.unlocked;
  $("#list-unlocked").hidden = !unlocked.length;
  $("#list-unlocked-text").textContent =
    unlocked.length === 1
      ? `${unlocked[0]} hasn't been downloaded yet, so what it has isn't here.`
      : `${unlocked.length} mods haven't been downloaded yet, so what they have isn't here.`;

  const fixed = list.fixed;
  $("#list-fixed").hidden = !fixed.length;
  $("#list-fixed summary").textContent =
    `${fixed.length} always ${plural(fixed.length, "comes", "come")} first`;
  $("#list-fixed-list").textContent = fixed.map((i) => i.name).join(", ");

  const tiers = [...new Set(list.items.map((i) => i.tier))].sort((a, b) => a - b);
  $("#list-tiers").replaceChildren(
    ...tiers.flatMap((tier) => {
      const title = list.tiers[tier];
      const items = list.items.filter((i) => i.tier === tier);
      return [
        ...(title ? [h("h3", { class: "list-tier" }, title)] : []),
        h("ul", { class: "mod-list list-items", "data-tier": tier }, ...items.map(itemRow)),
      ];
    }),
  );
  $("#list-empty").textContent =
    `None of this profile's mods have anything for ${list.title.toLowerCase()}.`;
  $("#list-empty").hidden = !!list.items.length || !!unlocked.length;
}

/** An item's row: a handle, its name, badges and mod, and a switch. */
function itemRow(i) {
  return h(
    "li",
    { class: i.enabled ? "mod-row list-row" : "mod-row list-row disabled", "data-name": i.name },
    dragHandle(i, {
      save: saveOrder,
      title: "Drag to change where it comes. Further down comes later.",
    }),
    h(
      "div",
      { class: "row-head", title: i.detail ?? "" },
      h(
        "div",
        { class: "row-title" },
        h(
          "span",
          { class: "mod-name" },
          i.name,
          ...i.badges.map((b) => h("span", { class: "badge list-badge" }, capitalized(b))),
        ),
        i.modName && h("span", { class: "muted" }, `from ${i.modName}`),
      ),
    ),
    h(
      "label",
      {
        class: "switch",
        title: i.canDisable
          ? "Turned off, its files are still deployed, but it's left out of the list"
          : "This one can't be turned off",
      },
      h("input", {
        type: "checkbox",
        role: "switch",
        checked: i.enabled,
        disabled: !i.canDisable,
        "aria-label": `Use ${i.name}`,
        onchange: (e) => toggle(i.name, e.target.checked),
      }),
    ),
  );
}

/** The order as the tab shows it, switches and all. */
const shown = () =>
  [...document.querySelectorAll("#list-tiers .list-items > li")].map((li) => ({
    name: li.dataset.name,
    enabled: li.querySelector("input[role=switch]").checked,
  }));

/** The order as it was last saved. */
const saved = () => (state.list?.items ?? []).map((i) => ({ name: i.name, enabled: i.enabled }));

/** Saves the items dragged around in one of the tiers. */
async function saveOrder(_after, _before, name) {
  await save(shown(), `Moved ${name}`, saved());
}

async function toggle(name, enabled) {
  await save(shown(), `${enabled ? "Turned on" : "Turned off"} ${name}`, saved()).catch(() => {});
}

/** Saves `entries` as the open list's order, offering to undo it back to `old`. */
async function save(entries, message, old) {
  const { id } = state.profile;
  const list = state.list.id;
  try {
    renderList(await run("set_list", { id, list, entries }));
    // for the deploy button and the revert button
    showProfile(await run("profile", { id }), { keepMods: true });
  } catch {
    if (state.profile?.id === id) await openList(list);
    throw new Error("not saved");
  }
  status(message);
  const undo = $("#undo");
  undo.hidden = false;
  undo.onclick = async () => {
    undo.hidden = true;
    await run("set_list", { id, list, entries: old });
    status(`Undid: ${message}`);
    if (state.profile?.id === id) showProfile(await run("profile", { id }), { keepMods: true });
  };
}

/** Runs `action` and saves what it changes, offering to undo it. */
async function runAction(action, button) {
  const { id } = state.profile;
  button.disabled = true;
  status(`${action.title}…`);
  let report;
  try {
    report = await run("run_action", { id, action: action.id });
  } catch {
    return;
  } finally {
    button.disabled = false;
  }
  const changed = report.lists.filter((l) => l.moved.length);
  const moved = changed.reduce((n, l) => n + l.moved.length, 0);
  const said = report.message ?? action.title;
  const message = changed.length
    ? `${said}; moved ${moved} ${plural(moved, "item", "items")}`
    : `${said}; it was already in order`;
  const warned = report.warnings.length ? ` (${report.warnings.join(" ")})` : "";
  if (state.profile?.id === id) {
    showProfile(await run("profile", { id }), { keepMods: true });
    refreshList();
  }
  status(message + warned);
  if (!changed.length) return;
  const undo = $("#undo");
  undo.hidden = false;
  undo.onclick = async () => {
    undo.hidden = true;
    for (const l of changed) await run("set_list", { id, list: l.id, entries: l.before });
    status(`Undid: ${action.title}`);
    if (state.profile?.id === id) {
      showProfile(await run("profile", { id }), { keepMods: true });
      refreshList();
    }
  };
}

$("#list-lock").addEventListener("click", async (e) => {
  const { id } = state.profile;
  e.target.disabled = true;
  status("Downloading mods…");
  try {
    showProfile(await run("lock_profile", { id }), { keepMods: true });
    status("Downloaded mods");
  } catch {
    // run() has said what went wrong
  } finally {
    e.target.disabled = false;
  }
});
