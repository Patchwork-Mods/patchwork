// Deploying the profile, updating its mods, and progress from main.rs.

import { gameInfo, listen, run, state, status } from "./state.js";
import { $, h, link } from "./dom.js";
import { openProfile, renderDeploy, showProfile } from "./profile.js";
import { renderEntries } from "./changelog.js";

// Deploying doesn't ask first: undeploying, or reverting to an earlier
// deploy in History, takes it back. While one runs, the button that started
// it (Deploy, or Play deploying first) turns into Cancel, which stops it
// before the next file.

/**
 * Deploys profile `id`, with `button` (a selector) showing Cancel until it's
 * done. With `verify`, every deployed file is read to check nothing's
 * changed it since, rather than just that it's there and the right size.
 * Resolves to whether it deployed; it says how it went in the status bar
 * either way.
 */
export async function deployProfile(id, { verify = false, button = "#deploy", message } = {}) {
  status(message ?? (verify ? "Checking deployed files…" : "Working out what to deploy…"));
  const outcome = await job(button, () => run("deploy", { id, verify }), id);
  if (outcome.outcome === "cancelled") {
    status(
      outcome.partway
        ? "Deploy stopped partway; deploy again to finish it, or undeploy to take it back out"
        : "Deploy cancelled; nothing in the game was changed",
    );
    return false;
  }
  const { changes, files, unchanged, warnings } = outcome;
  const done = changes
    ? `Deployed ${changes} change${changes === 1 ? "" : "s"}, ${files} file${files === 1 ? "" : "s"} in place`
    : `Nothing to deploy; ${unchanged} file${unchanged === 1 ? "" : "s"} already in place`;
  showWarnings(done, warnings);
  return true;
}

/** Deploys profile `id` before playing, with Play showing Cancel meanwhile. */
export const deployFirst = (id) =>
  deployProfile(id, { button: "#play", message: "Deploying before playing…" });

/**
 * Runs `work` as the job `button` cancels, and shows profile `id` again once
 * it's done (or failed), since what's deployed has changed either way.
 */
async function job(button, work, id) {
  state.job = { button, cancelled: false };
  $("#deploy-menu").open = false;
  $("#play-menu").open = false;
  renderDeploy(state.profile);
  try {
    return await work();
  } finally {
    state.job = null;
    if (id && state.profile?.id === id) {
      await openProfile(id).catch(() => {});
    } else {
      renderDeploy(state.profile);
    }
  }
}

/** Calls off the running deploy or undeploy, if there is one. */
export function cancelJob() {
  if (!state.job || state.job.cancelled) return;
  state.job.cancelled = true;
  renderDeploy(state.profile);
  status("Cancelling…");
  run("cancel_job").catch(() => {});
}

/** Says `message` in the status bar, with a button to read `warnings` if there are any. */
function showWarnings(message, warnings) {
  if (!warnings.length) return status(message);
  const count = warnings.length;
  status(`${message}, with ${count} warning${count === 1 ? "" : "s"}`);
  $("#warnings-list").replaceChildren(...warnings.map((w) => h("li", {}, w)));
  $("#status-warnings").hidden = false;
}

$("#status-warnings").addEventListener("click", () => $("#warnings-dialog").showModal());

$("#deploy").addEventListener("click", () => {
  if (state.job) return cancelJob();
  deployProfile(state.profile.id).catch(() => {});
});

$("#verify-deploy").addEventListener("click", () => {
  $("#deploy-menu").open = false;
  deployProfile(state.profile.id, { verify: true }).catch(() => {});
});

// Escape cancels too, unless it's closing a dialog.
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && state.job && !document.querySelector("dialog[open]")) cancelJob();
});

// Undeploy. Takes every file patchwork put in the game folder (or the open
// profile's instance) back out and puts back the game's own files that mods
// replaced, whichever profile was deployed. The profiles themselves aren't
// touched. Unlike deploying, it asks first.

$("#undeploy").addEventListener("click", async () => {
  $("#deploy-menu").open = false;
  const game = gameInfo(state.game);
  status("Working out what to undeploy…");
  const plan = await run("plan_undeploy", { game: game.id, id: state.profile?.id });
  const files = plan.ops.length;
  $("#plan-dialog [value=apply]").disabled = !files;
  $("#plan-summary").textContent =
    `Remove ${files} file${files === 1 ? "" : "s"} patchwork put in ${game.name}'s folder ` +
    `and put back any of the game's own files they replaced? Your profiles stay as they are.`;
  $("#plan-warnings").replaceChildren(...plan.warnings.map((w) => h("li", {}, w)));
  $("#plan-ops").replaceChildren(...plan.ops.map(opRow));
  status("Ready");
  $("#plan-dialog").returnValue = "";
  $("#plan-dialog").showModal();
});

$("#plan-dialog").addEventListener("close", async () => {
  if ($("#plan-dialog").returnValue !== "apply") return;
  const name = gameInfo(state.game).name;
  status("Undeploying…");
  const outcome = await job("#deploy", () => run("apply_plan"), state.profile?.id);
  if (outcome.outcome === "cancelled") {
    status("Undeploy stopped partway; undeploy again to finish it");
  } else {
    showWarnings(`Undeployed everything from ${name}`, outcome.warnings);
  }
});

function opRow(op) {
  switch (op.op) {
    case "remove":
      return h("li", { class: "remove" }, `− ${op.dst}`);
    case "backup":
      return h("li", {}, `~ ${op.dst} (backing up original)`);
    case "place":
      return h("li", {}, op.modName ? `+ ${op.dst}  [${op.modName}]` : `+ ${op.dst}`);
  }
}

// Revert. Puts the profile's mods and locked versions back how they were at
// its last deploy; the game itself isn't touched.

$("#revert").addEventListener("click", () => {
  const dialog = $("#revert-dialog");
  $("#revert-summary").textContent =
    `Undo every change made to ${state.profile.label} since it was last deployed?`;
  dialog.returnValue = "";
  dialog.showModal();
});

$("#revert-dialog").addEventListener("close", async () => {
  if ($("#revert-dialog").returnValue !== "revert") return;
  showProfile(await run("revert_profile", { id: state.profile.id }));
  status("Reverted to the last deploy");
});

// Update. Checks which mods that follow the latest version have newer ones
// out, and shows what each would bring in from its changelog. The ones left
// ticked are then relocked at their newest versions, like `patchwork lock
// --update`; pinned mods stay where they are. Nothing changes in the game
// until the next deploy.

$("#update").addEventListener("click", async () => {
  $("#update").disabled = true;
  await previewUpdates();
});

/** Offers to move one mod that follows the latest version to the newest one. */
export function updateMod(m) {
  return previewUpdates([m.key], m.name);
}

/**
 * Shows #update-dialog with the updates for the mods in `keys` (every mod,
 * if not given; `name` is the one mod's) and makes the ones that are kept
 * ticked.
 */
async function previewUpdates(keys, name) {
  const id = state.profile.id;
  status(name ? `Checking for updates to ${name}…` : "Checking for updates…");
  try {
    const { updates, failures } = await run("preview_updates", { id, keys });
    for (const f of failures) console.warn(f);
    if (!updates.length) {
      if (failures.length) {
        const count = failures.length;
        status(`Couldn't check ${count} mod${count === 1 ? "" : "s"} for updates`, true);
      } else {
        status(name ? `${name} is already up to date` : "Everything's up to date");
      }
      return;
    }
    const picked = await confirmUpdates(updates, failures);
    if (!picked.length) {
      status("Ready");
      return;
    }
    status(picked.length === 1 ? `Updating ${picked[0].name}…` : `Updating ${picked.length} mods…`);
    const changes = await run("update_mods", { id, keys: picked.map((u) => u.key) });
    const updated = changes.filter((c) => picked.some((u) => u.name === c.name));
    const extra = changes.length - updated.length;
    if (!changes.length) {
      status("Nothing needed updating after all");
      return;
    }
    status(
      (updated.length === 1
        ? `Updated ${updated[0].name} to ${updated[0].to ?? "(no version)"}`
        : `Updated ${updated.length} mods`) +
        (extra ? `, and ${extra} more they need` : "") +
        "; deploy to apply",
    );
  } finally {
    // the mods that changed get their details again, and the versions just
    // checked show up
    await openProfile(id);
  }
}

/**
 * Lists `updates` in #update-dialog, each with its changelog and ticked, and
 * resolves to the ones still ticked once it's confirmed (none if cancelled).
 */
function confirmUpdates(updates, failures) {
  const dialog = $("#update-dialog");
  const confirm = $("#update-dialog [value=update]");
  const count = updates.length;
  const boxes = [];
  const sync = () => {
    const ticked = boxes.filter((b) => b.checked).length;
    confirm.disabled = !ticked;
    confirm.textContent = ticked === count ? "Update" : `Update ${ticked}`;
  };
  $("#update-title").textContent = count === 1 ? `Update ${updates[0].name}?` : `Update ${count} mods?`;
  $("#update-summary").textContent =
    count === 1
      ? "Here's what's changed. Nothing changes in the game until you deploy."
      : "Here's what's changed in each. Untick any to leave as they are; nothing changes in the game until you deploy.";
  $("#update-warnings").replaceChildren(
    ...failures.map((f) => h("li", {}, `Couldn't check ${f}`)),
  );
  $("#update-list").replaceChildren(
    ...updates.map((u) => {
      const box = h("input", {
        type: "checkbox",
        checked: true,
        "aria-label": `Update ${u.name}`,
        onchange: sync,
      });
      boxes.push(box);
      const notes = u.changes.length
        ? renderEntries(u.changes)
        : [
            h(
              "p",
              { class: "muted" },
              u.changelogError
                ? `Couldn't get the changelog: ${u.changelogError}`
                : "No changelog for these versions.",
              u.url && [" ", link(u.url, "See the mod's page")],
            ),
          ];
      return h(
        "li",
        {},
        box,
        h(
          "details",
          // a long list starts folded up, so it can be read through
          { open: count <= 3 },
          h(
            "summary",
            {},
            h("span", { class: "mod-name" }, u.name),
            h("span", { class: "muted" }, ` ${u.from ?? "(new)"} → ${u.to}`),
          ),
          h("div", { class: "changelog" }, notes),
        ),
      );
    }),
  );
  sync();
  dialog.returnValue = "";
  dialog.showModal();
  return new Promise((resolve) =>
    dialog.addEventListener(
      "close",
      () =>
        resolve(dialog.returnValue === "update" ? updates.filter((_, i) => boxes[i].checked) : []),
      { once: true },
    ),
  );
}

/** Whether a mod follows the latest version and a newer one than it's locked at is out. */
const canUpdate = (m) =>
  m.versioned && !m.version && m.locked && m.versions?.[0] && m.versions[0] !== m.locked;

/**
 * Has the Update button stand out, as "Update (N)", while any mod that
 * follows the latest version has a newer one it could move to.
 */
export function markUpdates() {
  const count = state.profile?.mods.filter(canUpdate).length ?? 0;
  const button = $("#update");
  button.classList.toggle("has-updates", count > 0);
  button.textContent = count ? `Update (${count})` : "Update";
}

// Progress from long-running commands (adding, locking, deploying).

listen("patchwork://event", ({ payload: e }) => {
  // "Cancelling…" stays up until the deploy stops
  if (state.job?.cancelled && e.kind !== "warning") return;
  switch (e.kind) {
    case "resolving":
      return status(`Resolving ${e.name}…`);
    case "fetching":
      return status(`Downloading ${e.name}…`);
    case "building":
      return status(`Building ${e.step}…`);
    case "applying":
      return status(`${e.op === "remove" ? "Removing" : "Deploying"} ${e.dst}`);
    case "warning":
      return status(e.message, true);
  }
});
