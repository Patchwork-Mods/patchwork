// A mod's FOMOD installer, from the Installer button on its card or row: a
// step at a time, like the installers the mods were made for. Each change is
// worked out again by main.rs (fomod.rs in core), since picks can show or hide
// later steps and change which options can be picked. Nothing's saved until
// the last step's Save.

import { $, h } from "./dom.js";
import { run, state, status } from "./state.js";
import { showProfile } from "./profile.js";
import { openPlugins } from "./plugins.js";

/**
 * The installer being shown: the mod it's for, what's picked so far, the
 * step that's showing, and the pictures already fetched, by path in the mod.
 */
let open = null;
/** The latest load, so an older one finishing late is ignored. */
let loads = 0;

const RULES = {
  exactlyOne: "Pick one",
  atMostOne: "Pick one or none",
  atLeastOne: "Pick at least one",
  all: "All of these are installed",
  any: "Pick any",
};

const KINDS = {
  required: "Required",
  recommended: "Recommended",
  notUsable: "Can't be used",
  couldBeUsable: "Might not work",
};

/** Opens #installer-dialog on `m`'s installer, starting from what's saved. */
export async function showInstaller(m) {
  open = {
    id: state.profile.id,
    name: m.name,
    picks: null,
    view: null,
    step: 0,
    pictures: new Map(),
  };
  $("#installer-title").textContent = `${m.name} installer`;
  $("#installer-step").textContent = "";
  $("#installer-groups").replaceChildren(h("p", { class: "muted" }, "Loading…"));
  preview(null);
  $("#installer-warnings").replaceChildren();
  $("#installer-files").textContent = "";
  $("#installer-next").disabled = true;
  $("#installer-back").disabled = true;
  $("#installer-dialog").showModal();
  await load(null);
}

/** Works out the installer for `picks` (what's saved, if null) and shows it. */
async function load(picks) {
  const cur = open;
  const seq = ++loads;
  let view;
  try {
    view = await run("installer", {
      id: cur.id,
      name: cur.name,
      picks,
      have: [...cur.pictures.keys()],
    });
  } catch (e) {
    if (cur === open) $("#installer-groups").replaceChildren(h("p", { class: "error" }, String(e)));
    return;
  }
  if (seq !== loads || cur !== open) return;
  if (!view) {
    $("#installer-groups").replaceChildren(
      h("p", {}, `${cur.name} doesn't have an installer that this game uses.`),
    );
    return;
  }
  for (const [path, src] of Object.entries(view.pictures)) cur.pictures.set(path, src);
  cur.view = view;
  cur.picks = view.selections;
  cur.step = Math.max(0, Math.min(cur.step, view.steps.length - 1));
  render();
}

function render() {
  const { view, step: at, name } = open;
  const steps = view.steps;
  const step = steps[at];
  $("#installer-title").textContent =
    view.name && view.name !== name ? `${name} installer: ${view.name}` : `${name} installer`;
  $("#installer-step").textContent = step
    ? steps.length > 1
      ? `Step ${at + 1} of ${steps.length}: ${step.name}`
      : step.name
    : "";
  $("#installer-groups").replaceChildren(
    ...(step
      ? step.groups.map((g, i) => groupView(step, g, i))
      : [h("p", { class: "muted" }, "This installer doesn't have any options to pick.")]),
  );
  const first = step?.groups.flatMap((g) => g.options).find((o) => o.picked);
  preview(first ?? step?.groups[0]?.options[0] ?? null);

  $("#installer-warnings").replaceChildren(
    ...view.warnings.map((w) => h("li", { class: "warning" }, w)),
  );
  const files = `${view.files} file${view.files === 1 ? "" : "s"}`;
  $("#installer-files").textContent = view.saved
    ? `Installs ${files}.`
    : `Installs ${files}. Until you save, deploys use the options it recommends.`;
  $("#installer-back").disabled = at === 0;
  const last = at >= steps.length - 1;
  $("#installer-next").textContent = last ? "Save" : "Next";
  $("#installer-next").disabled = false;
  $("#installer-reset").hidden = !view.saved;
}

/** A group's options, as radio buttons or checkboxes. */
function groupView(step, group, index) {
  const single = group.kind === "exactlyOne" || group.kind === "atMostOne";
  const input = `installer-group-${index}`;
  const options = group.options.map((o) => {
    const locked = o.kind === "required" || o.kind === "notUsable" || group.kind === "all";
    return h(
      "label",
      {
        class: o.kind === "notUsable" ? "installer-option disabled" : "installer-option",
        onmouseenter: () => preview(o),
        onfocusin: () => preview(o),
      },
      h("input", {
        type: single ? "radio" : "checkbox",
        name: input,
        value: o.name,
        checked: o.picked,
        disabled: locked,
        onchange: () => pick(step, group, input),
      }),
      h("span", {}, o.name),
      KINDS[o.kind] && h("span", { class: "badge" }, KINDS[o.kind]),
    );
  });
  if (group.kind === "atMostOne") {
    options.push(
      h(
        "label",
        { class: "installer-option" },
        h("input", {
          type: "radio",
          name: input,
          value: "",
          checked: !group.options.some((o) => o.picked),
          onchange: () => pick(step, group, input),
        }),
        h("span", { class: "muted" }, "None"),
      ),
    );
  }
  return h(
    "fieldset",
    { class: "installer-group" },
    h("legend", {}, group.name),
    h("p", { class: "muted" }, RULES[group.kind]),
    ...options,
  );
}

/** Takes what's checked in a group into the picks and works them out again. */
function pick(step, group, input) {
  const options = [...document.querySelectorAll(`input[name="${input}"]:checked`)]
    .map((i) => i.value)
    .filter(Boolean);
  const picks = open.picks.filter((p) => !(p.step === step.name && p.group === group.name));
  const at = open.picks.findIndex((p) => p.step === step.name && p.group === group.name);
  picks.splice(at < 0 ? picks.length : at, 0, { step: step.name, group: group.name, options });
  open.picks = picks;
  load(picks);
}

/** Shows an option's picture and description beside the groups. */
function preview(option) {
  const picture = $("#installer-picture");
  const src = option?.image && open?.pictures.get(option.image);
  picture.hidden = !src;
  if (src) picture.src = src;
  else picture.removeAttribute("src");
  $("#installer-description").textContent = option ? option.description || "No description." : "";
}

$("#installer-back").addEventListener("click", () => {
  if (!open?.view || open.step === 0) return;
  open.step -= 1;
  render();
});

$("#installer-next").addEventListener("click", async () => {
  if (!open?.view) return;
  if (open.step < open.view.steps.length - 1) {
    open.step += 1;
    render();
    return;
  }
  await save(open.picks, `Saved ${open.name}'s installer options`);
});

$("#installer-reset").addEventListener("click", () =>
  save(null, `${open.name} is back to the options its installer recommends`),
);

/** Saves `picks` (or forgets them, if null) and closes the dialog. */
async function save(picks, message) {
  const { id, name } = open;
  $("#installer-next").disabled = true;
  try {
    const profile = await run("set_installer", { id, name, picks });
    $("#installer-dialog").close();
    if (state.profile?.id === id) {
      showProfile(profile);
      if (state.tab === "plugins") openPlugins();
    }
    status(message);
  } catch {
    $("#installer-next").disabled = false;
  }
}

$("#installer-dialog").addEventListener("close", () => {
  open = null;
});
