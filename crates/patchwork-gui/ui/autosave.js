// Settings dialogs that save each change as it's made, with no Save button,
// and undo and redo to take changes back: Settings (settings.js), a game's
// settings (games.js) and a profile's (setup.js).
//
// A dialog gets all of this from `autosave(dialog)` going by what's in it:
//
//   <dialog class="settings">
//     <form method="dialog" class="stack">
//       <header class="settings-header">          stays at the top
//         <div class="row settings-title"><h2>…</h2></div>   undo buttons go here
//         <div class="tabs" role="tablist">        optional
//           <button type="button" role="tab" data-tab="general">…
//       </header>
//       <div data-panel="general">…</div>          shown with its tab
//       <p class="error settings-error" hidden></p>
//       (fields' labels say what changed, or data-label="…" where they don't read well)
//       <div class="row end">
//         <small class="muted settings-saved">Changes are saved as you make them.</small>
//         <button type="button" value="cancel">Close</button>
//
// Whoever saves a setting does it through `change()`, which runs saves (and
// undoing them) one at a time, in order, shows what goes wrong under the
// form, and records how to undo it. The Undo and Redo buttons, or Ctrl+Z
// and Ctrl+Shift+Z (or Ctrl+Y), step through those. A dialog about one of
// several things (a game, a profile) keeps a history for each, picked with
// `use(key)`, so undo only takes back changes to what's shown. Histories
// last until Patchwork closes.

import { h } from "./dom.js";
import { errorText, status } from "./state.js";

/** How many changes each history keeps to undo. */
const KEPT = 100;

const controllers = new WeakMap();

/** The autosave controller for `dialog`, the same one each time. */
export function autosave(dialog) {
  let controller = controllers.get(dialog);
  if (!controller) {
    controller = new Autosave(dialog);
    controllers.set(dialog, controller);
  }
  return controller;
}

/** A 24×24 line icon, stroked in the current colour. */
function icon(...paths) {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  for (const d of paths) {
    const path = document.createElementNS(ns, "path");
    path.setAttribute("d", d);
    svg.append(path);
  }
  return svg;
}

/** Whether `el` is a box text is typed into. */
const isText = (el) =>
  el.matches("textarea") ||
  (el.matches("input") && !/^(checkbox|radio|range|color|button|submit|file|hidden)$/.test(el.type));

/** Whether `el` holds typing that isn't saved: its value isn't the saved
 *  one, which is kept as its defaultValue. */
const unsaved = (el) => isText(el) && el.value !== el.defaultValue;

class Autosave {
  constructor(dialog) {
    this.dialog = dialog;
    this.form = dialog.querySelector("form");
    this.error = dialog.querySelector(".settings-error");
    /** Each thing's history, as `{ done, undone }`, the latest of each last. */
    this.histories = new Map();
    this.key = null;
    /** The save or undo running now, which the next waits for. */
    this.running = Promise.resolve();

    this.undoButton = h(
      "button",
      { type: "button", class: "icon-action", "aria-label": "Undo", onclick: () => this.step(false) },
      icon("M9 14L4 9l5-5", "M4 9h10.5a5.5 5.5 0 0 1 0 11H11"),
    );
    this.redoButton = h(
      "button",
      { type: "button", class: "icon-action", "aria-label": "Redo", onclick: () => this.step(true) },
      icon("M15 14l5-5-5-5", "M20 9H9.5a5.5 5.5 0 0 0 0 11H13"),
    );
    dialog
      .querySelector(".settings-title")
      ?.append(h("div", { class: "row settings-undo" }, this.undoButton, this.redoButton));
    this.showButtons();

    this.tabs = [...dialog.querySelectorAll("[role=tab][data-tab]")];
    for (const tab of this.tabs) tab.addEventListener("click", () => this.selectTab(tab.dataset.tab));
    const first = this.tabs.find((t) => t.getAttribute("aria-selected") === "true") ?? this.tabs[0];
    if (first) this.selectTab(first.dataset.tab);

    this.form.querySelector("[value=cancel]")?.addEventListener("click", () => dialog.close());
    // nothing's submitted; each part saves as it changes
    this.form.addEventListener("submit", (e) => e.preventDefault());

    // Ctrl+Z and Ctrl+Shift+Z (or Ctrl+Y), except in a box with typing in it
    // that isn't saved yet, where they undo the typing instead.
    dialog.addEventListener("keydown", (e) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      const key = e.key.toLowerCase();
      const redo = (key === "z" && e.shiftKey) || (key === "y" && !e.shiftKey);
      if (!redo && !(key === "z" && !e.shiftKey)) return;
      if (unsaved(e.target)) return;
      e.preventDefault();
      this.step(redo);
    });
  }

  /** The history of what's shown. */
  get history() {
    let history = this.histories.get(this.key);
    if (!history) {
      history = { done: [], undone: [] };
      this.histories.set(this.key, history);
    }
    return history;
  }

  /** Shows the history for `key` (a game's id, say) from now on. */
  use(key) {
    this.key = key;
    this.showButtons();
  }

  /** Opens the dialog, on `tab` if given, for what `key` names if given. */
  open({ key, tab } = {}) {
    if (key !== undefined) this.use(key);
    this.hideError();
    if (tab) this.selectTab(tab);
    if (!this.dialog.open) this.dialog.showModal();
  }

  /** Shows tab `key`, and the parts marked data-panel="<key>". */
  selectTab(key) {
    for (const tab of this.tabs) tab.setAttribute("aria-selected", String(tab.dataset.tab === key));
    for (const panel of this.form.querySelectorAll("[data-panel]")) {
      // a class rather than hidden, so parts with nothing to show stay hidden
      panel.classList.toggle("other-tab", panel.dataset.panel !== key);
    }
  }

  /** The tab showing, if there are tabs. */
  shownTab() {
    return this.tabs.find((t) => t.getAttribute("aria-selected") === "true")?.dataset.tab ?? null;
  }

  showError(err) {
    if (!this.error) return;
    this.error.textContent = errorText(err);
    this.error.hidden = false;
  }

  hideError() {
    if (this.error) this.error.hidden = true;
  }

  /** Resolves once every save or undo started so far is finished, failed
   *  or not. */
  settled() {
    return this.running;
  }

  /** Runs `work` once any save or undo before it is finished. */
  queue(work) {
    const next = this.running.then(work);
    this.running = next.catch(() => {});
    return next;
  }

  /**
   * Saves a change to `label` (what's changed, like "install folder") by
   * running `work` in turn. `work` resolves to null if there was nothing
   * to save, or else to how to take it back, `{ undo, redo }`, without
   * which it can't be undone; it can say what to show with `message` too.
   * Resolves to whether something was saved. `tab` is where it's shown,
   * to go back to when it's undone.
   */
  async change(label, work, { tab = this.shownTab() } = {}) {
    const history = this.history;
    this.hideError();
    let saved;
    try {
      saved = await this.queue(work);
    } catch (err) {
      this.showError(err);
      status(`Couldn't save the ${label}`, true);
      return false;
    }
    if (!saved) return false;
    if (saved.undo) this.record({ label, tab, undo: saved.undo, redo: saved.redo }, history);
    status(saved.message ?? `Saved the ${label}`);
    return true;
  }

  /**
   * Notes a change that's been saved some other way, to be undone: `label`
   * says what was changed, `undo()` and `redo()` save it as it was and as
   * it is, and `tab` is where it's shown. Anything undone can't be redone
   * after.
   */
  record(change, history = this.history) {
    history.done.push({ tab: this.shownTab(), ...change });
    if (history.done.length > KEPT) history.done.shift();
    history.undone = [];
    this.showButtons();
  }

  /** Undoes (or with `redo`, redoes) the latest change, saying so. */
  async step(redo) {
    const history = this.history;
    const [from, to] = redo ? [history.undone, history.done] : [history.done, history.undone];
    this.hideError();
    try {
      const change = await this.queue(async () => {
        const change = from.pop();
        if (!change) return null;
        this.showButtons();
        // shows what's being undone, if it's on another tab
        if (change.tab && change.tab !== this.shownTab()) this.selectTab(change.tab);
        try {
          await (redo ? change.redo() : change.undo());
        } catch (err) {
          // still there to try again
          from.push(change);
          throw err;
        }
        to.push(change);
        return change;
      });
      if (change) status(`${redo ? "Redid" : "Undid"} the change to the ${change.label}`);
    } catch (err) {
      this.showError(err);
      status(`Couldn't ${redo ? "redo" : "undo"} that`, true);
    } finally {
      this.showButtons();
    }
  }

  showButtons() {
    const { done, undone } = this.history;
    const next = { undo: done.at(-1), redo: undone.at(-1) };
    this.undoButton.disabled = !next.undo;
    this.redoButton.disabled = !next.redo;
    this.undoButton.title = next.undo ? `Undo the change to the ${next.undo.label} (Ctrl+Z)` : "Nothing to undo";
    this.redoButton.title = next.redo
      ? `Redo the change to the ${next.redo.label} (Ctrl+Shift+Z)`
      : "Nothing to redo";
  }

  /**
   * Saves the fields in `container` as they change, with `save(field)`:
   * drop-downs, checkboxes and the like straight away, a box of text once
   * it's left or Enter is pressed, and any typing not saved yet when the
   * dialog closes. Once `save` resolves to anything but false, a box's
   * text counts as saved. Fields marked data-autosave="off" are left out.
   */
  watch(container, save) {
    const saveField = async (field) => {
      if (field.closest("[data-autosave=off]")) return;
      const typed = field.value;
      const saved = await save(field);
      if (saved !== false && isText(field) && field.value === typed) field.defaultValue = typed;
    };
    container.addEventListener("change", (e) => {
      if (e.target.matches("input, select, textarea")) saveField(e.target);
    });
    container.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && e.target.matches("input") && isText(e.target)) {
        e.preventDefault();
        saveField(e.target);
      }
    });
    this.dialog.addEventListener("close", () => {
      for (const field of container.querySelectorAll("input, textarea")) {
        if (unsaved(field)) saveField(field);
      }
    });
  }

  /**
   * Runs `fill`, which fills the form in again, keeping focus where it was
   * and typing that isn't saved yet, matching fields by id or name.
   */
  async refill(fill) {
    const name = (el) => el.id || el.name;
    const focused = this.dialog.contains(document.activeElement) ? name(document.activeElement) : null;
    const typed = [...this.form.querySelectorAll("input, textarea")].filter(unsaved).map((el) => [name(el), el.value]);
    await fill();
    const find = (key) =>
      key && (document.getElementById(key) ?? this.form.querySelector(`[name="${CSS.escape(key)}"]`));
    for (const [key, value] of typed) {
      const el = find(key);
      if (el && this.form.contains(el) && !el.disabled) el.value = value;
    }
    const el = find(focused);
    if (el && this.form.contains(el)) el.focus({ focusVisible: false });
  }

  /** What `field` is called, for saying what changed: its data-label, or
   *  else its label. */
  labelOf(field) {
    if (field.dataset.label) return field.dataset.label;
    const label =
      (field.id && this.form.querySelector(`label[for="${CSS.escape(field.id)}"]`)) || field.closest("label");
    // the label's own words, not its notes or the field itself
    const words = (el) =>
      [...el.childNodes]
        .filter((n) => n.nodeType === Node.TEXT_NODE || !n.matches("small, input, select, textarea, button"))
        .map((n) => n.textContent)
        .join("");
    const text = (label && words(label).trim()) || field.getAttribute("aria-label") || "setting";
    return text.trim().replace(/\s+/g, " ").toLowerCase();
  }
}
