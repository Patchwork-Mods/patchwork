// Updating patchwork itself: the button at the top of the game picker, and
// the dialog it opens when there's a newer release. See check_for_update,
// install_update and cancel_update in selfupdate.rs.

import { invoke, status } from "./state.js";
import { $, openExternal } from "./dom.js";

const { Channel } = window.__TAURI__.core;

const button = $("#self-update");
const dialog = $("#self-update-dialog");
const install = $("#self-update-install");
const later = $("#self-update-later");
const stop = $("#self-update-stop");
const progressBox = $("#self-update-progress");
const bar = $("#self-update-bar");
const progressText = $("#self-update-progress-text");
const error = $("#self-update-error");
/** The last check_for_update, for the dialog's buttons. */
let found = null;
/** Whether install_update is running, which the dialog has to stay open for. */
let updating = false;

async function check() {
  found = await invoke("check_for_update");
  button.querySelector(".update-mark").hidden = !found.available;
  return found;
}

/** Puts the dialog's buttons back as they are before an update starts. */
function resetDialog() {
  progressBox.hidden = true;
  stop.hidden = true;
  later.hidden = false;
  install.hidden = !found.installable;
  install.disabled = false;
  install.textContent = "Update and restart";
}

button.addEventListener("click", async () => {
  // Closed mid-update somehow; show how it's going again.
  if (updating) {
    dialog.showModal();
    return;
  }
  button.disabled = true;
  status("Checking for updates…");
  try {
    await check();
  } catch (e) {
    status(`Couldn't check for updates: ${e}`, true);
    return;
  } finally {
    button.disabled = false;
  }
  if (!found.available) {
    status(`Patchwork ${found.current} is up to date`);
    return;
  }
  status(`Patchwork ${found.latest} is available`);
  $("#self-update-summary").textContent =
    `Patchwork ${found.latest} is out. You have ${found.current}.`;
  $("#self-update-manual").hidden = found.installable;
  $("#self-update-download").hidden = found.installable;
  error.hidden = true;
  resetDialog();
  dialog.showModal();
});

$("#self-update-download").addEventListener("click", () => {
  openExternal(found.releasesUrl);
  dialog.close();
});

// Escape would hide the dialog while the update carries on behind it.
dialog.addEventListener("cancel", (e) => {
  if (updating) e.preventDefault();
});

const megabytes = (bytes) => `${(bytes / 1e6).toFixed(1)} MB`;

/** Shows how the update's going, in the dialog and the status bar. */
function showProgress(p) {
  let text;
  if (p.kind === "downloading") {
    if (p.total) {
      bar.max = p.total;
      bar.value = p.downloaded;
      text = `Downloading ${megabytes(p.downloaded)} of ${megabytes(p.total)}`;
    } else {
      bar.removeAttribute("value");
      text = `Downloading ${megabytes(p.downloaded)}`;
    }
  } else {
    // Past the point it can be stopped.
    stop.disabled = true;
    bar.removeAttribute("value");
    text = "Installing; Patchwork will restart when it's done";
  }
  progressText.textContent = text;
  status(`Updating Patchwork: ${text}`);
}

install.addEventListener("click", async () => {
  updating = true;
  install.disabled = true;
  install.textContent = "Updating…";
  error.hidden = true;
  later.hidden = true;
  stop.hidden = false;
  stop.disabled = false;
  progressBox.hidden = false;
  bar.removeAttribute("value");
  progressText.textContent = "Starting download…";
  const progress = new Channel();
  progress.onmessage = showProgress;
  let outcome;
  try {
    // Restarts into the new version once it's in place.
    outcome = await invoke("install_update", { progress });
  } catch (e) {
    error.textContent = String(e);
    error.hidden = false;
    status(`Couldn't update Patchwork: ${e}`, true);
    return;
  } finally {
    updating = false;
    resetDialog();
  }
  dialog.close();
  if (outcome === "cancelled") {
    status("Update cancelled");
  } else {
    // It turned out there was nothing to install.
    status(`Patchwork ${found.current} is up to date`);
  }
});

stop.addEventListener("click", () => {
  stop.disabled = true;
  invoke("cancel_update").catch(() => {});
});

// Quietly, so being offline at startup doesn't greet people with an error.
check().catch(() => {});
