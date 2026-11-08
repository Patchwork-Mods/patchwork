// Which versions of the game a mod works with. Mods that say they don't work
// with the installed version get a warning; this says otherwise, in place of
// what the mod's source says (Nexus Mods' file names, say).

import { invoke, state, status } from "./state.js";
import { $ } from "./dom.js";
import { showProfile } from "./profile.js";

const form = $("#game-versions-form");
const dialog = $("#game-versions-dialog");
/** The mod the dialog is about, and the profile it's in. */
let open = null;

/** What the mod is said to work with, by the profile or else by its source, or null. */
export function gameVersionsOf(m) {
  const own = m.game_versions;
  if (own?.length) return { versions: own, set: true };
  if (m.sourceGameVersions?.length) return { versions: m.sourceGameVersions, set: false };
  return null;
}

/** `["1.5.97", "1.6.*"]` as "1.5.97 or 1.6.*". */
export const describeVersions = (versions) => versions.join(" or ");

/** Opens the dialog for mod `m` of the open profile. */
export function showGameVersions(m) {
  open = { id: state.profile.id, name: m.name };
  form.reset();
  $("#game-versions-title").textContent = `Game versions for ${m.name}`;
  const source = m.sourceGameVersions?.length
    ? `Its source says it works with ${describeVersions(m.sourceGameVersions)}.`
    : "Its source doesn't say which versions it works with.";
  $("#game-versions-source").textContent =
    m.game_versions?.length && m.sourceGameVersions?.length
      ? `${source} The profile says otherwise.`
      : source;
  form.elements.versions.value = (m.game_versions ?? []).join(", ");
  $("#game-versions-reset").hidden = !m.game_versions?.length;
  $("#game-versions-error").hidden = true;
  dialog.showModal();
  form.elements.versions.focus();
}

/** Saves `versions` (null goes back to the source's) and shows the profile. */
async function save(versions) {
  const { id, name } = open;
  const profile = await invoke("set_game_versions", { id, name, versions });
  dialog.close();
  if (state.profile?.id === id) showProfile(profile);
  status(
    versions
      ? `${name} works with ${describeVersions(versions)}`
      : `${name} goes by what its source says about game versions`,
  );
}

async function attempt(versions) {
  try {
    await save(versions);
  } catch (err) {
    $("#game-versions-error").textContent = String(err);
    $("#game-versions-error").hidden = false;
  }
}

form.addEventListener("submit", (e) => {
  e.preventDefault();
  const versions = form.elements.versions.value
    .split(",")
    .map((v) => v.trim())
    .filter(Boolean);
  // leaving it empty means not saying anything of its own
  attempt(versions.length ? versions : null);
});

$("#game-versions-reset").addEventListener("click", () => attempt(null));

form.querySelector("[value=cancel]").addEventListener("click", () => dialog.close());
