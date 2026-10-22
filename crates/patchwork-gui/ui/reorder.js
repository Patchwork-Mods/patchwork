// Moving the open profile's mods around by hand, by dragging them by their
// handles or pressing the arrow keys on one. The mods around the one being
// moved slide out of its way, and it's saved once it's let go.

import { $, h } from "./dom.js";
import { run, state, status } from "./state.js";
import { openProfile, showProfile } from "./profile.js";

/** The drag going on, if any. */
let drag = null;
/** Whether a new order is being saved, during which nothing else can move. */
let saving = false;

export const dragging = () => drag != null;

/** How close to the top or bottom of the Mods tab dragging starts scrolling it, and how fast. */
const SCROLL_EDGE = 48;
const SCROLL_SPEED = 16;

/** The handle that moves a mod's card or row. */
export function dragHandle(m) {
  return h(
    "button",
    {
      type: "button",
      class: "drag-handle",
      title: "Drag to move in the load order. Mods further down win when two change the same file.",
      "aria-label": `Move ${m.name}`,
      onpointerdown: startDrag,
      onkeydown: moveByKey,
    },
    gripIcon(),
  );
}

function gripIcon() {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  for (const x of [9, 15]) {
    for (const y of [6, 12, 18]) {
      const dot = document.createElementNS(ns, "circle");
      dot.setAttribute("cx", x);
      dot.setAttribute("cy", y);
      dot.setAttribute("r", 1.6);
      svg.append(dot);
    }
  }
  return svg;
}

const names = (list) => [...list.children].map((el) => el.dataset.name);

// The card or row being dragged stays where it is in the page and is only
// drawn wherever the pointer has it, so the gap it leaves shows where it'll
// land. Hovering it over another mod moves it there in the page.

function startDrag(e) {
  if (e.button !== 0 || drag || saving) return;
  e.preventDefault();
  const handle = e.currentTarget;
  const item = handle.closest("li");
  const list = item.parentElement;
  handle.setPointerCapture(e.pointerId);
  const rect = item.getBoundingClientRect();
  drag = {
    item,
    list,
    before: names(list),
    // rows only move up and down; cards follow the pointer anywhere
    vertical: getComputedStyle(list).display !== "grid",
    x: e.clientX,
    y: e.clientY,
    // where on the card or row it was picked up, so that stays under the pointer
    grabX: e.clientX - rect.left,
    grabY: e.clientY - rect.top,
    /** The mod it was last moved past, ignored until the pointer leaves it. */
    passed: null,
    slots: null,
    frame: requestAnimationFrame(frame),
  };
  measure();
  // it may still be gliding in from the last time it was let go
  slides.delete(item);
  item.classList.add("dragging");
  document.body.classList.add("dragging-mod");
  // on the window rather than the handle, so a lost capture can't leave the
  // drag going without anything to end it
  addEventListener("pointermove", onMove);
  addEventListener("pointerup", endDrag);
  addEventListener("pointercancel", endDrag);
}

/**
 * Notes where each mod in the list is, in the list. offsetLeft and offsetTop
 * leave out transforms, so the mods sliding out of the way don't count.
 * They only change when the mods are rearranged, so they're only read then
 * rather than every frame.
 */
function measure() {
  drag.slots = [...drag.list.children].map((el) => ({
    el,
    left: el.offsetLeft,
    top: el.offsetTop,
    right: el.offsetLeft + el.offsetWidth,
    bottom: el.offsetTop + el.offsetHeight,
  }));
  drag.home = drag.slots.find((s) => s.el === drag.item);
}

function onMove(e) {
  drag.x = e.clientX;
  drag.y = e.clientY;
}

/** Follows the pointer each frame, scrolling the tab when it's near an edge. */
function frame() {
  const tab = $("#tab-mods");
  const edges = tab.getBoundingClientRect();
  const speed = (into) => Math.min(1, Math.max(0, 1 - into / SCROLL_EDGE)) * SCROLL_SPEED;
  const scroll = speed(edges.bottom - drag.y) - speed(drag.y - edges.top);
  if (scroll) tab.scrollTop += scroll;

  const { item, list, vertical } = drag;
  // the pointer within the list, where the slots are
  const box = list.getBoundingClientRect();
  const x = drag.x - box.left - list.clientLeft;
  const y = drag.y - box.top - list.clientTop;
  const over = drag.slots.find(
    (s) =>
      s.el !== item &&
      (vertical || (x >= s.left && x < s.right)) &&
      y >= s.top &&
      y < s.bottom,
  )?.el;
  // Moving past a bigger mod can leave the pointer still over it, which
  // would move it straight back, so it has to leave first.
  if (over !== drag.passed) {
    drag.passed = over;
    if (over) {
      moveTo(item, over);
      measure();
    }
  }
  drag.dx = vertical ? 0 : x - drag.grabX - drag.home.left;
  drag.dy = y - drag.grabY - drag.home.top;
  item.style.transform = `translate(${drag.dx}px, ${drag.dy}px)`;
  drag.frame = requestAnimationFrame(frame);
}

/**
 * Moves `item` to where `over` is in the page, and slides the mods between
 * them from where they were to where they are now; the rest stay put.
 *
 * It's those mods that move in the page, not `item`: taking out the one
 * being dragged, even to put it straight back, would end the pointer
 * capture that's dragging it.
 */
function moveTo(item, over) {
  const kids = [...item.parentElement.children];
  const from = kids.indexOf(item);
  const to = kids.indexOf(over);
  const passed = to > from ? kids.slice(from + 1, to + 1) : kids.slice(to, from);
  // where they're drawn, partway through sliding if they already were
  const was = passed.map((el) => el.getBoundingClientRect());
  if (to > from) item.before(...passed);
  else item.after(...passed);
  for (const el of passed) el.style.transform = "";
  passed.forEach((el, i) => {
    const now = el.getBoundingClientRect();
    slide(el, was[i].left - now.left, was[i].top - now.top);
  });
}

// Mods slide by having their transform set every frame here, rather than
// with CSS transitions or el.animate(). WebKitGTK runs those on its
// compositor, which can fall out of step with the page: a mod would finish
// sliding, jump back partway, and slide the rest of the way again.

const SLIDE_MS = 150;
const easeOut = (t) => 1 - (1 - t) ** 3;

/** The mods sliding into place, each from `x` and `y` away, since `start`. */
const slides = new Map();
let ticking = null;

/** Slides `el` from `x` and `y` away to where it is. */
function slide(el, x, y) {
  if (!x && !y) {
    slides.delete(el);
    el.style.transform = "";
    return;
  }
  slides.set(el, { x, y, start: performance.now() });
  el.style.transform = `translate(${x}px, ${y}px)`;
  ticking ??= requestAnimationFrame(tick);
}

function tick(now) {
  ticking = null;
  for (const [el, { x, y, start }] of slides) {
    const t = Math.min(1, Math.max(0, (now - start) / SLIDE_MS));
    const left = 1 - easeOut(t);
    el.style.transform = left ? `translate(${x * left}px, ${y * left}px)` : "";
    if (!left) slides.delete(el);
  }
  if (slides.size) ticking = requestAnimationFrame(tick);
}

function endDrag() {
  const { item, list, before, dx, dy } = drag;
  cancelAnimationFrame(drag.frame);
  drag = null;
  removeEventListener("pointermove", onMove);
  removeEventListener("pointerup", endDrag);
  removeEventListener("pointercancel", endDrag);
  document.body.classList.remove("dragging-mod");
  // glides into the gap it left
  item.classList.remove("dragging");
  slide(item, dx ?? 0, dy ?? 0);
  save(list, before, item.dataset.name).catch(() => {});
}

/** Arrow keys on a handle move its mod one place. */
function moveByKey(e) {
  if (drag || saving || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
  const back = e.key === "ArrowUp" || e.key === "ArrowLeft";
  if (!back && e.key !== "ArrowDown" && e.key !== "ArrowRight") return;
  e.preventDefault();
  const item = e.currentTarget.closest("li");
  const next = back ? item.previousElementSibling : item.nextElementSibling;
  if (!next) return;
  const list = item.parentElement;
  const before = names(list);
  moveTo(item, next);
  item.scrollIntoView({ block: "nearest" });
  save(list, before, item.dataset.name).catch(() => {});
}

/**
 * Saves the order `list` has been rearranged into, which was `before`. A
 * list only shows some of the mods when others are hidden by tag, so the
 * ones it shows are put back in the places they had between the rest.
 */
async function save(list, before, name) {
  const after = names(list);
  if (after.every((n, i) => n === before[i])) return;
  const p = state.profile;
  const { id } = p;
  const shown = new Set(after);
  const old = p.mods.map((m) => m.name);
  let next = 0;
  const order = old.map((n) => (shown.has(n) ? after[next++] : n));
  saving = true;
  let profile;
  try {
    await run("reorder_mods", { id, names: order });
    profile = await run("profile", { id });
  } catch {
    if (state.profile?.id === id) await openProfile(id);
    return;
  } finally {
    saving = false;
  }
  if (state.profile?.id !== id) return;
  // the cards and rows on screen were made from these, so keep them up to date
  const mods = new Map(state.profile.mods.map((m) => [m.key, m]));
  profile.mods = profile.mods.map((n) => Object.assign(mods.get(n.key) ?? n, n));
  showProfile(profile, { keepMods: true });
  status(`Moved ${name}`);
  const undo = $("#undo");
  undo.hidden = false;
  undo.onclick = async () => {
    undo.hidden = true;
    await run("reorder_mods", { id, names: old });
    status(`Put ${name} back`);
    if (state.profile?.id === id) await openProfile(id);
  };
}
