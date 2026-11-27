// Appearance and sounds: theme presets, bubble shape and colours, message font and
// reduced motion, applied as CSS variables and data-* attributes on <html> (see look.css),
// synthesized ring and message tones, and the goofysound ringtone.
// Everything is per device, in localStorage. The theme is applied as soon as
// this module loads, before the interface is first drawn.

const THEME_KEY = "goofychat.theme";
const CACHE_KEY = "goofychat.theme.cache"; // read by the inline script of index.html
const SOUND_KEY = "goofychat.sounds";

function read(key) {
  try { return JSON.parse(localStorage.getItem(key) || "null") || {}; } catch { return {}; }
}
function write(key, value) {
  try { localStorage.setItem(key, typeof value === "string" ? value : JSON.stringify(value)); } catch {}
}

// ---- presets ----
// l / d: light and dark palettes [bg, surface, text, muted, line, incoming bubble].
const PRESETS = [
  { id: "violet", name: "Violet", a: "#6c4cf1", b: "#a24cf1",
    l: ["#f5f5fa", "#ffffff", "#17171f", "#6f7080", "#e6e6ef", "#ffffff"],
    d: ["#0f0f16", "#191923", "#f0f0f7", "#9a9bad", "#272734", "#22222f"] },
  { id: "ocean", name: "Océan", a: "#0b84d8", b: "#13b7c4",
    l: ["#f0f6fb", "#ffffff", "#0f1c26", "#5f7482", "#dbe8f1", "#ffffff"],
    d: ["#07131c", "#0e1e2a", "#e8f3fa", "#8aa3b4", "#1b3040", "#142838"] },
  { id: "forest", name: "Forêt", a: "#2f9e5b", b: "#7fb83a",
    l: ["#f2f7f1", "#ffffff", "#142017", "#627265", "#dfe9de", "#ffffff"],
    d: ["#0a120c", "#121e16", "#e7f2e9", "#8fa794", "#1f3324", "#1a2b1f"] },
  { id: "sunset", name: "Coucher de soleil", a: "#ef5a3c", b: "#f5a524",
    l: ["#fdf4ef", "#ffffff", "#24140e", "#806a60", "#f1dfd5", "#ffffff"],
    d: ["#160c09", "#22140f", "#fbeee8", "#b39a8e", "#3a251c", "#2e1c15"] },
  { id: "candy", name: "Rose bonbon", a: "#ec4899", b: "#b05cf6",
    l: ["#fdf2f8", "#ffffff", "#2a1020", "#8a6178", "#f6dbe9", "#ffffff"],
    d: ["#16090f", "#22101b", "#fbeaf3", "#b8909f", "#3a1c2e", "#2e1626"] },
  { id: "neon", name: "Néon", a: "#19e3ff", b: "#c03cff", on: "#06060c",
    l: ["#eef4fb", "#ffffff", "#0b0b16", "#5b6283", "#dae2f0", "#ffffff"],
    d: ["#06060c", "#0e0e1a", "#e9f7ff", "#8c93b8", "#1f2244", "#151931"] },
  { id: "midnight", name: "Minuit", a: "#7c6cff", b: "#3f8cff", dark: true,
    l: ["#000000", "#0a0a0a", "#f2f2f2", "#8a8a8a", "#1c1c1c", "#161616"],
    d: ["#000000", "#0a0a0a", "#f2f2f2", "#8a8a8a", "#1c1c1c", "#161616"] },
  { id: "paper", name: "Papier", a: "#b0601c", b: "#d4903f",
    l: ["#f3eee4", "#fbf8f1", "#2b2620", "#7d7264", "#e4dbcb", "#fffdf8"],
    d: ["#1a1714", "#23201b", "#efe7da", "#a89a87", "#352f27", "#2c2822"] },
  { id: "lava", name: "Lave", a: "#e11d48", b: "#f97316",
    l: ["#fbf3f3", "#ffffff", "#220c0f", "#7d5f63", "#f0dcdd", "#ffffff"],
    d: ["#120708", "#1d0d10", "#fbe9eb", "#b28d92", "#371a1f", "#2a1418"] },
  { id: "mint", name: "Menthe", a: "#0fb39a", b: "#4ade80", on: "#04211b",
    l: ["#effaf7", "#ffffff", "#0c201c", "#5c7771", "#d8eee8", "#ffffff"],
    d: ["#061411", "#0c1f1b", "#e5f7f2", "#88aaa2", "#183531", "#132b27"] },
];

const DEFAULT_LOOK = {
  preset: "violet", mode: "auto", accent: "", bubble: "round", size: "m", wall: "none", compact: false,
  mineColor: "", inColor: "", font: "", motion: "auto",
};
const SIZES = { s: "14px", m: "16px", l: "18px", xl: "20px" };
const BUBBLES = ["round", "square", "flat", "pill", "tail"];
// Message fonts (the `body: true` entries of proto::FONTS) and their CSS families (fonts.css).
const FONTS = {
  inter: "GC Inter", nunito: "GC Nunito", atkinson: "GC Atkinson Hyperlegible", lora: "GC Lora",
  "space-mono": "GC Space Mono", comfortaa: "GC Comfortaa", caveat: "GC Caveat",
};
const HEX = /^#[0-9a-f]{6}$/i;

// ---- colour helpers ----
function hexRgb(hex) {
  const h = hex.replace("#", "");
  const full = h.length === 3 ? h.split("").map((c) => c + c).join("") : h;
  const n = parseInt(full, 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
function rgbHex(r, g, b) {
  return "#" + [r, g, b].map((v) => Math.round(Math.max(0, Math.min(255, v))).toString(16).padStart(2, "0")).join("");
}
function luminance(hex) {
  const [r, g, b] = hexRgb(hex).map((v) => {
    v /= 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
// Readable text colour on a solid background.
function textOn(hex) { return luminance(hex) > 0.5 ? "#0b0b12" : "#ffffff"; }
function contrast(a, b) {
  const la = luminance(a), lb = luminance(b);
  return (Math.max(la, lb) + 0.05) / (Math.min(la, lb) + 0.05);
}
function toHsl(hex) {
  let [r, g, b] = hexRgb(hex).map((v) => v / 255);
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  let h = 0, s = 0;
  const l = (max + min) / 2;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    h = max === r ? (g - b) / d + (g < b ? 6 : 0) : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
    h *= 60;
  }
  return [h, s, l];
}
function fromHsl(h, s, l) {
  h = ((h % 360) + 360) % 360;
  const c = (1 - Math.abs(2 * l - 1)) * s;
  const x = c * (1 - Math.abs(((h / 60) % 2) - 1));
  const m = l - c / 2;
  const [r, g, b] = h < 60 ? [c, x, 0] : h < 120 ? [x, c, 0] : h < 180 ? [0, c, x] : h < 240 ? [0, x, c] : h < 300 ? [x, 0, c] : [c, 0, x];
  return rgbHex((r + m) * 255, (g + m) * 255, (b + m) * 255);
}

// ---- theme ----
const darkQuery = matchMedia("(prefers-color-scheme: dark)");

export function lookGet() { return JSON.stringify({ ...DEFAULT_LOOK, ...read(THEME_KEY) }); }

function isDark(look) {
  const preset = PRESETS.find((p) => p.id === look.preset) || PRESETS[0];
  if (preset.dark) return true;
  return look.mode === "dark" || (look.mode !== "light" && darkQuery.matches);
}
export function lookDark() { return isDark({ ...DEFAULT_LOOK, ...read(THEME_KEY) }); }

function variables(look) {
  const preset = PRESETS.find((p) => p.id === look.preset) || PRESETS[0];
  const dark = isDark(look);
  const [bg, surface, text, muted, line, incoming] = dark ? preset.d : preset.l;
  let a = preset.a, b = preset.b, on = preset.on || "#ffffff";
  if (HEX.test(look.accent || "")) {
    a = look.accent;
    const [h, s, l] = toHsl(a);
    b = fromHsl(h + 38, Math.min(1, s * 1.05 + 0.05), Math.min(0.72, l + 0.06));
    on = textOn(a);
  }
  // Bubble colours: a custom colour is solid; the theme keeps the gradient.
  const mineSolid = HEX.test(look.mineColor || "") ? look.mineColor : null;
  const inSolid = HEX.test(look.inColor || "") ? look.inColor : null;
  const font = Object.prototype.hasOwnProperty.call(FONTS, look.font) ? look.font : "";
  return {
    vars: {
      "--bg": bg, "--surface": surface, "--text": text, "--muted": muted, "--line": line, "--in": inSolid || incoming,
      "--accent": a, "--accent-2": b, "--on-accent": on, "--mine-text": mineSolid ? textOn(mineSolid) : on,
      "--bubble-mine": mineSolid || `linear-gradient(135deg, ${a}, ${b})`,
      // Flat and tail shapes: a solid colour (the tail takes the end colour of the gradient).
      "--mine-solid": mineSolid || a, "--mine-tail": mineSolid || b,
      "--in-text": inSolid ? textOn(inSolid) : text,
      "--in-flat": inSolid || "color-mix(in srgb, var(--line) 70%, var(--in))",
      "--msg-font": font ? `"${FONTS[font]}", system-ui, sans-serif` : null,
      "--fs": SIZES[look.size] || SIZES.m,
      "--toast-bg": dark ? "#f2f2f7" : "#1d1d26", "--toast-fg": dark ? "#121218" : "#ffffff",
      "color-scheme": dark ? "dark" : "light",
    },
    attrs: {
      "data-theme": dark ? "dark" : "light",
      "data-bubble": BUBBLES.includes(look.bubble) ? look.bubble : "round",
      "data-wall": look.wall || "none",
      "data-compact": look.compact ? "1" : null,
      "data-font": font || null,
      "data-motion": look.motion === "reduced" ? "reduced" : null,
    },
    bg,
  };
}

function applyLook(look) {
  const root = document.documentElement;
  const { vars, attrs, bg } = variables(look);
  for (const [k, v] of Object.entries(vars)) v == null ? root.style.removeProperty(k) : root.style.setProperty(k, v);
  for (const [k, v] of Object.entries(attrs)) v == null ? root.removeAttribute(k) : root.setAttribute(k, v);
  let meta = document.querySelector('meta[name="theme-color"]');
  if (!meta) {
    meta = document.createElement("meta");
    meta.name = "theme-color";
    document.head.appendChild(meta);
  }
  meta.content = bg;
  write(CACHE_KEY, { css: root.style.cssText, attrs });
  window.dispatchEvent(new Event("goofy-theme")); // players redraw their colours
}

export function lookSet(json) {
  let look;
  try { look = { ...DEFAULT_LOOK, ...JSON.parse(json) }; } catch { return; }
  write(THEME_KEY, look);
  applyLook(look);
}

/// The presets, coloured for the mode in effect, for the swatches.
export function lookPresets() {
  const look = { ...DEFAULT_LOOK, ...read(THEME_KEY) };
  return JSON.stringify(PRESETS.map((p) => {
    const dark = p.dark || look.mode === "dark" || (look.mode !== "light" && darkQuery.matches);
    const [bg, surface, text, , , incoming] = dark ? p.d : p.l;
    return { id: p.id, name: p.name, a: p.a, b: p.b, on: p.on || "#ffffff", bg, surface, text, incoming };
  }));
}

/// A profile name colour (`#rrggbb`) made readable on the current theme: lightened (dark
/// theme) or darkened (light theme), keeping its hue, until its contrast against the
/// background and the surface colours is at least 3:1. "" or an invalid colour gives "".
export function lookNameColor(color) {
  if (!HEX.test(color || "")) return "";
  const look = { ...DEFAULT_LOOK, ...read(THEME_KEY) };
  const preset = PRESETS.find((p) => p.id === look.preset) || PRESETS[0];
  const dark = isDark(look);
  const [bg, surface] = dark ? preset.d : preset.l;
  const readable = (c) => contrast(c, bg) >= 3 && contrast(c, surface) >= 3;
  if (readable(color)) return color.toLowerCase();
  const [h, s, l0] = toHsl(color);
  for (let i = 1; i <= 50; i++) {
    const l = Math.max(0, Math.min(1, l0 + (dark ? i : -i) * 0.02));
    const c = fromHsl(h, s, l);
    if (readable(c)) return c;
  }
  return dark ? "#ffffff" : "#000000";
}

applyLook({ ...DEFAULT_LOOK, ...read(THEME_KEY) });
darkQuery.addEventListener("change", () => applyLook({ ...DEFAULT_LOOK, ...read(THEME_KEY) }));

// ---- sounds ----

const DEFAULT_SOUNDS = { ring: "classic", track: null, start: 30, msg: false, msgTone: "pop" };
export function soundsGet() { return JSON.stringify({ ...DEFAULT_SOUNDS, ...read(SOUND_KEY) }); }
export function soundsSet(json) {
  try { write(SOUND_KEY, { ...DEFAULT_SOUNDS, ...JSON.parse(json) }); } catch {}
}
function sounds() { return { ...DEFAULT_SOUNDS, ...read(SOUND_KEY) }; }

let audioCtx = null;
function context() {
  if (!audioCtx || audioCtx.state === "closed") audioCtx = new (window.AudioContext || window.webkitAudioContext)();
  if (audioCtx.state === "suspended") audioCtx.resume().catch(() => {});
  return audioCtx;
}

// One note: oscillator through an envelope. Returns nothing; scheduled ahead.
function note(ctx, out, { at, freq, len, type = "sine", vol = 0.16, attack = 0.012, slide = 0, vibrato = 0 }) {
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();
  osc.type = type;
  osc.frequency.setValueAtTime(freq, at);
  if (slide) osc.frequency.exponentialRampToValueAtTime(slide, at + len);
  if (vibrato) {
    const lfo = ctx.createOscillator();
    const depth = ctx.createGain();
    lfo.frequency.value = vibrato;
    depth.gain.value = freq * 0.04;
    lfo.connect(depth).connect(osc.frequency);
    lfo.start(at);
    lfo.stop(at + len + 0.05);
  }
  gain.gain.setValueAtTime(0.0001, at);
  gain.gain.exponentialRampToValueAtTime(vol, at + attack);
  gain.gain.exponentialRampToValueAtTime(0.0001, at + len);
  osc.connect(gain).connect(out);
  osc.start(at);
  osc.stop(at + len + 0.02);
}

// Ring tones: one cycle each, repeated every `every` seconds.
const RINGS = {
  classic: { every: 2.2, play(ctx, out, t) {
    [[0, 784], [0.18, 988], [0.36, 784], [0.54, 988]].forEach(([at, freq]) => note(ctx, out, { at: t + at, freq, len: 0.16, vol: 0.18 }));
  } },
  soft: { every: 3, play(ctx, out, t) {
    [523.25, 659.25, 783.99, 1046.5].forEach((freq, i) => {
      note(ctx, out, { at: t + i * 0.22, freq, len: 1.3, vol: 0.09, attack: 0.08 });
      note(ctx, out, { at: t + i * 0.22, freq: freq * 2, len: 0.8, vol: 0.02, attack: 0.08 });
    });
  } },
  retro: { every: 1.8, play(ctx, out, t) {
    [523, 659, 784, 1047, 784, 1047, 1319, 1047].forEach((freq, i) =>
      note(ctx, out, { at: t + i * 0.085, freq, len: 0.08, type: "square", vol: 0.05, attack: 0.004 }));
  } },
  goofy: { every: 2.2, play(ctx, out, t) {
    note(ctx, out, { at: t, freq: 220, slide: 880, len: 0.32, type: "triangle", vol: 0.2, vibrato: 18 });
    note(ctx, out, { at: t + 0.38, freq: 880, slide: 330, len: 0.26, type: "triangle", vol: 0.18 });
    note(ctx, out, { at: t + 0.7, freq: 660, len: 0.09, type: "square", vol: 0.05 });
    note(ctx, out, { at: t + 0.82, freq: 990, len: 0.12, type: "square", vol: 0.05 });
  } },
  marimba: { every: 2.4, play(ctx, out, t) {
    [[0, 880], [0.14, 1108.7], [0.28, 1318.5], [0.42, 1108.7], [0.7, 880], [0.84, 1318.5]].forEach(([at, freq]) => {
      note(ctx, out, { at: t + at, freq, len: 0.35, vol: 0.16, attack: 0.004 });
      note(ctx, out, { at: t + at, freq: freq * 4, len: 0.06, vol: 0.03, attack: 0.002 });
    });
  } },
};

const TONES = {
  pop(ctx, out, t) { note(ctx, out, { at: t, freq: 520, slide: 1240, len: 0.12, vol: 0.2, attack: 0.005 }); },
  ding(ctx, out, t) {
    note(ctx, out, { at: t, freq: 1318.5, len: 0.6, vol: 0.13, attack: 0.004 });
    note(ctx, out, { at: t, freq: 2637, len: 0.25, vol: 0.03, attack: 0.004 });
    note(ctx, out, { at: t + 0.12, freq: 1760, len: 0.7, vol: 0.1, attack: 0.004 });
  },
  drop(ctx, out, t) { note(ctx, out, { at: t, freq: 380, slide: 1500, len: 0.16, vol: 0.2, attack: 0.004 }); },
  bit(ctx, out, t) {
    note(ctx, out, { at: t, freq: 988, len: 0.07, type: "square", vol: 0.05, attack: 0.003 });
    note(ctx, out, { at: t + 0.08, freq: 1319, len: 0.12, type: "square", vol: 0.05, attack: 0.003 });
  },
  boing(ctx, out, t) { note(ctx, out, { at: t, freq: 180, slide: 620, len: 0.28, type: "triangle", vol: 0.2, vibrato: 22 }); },
};

// A tone loop on its own gain node, so stopping silences scheduled notes too.
function loopTone(id) {
  const ring = RINGS[id] || RINGS.classic;
  const ctx = context();
  const out = ctx.createGain();
  out.connect(ctx.destination);
  const cycle = () => ring.play(ctx, out, ctx.currentTime + 0.03);
  cycle();
  const timer = setInterval(cycle, ring.every * 1000);
  return () => {
    clearInterval(timer);
    out.gain.setValueAtTime(out.gain.value, ctx.currentTime);
    out.gain.linearRampToValueAtTime(0, ctx.currentTime + 0.05);
    setTimeout(() => out.disconnect(), 200);
  };
}

// A goofysound song, looped from the chosen start. Falls back to the built-in
// tone if it does not start within 3 s or fails (403/503 from the server).
function loopTrack(track, start, fallback) {
  const audio = new Audio();
  let stopped = false, fell = null;
  const fail = () => {
    if (stopped || fell) return;
    audio.pause();
    audio.removeAttribute("src");
    fell = loopTone(fallback);
  };
  const timer = setTimeout(() => { if (audio.paused || audio.readyState < 3) fail(); }, 3000);
  audio.preload = "auto";
  audio.src = track.url;
  audio.addEventListener("loadedmetadata", () => {
    const d = audio.duration;
    audio.currentTime = isFinite(d) && d > start + 5 ? start : 0;
  }, { once: true });
  audio.addEventListener("ended", () => {
    if (stopped || fell) return;
    audio.currentTime = isFinite(audio.duration) && audio.duration > start + 5 ? start : 0;
    audio.play().catch(fail);
  });
  audio.addEventListener("error", fail);
  audio.play().catch(fail);
  return () => {
    stopped = true;
    clearTimeout(timer);
    // Fades out rather than cutting.
    const fade = setInterval(() => {
      if (audio.volume > 0.1) audio.volume -= 0.1;
      else { clearInterval(fade); audio.pause(); audio.removeAttribute("src"); audio.load(); }
    }, 25);
    fell?.();
  };
}

function startRing(s = sounds()) {
  const fallback = RINGS[s.ring] ? s.ring : "classic";
  if (s.ring === "track" && s.track?.url) return loopTrack(s.track, Number(s.start) || 0, "classic");
  return loopTone(fallback);
}

let ringStop = null;
function ring(on) {
  if (!on) {
    ringStop?.();
    ringStop = null;
    return;
  }
  if (ringStop) return;
  stopPreview();
  try { ringStop = startRing(); } catch { ringStop = null; }
}

let previewStop = null, previewTimer = null;
function stopPreview() {
  clearTimeout(previewTimer);
  previewStop?.();
  previewStop = null;
}

/// "ring:<id>" (current settings for "ring:track"), "msg:<id>". Plays a few
/// seconds; a second call while playing stops it.
export function soundPreview(kind) {
  const playing = !!previewStop;
  stopPreview();
  if (playing && kind === previewKind) { previewKind = ""; return false; }
  previewKind = kind;
  try {
    const [group, id] = kind.split(":");
    if (group === "msg") {
      const ctx = context();
      (TONES[id] || TONES.pop)(ctx, ctx.destination, ctx.currentTime + 0.02);
      previewKind = "";
      return false;
    }
    const s = sounds();
    window.dispatchEvent(new Event("goofy-media-play")); // pauses the music player
    previewStop = id === "track" ? startRing({ ...s, ring: "track" }) : loopTone(id);
    previewTimer = setTimeout(() => { stopPreview(); previewKind = ""; window.dispatchEvent(new Event("goofy-preview-end")); },
      id === "track" ? 10000 : 4500);
    return true;
  } catch { return false; }
}
let previewKind = "";
export function soundStop() { stopPreview(); previewKind = ""; }
export function onPreviewEnd(callback) { window.addEventListener("goofy-preview-end", () => callback("")); }

let lastMessageSound = 0;
/// Short sound for a new message, if the user turned it on.
export function messageSound() {
  const s = sounds();
  if (!s.msg || ringStop) return;
  // Android rings through its own notification while the app is hidden.
  if (window.GoofyNative && document.hidden) return;
  const now = Date.now();
  if (now - lastMessageSound < 800) return;
  lastMessageSound = now;
  try {
    const ctx = context();
    (TONES[s.msgTone] || TONES.pop)(ctx, ctx.destination, ctx.currentTime + 0.02);
  } catch {}
}

// Used by bridge.js for incoming calls.
window.GoofyLook = { ring, nameColor: lookNameColor };
