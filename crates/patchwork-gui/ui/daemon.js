// The Background sync tab in Settings: installing the sync daemon, which
// keeps synced profiles in step while Patchwork is closed, and seeing how
// it's doing (see daemon.rs). While Patchwork's open the daemon is paused,
// so this mostly shows what it did while Patchwork was closed.

import { invoke, run, status } from "./state.js";
import { $, ago, h } from "./dom.js";

/** How many of the daemon's recent doings to show. */
const SHOWN = 15;

/** What each of the daemon's states means, for someone looking from the app. */
const STATES = {
  starting: "It's starting.",
  syncing: "It's keeping your profiles in step.",
  paused: "It's paused while Patchwork is open, and picks up again once it's closed.",
  waiting: "It's waiting for Patchwork to close before it syncs.",
  idle: "Nothing is synced on this computer yet, so it has nothing to do. Share a profile or link your computers first.",
};

export async function renderDaemon() {
  show(await run("daemon_status"));
}

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
$("#daemon-refresh").addEventListener("click", () => renderDaemon().catch(() => {}));
