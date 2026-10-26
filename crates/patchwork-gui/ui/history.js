// History. Every deploy that changed something is kept, newest first here,
// with what changed since the one before it. Rolling back puts the profile's
// mods and locked versions back how they were at one, like `patchwork
// rollback`; the game itself isn't touched until the next deploy, and the
// Revert button undoes the rollback like any other change.

import { $, h } from "./dom.js";
import { run, state, status } from "./state.js";
import { showProfile } from "./profile.js";

const when = (at) =>
  at
    ? new Date(at).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" })
    : "Unknown time";

const version = (v) => (v ? ` ${v}` : "");

function changeText(c) {
  switch (c.kind) {
    case "added":
      return `+ ${c.name}${version(c.version)}`;
    case "removed":
      return `− ${c.name}${version(c.version)}`;
    case "updated":
      return `${c.name} ${c.from ?? "?"} → ${c.to ?? "?"}`;
    case "enabled":
      return `Enabled ${c.name}`;
    case "disabled":
      return `Disabled ${c.name}`;
    case "reordered":
      return "Reordered mods";
    case "pluginOrder":
      return "Changed the plugin load order";
  }
}

function entryRow(entry, oldest) {
  const rollback = entry.current
    ? h("span", { class: "muted" }, "Current")
    : h(
        "button",
        { type: "button", onclick: () => rollBack(entry.number) },
        "Roll back",
      );
  const note = oldest ? "Oldest kept" : entry.changes.length ? null : "No changes";
  return h(
    "li",
    {},
    h(
      "div",
      { class: "history-head" },
      h("b", {}, `#${entry.number}`),
      h("span", {}, when(entry.deployedAt)),
      h("span", { class: "muted" }, `${entry.enabled} enabled`),
      rollback,
    ),
    note ? h("p", { class: "muted" }, note) : null,
    entry.changes.length
      ? h("ul", { class: "history-changes" }, entry.changes.map((c) => h("li", {}, changeText(c))))
      : null,
  );
}

async function rollBack(number) {
  $("#history-dialog").close();
  showProfile(await run("rollback_profile", { id: state.profile.id, number }));
  status(`Rolled back to deploy #${number}; deploy to apply it`);
}

$("#history").addEventListener("click", async () => {
  $("#deploy-menu").open = false;
  const history = await run("profile_history", { id: state.profile.id });
  $("#history-list").replaceChildren(
    ...(history.length
      ? history.map((e, i) => entryRow(e, i === history.length - 1 && history.length > 1))
      : [h("li", { class: "muted" }, `${state.profile.label} hasn't been deployed yet.`)]),
  );
  $("#history-dialog").showModal();
});
