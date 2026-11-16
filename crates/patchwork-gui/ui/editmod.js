// The Edit dialog: how one of the profile's mods is shown (its name,
// picture, description, tags, ...) in place of what its source says.
//
// The fields come from the backend (`mod_fields`, see core's edits.rs), each
// with what the source says and what the user's changed it to, so a field
// added there turns up here without changes. Each starts out with the
// user's value, or else the source's, to tweak rather than retype; one left
// as (or put back to) the source's isn't saved as a change, so it follows
// the source when that changes.

import { invoke, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { showProfile } from "./profile.js";

const dialog = $("#edit-mod-dialog");
const form = $("#edit-mod-form");
const fieldsEl = $("#edit-mod-fields");
const errorEl = $("#edit-mod-error");
/** The mod the dialog is about, the profile it's in, and its fields' views. */
let open = null;

const PICTURE_TYPES = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif", "svg"];

/** A field's value as the text its box holds: lists are comma-separated. */
const asText = (value) => (Array.isArray(value) ? value.join(", ") : (value ?? ""));

/**
 * What a box's text means for a field, tidied the way the backend tidies it,
 * so it can be compared with the source's: null when it's empty.
 */
function parse(field, text) {
  if (field.kind === "list") {
    const items = [...new Set(text.split(",").map((t) => t.trim()).filter(Boolean))];
    return items.length ? items : null;
  }
  const tidy = field.kind === "longText" ? text.replace(/^[\r\n]+/, "").trimEnd() : text.trim();
  return tidy || null;
}

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);

/** Opens the dialog for mod `m` of the open profile. */
export async function showEditMod(m) {
  const id = state.profile.id;
  let fields;
  try {
    fields = await invoke("mod_fields", { id, name: m.name });
  } catch (err) {
    status(String(err), true);
    return;
  }
  const shown = m.summary?.name ?? m.name;
  $("#edit-mod-title").textContent = `Edit ${shown}`;
  open = { id, name: m.name, views: fields.map(fieldView) };
  fieldsEl.replaceChildren(...open.views.map((v) => v.el));
  errorEl.hidden = true;
  updateResetAll();
  dialog.showModal();
  open.views[0]?.control.focus();
}

/** A field's label, box and reset button, and how to read it back. */
function fieldView(field) {
  const long = field.kind === "longText";
  const control = long
    ? h("textarea", { rows: field.id === "description" ? 4 : 3, spellcheck: "true" })
    : h("input", {
        type: field.kind === "link" ? "url" : "text",
        autocomplete: "off",
        spellcheck: field.kind === "text" || field.kind === "list" ? "true" : "false",
      });
  control.name = field.id;
  control.value = asText(field.edited ?? field.source);
  // emptied, it goes back to the source's, so that's what shows
  control.placeholder =
    field.source != null
      ? asText(field.source)
      : field.kind === "picture"
        ? "A link, or choose a picture on this computer"
        : field.kind === "list"
          ? "Separate several with commas"
          : "";
  const changed = () => !same(parse(field, control.value), field.source ?? null);
  const reset = h(
    "button",
    {
      type: "button",
      class: "link edit-mod-reset",
      onclick: () => {
        control.value = asText(field.source);
        control.dispatchEvent(new Event("input"));
        control.focus();
      },
    },
    field.source != null ? "Use the source's" : "Clear",
  );
  const badge = h("span", { class: "edit-mod-changed" }, "Changed");
  const help = field.help && h("small", { class: "muted" }, field.help);

  let box = control;
  let preview = null;
  if (field.kind === "picture") {
    preview = h("div", { class: "edit-mod-preview", "aria-hidden": "true" });
    const choose = h("button", { type: "button", onclick: () => choosePicture(control) }, "Choose…");
    box = h("div", { class: "edit-mod-picture" }, preview, h("div", { class: "row" }, control, choose));
  }

  const el = h(
    "div",
    { class: `field edit-mod-field edit-mod-${field.kind}`, "data-field": field.id },
    h(
      "div",
      { class: "edit-mod-label" },
      h("label", { for: `edit-mod-${field.id}` }, field.label),
      badge,
      reset,
    ),
    box,
    help,
  );
  control.id = `edit-mod-${field.id}`;
  const update = () => {
    const isChanged = changed();
    el.classList.toggle("changed", isChanged);
    badge.hidden = !isChanged;
    reset.hidden = !isChanged;
    if (preview) showPreview(preview, parse(field, control.value));
    updateResetAll();
  };
  control.addEventListener("input", update);
  update();
  return { field, control, el, changed, value: () => parse(field, control.value) };
}

/** Shows `picture` (a link or a path on this computer) in a field's preview. */
let previewToken = 0;
async function showPreview(el, picture) {
  const token = ++previewToken;
  let src = null;
  if (picture && (picture.includes("://") || picture.startsWith("data:"))) {
    src = picture;
  } else if (picture) {
    // the window can't load files itself
    src = await invoke("picture_preview", { path: picture }).catch(() => null);
    if (token !== previewToken) return;
  }
  el.replaceChildren(
    src
      ? h("img", { src, alt: "", onerror: (e) => e.target.replaceWith(previewMissing()) })
      : previewMissing(picture),
  );
}

const previewMissing = (picture) =>
  h("span", { class: "muted" }, picture ? "Can't show it" : "No picture");

/** Picks a picture on this computer with the system's picker and puts its path in `input`. */
async function choosePicture(input) {
  try {
    const path = await window.__TAURI__.dialog.open({
      directory: false,
      multiple: false,
      filters: [{ name: "Pictures", extensions: PICTURE_TYPES }],
    });
    if (path) {
      input.value = path;
      input.dispatchEvent(new Event("input"));
    }
  } catch (err) {
    status(String(err), true);
  }
}

/** Only offers to put every field back when some have changed. */
function updateResetAll() {
  $("#edit-mod-reset").disabled = !open?.views.some((v) => v.changed());
}

async function save() {
  const { id, name, views } = open;
  // every field is sent, so one put back to the source's stops being a change
  const changes = Object.fromEntries(views.map((v) => [v.field.id, v.changed() ? v.value() : null]));
  const profile = await invoke("set_mod_edits", { id, name, changes });
  dialog.close();
  if (state.profile?.id === id) showProfile(profile);
  status(`Saved changes to ${name}`);
}

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const button = form.querySelector("[type=submit]");
  button.disabled = true;
  try {
    await save();
  } catch (err) {
    errorEl.textContent = String(err);
    errorEl.hidden = false;
  } finally {
    button.disabled = false;
  }
});

$("#edit-mod-reset").addEventListener("click", () => {
  for (const v of open?.views ?? []) {
    v.control.value = asText(v.field.source);
    v.control.dispatchEvent(new Event("input"));
  }
});

form.querySelector("[value=cancel]").addEventListener("click", () => dialog.close());
