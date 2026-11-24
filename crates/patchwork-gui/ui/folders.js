// Folders: groups of the open profile's mods (see `folders` in profile.rs).
// In the Mods tab's own order a folder is a section among the mods, dragged
// as one, with its mods inside it; the load order and search results stay
// flat. A folder's mods are always next to each other in the profile, so it
// has one place in the load order.

import { $, chevronIcon, folderIcon, h } from "./dom.js";
import { errorText, invoke, run, state, status } from "./state.js";
import { showProfile } from "./profile.js";
import { dragHandle, saveOrder } from "./reorder.js";

/** The folders shown closed, by id; the viewer's own, kept between runs. */
const collapsed = new Set();
try {
  for (const id of JSON.parse(localStorage.getItem("collapsedFolders") ?? "[]")) collapsed.add(id);
} catch {}

function setCollapsed(id, closed) {
  if (closed) collapsed.add(id);
  else collapsed.delete(id);
  try {
    localStorage.setItem("collapsedFolders", JSON.stringify([...collapsed]));
  } catch {}
}

/** What a folder's section is called in the list's `data-name`s. */
const TOKEN = "folder:";

/** The folder `m` is shown in, if it's in one the profile has. */
export const folderOf = (m) => state.profile.folders.find((f) => f.id === m.folder);

/**
 * `mods` (in the profile's order) as the Mods tab shows them: each mod not
 * in a folder on its own, and each folder with its mods, where its first
 * one is. Empty folders go last. Mods `isHidden` leaves out are only
 * counted.
 */
export function arrange(mods, isHidden) {
  const folders = new Map(state.profile.folders.map((f) => [f.id, f]));
  const blocks = [];
  const byId = new Map();
  for (const m of mods) {
    const folder = folders.get(m.folder);
    if (!folder) {
      if (!isHidden(m)) blocks.push({ mod: m });
      continue;
    }
    let block = byId.get(folder.id);
    if (!block) {
      block = { folder, mods: [], all: [] };
      byId.set(folder.id, block);
      blocks.push(block);
    }
    block.all.push(m);
    if (!isHidden(m)) block.mods.push(m);
  }
  for (const folder of folders.values()) {
    if (!byId.has(folder.id)) blocks.push({ folder, mods: [], all: [] });
  }
  return blocks;
}

/**
 * Saves the Mods tab's top level rearranged: mods and whole folders. Like
 * saving a list of mods, the ones it doesn't show (hidden by tag) keep
 * their places between the rest.
 */
export async function saveTop(after, _before, moved) {
  const p = state.profile;
  const known = new Set(p.folders.map((f) => f.id));
  const token = (m) => (known.has(m.folder) ? TOKEN + m.folder : m.name);
  // a folder's mods are next to each other, so each block is once, in order
  const blocks = [...new Set(p.mods.map(token))];
  const inBlocks = new Set(blocks);
  // empty folders are shown, but have no place to keep
  const shown = after.filter((t) => inBlocks.has(t));
  const isShown = new Set(shown);
  let next = 0;
  const order = blocks
    .map((t) => (isShown.has(t) ? shown[next++] : t))
    .flatMap((t) => (t.startsWith(TOKEN) ? p.mods.filter((m) => token(m) === t).map((m) => m.name) : [t]));
  const name = moved.startsWith(TOKEN) ? p.folders.find((f) => TOKEN + f.id === moved)?.name : moved;
  await saveOrder(p.id, p.mods.map((m) => m.name), order, name);
}

/**
 * A folder's section: a header with a handle to drag it by, an arrow to
 * open or close it, its name, how many mods it has, a switch for all of
 * them and a menu; then its mods, made by `item`, in a list laid out like
 * the rest (`layout`).
 */
export function folderSection({ folder, mods, all }, { item, layout, movable }) {
  const open = !collapsed.has(folder.id);
  const list = h("ul", { class: `${layout} folder-mods` }, ...mods.map(item));
  const empty = h(
    "p",
    { class: "folder-empty muted" },
    "Empty. Put mods in it from the folder button on each mod.",
  );
  const toggle = h("input", {
    type: "checkbox",
    role: "switch",
    "aria-label": `Enable the mods in ${folder.name}`,
    onchange: (e) => setEnabled(folder, e.target),
  });
  const arrow = h(
    "button",
    { type: "button", class: "expand", "aria-label": `Show the mods in ${folder.name}` },
    chevronIcon(),
  );
  const section = h("li", { class: "mod-folder", "data-name": TOKEN + folder.id });
  const setOpen = (open) => {
    section.classList.toggle("collapsed", !open);
    arrow.setAttribute("aria-expanded", String(open));
    list.hidden = !open || !mods.length;
    empty.hidden = !open || all.length > 0;
  };
  const flip = () => {
    const opening = section.classList.contains("collapsed");
    setCollapsed(folder.id, !opening);
    setOpen(opening);
  };
  arrow.addEventListener("click", flip);
  const menu = h(
    "button",
    {
      type: "button",
      class: "folder-menu",
      title: "Rename or remove this folder",
      "aria-label": `${folder.name} options`,
      "aria-haspopup": "menu",
      onclick: (e) =>
        popupMenu(e.currentTarget, [
          { label: "Rename…", run: () => renameFolder(folder) },
          { label: "Remove folder", danger: true, run: () => removeFolder(folder) },
        ]),
    },
    "⋯",
  );
  section.append(
    h(
      "div",
      { class: "folder-head" },
      // an empty folder has no place in the order to move
      movable && all.length > 0 && dragHandle(
        { name: folder.name },
        { save: saveTop, title: "Drag to move the whole folder in the load order." },
      ),
      arrow,
      h(
        "span",
        { class: "folder-title", onclick: flip },
        folderIcon(),
        h("span", { class: "folder-name" }, folder.name),
        h("span", { class: "folder-count muted" }),
      ),
      h(
        "label",
        { class: "switch", title: "Turn all of this folder's mods on or off" },
        toggle,
      ),
      menu,
    ),
    list,
    empty,
  );
  setOpen(open);
  refreshFolder(section);
  return section;
}

/**
 * Brings a folder's section's switch and counts up to date with its mods,
 * as one of them is turned on or off.
 */
export function refreshFolder(section) {
  const id = section.dataset.name.slice(TOKEN.length);
  const all = state.profile.mods.filter((m) => m.folder === id);
  const on = all.filter((m) => m.enabled).length;
  const off = all.length - on;
  const hidden = all.length - section.querySelector(".folder-mods").childElementCount;
  const toggle = section.querySelector(".folder-head input");
  toggle.checked = all.length > 0 && off === 0;
  toggle.disabled = !all.length;
  // some on and some off
  toggle.indeterminate = on > 0 && off > 0;
  if (toggle.indeterminate) toggle.setAttribute("aria-checked", "mixed");
  else toggle.removeAttribute("aria-checked");
  section.classList.toggle("disabled", all.length > 0 && on === 0);
  section.querySelector(".folder-count").textContent = [
    `${all.length} ${all.length === 1 ? "mod" : "mods"}`,
    on > 0 && off > 0 && `${off} off`,
    hidden > 0 && `${hidden} hidden`,
  ]
    .filter(Boolean)
    .join(" · ");
}

async function setEnabled(folder, input) {
  const enabled = input.checked;
  try {
    const profile = await run("set_folder_enabled", {
      id: state.profile.id,
      folder: folder.id,
      enabled,
    });
    showProfile(profile);
    status(`Turned ${enabled ? "on" : "off"} the mods in ${folder.name}`);
  } catch {
    input.checked = !enabled;
  }
}

/**
 * A button that pops up where mod `m` can go: the profile's other folders,
 * out of its own, or a new one.
 */
export function folderButton(m, iconAction) {
  return iconAction(folderIcon(), "Folder", {
    class: "mod-folder-button",
    title: "Folder: put it in a folder, or take it out of one",
    "aria-haspopup": "menu",
    onclick: (e) => {
      const own = folderOf(m);
      const others = state.profile.folders.filter((f) => f !== own);
      popupMenu(e.currentTarget, [
        ...others.map((f) => ({ label: `Move to ${f.name}`, run: () => moveTo(m, f) })),
        own && { label: `Take out of ${own.name}`, run: () => moveTo(m, null) },
        (others.length > 0 || own) && "-",
        { label: "New folder…", run: () => newFolder([m.name]) },
      ]);
    },
  });
}

async function moveTo(m, folder) {
  const profile = await run("set_mod_folder", {
    id: state.profile.id,
    name: m.name,
    folder: folder?.id ?? null,
  });
  showProfile(profile);
  status(folder ? `Put ${m.name} in ${folder.name}` : `Took ${m.name} out of its folder`);
}

/** Asks for a name and makes a folder with `mods` in it. */
export function newFolder(mods = []) {
  askFolderName("New folder", "Create", "", async (name) => {
    const id = state.profile.id;
    const profile = await invoke("add_folder", { id, name, mods });
    // it's shown open, even if one of that name was closed before
    const made = profile.folders.find((f) => f.name.toLowerCase() === name.toLowerCase());
    if (made) setCollapsed(made.id, false);
    showProfile(profile);
    status(`Made folder ${name}`);
  });
}

function renameFolder(folder) {
  askFolderName(`Rename ${folder.name}`, "Rename", folder.name, async (name) => {
    const profile = await invoke("rename_folder", { id: state.profile.id, folder: folder.id, name });
    showProfile(profile);
    status(`Renamed ${folder.name} to ${name}`);
  });
}

async function removeFolder(folder) {
  const profile = await run("remove_folder", { id: state.profile.id, folder: folder.id });
  setCollapsed(folder.id, false);
  showProfile(profile);
  status(`Removed folder ${folder.name}; its mods are still in the profile`);
}

// The name dialog. Errors (a name that's taken, say) show in it, so it
// stays open to fix them.

const nameForm = $("#folder-name-form");
let nameAction = null;

function askFolderName(title, submit, value, action) {
  nameForm.reset();
  nameAction = action;
  $("#folder-name-title").textContent = title;
  $("#folder-name-submit").textContent = submit;
  $("#folder-name-error").hidden = true;
  nameForm.elements.name.value = value;
  $("#folder-name-dialog").showModal();
  nameForm.elements.name.select();
}

nameForm.querySelector("[value=cancel]").addEventListener("click", () => {
  $("#folder-name-dialog").close();
});

nameForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const submit = $("#folder-name-submit");
  submit.disabled = true;
  try {
    await nameAction(nameForm.elements.name.value.trim());
  } catch (err) {
    $("#folder-name-error").textContent = errorText(err);
    $("#folder-name-error").hidden = false;
    return;
  } finally {
    submit.disabled = false;
  }
  $("#folder-name-dialog").close();
});

$("#new-folder").addEventListener("click", () => newFolder());

// A menu popped up under a button, on the page itself so a card can't clip
// it. `items` are `{ label, run, danger }`, "-" for a line between, or
// false to leave out.

let popup = null;

function closePopup() {
  popup?.remove();
  popup = null;
}

function popupMenu(anchor, items) {
  const wasOpen = popup?.anchor === anchor;
  closePopup();
  // a second click on its button closes it
  if (wasOpen) return;
  const menu = h(
    "div",
    { class: "menu-items context-menu", role: "menu" },
    ...items.filter(Boolean).map((item) =>
      item === "-"
        ? h("hr")
        : h(
            "button",
            {
              type: "button",
              role: "menuitem",
              class: item.danger ? "danger-text" : "",
              onclick: () => {
                closePopup();
                Promise.resolve(item.run()).catch(() => {});
              },
            },
            item.label,
          ),
    ),
  );
  document.body.append(menu);
  popup = menu;
  popup.anchor = anchor;
  // below the button, kept inside the window
  const box = anchor.getBoundingClientRect();
  const x = Math.min(box.left, innerWidth - menu.offsetWidth - 4);
  const below = box.bottom + 4;
  const y = below + menu.offsetHeight > innerHeight ? box.top - menu.offsetHeight - 4 : below;
  menu.style.left = `${Math.max(x, 0)}px`;
  menu.style.top = `${Math.max(y, 0)}px`;
  menu.querySelector("button")?.focus();
}

document.addEventListener("pointerdown", (e) => {
  if (popup && !popup.contains(e.target) && !popup.anchor.contains(e.target)) closePopup();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closePopup();
});
document.addEventListener("scroll", closePopup, true);
addEventListener("blur", closePopup);
addEventListener("resize", closePopup);
