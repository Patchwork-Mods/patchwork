// How the open profile's game is set up to run: what it picks for the
// game's [env] axes (its mod loader, say), whether it's deployed to an
// instance of its own, and setting the game up there. Also who the user
// plays as, on the Playing tab of Settings. Like the CLI's `env`,
// `instance`, `provision` and `account`.

import { invoke, run, state, status } from "./state.js";
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

/** Says whether the profile's instance is set up as it picks now. */
async function renderProvision(p) {
  const section = $("#profile-settings-provision");
  section.hidden = !p.setup.provisions;
  if (!p.setup.provisions) return;
  const line = $("#profile-settings-provisioned");
  line.textContent = "Checking…";
  const setup = await invoke("provisioned", { id: p.id }).catch((err) => {
    line.textContent = String(err);
    return undefined;
  });
  if (setup === undefined) return;
  line.textContent = setup
    ? `Set up with ${setup}. Setting up again checks for missing files.`
    : "Not set up yet for what this profile picks. It's set up when you play, or you can do it now.";
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
  await renderProvision(p);
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

$("#profile-settings-provision-now").addEventListener("click", async () => {
  const p = state.profile;
  const button = $("#profile-settings-provision-now");
  button.disabled = true;
  showError(null);
  $("#profile-settings-provisioned").textContent =
    "Setting up… The first time can take a few minutes, since it downloads the game.";
  status(`Setting up ${p.label}…`);
  try {
    const done = await invoke("provision_profile", { id: p.id });
    const fetched = done.downloaded
      ? `downloaded ${done.downloaded} of its ${done.files} files (${(done.bytes / 1e6).toFixed(1)} MB)`
      : "nothing needed downloading";
    status(`Set up ${p.label} with ${done.setup}: ${fetched}`);
  } catch (err) {
    showError(err);
    status(`Couldn't set up ${p.label}`, true);
  } finally {
    button.disabled = false;
    await renderProvision(p);
  }
});

// Playing: who the user plays as. Only offline names until signing in with
// Microsoft is supported.

function showAccountError(err) {
  $("#account-error").textContent = String(err);
  $("#account-error").hidden = !err;
}

function showAccount(account) {
  $("#account-state").textContent = account
    ? `Playing offline as ${account.name}.`
    : "No name picked yet; games Patchwork starts itself need one.";
  $("#account-forget").hidden = !account;
  $("#account-name").value = account?.name ?? "";
}

/** Fills in the Playing tab; settings.js calls this when Settings opens. */
export async function renderAccount() {
  showAccountError(null);
  showAccount(await run("account"));
}

async function saveAccount(name) {
  showAccountError(null);
  let account;
  try {
    account = await invoke("set_offline_account", { name });
  } catch (err) {
    showAccountError(err);
    return;
  }
  showAccount(account);
  status(account ? `Playing as ${account.name}` : "Forgot who you play as");
  // Play stops asking for a name
  if (state.profile) await openProfile(state.profile.id).catch(() => {});
}

// Not checked with a pattern on the field: it's in the settings form, where
// a bad name on a hidden tab would quietly stop sources' settings saving.
// Core says what's wrong with one instead.
$("#account-save").addEventListener("click", () => saveAccount($("#account-name").value.trim()));

$("#account-forget").addEventListener("click", () => saveAccount(null));

// Enter would otherwise submit the settings form, which saves sources'
// settings instead.
$("#account-name").addEventListener("keydown", (e) => {
  if (e.key !== "Enter") return;
  e.preventDefault();
  $("#account-save").click();
});
