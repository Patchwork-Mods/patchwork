// Deploying the profile, updating its mods, and progress from main.rs.

import { gameInfo, listen, run, state, status } from "./state.js";
import { $, h, link } from "./dom.js";
import { openProfile, renderDeploy, showProfile } from "./profile.js";
import { renderEntries } from "./changelog.js";

/** Whether #plan-dialog is confirming a deploy or an undeploy. */
let planKind = "deploy";

/**
 * While play.js is waiting on a deploy being confirmed, settles it with
 * whether it was applied.
 */
let afterDeploy = null;

/**
 * Deploys profile `id` before playing: straight away if nothing would
 * change, otherwise once the user has confirmed the plan. Resolves to
 * whether it was deployed.
 */
export async function deployFirst(id) {
  status("Working out what to deploy first…");
  const plan = await run("plan_deploy", { id });
  const changes = plan.ops.length;
  if (!changes) {
    await run("apply_plan");
    await openProfile(id);
    return true;
  }
  showPlan(
    "deploy",
    plan,
    `${changes} change${changes === 1 ? "" : "s"} to deploy before playing, ` +
      `${plan.unchanged} files already in place.`,
  );
  $("#plan-dialog [value=apply]").textContent = "Deploy and play";
  return new Promise((resolve) => (afterDeploy = resolve));
}

/** Fills in #plan-dialog for `plan`, worded for `kind`, and opens it. */
function showPlan(kind, plan, summary) {
  planKind = kind;
  const apply = $("#plan-dialog [value=apply]");
  $("#plan-title").textContent = kind === "undeploy" ? "Undeploy" : "Deploy";
  apply.textContent = kind === "undeploy" ? "Undeploy" : "Apply";
  apply.className = kind === "undeploy" ? "danger" : "primary";
  apply.disabled = !plan.ops.length;
  $("#plan-summary").textContent = summary;
  $("#plan-warnings").replaceChildren(...plan.warnings.map((w) => h("li", {}, w)));
  $("#plan-ops").replaceChildren(...plan.ops.map(opRow));
  status("Ready");
  $("#plan-dialog").showModal();
}

$("#deploy").addEventListener("click", async () => {
  const id = state.profile.id;
  $("#deploy").disabled = true;
  status("Working out what to deploy…");
  try {
    const plan = await run("plan_deploy", { id });
    const changes = plan.ops.length;
    showPlan(
      "deploy",
      plan,
      changes
        ? `${changes} change${changes === 1 ? "" : "s"}, ${plan.unchanged} files already in place.`
        : `Nothing to do; ${plan.unchanged} files already in place.`,
    );
    if (!changes) {
      // Nothing to place, but applying records that this profile is
      // deployed as it is now, which tones the button down again.
      await run("apply_plan");
      await openProfile(id);
    }
  } finally {
    renderDeploy(state.profile);
  }
});

// Undeploy. Takes every file patchwork put in the game folder back out and
// puts back the game's own files that mods replaced, whichever profile was
// deployed. The profiles themselves aren't touched.

$("#undeploy").addEventListener("click", async () => {
  $("#deploy-menu").open = false;
  const game = gameInfo(state.game);
  status("Working out what to undeploy…");
  const plan = await run("plan_undeploy", { game: game.id });
  const files = plan.ops.length;
  showPlan(
    "undeploy",
    plan,
    `Remove ${files} file${files === 1 ? "" : "s"} patchwork put in ${game.name}'s folder ` +
      `and put back any of the game's own files they replaced? Your profiles stay as they are.`,
  );
});

$("#plan-dialog").addEventListener("close", async () => {
  const settle = afterDeploy ?? (() => {});
  afterDeploy = null;
  if ($("#plan-dialog").returnValue !== "apply") return settle(false);
  let applied = false;
  try {
    if (planKind === "undeploy") {
      status("Undeploying…");
      await run("apply_plan");
      status(`Undeployed everything from ${gameInfo(state.game).name}`);
    } else {
      status("Deploying…");
      const files = await run("apply_plan");
      status(`Deployed ${files} files`);
    }
    applied = true;
    await openProfile(state.profile.id);
  } finally {
    settle(applied);
  }
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
  switch (e.kind) {
    case "resolving":
      return status(`Resolving ${e.name}…`);
    case "fetching":
      return status(`Downloading ${e.name}…`);
    case "building":
      return status(`Building ${e.step}…`);
    case "applying":
      return status(`Deploying ${e.dst}`);
    case "warning":
      return status(e.message, true);
  }
});
