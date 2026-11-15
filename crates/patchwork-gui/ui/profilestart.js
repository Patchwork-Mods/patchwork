// Starting a profile from one box, wherever one's needed: the sidebar's
// New profile, the Mods tab of a game with none yet, and adding a mod
// before there's a profile to add it to. What's typed says how: a name
// makes an empty profile, a sync ticket joins a shared one, an r2modman
// code or a list of mod links imports one. The button and the hint under
// it say which, as it's typed, so a wrong guess shows before it's acted on.

import { invoke, run, state, status } from "./state.js";
import { h } from "./dom.js";
import { openProfile, renderSidebar } from "./profile.js";
import { joinTicket, showJoined } from "./sync.js";
import { go } from "./nav.js";

/** Profile names: what create_profile accepts. */
const NAME = /^[A-Za-z0-9._-]+$/;
/** An iroh-docs ticket: "doc" then base32, as `Share…` makes them. */
const TICKET = /^doc[a-zA-Z2-7]{40,}$/;
/** Thunderstore's r2modman codes are UUIDs. */
const CODE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Everything a profile can be started from. */
export const KINDS = ["name", "ticket", "code", "links"];

/** The links in `text`, if it's nothing but web links, as urls.rs reads them. */
function linksIn(text) {
  const parts = text.split(/[\s,]+/).filter(Boolean);
  return parts.length && parts.every((p) => /^https?:\/\/\S+$/i.test(p)) ? parts : null;
}

/**
 * What starting a profile from `text` would do, as `{ kind, label, hint }`
 * with `kind` one of `kinds` ("name", "ticket", "code" or "links"), or
 * `{ error }` if it's none of them, or null when nothing's typed.
 */
export function classify(text, kinds = KINDS) {
  const t = text.trim();
  if (!t) return null;
  const allowed = (kind) => kinds.includes(kind);
  if (allowed("ticket") && TICKET.test(t)) {
    return { kind: "ticket", label: "Join", hint: "Joins the profile shared with this ticket" };
  }
  if (allowed("code") && CODE.test(t)) {
    return { kind: "code", label: "Import", hint: "Imports the r2modman profile with this code" };
  }
  const links = allowed("links") && linksIn(t);
  if (links) {
    const n = links.length;
    return {
      kind: "links",
      label: "Import",
      hint: `Imports a profile with ${n === 1 ? "this mod" : `these ${n} mods`}`,
    };
  }
  if (allowed("name") && NAME.test(t)) {
    return { kind: "name", label: "Create", hint: `Creates an empty profile called ${t}` };
  }
  return {
    error: allowed("name")
      ? "Names can only have letters, numbers, dots, dashes and underscores"
      : "That isn't something a profile can be started from",
  };
}

/**
 * Starts a profile of `game` from `text` as classify() read it, returning
 * its id, or null for a ticket that linked computers instead. Throws if it
 * can't, for the caller to show.
 */
async function start(game, text, { kind }) {
  const t = text.trim();
  switch (kind) {
    case "name":
      return invoke("create_profile", { game, name: t });
    case "code":
    case "links":
      status("Importing…");
      return invoke("import_profile", { input: { text: t }, game, name: null, replace: false });
    case "ticket": {
      const joined = await joinTicket(t);
      // Joining opens the profile wherever it belongs, which may be another game.
      await showJoined(joined);
      return joined.kind === "devices" ? null : joined.id;
    }
  }
}

/** Opens a profile started here, the usual way. */
async function openStarted(id, kind) {
  if (kind === "ticket") return; // showJoined() has
  state.overview = await run("overview");
  await go(() => openProfile(id));
  renderSidebar();
  if (kind !== "name") status(`Imported ${state.profile.label}; deploy it to install its mods`);
}

/**
 * A form to start a profile of `game` from. Options:
 *
 * - `kinds`: what it takes (see classify()); all of them by default.
 * - `value`: what's in the box to begin with.
 * - `placeholder`: the box's placeholder.
 * - `id`: the form's id, for themes.
 * - `onStarted(id, kind)`: called once it's started, in place of opening it;
 *   `id` is null if a ticket linked computers rather than joining a profile.
 */
export function profileStarter(game, options = {}) {
  const {
    kinds = KINDS,
    value = "",
    placeholder = kinds.length > 1 ? "Name, code, links or ticket" : "Name",
    id = null,
    onStarted = openStarted,
  } = options;
  const input = h("input", {
    name: "name",
    placeholder,
    autocomplete: "off",
    spellcheck: "false",
    "aria-label":
      kinds.length > 1 ? "Profile name, or a code, links or ticket to start it from" : "Profile name",
  });
  input.value = value;
  const submit = h("button", { type: "submit", class: "primary" }, "Create");
  const hint = h("small", { class: "muted profile-start-hint" });
  const error = h("p", { class: "error", hidden: true });
  const form = h(
    "form",
    { class: "profile-start stack", id },
    h("div", { class: "row" }, input, submit),
    hint,
    error,
  );

  function render() {
    const what = classify(input.value, kinds);
    submit.textContent = what?.label ?? "Create";
    submit.disabled = !what || !!what.error;
    hint.textContent = what ? (what.error ?? what.hint) : "";
    hint.classList.toggle("error", !!what?.error);
    hint.hidden = !what;
  }

  input.addEventListener("input", () => {
    error.hidden = true;
    render();
  });

  // A list of links pasted one per line would have its line breaks dropped
  // by some webviews, running the links together, so they become spaces.
  input.addEventListener("paste", (e) => {
    const text = e.clipboardData?.getData("text/plain");
    if (!text || !/[\r\n]/.test(text)) return;
    e.preventDefault();
    input.setRangeText(text.trim().replace(/\s*[\r\n]+\s*/g, " "), input.selectionStart, input.selectionEnd, "end");
    input.dispatchEvent(new Event("input"));
  });

  form.addEventListener("submit", async (e) => {
    e.preventDefault();
    // it may be in a card or next to another form, which shouldn't see it
    e.stopPropagation();
    const what = classify(input.value, kinds);
    if (!what || what.error) return;
    submit.disabled = true;
    input.disabled = true;
    error.hidden = true;
    let started;
    try {
      started = await start(game, input.value, what);
    } catch (err) {
      error.textContent = String(err);
      error.hidden = false;
      return;
    } finally {
      submit.disabled = false;
      input.disabled = false;
    }
    input.value = "";
    render();
    await onStarted(started, what.kind);
  });

  render();
  return form;
}

/**
 * Asks for a profile to add mods to, in a starter put at the end of
 * `container` (or just after it, with `after`, for a form it can't go
 * inside), and resolves with the profile once it's made, or null if the
 * user cancels. Only makes empty ones: anything else would replace the
 * mod being added.
 */
export function askForProfile(container, { after = false, text = "Make a profile to add mods to" } = {}) {
  // only one at a time: asking again elsewhere cancels the last one
  document.querySelector(".profile-ask")?.cancel();
  return new Promise((resolve) => {
    const done = (profile) => {
      ask.remove();
      resolve(profile);
    };
    const ask = h(
      "div",
      { class: "profile-ask stack" },
      h(
        "div",
        { class: "row" },
        h("small", {}, text),
        h("button", { type: "button", class: "link", onclick: () => done(null) }, "Cancel"),
      ),
      profileStarter(state.game, {
        kinds: ["name"],
        value: "Default",
        onStarted: async (id) => {
          state.overview = await run("overview");
          // The same as switching to it, but without forgetting the search.
          await openProfile(id, { keepSearch: true });
          renderSidebar();
          done(state.profile);
        },
      }),
    );
    ask.cancel = () => done(null);
    if (after) container.after(ask);
    else container.append(ask);
    const input = ask.querySelector("input");
    input.focus();
    input.select();
  });
}
