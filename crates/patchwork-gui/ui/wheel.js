// Mouse-wheel scrolling, done here instead of by the webview.
//
// WebKitGTK animates each wheel notch from a standstill toward where it should
// end up, and a new notch restarts that animation. Mid-list the end keeps
// running ahead and that goes unnoticed, but near the top or bottom it's held
// at the edge, so each notch only covers a sliver of the little way left and
// the list crawls the last stretch however fast the wheel turns (turning off
// its smooth scrolling setting doesn't reach scrolling areas inside the page).
// So notches move a running target here and each frame closes a fixed share
// of the gap to it. Touchpads send many small deltas instead and are left to
// the webview, which keeps their momentum.

/** A delta at least this big, in pixels, is a wheel notch rather than a touchpad. */
const NOTCH = 50;
/** Pixels a line or a page counts as, for wheels that report in those. */
const LINE = 40;
/** The share of the way to the target covered each frame. */
const EASE = 0.35;

/** The area being scrolled, where it's headed, and its pending frame. */
let glide = null;

function pixels(e) {
  if (e.deltaMode === WheelEvent.DOM_DELTA_LINE) return e.deltaY * LINE;
  if (e.deltaMode === WheelEvent.DOM_DELTA_PAGE) return e.deltaY * window.innerHeight;
  return e.deltaY;
}

/** The nearest area around `el` that can still scroll `dy`'s way. */
function scroller(el, dy) {
  for (; el && el !== document.documentElement; el = el.parentElement) {
    const { overflowY } = getComputedStyle(el);
    if (overflowY !== "auto" && overflowY !== "scroll") continue;
    const max = el.scrollHeight - el.clientHeight;
    if (max < 1) continue;
    if (dy > 0 ? el.scrollTop < max - 0.5 : el.scrollTop > 0.5) return el;
  }
  return null;
}

function frame() {
  const { el, target } = glide;
  const gap = target - el.scrollTop;
  if (Math.abs(gap) < 1) {
    el.scrollTop = target;
    glide = null;
    return;
  }
  // at least a pixel, so rounding can't leave it stuck short of the target
  const step = gap * EASE;
  el.scrollTop += Math.sign(gap) * Math.max(1, Math.abs(step));
  glide.frame = requestAnimationFrame(frame);
}

document.addEventListener(
  "wheel",
  (e) => {
    // zooming, sideways scrolling, and anything else that's already handled it
    if (e.defaultPrevented || e.ctrlKey || e.shiftKey) return;
    if (Math.abs(e.deltaX) > Math.abs(e.deltaY)) return;
    const dy = pixels(e);
    if (Math.abs(dy) < NOTCH) return;
    const el = scroller(e.target, dy);
    if (!el) return;
    e.preventDefault();
    if (glide?.el !== el) {
      if (glide) cancelAnimationFrame(glide.frame);
      glide = { el, target: el.scrollTop, frame: null };
    }
    const max = el.scrollHeight - el.clientHeight;
    glide.target = Math.min(max, Math.max(0, glide.target + dy));
    glide.frame ??= requestAnimationFrame(frame);
  },
  { passive: false },
);

// dragging the scroll bar, keys and the like take over from a glide
for (const type of ["mousedown", "keydown"]) {
  document.addEventListener(type, () => {
    if (!glide) return;
    cancelAnimationFrame(glide.frame);
    glide = null;
  });
}
