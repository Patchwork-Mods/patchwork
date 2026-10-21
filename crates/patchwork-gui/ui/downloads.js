// Mods the user has to download themselves, because their site (Nexus,
// without premium) won't hand patchwork the file. Locking sends them as
// patchwork://pending. Each is done once the browser hands back its "Mod
// Manager Download" link (see accept_links in main.rs) or the user picks a
// file they downloaded, and the profile is locked again once they all are.

import { invoke, listen, run, state, status } from "./state.js";
import { $, h, openExternal } from "./dom.js";
import { showProfile } from "./profile.js";

/** The pending downloads, as `{ pending, done }`, and whose they are. */
let waiting = [];
let profileId = null;

const sameFile = (a, b) =>
  a.game === b.game && a.source === b.source && a.id === b.id && a.file === b.file;

listen("patchwork://pending", ({ payload }) => {
  waiting = payload.map((pending) => ({ pending, done: false }));
  profileId = state.profile?.id ?? null;
  render();
  const dialog = $("#downloads-dialog");
  if (!dialog.open) dialog.showModal();
  // so "Mod Manager Download" links come back here
  invoke("handle_links").catch((err) => status(`Couldn't take links from the browser: ${err}`, true));
});

listen("patchwork://link", ({ payload: e }) => {
  switch (e.kind) {
    case "downloading":
      return status("Downloading from the browser…");
    case "failed":
      return status(e.message, true);
    case "downloaded": {
      const item = waiting.find((w) => sameFile(w.pending.file, e.file));
      if (item) return markDone(item);
      return status(`Downloaded ${e.file.source}:${e.file.id}; it's used whenever a profile needs it`);
    }
  }
});

function render() {
  $("#downloads-list").replaceChildren(
    ...waiting.map((item) =>
      h(
        "li",
        // .row too, so it's laid out even with an old copy of default.css
        { class: item.done ? "row done" : "row" },
        h("span", { class: "name" }, item.pending.name),
        item.done
          ? h("span", { class: "muted" }, "Downloaded")
          : [
              h("button", { type: "button", onclick: () => openExternal(item.pending.page) }, "Open page"),
              h("button", { type: "button", onclick: () => chooseFile(item) }, "Choose file…"),
            ],
      ),
    ),
  );
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
  status(`Downloaded ${item.pending.name}`);
  render();
  if (waiting.every((w) => w.done)) finish().catch(() => {});
}

/** Locks the profile again now that everything it was waiting on is here. */
async function finish() {
  $("#downloads-dialog").close();
  waiting = [];
  if (!profileId) return;
  status("Locking…");
  const profile = await run("lock_profile", { id: profileId });
  if (state.profile?.id === profileId) showProfile(profile);
  status("Everything's downloaded; deploy to apply");
}
