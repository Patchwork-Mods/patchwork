// Settings: the app-wide dialog, in tabs, for sources' settings (a Nexus API
// key, say), which program opens nxm:// links (links.js), linking computers (sync.js), background sync (daemon.js) and
// who the user plays as (setup.js), and the app's look (theme.js), each
// saved as it's changed and undone with the buttons by the title
// (autosave.js);
// and the fields any setting gets, which game settings use for deployers
// too. Sources and deployers describe their own settings (see settings.rs in
// core), so nothing here knows about any one of them.

import { invoke, run, status, windowFixes } from "./state.js";
import { $, h, link, pickFolder } from "./dom.js";
import { renderComputers } from "./sync.js";
import { renderDaemon } from "./daemon.js";
import { renderLinks } from "./links.js";
import { renderAppearance } from "./theme.js";
import { autosave } from "./autosave.js";

/** What to say under a field about where its value came from. */
function originNote(setting) {
  switch (setting.origin?.from) {
    case "env":
      return `Set by ${setting.origin.var}, which is used instead of anything saved here.`;
    case "keychain":
      return "Saved in the system keychain.";
    default:
      return null;
  }
}

/** How to show a value, for "Default (…)". */
function describe(setting, value) {
  if (setting.kind === "choice") {
    return setting.choices.find((c) => c.value === value)?.label ?? value;
  }
  if (setting.kind === "bool") return value ? "On" : "Off";
  return String(value);
}

/**
 * A labelled field for `setting`, with an input named after its key. Leaving
 * it empty (or on "Default") means it isn't set. `onClear` is for removing a
 * saved secret, which can't otherwise be emptied.
 */
export function settingField(setting, { idPrefix, onClear } = {}) {
  const id = `${idPrefix}-${setting.key}`;
  const fromEnv = setting.origin?.from === "env";
  const isSet = !!setting.origin;
  const value = setting.value;
  let input;
  if (setting.kind === "choice" || setting.kind === "bool") {
    const choices =
      setting.kind === "bool"
        ? [
            { value: "true", label: "On" },
            { value: "false", label: "Off" },
          ]
        : setting.choices;
    const fallback = setting.default != null ? `Default (${describe(setting, setting.default)})` : "Default";
    input = h(
      "select",
      { id, name: setting.key, disabled: fromEnv },
      h("option", { value: "" }, fallback),
      ...choices.map((c) => h("option", { value: c.value, selected: String(value) === c.value }, c.label)),
    );
  } else {
    input = h("input", {
      id,
      name: setting.key,
      type: setting.kind === "secret" ? "password" : "text",
      autocomplete: "off",
      spellcheck: "false",
      disabled: fromEnv,
      // secrets are never sent back, so all there is to show is that one's saved
      placeholder: fromEnv
        ? ""
        : setting.kind === "secret" && isSet
          ? "Saved; type a new one to replace it"
          : (setting.default ?? ""),
    });
    // defaultValue is what's saved, so value differs from it while there's
    // an edit that isn't saved yet
    if (value != null) input.defaultValue = value;
  }

  const controls = [input];
  if (setting.kind === "path") {
    controls.push(h("button", { type: "button", onclick: () => pickFolder(input) }, "Browse…"));
  }
  if (setting.kind === "secret" && isSet && !fromEnv && onClear) {
    controls.push(h("button", { type: "button", onclick: () => onClear(setting) }, "Remove"));
  }

  const notes = [setting.help, originNote(setting)].filter(Boolean).join(" ");
  return h(
    "div",
    { class: "field" },
    h("label", { for: id }, setting.label),
    controls.length > 1 ? h("div", { class: "row" }, ...controls) : input,
    notes || setting.link
      ? h("small", { class: "muted" }, notes, notes && setting.link ? " " : null, setting.link ? link(setting.link, "Learn more") : null)
      : null,
  );
}

/**
 * What `form`'s field for `setting` asks for: a value, `null` to clear it,
 * or `undefined` to leave it as it is. A secret's field is empty unless a
 * new one is typed in, so empty leaves it alone.
 */
export function readSetting(form, setting) {
  return readInput(form.elements[setting.key], setting);
}

/** What `input`, the field for `setting`, asks for; see readSetting. */
function readInput(input, setting) {
  if (!input || input.disabled) return undefined;
  const text = input.value.trim();
  if (setting.kind === "secret") return text ? text : undefined;
  if (!text) return null;
  if (setting.kind === "bool") return text === "true";
  return text;
}

// The settings dialog

const dialog = $("#settings-dialog");
const form = $("#settings-form");
/** Saving, undo and tabs for Settings. The other tabs' modules record
 * their changes in it too, with autosave(#settings-dialog). */
const saving = autosave(dialog);
/** The sources as last shown, to tell what's been changed on saving. */
let shown = [];

/** Shows the sources' settings, keeping focus and any typing not saved. */
function render() {
  return saving.refill(async () => {
    shown = await run("source_settings");
    const sections = shown.map((source) => {
      const check = h("p", { class: "muted hint", "data-check": source.id });
      return h(
        "fieldset",
        { class: "stack" },
        h("legend", {}, source.name),
        ...source.settings.map((s) =>
          settingField(s, { idPrefix: `setting-${source.id}`, onClear: (s) => clear(source, s) }),
        ),
        check,
      );
    });
    $("#settings-sources").replaceChildren(...sections);
    for (const source of shown) {
      if (source.settings.some((s) => s.origin)) checkSource(source.id);
    }
  });
}

/** Asks a source what it makes of its settings, and says so under them. */
async function checkSource(id) {
  const line = form.querySelector(`[data-check="${id}"]`);
  if (!line) return;
  line.classList.remove("error");
  line.textContent = "Checking…";
  try {
    line.textContent = (await invoke("check_source_settings", { source: id })) ?? "";
  } catch (err) {
    line.textContent = String(err);
    line.classList.add("error");
  }
}

async function clear(source, setting) {
  try {
    await invoke("set_source_setting", { source: source.id, key: setting.key, value: null });
  } catch (err) {
    saving.showError(err);
    return;
  }
  status(`Removed the ${source.name} ${setting.label.toLowerCase()}`);
  await render();
}

/** The source and setting `input` is the field for, if it's one. */
function settingOf(input) {
  for (const source of shown) {
    for (const setting of source.settings) {
      if (input.id === `setting-${source.id}-${setting.key}`) return { source, setting };
    }
  }
  return null;
}

/** Saves source `sourceId`'s setting `key` as `value`, and shows it. */
async function setSourceSetting(sourceId, key, value) {
  await invoke("set_source_setting", { source: sourceId, key, value });
  await render();
}

// Each source setting saves as it's changed. Secrets can't be undone, since
// what they were is never sent back to put back.
saving.watch($("#settings-sources"), (input) => {
  const found = settingOf(input);
  if (!found) return false;
  const { source, setting } = found;
  const name = `${source.name} ${setting.label.toLowerCase()}`;
  return saving.change(
    name,
    async () => {
      const value = readInput(input, setting);
      const before = setting.value ?? null;
      if (value === undefined || value === before) return null;
      const origin = await invoke("set_source_setting", { source: source.id, key: setting.key, value });
      // saved, so render() doesn't keep it as typing not saved
      if (setting.kind === "secret") input.value = "";
      else input.defaultValue = input.value;
      await render();
      const message =
        setting.kind === "secret" && origin?.from === "file"
          ? `There's no system keychain to keep the ${name} in, so it's saved in settings.toml.`
          : `Saved the ${name}`;
      if (setting.kind === "secret") return { message };
      return {
        message,
        undo: () => setSourceSetting(source.id, setting.key, before),
        redo: () => setSourceSetting(source.id, setting.key, value),
      };
    },
    { tab: "accounts" },
  );
});

/** Opens the dialog, on `tab` if given ("accounts", "computers", "daemon"
 * or "appearance"), otherwise where it was last left. It opens straight
 * away and each part fills in as it loads, since some are slow (asking
 * xdg-mime about nxm:// links can take a second or more). */
export async function openSettings(tab) {
  $("#links-state").textContent = "Checking…";
  $("#links-handle").hidden = true;
  saving.open({ tab });
  const loads = [render(), renderComputers(), renderDaemon(), renderLinks(), renderAppearance()];
  const failed = (await Promise.allSettled(loads)).find((r) => r.status === "rejected");
  if (failed) saving.showError(failed.reason);
}

$("#app-settings").addEventListener("click", () => openSettings().catch(() => {}));

// A blocker's Open Settings (see state.js), for a source that needs
// something set, like a Nexus API key: opens Settings at that source's
// fields, and once it's closed and what was typed is saved, tries again.
windowFixes.sourceSettings = async (fix) => {
  // Registered after autosave's own close handler, so the saves that
  // closing starts are queued by the time this runs.
  const closed = new Promise((resolve) =>
    dialog.addEventListener("close", () => resolve(saving.settled()), { once: true }),
  );
  await openSettings("accounts");
  const field = form.querySelector(`[id^="setting-${CSS.escape(fix.source)}-"]`);
  field?.scrollIntoView({ block: "center" });
  field?.focus();
  await closed;
  return true;
};
