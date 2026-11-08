// Profiles kept in step with other computers, three ways: My computers links
// the user's own, so every profile is on all of them; Share… shares one
// profile with friends, who can all change it, or read only, for others to
// follow. Either makes a ticket to paste on the other computer. sync.rs then
// keeps things in step in the background, and sends patchwork://synced
// whenever a profile changes. Changes that come in are shown, but never
// deployed by themselves. Both list the other computers syncing, which the
// user can name to tell apart; names stay on this computer.

import { invoke, listen, run, state, status } from "./state.js";
import { $, copyText, h } from "./dom.js";
import { closeProfileMenu, menuTarget, openProfile, refresh, renderSidebar } from "./profile.js";
import { openGame } from "./games.js";
import { go } from "./nav.js";
import { offerDaemon, offerDaemonAfterJoining } from "./daemon.js";

listen("patchwork://synced", ({ payload: e }) => {
  for (const warning of e.warnings) status(warning, true);
  if (e.changed) {
    if (e.profile && e.profile === state.profile?.id) {
      status("Updated by another computer; deploy to apply the changes");
    }
    refresh().catch(() => {});
  }
  // someone may have joined while the dialog's open
  if (e.profile && $("#share-dialog").open && sharingStatus && e.profile === sharing?.id) {
    renderPeers("share", sharing.id).catch(() => {});
  }
});

// Other computers, named.

/** A computer's id, short enough to show; it's in full on hover. */
const shortId = (id) => (id.length > 12 ? `${id.slice(0, 12)}…` : id);

/** Lists the computers that can change profile `profile`, or with null,
 * the user's other computers, in the `#<prefix>-peers` block. A name is
 * saved as soon as it's changed. */
async function renderPeers(prefix, profile) {
  const block = $(`#${prefix}-peers`);
  const list = $(`#${prefix}-peer-list`);
  // a list being typed in isn't pulled out from under the user
  if (list.contains(document.activeElement)) return;
  let found;
  try {
    found = await invoke("sync_peers", { id: profile });
  } catch {
    block.hidden = true;
    return;
  }
  $(`#${prefix}-me`).textContent = `This computer is ${shortId(found.me)}`;
  $(`#${prefix}-me`).title = found.me;
  list.replaceChildren(
    ...found.peers.map((peer) =>
      h(
        "li",
        { class: "peer" },
        peerName(peer),
        peerId(peer.id),
        prefix === "share" && sharingStatus?.writable && removeButton(profile, peer),
      ),
    ),
  );
  if (!found.peers.length) {
    list.append(h("li", { class: "muted" }, "No one else yet; they appear here once they've joined."));
  }
  block.hidden = false;
}

function peerId(id) {
  return h("code", { class: "peer-id", title: id }, shortId(id));
}

function peerName(peer) {
  let saved = peer.name ?? "";
  const input = h("input", {
    class: "peer-name",
    type: "text",
    value: saved,
    placeholder: "Give them a name",
    maxlength: 64,
    spellcheck: "false",
    "aria-label": `Name for ${peer.id}`,
  });
  input.addEventListener("keydown", (e) => {
    // Enter would submit the dialog's form
    if (e.key === "Enter") {
      e.preventDefault();
      input.blur();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      input.value = saved;
      input.blur();
    }
  });
  input.addEventListener("change", async () => {
    const name = input.value.trim();
    if (name === saved) return;
    try {
      await invoke("sync_name_peer", { peer: peer.id, name });
    } catch (err) {
      status(String(err), true);
      input.value = saved;
      return;
    }
    saved = name;
    input.value = name;
    status(name ? `Named ${shortId(peer.id)} ${name}` : `Forgot the name of ${shortId(peer.id)}`);
  });
  return input;
}

const REMOVE_TITLE =
  "Stop this computer syncing the profile with anyone. Everyone else moves to a new copy " +
  "of it; whoever follows it needs a new ticket.";

/** A button that removes `peer` from profile `profile`, once clicked twice. */
function removeButton(profile, peer) {
  const who = peer.name || shortId(peer.id);
  let armed = false;
  const attrs = { type: "button", class: "peer-remove", title: REMOVE_TITLE };
  const button = h("button", attrs, "Remove");
  const disarm = () => {
    armed = false;
    button.textContent = "Remove";
    button.classList.remove("danger");
  };
  button.addEventListener("blur", disarm);
  button.addEventListener("click", async () => {
    if (!armed) {
      armed = true;
      button.textContent = `Remove ${who}?`;
      button.classList.add("danger");
      return;
    }
    button.disabled = true;
    status(`Removing ${who}; moving everyone else to a new copy…`);
    let removed;
    try {
      removed = await invoke("sync_remove_peers", { id: profile, peers: [peer.id] });
    } catch (err) {
      $("#share-error").textContent = String(err);
      $("#share-error").hidden = false;
      status(`Couldn't remove ${who}`, true);
      button.disabled = false;
      disarm();
      return;
    }
    const waiting = removed.toTell
      ? `; ${removed.toTell} more are told when they're next online with a computer syncing it`
      : "";
    status(`Removed ${who}; told ${removed.told} of the others${waiting}`);
    renderPeers("share", profile).catch(() => {});
  });
  return button;
}

// Sharing, from the profile's menu in the sidebar.

const shareForm = $("#share-form");
/** The profile being shared, and how it's synced already (or null). */
let sharing = null;
let sharingStatus = null;

$("#share-profile").addEventListener("click", async () => {
  closeProfileMenu();
  sharing = menuTarget();
  sharingStatus = await run("sync_status", { id: sharing.id });
  shareForm.reset();
  $("#share-title").textContent = `Share ${sharing.label}`;
  const following = sharingStatus && !sharingStatus.writable;
  // A profile this computer only follows can only be passed on to follow.
  shareForm.elements.mode.value = following ? "read" : "write";
  shareForm.querySelector("[value=write]").disabled = following;
  const note = $("#share-state");
  note.hidden = !sharingStatus;
  note.textContent = following
    ? "You follow this profile, so you can only pass it on read only. Mods you add stay on this computer."
    : "This profile is already synced; a new ticket lets more people join.";
  $("#share-leave").hidden = !sharingStatus;
  $("#share-peers").hidden = true;
  if (sharingStatus) renderPeers("share", sharing.id).catch(() => {});
  $("#share-offer").replaceChildren();
  showTicket(null);
  $("#share-dialog").showModal();
});

/** Shows the ticket made for the profile, or goes back to choosing how. */
function showTicket(ticket) {
  $("#share-ticket").hidden = !ticket;
  $("#share-copy").hidden = !ticket;
  $("#share-modes").hidden = !!ticket;
  $("#share-submit").hidden = !!ticket;
  $("#share-error").hidden = true;
  if (ticket) {
    shareForm.elements.ticket.value = ticket;
    shareForm.elements.ticket.select();
  }
}

shareForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#share-dialog").close();
});

shareForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const profile = sharing;
  const writable = shareForm.elements.mode.value === "write";
  const submit = $("#share-submit");
  submit.disabled = true;
  status(`Sharing ${profile.label}…`);
  let ticket;
  try {
    ticket = await invoke("sync_share", { id: profile.id, writable });
  } catch (err) {
    $("#share-error").textContent = String(err);
    $("#share-error").hidden = false;
    status(`Couldn't share ${profile.label}`, true);
    return;
  } finally {
    submit.disabled = false;
  }
  $("#share-note").textContent = writable
    ? "Keep Patchwork open here until they've joined. Anyone with the ticket can change this profile for everyone, " +
      "and that can't be taken back, so only give it to people you mean to."
    : "Keep Patchwork open here until they've joined. They get your mods and your changes, and can pass the " +
      "profile on to each other, but only you can change what everyone gets.";
  showTicket(ticket);
  sharingStatus ??= { writable };
  renderPeers("share", profile.id).catch(() => {});
  status(`Sharing ${profile.label}; keep Patchwork open until they've joined`);
  state.overview = await run("overview");
  renderSidebar();
  await offerDaemon($("#share-offer"));
});

$("#share-copy").addEventListener("click", () => copyText(shareForm.elements.ticket));

$("#share-leave").addEventListener("click", async () => {
  const profile = sharing;
  try {
    await invoke("sync_leave", { id: profile.id });
  } catch (err) {
    $("#share-error").textContent = String(err);
    $("#share-error").hidden = false;
    return;
  }
  $("#share-dialog").close();
  status(`Stopped syncing ${profile.label}; it stays here as it is`);
  state.overview = await run("overview");
  renderSidebar();
});

// Joining, from the sidebar. The profile can be for any game, so it's
// opened wherever it belongs.

const joinForm = $("#join-form");

$("#join-profile").addEventListener("click", () => {
  joinForm.reset();
  $("#join-error").hidden = true;
  $("#join-dialog").showModal();
});

joinForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#join-dialog").close();
});

joinForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const ticket = joinForm.elements.ticket.value.trim();
  const error = $("#join-error");
  if (!ticket) {
    error.textContent = "Paste the ticket you were sent.";
    error.hidden = false;
    return;
  }
  const submit = $("#join-submit");
  submit.disabled = true;
  error.hidden = true;
  status("Waiting for the other computer…");
  let joined;
  try {
    joined = await invoke("sync_join", { ticket });
  } catch (err) {
    error.textContent = String(err);
    error.hidden = false;
    status("Couldn't join the profile", true);
    return;
  } finally {
    submit.disabled = false;
  }
  $("#join-dialog").close();
  if (joined.kind === "devices") {
    linked(joined);
    return;
  }
  state.overview = await run("overview");
  const { id, warnings } = joined;
  const profile = state.overview.profiles.find((p) => p.id === id);
  await go(() => (profile.game === state.game ? openProfile(id) : openGame(profile.game, id)));
  renderSidebar();
  if (warnings.length) status(warnings.join("; "), true);
  else status(`Joined ${profile.label}; deploy it to install its mods`);
  await offerDaemonAfterJoining(profile.label);
});

// My computers, in Settings: linking this computer to the user's others, by
// making a ticket here or pasting one from there. Its buttons act at once,
// apart from the dialog's Save.

/** Shows whether this computer's linked, for when Settings opens. */
export async function renderComputers() {
  $("#computers-text").value = "";
  $("#computers-paste").value = "";
  $("#computers-offer").replaceChildren();
  const isLinked = await run("sync_linked");
  showComputers(isLinked, null);
  if (isLinked) await renderPeers("computers", null);
}

/** Shows whether this computer's linked, and the ticket made for linking
 * another, if there is one. */
function showComputers(isLinked, ticket) {
  $("#computers-state").hidden = !isLinked;
  $("#computers-unlink").hidden = !isLinked;
  if (!isLinked) $("#computers-peers").hidden = true;
  $("#computers-ticket").hidden = !ticket;
  $("#computers-copy").hidden = !ticket;
  $("#computers-create").hidden = !!ticket;
  // a computer that's linked already links others by its own ticket
  $("#computers-join").hidden = isLinked || !!ticket;
  $("#computers-submit").hidden = isLinked || !!ticket;
  $("#computers-error").hidden = true;
  if (ticket) {
    $("#computers-text").value = ticket;
    $("#computers-text").select();
  }
}

function computersError(err) {
  $("#computers-error").textContent = String(err);
  $("#computers-error").hidden = false;
}

$("#computers-create").addEventListener("click", async () => {
  const create = $("#computers-create");
  create.disabled = true;
  status("Linking…");
  let ticket;
  try {
    ticket = await invoke("sync_link");
  } catch (err) {
    computersError(err);
    status("Couldn't link this computer", true);
    return;
  } finally {
    create.disabled = false;
  }
  showComputers(true, ticket);
  renderPeers("computers", null).catch(() => {});
  status("Keep Patchwork open until your other computer has linked");
  await refresh().catch(() => {});
  await offerDaemon($("#computers-offer"));
});

$("#computers-copy").addEventListener("click", () => copyText($("#computers-text")));

$("#computers-submit").addEventListener("click", async () => {
  const ticket = $("#computers-paste").value.trim();
  if (!ticket) {
    computersError("Paste the ticket from your other computer, or create one here to paste there.");
    return;
  }
  const submit = $("#computers-submit");
  submit.disabled = true;
  status("Waiting for your other computer…");
  let joined;
  try {
    joined = await invoke("sync_join", { ticket });
  } catch (err) {
    computersError(err);
    status("Couldn't link this computer", true);
    return;
  } finally {
    submit.disabled = false;
  }
  $("#computers-paste").value = "";
  if (joined.kind === "devices") {
    showComputers(true, null);
    renderPeers("computers", null).catch(() => {});
    await linked(joined);
    await offerDaemon($("#computers-offer"));
  } else {
    // a ticket from Share… works here too, for just that profile
    showComputers(false, null);
    status("That was a ticket for one shared profile, which has been joined; it's under its game");
    await refresh().catch(() => {});
  }
});

$("#computers-unlink").addEventListener("click", async () => {
  try {
    await invoke("sync_unlink");
  } catch (err) {
    computersError(err);
    return;
  }
  showComputers(false, null);
  status("Unlinked; profiles synced so far stay synced until you stop each one");
});

/** Says this computer's been linked to the user's others. */
async function linked({ profiles }) {
  const what = profiles === 1 ? "1 profile" : `${profiles} profiles`;
  status(`Linked to your other computers; ${what} will appear as they arrive`);
  await refresh().catch(() => {});
}
