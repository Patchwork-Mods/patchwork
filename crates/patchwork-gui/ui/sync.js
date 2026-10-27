// Profiles kept in step with other computers. Sharing one makes a ticket to
// paste into Join… on the other computer; sync.rs then keeps it in step in
// the background, and sends patchwork://synced whenever a profile changes.
// Changes that come in are shown, but never deployed by themselves.

import { invoke, listen, run, state, status } from "./state.js";
import { $, copyText } from "./dom.js";
import { closeProfileMenu, menuTarget, openProfile, refresh, renderSidebar } from "./profile.js";
import { openGame } from "./games.js";
import { go } from "./nav.js";

listen("patchwork://synced", ({ payload: e }) => {
  for (const warning of e.warnings) status(warning, true);
  if (e.changed) {
    if (e.profile && e.profile === state.profile?.id) {
      status("Updated from another computer; deploy to apply the changes");
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
    ? "This computer follows this profile, so others can only follow it from here too."
    : "This profile is already kept in step with other computers.";
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
  showTicket(ticket);
  status(`Sharing ${profile.label}; keep Patchwork open until the other computer joins`);
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
    error.textContent = "Paste the ticket from the other computer.";
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
  state.overview = await run("overview");
  const { id, warnings } = joined;
  const profile = state.overview.profiles.find((p) => p.id === id);
  await go(() => (profile.game === state.game ? openProfile(id) : openGame(profile.game, id)));
  renderSidebar();
  if (warnings.length) status(warnings.join("; "), true);
  else status(`Joined ${profile.label}; deploy it to install its mods`);
});
