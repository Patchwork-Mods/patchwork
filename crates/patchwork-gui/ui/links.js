// Which program the system opens nxm:// links with, under Accounts in
// Settings, and making it Patchwork (see links.rs), so Nexus's "Mod Manager
// Download" buttons send their downloads here (see downloads.js).

import { invoke, run, status } from "./state.js";
import { $ } from "./dom.js";

/** What to say about each handler (see LinkHandler in links.rs). */
function describe(handler) {
  switch (handler.kind) {
    case "patchwork":
      return "Patchwork opens them.";
    case "otherCopy":
      return `Another copy of Patchwork opens them, at ${handler.exec}.`;
    case "other":
      return handler.name
        ? `Another program opens them (${handler.name}).`
        : "Another program opens them, or nothing does.";
    case "nothing":
      return "Nothing opens them yet.";
    case "bundled":
      return "Patchwork opens them; that comes with installing it.";
  }
}

export async function renderLinks() {
  const handler = await run("link_handler");
  $("#links-state").textContent = describe(handler);
  $("#links-handle").hidden = handler.kind === "patchwork" || handler.kind === "bundled";
  $("#links-error").hidden = true;
}

$("#links-handle").addEventListener("click", async () => {
  const button = $("#links-handle");
  button.disabled = true;
  $("#links-error").hidden = true;
  try {
    await invoke("handle_links");
  } catch (err) {
    $("#links-error").textContent = String(err);
    $("#links-error").hidden = false;
    status("Couldn't make Patchwork open nxm:// links", true);
    return;
  } finally {
    button.disabled = false;
  }
  status("Patchwork opens nxm:// links now");
  await renderLinks().catch(() => {});
});
