// Audio players in messages. The interface (Rust) draws the frame; this file
// draws the waveform on a canvas, plays through one shared <audio> element (so
// only one plays at a time) and writes the time and speed into data attributes
// that CSS displays. Peaks are decoded once per media and cached.

const BARS = 40;
const PEAKS_KEY = "goofychat.peaks";
const SPEEDS = [1, 1.5, 2];

const players = new Map(); // element id -> state
let peaksCache = null;
const pending = new Map(); // media id -> Promise<{ peaks, duration }>
let queue = Promise.resolve();

function storedPeaks() {
  if (!peaksCache) {
    try { peaksCache = JSON.parse(localStorage.getItem(PEAKS_KEY) || "{}"); } catch { peaksCache = {}; }
  }
  return peaksCache;
}
function storePeaks(id, value) {
  const all = storedPeaks();
  all[id] = value;
  const keys = Object.keys(all);
  if (keys.length > 300) keys.slice(0, keys.length - 300).forEach((k) => delete all[k]);
  try { localStorage.setItem(PEAKS_KEY, JSON.stringify(all)); } catch {}
}

// Stable pseudo-random waveform from the media id, for files that cannot be decoded.
function fakePeaks(id) {
  let h = 2166136261;
  for (const c of id) h = Math.imul(h ^ c.charCodeAt(0), 16777619);
  const rand = () => ((h = Math.imul(h ^ (h >>> 15), 2246822507) ^ Math.imul(h ^ (h >>> 13), 3266489909)) >>> 0) / 4294967296;
  const raw = Array.from({ length: BARS }, () => 0.2 + rand() * 0.8);
  return raw.map((v, i) => Math.round(100 * (0.5 * v + 0.25 * (raw[i - 1] ?? v) + 0.25 * (raw[i + 1] ?? v))));
}

async function decode(id, url) {
  const res = await fetch(url);
  if (!res.ok) throw new Error("http");
  const data = await res.arrayBuffer();
  const ctx = new OfflineAudioContext(1, 1, 44100);
  const buffer = await new Promise((ok, ko) => {
    const p = ctx.decodeAudioData(data, ok, ko);
    if (p && p.then) p.then(ok, ko);
  });
  const channel = buffer.getChannelData(0);
  const size = Math.floor(channel.length / BARS) || 1;
  const levels = [];
  for (let i = 0; i < BARS; i++) {
    let sum = 0;
    const from = i * size, to = Math.min(channel.length, from + size);
    for (let j = from; j < to; j += 4) sum += channel[j] * channel[j];
    levels.push(Math.sqrt(sum / Math.max(1, (to - from) / 4)));
  }
  const max = Math.max(...levels, 1e-4);
  const peaks = levels.map((v) => Math.round(100 * Math.max(0.1, Math.pow(v / max, 0.7))));
  return { p: peaks, d: buffer.duration };
}

function peaksFor(id, url) {
  const known = storedPeaks()[id];
  if (known) return Promise.resolve(known);
  if (!pending.has(id)) {
    // One decode at a time: a conversation may hold many voice messages.
    const job = queue.then(() => decode(id, url)).then(
      (value) => { storePeaks(id, value); return value; },
      () => ({ p: fakePeaks(id), d: 0, fake: true }),
    );
    queue = job.catch(() => {});
    pending.set(id, job);
  }
  return pending.get(id);
}

// ---- the shared player ----

const audio = new Audio();
audio.preload = "auto";
let current = null; // state of the player in use
let frame = 0;

function fmt(s) {
  if (!isFinite(s) || s < 0) s = 0;
  s = Math.floor(s);
  return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0");
}

function duration(state) {
  if (state === current && isFinite(audio.duration) && audio.duration > 0) return audio.duration;
  return state.duration || 0;
}

function render(state) {
  const el = document.getElementById(state.id);
  if (!el) return;
  const total = duration(state);
  const pos = state === current ? audio.currentTime : state.pos;
  const progress = total > 0 ? Math.min(1, pos / total) : 0;
  const active = state === current && (!audio.paused || state.pos > 0 || audio.currentTime > 0);
  el.dataset.state = state === current && !audio.paused ? "playing" : active ? "paused" : "idle";
  const time = el.querySelector(".vm-time");
  if (time) time.dataset.t = active ? fmt(pos) + " / " + fmt(total) : total ? fmt(total) : "0:00";
  const speed = el.querySelector(".vm-speed");
  if (speed) speed.dataset.s = String(state.speed).replace(".", ",") + "×";
  draw(state, el, progress);
}

function draw(state, el, progress) {
  const canvas = el.querySelector("canvas");
  if (!canvas) return;
  const ratio = window.devicePixelRatio || 1;
  const w = canvas.clientWidth, h = canvas.clientHeight;
  if (!w || !h) return;
  if (canvas.width !== Math.round(w * ratio) || canvas.height !== Math.round(h * ratio)) {
    canvas.width = Math.round(w * ratio);
    canvas.height = Math.round(h * ratio);
  }
  const g = canvas.getContext("2d");
  g.setTransform(ratio, 0, 0, ratio, 0, 0);
  g.clearRect(0, 0, w, h);
  const color = getComputedStyle(canvas).color;
  g.fillStyle = color;
  if (state.kind === "file") {
    // Slim track with a knob.
    const y = h / 2, r = 2;
    g.globalAlpha = 0.28;
    g.beginPath(); round(g, 0, y - r, w, r * 2, r); g.fill();
    g.globalAlpha = 1;
    g.beginPath(); round(g, 0, y - r, Math.max(r * 2, w * progress), r * 2, r); g.fill();
    g.beginPath(); g.arc(Math.max(5, Math.min(w - 5, w * progress)), y, 5, 0, Math.PI * 2); g.fill();
    return;
  }
  const peaks = state.peaks || [];
  const step = w / BARS;
  const bar = Math.max(2, Math.min(4, step * 0.58));
  for (let i = 0; i < BARS; i++) {
    const v = (peaks[i] ?? 20) / 100;
    const bh = Math.max(3, v * (h - 2));
    const x = i * step + (step - bar) / 2;
    const done = (i + 0.5) / BARS <= progress;
    g.globalAlpha = done ? 1 : 0.36;
    g.beginPath();
    round(g, x, (h - bh) / 2, bar, bh, bar / 2);
    g.fill();
  }
  g.globalAlpha = 1;
}

function round(g, x, y, w, h, r) {
  if (g.roundRect) g.roundRect(x, y, w, h, r);
  else g.rect(x, y, w, h);
}

function tick() {
  cancelAnimationFrame(frame);
  if (!current) return;
  render(current);
  if (!audio.paused) frame = requestAnimationFrame(tick);
}

audio.addEventListener("play", tick);
audio.addEventListener("pause", tick);
audio.addEventListener("loadedmetadata", () => {
  // WebM from MediaRecorder has no duration until the end is found.
  if (audio.duration === Infinity && current && !current.duration) {
    const back = audio.currentTime;
    audio.currentTime = 1e9;
    audio.addEventListener("timeupdate", function fix() {
      audio.removeEventListener("timeupdate", fix);
      if (current && isFinite(audio.duration)) current.duration = audio.duration;
      audio.currentTime = back;
    });
  }
  if (current) render(current);
});
audio.addEventListener("ended", () => {
  if (!current) return;
  const done = current;
  done.pos = 0;
  current = null;
  audio.removeAttribute("src");
  done.loaded = false;
  render(done);
});

function select(state) {
  if (current === state) return;
  if (current) {
    const old = current;
    old.pos = 0;
    audio.pause();
    current = null;
    render(old);
  }
  current = state;
  audio.src = state.url;
  audio.playbackRate = state.speed;
  audio.currentTime = state.pos || 0;
}

export function voiceMount(id) {
  const el = document.getElementById(id);
  if (!el) return;
  let state = players.get(id);
  if (!state) {
    state = { id, media: el.dataset.media, url: el.dataset.url, kind: el.dataset.kind || "voice", speed: 1, pos: 0, duration: 0, peaks: null };
    players.set(id, state);
  }
  render(state);
  if (state.kind === "voice") {
    peaksFor(state.media, state.url).then((v) => {
      state.peaks = v.p;
      if (v.d) state.duration = v.d;
      render(state);
    });
  }
  if (!state.duration) {
    const probe = new Audio();
    probe.preload = "metadata";
    probe.addEventListener("loadedmetadata", () => {
      if (isFinite(probe.duration) && !state.duration) { state.duration = probe.duration; render(state); }
      probe.removeAttribute("src");
    }, { once: true });
    probe.src = state.url;
  }
  // Tap or drag on the waveform seeks.
  const canvas = el.querySelector("canvas");
  if (canvas && !canvas.dataset.bound) {
    canvas.dataset.bound = "1";
    const seek = (e) => {
      const box = canvas.getBoundingClientRect();
      const f = Math.max(0, Math.min(1, (e.clientX - box.left) / box.width));
      const total = duration(state);
      if (!total) return;
      if (current !== state) select(state);
      audio.currentTime = f * total;
      state.pos = audio.currentTime;
      render(state);
    };
    canvas.addEventListener("pointerdown", (e) => {
      e.preventDefault();
      canvas.setPointerCapture?.(e.pointerId);
      state.dragging = true;
      seek(e);
    });
    canvas.addEventListener("pointermove", (e) => { if (state.dragging) seek(e); });
    const up = () => {
      if (!state.dragging) return;
      state.dragging = false;
      if (current === state && audio.paused) audio.play().catch(() => {});
    };
    canvas.addEventListener("pointerup", up);
    canvas.addEventListener("pointercancel", up);
    canvas.addEventListener("click", (e) => e.stopPropagation());
  }
}

export function voiceUnmount(id) {
  const state = players.get(id);
  if (!state) return;
  if (current === state) {
    audio.pause();
    audio.removeAttribute("src");
    current = null;
  }
  players.delete(id);
}

export function voiceToggle(id) {
  const state = players.get(id);
  if (!state) return;
  if (current !== state) select(state);
  if (audio.paused) {
    window.dispatchEvent(new Event("goofy-media-play")); // the music player (music.js) pauses
    audio.play().catch(() => {});
  } else {
    state.pos = audio.currentTime;
    audio.pause();
  }
}

export function voiceSpeed(id) {
  const state = players.get(id);
  if (!state) return;
  state.speed = SPEEDS[(SPEEDS.indexOf(state.speed) + 1) % SPEEDS.length];
  if (current === state) audio.playbackRate = state.speed;
  render(state);
}

/// Stops whatever message is playing (a call started, the screen changed).
export function voiceStop() {
  if (!current) return;
  const old = current;
  audio.pause();
  old.pos = audio.currentTime;
  render(old);
}

window.addEventListener("goofy-theme", () => players.forEach(render));
window.addEventListener("goofy-call", voiceStop);
window.addEventListener("goofy-music-play", voiceStop);
window.addEventListener("resize", () => players.forEach(render));
