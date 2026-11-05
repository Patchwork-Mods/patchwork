// How the open profile's game is set up to run: what it picks for the
// game's [env] axes (its mod loader, say) and whether it's deployed to an
// instance of its own. Like the CLI's `env` and `instance`.

import { invoke, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { openProfile, showProfile } from "./profile.js";

const dialog = $("#profile-settings-dialog");
const form = $("#profile-settings-form");

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
  if (!axis.choices.length) input.value = axis.picked ?? "";
  return h("div", { class: "field" }, h("label", { for: id }, axis.name), input);
}

/** The instance select's value for how profile `p` is set: "" for the game's default. */
function instanceValue(p) {
  const inst = p.setup.instance;
  if (!inst || inst.on === inst.default) return "";
  return inst.on ? "on" : "off";
}

function showError(err) {
  $("#profile-settings-error").textContent = String(err);
  $("#profile-settings-error").hidden = !err;
}

async function openSetup() {
  const p = state.profile;
  form.reset();
  showError(null);
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

  dialog.showModal();
}

$("#profile-settings").addEventListener("click", () => openSetup().catch(() => {}));

form.querySelector("[value=cancel]").addEventListener("click", () => dialog.close());

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const p = state.profile;
  const submit = form.querySelector("[type=submit]");
  submit.disabled = true;
  showError(null);
  let detail = null;
  try {
    // Where it's deployed first, since that can refuse (while it's
    // deployed) and relocking for new picks can take a while.
    const instance = form.elements.instance.value;
    if (p.setup.instance && instance !== instanceValue(p)) {
      const on = instance === "" ? null : instance === "on";
      detail = await invoke("set_profile_instance", { id: p.id, on });
    }
    const picks = {};
    for (const axis of p.setup.env) {
      const value = form.elements[axis.id].value.trim() || null;
      if (value !== (axis.picked ?? null)) picks[axis.id] = value;
    }
    if (Object.keys(picks).length) {
      status("Finding versions of the mods that suit…");
      detail = await invoke("set_profile_env", { id: p.id, picks });
    }
  } catch (err) {
    showError(err);
    status("Couldn't save the profile's settings", true);
    // some of it may have been saved
    await openProfile(p.id).catch(() => {});
    return;
  } finally {
    submit.disabled = false;
  }
  dialog.close();
  if (detail) {
    showProfile(detail);
    status(`Saved ${p.label}'s settings`);
  }
});
