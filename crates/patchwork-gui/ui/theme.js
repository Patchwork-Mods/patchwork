// Themes and light/dark mode. Which theme applies is worked out by theme()
// in main.rs; this puts its CSS in the <style> elements in index.html, each
// in its own cascade layer so a theme beats the built-in look whatever its
// selectors are, and user.css (in no layer) beats everything. The open
// game's accent colour goes between the picked theme and the game's own.
// The mode goes on <html> as data-mode="light|dark" for CSS to target.
//
// Settings → Appearance has the theme picker: one button per theme for
// every game, each drawn in that theme's colours, which shows the theme on
// the whole window while hovered or focused. A game's settings pick a
// theme for just that game.
//
// main.rs watches the themes folders and says when a file in them changes,
// and the themes are applied again, so a theme shows as it's saved.

import { invoke, listen, run, status } from "./state.js";
import { $, h } from "./dom.js";

const systemDark = matchMedia("(prefers-color-scheme: dark)");

/** The last theme() answer, for working the mode out again and for putting
 *  things back after a preview. */
let current = null;

const layer = (name, css) => (css ? `@layer ${name} {\n${css}\n}` : "");

/** Applies the themes for `game` (or none, for All games). */
export async function applyTheme(game) {
  document.body.dataset.game = game ?? "";
  const theme = await invoke("theme", { game });
  current = theme;
  paint(theme);
  if (theme.problems.length) status(theme.problems.join("; "), true);
}

/** Puts `theme` (a theme() answer, or a preview made from one) on the page. */
function paint(theme) {
  applyMode(theme);
  // Which themes are in use, for user.css to target with [data-theme=...].
  const root = document.documentElement;
  root.dataset.theme = theme.global?.id ?? "";
  root.dataset.gameTheme = theme.game?.id ?? "";
  $("#theme-builtin").textContent =
    "@layer pw-builtin, pw-theme, pw-accent, pw-game;\n" + layer("pw-builtin", theme.builtin);
  $("#theme-global").textContent = layer("pw-theme", theme.global?.css);
  // Checked to be a hex colour by patchwork, so safe to put in CSS.
  const accent = theme.accent;
  $("#theme-accent").textContent = accent
    ? layer("pw-accent", `:root { --pw-accent: ${accent.color}; --pw-accent-text: ${accent.text}; }`)
    : "";
  $("#theme-game").textContent = layer("pw-game", theme.game?.css);
  $("#theme-user").textContent = theme.user ?? "";
}

/** The mode to show: the one picked, unless the themes in use only have one. */
function modeFor(picked, modes) {
  if (picked === "system") picked = systemDark.matches ? "dark" : "light";
  return modes.length && !modes.includes(picked) ? modes[0] : picked;
}

/** The modes `themes` have looks for together, as Resolved::modes in
 *  themes.rs: the later one wins where they disagree. */
function modesOf(themes) {
  let modes = null;
  for (const theme of themes) {
    if (!theme?.modes.length) continue;
    const both = modes ? modes.filter((m) => theme.modes.includes(m)) : [];
    modes = both.length ? both : theme.modes;
  }
  return modes ?? [];
}

function applyMode(theme = current) {
  if (!theme) return;
  const mode = modeFor(theme.mode, theme.modes);
  document.documentElement.dataset.mode = mode;
  // The title bar too, where the system draws one. Following the system
  // when nothing forces a mode keeps the media query above honest.
  const forced = theme.mode !== "system" || theme.modes.length === 1;
  window.__TAURI__.window
    .getCurrentWindow()
    .setTheme(forced ? mode : null)
    .catch(() => {});
}

systemDark.addEventListener("change", () => {
  applyMode(previewing === undefined ? current : previewFor(previewing));
  if (appearance.closest("dialog").open && prefs) renderPicker();
});

// Previews.

/** The theme being previewed: a theme_list entry, null for the built-in
 *  look, or undefined when nothing is. */
let previewing;

/** `current` with `candidate` in place of the picked theme. A game theme
 *  that would replace it is left out, so there's something to see. */
function previewFor(candidate) {
  const game = current.game?.layer ? current.game : null;
  return { ...current, global: candidate, game, modes: modesOf([candidate, game]) };
}

function preview(candidate) {
  if (!current || previewing === candidate) return;
  previewing = candidate;
  paint(previewFor(candidate));
  describe(candidate);
}

function endPreview() {
  clearTimeout(hoverTimer);
  if (previewing === undefined || !current) return;
  previewing = undefined;
  paint(current);
  describe(picked());
}

// A theme file changed: apply the themes again, keeping any preview going
// with the theme's new CSS.
listen("patchwork://themes-changed", () => refresh().catch(() => {}));

async function refresh() {
  const theme = await invoke("theme", { game: document.body.dataset.game || null });
  current = theme;
  if (theme.problems.length) status(theme.problems.join("; "), true);
  if (appearance.closest("dialog").open) {
    const id = previewing?.id;
    await renderAppearance();
    if (previewing !== undefined) {
      // The new copy of the theme being previewed, if it's still there.
      previewing = previewing && (themes.find((t) => t.id === id) ?? null);
      paint(previewFor(previewing));
      describe(previewing);
      return;
    }
  }
  // A game's settings may be showing a theme picked there and not saved
  // yet; that dialog puts `current` back when it closes.
  if (gameThemeSelect.closest("dialog").open) return;
  paint(current);
  renderModeNote();
}

// Settings → Appearance.

const appearance = $("#appearance");
const picker = $("#theme-picker");
const modeSelect = $("#appearance-mode");
const gameThemesSelect = $("#appearance-game-themes");
/** gui.toml's [theme] table as last read, sent back with changes. */
let prefs = null;
/** The themes for every game, from theme_list. */
let themes = [];
/** The built-in look's --pw-* variables come under every sample, standing
 *  in for the ones a theme doesn't set. */
let builtinCss = "";

/** The theme picked, or null for the built-in look. */
const picked = () => themes.find((t) => t.id === prefs?.theme) ?? null;

export async function renderAppearance() {
  const list = await invoke("theme_list");
  prefs = list.prefs;
  themes = list.themes.filter((t) => !t.game);
  builtinCss = current?.builtin ?? "";
  modeSelect.value = prefs.mode;
  gameThemesSelect.value = prefs.game_themes;
  renderPicker();
  renderModeNote();
  const problems = $("#appearance-problems");
  problems.textContent = list.problems.join("; ");
  problems.hidden = !list.problems.length;
}

/** The `--pw-*` variables `css` sets on :root in `mode`, in the order the
 *  rules come, so a :root[data-mode=dark] rule after :root overrides it.
 *  Rules inside @media and the like count too, which is near enough for a
 *  sample. */
function rootVars(css, mode) {
  const vars = {};
  const rootSelector = new RegExp(`^:root(\\[data-mode=(["']?)${mode}\\2\\])?$`);
  const text = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const [, selectors, body] of text.matchAll(/([^{}]*)\{([^{}]*)\}/g)) {
    if (!selectors.split(",").some((s) => rootSelector.test(s.trim()))) continue;
    for (const [, name, value] of body.matchAll(/(--pw-[\w-]+)\s*:\s*([^;]+)/g)) {
      vars[name] = value.trim();
    }
  }
  return vars;
}

/** A small drawing of the app in `theme`'s colours (null: the built-in
 *  look), in the mode it would show in. */
function sample(theme) {
  const mode = modeFor(prefs.mode, theme?.modes ?? []);
  const vars = { ...rootVars(builtinCss, mode), ...rootVars(theme?.css ?? "", mode) };
  const el = h(
    "span",
    { class: "theme-sample", "aria-hidden": "true" },
    h("span", { class: "theme-sample-sidebar" }, h("i"), h("i"), h("i")),
    h(
      "span",
      { class: "theme-sample-main" },
      h("span", { class: "theme-sample-card" }, "Aa", h("span", { class: "row" }, h("s"), h("b", {}, "Play"))),
    ),
  );
  for (const [name, value] of Object.entries(vars)) el.style.setProperty(name, value);
  return el;
}

/** Says which modes a theme has, if it doesn't have both. */
function modesBadge(theme) {
  const modes = theme?.modes ?? [];
  return modes.length === 1 ? h("span", { class: "badge" }, `${modes[0]} only`) : null;
}

let hoverTimer;
/** Set while the picker moves focus itself, which isn't asking for a preview. */
let quiet = false;

function renderPicker() {
  const chosen = picked();
  const choices = [null, ...themes].map((theme) => {
    const checked = theme === chosen;
    const button = h(
      "button",
      {
        type: "button",
        role: "radio",
        class: "theme-choice",
        "aria-checked": String(checked),
        tabindex: checked ? "0" : "-1",
        "data-theme-id": theme?.id ?? "default",
        title: theme?.description,
        onclick: () => pick(theme).catch(() => {}),
        onfocus: () => {
          if (!quiet) preview(theme);
        },
      },
      sample(theme),
      h(
        "span",
        { class: "theme-choice-name" },
        theme?.name ?? "Default",
        theme?.origin === "user" ? h("span", { class: "badge" }, "yours") : null,
        modesBadge(theme),
      ),
    );
    button.theme = theme;
    return button;
  });
  picker.replaceChildren(...choices);
  describe(previewing === undefined ? chosen : previewing);
  renderGameNote();
}

/** The description under the picker, for the theme shown. */
function describe(theme) {
  $("#theme-picker-description").textContent =
    theme === null ? "Patchwork's own look." : (theme?.description ?? "");
}

/** Says so when the open game uses another theme than the picked one. */
function renderGameNote() {
  const note = $("#theme-picker-note");
  const game = current?.game;
  const inUse = game && !game.layer ? game : current?.global;
  note.hidden = !current || (inUse?.id ?? null) === (picked()?.id ?? null);
  if (!note.hidden) {
    note.textContent = `While this game is open, it uses ${inUse ? `${inUse.name}` : "the default look"} instead.`;
  }
}

async function pick(theme) {
  clearTimeout(hoverTimer);
  previewing = undefined;
  await savePrefs({ theme: theme?.id ?? null });
  renderPicker();
  quiet = true;
  picker.querySelector("[aria-checked=true]")?.focus({ focusVisible: false });
  quiet = false;
}

// Hover previews follow the pointer moving, not the pointer being over a
// button: a theme can change spacing and fonts, which moves the buttons, and
// a preview that followed whatever ended up under a still pointer could
// flip back and forth. The short wait keeps passing over buttons from
// repainting the window for each one.
picker.addEventListener("pointermove", (e) => {
  const button = e.target.closest(".theme-choice");
  clearTimeout(hoverTimer);
  if (!button) return;
  hoverTimer = setTimeout(() => preview(button.theme), 80);
});
// Back to the theme focused from the keyboard, if any, otherwise the one in use.
picker.addEventListener("pointerleave", () => {
  const focused = picker.querySelector(".theme-choice:focus-visible");
  if (focused) preview(focused.theme);
  else endPreview();
});
picker.addEventListener("focusout", (e) => {
  if (!picker.contains(e.relatedTarget) && !picker.matches(":hover")) endPreview();
});
// Arrow keys move between themes, as in any radio group; Space and Enter
// pick, being buttons.
picker.addEventListener("keydown", (e) => {
  const step = { ArrowRight: 1, ArrowDown: 1, ArrowLeft: -1, ArrowUp: -1 }[e.key];
  if (!step) return;
  e.preventDefault();
  const buttons = [...picker.querySelectorAll(".theme-choice")];
  const at = buttons.indexOf(document.activeElement);
  buttons[(at + step + buttons.length) % buttons.length].focus();
});
appearance.closest("dialog").addEventListener("close", endPreview);

/** Says so when the themes in use only have one mode. */
function renderModeNote() {
  const note = $("#appearance-mode-note");
  const only = current?.modes.length === 1 ? current.modes[0] : null;
  const name = current?.game?.name ?? current?.global?.name;
  note.hidden = !only;
  if (only) note.textContent = `The theme in use${name ? ` (${name})` : ""} only has a ${only} look.`;
}

async function savePrefs(change) {
  prefs = { ...prefs, ...change };
  await run("set_theme_prefs", { prefs });
  await applyTheme(document.body.dataset.game || null);
  renderModeNote();
}

modeSelect.addEventListener("change", () =>
  savePrefs({ mode: modeSelect.value })
    .then(renderPicker)
    .catch(() => {}),
);
gameThemesSelect.addEventListener("change", () =>
  savePrefs({ game_themes: gameThemesSelect.value })
    .then(renderPicker)
    .catch(() => {}),
);

// A game's settings: the theme for just that game, a value of gui.toml's
// [theme.games] ("" for Automatic). Picking one shows it straight away;
// it's saved with the rest of the game's settings.

const gameThemeSelect = $("#game-settings-theme");
let gameThemeFor = null;
/** What each of the game's Theme options does, by value. */
let gameThemeNotes = {};

/** Fills in the game settings' Theme for `game`. */
export async function renderGameTheme(game) {
  gameThemeFor = game;
  const [list, automatic] = await Promise.all([
    invoke("theme_list"),
    invoke("theme", { game, pick: "" }),
  ]);
  prefs = list.prefs;
  // The theme picked in Settings → Appearance, by name.
  const usual = list.themes.find((t) => t.id === prefs.theme && !t.game)?.name ?? "Default";
  const own = list.themes.filter((t) => t.game === game);
  const others = list.themes.filter((t) => !t.game);

  // Automatic says what it comes to right now.
  const gameTheme = automatic.game;
  const automaticShows = gameTheme
    ? gameTheme.layer
      ? `${usual} with ${gameTheme.name}`
      : gameTheme.name
    : automatic.accent
      ? `${usual} in the game's colour`
      : usual;
  gameThemeNotes = {
    "": "Follows Game themes in Settings → Appearance: the game's own theme when it has one, otherwise its colour on your usual theme.",
    none: `Your usual theme (now ${usual}), changing when you change it, without this game's ${gameTheme ? "theme" : "colour"}.`,
    default: "Patchwork's own look while this game is open, whatever your usual theme is.",
  };
  for (const t of own) gameThemeNotes[t.id] = "This game theme, even when Game themes is turned off in Settings → Appearance.";
  for (const t of others) gameThemeNotes[t.id] = `${t.name} while this game is open, whatever your usual theme is.`;

  const option = (t) => h("option", { value: t.id }, t.name);
  const set = prefs.games?.[game] ?? "";
  const known = Object.hasOwn(gameThemeNotes, set);
  // Leaving the game's look out only differs from Automatic when there's some.
  const showNone = gameTheme || automatic.accent || set === "none";
  gameThemeSelect.replaceChildren(
    h("option", { value: "" }, `Automatic: ${automaticShows}`),
    showNone ? h("option", { value: "none" }, `${usual} (your usual theme), without the game's ${gameTheme ? "theme" : "colour"}`) : null,
    own.length ? h("optgroup", { label: "This game's themes" }, ...own.map(option)) : null,
    h(
      "optgroup",
      { label: own.length || showNone ? "Other themes" : "Themes" },
      h("option", { value: "default" }, "Default"),
      ...others.map(option),
    ),
    known ? null : h("option", { value: set }, `${set} (missing)`),
  );
  gameThemeSelect.value = set;
  describeGameTheme();
}

function describeGameTheme() {
  $("#game-settings-theme-note").textContent =
    (Object.hasOwn(gameThemeNotes, gameThemeSelect.value) && gameThemeNotes[gameThemeSelect.value]) || "There's no theme with this name any more.";
}

gameThemeSelect.addEventListener("change", async () => {
  describeGameTheme();
  const preview = await invoke("theme", { game: gameThemeFor, pick: gameThemeSelect.value });
  if (gameThemeSelect.closest("dialog").open) paint(preview);
});

/** Saves the game settings' Theme. */
export async function saveGameTheme() {
  const games = { ...prefs.games };
  if (gameThemeSelect.value) games[gameThemeFor] = gameThemeSelect.value;
  else delete games[gameThemeFor];
  await savePrefs({ games });
}

// Back to the theme in use, after a preview that wasn't saved.
gameThemeSelect.closest("dialog").addEventListener("close", () => {
  if (current) paint(current);
});
