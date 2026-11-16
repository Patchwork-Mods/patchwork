// The fields of an Edit dialog (a mod's, see editmod.js, or a game's, see
// editgame.js), built from what the backend says about each (core's
// edits.rs): its kind, what its source says, and what the user's changed it
// to. Each starts out with the user's value, or else the source's, to tweak
// rather than retype; one left as (or put back to) the source's isn't saved
// as a change, so it follows the source when that changes.
//
// Ids and classes start with the form's prefix (`edit-mod`, `game-appearance`),
// so themes can tell the dialogs apart.

import { invoke, status } from "./state.js";
import { h } from "./dom.js";
import { colorPicker, fullHex } from "./colorpick.js";

const PICTURE_TYPES = ["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif", "svg"];

/** A field's value as the text its box holds: lists are comma-separated. */
export const asText = (value) => (Array.isArray(value) ? value.join(", ") : (value ?? ""));

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

/**
 * A field's label, box and reset button, and how to read it back.
 * `onChange` is called whenever its box changes. With `noSource`, the
 * fields have nothing to go back to (a game the user added), so nothing is
 * marked as changed. `sourceName` is where the field's value comes from,
 * as in "Go back to what the mod's source says". `defaultColor` is what a
 * color field with nothing set comes to, to show it.
 */
export function fieldView(
  field,
  prefix,
  onChange,
  { noSource = false, sourceName = "the mod's source", defaultColor = null } = {},
) {
  if (field.kind === "color") return colorView(field, prefix, onChange, { noSource, defaultColor });
  const long = field.kind === "longText";
  const control = long
    ? h("textarea", { rows: field.id === "description" ? 4 : 3, spellcheck: "true" })
    : h("input", {
        type: field.kind === "link" ? "url" : "text",
        autocomplete: "off",
        spellcheck: field.kind === "text" || field.kind === "list" ? "true" : "false",
      });
  control.name = field.id;
  control.id = `${prefix}-${field.id}`;
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
  const reset = () => {
    control.value = asText(field.source);
    control.dispatchEvent(new Event("input"));
  };
  const resetButton = h(
    "button",
    {
      type: "button",
      class: `link ${prefix}-reset`,
      title: field.source != null ? `Go back to what ${sourceName} says` : "",
      onclick: () => {
        reset();
        control.focus();
      },
    },
    field.source != null ? "Reset" : "Clear",
  );
  const badge = h("span", { class: `${prefix}-changed` }, "Changed");

  let box = control;
  let preview = null;
  if (field.kind === "picture") {
    preview = h("div", { class: `${prefix}-preview`, "aria-hidden": "true" });
    const choose = h("button", { type: "button", onclick: () => choosePicture(control) }, "Choose…");
    box = h("div", { class: `${prefix}-picture` }, preview, h("div", { class: "row" }, control, choose));
  }

  const el = fieldEl(field, prefix, control.id, [badge, resetButton], box);
  const update = () => {
    const isChanged = !noSource && changed();
    el.classList.toggle("changed", isChanged);
    badge.hidden = !isChanged;
    resetButton.hidden = noSource ? !control.value : !isChanged;
    if (preview) showPreview(preview, parse(field, control.value));
    onChange();
  };
  control.addEventListener("input", update);
  update();
  return { field, control, el, changed, reset, value: () => parse(field, control.value) };
}

/** A field's outside: its label (with `extras` beside it), box and help. */
function fieldEl(field, prefix, controlId, extras, box) {
  return h(
    "div",
    { class: `field ${prefix}-field ${prefix}-${field.kind}`, "data-field": field.id },
    h("div", { class: `${prefix}-label` }, h("label", { for: controlId }, field.label), ...extras),
    box,
    field.help && h("small", { class: "muted" }, field.help),
  );
}

/**
 * A color field: a checkbox for whether to set one at all, and a color
 * picker (colorpick.js) with a box for its code beside it, each kept showing what
 * the other has. Unchecked, both show the color used instead (the source's,
 * or else `defaultColor`) and can't be changed.
 */
function colorView(field, prefix, onChange, { noSource, defaultColor }) {
  const fallback = () => fullHex(field.source) ?? fullHex(defaultColor) ?? "#808080";
  const id = `${prefix}-${field.id}`;
  const own = h("input", { type: "checkbox", id: `${id}-own` });
  const swatch = colorPicker({
    id,
    class: `${prefix}-swatch`,
    "aria-label": field.label,
  });
  const code = h("input", {
    type: "text",
    class: `${prefix}-color-code`,
    name: field.id,
    autocomplete: "off",
    spellcheck: "false",
    maxlength: "7",
    "aria-label": `${field.label} code`,
  });
  const badge = h("span", { class: `${prefix}-changed` }, "Changed");

  const value = () => (own.checked ? swatch.value : null);
  const changed = () => own.checked && swatch.value !== fullHex(field.source);
  const show = (color) => {
    swatch.value = color;
    code.value = color;
  };
  const update = () => {
    swatch.disabled = code.disabled = !own.checked;
    const isChanged = !noSource && changed();
    el.classList.toggle("changed", isChanged);
    badge.hidden = !isChanged;
    onChange();
  };
  own.addEventListener("change", () => {
    // unchecked, it shows what's used instead
    if (!own.checked) show(fallback());
    update();
    if (own.checked) swatch.focus();
  });
  swatch.addEventListener("input", () => {
    code.value = swatch.value;
    update();
  });
  code.addEventListener("input", () => {
    const hex = fullHex(code.value.trim().toLowerCase());
    if (hex) {
      swatch.value = hex;
      update();
    }
  });
  // a code that isn't a color goes back to the picker's
  code.addEventListener("change", () => {
    code.value = swatch.value;
  });

  const box = h(
    "div",
    { class: `row ${prefix}-color` },
    h("label", { class: `${prefix}-color-own` }, own, "Custom"),
    swatch,
    code,
  );
  const el = fieldEl(field, prefix, own.id, [badge], box);
  own.checked = field.edited != null;
  show(own.checked ? (fullHex(field.edited ?? field.source) ?? fallback()) : fallback());
  update();
  const reset = () => {
    own.checked = false;
    own.dispatchEvent(new Event("change"));
  };
  return { field, control: own, el, changed, reset, value };
}


/** Shows `picture` (a link or a path on this computer) in a field's preview. */
const previewTokens = new WeakMap();
async function showPreview(el, picture) {
  const token = (previewTokens.get(el) ?? 0) + 1;
  previewTokens.set(el, token);
  let src = null;
  if (picture && (picture.includes("://") || picture.startsWith("data:"))) {
    src = picture;
  } else if (picture) {
    // the window can't load files itself
    src = await invoke("picture_preview", { path: picture }).catch(() => null);
    if (previewTokens.get(el) !== token) return;
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

/**
 * What to send the backend for `views`: every field, so one put back to the
 * source's stops being a change.
 */
export const changesOf = (views) =>
  Object.fromEntries(views.map((v) => [v.field.id, v.changed() ? v.value() : null]));

/** Puts every field back to its source's. */
export function resetAll(views) {
  for (const v of views) v.reset();
}

/**
 * Wires up an Edit dialog's form: Save calls `save` (showing what goes wrong
 * in `errorEl`), and Cancel closes it.
 */
export function wireForm(dialog, form, errorEl, save) {
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
  form.querySelector("[value=cancel]").addEventListener("click", () => dialog.close());
}
