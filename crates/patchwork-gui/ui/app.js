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

// Profiles

async function refresh() {
  state.overview = await run("overview");
  for (const warning of state.overview.warnings) console.warn(warning);
  renderSidebar();
  const current = state.overview.current;
  if (current) await openProfile(current);
  else {
    await applyTheme(null);
    renderProfile();
  }
}

function renderSidebar() {
  const { games, profiles, current } = state.overview;
  const names = Object.fromEntries(games.map((g) => [g.id, g.name]));
  const byGame = new Map();
  for (const p of profiles) byGame.set(p.game, [...(byGame.get(p.game) ?? []), p]);

  $("#profiles").replaceChildren(
    ...[...byGame].map(([game, ps]) => [
      h("h3", {}, names[game] ?? game),
      ...ps.map((p) =>
        h(
          "button",
          { "aria-current": String(p.id === current), onclick: () => switchProfile(p.id) },
          h("span", {}, p.name),
          h("span", { class: "muted" }, String(p.mods)),
        ),
      ),
    ]).flat(),
  );
  if (!profiles.length) {
    $("#profiles").append(h("p", { class: "muted" }, "No profiles yet."));
  }

  $("#new-profile [name=game]").replaceChildren(
    ...games.map((g) => h("option", { value: g.id }, g.name)),
  );
}

async function switchProfile(id) {
  await run("use_profile", { id });
  state.overview.current = id;
  renderSidebar();
  await openProfile(id);
}

async function openProfile(id) {
  state.profile = await run("profile", { id });
  state.search = { page: 0, query: null };
  $("#results").replaceChildren();
  $("#pager").hidden = true;
  await applyTheme(state.profile.game);
  renderProfile();
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
  if (!p) {
    $("#profile-title").textContent = "No profile selected";
    $("#profile-meta").textContent = "Create one in the sidebar to get started.";
    $("#mods").replaceChildren();
    $("#mods-empty").hidden = true;
    $("#game-path").hidden = true;
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

  $("#search [name=source]").replaceChildren(
    ...p.searchSources.map((s) => h("option", { value: s }, s)),
  );
}

function modRow(m, { onToggle, onRemove } = {}) {
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
    h("span", { class: "mod-source muted" }, `${m.source}:${m.id}${m.version ? " @ " + m.version : ""}`),
    h("button", { class: "remove", onclick: onRemove }, "Remove"),
  );
}

function renderMods(mods) {
  const id = state.profile.id;
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
      }),
    ),
  );
  $("#mods-empty").hidden = mods.length > 0;
}

$("#new-profile").addEventListener("submit", async (e) => {
  e.preventDefault();
  const form = new FormData(e.target);
  await run("create_profile", { game: form.get("game"), name: form.get("name") });
  e.target.reset();
  await refresh();
});

$("#game-path").addEventListener("submit", async (e) => {
  e.preventDefault();
  const path = new FormData(e.target).get("path");
  await run("set_game_path", { game: state.profile.game, path });
  await openProfile(state.profile.id);
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
    version: `1.${i % 10}.${i % 7}`,
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
