// The Config tab, for games with config files (like the ones BepInEx makes
// for each mod): every config file the profile could deploy, each opening
// the options dialog (form.js) on its settings. The profile only keeps the
// settings changed, and deploying applies them; settings changed in game
// are read back before the next deploy. Which files are config, and how
// they're read, is up to the game's data and extensions.

import { $, h } from "./dom.js";
import { run, state } from "./state.js";
import { showConfig } from "./form.js";
import { showProfile } from "./profile.js";

/** The tab key for the Config tab. */
export const CONFIG_TAB = "config";

/** The latest load, so an older one finishing late is ignored. */
let loads = 0;
/** The files last loaded, for filtering without loading again. */
let files = [];

/** Loads and shows the open profile's config files. */
export async function openConfig() {
  const p = state.profile;
  if (!p?.hasConfig) return;
  const seq = ++loads;
  $("#config-files").replaceChildren(h("li", { class: "muted" }, "Looking for config files…"));
  let found;
  try {
    found = await run("config_files", { id: p.id });
  } catch (e) {
    if (seq === loads) $("#config-files").replaceChildren(h("li", { class: "error" }, String(e)));
    return;
  }
  if (seq !== loads || state.profile?.id !== p.id || state.tab !== CONFIG_TAB) return;
  files = found;
  renderConfig();
}

function renderConfig() {
  const filter = $("#config-filter").value.trim().toLowerCase();
  const shown = files.filter(
    (f) =>
      !filter ||
      [f.path, f.title, f.mod].some((s) => s?.toLowerCase().includes(filter)),
  );
  $("#config-empty").hidden = files.length > 0;
  $("#config-files").replaceChildren(
    ...shown.map((f) =>
      h(
        "li",
        { class: f.missing ? "config-file missing" : "config-file" },
        h(
          "button",
          {
            type: "button",
            class: "config-open",
            disabled: f.missing,
            title: f.missing
              ? "It isn't there now, so the settings changed in it aren't applied"
              : "Change its settings",
            onclick: () => showConfig(f),
          },
          h("span", { class: "config-title" }, f.title ?? f.path.split("/").pop()),
          h("span", { class: "config-path muted" }, f.path),
        ),
        f.mod && h("span", { class: "badge" }, f.mod),
        f.edited && h("span", { class: "badge config-edited" }, f.missing ? "Not there" : "Changed"),
        f.edited &&
          f.missing &&
          h(
            "button",
            {
              type: "button",
              class: "config-forget",
              onclick: () => forget(f),
            },
            "Forget",
          ),
      ),
    ),
  );
}

/** Forgets the settings changed in a file that isn't there any more. */
async function forget(f) {
  // Showing the profile again loads the tab again.
  showProfile(await run("set_config", { id: state.profile.id, path: f.path, answers: null }));
}

$("#config-filter").addEventListener("input", renderConfig);
