// Themes and light/dark mode. Which theme applies is worked out by theme()
// in main.rs; this puts its CSS in the <style> elements in index.html, each
// in its own cascade layer so a theme beats the built-in look whatever its
// selectors are, and user.css (in no layer) beats everything. The open
// game's accent color goes between the picked theme and the game's own.
// The mode goes on <html> as data-mode="light|dark" for CSS to target.
//
// Settings → Appearance has the theme picker: one button per theme for
// every game, each drawn in that theme's colors, which shows the theme on
// the whole window while hovered or focused. A game's settings pick a
// theme for just that game.
//
// A theme can offer options ([[option]] in its header, see
// theme_options.rs), shown under the picker for the themes in use and in a
// game's settings for its theme. Each sets a CSS variable in a layer just
// after its theme's, and toggles and choices set data-opt-<id> on <html>.
// "Reduce motion", or the system asking for less, sets data-motion="reduced"
// on <html> for themes to stop what moves.
//
// main.rs watches the themes folders and says when a file in them changes,
// and the themes are applied again, so a theme shows as it's saved.

import { invoke, listen, run, status } from "./state.js";
import { $, h } from "./dom.js";
import { colorPicker, fullHex } from "./colorpick.js";
import { autosave } from "./autosave.js";

const systemDark = matchMedia("(prefers-color-scheme: dark)");
const systemReduced = matchMedia("(prefers-reduced-motion: reduce)");

/** The last theme() answer, for working the mode out again and for putting
 *  things back after a preview. */
let current = null;

const layer = (name, css) => (css ? `@layer ${name} {\n${css}\n}` : "");

/** theme() answers by game ("" for none), so that moving between screens
 *  can paint straight away. Each is checked again in the background after
 *  being used, and the cache is cleared when themes or their settings change. */
const cache = new Map();

/** Applies the themes for `game` (or none, for All games). Paints at once
 *  from the cache when it can, returning without waiting on main.rs. */
export async function applyTheme(game) {
  const key = game ?? "";
  document.body.dataset.game = key;
  const cached = cache.get(key);
  if (cached) {
    current = cached;
    paint(cached);
    fetchTheme(key).catch(() => {});
    return;
  }
  await fetchTheme(key);
}

async function fetchTheme(key) {
  const theme = await invoke("theme", { game: key || null });
  const was = cache.get(key);
  cache.set(key, theme);
  // Another game may have been opened meanwhile, or nothing changed.
  if (document.body.dataset.game !== key) return;
  if (was && JSON.stringify(was) === JSON.stringify(theme)) return;
  current = theme;
  paint(theme);
  if (theme.problems.length) status(theme.problems.join("; "), true);
}

/** An option's value in `theme`: as picked, or its default. */
function valueOf(theme, option) {
  const value = theme.values?.[option.id];
  if (value !== undefined) return value;
  if (option.type === "choice") return option.default ?? option.choices[0].value;
  return option.default;
}

/** What an option puts in its CSS variable, with `value` picked. */
function optionCss(option, value) {
  switch (option.type) {
    case "toggle":
      return value ? option.on : option.off;
    case "choice": {
      const choice = option.choices.find((c) => c.value === value);
      return choice && (choice.css ?? choice.value);
    }
    case "color":
      return value;
    case "range":
      return `${value}${option.unit ?? ""}`;
  }
}

/** `theme`'s options as CSS variables on :root, in layer `name`. The
 *  values were checked by theme_options.rs, or come from our own controls. */
function optionsLayer(name, theme) {
  const decls = (theme?.options ?? [])
    .filter((o) => o.var)
    .map((o) => [o.var, optionCss(o, valueOf(theme, o))])
    .filter(([, css]) => css != null)
    .map(([name, css]) => `  ${name}: ${css};\n`)
    .join("");
  return decls ? `\n@layer ${name} {\n:root {\n${decls}}\n}` : "";
}

/** data-opt-<id> on <html> for each toggle and choice of `themes`. */
function optionAttrs(themes) {
  const root = document.documentElement;
  for (const { name } of [...root.attributes]) {
    if (name.startsWith("data-opt-")) root.removeAttribute(name);
  }
  for (const theme of themes) {
    for (const option of theme?.options ?? []) {
      const value = valueOf(theme, option);
      if (option.type === "toggle") root.setAttribute(`data-opt-${option.id}`, value ? "on" : "off");
      if (option.type === "choice") root.setAttribute(`data-opt-${option.id}`, value);
    }
  }
}

function applyMotion(theme = current) {
  const reduced = systemReduced.matches || !!theme?.reduce_motion;
  document.documentElement.dataset.motion = reduced ? "reduced" : "full";
}
systemReduced.addEventListener("change", () => applyMotion());

/** Puts `theme` (a theme() answer, or a preview made from one) on the page. */
function paint(theme) {
  applyMode(theme);
  applyMotion(theme);
  // Which themes are in use, for user.css to target with [data-theme=...].
  const root = document.documentElement;
  root.dataset.theme = theme.global?.id ?? "";
  root.dataset.gameTheme = theme.game?.id ?? "";
  optionAttrs([theme.global, theme.game]);
  $("#theme-builtin").textContent =
    "@layer pw-builtin, pw-theme, pw-theme-options, pw-accent, pw-game, pw-game-options;\n" +
    layer("pw-builtin", theme.builtin);
  $("#theme-global").textContent =
    layer("pw-theme", theme.global?.css) + optionsLayer("pw-theme-options", theme.global);
  // Checked to be a hex color by patchwork, so safe to put in CSS.
  const accent = theme.accent;
  $("#theme-accent").textContent = accent
    ? layer("pw-accent", `:root { --pw-accent: ${accent.color}; --pw-accent-text: ${accent.text}; }`)
    : "";
  $("#theme-game").textContent =
    layer("pw-game", theme.game?.css) + optionsLayer("pw-game-options", theme.game);
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
  cache.clear();
  const key = document.body.dataset.game;
  const theme = await invoke("theme", { game: key || null });
  cache.set(key, theme);
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

// Theme options' controls.

/** A range's value as shown beside it. */
const rangeText = (option, value) => `${Number(value.toFixed(2))}${option.unit ?? ""}`;

/** How far along a slider is, 0 to 1, as --pw-range on it, so a theme can
 *  draw the line before the knob differently from the line after it. */
const rangeAt = (option, value) => `--pw-range: ${(value - option.min) / (option.max - option.min)}`;

/** A control for each of `theme`'s options. `set(option, value, done)` is
 *  called as one changes; `done` is false while a slider or color is still
 *  being moved. */
function optionControls(theme, set) {
  return theme.options.map((option) => {
    const value = valueOf(theme, option);
    const hint = option.description ? h("small", { class: "muted" }, option.description) : null;
    const attrs = { "data-option": option.id };
    switch (option.type) {
      case "toggle":
        return h(
          "div",
          { class: "field theme-option" },
          h(
            "label",
            { class: "row" },
            h("input", { ...attrs, type: "checkbox", checked: value, onchange: (e) => set(option, e.target.checked, true) }),
            h("span", {}, option.label),
          ),
          hint,
        );
      case "choice":
        return h(
          "label",
          { class: "field theme-option" },
          h("span", {}, option.label),
          h(
            "select",
            { ...attrs, onchange: (e) => set(option, e.target.value, true) },
            ...option.choices.map((c) => h("option", { value: c.value, selected: c.value === value }, c.label)),
          ),
          hint,
        );
      case "color": {
        const picker = colorPicker({
          ...attrs,
          "aria-label": option.label,
          oninput: (e) => set(option, e.target.value, false),
          onchange: (e) => set(option, e.target.value, true),
        });
        picker.value = fullHex(value) ?? "#808080";
        return h("div", { class: "field theme-option" }, h("span", {}, option.label), h("div", { class: "row" }, picker), hint);
      }
      case "range": {
        const output = h("output", {}, rangeText(option, value));
        const input = h("input", {
          ...attrs,
          type: "range",
          min: option.min,
          max: option.max,
          step: option.step ?? "any",
          value,
          style: rangeAt(option, value),
          oninput: (e) => {
            e.target.setAttribute("style", rangeAt(option, Number(e.target.value)));
            output.textContent = rangeText(option, Number(e.target.value));
            set(option, Number(e.target.value), false);
          },
          onchange: (e) => set(option, Number(e.target.value), true),
        });
        return h("label", { class: "field theme-option" }, h("span", {}, option.label), h("span", { class: "row" }, input, output), hint);
      }
      default:
        return null;
    }
  });
}

/** `theme`'s options under its name, with a button that puts them back to
 *  the theme's own, shown when `changed`. */
function optionsGroup(theme, changed, set, reset) {
  const resetButton = h(
    "button",
    { type: "button", class: "link theme-options-reset", hidden: !changed, onclick: reset },
    "Back to the theme's defaults",
  );
  return h(
    "div",
    { class: "theme-options stack", "data-theme-id": theme.id },
    h("div", { class: "section-rule" }, `${theme.name} options`),
    ...optionControls(theme, (option, value, done) => {
      set(option, value, done);
      if (done) resetButton.hidden = false;
    }),
    resetButton,
  );
}

// Settings → Appearance.

const appearance = $("#appearance");
const picker = $("#theme-picker");
const modeSelect = $("#appearance-mode");
const gameThemesSelect = $("#appearance-game-themes");
const reduceMotion = $("#appearance-reduce-motion");
const themeOptions = $("#theme-options");
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
  reduceMotion.checked = !!prefs.reduce_motion;
  renderPicker();
  renderModeNote();
  renderThemeOptions();
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

/** A small drawing of the app in `theme`'s colors (null: the built-in
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
  await savePrefs({ theme: theme?.id ?? null }, "theme");
  renderPicker();
  renderThemeOptions();
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

/** The options of the themes in use, which are saved as they change. */
function renderThemeOptions() {
  const inUse = [current?.global, current?.game].filter((t) => t?.options?.length);
  themeOptions.replaceChildren(
    ...inUse.map((theme) =>
      optionsGroup(
        theme,
        Object.keys(prefs.options?.[theme.id] ?? {}).length > 0,
        (option, value, done) => setOption(theme.id, option, value, done),
        () => resetOptions(theme.id).catch(() => {}),
      ),
    ),
  );
  themeOptions.hidden = !inUse.length;
}

function setOption(themeId, option, value, done) {
  // `current` is replaced each time something is saved, so the theme is
  // looked up again rather than kept.
  const theme = [current?.global, current?.game].find((t) => t?.id === themeId);
  if (!theme) return;
  theme.values = { ...theme.values, [option.id]: value };
  paint(previewing === undefined ? current : previewFor(previewing));
  if (!done) return;
  const options = { ...prefs.options };
  options[themeId] = { ...options[themeId], [option.id]: value };
  savePrefs({ options }, `${theme.name} ${option.label.toLowerCase()}`).catch(() => {});
}

async function resetOptions(themeId) {
  const options = { ...prefs.options };
  delete options[themeId];
  const name = themes.find((t) => t.id === themeId)?.name ?? "theme's";
  await savePrefs({ options }, `${name} options`);
  renderThemeOptions();
}

/** Says so when the themes in use only have one mode. */
function renderModeNote() {
  const note = $("#appearance-mode-note");
  const only = current?.modes.length === 1 ? current.modes[0] : null;
  const name = current?.game?.name ?? current?.global?.name;
  note.hidden = !only;
  if (only) note.textContent = `The theme in use${name ? ` (${name})` : ""} only has a ${only} look.`;
}

/** Settings → Appearance, as where a change to [theme] is undone: the
 *  dialog it's recorded in, and how to show it again once it is. */
const inSettings = { dialog: () => $("#settings-dialog"), show: () => renderAppearance() };

/** Saves `change` to gui.toml's [theme]. With `label` (what it changes),
 *  records it to be undone `where` it was made (see inSettings). */
async function savePrefs(change, label, where = inSettings) {
  // only what's changed is put back, not anything changed since
  const before = Object.fromEntries(Object.keys(change).map((key) => [key, prefs[key]]));
  prefs = { ...prefs, ...change };
  const saved = prefs;
  const saving = label ? autosave(where.dialog()) : null;
  // waits for any undo running, so they're saved in order
  const save = () => run("set_theme_prefs", { prefs: saved });
  await (saving ? saving.queue(save) : save());
  saving?.record({
    label,
    tab: "appearance",
    undo: () => restorePrefs(before, where),
    redo: () => restorePrefs(change, where),
  });
  cache.clear();
  await applyTheme(document.body.dataset.game || null);
  renderModeNote();
}

/** Saves `change` to [theme] again, for undo and redo, and shows it `where`
 *  it was made. */
async function restorePrefs(change, where) {
  clearTimeout(hoverTimer);
  previewing = undefined;
  prefs = { ...prefs, ...change };
  await run("set_theme_prefs", { prefs });
  cache.clear();
  await applyTheme(document.body.dataset.game || null);
  await where.show();
}

modeSelect.addEventListener("change", () =>
  savePrefs({ mode: modeSelect.value }, "mode")
    .then(renderPicker)
    .catch(() => {}),
);
gameThemesSelect.addEventListener("change", () =>
  savePrefs({ game_themes: gameThemesSelect.value }, "game themes setting")
    .then(() => {
      renderPicker();
      renderThemeOptions();
    })
    .catch(() => {}),
);
reduceMotion.addEventListener("change", () => savePrefs({ reduce_motion: reduceMotion.checked }, "reduce motion setting").catch(() => {}));

// A game's settings: the theme for just that game, a value of gui.toml's
// [theme.games] ("" for Automatic), and its options. Each is saved as it's
// changed, and undone from the game's settings (see autosave.js). While
// they're open, the window shows the game's theme as it's picked there.

const gameThemeSelect = $("#game-settings-theme");
const gameThemeOptions = $("#game-theme-options");
const gameDialog = gameThemeSelect.closest("dialog");
let gameThemeFor = null;
/** theme() for the game with the Theme picked there. */
let gamePreview = null;

/** A game's settings, as where a change to [theme] is undone (see
 *  inSettings): shown again there if they're still open on `game`. */
const inGame = (game) => ({
  dialog: () => gameDialog,
  show: async () => {
    if (gameThemeFor !== game || !gameDialog.open) return;
    await renderGameTheme(game);
    if (gamePreview) paint(gamePreview);
  },
});

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
      ? `${usual} in the game's color`
      : usual;
  const option = (t) => h("option", { value: t.id }, t.name);
  const set = prefs.games?.[game] ?? "";
  const known = set === "" || set === "none" || set === "default" || list.themes.some((t) => t.id === set);
  // Leaving the game's look out only differs from Automatic when there's some.
  const showNone = gameTheme || automatic.accent || set === "none";
  gameThemeSelect.replaceChildren(
    h("option", { value: "" }, `Automatic: ${automaticShows}`),
    showNone ? h("option", { value: "none" }, `${usual} (your usual theme), without the game's ${gameTheme ? "theme" : "color"}`) : null,
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
  await showGameOptions();
}

/** Shows the options of the theme picked in a game's settings: its own
 *  theme, or a theme for every game picked for just this game. With
 *  `show`, paints it on the window too. */
async function showGameOptions(show = false) {
  const game = gameThemeFor;
  const pick = gameThemeSelect.value;
  const preview = await invoke("theme", { game, pick });
  if (gameThemeSelect.value !== pick || gameThemeFor !== game) return;
  gamePreview = preview;
  const own = [preview.game, preview.global?.id === pick ? preview.global : null].filter((t) => t?.options?.length);
  gameThemeOptions.replaceChildren(
    ...own.map((theme) =>
      optionsGroup(
        theme,
        Object.keys(prefs.options?.[theme.id] ?? {}).length > 0,
        (option, value, done) => {
          theme.values = { ...theme.values, [option.id]: value };
          paint(gamePreview);
          if (!done) return;
          // Options belong to a theme, wherever it's used.
          const options = { ...prefs.options };
          options[theme.id] = { ...options[theme.id], [option.id]: value };
          savePrefs({ options }, `${theme.name} ${option.label.toLowerCase()}`, inGame(game))
            .then(() => paint(gamePreview))
            .catch(() => {});
        },
        () => {
          const options = { ...prefs.options };
          delete options[theme.id];
          savePrefs({ options }, `${theme.name} options`, inGame(game))
            .then(() => showGameOptions(true))
            .catch(() => {});
        },
      ),
    ),
  );
  gameThemeOptions.hidden = !own.length;
  if (show && gameDialog.open) paint(preview);
}

gameThemeSelect.addEventListener("change", () => {
  const game = gameThemeFor;
  const games = { ...prefs.games };
  if (gameThemeSelect.value) games[game] = gameThemeSelect.value;
  else delete games[game];
  savePrefs({ games }, "theme", inGame(game))
    .then(() => showGameOptions(true))
    .catch(() => {});
});

// Back to the theme in use, after showing the game's in its settings.
gameDialog.addEventListener("close", () => {
  if (current) paint(current);
});
