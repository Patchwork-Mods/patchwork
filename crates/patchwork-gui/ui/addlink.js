// Adding a mod from a pasted link, like `patchwork add <link>`. The source
// that recognizes the link may have several files to choose between, like
// a GitHub release's; the one picked is what later updates look for.

import { run, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { showProfile } from "./profile.js";
import { refreshDetails } from "./details.js";

const dialog = $("#link-dialog");
const form = $("#link-form");
const submit = $("#link-submit");

/** What the link in the box is for, once it's been looked up. */
let linked = null;

function showError(err) {
  $("#link-error").textContent = err ? String(err) : "";
  $("#link-error").hidden = !err;
}

function size(bytes) {
  if (bytes == null) return "";
  if (bytes < 1000) return `${bytes} B`;
  if (bytes < 1e6) return `${(bytes / 1e3).toFixed(0)} KB`;
  return `${(bytes / 1e6).toFixed(1)} MB`;
}

/** Forgets the file choices, for a new link. */
function forget() {
  linked = null;
  $("#link-files").hidden = true;
  $("#link-file-list").replaceChildren();
}

function showChoices(choices) {
  $("#link-file-list").replaceChildren(
    ...choices.map((c) =>
      h(
        "label",
        { class: "row" },
        h("input", { type: "radio", name: "file", value: c.id, required: true }),
        h(
          "span",
          {},
          c.name,
          c.size != null && h("small", { class: "muted" }, ` ${size(c.size)}`),
          c.description && h("br"),
          c.description && h("small", { class: "muted" }, c.description),
        ),
      ),
    ),
  );
  $("#link-files").hidden = false;
  $("#link-file-list").querySelector("input")?.focus();
}

$("#add-link").addEventListener("click", () => {
  form.reset();
  forget();
  showError(null);
  dialog.showModal();
  form.elements.link.focus();
});

form.querySelector("[value=cancel]").addEventListener("click", () => dialog.close());
form.elements.link.addEventListener("input", () => {
  forget();
  showError(null);
});

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const p = state.profile;
  if (!p) return;
  submit.disabled = true;
  showError(null);
  try {
    let modId;
    if (linked) {
      modId = new FormData(form).get("file");
    } else {
      status("Looking up the link…");
      const found = await run("parse_link", { id: p.id, link: form.elements.link.value });
      if (found.choices.length > 1) {
        linked = found;
        showChoices(found.choices);
        status("Pick a file to add");
        return;
      }
      linked = found;
      modId = found.choices[0]?.id ?? found.id;
    }
    status("Adding…");
    const added = await run("add_mod", {
      id: p.id,
      source: linked.source,
      modId,
      version: linked.version,
    });
    dialog.close();
    status(`Added ${added.name}`);
    if (state.profile?.id === p.id) {
      showProfile(added.profile);
      refreshDetails();
    }
  } catch (err) {
    showError(err);
    // a link that didn't work is looked up again once it's changed
    if (!$("#link-files").hidden) return;
    linked = null;
  } finally {
    submit.disabled = false;
  }
});
