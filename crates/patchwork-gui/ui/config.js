// The Config tab, for games with config files (like the ones BepInEx makes
// for each mod): every config file the profile could deploy, each opening
// the options dialog (form.js) on its settings. The profile only keeps the
// settings changed, and deploying applies them; settings changed in game
// are read back before the next deploy. Which files are config, and how
// they're read, is up to the game's data and extensions.

import { $, gearIcon, h } from "./dom.js";
import { run, state } from "./state.js";
import { showConfig, showOptions } from "./form.js";
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
      [f.path, f.title, f.mod, f.owner].some((s) => s?.toLowerCase().includes(filter)),
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
        (f.mod ?? f.owner) &&
          h(
            "span",
            {
              class: "badge",
              title: f.mod ? `${f.mod}'s copy` : `Named like ${f.owner}, so probably made by it`,
            },
            f.mod ?? f.owner,
          ),
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

// Each mod's own settings button, on its card or list row. It covers all of
// the mod's settings: its sets of choices (like SMF's options or a FOMOD
// installer, from the profile) and the config files for it (see `owner` in
// config_files), which are loaded in the background after the profile is,
// like its conflicts. It only shows when there's something. One thing opens
// straight away; more open a menu of them.

/** Each mod's config files by name, for the profile they were loaded for. */
let owned = { id: null, byMod: new Map() };

/** The settings button of each mod on screen, and the mod, by name. */
const buttons = new Map();

/** The latest owner load, so an older one finishing late is ignored. */
let ownerLoads = 0;

/** Loads which config files are each of the open profile's mods'. */
export async function loadModConfigs() {
  const p = state.profile;
  if (!p) return;
  const seq = ++ownerLoads;
  markShown();
  let list = [];
  if (p.hasConfig) {
    try {
      list = await run("config_files", { id: p.id });
    } catch {
      // the game isn't set up, say; the Config tab tells why
    }
  }
  if (seq !== ownerLoads || state.profile?.id !== p.id) return;
  const byMod = new Map();
  for (const f of list) {
    if (f.owner && !f.missing) byMod.set(f.owner, [...(byMod.get(f.owner) ?? []), f]);
  }
  owned = { id: p.id, byMod };
  markShown();
  for (const [name, { button, m }] of buttons) {
    if (button.isConnected) fillSettings(button, m);
    else buttons.delete(name);
  }
}

/** List rows keep a gap where a mod has no settings, once any has, so their
 * cogs and switches line up. */
function markShown() {
  const mods = state.profile?.mods ?? [];
  const any = mods.some((m) => m.options?.length) || filesOwned().size > 0;
  document.body.classList.toggle("mod-settings-shown", any);
}

const filesOwned = () => (owned.id === state.profile?.id ? owned.byMod : new Map());

/** Everything `m` has settings in, each with a label and a way to open it. */
function settingsOf(m) {
  const fileName = (f) => f.title ?? f.path.split("/").pop();
  const all = [
    ...(m.options ?? []).map((o) => ({
      label: o.title,
      about: `Pick the options in its ${o.title.toLowerCase()}`,
      open: () => showOptions(m, o),
    })),
    ...(filesOwned().get(m.name) ?? []).map((f) => ({
      label: fileName(f),
      about: f.edited ? `${f.path}, changed in this profile` : f.path,
      edited: f.edited,
      open: () => showConfig(f),
      file: f.path.split("/").pop(),
    })),
  ];
  // two files with the same title are told apart by their names
  for (const s of all) {
    if (s.file && all.filter((t) => t.label === s.label).length > 1) {
      s.shown = `${s.label} (${s.file})`;
    }
  }
  for (const s of all) s.label = s.shown ?? s.label;
  return all;
}

/** A mod's settings button, hidden while it has none; with `label`, it says "Settings". */
export function settingsButton(m, { label = false } = {}) {
  const button = h(
    "button",
    { type: "button", class: "mod-settings", hidden: true, "aria-haspopup": "menu" },
    gearIcon(),
    label && h("span", {}, "Settings"),
  );
  button.addEventListener("click", (e) => {
    // it sits on a list row, which expands when clicked
    e.stopPropagation();
    const all = settingsOf(m);
    if (all.length === 1) all[0].open();
    else if (all.length > 1) openMenu(button, all);
  });
  buttons.set(m.name, { button, m });
  fillSettings(button, m);
  return button;
}

function fillSettings(button, m) {
  const all = settingsOf(m);
  button.hidden = !all.length;
  button.classList.toggle("edited", all.some((s) => s.edited));
  button.title =
    all.length === 1
      ? `${m.name}'s settings: ${all[0].label} (${all[0].about})`
      : `${m.name}'s settings: ${all.map((s) => s.label).join(", ")}`;
  button.setAttribute("aria-label", button.title);
}

// The menu a settings button opens when there's more than one thing. It's
// put on the page itself, so a card's edges don't cut it off.

/** The open menu, if any. */
let menu = null;

function closeMenu() {
  menu?.remove();
  menu = null;
}

function openMenu(button, all) {
  closeMenu();
  menu = h(
    "div",
    { class: "settings-menu", role: "menu" },
    all.map((s) =>
      h(
        "button",
        {
          type: "button",
          role: "menuitem",
          class: s.edited ? "edited" : null,
          title: s.about,
          onclick: () => {
            closeMenu();
            s.open();
          },
        },
        s.label,
      ),
    ),
  );
  document.body.append(menu);
  // below the button, or above it if there isn't room, kept on screen
  const at = button.getBoundingClientRect();
  const { offsetWidth: w, offsetHeight: ht } = menu;
  const left = Math.max(4, Math.min(at.right - w, innerWidth - w - 4));
  const top = at.bottom + 4 + ht > innerHeight ? Math.max(4, at.top - 4 - ht) : at.bottom + 4;
  menu.style.left = `${left}px`;
  menu.style.top = `${top}px`;
  menu.querySelector("button").focus();
}

document.addEventListener("click", (e) => {
  if (menu && !menu.contains(e.target)) closeMenu();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && menu) closeMenu();
});
// it'd be left behind on scrolling
document.addEventListener("scroll", closeMenu, true);
