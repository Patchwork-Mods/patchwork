// Profiles kept in step with other computers, three ways: My computers links
// the user's own, so every profile is on all of them; Share… shares one
// profile with friends, who can all change it, or read only, for others to
// follow. Either makes a ticket to paste on the other computer. sync.rs then
// keeps things in step in the background, and sends patchwork://synced
// whenever a profile changes. Changes that come in are shown, but never
// deployed by themselves.

import { invoke, listen, run, state, status } from "./state.js";
import { $, copyText } from "./dom.js";
import { closeProfileMenu, menuTarget, openProfile, refresh, renderSidebar } from "./profile.js";
import { openGame } from "./games.js";
import { go } from "./nav.js";

listen("patchwork://synced", ({ payload: e }) => {
  for (const warning of e.warnings) status(warning, true);
  if (e.changed) {
    if (e.profile && e.profile === state.profile?.id) {
      status("Updated by another computer; deploy to apply the changes");
    }
    refresh().catch(() => {});
  }
});

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
  status(`Sharing ${profile.label}; keep Patchwork open until they've joined`);
  state.overview = await run("overview");
  renderSidebar();
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
});

// My computers, from the games screen: linking this computer to the user's
// others, by making a ticket here or pasting one from there.

const computersForm = $("#computers-form");

$("#link-computers").addEventListener("click", async () => {
  computersForm.reset();
  showComputers(await run("sync_linked"), null);
  $("#computers-dialog").showModal();
});

/** Shows whether this computer's linked, and the ticket made for linking
 * another, if there is one. */
function showComputers(isLinked, ticket) {
  $("#computers-state").hidden = !isLinked;
  $("#computers-unlink").hidden = !isLinked;
  $("#computers-ticket").hidden = !ticket;
  $("#computers-copy").hidden = !ticket;
  $("#computers-create").hidden = !!ticket;
  // a computer that's linked already links others by its own ticket
  $("#computers-join").hidden = isLinked || !!ticket;
  $("#computers-submit").hidden = isLinked || !!ticket;
  $("#computers-error").hidden = true;
  if (ticket) {
    computersForm.elements.ticket.value = ticket;
    computersForm.elements.ticket.select();
  }
}

function computersError(err) {
  $("#computers-error").textContent = String(err);
  $("#computers-error").hidden = false;
}

computersForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#computers-dialog").close();
});

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
  status("Keep Patchwork open until your other computer has linked");
  await refresh().catch(() => {});
});

$("#computers-copy").addEventListener("click", () => copyText(computersForm.elements.ticket));

computersForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const ticket = computersForm.elements.paste.value.trim();
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
  $("#computers-dialog").close();
  if (joined.kind === "devices") {
    linked(joined);
  } else {
    // a ticket from Share… works here too, for just that profile
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
  $("#computers-dialog").close();
  status("Unlinked; profiles synced so far stay synced until you stop each one");
});

/** Says this computer's been linked to the user's others. */
async function linked({ profiles }) {
  const what = profiles === 1 ? "1 profile" : `${profiles} profiles`;
  status(`Linked to your other computers; ${what} will appear as they arrive`);
  await refresh().catch(() => {});
}
