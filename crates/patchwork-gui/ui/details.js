import { listen, run, state, status } from "./state.js";
import {
  fillUpdate,
  fillVersions,
  renderModsSoon,
  summaryViews,
  updateNotes,
  versionSelects,
} from "./mods.js";
import { isHidden, modQuery } from "./toolbar.js";
import { markUpdates } from "./deploy.js";

// Versions and summaries. The profile comes with each mod's versions and
// summary as last fetched, so the dropdowns, cards and rows are complete straight
// away; refresh_details then fetches whichever are out of date in the
// background (see details.rs), and they're put in here as they come in.

/** Puts freshly fetched versions into a mod's dropdown. */
function showVersions(key, versions) {
  const m = state.profile?.mods.find((m) => m.key === key);
  if (!m) return;
  m.versions = versions;
  markUpdates();
  const note = updateNotes.get(key);
  if (note?.el.isConnected) fillUpdate(note.el, m, note.onVersion);
  const select = versionSelects.get(key);
  if (!select?.isConnected) return;
  // Replacing the options of an open dropdown closes it (or leaves the old
  // list showing), so wait until it's no longer in use.
  if (document.activeElement === select) {
    select.addEventListener("blur", () => fillVersions(select, m), { once: true });
  } else {
    fillVersions(select, m);
  }
}

/**
 * Puts a freshly fetched summary on a mod's card or row, or moves the mod to the
 * other section if its new tags change whether it's hidden.
 */
function showSummary(key, summary) {
  const m = state.profile?.mods.find((m) => m.key === key);
  if (!m) {
    // a dependency's, for its row in the Dependencies section
    const d = state.profile?.dependencies.find((d) => d.key === key && !d.inProfile);
    if (d) {
      d.summary = summary;
      summaryViews.get(key)?.();
    }
    return;
  }
  const wasHidden = isHidden(m);
  m.summary = summary;
  // its author and description may change where a search puts it
  if (isHidden(m) !== wasHidden || modQuery()) {
    renderModsSoon();
    return;
  }
  summaryViews.get(key)?.();
}

/** Has main.rs fetch whatever's out of date about the open profile's mods. */
export function refreshDetails() {
  run("refresh_details", { id: state.profile.id }).catch(() => {});
}

listen("patchwork://details", ({ payload: e }) => {
  switch (e.kind) {
    case "versions":
      if (e.game === state.profile?.game) showVersions(e.key, e.versions);
      return;
    case "summary":
      if (e.game === state.profile?.game) showSummary(e.key, e.summary);
      return;
    case "failed":
      for (const f of e.failures) console.warn(f);
      return status(e.message, true);
  }
});
