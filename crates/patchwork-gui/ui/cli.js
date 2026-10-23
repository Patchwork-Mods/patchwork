// The button that puts the patchwork CLI bundled with the app on PATH, for
// installs that can't do it themselves. See cli.rs.

import { invoke, run, status } from "./state.js";
import { $ } from "./dom.js";

const button = $("#install-cli");

async function refreshButton() {
  const { canInstall } = await invoke("cli_status");
  button.hidden = !canInstall;
}

button.addEventListener("click", async () => {
  button.disabled = true;
  try {
    status(await run("install_cli"));
  } catch {
    return;
  } finally {
    button.disabled = false;
  }
  await refreshButton();
});

refreshButton().catch(() => {});
