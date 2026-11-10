// Adding a mod from a link pasted into a link box, like `patchwork add
// <link>`. There's one at the top of the Add mods tab (#browse-link), and the
// Mods tab's search box (#mods-link) is one too while what's in it looks like
// a link; each works on its own. The source that recognizes the link may have
// several files to choose between, like a GitHub release's; they're listed
// under the link, and the one picked is what later updates look for.

import { run, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { renderProfile } from "./profile.js";
import { refreshDetails } from "./details.js";
import { boxChanged } from "./sourcesearch.js";

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

/**
 * Makes `form` a link box, returning a function that empties it. With
 * `search`, its box is for searching too, and only adds a mod when what's
 * in it looks like a link, which is when its Add button shows; `onChange`
 * is called whenever what's in the box changes, including being emptied.
 */
function linkBox(form, { search = false, onChange = () => {} } = {}) {
  const input = form.querySelector(".link-url");
  const submit = form.querySelector(".link-submit");
  const files = form.querySelector(".link-files");
  const fileList = form.querySelector(".link-file-list");
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
  }

  /** Shows the Add button only for a link, in a search box. */
  function showMode() {
    if (search) submit.hidden = !asLink(input.value);
    onChange();
  }

  function clear() {
    input.value = "";
    forget();
    showError(null);
    showMode();
  }

  function showChoices(choices) {
    fileList.replaceChildren(
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
    files.hidden = false;
    fileList.querySelector("input")?.focus();
  }

  input.addEventListener("input", () => {
    forget();
    showError(null);
    showMode();
  });

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    const p = state.profile;
    const link = asLink(input.value) ?? (search ? "" : input.value.trim());
    if (!p || !link) return;
    submit.disabled = true;
    showError(null);
    try {
      let modId;
      if (linked) {
        modId = new FormData(form).get("file");
      } else {
        status("Looking up the link…");
        const found = await run("parse_link", { id: p.id, link });
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
    }
  });

  return clear;
}

const clears = [
  linkBox($("#mods-link"), { search: true, onChange: () => boxChanged() }),
  linkBox($("#browse-link")),
];

/** Empties both link boxes, for another profile. */
export function clearLink() {
  for (const clear of clears) clear();
}
