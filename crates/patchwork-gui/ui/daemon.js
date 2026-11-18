// The daemon, which does Patchwork's background tasks (such as keeping
// synced profiles in step) while Patchwork is closed (see daemon.rs): the
// Background tab in Settings, for installing it, turning its tasks on and
// off, and seeing how it's doing; offering it once something's shared or
// linked; and showing what it synced while Patchwork was closed. While
// Patchwork's open the daemon is paused, and Patchwork does these itself.

import { invoke, run, state, status } from "./state.js";
import { $, ago, h } from "./dom.js";
import { openProfile, refresh, renderSidebar } from "./profile.js";
import { openGame } from "./games.js";
import { go } from "./nav.js";

/** Remembers "Not now", so sharing again doesn't ask again. */
const OFFER_DISMISSED = "daemon-offer-dismissed";

/** How many of the daemon's recent doings to show. */
const SHOWN = 15;

/** What each of the daemon's states means, for someone looking from the app. */
const STATES = {
  starting: "It's starting.",
  running: "It's busy with what's below.",
  paused: "It's paused while Patchwork is open, and picks up again once it's closed.",
  waiting: "It's waiting for Patchwork to close.",
  idle: "It has nothing to do for now.",
};

export async function renderDaemon() {
  const [daemon, settings] = await Promise.all([run("daemon_status"), run("daemon_tasks")]);
  show(daemon);
  showTasks(settings, daemon.running?.tasks ?? []);
}

/**
 * A checkbox for each of the daemon's tasks, with when it next runs if it's
 * running. Rows already shown are updated in place, so refreshing doesn't
 * take focus from a checkbox or flip one that's being changed. While the
 * whole daemon's off they're greyed out, and keep their own settings.
 */
function showTasks({ enabled, tasks }, running) {
  const master = $("#daemon-enabled");
  if (!master.disabled) master.checked = enabled;
  const on = master.checked;
  const list = $("#daemon-tasks");
  list.classList.toggle("off", !on);
  const shown = [...list.children];
  const same = shown.length === tasks.length && tasks.every((t, i) => shown[i].dataset.task === t.id);
  if (!same) {
    list.replaceChildren(
      ...tasks.map((task) =>
        h(
          "label",
          { "data-task": task.id },
          h("input", {
            type: "checkbox",
            name: `daemon-task-${task.id}`,
            onchange: (e) => setTask(task.id, e.target),
          }),
          h("span", {}, task.description),
          h("small", { class: "muted" }),
        ),
      ),
    );
  }
  for (const [i, task] of tasks.entries()) {
    const row = list.children[i];
    const input = row.querySelector("input");
    if (!input.dataset.busy) input.checked = task.enabled;
    input.disabled = !on || !!input.dataset.busy;
    const now = running.find((t) => t.id === task.id);
    let note = "";
    if (on && now?.running) note = "running now";
    else if (on && now?.enabled && now.next) note = `next ${new Date(now.next).toLocaleString()}`;
    row.querySelector("small").textContent = note;
  }
}

async function setTask(id, input) {
  input.disabled = true;
  input.dataset.busy = "1";
  try {
    await invoke("daemon_set_task", { id, enabled: input.checked });
    status(input.checked ? "Turned on; it starts once Patchwork is closed" : "Turned off");
  } catch (err) {
    input.checked = !input.checked;
    $("#daemon-error").textContent = String(err);
    $("#daemon-error").hidden = false;
    status("Couldn't change that", true);
  } finally {
    delete input.dataset.busy;
    input.disabled = !$("#daemon-enabled").checked;
  }
}

$("#daemon-enabled").addEventListener("change", async (e) => {
  const input = e.target;
  const list = $("#daemon-tasks");
  const tasks = () => list.querySelectorAll("input");
  input.disabled = true;
  try {
    await invoke("daemon_set_enabled", { enabled: input.checked });
    status(input.checked ? "Turned on; it starts once Patchwork is closed" : "Turned off; it won't do anything in the background");
  } catch (err) {
    input.checked = !input.checked;
    $("#daemon-error").textContent = String(err);
    $("#daemon-error").hidden = false;
    status("Couldn't change that", true);
  } finally {
    input.disabled = false;
    list.classList.toggle("off", !input.checked);
    for (const t of tasks()) t.disabled = !input.checked || !!t.dataset.busy;
  }
});

function show({ installed, running, version }) {
  const lines = [];
  if (installed) lines.push(`Installed as ${installed}, so it starts when you log in.`);
  else if (running) lines.push("Running, but not installed, so it won't start again when you log in.");
  else lines.push("Not installed. Profiles only sync while Patchwork is open.");
  if (running) lines.push(STATES[running.state] ?? "");
  else if (installed) lines.push("It isn't running right now.");
  $("#daemon-state").textContent = lines.join(" ");

  const outdated = running && running.version !== version;
  $("#daemon-version").hidden = !outdated;
  if (outdated) {
    $("#daemon-version").textContent =
      `It's running Patchwork ${running.version}, not ${version} like this app. Restart it to update it.`;
  }

  const activity = running?.activity ?? [];
  $("#daemon-recent").hidden = !running;
  $("#daemon-activity").replaceChildren(
    ...activity
      .slice(-SHOWN)
      .reverse()
      .map((a) =>
        h(
          "li",
          { class: a.error ? "error" : "", title: new Date(a.at).toLocaleString() },
          h("span", { class: "muted" }, ago(a.at)),
          a.message,
        ),
      ),
  );
  $("#daemon-log").textContent = running ? `Everything it does is logged to ${running.log}` : "";

  $("#daemon-install").hidden = !!installed && !!running;
  $("#daemon-install").textContent = installed ? "Start" : "Install";
  $("#daemon-restart").hidden = !outdated;
  $("#daemon-uninstall").hidden = !installed && !running;
  $("#daemon-uninstall").textContent = installed ? "Uninstall" : "Stop";
  $("#daemon-error").hidden = true;
}

/** Runs a daemon command with the tab's buttons disabled, then shows how it's doing. */
async function act(command, doing, done, failed) {
  const buttons = document.querySelectorAll("#daemon button");
  for (const b of buttons) b.disabled = true;
  status(doing);
  try {
    await invoke(command);
    status(done);
  } catch (err) {
    $("#daemon-error").textContent = String(err);
    $("#daemon-error").hidden = false;
    status(failed, true);
    return;
  } finally {
    for (const b of buttons) b.disabled = false;
  }
  await renderDaemon().catch(() => {});
}

$("#daemon-install").addEventListener("click", () => {
  if ($("#daemon-install").textContent === "Start") {
    act("daemon_restart", "Starting…", "Started background sync", "Couldn't start background sync");
  } else {
    act(
      "daemon_install",
      "Installing…",
      "Installed background sync; it takes over once Patchwork is closed",
      "Couldn't install background sync",
    );
  }
});
$("#daemon-restart").addEventListener("click", () =>
  act("daemon_restart", "Restarting…", "Restarted background sync", "Couldn't restart background sync"),
);
$("#daemon-uninstall").addEventListener("click", () =>
  act("daemon_uninstall", "Stopping…", "Stopped background sync", "Couldn't stop background sync"),
);

/** How often the Background tab refreshes itself while it's showing. */
const REFRESH_MS = 3000;

// Keeps the Background tab current while it's open, rather than having a
// Refresh button. Skipped while a button's busy, since act() refreshes
// once it's done.
setInterval(() => {
  const panel = $("#daemon");
  if (panel.hidden || !panel.closest("dialog")?.open) return;
  if ([...panel.querySelectorAll("button")].some((b) => b.disabled)) return;
  Promise.all([invoke("daemon_status"), invoke("daemon_tasks")])
    .then(([daemon, settings]) => {
      const error = $("#daemon-error");
      const shownError = !error.hidden && error.textContent;
      show(daemon);
      showTasks(settings, daemon.running?.tasks ?? []);
      // keep showing what last went wrong
      if (shownError) {
        error.textContent = shownError;
        error.hidden = false;
      }
    })
    .catch(() => {});
}, REFRESH_MS);

// Offering background sync, where something's just been shared or linked.

function offerDismissed() {
  try {
    return localStorage.getItem(OFFER_DISMISSED) === "1";
  } catch {
    return false;
  }
}

/**
 * Offers to install background sync in `slot`, unless it's installed or
 * running already, or the offer's been turned down. Calls `after` once
 * it's been answered, with whether it was installed. True if it's offered.
 */
export async function offerDaemon(slot, after = () => {}) {
  slot.replaceChildren();
  if (offerDismissed()) return false;
  let daemon;
  try {
    daemon = await invoke("daemon_status");
  } catch {
    return false;
  }
  if (daemon.installed || daemon.running) return false;

  const text = h(
    "p",
    {},
    "Patchwork only syncs while it's open. With background sync, changes still go back and forth, " +
      "and others can still join, after you close it, whenever you're logged in.",
  );
  const error = h("p", { class: "error", hidden: true });
  const later = h("button", { type: "button" }, "Not now");
  const install = h("button", { type: "button", class: "primary" }, "Install background sync");
  const buttons = h("div", { class: "row end" }, later, install);
  later.addEventListener("click", () => {
    try {
      localStorage.setItem(OFFER_DISMISSED, "1");
    } catch {
      // asked again next time, then
    }
    slot.replaceChildren();
    after(false);
  });
  install.addEventListener("click", async () => {
    install.disabled = later.disabled = true;
    error.hidden = true;
    status("Installing…");
    try {
      await invoke("daemon_install");
    } catch (err) {
      error.textContent = String(err);
      error.hidden = false;
      status("Couldn't install background sync", true);
      install.disabled = later.disabled = false;
      return;
    }
    text.textContent = "Background sync is installed; it takes over whenever Patchwork is closed. It's under Settings.";
    buttons.remove();
    status("Installed background sync");
    after(true);
  });
  slot.replaceChildren(h("div", { class: "daemon-offer stack" }, text, error, buttons));
  return true;
}

/** After joining a profile, offers background sync in a dialog of its own. */
export async function offerDaemonAfterJoining(label) {
  const dialog = $("#daemon-offer-dialog");
  $("#daemon-offer-title").textContent = `Joined ${label}`;
  // long enough to see that it's installed
  const close = (installed) => setTimeout(() => dialog.close(), installed ? 1500 : 0);
  if (await offerDaemon($("#daemon-offer-slot"), close)) dialog.showModal();
}

// What background sync did while Patchwork was closed, shown when it opens.

/** Shows what background sync changed since Patchwork was last open, if anything. */
export async function showMissed() {
  const missed = await invoke("daemon_missed");
  if (!missed.length) return;
  // they may be new, or have new mods
  await refresh().catch(() => {});
  const dialog = $("#missed-dialog");
  $("#missed-list").replaceChildren(
    ...missed.map((m) => {
      const what = m.created
        ? "Arrived from another computer"
        : m.changed
          ? "Changed on another computer"
          : "Couldn't be synced fully";
      const open = h("button", { type: "button" }, "Open");
      open.addEventListener("click", async () => {
        dialog.close();
        const profile = state.overview.profiles.find((p) => p.id === m.id);
        if (!profile) return;
        await go(() => (profile.game === state.game ? openProfile(m.id) : openGame(profile.game, m.id)));
        renderSidebar();
      });
      return h(
        "li",
        {},
        h(
          "div",
          {},
          h("p", {}, h("b", {}, m.label)),
          h("p", { class: "muted", title: new Date(m.at).toLocaleString() }, `${what}, ${ago(m.at)}`),
          ...m.warnings.map((w) => h("p", { class: "error" }, w)),
        ),
        open,
      );
    }),
  );
  dialog.showModal();
}
