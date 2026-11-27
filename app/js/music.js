// One shared player for goofysound tracks: shared songs, previews in the music
// sheet and "listen along" from the activity pane. Only one plays at a time, and
// it pauses (and is paused by) every other <audio>/<video> of the page.

let audio = null;
let slot = "";
let failed = "";
const listeners = [];

// Call streams must never be paused by music.
const CALL = new Set(["remote-video", "local-video"]);

function state() {
  const a = audio;
  return JSON.stringify({
    slot,
    playing: !!a && !a.paused && !a.ended,
    loading: !!a && !a.paused && a.readyState < 3,
    time: a ? a.currentTime || 0 : 0,
    duration: a && isFinite(a.duration) ? a.duration : 0,
    ended: !!a && a.ended,
    error: failed,
  });
}

function emit() {
  const s = state();
  listeners.forEach((f) => { try { f(s); } catch { /* listener gone */ } });
}

function player() {
  if (audio) return audio;
  audio = new Audio();
  audio.preload = "none";
  ["play", "playing", "pause", "ended", "timeupdate", "loadedmetadata", "durationchange", "waiting", "seeked", "emptied"]
    .forEach((name) => audio.addEventListener(name, emit));
  audio.addEventListener("error", () => {
    if (audio.getAttribute("src")) { failed = "Lecture impossible."; emit(); }
  });
  if ("mediaSession" in navigator) {
    try {
      navigator.mediaSession.setActionHandler("play", () => audio.play().catch(() => {}));
      navigator.mediaSession.setActionHandler("pause", () => audio.pause());
    } catch { /* unsupported action */ }
  }
  return audio;
}

function pauseOthers() {
  document.querySelectorAll("audio, video").forEach((m) => {
    if (m !== audio && !CALL.has(m.id) && !m.paused) m.pause();
  });
  window.dispatchEvent(new CustomEvent("goofy-music-play"));
}

// Any other media of the page that starts playing pauses the music.
document.addEventListener("play", (e) => {
  if (audio && e.target !== audio && !CALL.has(e.target?.id) && !audio.paused) audio.pause();
}, true);
// Other players (voice messages, ringtone previews) may ask for silence too.
window.addEventListener("goofy-media-play", () => { if (audio && !audio.paused) audio.pause(); });

export function musicOnState(callback) {
  listeners.push(callback);
  callback(state());
}

/** Plays `url` in `key`'s slot, resuming when that slot is already loaded. */
export function musicPlay(key, url, title, artist, cover) {
  const a = player();
  pauseOthers();
  failed = "";
  if (slot !== key || a.getAttribute("src") !== url) {
    slot = key;
    a.src = url;
  } else if (a.ended) {
    a.currentTime = 0;
  }
  if ("mediaSession" in navigator && window.MediaMetadata) {
    try {
      navigator.mediaSession.metadata = new MediaMetadata({
        title, artist, artwork: cover ? [{ src: cover, sizes: "300x300" }] : [],
      });
    } catch { /* ignore */ }
  }
  a.play().catch((e) => {
    if (e && e.name === "AbortError") return;
    failed = e && e.name === "NotAllowedError" ? "Touchez ▶ pour lancer la lecture." : "Lecture impossible.";
    emit();
  });
  emit();
}

export function musicPause() { if (audio) audio.pause(); }

export function musicSeek(seconds) {
  if (!audio || !audio.getAttribute("src")) return;
  try { audio.currentTime = Math.max(0, seconds); } catch { /* not seekable yet */ }
  emit();
}

/** Stops and unloads the player when its slot starts with `prefix` ("" stops anything). */
export function musicStop(prefix) {
  if (!audio || !slot.startsWith(prefix)) return;
  audio.pause();
  audio.removeAttribute("src");
  audio.load();
  slot = "";
  failed = "";
  emit();
}

// Small per-device preferences of these features (e.g. the collapsed activity pane).
export function musicPref(key) {
  try { return localStorage.getItem("goofychat." + key) || ""; } catch { return ""; }
}
export function setMusicPref(key, value) {
  try { localStorage.setItem("goofychat." + key, value); } catch { /* storage blocked */ }
}
