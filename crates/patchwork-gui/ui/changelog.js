// Mods' changelogs: a mod's whole history, from the Changelog button on its
// list row, and the notes deploy.js shows before an update.

import { run, state } from "./state.js";
import { $, h, link, openExternal } from "./dom.js";

/**
 * Changelog entries (from changelog.rs) as elements. Their HTML has been
 * made safe there, and links in it open in the browser rather than the app.
 */
export function renderEntries(entries) {
  return entries.map((e) => {
    const notes = h("div", { class: "changelog-notes" });
    notes.innerHTML = e.html;
    notes.addEventListener("click", (event) => {
      const a = event.target.closest("a[href]");
      if (!a) return;
      event.preventDefault();
      openExternal(a.href);
    });
    return h("section", { class: "changelog-entry" }, e.version && h("h4", {}, e.version), notes);
  });
}

/** Which mod #changelog-dialog was last opened for, so a slow fetch for another is dropped. */
let showing = null;

/** Opens #changelog-dialog on `m`'s whole changelog, newest first. */
export async function showChangelog(m) {
  const key = m.key;
  showing = key;
  const body = $("#changelog-body");
  $("#changelog-title").textContent = `${m.name} changelog`;
  body.replaceChildren(h("p", { class: "muted" }, "Loading…"));
  $("#changelog-dialog").showModal();
  let entries;
  try {
    entries = await run("mod_changelog", { id: state.profile.id, key });
  } catch (e) {
    if (showing === key) body.replaceChildren(h("p", { class: "error" }, String(e)));
    return;
  }
  if (showing !== key) return;
  const url = m.summary?.url;
  body.replaceChildren(
    ...(entries.length
      ? renderEntries(entries)
      : [
          h(
            "p",
            { class: "muted" },
            `${m.name} doesn't have a changelog here.`,
            url && [" ", link(url, "See the mod's page")],
          ),
        ]),
  );
}
