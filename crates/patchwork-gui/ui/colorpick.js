// A color picker: a swatch button that opens the system's picker when it's
// safe to, and otherwise a popover of our own. On Linux the webview's
// <input type="color"> opens GTK's color chooser, which aborts the whole
// app when GTK's settings schema for it isn't installed; the backend's
// `native_color_picker` checks for it.
//
// The button acts like a color input: `value` is `#rrggbb`, `disabled`
// works, and it fires "input" while a color is being picked and "change"
// once one is picked. Our popover has a saturation/brightness square, a hue
// slider and a few preset colors.

import { h } from "./dom.js";
import { invoke } from "./state.js";

/** Whether to use the system's picker; until the backend says, not. */
let native = false;
invoke("native_color_picker").then(
  (ok) => (native = ok),
  () => {},
);

const PRESETS = [
  "#e5484d", "#f76b15", "#ffc53d", "#46a758", "#12a594",
  "#0090ff", "#3e63dd", "#8e4ec6", "#d6409f", "#8b8d98",
];

/** The open picker, if any. */
let open = null;

/** A swatch button that picks a color. `attrs` go on the button. */
export function colorPicker(attrs = {}) {
  // `attrs.class` adds to ours rather than replacing it, which would lose the swatch
  const button = h("button", {
    type: "button",
    ...attrs,
    class: ["color-pick", attrs.class].filter(Boolean).join(" "),
  });
  let value = "#808080";
  Object.defineProperty(button, "value", {
    get: () => value,
    set: (v) => {
      const hex = fullHex(String(v ?? "").toLowerCase());
      if (!hex) return;
      value = hex;
      button.style.setProperty("--pick", hex);
    },
  });
  button.value = value;
  button.addEventListener("click", () => {
    if (native) openNative(button);
    else if (open?.button === button) close();
    else openFor(button);
  });
  return button;
}

/** The hidden color inputs that open the system's picker, by button. */
const nativeInputs = new WeakMap();

function openNative(button) {
  let input = nativeInputs.get(button);
  if (!input) {
    input = h("input", { type: "color", class: "color-pick-native", tabindex: "-1", "aria-hidden": "true" });
    input.addEventListener("input", () => {
      button.value = input.value;
      button.dispatchEvent(new Event("input"));
    });
    input.addEventListener("change", () => {
      button.value = input.value;
      button.dispatchEvent(new Event("change"));
    });
    nativeInputs.set(button, input);
  }
  // next to the button, so the picker opens by it
  if (input.previousSibling !== button) button.after(input);
  input.value = button.value;
  input.click();
}

function close() {
  const was = open;
  if (!was) return;
  open = null;
  was.el.remove();
  was.button.setAttribute("aria-expanded", "false");
  if (was.button.value !== was.start) was.button.dispatchEvent(new Event("change"));
}

function openFor(button) {
  close();
  let [hue, sat, val] = toHsv(button.value);
  const area = h("div", { class: "color-pick-area" });
  const dot = h("div", { class: "color-pick-dot" });
  area.append(dot);
  const hueRange = h("input", {
    type: "range",
    class: "color-pick-hue",
    min: 0,
    max: 359,
    "aria-label": "Hue",
  });
  const presets = h(
    "div",
    { class: "color-pick-presets" },
    ...PRESETS.map((c) =>
      h("button", {
        type: "button",
        class: "color-pick-preset",
        style: `background:${c}`,
        title: c,
        "aria-label": c,
        onclick: () => {
          [hue, sat, val] = toHsv(c);
          paint(true);
        },
      }),
    ),
  );
  const el = h(
    "div",
    { class: "color-pick-popover", role: "dialog", "aria-label": "Pick a color" },
    area,
    hueRange,
    presets,
  );

  // shows hue/sat/val, and with `emit` sets the button to it
  const paint = (emit) => {
    area.style.setProperty("--hue", `hsl(${hue} 100% 50%)`);
    dot.style.left = `${sat * 100}%`;
    dot.style.top = `${(1 - val) * 100}%`;
    hueRange.value = String(Math.round(hue));
    if (emit) {
      button.value = toHex(hue, sat, val);
      button.dispatchEvent(new Event("input"));
    }
  };

  const fromPointer = (e) => {
    const r = area.getBoundingClientRect();
    sat = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    val = 1 - Math.min(1, Math.max(0, (e.clientY - r.top) / r.height));
    paint(true);
  };
  area.addEventListener("pointerdown", (e) => {
    area.setPointerCapture(e.pointerId);
    fromPointer(e);
  });
  area.addEventListener("pointermove", (e) => {
    if (area.hasPointerCapture(e.pointerId)) fromPointer(e);
  });
  hueRange.addEventListener("input", () => {
    hue = Number(hueRange.value);
    paint(true);
  });
  el.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
      button.focus();
    }
  });

  open = { el, button, start: button.value };
  button.setAttribute("aria-expanded", "true");
  // in the button's dialog, if it's in one, so it's above it
  (button.closest("dialog") ?? document.body).append(el);
  paint(false);
  // below the button, or above it if there isn't room, kept on screen
  const at = button.getBoundingClientRect();
  const { offsetWidth: w, offsetHeight: ht } = el;
  const left = Math.max(4, Math.min(at.left, innerWidth - w - 4));
  const top = at.bottom + 4 + ht > innerHeight ? Math.max(4, at.top - 4 - ht) : at.bottom + 4;
  el.style.left = `${left}px`;
  el.style.top = `${top}px`;
  hueRange.focus();
}

document.addEventListener("pointerdown", (e) => {
  if (open && !open.el.contains(e.target) && !open.button.contains(e.target)) close();
});
document.addEventListener("scroll", (e) => {
  if (open && !open.el.contains(e.target)) close();
}, true);
// a dialog closing takes its picker with it
document.addEventListener("close", () => close(), true);

/** `#rgb` or `#rrggbb` as `#rrggbb`, or null if it's neither. */
export function fullHex(color) {
  if (/^#[0-9a-f]{6}$/.test(color ?? "")) return color;
  if (/^#[0-9a-f]{3}$/.test(color ?? "")) return "#" + [...color.slice(1)].map((c) => c + c).join("");
  return null;
}

function toHsv(hex) {
  const n = Number.parseInt(hex.slice(1), 16);
  const r = (n >> 16) / 255;
  const g = ((n >> 8) & 255) / 255;
  const b = (n & 255) / 255;
  const max = Math.max(r, g, b);
  const d = max - Math.min(r, g, b);
  let hue = 0;
  if (d) {
    if (max === r) hue = ((g - b) / d) % 6;
    else if (max === g) hue = (b - r) / d + 2;
    else hue = (r - g) / d + 4;
    hue = (hue * 60 + 360) % 360;
  }
  return [hue, max ? d / max : 0, max];
}

function toHex(hue, sat, val) {
  const f = (k) => {
    const x = (k + hue / 60) % 6;
    return val - val * sat * Math.max(0, Math.min(x, 4 - x, 1));
  };
  return "#" + [f(5), f(3), f(1)].map((c) => Math.round(c * 255).toString(16).padStart(2, "0")).join("");
}
