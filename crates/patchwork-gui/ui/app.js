// Patchwork's UI. Plain JS with no build step: everything goes through
// invoke() to the commands in main.rs.
//
// Mod names, summaries and so on come from remote sources, so they're only
// ever put in the page as text (see h()), never as HTML.

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;

const $ = (sel) => document.querySelector(sel);

/** Makes an element. `attrs` keys starting with "on" are event listeners. */
function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value == null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else el.setAttribute(key, value === true ? "" : value);
  }
  el.append(...children.flat().filter((c) => c != null));
  return el;
}

const state = {
  overview: null,
  /** The game being looked at, or null on the game picker. */
  game: null,
  profile: null,
  search: { page: 0, query: null },
};

function status(text, isError = false) {
  const el = $("#status");
  el.textContent = text;
  el.classList.toggle("error", isError);
}

/** Runs a command, showing its error in the status bar if it fails. */
async function run(command, args) {
  try {
    return await invoke(command, args);
  } catch (e) {
    status(String(e), true);
    throw e;
  }
}

function compact(n) {
  if (n < 1e3) return String(n);
  if (n < 1e6) return (n / 1e3).toFixed(1) + "k";
  return (n / 1e6).toFixed(1) + "M";
}

/** Reloads everything, staying on whichever screen and profile is open. */
async function refresh() {
  state.overview = await run("overview");
  for (const warning of state.overview.warnings) console.warn(warning);
  if (state.game && gameInfo(state.game)) {
    renderSidebar();
    if (state.profile) await openProfile(state.profile.id);
    else renderProfile();
  } else {
    await showGames();
  }
}

const gameInfo = (id) => state.overview.games.find((g) => g.id === id);
const profilesOf = (game) => state.overview.profiles.filter((p) => p.game === game);

// Games

function showScreen(screen) {
  document.body.dataset.screen = screen;
  $("#games-screen").hidden = screen !== "games";
  $("#sidebar").hidden = screen !== "game";
  $("#main").hidden = screen !== "game";
}

async function showGames() {
  state.game = null;
  state.profile = null;
  showScreen("games");
  await applyTheme(null);
  renderGames();
}

/** Up to two letters standing in for a game's picture. */
function initials(name) {
  const words = name
    .split(/[\s\-_:]+/)
    .filter((w) => w && !/^(a|an|and|of|the)$/i.test(w));
  return words.slice(0, 2).map((w) => w[0].toUpperCase()).join("") || "?";
}

function renderGames() {
  const { games } = state.overview;
  $("#game-grid").replaceChildren(
    ...games.map((g) => {
      const count = profilesOf(g.id).length;
      const facts = [
        `${count} profile${count === 1 ? "" : "s"}`,
        g.install ? "installed" : "not set up",
      ];
      return h(
        "li",
        {},
        h(
          "button",
          { class: "game-card", "data-game": g.id, onclick: () => openGame(g.id) },
          h("div", { class: "game-art", "aria-hidden": "true" }, initials(g.name)),
          h("span", { class: "game-name" }, g.name),
          h("span", { class: "muted" }, facts.join(" · ")),
          g.origin === "user" ? h("span", { class: "badge" }, "Added by you") : null,
        ),
      );
    }),
    h(
      "li",
      {},
      h(
        "button",
        { class: "game-card add", onclick: openGameDialog },
        h("div", { class: "game-art", "aria-hidden": "true" }, "+"),
        h("span", { class: "game-name" }, "Add a game"),
        h("span", { class: "muted" }, "Set up one patchwork doesn't know yet"),
      ),
    ),
  );
}

/**
 * Shows a game's profiles, opening the current profile if it's for this
 * game, or else its first one.
 */
async function openGame(id) {
  state.game = id;
  state.profile = null;
  showScreen("game");
  document.querySelector("[data-tab=mods]").click();
  await applyTheme(id);
  renderSidebar();
  const current = state.overview.current;
  const ps = profilesOf(id);
  const pick = ps.find((p) => p.id === current) ?? ps[0];
  if (pick) await switchProfile(pick.id);
  else renderProfile();
}

$("#back").addEventListener("click", () => showGames());

// Profiles

function renderSidebar() {
  const game = gameInfo(state.game);
  const current = state.profile?.id;
  $("#game-title").textContent = game.name;
  const ps = profilesOf(game.id);
  $("#profiles").replaceChildren(
    ...ps.map((p) =>
      h(
        "button",
        { "aria-current": String(p.id === current), onclick: () => switchProfile(p.id) },
        h("span", {}, p.name),
        h("span", { class: "muted" }, String(p.mods)),
      ),
    ),
  );
  if (!ps.length) {
    $("#profiles").append(h("p", { class: "muted" }, "No profiles yet."));
  }
}

async function switchProfile(id) {
  await run("use_profile", { id });
  state.overview.current = id;
  await openProfile(id);
}

async function openProfile(id) {
  state.profile = await run("profile", { id });
  state.search = { page: 0, query: null };
  $("#results").replaceChildren();
  $("#pager").hidden = true;
  renderSidebar();
  renderProfile();
  refreshVersions();
}

async function applyTheme(game) {
  document.body.dataset.game = game ?? "";
  const theme = await invoke("theme", { game });
  $("#theme-default").textContent = theme.default;
  $("#theme-game").textContent = theme.game ?? "";
}

function renderProfile() {
  const p = state.profile;
  $("#deploy").disabled = !p?.install;
  $("#update").disabled = !p?.mods.length;
  if (!p) {
    const game = gameInfo(state.game);
    $("#profile-title").textContent = `No ${game.name} profiles yet`;
    $("#profile-meta").textContent = "Create one in the sidebar to start adding mods.";
    $("#mods").replaceChildren();
    $("#mod-problems").replaceChildren();
    $("#mods-empty").hidden = true;
    $("#game-path").hidden = !!game.install;
    $("#search [name=source]").replaceChildren();
    return;
  }

  $("#profile-title").textContent = p.label;
  const enabled = p.mods.filter((m) => m.enabled).length;
  const deployed =
    p.deployedFiles != null ? `deployed (${p.deployedFiles} files)` : "not deployed";
  $("#profile-meta").textContent =
    `${enabled}/${p.mods.length} mods enabled · ${p.install ?? "game not set up"} · ${deployed}`;
  $("#game-path").hidden = !!p.install;

  renderMods(p.mods);
  $("#mod-problems").replaceChildren(...p.problems.map((w) => h("li", {}, w)));

  $("#search [name=source]").replaceChildren(
    ...p.searchSources.map((s) => h("option", { value: s }, s)),
  );
}

function modRow(m, { onToggle, onRemove, onVersion } = {}) {
  return h(
    "li",
    { class: m.enabled ? null : "disabled" },
    h("input", {
      type: "checkbox",
      checked: m.enabled,
      "aria-label": `Enable ${m.name}`,
      onchange: onToggle,
    }),
    h("span", { class: "mod-name" }, m.name),
    h("span", { class: "mod-source muted" }, `${m.source}:${m.id}`),
    m.versioned
      ? versionSelect(m, onVersion)
      : h("span", { class: "mod-source muted" }, m.version ?? m.locked ?? ""),
    h("button", { class: "remove", onclick: onRemove }, "Remove"),
  );
}

// Versions. The profile comes with each mod's versions as last fetched, so
// the dropdowns are complete straight away; lists older than an hour (see
// VERSIONS_TTL in main.rs) are then refreshed in the background.

/** The version dropdown of each mod on screen, by versionKey(). */
const versionSelects = new Map();
/** Refreshes in flight, by versionKey(), so re-renders don't repeat them. */
const versionFetches = new Map();
/** Mods whose versions couldn't be fetched; not retried until a restart. */
const versionFailures = new Set();

const versionKey = (m) => `${state.profile.game}/${m.source}:${m.id}`;

/**
 * Picks a mod's version: "Auto" follows the latest one, anything else
 * freezes the mod at it.
 */
function versionSelect(m, onVersion) {
  const select = h("select", {
    class: "mod-version",
    "aria-label": `Version of ${m.name}`,
    title: "Auto follows the latest version; pick one to freeze the mod at it",
    onchange: async () => {
      select.disabled = true;
      try {
        await onVersion?.(select.value || null);
      } finally {
        select.disabled = false;
      }
    },
  });
  fillVersions(select, m);
  if (state.profile) versionSelects.set(versionKey(m), select);
  return select;
}

function fillVersions(select, m) {
  // keep whatever's showing, which may be a choice that's still being saved
  const current = select.options.length ? select.value : (m.version ?? "");
  const versions = m.versions ?? [];
  const auto = !m.version && m.locked ? `Auto (${m.locked})` : "Auto";
  const options = [
    h("option", { value: "" }, auto),
    ...versions.map((v) => h("option", { value: v }, v)),
  ];
  // a pinned version that's since been taken down still shows
  if (current && !versions.includes(current)) {
    options.push(h("option", { value: current }, m.versions ? `${current} (unavailable)` : current));
  }
  select.replaceChildren(...options);
  select.value = current;
}

/** Puts freshly fetched versions into a mod's dropdown. */
function showVersions(key, versions) {
  const m = state.profile?.mods.find((m) => versionKey(m) === key);
  if (!m) return;
  m.versions = versions;
  m.versionsStale = false;
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

/** Refreshes the profile's stale version lists, a few at a time. */
async function refreshVersions() {
  const p = state.profile;
  const queue = p.mods.filter(
    (m) => m.versioned && m.versionsStale && !versionFailures.has(versionKey(m)),
  );
  const failed = [];
  const worker = async () => {
    while (queue.length && state.profile?.id === p.id) {
      const m = queue.shift();
      const key = versionKey(m);
      if (!versionFetches.has(key)) {
        const fetch = invoke("versions", { game: p.game, source: m.source, modId: m.id });
        versionFetches.set(key, fetch);
        fetch.finally(() => versionFetches.delete(key)).catch(() => {});
      }
      try {
        showVersions(key, await versionFetches.get(key));
      } catch (e) {
        versionFailures.add(key);
        failed.push(`${m.name}: ${e}`);
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(4, queue.length) }, worker));
  if (failed.length) {
    for (const f of failed) console.warn(f);
    status(`Couldn't check for new versions of ${failed.length} mod(s); ${failed[0]}`, true);
  }
}

function renderMods(mods) {
  const id = state.profile.id;
  versionSelects.clear();
  $("#mods").replaceChildren(
    ...mods.map((m) =>
      modRow(m, {
        onToggle: async (e) => {
          await run("set_enabled", { id, name: m.name, enabled: e.target.checked });
          await openProfile(id);
        },
        onRemove: async () => {
          await run("remove_mod", { id, name: m.name });
          status(`Removed ${m.name}`);
          await refresh();
        },
        onVersion: async (version) => {
          status(version ? `Pinning ${m.name} to ${version}…` : `Unpinning ${m.name}…`);
          try {
            await run("set_version", { id, name: m.name, version });
            status(version ? `Pinned ${m.name} to ${version}` : `${m.name} follows the latest version`);
          } finally {
            await openProfile(id);
          }
        },
      }),
    ),
  );
  $("#mods-empty").hidden = mods.length > 0;
}

$("#new-profile").addEventListener("submit", async (e) => {
  e.preventDefault();
  const name = new FormData(e.target).get("name");
  const id = await run("create_profile", { game: state.game, name });
  e.target.reset();
  state.overview = await run("overview");
  await openProfile(id);
});

$("#game-path").addEventListener("submit", async (e) => {
  e.preventDefault();
  const path = new FormData(e.target).get("path");
  await run("set_game_path", { game: state.game, path });
  await refresh();
});

// Adding a game. Writes <config>/games/<id>.toml through create_game in
// main.rs; anything the form doesn't cover can be added to that file later.

const gameForm = $("#new-game");
/** Whether the id has been typed in, rather than filled in from the name. */
let idEdited = false;

const slug = (name) =>
  name.toLowerCase().normalize("NFKD").replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");

function openGameDialog() {
  gameForm.reset();
  idEdited = false;
  $("#new-game-error").hidden = true;
  $("#new-game-rulesets").replaceChildren(
    h("span", {}, "Install rules"),
    ...state.overview.rulesets.map((r) =>
      h(
        "label",
        { class: "row" },
        h("input", { type: "checkbox", name: "rulesets", value: r }),
        h("span", {}, r),
        r === "bepinex"
          ? h("small", { class: "muted" }, "most Thunderstore games")
          : null,
      ),
    ),
  );
  if (!state.overview.rulesets.length) {
    $("#new-game-rulesets").append(
      h("p", { class: "muted hint" }, "No shared rules are installed; files go in as they are."),
    );
  }
  $("#game-dialog").showModal();
}

gameForm.elements.name.addEventListener("input", (e) => {
  if (!idEdited) gameForm.elements.id.value = slug(e.target.value);
});
gameForm.elements.id.addEventListener("input", () => (idEdited = true));
gameForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#game-dialog").close();
});

gameForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(gameForm);
  const game = {
    id: form.get("id"),
    name: form.get("name"),
    install: form.get("install") || null,
    thunderstore: form.get("thunderstore") || null,
    nexus: form.get("nexus") || null,
    rulesets: form.getAll("rulesets"),
    link: form.get("link"),
  };
  const submit = gameForm.querySelector("[type=submit]");
  const existed = !!gameInfo(game.id);
  submit.disabled = true;
  let error = null;
  try {
    await invoke("create_game", { game });
  } catch (err) {
    error = String(err);
  } finally {
    submit.disabled = false;
  }
  state.overview = await run("overview");
  // The game can be saved even if its install folder couldn't be set, so
  // carry on to it with the error showing rather than leaving the dialog up.
  if (error && (existed || !gameInfo(game.id))) {
    $("#new-game-error").textContent = error;
    $("#new-game-error").hidden = false;
    $("#new-game-error").scrollIntoView({ block: "nearest" });
    return;
  }
  $("#game-dialog").close();
  await openGame(game.id);
  status(error ?? `Added ${game.name}`, !!error);
});

// Tabs

for (const tab of document.querySelectorAll("[role=tab]")) {
  tab.addEventListener("click", () => {
    for (const other of document.querySelectorAll("[role=tab]")) {
      const selected = other === tab;
      other.setAttribute("aria-selected", String(selected));
      $(`#tab-${other.dataset.tab}`).hidden = !selected;
    }
  });
}

// Browse

async function search(page) {
  const p = state.profile;
  if (!p) return;
  const form = new FormData($("#search"));
  const query = {
    game: p.game,
    source: form.get("source"),
    text: form.get("text"),
    sort: form.get("sort") || null,
    page,
  };
  if (!query.source) {
    status(`None of ${p.game}'s sources can be searched`, true);
    return;
  }
  status("Searching…");
  const started = performance.now();
  const results = await run("search", query);
  const fetched = performance.now();
  state.search = { page, query };
  renderResults(results);
  await nextFrame();
  status(
    `${results.total ?? results.items.length} mods · fetched in ${ms(fetched - started)}, ` +
      `rendered in ${ms(performance.now() - fetched)}`,
  );
}

function renderResults(results) {
  const added = new Set(state.profile.mods.map((m) => `${m.source}:${m.id}`));
  $("#results").replaceChildren(
    ...results.items.map((item) => {
      const facts = [
        item.author && `by ${item.author}`,
        item.downloads != null && `${compact(item.downloads)} downloads`,
      ].filter(Boolean);
      const isAdded = added.has(`${item.source}:${item.id}`);
      return h(
        "li",
        { class: "result" },
        item.icon_url
          ? h("img", { src: item.icon_url, alt: "", loading: "lazy", decoding: "async" })
          : h("div", { class: "no-icon" }),
        h("h3", {}, item.name),
        h("p", { class: "muted" }, item.summary?.trim() ?? ""),
        h(
          "div",
          { class: "facts muted" },
          h("span", {}, facts.join(" · ")),
          h(
            "button",
            {
              disabled: isAdded,
              onclick: async (e) => {
                e.target.disabled = true;
                status(`Adding ${item.name}…`);
                const name = await run("add_mod", {
                  id: state.profile.id,
                  source: item.source,
                  modId: item.id,
                }).catch(() => (e.target.disabled = false));
                if (typeof name === "string") {
                  status(`Added ${name}`);
                  e.target.textContent = "Added";
                  state.profile = await run("profile", { id: state.profile.id });
                  renderProfile();
                  refreshVersions();
                }
              },
            },
            isAdded ? "Added" : "Add",
          ),
        ),
      );
    }),
  );
  if (!results.items.length) {
    $("#results").append(h("p", { class: "empty" }, "No mods found."));
  }
  const page = state.search.page;
  $("#pager").hidden = page === 0 && !results.has_more;
  $("#prev").disabled = page === 0;
  $("#next").disabled = !results.has_more;
  $("#page-info").textContent = `Page ${page + 1}`;
  $("#tab-browse").scrollTop = 0;
}

$("#search").addEventListener("submit", (e) => {
  e.preventDefault();
  search(0);
});
$("#prev").addEventListener("click", () => search(state.search.page - 1));
$("#next").addEventListener("click", () => search(state.search.page + 1));

// Deploy

$("#deploy").addEventListener("click", async () => {
  const id = state.profile.id;
  $("#deploy").disabled = true;
  status("Working out what to deploy…");
  try {
    const plan = await run("plan_deploy", { id });
    const changes = plan.ops.length;
    $("#plan-summary").textContent = changes
      ? `${changes} change${changes === 1 ? "" : "s"}, ${plan.unchanged} files already in place.`
      : `Nothing to do; ${plan.unchanged} files already in place.`;
    $("#plan-warnings").replaceChildren(...plan.warnings.map((w) => h("li", {}, w)));
    $("#plan-ops").replaceChildren(...plan.ops.map(opRow));
    $("#plan-dialog [value=apply]").disabled = !changes;
    status("Ready");
    $("#plan-dialog").showModal();
  } finally {
    $("#deploy").disabled = false;
  }
});

$("#plan-dialog").addEventListener("close", async () => {
  if ($("#plan-dialog").returnValue !== "apply") return;
  status("Deploying…");
  const files = await run("apply_plan");
  status(`Deployed ${files} files`);
  await openProfile(state.profile.id);
});

function opRow(op) {
  switch (op.op) {
    case "remove":
      return h("li", { class: "remove" }, `− ${op.dst}`);
    case "backup":
      return h("li", {}, `~ ${op.dst} (backing up original)`);
    case "place":
      return h("li", {}, `+ ${op.dst}  [${op.modName}]`);
  }
}

// Update. Relocks the profile at the newest versions, like `patchwork lock
// --update`; pinned mods stay where they are. Nothing changes in the game
// until the next deploy.

$("#update").addEventListener("click", async () => {
  const id = state.profile.id;
  $("#update").disabled = true;
  status("Checking for updates…");
  try {
    const changes = await run("update_lock", { id });
    if (!changes.length) {
      status("Everything's up to date");
      return;
    }
    const count = changes.length;
    const summary = `${count} mod${count === 1 ? "" : "s"} changed; deploy to apply`;
    $("#update-summary").textContent = summary;
    $("#update-list").replaceChildren(
      ...changes.map((c) =>
        h("li", {}, `${c.name}  ${c.from ?? "(new)"} → ${c.to ?? "(no version)"}`),
      ),
    );
    status(summary);
    $("#update-dialog").showModal();
  } finally {
    await openProfile(id);
  }
});

// Progress from long-running commands (adding, locking, deploying).

listen("patchwork://event", ({ payload: e }) => {
  switch (e.kind) {
    case "resolving":
      return status(`Resolving ${e.name}…`);
    case "fetching":
      return status(`Downloading ${e.name}…`);
    case "applying":
      return status(`Deploying ${e.dst}`);
    case "warning":
      return status(e.message, true);
  }
});

// Perf test: fills the mod list with fake mods, then measures how long that
// took and how smooth scrolling it is. Switch profiles to get the real list
// back.

const nextFrame = () => new Promise((r) => requestAnimationFrame(() => r()));
const ms = (t) => `${Math.round(t)}ms`;

$("#stress").addEventListener("click", async () => {
  const count = Number($("#stress-count").value);
  const mods = Array.from({ length: count }, (_, i) => ({
    name: `Fake Mod ${i + 1} with a reasonably long name`,
    source: i % 2 ? "thunderstore" : "nexus",
    id: `author-${i}/mod-${i}`,
    locked: `1.${i % 10}.${i % 7}`,
    versioned: true,
    enabled: i % 5 !== 0,
  }));
  document.querySelector("[data-tab=mods]").click();
  const started = performance.now();
  $("#mods").replaceChildren(...mods.map((m) => modRow(m)));
  $("#mods-empty").hidden = true;
  await nextFrame();
  await nextFrame();
  $("#perf").textContent = `${count} rows rendered in ${ms(performance.now() - started)}; scroll to measure frames`;
  measureScroll($("#tab-mods"), count);
});

/** Reports the average and worst frame times while `el` is scrolled. */
function measureScroll(el, count) {
  let frames = [];
  let last = 0;
  let idle;
  const tick = (t) => {
    if (last) frames.push(t - last);
    last = t;
    if (idle !== undefined) requestAnimationFrame(tick);
  };
  const report = () => {
    idle = undefined;
    last = 0;
    if (frames.length < 5) return;
    const avg = frames.reduce((a, b) => a + b, 0) / frames.length;
    const worst = Math.max(...frames);
    $("#perf").textContent =
      `${count} rows · scrolling: avg ${avg.toFixed(1)}ms/frame (${Math.round(1000 / avg)} fps), worst ${ms(worst)}`;
    frames = [];
  };
  el.onscroll = () => {
    if (idle === undefined) requestAnimationFrame(tick);
    clearTimeout(idle);
    idle = setTimeout(report, 300);
  };
}

refresh().catch(() => {});
