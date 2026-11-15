// Adding a mod from a link pasted into a search box, like `patchwork add
// <link>`. The Add mods tab's box (#browse-link) and the Mods tab's
// (#mods-link) both search, and add a mod instead while what's in them looks
// like a link, which the hint under them (.link-hint) says where from; each
// works on its own. The source that recognizes the link may have
// several files to choose between, like a GitHub release's; they're listed
// under the link, and the one picked is what later updates look for. A
// shared folder's files are listed the same way, with its folders opening
// in place to list theirs, and files that don't look like mods (a PDF, a
// picture) dimmed.

import { invoke, run, sourceName, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { renderProfile } from "./profile.js";
import { refreshDetails } from "./details.js";
import { boxChanged } from "./sourcesearch.js";
import { browseChanged } from "./browse.js";

function size(bytes) {
  if (bytes == null) return "";
  if (bytes < 1000) return `${bytes} B`;
  if (bytes < 1e6) return `${(bytes / 1e3).toFixed(0)} KB`;
  return `${(bytes / 1e6).toFixed(1)} MB`;
}

/**
 * The link `text` is, when it looks like one rather than words to search
 * for: anything with a scheme, like `https://` or `nxm://`, or a domain
 * with a path after it, which gets `https://` put in front. Words with
 * spaces, or a bare domain, aren't links.
 */
export function asLink(text) {
  const t = text.trim();
  if (!t || /\s/.test(t)) return null;
  if (/^[a-z][a-z0-9+.-]*:\/\/./i.test(t)) return t;
  if (/^(?:[a-z0-9-]+\.)+[a-z]{2,}\/\S/i.test(t)) return `https://${t}`;
  return null;
}

/** The site a web link is on, like `nexusmods.com`. */
export function site(link) {
  try {
    const url = new URL(link);
    return /^https?:$/.test(url.protocol) ? url.hostname.replace(/^www\./, "") : null;
  } catch {
    return null;
  }
}

/** Like invoke(), without putting a failure in the status bar; null if it fails. */
async function invokeQuietly(command, args) {
  try {
    return await invoke(command, args);
  } catch {
    return null;
  }
}

/**
 * Makes `form` a search box that adds a mod when what's in it looks like a
 * link, which is when its Add button shows, returning a function that
 * empties it. `onChange` is called whenever what's in the box changes,
 * including being emptied.
 */
function linkBox(form, onChange) {
  const input = form.querySelector(".link-url");
  const submit = form.querySelector(".link-submit");
  const hintEl = form.querySelector(".link-hint");
  const files = form.querySelector(".link-files");
  const fileList = form.querySelector(".link-file-list");
  const fileAdd = form.querySelector(".link-file-add");
  const error = form.querySelector(".link-error");

  /** What the link in the box is for, once it's been looked up. */
  let linked = null;

  function showError(err) {
    error.textContent = err ? String(err) : "";
    error.hidden = !err;
  }

  /** Forgets the file choices, for a new link. */
  function forget() {
    linked = null;
    files.hidden = true;
    fileList.replaceChildren();
    fileAdd.disabled = true;
  }

  /** Counts link lookups, so one finishing late doesn't replace a newer hint. */
  let lookups = 0;

  function showHint(text) {
    hintEl.textContent = text ?? "";
    hintEl.hidden = !text;
  }

  /** Says what Enter will do: add from where, for a link. */
  async function renderHint() {
    const p = state.profile;
    const lookup = ++lookups;
    const link = asLink(input.value);
    if (!p || !link) return showHint(null);
    const source = await invokeQuietly("link_source", { game: p.game, link });
    if (lookup !== lookups) return;
    const from = source === "url" ? site(link) : source && sourceName(source);
    showHint(
      source === null
        ? "Not a link patchwork recognizes"
        : from
          ? `Add the mod from ${from}`
          : "Add the mod from this link",
    );
  }

  /** Shows the Add button only for a link. */
  function showMode() {
    submit.hidden = !asLink(input.value);
    onChange();
    renderHint().catch(() => {});
  }

  function clear() {
    input.value = "";
    forget();
    showError(null);
    showMode();
  }

  /** A file to pick, or a folder that lists its own when opened. */
  function choiceRow(c) {
    if (c.folder) return folderRow(c);
    return h(
      "label",
      { class: c.notMod ? "row link-file not-mod" : "row link-file" },
      h("input", { type: "radio", name: "file", value: c.id, required: true }),
      h(
        "span",
        {},
        c.name,
        c.size != null && h("small", { class: "muted" }, ` ${size(c.size)}`),
        c.notMod && h("small", { class: "muted" }, " (probably not a mod)"),
        c.description && h("br"),
        c.description && h("small", { class: "muted" }, c.description),
      ),
    );
  }

  function folderRow(c) {
    const list = h("div", { class: "link-folder-list stack", hidden: true });
    const toggle = h(
      "button",
      {
        type: "button",
        class: "link-folder-toggle",
        "aria-expanded": "false",
        onclick: async () => {
          const open = toggle.getAttribute("aria-expanded") !== "true";
          toggle.setAttribute("aria-expanded", String(open));
          list.hidden = !open;
          if (!open || list.childElementCount) return;
          list.append(h("small", { class: "muted" }, "Looking in the folder…"));
          const choices = await run("file_choices", {
            id: state.profile.id,
            source: linked.source,
            modId: c.id,
          }).catch((err) => {
            list.replaceChildren(h("small", { class: "error" }, String(err)));
            return null;
          });
          if (!choices) return;
          list.replaceChildren(
            ...(choices.length ? choices.map(choiceRow) : [h("small", { class: "muted" }, "Nothing in it")]),
          );
        },
      },
      `${c.name}/`,
    );
    return h("div", { class: "link-folder stack" }, toggle, list);
  }

  function showChoices(choices) {
    fileList.replaceChildren(...choices.map(choiceRow));
    files.hidden = false;
    fileList.querySelector("input, button")?.focus();
  }

  // a file picked, including one in an opened folder
  fileList.addEventListener("change", () => {
    fileAdd.disabled = !new FormData(form).get("file");
  });

  input.addEventListener("input", () => {
    forget();
    showError(null);
    showMode();
  });

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const p = state.profile;
    const link = asLink(input.value);
    if (!p || !link) return;
    submit.disabled = true;
    fileAdd.disabled = true;
    showError(null);
    try {
      let modId;
      if (linked) {
        modId = new FormData(form).get("file");
      } else {
        status("Looking up the link…");
        const found = await run("parse_link", { id: p.id, link });
        if (found.choices.length > 1 || found.choices[0]?.folder) {
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
      status(`Added ${added.name}`);
      if (state.profile?.id !== p.id) return;
      clear();
      input.focus();
      state.profile = added.profile;
      renderProfile();
      refreshDetails();
    } catch (err) {
      showError(err);
      // a link that didn't work is looked up again once it's changed
      if (!files.hidden) return;
      linked = null;
    } finally {
      submit.disabled = false;
      fileAdd.disabled = files.hidden || !new FormData(form).get("file");
    }
  });

  return clear;
}

const clears = [
  linkBox($("#mods-link"), () => boxChanged()),
  linkBox($("#browse-link"), () => browseChanged()),
];

/** Empties both link boxes, for another profile. */
export function clearLink() {
  for (const clear of clears) clear();
}
