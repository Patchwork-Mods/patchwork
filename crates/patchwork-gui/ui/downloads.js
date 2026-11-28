// Mods the user has to download themselves, because their site (Nexus,
// without premium) won't hand patchwork the file. Locking sends them as
// patchwork://pending. Each is done once the browser hands back its "Mod
// Manager Download" link (see accept_links in main.rs) or the user picks a
// file they downloaded, and the profile is locked again once they all are.
// Each row shows how its download is going: a bar while it downloads (styled
// like a mod row's, see progress.js), and why if it failed.

import { invoke, listen, run, state, status } from "./state.js";
import { $, h, openExternal } from "./dom.js";
import { showProfile } from "./profile.js";

/**
 * The pending downloads, and whose they are. Each is `{ pending, done }`,
 * plus while it downloads `progress` (the name its progress comes under in
 * patchwork://event, and how far it's got) and, if it failed, `failed`.
 */
let waiting = [];
let profileId = null;

const sameFile = (a, b) =>
  a.game === b.game && a.source === b.source && a.id === b.id && a.file === b.file;

listen("patchwork://pending", ({ payload }) => {
  // Deploying again while some are still on their way keeps how they're
  // going, rather than starting their rows over.
  const id = state.profile?.id ?? null;
  const before = id === profileId ? waiting : [];
  waiting = payload.map(
    (pending) => before.find((w) => sameFile(w.pending.file, pending.file)) ?? { pending, done: false },
  );
  profileId = id;
  render();
  $("#downloads-status").hidden = true;
  const dialog = $("#downloads-dialog");
  if (!dialog.open) dialog.showModal();
  // so "Mod Manager Download" links come back here
  invoke("handle_links").catch((err) => status(`Couldn't take links from the browser: ${err}`, true));
});

/** Says `text` in the status bar, and in the dialog while it's open, where
 * background messages (like sync's) can't cover it up. */
function say(text, isError = false) {
  status(text, isError);
  const line = $("#downloads-status");
  line.textContent = text;
  line.classList.toggle("error", isError);
  line.hidden = false;
}

const itemFor = (file) => (file ? waiting.find((w) => sameFile(w.pending.file, file)) : undefined);

listen("patchwork://link", ({ payload: e }) => {
  const item = itemFor(e.file);
  switch (e.kind) {
    case "downloading":
      if (!item) return say("Downloading from the browser…");
      item.failed = null;
      item.progress = { name: e.name, downloaded: 0, total: null };
      say(`Downloading ${item.pending.name}…`);
      return render();
    case "failed":
      if (!item) return say(`Couldn't download from the browser: ${e.message}`, true);
      item.progress = null;
      item.failed = e.message;
      say(`Couldn't download ${item.pending.name}: ${e.message}`, true);
      return render();
    case "downloaded":
      if (item) return markDone(item);
      return say(`Downloaded ${e.file.source}:${e.file.id}; it's used whenever a profile needs it`);
  }
});

// How far each one's got, updating its bar in place rather than drawing the
// list again every tenth of a second.
listen("patchwork://event", ({ payload: e }) => {
  if (e.kind !== "downloading") return;
  const index = waiting.findIndex((w) => w.progress?.name === e.name);
  if (index < 0) return;
  Object.assign(waiting[index].progress, { downloaded: e.downloaded, total: e.total ?? null });
  const row = $("#downloads-list").children[index];
  if (row) showProgress(row, waiting[index].progress);
});

const megabytes = (bytes) => `${(bytes / 1e6).toFixed(1)} MB`;

function showProgress(row, { downloaded, total }) {
  const value = total ? Math.min(1, downloaded / total) : null;
  row.classList.toggle("indeterminate", value === null);
  row.style.setProperty("--pw-download-progress", String(value ?? 0));
  const text = total ? `${megabytes(downloaded)} of ${megabytes(total)}` : `${megabytes(downloaded)}`;
  const label = row.querySelector(".progress-text");
  if (label) label.textContent = text;
  row.setAttribute("aria-valuenow", String(Math.round((value ?? 0) * 100)));
  row.setAttribute("aria-valuetext", text);
}

function render() {
  $("#downloads-list").replaceChildren(...waiting.map(row));
  const left = waiting.filter((w) => !w.done).length;
  const button = $("#status-downloads");
  button.hidden = !left;
  button.textContent = `Downloads (${left} left)`;
  button.title = "Show the mods waiting on downloads from the browser";
}

$("#status-downloads").addEventListener("click", () => {
  const dialog = $("#downloads-dialog");
  if (!dialog.open) dialog.showModal();
});

function row(item) {
  const buttons = [
    h("button", { type: "button", onclick: () => openExternal(item.pending.page) }, "Open page"),
    h("button", { type: "button", onclick: () => chooseFile(item) }, "Choose file…"),
  ];
  if (item.done) {
    return h(
      "li",
      { class: "row done" },
      h("span", { class: "name" }, item.pending.name),
      h("span", { class: "muted" }, "Downloaded"),
    );
  }
  if (item.progress) {
    const li = h(
      "li",
      { class: "row downloading", role: "progressbar", "aria-label": item.pending.name },
      h("span", { class: "name" }, item.pending.name),
      h("span", { class: "muted progress-text" }),
    );
    showProgress(li, item.progress);
    return li;
  }
  if (item.failed) {
    return h(
      "li",
      { class: "row failed" },
      h(
        "span",
        { class: "name" },
        item.pending.name,
        h("span", { class: "error", title: item.failed }, `Couldn't download: ${item.failed}`),
      ),
      ...buttons,
    );
  }
  return h("li", { class: "row" }, h("span", { class: "name" }, item.pending.name), ...buttons);
}

async function chooseFile(item) {
  try {
    const path = await window.__TAURI__.dialog.open({ directory: false, multiple: false });
    if (!path) return;
    await run("provide_download", { file: item.pending.file, path });
    markDone(item);
  } catch (err) {
    status(String(err), true);
  }
}

function markDone(item) {
  item.done = true;
  item.progress = item.failed = null;
  say(`Downloaded ${item.pending.name}`);
  render();
  if (waiting.every((w) => w.done)) finish().catch(() => {});
}

/** Locks the profile again now that everything it was waiting on is here. */
async function finish() {
  $("#downloads-dialog").close();
  waiting = [];
  render();
  if (!profileId) return;
  status("Locking…");
  const profile = await run("lock_profile", { id: profileId });
  if (state.profile?.id === profileId) showProfile(profile);
  status("Everything's downloaded; deploy to apply");
}
