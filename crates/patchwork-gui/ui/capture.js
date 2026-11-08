// Capturing tool output. Tools like DynDOLOD, Nemesis or BodySlide write
// into the game's folder; this keeps what they wrote since the profile was
// deployed as a mod, then deploys again so it's put back from there. Like
// `patchwork capture`.

import { run, state, status } from "./state.js";
import { $, h } from "./dom.js";
import { openProfile } from "./profile.js";
import { deployProfile } from "./deploy.js";

const dialog = $("#capture-dialog");
const form = $("#capture-form");

function showError(err) {
  $("#capture-error").textContent = String(err);
  $("#capture-error").hidden = !err;
}

const files = (n) => `${n} file${n === 1 ? "" : "s"}`;

$("#capture").addEventListener("click", async () => {
  $("#deploy-menu").open = false;
  const p = state.profile;
  form.reset();
  showError(null);
  $("#capture-summary").textContent = "";
  $("#capture-warnings").replaceChildren();
  $("#capture-files").replaceChildren();
  // mods captured into before can be added to
  $("#capture-names").replaceChildren(
    ...p.mods.filter((m) => m.source === "local").map((m) => h("option", { value: m.name })),
  );
  status("Looking for what tools wrote…");
  let output;
  try {
    output = await run("tool_output", { id: p.id });
  } catch (err) {
    // usually that it isn't deployed; say so where it's being looked for
    showError(err);
    output = { files: [], changed: [] };
  }
  status("Ready");
  const count = output.files.length;
  $("#capture-summary").textContent = count
    ? `Tools have written ${files(count)} into the game's folder since ${p.name} was deployed.`
    : $("#capture-error").hidden
      ? `Nothing new in the game's folder since ${p.name} was deployed. Run the tool, then capture what it made.`
      : "";
  if (output.changed.length) {
    $("#capture-warnings").replaceChildren(
      h(
        "li",
        {},
        `The game's own ${output.changed.length === 1 ? "file was" : "files were"} changed, ` +
          `and can't be kept as a mod: ${output.changed.join(", ")}`,
      ),
    );
  }
  $("#capture-files").replaceChildren(
    ...output.files.map((f) =>
      h("li", {}, `+ ${f.path}`, f.replaces && h("span", { class: "muted" }, `  (replaces ${f.replaces}'s)`)),
    ),
  );
  $("#capture-name-field").hidden = !count;
  $("#capture-submit").disabled = !count;
  dialog.showModal();
});

form.querySelector("[value=cancel]").addEventListener("click", () => dialog.close());

form.addEventListener("submit", async (e) => {
  e.preventDefault();
  const p = state.profile;
  const name = form.elements.name.value.trim();
  const submit = $("#capture-submit");
  submit.disabled = true;
  showError(null);
  let captured;
  try {
    captured = await run("capture", { id: p.id, name });
  } catch (err) {
    showError(err);
    return;
  } finally {
    submit.disabled = false;
  }
  dialog.close();
  // the sidebar's count of its mods
  state.overview = await run("overview");
  await openProfile(p.id);
  // They were taken out of the game's folder, so they go back in from the
  // mod.
  const deployed = await deployProfile(p.id);
  const where = captured.added ? `the new mod ${captured.name}` : captured.name;
  const what = `Captured ${files(captured.output.files.length)} into ${where}`;
  status(deployed ? `${what} and deployed it` : `${what}; deploy to put them back`);
});
