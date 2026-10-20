// Deploying the profile, updating its mods, and progress from main.rs.

import { listen, run, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { openProfile } from "./profile.js";

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
    // update_lock has the mods that changed get their details again
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
