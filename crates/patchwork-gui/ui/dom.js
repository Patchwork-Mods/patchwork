// Building the page, and small helpers for showing things in it.

import { status } from "./state.js";

export const $ = (sel) => document.querySelector(sel);
export const nextFrame = () => new Promise((r) => requestAnimationFrame(() => r()));
export const ms = (t) => `${Math.round(t)}ms`;

/** Makes an element. `attrs` keys starting with "on" are event listeners.
 * Children that are null, undefined or false are left out, so
 * `cond && h(...)` adds one only if `cond`. */
export function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(attrs)) {
    if (value == null || value === false) continue;
    if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else if (key === "class") el.className = value;
    else el.setAttribute(key, value === true ? "" : value);
  }
  el.append(...children.flat().filter((c) => c != null && c !== false));
  return el;
}

export function compact(n) {
  if (n < 1e3) return String(n);
  if (n < 1e6) return (n / 1e3).toFixed(1) + "k";
  return (n / 1e6).toFixed(1) + "M";
}

/** Copies a text field's contents to the clipboard. */
export async function copyText(field) {
  try {
    await navigator.clipboard.writeText(field.value);
  } catch {
    // Some webviews don't allow the clipboard API; it's selected, at least.
    field.select();
    document.execCommand("copy");
  }
  status("Copied to the clipboard");
}

/** Opens a URL in the browser rather than the app. */
export function openExternal(url) {
  window.__TAURI__.opener.openUrl(url).catch((err) => status(String(err), true));
}

/** A link that opens in the browser rather than the app, or plain text when
 * there's no URL. */
export function link(url, text) {
  if (!url) return text;
  return h(
    "a",
    {
      href: url,
      title: url,
      onclick: (e) => {
        e.preventDefault();
        openExternal(url);
      },
    },
    text,
  );
}

const relativeTime = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });

/** "3 days ago" and the like, for an ISO timestamp. */
export function ago(timestamp) {
  let delta = (Date.parse(timestamp) - Date.now()) / 1000;
  for (const [unit, size] of [
    ["second", 60],
    ["minute", 60],
    ["hour", 24],
    ["day", 30],
    ["month", 12],
    ["year", Infinity],
  ]) {
    if (Math.abs(delta) < size) return relativeTime.format(Math.round(delta), unit);
    delta /= size;
  }
}

/** Up to two letters standing in for a game's picture. */
export function initials(name) {
  const words = name
    .split(/[\s\-_:]+/)
    .filter((w) => w && !/^(a|an|and|of|the)$/i.test(w));
  return words.slice(0, 2).map((w) => w[0].toUpperCase()).join("") || "?";
}

export function chevronIcon() {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(ns, "path");
  path.setAttribute("d", "M6 9l6 6 6-6");
  svg.append(path);
  return svg;
}

export function trashIcon() {
  const ns = "http://www.w3.org/2000/svg";
  const svg = document.createElementNS(ns, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(ns, "path");
  path.setAttribute(
    "d",
    "M4 7h16M10 11v6M14 11v6M5 7l1 12a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2l1-12M9 7V4h6v3",
  );
  svg.append(path);
  return svg;
}

/**
 * Asks for a folder with the system's own picker and puts it in `input`.
 * Leaves `input` alone if the picker is cancelled.
 */
export async function pickFolder(input) {
  try {
    const path = await window.__TAURI__.dialog.open({
      directory: true,
      defaultPath: input.value || undefined,
    });
    if (path) input.value = path;
  } catch (err) {
    status(String(err), true);
  }
}
