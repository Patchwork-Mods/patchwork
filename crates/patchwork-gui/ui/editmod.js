// The Edit dialog for a mod: how one of the profile's mods is shown (its
// name, picture, description, tags, ...) in place of what its source says.
//
// The fields come from the backend (`mod_fields`, see core's edits.rs), so
// a field added there turns up here without changes; editfields.js builds
// them.

import { invoke, state, status } from "./state.js";
import { $ } from "./dom.js";
import { showProfile } from "./profile.js";
import { changesOf, fieldView, resetAll, wireForm } from "./editfields.js";

const dialog = $("#edit-mod-dialog");
const form = $("#edit-mod-form");
const fieldsEl = $("#edit-mod-fields");
const errorEl = $("#edit-mod-error");
/** The mod the dialog is about, the profile it's in, and its fields' views. */
let open = null;

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
  open = { id, name: m.name, views: [] };
  open.views = fields.map((f) => fieldView(f, "edit-mod", updateResetAll));
  fieldsEl.replaceChildren(...open.views.map((v) => v.el));
  errorEl.hidden = true;
  updateResetAll();
  dialog.showModal();
  open.views[0]?.control.focus();
}

/** Only offers to put every field back when some have changed. */
function updateResetAll() {
  $("#edit-mod-reset").disabled = !open?.views.some((v) => v.changed());
}

wireForm(dialog, form, errorEl, async () => {
  const { id, name, views } = open;
  const profile = await invoke("set_mod_edits", { id, name, changes: changesOf(views) });
  dialog.close();
  if (state.profile?.id === id) showProfile(profile);
  status(`Saved changes to ${name}`);
});

$("#edit-mod-reset").addEventListener("click", () => resetAll(open?.views ?? []));
