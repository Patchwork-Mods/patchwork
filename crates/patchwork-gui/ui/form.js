// A mod's choices, from the button on its card or row for them (Installer,
// for a FOMOD installer): a page at a time, like the installers mods were
// made for. Also a config file's settings, from the Config tab (config.js),
// which are one page of fields. Each change is worked out again by main.rs (the extension in
// core), since picks can show or hide later pages and change what can be
// picked. Nothing's saved until the last page's Save. Nothing here knows
// what the choices are for; it draws the form core hands it: groups of items
// to pick, or of fields to set.

import { $, h } from "./dom.js";
import { run, state, status } from "./state.js";
import { showProfile } from "./profile.js";
import { refreshList } from "./list.js";

/**
 * The form being shown: the mod and choices it's for (or the config file's
 * `path`), what's picked so far, the page that's showing, and the pictures
 * already fetched, by path in the mod.
 */
let open = null;
/** The latest load, so an older one finishing late is ignored. */
let loads = 0;

const RULES = {
  exactlyOne: "Pick one",
  atMostOne: "Pick one or none",
  atLeastOne: "Pick at least one",
  all: "All of these are picked",
  any: "Pick any",
};

const STATES = {
  required: "Required",
  recommended: "Recommended",
  notUsable: "Can't be used",
  couldBeUsable: "Might not work",
};

/** Opens #options-dialog on `m`'s choices `options` (one of `m.options`),
 * starting from what's saved. */
export async function showOptions(m, options) {
  open = {
    id: state.profile.id,
    name: m.name,
    options: options.id,
    title: options.title,
    answers: null,
    view: null,
    page: 0,
    pictures: new Map(),
  };
  $("#options-title").textContent = `${m.name} ${options.title.toLowerCase()}`;
  $("#options-page").textContent = "";
  $("#options-groups").replaceChildren(h("p", { class: "muted" }, "Loading…"));
  preview(null);
  $("#options-warnings").replaceChildren();
  $("#options-summary").textContent = "";
  $("#options-next").disabled = true;
  $("#options-back").disabled = true;
  $("#options-dialog").showModal();
  await load(null);
}

/** Opens #options-dialog on a config file's settings (one of those from
 * config_files in main.rs), starting from what's saved. */
export async function showConfig(file) {
  const name = file.title ?? file.path.split("/").pop();
  open = {
    id: state.profile.id,
    name,
    path: file.path,
    title: "Settings",
    answers: null,
    view: null,
    page: 0,
    pictures: new Map(),
  };
  $("#options-title").textContent = `${name} settings`;
  $("#options-page").textContent = file.path;
  $("#options-groups").replaceChildren(h("p", { class: "muted" }, "Loading…"));
  preview(null);
  $("#options-warnings").replaceChildren();
  $("#options-summary").textContent = "";
  $("#options-next").disabled = true;
  $("#options-back").disabled = true;
  $("#options-dialog").showModal();
  await load(null);
}

/** Works out the form for `answers` (what's saved, if null) and shows it. */
async function load(answers) {
  const cur = open;
  const seq = ++loads;
  let view;
  try {
    view = cur.path
      ? { ...(await run("config_form", { id: cur.id, path: cur.path, answers })), pictures: {} }
      : await run("mod_options", {
          id: cur.id,
          name: cur.name,
          options: cur.options,
          answers,
          have: [...cur.pictures.keys()],
        });
  } catch (e) {
    if (cur === open) $("#options-groups").replaceChildren(h("p", { class: "error" }, String(e)));
    return;
  }
  if (seq !== loads || cur !== open) return;
  for (const [path, src] of Object.entries(view.pictures)) cur.pictures.set(path, src);
  cur.view = view;
  cur.answers = view.form.answers;
  cur.page = Math.max(0, Math.min(cur.page, view.form.pages.length - 1));
  render();
}

function render() {
  const { view, page: at, name } = open;
  const form = view.form;
  const pages = form.pages;
  const page = pages[at];
  // Along with the name the choices give the mod, like an installer's, if
  // it's something else. A config file's is its own name, and the mod it's
  // from.
  if (open.path) {
    $("#options-title").textContent = view.mod
      ? `${form.title} settings (from ${view.mod})`
      : `${form.title} settings`;
  } else {
    const title = `${name} ${view.title.toLowerCase()}`;
    $("#options-title").textContent =
      form.title && form.title !== name ? `${title}: ${form.title}` : title;
  }
  $("#options-page").textContent = page
    ? pages.length > 1
      ? `Step ${at + 1} of ${pages.length}: ${page.title}`
      : page.title
    : "";
  // Each change draws the form again, so the field that had focus gets it
  // back, as tabbing out of one changes it.
  const focused = document.activeElement?.dataset?.field;
  $("#options-groups").replaceChildren(
    ...(page
      ? page.groups.map((g, i) => (g.kind === "fields" ? fieldsView(g) : groupView(g, i)))
      : [h("p", { class: "muted" }, "There aren't any options to pick.")]),
  );
  if (focused) {
    const again = [...$("#options-groups").querySelectorAll("[data-field]")].find(
      (el) => el.dataset.field === focused,
    );
    again?.focus();
  }
  const items = page?.groups.flatMap((g) => g.items ?? []) ?? [];
  const fields = page?.groups.flatMap((g) => g.fields ?? []) ?? [];
  preview(items.find((i) => i.picked) ?? items[0] ?? fields[0] ?? null);

  $("#options-warnings").replaceChildren(
    ...form.warnings.map((w) => h("li", { class: "warning" }, w)),
  );
  const summary = form.summary ? `${form.summary} ` : "";
  $("#options-summary").textContent =
    view.saved || open.path ? summary : `${summary}Until you save, deploys use the defaults.`;
  $("#options-back").disabled = at === 0;
  // A form of one page, like a config file's, has nowhere to go back to.
  $("#options-back").hidden = pages.length <= 1;
  const last = at >= pages.length - 1;
  $("#options-next").textContent = last ? "Save" : "Next";
  $("#options-next").disabled = false;
  $("#options-reset").hidden = !view.saved;
  $("#options-reset").textContent = open.path ? "Undo all changes" : "Use defaults";
  $("#options-reset").title = open.path
    ? "Forget the settings changed in this file, and deploy it as it is"
    : "Forget the options picked, and deploy with the defaults";
}

/** A group's items, as radio buttons or checkboxes. */
function groupView(group, index) {
  const single = group.pick === "exactlyOne" || group.pick === "atMostOne";
  const input = `options-group-${index}`;
  const items = group.items.map((item) => {
    const locked = item.state === "required" || item.state === "notUsable" || group.pick === "all";
    return h(
      "label",
      {
        class: item.state === "notUsable" ? "options-item disabled" : "options-item",
        onmouseenter: () => preview(item),
        onfocusin: () => preview(item),
      },
      h("input", {
        type: single ? "radio" : "checkbox",
        name: input,
        value: item.id,
        checked: item.picked,
        disabled: locked,
        onchange: () => pick(group, input),
      }),
      h("span", {}, item.title),
      STATES[item.state] && h("span", { class: "badge" }, STATES[item.state]),
    );
  });
  if (group.pick === "atMostOne") {
    items.push(
      h(
        "label",
        { class: "options-item" },
        h("input", {
          type: "radio",
          name: input,
          value: "",
          checked: !group.items.some((i) => i.picked),
          onchange: () => pick(group, input),
        }),
        h("span", { class: "muted" }, "None"),
      ),
    );
  }
  return h(
    "fieldset",
    { class: "options-group" },
    h("legend", {}, group.title),
    h("p", { class: "muted" }, RULES[group.pick]),
    ...items,
  );
}

/** Takes what's checked in a group into the answers and works them out
 * again. */
function pick(group, input) {
  const items = [...document.querySelectorAll(`input[name="${input}"]:checked`)]
    .map((i) => i.value)
    .filter(Boolean);
  answer(group, (a) => ({ ...a, items }));
}

/** Replaces `group`'s answer with what `change` makes of it (an empty one,
 * if it hasn't got one), and works them out again. */
function answer(group, change) {
  const at = open.answers.findIndex((a) => a.group === group.id);
  const answers = [...open.answers];
  const changed = change(at < 0 ? { group: group.id, items: [] } : answers[at]);
  if (at < 0) answers.push(changed);
  else answers[at] = changed;
  open.answers = answers;
  load(answers);
}

/** A group's fields, each with something to set it with that suits it. */
function fieldsView(group) {
  return h(
    "fieldset",
    { class: "options-group options-fields" },
    h("legend", {}, group.title),
    ...group.fields.map((field) => {
      const set = (value) =>
        answer(group, (a) => ({ ...a, values: { ...a.values, [field.id]: value } }));
      const key = JSON.stringify([group.id, field.id]);
      const control = fieldControl(field, set, key);
      const toggle = field.type === "toggle";
      return h(
        "label",
        {
          class: toggle ? "options-field options-item" : "options-field",
          onmouseenter: () => preview(field),
          onfocusin: () => preview(field),
        },
        toggle && control,
        h("span", {}, field.title),
        !toggle && control,
      );
    }),
  );
}

/** What sets `field`: `set` gets the value it's changed to. `key` finds it
 * again once the form's drawn again. */
function fieldControl(field, set, key) {
  const value = field.value;
  switch (field.type) {
    case "toggle":
      return h("input", {
        type: "checkbox",
        checked: value === true,
        "data-field": key,
        onchange: (e) => set(e.target.checked),
      });
    case "int":
    case "float": {
      const whole = field.type === "int";
      return h("input", {
        type: "number",
        class: "options-number",
        value: String(value),
        min: field.min,
        max: field.max,
        step: field.step ?? (whole ? 1 : "any"),
        "data-field": key,
        onchange: (e) => {
          const n = whole ? Number.parseInt(e.target.value, 10) : Number(e.target.value);
          // Left as it was, if it isn't a number at all; core says what's
          // wrong with ones that are, but are out of range.
          if (e.target.value.trim() === "" || !Number.isFinite(n)) e.target.value = String(value);
          else set(n);
        },
      });
    }
    case "text":
      return h(field.multiline ? "textarea" : "input", {
        type: field.multiline ? null : "text",
        class: "options-text",
        value: field.multiline ? null : value,
        rows: field.multiline ? 4 : null,
        "data-field": key,
        onchange: (e) => set(e.target.value),
        // A textarea's text is its contents rather than an attribute.
      }, field.multiline ? String(value) : null);
    case "choice":
      return h(
        "select",
        { "data-field": key, onchange: (e) => set(e.target.value) },
        ...field.choices.map((c) =>
          h("option", { value: c.id, selected: c.id === value }, c.title),
        ),
      );
    case "color": {
      const rgb = String(value).slice(0, 7);
      const alpha = String(value).slice(7) || "ff";
      const picker = h("input", {
        type: "color",
        value: rgb,
        "data-field": key,
        onchange: (e) => set(field.alpha ? e.target.value + alpha : e.target.value),
      });
      if (!field.alpha) return picker;
      const opacity = h("input", {
        type: "range",
        class: "options-alpha",
        min: 0,
        max: 255,
        value: Number.parseInt(alpha, 16),
        title: "Opacity",
        "data-field": `${key}alpha`,
        onchange: (e) => set(rgb + Number(e.target.value).toString(16).padStart(2, "0")),
      });
      return h("span", { class: "options-color" }, picker, opacity);
    }
    default:
      return h("span", { class: "muted" }, String(value));
  }
}

/** Shows an item's or field's picture and description beside the groups,
 * with a field's default. */
function preview(item) {
  const picture = $("#options-picture");
  const src = item?.image && open?.pictures.get(item.image);
  picture.hidden = !src;
  if (src) picture.src = src;
  else picture.removeAttribute("src");
  let text = item ? item.description || "No description." : "";
  if (item && "default" in item && item.default != null) {
    text += `\n\nDefault: ${shown(item, item.default)}`;
  }
  $("#options-description").textContent = text;
}

/** `value` as `field` shows it: a choice by its title. */
function shown(field, value) {
  if (field.type === "choice") return field.choices.find((c) => c.id === value)?.title ?? value;
  if (field.type === "toggle") return value ? "On" : "Off";
  return String(value);
}

$("#options-back").addEventListener("click", () => {
  if (!open?.view || open.page === 0) return;
  open.page -= 1;
  render();
});

$("#options-next").addEventListener("click", async () => {
  if (!open?.view) return;
  if (open.page < open.view.form.pages.length - 1) {
    open.page += 1;
    render();
    return;
  }
  await save(
    open.answers,
    open.path
      ? `Saved ${open.name}'s settings; deploy to apply them`
      : `Saved ${open.name}'s ${open.title.toLowerCase()} options`,
  );
});

$("#options-reset").addEventListener("click", () =>
  save(
    null,
    open.path
      ? `${open.name} is back to its own settings`
      : `${open.name} is back to the default ${open.title.toLowerCase()} options`,
  ),
);

/** Saves `answers` (or forgets them, if null) and closes the dialog. */
async function save(answers, message) {
  const { id, name, options, path } = open;
  $("#options-next").disabled = true;
  try {
    const profile = path
      ? await run("set_config", { id, path, answers })
      : await run("set_mod_options", { id, name, options, answers });
    $("#options-dialog").close();
    if (state.profile?.id === id) {
      // Showing the profile loads the open list or Config tab again.
      showProfile(profile);
      if (!path) refreshList();
    }
    status(message);
  } catch {
    $("#options-next").disabled = false;
  }
}

$("#options-dialog").addEventListener("close", () => {
  open = null;
});
