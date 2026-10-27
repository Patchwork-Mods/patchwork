// Settings: the app-wide dialog for sources' settings (a Nexus API key, say),
// and the fields any setting gets, which game settings use for deployers
// too. Sources and deployers describe their own settings (see settings.rs in
// core), so nothing here knows about any one of them.

import { invoke, run, status } from "./state.js";
import { $, h, link, pickFolder } from "./dom.js";
import { renderComputers } from "./sync.js";

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
    if (value != null) input.value = value;
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
  const input = form.elements[setting.key];
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
/** The sources as last shown, to tell what's been changed on saving. */
let shown = [];

async function render() {
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
}

/** Asks a source what it makes of its settings, and says so under them. */
async function checkSource(id) {
  const line = form.querySelector(`[data-check="${id}"]`);
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
    showError(err);
    return;
  }
  status(`Removed the ${source.name} ${setting.label.toLowerCase()}`);
  await render();
}

function showError(err) {
  $("#settings-error").textContent = String(err);
  $("#settings-error").hidden = false;
}

export async function openSettings() {
  $("#settings-error").hidden = true;
  await Promise.all([render(), renderComputers()]);
  dialog.showModal();
}

$("#app-settings").addEventListener("click", () => openSettings().catch(() => {}));

form.querySelector("[value=cancel]").addEventListener("click", () => dialog.close());

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  $("#settings-error").hidden = true;
  const submit = form.querySelector("[type=submit]");
  submit.disabled = true;
  const notes = [];
  try {
    for (const source of shown) {
      for (const setting of source.settings) {
        const value = readSetting(form, setting);
        if (value === undefined || value === (setting.value ?? null)) continue;
        const origin = await invoke("set_source_setting", { source: source.id, key: setting.key, value });
        if (setting.kind === "secret" && origin?.from === "file") {
          notes.push(`There's no system keychain to keep the ${source.name} ${setting.label.toLowerCase()} in, so it's saved in settings.toml.`);
        }
      }
    }
  } catch (err) {
    showError(err);
    return;
  } finally {
    submit.disabled = false;
  }
  await render();
  status(notes.length ? notes.join(" ") : "Saved settings");
});
