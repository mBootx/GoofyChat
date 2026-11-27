// Message gestures and composer helpers (package C).
// - Long press (450 ms, < 10 px movement) on a touch screen, or right click, on an element
//   with data-msg="<id>": vibrates 12 ms and reports "long:<id>:<x>:<y>".
// - Double tap / double click: "double:<id>".
// - Single tap on a photo of a message (img[data-view]): "tap:<id>", sent once it is clear
//   the tap is not the start of a double tap.
// Other modules are reached through globals only (window.GoofyBridge).

const LONG_MS = 450;
const MOVE_PX = 10;
const DOUBLE_MS = 300;
const android = !!window.GoofyNative;

export function vibrate(ms) {
  try { window.GoofyBridge?.vibrate(ms); } catch {}
}

// Returns a function for offListener (bridge.js) that removes every listener.
export function attachGestures(containerId, callback) {
  let press = null;   // { id, x, y, target, timer, fired }
  let lastTap = null; // { id, x, y, t }
  let single = null;  // pending "tap" timer
  let suppressUntil = 0;
  let longAt = 0;

  const bubbleOf = (e) => {
    const container = document.getElementById(containerId);
    const bubble = e.target?.closest?.("[data-msg]");
    return container && bubble && container.contains(bubble) ? bubble : null;
  };
  const clear = () => {
    if (press) clearTimeout(press.timer);
    press = null;
  };
  const fireLong = (id, x, y) => {
    longAt = Date.now();
    suppressUntil = longAt + 500;
    clearTimeout(single);
    lastTap = null;
    vibrate(12);
    try { window.getSelection()?.removeAllRanges(); } catch {}
    callback(`long:${id}:${Math.round(x)}:${Math.round(y)}`);
  };

  const down = (e) => {
    if (e.pointerType === "mouse" && e.button !== 0) return; // right click: contextmenu
    const bubble = bubbleOf(e);
    if (!bubble || e.target.closest("button, a, input, textarea, select")) return;
    clear();
    press = { id: bubble.dataset.msg, x: e.clientX, y: e.clientY, target: e.target, fired: false };
    // A mouse opens the menu with the right button; holding the left one selects text.
    if (e.pointerType !== "mouse") {
      const current = press;
      current.timer = setTimeout(() => {
        if (press !== current) return;
        current.fired = true;
        fireLong(current.id, current.x, current.y);
      }, LONG_MS);
    }
  };
  const move = (e) => {
    if (press && Math.hypot(e.clientX - press.x, e.clientY - press.y) > MOVE_PX) clear();
  };
  const up = (e) => {
    const p = press;
    clear();
    if (!p || p.fired || Math.hypot(e.clientX - p.x, e.clientY - p.y) > MOVE_PX) return;
    if (p.target.closest("video, audio, .vm, [data-nodouble]")) return;
    const now = Date.now();
    if (lastTap && lastTap.id === p.id && now - lastTap.t < DOUBLE_MS
        && Math.hypot(e.clientX - lastTap.x, e.clientY - lastTap.y) < 30) {
      lastTap = null;
      clearTimeout(single);
      try { window.getSelection()?.removeAllRanges(); } catch {}
      callback(`double:${p.id}`);
      return;
    }
    lastTap = { id: p.id, x: e.clientX, y: e.clientY, t: now };
    if (p.target.closest("img[data-view]")) {
      clearTimeout(single);
      single = setTimeout(() => callback(`tap:${p.id}`), DOUBLE_MS);
    }
  };
  const cancel = () => clear();
  const context = (e) => {
    const bubble = bubbleOf(e);
    if (!bubble) return;
    e.preventDefault();
    // Touch screens also send contextmenu after our own long press.
    if (Date.now() - longAt < 1000) return;
    clear();
    fireLong(bubble.dataset.msg, e.clientX, e.clientY);
  };
  // The click that ends a long press must not reach the page.
  const click = (e) => {
    if (Date.now() < suppressUntil && bubbleOf(e)) {
      e.preventDefault();
      e.stopPropagation();
    }
  };
  const dbl = (e) => { if (bubbleOf(e)) try { window.getSelection()?.removeAllRanges(); } catch {} };

  const listeners = [
    ["pointerdown", down], ["pointermove", move], ["pointerup", up], ["pointercancel", cancel],
    ["contextmenu", context], ["click", click], ["dblclick", dbl],
  ];
  for (const [type, fn] of listeners) document.addEventListener(type, fn, true);
  return () => {
    clear();
    clearTimeout(single);
    for (const [type, fn] of listeners) document.removeEventListener(type, fn, true);
  };
}

// Grows the composer textarea with its content, up to 5 lines.
export function autosize(textareaId) {
  const el = document.getElementById(textareaId);
  if (!el) return;
  const style = getComputedStyle(el);
  const line = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.4 || 22;
  const extra = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)
    + parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth);
  const max = line * 5 + extra;
  el.style.height = "auto";
  const wanted = el.scrollHeight + parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth);
  el.style.height = Math.min(wanted, max) + "px";
  el.style.overflowY = wanted > max ? "auto" : "hidden";
}

// Enter sends on devices with a real keyboard (desktop app, mouse-driven browser); on phones
// Enter makes a new line and the send button sends.
export function enterSends() {
  if (android) return false;
  return !!window.__TAURI_INTERNALS__ || matchMedia("(hover: hover) and (pointer: fine)").matches;
}

// Places the long-press menu next to the press point, inside the window, then shows it.
export function placeMenu(menuId, x, y) {
  requestAnimationFrame(() => {
    const el = document.getElementById(menuId);
    if (!el) return;
    const w = el.offsetWidth, h = el.offsetHeight;
    const vw = window.innerWidth, vh = window.innerHeight;
    const left = Math.max(8, Math.min(x - w / 2, vw - w - 8));
    let top = y + 14;
    if (top + h > vh - 8) top = y - h - 14;
    top = Math.max(8, Math.min(top, vh - h - 8));
    el.style.left = left + "px";
    el.style.top = top + "px";
    el.style.transformOrigin = `${x - left}px ${y - top}px`;
    el.classList.add("placed");
  });
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    let ok = false;
    try { ok = document.execCommand("copy"); } catch {}
    area.remove();
    return ok;
  }
}
