// A mod's choices, from the button on its card or row for them (Installer,
// for a FOMOD installer): a page at a time, like the installers mods were
// made for. Each change is worked out again by main.rs (the extension in
// core), since picks can show or hide later pages and change what can be
// picked. Nothing's saved until the last page's Save. Nothing here knows
// what the choices are for; it draws the form core hands it.

import { $, h } from "./dom.js";
import { run, state, status } from "./state.js";
import { showProfile } from "./profile.js";
import { refreshList } from "./list.js";

/**
 * The form being shown: the mod and choices it's for, what's picked so far,
 * the page that's showing, and the pictures already fetched, by path in the
 * mod.
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

/** Works out the form for `answers` (what's saved, if null) and shows it. */
async function load(answers) {
  const cur = open;
  const seq = ++loads;
  let view;
  try {
    view = await run("mod_options", {
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
  // it's something else.
  const title = `${name} ${view.title.toLowerCase()}`;
  $("#options-title").textContent =
    form.title && form.title !== name ? `${title}: ${form.title}` : title;
  $("#options-page").textContent = page
    ? pages.length > 1
      ? `Step ${at + 1} of ${pages.length}: ${page.title}`
      : page.title
    : "";
  $("#options-groups").replaceChildren(
    ...(page
      ? page.groups.map((g, i) => groupView(g, i))
      : [h("p", { class: "muted" }, "There aren't any options to pick.")]),
  );
  const first = page?.groups.flatMap((g) => g.items).find((i) => i.picked);
  preview(first ?? page?.groups[0]?.items[0] ?? null);

  $("#options-warnings").replaceChildren(
    ...form.warnings.map((w) => h("li", { class: "warning" }, w)),
  );
  const summary = form.summary ? `${form.summary} ` : "";
  $("#options-summary").textContent = view.saved
    ? summary
    : `${summary}Until you save, deploys use the defaults.`;
  $("#options-back").disabled = at === 0;
  const last = at >= pages.length - 1;
  $("#options-next").textContent = last ? "Save" : "Next";
  $("#options-next").disabled = false;
  $("#options-reset").hidden = !view.saved;
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
  const answers = open.answers.filter((a) => a.group !== group.id);
  const at = open.answers.findIndex((a) => a.group === group.id);
  answers.splice(at < 0 ? answers.length : at, 0, { group: group.id, items });
  open.answers = answers;
  load(answers);
}

/** Shows an item's picture and description beside the groups. */
function preview(item) {
  const picture = $("#options-picture");
  const src = item?.image && open?.pictures.get(item.image);
  picture.hidden = !src;
  if (src) picture.src = src;
  else picture.removeAttribute("src");
  $("#options-description").textContent = item ? item.description || "No description." : "";
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
  await save(open.answers, `Saved ${open.name}'s ${open.title.toLowerCase()} options`);
});

$("#options-reset").addEventListener("click", () =>
  save(null, `${open.name} is back to the default ${open.title.toLowerCase()} options`),
);

/** Saves `answers` (or forgets them, if null) and closes the dialog. */
async function save(answers, message) {
  const { id, name, options } = open;
  $("#options-next").disabled = true;
  try {
    const profile = await run("set_mod_options", { id, name, options, answers });
    $("#options-dialog").close();
    if (state.profile?.id === id) {
      showProfile(profile);
      refreshList();
    }
    status(message);
  } catch {
    $("#options-next").disabled = false;
  }
}

$("#options-dialog").addEventListener("close", () => {
  open = null;
});
