// The Plugins tab, for games like Skyrim: the order the game loads the
// plugins in the profile's mods (.esp, .esm and .esl files), apart from the
// mods' own order. Masters always load before the rest, so they're a list of
// their own, and each plugin can be switched off without disabling its mod.

import { $, h } from "./dom.js";
import { run, state, status } from "./state.js";
import { dragHandle } from "./reorder.js";
import { showProfile } from "./profile.js";

/** The latest load, so an older one finishing late is ignored. */
let loads = 0;

/** Loads and shows the open profile's plugins. */
export async function openPlugins() {
  const p = state.profile;
  if (!p?.hasPlugins) return;
  const seq = ++loads;
  let order;
  try {
    order = await run("load_order", { id: p.id });
  } catch {
    return;
  }
  if (seq === loads && state.profile?.id === p.id) renderPlugins(order);
}

const plural = (n, one, many) => (n === 1 ? one : many);

function renderPlugins(order) {
  state.plugins = order;
  const plugins = order?.plugins ?? [];
  const masters = plugins.filter((p) => p.master);
  const rest = plugins.filter((p) => !p.master);

  $("#plugin-problems").replaceChildren(
    ...(order?.warnings ?? []).map((w) => h("li", { class: "warning" }, w)),
  );
  const unlocked = order?.unlocked ?? [];
  $("#plugins-unlocked").hidden = !unlocked.length;
  $("#plugins-unlocked-text").textContent =
    unlocked.length === 1
      ? `${unlocked[0]} hasn't been downloaded yet, so its plugins aren't here.`
      : `${unlocked.length} mods haven't been downloaded yet, so their plugins aren't here.`;

  const implicit = order?.implicit ?? [];
  $("#plugins-implicit").hidden = !implicit.length;
  $("#plugins-implicit summary").textContent =
    `${implicit.length} of the game's own ${plural(implicit.length, "plugin loads", "plugins load")} first`;
  $("#plugins-implicit-list").textContent = implicit.join(", ");

  $("#plugin-masters-title").hidden = !masters.length;
  $("#plugin-masters").hidden = !masters.length;
  $("#plugin-masters").replaceChildren(...masters.map(pluginRow));
  $("#plugin-others-title").hidden = !rest.length;
  $("#plugin-others").hidden = !rest.length;
  $("#plugin-others").replaceChildren(...rest.map(pluginRow));
  $("#plugins-empty").hidden = !!plugins.length || !!unlocked.length;
}

/** A plugin's row: a handle, its name and mod, and a switch. */
function pluginRow(p) {
  const kind = p.light ? "Light" : p.master ? "Master" : null;
  const needs = p.masters.length
    ? `Needs ${p.masters.join(", ")}`
    : "Doesn't need any other plugins";
  return h(
    "li",
    { class: p.enabled ? "mod-row plugin-row" : "mod-row plugin-row disabled", "data-name": p.name },
    dragHandle(p, {
      save: savePlugins,
      title:
        "Drag to change when the game loads it. Plugins further down win when two change the same thing.",
    }),
    h(
      "div",
      { class: "row-head", title: needs },
      h(
        "div",
        { class: "row-title" },
        h(
          "span",
          { class: "mod-name" },
          p.name,
          kind && h("span", { class: "badge plugin-kind" }, kind),
        ),
        h("span", { class: "muted" }, `from ${p.modName}`),
      ),
    ),
    h(
      "label",
      { class: "switch", title: "The game doesn't load disabled plugins, though their files are still deployed" },
      h("input", {
        type: "checkbox",
        role: "switch",
        checked: p.enabled,
        "aria-label": `Load ${p.name}`,
        onchange: (e) => toggle(p.name, e.target.checked),
      }),
    ),
  );
}

/** The load order as the lists show it, switches and all. */
const shown = () =>
  [...$("#plugin-masters").children, ...$("#plugin-others").children].map((li) => ({
    name: li.dataset.name,
    enabled: li.querySelector("input[role=switch]").checked,
  }));

/** The load order as it was last saved. */
const saved = () => (state.plugins?.plugins ?? []).map((p) => ({ name: p.name, enabled: p.enabled }));

/** Saves the plugins dragged around in one of the two lists. */
async function savePlugins(_after, _before, name) {
  await save(shown(), `Moved ${name}`, saved());
}

async function toggle(name, enabled) {
  await save(shown(), `${enabled ? "Enabled" : "Disabled"} ${name}`, saved()).catch(() => {});
}

/** Saves `plugins` as the load order, offering to undo it back to `old`. */
async function save(plugins, message, old) {
  const { id } = state.profile;
  try {
    renderPlugins(await run("set_load_order", { id, plugins }));
    // for the deploy button and the revert button
    showProfile(await run("profile", { id }), { keepMods: true });
  } catch {
    if (state.profile?.id === id) await openPlugins();
    throw new Error("not saved");
  }
  status(message);
  const undo = $("#undo");
  undo.hidden = false;
  undo.onclick = async () => {
    undo.hidden = true;
    await run("set_load_order", { id, plugins: old });
    status(`Undid: ${message}`);
    if (state.profile?.id === id) showProfile(await run("profile", { id }), { keepMods: true });
  };
}

$("#plugins-lock").addEventListener("click", async (e) => {
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
