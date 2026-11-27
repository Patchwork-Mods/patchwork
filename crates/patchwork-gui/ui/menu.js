// Menus put on the page itself, so a card's edges can't clip them: popped
// up under a button (popupMenu), or where something was right-clicked
// (onContext). Both take the same items:
//
//   { label, run, danger, disabled, title, checked, items }
//
// where `items` makes it a submenu, `checked` (true or false) marks it as
// one of a set to pick from, "-" is a line between, and false or null
// leaves it out, so `cond && { ... }` adds one only if `cond`.
//
// Right-clicking anywhere shows our menu or none, never the webview's own
// (Reload, Inspect and the like), except in text fields, which keep theirs
// for cut and paste. Shift+right-click still gets the webview's, for
// inspecting the page.

import { copyString, h, openExternal } from "./dom.js";

/** The open menus, the outermost first: a submenu goes after its parent. */
let open = [];
/** What had the focus before the outermost menu opened, to go back to. */
let returnFocus = null;

export function closeMenus({ restore = false } = {}) {
  for (const menu of open) menu.remove();
  open = [];
  if (restore) returnFocus?.focus?.({ preventScroll: true });
  returnFocus = null;
}

/** Closes the menus after `level` (0 is the outermost). */
function closeAfter(level) {
  for (const menu of open.splice(level + 1)) menu.remove();
}

/** Leaves out what's false, and lines at either end or next to another. */
function tidy(items) {
  const kept = [];
  for (const item of items) {
    if (!item) continue;
    if (item === "-" && (!kept.length || kept.at(-1) === "-")) continue;
    kept.push(item);
  }
  if (kept.at(-1) === "-") kept.pop();
  return kept;
}

const buttonsOf = (menu) => [...menu.querySelectorAll(":scope > button:not(:disabled)")];

function buildMenu(items, level) {
  const menu = h("div", { class: "menu-items context-menu", role: "menu" });
  for (const item of tidy(items)) {
    if (item === "-") {
      menu.append(h("hr"));
      continue;
    }
    const sub = item.items && tidy(item.items);
    const button = h(
      "button",
      {
        type: "button",
        role: item.checked != null ? "menuitemradio" : "menuitem",
        "aria-checked": item.checked != null ? String(item.checked) : null,
        "aria-haspopup": sub ? "menu" : null,
        "aria-expanded": sub ? "false" : null,
        class: item.danger ? "danger-text" : null,
        title: item.title ?? null,
        disabled: item.disabled || (sub && !sub.length),
      },
      h("span", {}, item.label),
      sub && h("span", { class: "submenu-arrow", "aria-hidden": "true" }, "›"),
    );
    if (sub) {
      const show = (focus) => openSubmenu(button, sub, level, focus);
      button.addEventListener("click", () => show(true));
      button.addEventListener("pointerenter", () => show(false));
      button.addEventListener("keydown", (e) => {
        if (e.key === "ArrowRight") {
          e.preventDefault();
          show(true);
        }
      });
    } else {
      button.addEventListener("pointerenter", () => closeAfter(level));
      button.addEventListener("click", () => {
        closeMenus({ restore: true });
        Promise.resolve(item.run()).catch(() => {});
      });
    }
    menu.append(button);
  }
  menu.addEventListener("keydown", (e) => keys(e, menu, level));
  return menu;
}

function keys(e, menu, level) {
  const buttons = buttonsOf(menu);
  const at = buttons.indexOf(document.activeElement);
  const move = { ArrowDown: at + 1, ArrowUp: at - 1, Home: 0, End: buttons.length - 1 }[e.key];
  if (move != null) {
    e.preventDefault();
    buttons[(move + buttons.length) % buttons.length]?.focus();
  } else if ((e.key === "ArrowLeft" || e.key === "Escape") && level > 0) {
    e.preventDefault();
    e.stopPropagation();
    closeAfter(level - 1);
    open[level - 1].opener?.focus();
  } else if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    closeMenus({ restore: true });
  } else if (e.key === "Tab") {
    closeMenus({ restore: true });
  }
}

/** Shows `menu` with its corner at `x`, `y`, moved to stay inside the window. */
function place(menu, x, y, { flipX, flipY } = {}) {
  document.body.append(menu);
  const { offsetWidth: w, offsetHeight: ht } = menu;
  if (x + w > innerWidth - 4) x = flipX ?? innerWidth - w - 4;
  if (y + ht > innerHeight - 4) y = flipY ?? innerHeight - ht - 4;
  menu.style.left = `${Math.max(4, x)}px`;
  menu.style.top = `${Math.max(4, Math.min(y, innerHeight - ht - 4))}px`;
}

function openSubmenu(button, items, level, focus) {
  if (open[level + 1]?.opener === button) {
    if (focus) buttonsOf(open[level + 1])[0]?.focus();
    return;
  }
  closeAfter(level);
  for (const b of open[level].querySelectorAll("[aria-expanded]")) b.setAttribute("aria-expanded", "false");
  const menu = buildMenu(items, level + 1);
  menu.opener = button;
  button.setAttribute("aria-expanded", "true");
  const box = button.getBoundingClientRect();
  open.push(menu);
  // to the right of its item, or the left if there isn't room
  place(menu, box.right - 2, box.top - 5, { flipX: box.left - menu.offsetWidth + 2 });
  if (focus) buttonsOf(menu)[0]?.focus();
}

function openMenu(items, at, anchor = null) {
  const focused = document.activeElement;
  closeMenus();
  const menu = buildMenu(items, 0);
  if (!menu.children.length) return;
  returnFocus = anchor ?? focused;
  menu.anchor = anchor;
  open = [menu];
  place(menu, at.x, at.y, at);
  buttonsOf(menu)[0]?.focus({ preventScroll: true });
}

/**
 * Pops up a menu of `items` under `anchor`, a button, or above it if there
 * isn't room. A second click on the button closes it.
 */
export function popupMenu(anchor, items) {
  if (open[0]?.anchor === anchor) {
    closeMenus({ restore: true });
    return;
  }
  const box = anchor.getBoundingClientRect();
  // so its height can be known for flipping it above
  const probe = buildMenu(items, 0);
  document.body.append(probe);
  const height = probe.offsetHeight;
  probe.remove();
  openMenu(items, { x: box.left, y: box.bottom + 4, flipY: box.top - height - 4 }, anchor);
}

/** What each element with a menu of its own shows; see onContext(). */
const contexts = new WeakMap();

/**
 * Gives `el` a right-click menu: `items()` makes its items when it's
 * opened. Right-clicking something inside it that has a menu of its own
 * shows that one's items first, then a line, then `el`'s. Selected text
 * adds Copy at the top, and a link outside anything with a menu gets one
 * to open or copy it.
 */
export function onContext(el, items) {
  contexts.set(el, items);
}

document.addEventListener("contextmenu", (e) => {
  // something showed a menu of its own, like the profiles in the sidebar
  if (e.defaultPrevented || e.shiftKey) return;
  const field = e.target.closest?.("textarea, [contenteditable], input");
  if (field && !["checkbox", "radio", "button", "submit", "range", "color"].includes(field.type)) return;
  e.preventDefault();
  if (open.some((m) => m.contains(e.target))) return;
  const groups = [];
  for (let el = e.target; el; el = el.parentElement) {
    const items = contexts.get(el);
    if (items) groups.push(items());
  }
  // a mod's links are to its page and its author's, which its menu opens
  const a = !groups.length && e.target.closest?.("a[href]");
  if (a && /^https?:/.test(a.href)) {
    groups.unshift([
      { label: "Open link in your browser", run: () => openExternal(a.href) },
      { label: "Copy link", run: () => copyString(a.href, "Copied the link") },
    ]);
  }
  const selected = getSelection()?.toString().trim();
  if (selected) groups.unshift([{ label: "Copy", run: () => copyString(selected, "Copied") }]);
  if (!groups.length) {
    closeMenus();
    return;
  }
  const items = groups.flatMap((g, i) => (i ? ["-", ...g] : g));
  // from the keyboard (the Menu key) there's no pointer to open it at
  const keyboard = e.clientX === 0 && e.clientY === 0;
  const box = keyboard && e.target.getBoundingClientRect?.();
  openMenu(items, box ? { x: box.left + 8, y: box.top + Math.min(box.height, 24) } : { x: e.clientX, y: e.clientY });
});

document.addEventListener("pointerdown", (e) => {
  if (!open.length) return;
  if (open.some((m) => m.contains(e.target)) || open[0].anchor?.contains(e.target)) return;
  closeMenus();
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && open.length) closeMenus({ restore: true });
});
// it'd be left behind, except when it's a long one being scrolled itself
document.addEventListener(
  "scroll",
  (e) => {
    if (!open.some((m) => m === e.target)) closeMenus();
  },
  true,
);
addEventListener("blur", () => closeMenus());
addEventListener("resize", () => closeMenus());
