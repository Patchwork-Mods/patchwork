// How the open profile's game is set up to run: what it picks for the
// game's [env] axes (its mod loader, say) and whether it's deployed to an
// instance of its own. Like the CLI's `env` and `instance`. Each change is
// saved as it's made, and can be undone (see autosave.js).

import { invoke, state, status, windowFixes } from "./state.js";
import { $, h } from "./dom.js";
import { openProfile, showProfile } from "./profile.js";
import { autosave } from "./autosave.js";

const dialog = $("#profile-settings-dialog");
const form = $("#profile-settings-form");
const saving = autosave(dialog);

/** Whether profile `p`'s game has anything to set up per profile. */
export const hasSetup = (p) => !!p && (p.setup.env.length > 0 || !!p.setup.instance);

/**
 * A short line on what profile `p` runs with, for under its name, like
 * "fabric · 1.21.1 · own folder". Values the game picks by default are
 * left out.
 */
export function setupSummary(p) {
  const parts = p.setup.env.filter((a) => a.picked).map((a) => a.picked);
  if (p.setup.instance?.on) parts.push("own folder");
  return parts.join(" · ");
}

/** The select or text field for one env axis, named after its id. */
function axisField(axis) {
  const id = `profile-env-${axis.id}`;
  const input = axis.choices.length
    ? h(
        "select",
        { id, name: axis.id },
        h("option", { value: "" }, axis.default ? `Default (${axis.default})` : "Default"),
        ...axis.choices.map((c) => h("option", { value: c, selected: c === axis.picked }, c)),
      )
    : h("input", {
        id,
        name: axis.id,
        autocomplete: "off",
        spellcheck: "false",
        // core's EnvDefs::VERSION, which stands in for the version found
        placeholder: axis.default ?? (axis.id === "version" ? "The installed game's" : "Not picked"),
      });
  // what's saved goes in as defaultValue (see autosave.js)
  if (!axis.choices.length) input.defaultValue = axis.picked ?? "";
  return h("div", { class: "field" }, h("label", { for: id }, axis.name), input);
}

/** The instance select's value for how profile `p` is set: "" for the game's default. */
function instanceValue(p) {
  const inst = p.setup.instance;
  if (!inst || inst.on === inst.default) return "";
  return inst.on ? "on" : "off";
}

/** Fills the form in for profile `p`, as it's saved. */
function fill(p) {
  $("#profile-settings-title").textContent = `${p.name} settings`;

  $("#profile-settings-env").hidden = !p.setup.env.length;
  $("#profile-settings-axes").replaceChildren(...p.setup.env.map(axisField));

  const inst = p.setup.instance;
  $("#profile-settings-instance").hidden = !inst;
  if (inst) {
    const select = form.elements.instance;
    select.options[0].textContent = `Default (${inst.default ? "a folder of its own" : "the game's folder"})`;
    select.value = instanceValue(p);
    $("#profile-settings-dir").textContent = `Its own folder is ${inst.dir}`;
  }
}

$("#profile-settings-open").addEventListener("click", () =>
  invoke("open_profile_folder", { profile: state.profile.id }).catch(() =>
    status("The profile's own folder isn't there yet; deploying it to its own folder makes it", true),
  ),
);

async function openSetup() {
  const p = state.profile;
  fill(p);
  saving.open({ key: p.id });
}

$("#profile-settings").addEventListener("click", () => openSetup().catch(() => {}));

// A blocker's Open profile settings (see state.js). What it was for is
// tried again once the settings are saved, by whoever does it.
windowFixes.profileSettings = async () => {
  await openSetup().catch(() => {});
  return false;
};

/**
 * Saves a change to profile `id` with `save()`, which resolves to the
 * profile as it is then, and shows it. If it fails, some of it may have
 * been saved, so the profile's loaded again.
 */
async function update(id, save) {
  let detail;
  try {
    detail = await save();
  } catch (err) {
    await openProfile(id).catch(() => {});
    if (state.profile?.id === id && dialog.open) await saving.refill(() => fill(state.profile));
    throw err;
  }
  if (!detail || state.profile?.id !== id) return;
  showProfile(detail);
  if (dialog.open) await saving.refill(() => fill(state.profile));
}

/** Where profile `id` is deployed: "" for the game's default, "on" or "off". */
const setInstance = (id, value) =>
  update(id, () => invoke("set_profile_instance", { id, on: value === "" ? null : value === "on" }));

/** Picks `value` (null for the default) for profile `id`'s env axis `axis`. */
function setAxis(id, axis, value) {
  status("Finding versions of the mods that suit…");
  return update(id, () => invoke("set_profile_env", { id, picks: { [axis]: value } }));
}

// Where it's deployed saves as it changes, which can be refused (while it's
// deployed); so do the picks, which can take a while, as relocking for
// them can mean downloading.
saving.watch(form, (field) => {
  const p = state.profile;
  const id = p.id;
  if (field.name === "instance") {
    return saving.change(saving.labelOf(field), async () => {
      const before = instanceValue(state.profile);
      const value = field.value;
      if (value === before) return null;
      await setInstance(id, value);
      return { undo: () => setInstance(id, before), redo: () => setInstance(id, value) };
    });
  }
  const axis = p.setup.env.find((a) => a.id === field.name);
  if (!axis) return false;
  return saving.change(saving.labelOf(field), async () => {
    const before = state.profile.setup.env.find((a) => a.id === axis.id)?.picked ?? null;
    const value = field.value.trim() || null;
    if (value === before) return null;
    await setAxis(id, axis.id, value);
    return { undo: () => setAxis(id, axis.id, before), redo: () => setAxis(id, axis.id, value) };
  });
});
