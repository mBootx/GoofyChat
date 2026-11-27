// Browser APIs that are much simpler to drive from JavaScript than through web-sys:
// WebSocket, session storage, uploads, voice recording and WebRTC calls.

let server = "";
let token = "";
let myId = 0;
let emit = () => {};
let socket = null;
let heartbeat = null;
let retry = null;
let lastSeen = 0;

const KEY = "goofychat.session";

// ---- platform ----
// Android injects GoofyNative; the desktop shell is Tauri without it; a plain
// browser has neither.
const android = !!window.GoofyNative;
const tauri = android ? null : window.__TAURI_INTERNALS__ || null;
document.documentElement.classList.add(android ? "android" : tauri ? "desktop" : "browser");

export function isDesktop() { return !!tauri; }

function invoke(command, args = {}) {
  return tauri ? tauri.invoke(command, args) : Promise.reject("Indisponible.");
}

export function loadSession() { return localStorage.getItem(KEY) || ""; }
export function saveSession(json) { localStorage.setItem(KEY, json); }
export function clearSession() { localStorage.removeItem(KEY); }

let outbox = []; // call signals waiting for the connection

function send(event) {
  if (socket && socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(event));
  else if (event.type === "call") outbox.push(event);
}

function open() {
  clearTimeout(retry);
  if (!token) return;
  const ws = new WebSocket(server.replace(/^http/, "ws") + "/api/ws");
  socket = ws;
  lastSeen = Date.now();
  ws.onopen = () => ws.send(JSON.stringify({ type: "auth", token }));
  ws.onmessage = (e) => {
    lastSeen = Date.now();
    const event = JSON.parse(e.data);
    if (event.type === "call") onSignal(event.from, event.signal);
    else if (event.type !== "pong") {
      if (event.type === "ready") {
        outbox.splice(0).forEach(send);
        sendActive();
      }
      emit(e.data);
    }
  };
  ws.onclose = () => {
    if (socket !== ws) return;
    socket = null;
    if (token) retry = setTimeout(open, 3000);
  };
}

export function connect(url, sessionToken, userId, callback) {
  disconnect();
  server = url;
  token = sessionToken;
  myId = userId;
  emit = callback;
  open();
  heartbeat = setInterval(() => {
    if (socket && Date.now() - lastSeen > 70000) socket.close();
    send({ type: "ping" });
  }, 25000);
  // Native background service, present only inside the Android app.
  window.GoofyNative?.start(url, sessionToken, String(userId));
}

export function disconnect() {
  if (call) stop("offline", "Appel interrompu : déconnexion du serveur.");
  token = "";
  outbox = [];
  clearInterval(heartbeat);
  clearTimeout(retry);
  socket?.close();
  socket = null;
}

export function logoutNative() { window.GoofyNative?.stop(); }

export function sendTyping(conversation) {
  send({ type: "typing", conversation_id: conversation });
}

document.addEventListener("visibilitychange", () => {
  if (document.hidden || !token) return;
  // A socket silent for this long is probably dead after a suspension.
  if (socket && Date.now() - lastSeen > 30000) {
    const old = socket;
    socket = null;
    old.close();
  }
  if (!socket) open();
});

// ---- presence ----
// The server counts a socket as "online" only while it has sent { active: true }. The
// desktop app counts as soon as it runs (even in the tray); a phone or browser tab counts
// while it is on screen. Sent after `ready` (see open()) and on every visibility change.
function activeNow() { return !!tauri || !document.hidden; }
function sendActive() { send({ type: "active", active: activeNow() }); }
document.addEventListener("visibilitychange", () => { if (token && !tauri) sendActive(); });

export function scrollBottom(id) {
  requestAnimationFrame(() => requestAnimationFrame(() => {
    const el = document.getElementById(id);
    if (el) el.scrollTop = el.scrollHeight;
  }));
}

// The Android back button walks the WebView history.
export function pushScreen() { history.pushState({}, ""); }
export function popScreen() { history.back(); }
export function onBack(callback) { window.addEventListener("popstate", () => callback()); }

export function pickFile(inputId) { document.getElementById(inputId)?.click(); }

// ---- layout and keyboard ----

const wideQuery = matchMedia("(min-width: 900px)");
export function isWide() { return !android && wideQuery.matches; }
export function onWide(callback) {
  if (!android) wideQuery.addEventListener("change", () => callback(wideQuery.matches ? "1" : ""));
}

export function onEscape(callback) {
  const handler = (e) => { if (e.key === "Escape" && !e.isComposing) callback(""); };
  window.addEventListener("keydown", handler);
  return () => window.removeEventListener("keydown", handler);
}
export function offListener(remove) { if (typeof remove === "function") remove(); }

export function focusInput(id) {
  requestAnimationFrame(() => document.getElementById(id)?.focus({ preventScroll: true }));
}

// Files dropped on the window or pasted from the clipboard (not on Android).
export function onAttach(callback) {
  if (android) return null;
  const accept = (f) => /^(image|video|audio)\//.test(f.type);
  const send = (files) => files.filter(accept).slice(0, 10).forEach((file) => {
    callback("start");
    upload(file, file.name || "image.png").then(
      (id) => callback("ok:" + id),
      (e) => callback("err:" + (typeof e === "string" ? e : "Échec de l'envoi.")),
    );
  });
  const hasFiles = (e) => [...(e.dataTransfer?.types || [])].includes("Files");
  const over = (e) => {
    if (!hasFiles(e)) return;
    e.preventDefault();
    document.body.classList.add("dropping");
  };
  const leave = (e) => { if (!e.relatedTarget) document.body.classList.remove("dropping"); };
  const drop = (e) => {
    document.body.classList.remove("dropping");
    if (!hasFiles(e)) return;
    e.preventDefault();
    send([...e.dataTransfer.files]);
  };
  const paste = (e) => {
    const files = [...(e.clipboardData?.files || [])].filter(accept);
    if (!files.length) return;
    e.preventDefault();
    send(files);
  };
  window.addEventListener("dragover", over);
  window.addEventListener("dragleave", leave);
  window.addEventListener("drop", drop);
  window.addEventListener("paste", paste);
  return () => {
    document.body.classList.remove("dropping");
    window.removeEventListener("dragover", over);
    window.removeEventListener("dragleave", leave);
    window.removeEventListener("drop", drop);
    window.removeEventListener("paste", paste);
  };
}

// ---- desktop shell ----

// The window may be in the tray or behind other windows.
export function isActive() { return !tauri || (document.hasFocus() && !document.hidden); }
export function onActive(callback) { if (tauri) window.addEventListener("focus", () => callback("")); }

// The shell only shows it when the window is hidden or not focused.
export function notifyDesktop(title, body, isCall) {
  if (tauri) invoke("notify", { title, body, call: !!isCall }).catch(() => {});
}
export function setUnread(count) { if (tauri) invoke("set_unread", { count }).catch(() => {}); }

export async function checkUpdate() {
  if (!tauri) return "";
  try {
    const found = await invoke("check_update");
    return found ? JSON.stringify(found) : "";
  } catch {
    return ""; // offline or rate limited: the next check will tell
  }
}
export async function installUpdate(url) {
  try { await invoke("install_update", { url }); } catch (e) { throw String(e); }
}
export async function getAutostart() {
  if (!tauri) return false;
  try { return !!(await invoke("autostart_enabled")); } catch { return false; }
}
export async function setAutostart(enabled) {
  try { await invoke("set_autostart", { enabled }); } catch (e) { throw String(e); }
}
export async function appVersion() {
  if (!tauri) return "";
  try { return String(await invoke("app_version")); } catch { return ""; }
}

// Ring tone for incoming calls while the call screen is shown (look.js plays
// the chosen tone or goofysound song). On Android the system rings through the
// call notification while the app is hidden, so the page stays quiet then.
function ringer(on) {
  window.GoofyLook?.ring(on && !(android && document.hidden));
}
document.addEventListener("visibilitychange", () => ringer(call?.state === "incoming"));

// ---- uploads ----

async function upload(blob, name) {
  const form = new FormData();
  form.append("file", blob, name);
  const res = await fetch(server + "/api/media", {
    method: "POST",
    headers: { Authorization: "Bearer " + token },
    body: form,
  });
  if (!res.ok) {
    const body = await res.json().catch(() => null);
    throw body?.error || (res.status === 413 ? "Fichier trop volumineux." : "Échec de l'envoi.");
  }
  return (await res.json()).id;
}

// Shared helpers for the other JS modules (they never import each other):
// window.GoofyBridge.{uploadBlob, session, vibrate}.
function uploadBlob(blob, name, onProgress) {
  return new Promise((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", server + "/api/media");
    xhr.setRequestHeader("Authorization", "Bearer " + token);
    if (onProgress && xhr.upload) {
      xhr.upload.onprogress = (e) => { if (e.lengthComputable) onProgress(e.loaded / e.total); };
    }
    xhr.onerror = xhr.ontimeout = () => reject("Échec de l'envoi.");
    xhr.onload = () => {
      let body = null;
      try { body = JSON.parse(xhr.responseText); } catch {}
      if (xhr.status >= 200 && xhr.status < 300 && body?.id) {
        if (onProgress) onProgress(1);
        resolve(body.id);
      } else {
        reject(body?.error || (xhr.status === 413 ? "Fichier trop volumineux." : "Échec de l'envoi."));
      }
    };
    const form = new FormData();
    form.append("file", blob, name);
    xhr.send(form);
  });
}

window.GoofyBridge = {
  // Uploads to /api/media with the session token; resolves to the media id, rejects with a
  // French message. onProgress(fraction 0..1) is optional.
  uploadBlob,
  // { server, token, userId } of the current session.
  session: () => ({ server, token, userId: myId }),
  // Short vibration where supported (Android native bridge first, then navigator.vibrate).
  vibrate: (ms) => {
    if (window.GoofyNative?.vibrate) window.GoofyNative.vibrate(ms);
    else navigator.vibrate?.(ms);
  },
};

export async function uploadFromInput(inputId) {
  const input = document.getElementById(inputId);
  const file = input?.files?.[0];
  if (!file) return "";
  try {
    return await upload(file, file.name);
  } finally {
    input.value = "";
  }
}

// ---- voice messages ----

let recorder = null;
let chunks = [];

let starting = false;
let generation = 0;

export function cancelRecording() {
  generation++;
  closeMeter();
  const active = recorder;
  recorder = null;
  if (!active) return;
  active.stop();
  active.stream.getTracks().forEach((t) => t.stop());
}

// Live input level while recording, for the recording bar.
let meter = null; // { ctx, analyser, data, start, levels, last }

function openMeter(stream) {
  meter = { start: Date.now(), levels: [], last: 0 };
  try {
    const ctx = new AudioContext();
    const analyser = ctx.createAnalyser();
    analyser.fftSize = 512;
    ctx.createMediaStreamSource(stream).connect(analyser);
    Object.assign(meter, { ctx, analyser, data: new Uint8Array(analyser.fftSize) });
  } catch {}
}

function closeMeter() {
  meter?.ctx?.close().catch(() => {});
  meter = null;
}

/// Draws scrolling level bars into the canvas and the elapsed time into the
/// `data-t` attribute of the time element, until the recording ends.
export function recordMeter(canvasId, timeId) {
  const loop = () => {
    const canvas = document.getElementById(canvasId);
    if (!meter || !canvas) return;
    const now = Date.now();
    let level = 0;
    if (meter.analyser) {
      meter.analyser.getByteTimeDomainData(meter.data);
      let sum = 0;
      for (const v of meter.data) sum += (v - 128) * (v - 128);
      level = Math.min(1, Math.sqrt(sum / meter.data.length) / 40);
    } else {
      level = 0.15 + 0.1 * Math.sin(now / 120);
    }
    if (now - meter.last > 70) {
      meter.last = now;
      meter.levels.push(level);
      if (meter.levels.length > 120) meter.levels.shift();
    }
    const s = Math.floor((now - meter.start) / 1000);
    const time = document.getElementById(timeId);
    if (time) time.dataset.t = Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0");
    const ratio = window.devicePixelRatio || 1;
    const w = canvas.clientWidth, h = canvas.clientHeight;
    if (canvas.width !== Math.round(w * ratio)) canvas.width = Math.round(w * ratio);
    if (canvas.height !== Math.round(h * ratio)) canvas.height = Math.round(h * ratio);
    const g = canvas.getContext("2d");
    g.setTransform(ratio, 0, 0, ratio, 0, 0);
    g.clearRect(0, 0, w, h);
    g.fillStyle = getComputedStyle(canvas).color;
    const step = 5, bar = 3;
    const count = Math.floor(w / step);
    const shown = meter.levels.slice(-count);
    shown.forEach((v, i) => {
      const bh = Math.max(3, Math.pow(v, 0.6) * (h - 2));
      const x = w - (shown.length - i) * step;
      g.globalAlpha = 0.45 + 0.55 * (i / Math.max(1, shown.length));
      g.beginPath();
      if (g.roundRect) g.roundRect(x, (h - bh) / 2, bar, bh, 1.5); else g.rect(x, (h - bh) / 2, bar, bh);
      g.fill();
    });
    requestAnimationFrame(loop);
  };
  requestAnimationFrame(loop);
}

export async function startRecording() {
  if (starting || recorder) return false;
  starting = true;
  const mine = generation;
  let stream;
  try {
    stream = await navigator.mediaDevices.getUserMedia({ audio: true });
  } catch {
    throw "Accès au micro refusé.";
  } finally {
    starting = false;
  }
  // The user left the screen while the permission dialog was open.
  if (mine !== generation) {
    stream.getTracks().forEach((t) => t.stop());
    return false;
  }
  chunks = [];
  recorder = new MediaRecorder(stream);
  recorder.ondataavailable = (e) => chunks.push(e.data);
  recorder.start();
  openMeter(stream);
  return true;
}

export async function stopRecording() {
  const active = recorder;
  if (!active) return "";
  recorder = null;
  closeMeter();
  await new Promise((done) => { active.onstop = done; active.stop(); });
  active.stream.getTracks().forEach((t) => t.stop());
  const type = (active.mimeType || "audio/webm").split(";")[0];
  return upload(new Blob(chunks, { type }), "message-vocal.webm");
}

// ---- calls ----

// { id, conversation, video, peer, state, pc, local, queued, timer, retry, offerer, restarts,
//   screenLocal, screenRemote, screenStream, screenSender, remoteScreenStreamId, remoteStreams,
//   remoteMedia, remoteCameraTrack, remoteCamera, speakingLocal, speakingRemote,
//   makingOffer, ignoreOffer, readyForReneg, since, meterStop }
let call = null;
let report = () => {};

export function onCallState(callback) { report = callback; }

export function canShareScreen() {
  return Boolean(!android && navigator.mediaDevices && typeof navigator.mediaDevices.getDisplayMedia === "function");
}

export function toggleFullscreen(elementId) {
  try {
    if (document.fullscreenElement) {
      document.exitFullscreen().catch(() => {});
    } else {
      const el = document.getElementById(elementId);
      el?.requestFullscreen?.().catch(() => {});
    }
  } catch {}
}

// Re-attaches active call streams whenever the call UI changes layout (e.g. ringing -> stage
// or entering/leaving screen share), so video and audio keep playing without interruption.
export function syncCallMedia() {
  if (!call) return;
  const bind = (id, stream, muted) => {
    const el = document.getElementById(id);
    if (!el) return;
    if (muted) el.muted = true;
    if (el.srcObject !== (stream || null)) {
      el.srcObject = stream || null;
      if (stream) el.play?.().catch(() => {});
    }
  };
  bind("local-video", call.local, true);
  bind("remote-video", call.remoteMedia, false);
  bind("local-screen", call.screenStream, true);
  const rScreen = call.screenRemote && call.remoteScreenStreamId
    ? call.remoteStreams?.get(call.remoteScreenStreamId) || null
    : null;
  bind("remote-screen", rScreen, false);
  if (call.since) updateCallTimer(call.since);
}

// Every step of a call goes to the console, so a failed call can be explained.
const trace = (...args) => console.info("[appel]", ...args);

// Why the other side ended the call: payload `{ reason }` of "reject" and "hangup".
// Apps before 0.4.0 send no payload and ignore it.
const REASONS = {
  busy: "Votre correspondant est déjà en communication.",
  media: "Votre correspondant n'a pas pu accéder au micro ou à la caméra.",
  failed: "La connexion de l'appel a échoué.",
  timeout: "Impossible d'établir la connexion de l'appel.",
  error: "Erreur pendant l'appel.",
  offline: "Votre correspondant a été déconnecté.",
};

function publish(message = "") {
  trace("état :", call?.state || "idle", message);
  ringer(call?.state === "incoming");
  if (call?.state === "connected" && !call.since) call.since = Date.now();
  callTimer();
  if (call) window.dispatchEvent(new Event("goofy-call")); // pauses voice messages
  if (call) requestAnimationFrame(() => syncCallMedia());
  const shareable = canShareScreen();
  report(JSON.stringify(call
    ? {
        state: call.state,
        video: call.video,
        peer: call.peer,
        avatar: call.avatar || null,
        peerId: call.peerId || 0,
        muted: !!call.muted,
        cameraOff: !!call.cameraOff,
        message,
        screenLocal: !!call.screenLocal,
        screenRemote: !!call.screenRemote,
        canShare: shareable,
        speakingLocal: !!call.speakingLocal && !call.muted,
        speakingRemote: !!call.speakingRemote,
        remoteCamera: !!call.remoteCamera,
        since: call.since || 0,
      }
    : {
        state: "idle",
        video: false,
        peer: "",
        avatar: null,
        peerId: 0,
        muted: false,
        cameraOff: false,
        message,
        screenLocal: false,
        screenRemote: false,
        canShare: shareable,
        speakingLocal: false,
        speakingRemote: false,
        remoteCamera: false,
        since: 0,
      }));
}

function updateCallTimer(since) {
  const el = document.getElementById("call-timer");
  if (!el || !since) return;
  const s = Math.floor((Date.now() - since) / 1000);
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60);
  el.dataset.t = (h ? h + ":" + String(m).padStart(2, "0") : m) + ":" + String(s % 60).padStart(2, "0");
}

// Call duration, written into the `data-t` attribute of #call-timer.
let durationTimer = null;
function callTimer() {
  const since = call?.since;
  clearInterval(durationTimer);
  durationTimer = null;
  if (!since) return;
  const show = () => updateCallTimer(since);
  requestAnimationFrame(show);
  durationTimer = setInterval(show, 1000);
}

// Voice activity detection (AnalyserNode, RMS > 0.04, 300 ms hold) for speaking rings.
function startSpeaking(current) {
  stopSpeaking(current);
  let audioCtx = null;
  try {
    audioCtx = new AudioContext();
  } catch {
    return;
  }
  const makeProbe = (stream) => {
    if (!stream || stream.getAudioTracks().length === 0) return null;
    try {
      const analyser = audioCtx.createAnalyser();
      analyser.fftSize = 512;
      const src = audioCtx.createMediaStreamSource(stream);
      src.connect(analyser);
      return { analyser, data: new Uint8Array(analyser.fftSize), holdUntil: 0, src };
    } catch {
      return null;
    }
  };
  let localProbe = makeProbe(current.local);
  let remoteProbe = null;
  let remoteTrackId = "";

  const measure = (probe, now, forcedOff) => {
    if (!probe || forcedOff) return false;
    probe.analyser.getByteTimeDomainData(probe.data);
    let sum = 0;
    for (let i = 0; i < probe.data.length; i++) {
      const n = (probe.data[i] - 128) / 128;
      sum += n * n;
    }
    const rms = Math.sqrt(sum / probe.data.length);
    if (rms >= 0.04) probe.holdUntil = now + 300;
    return now < probe.holdUntil;
  };

  const timer = setInterval(() => {
    if (call !== current) {
      clearInterval(timer);
      audioCtx?.close().catch(() => {});
      return;
    }
    if (audioCtx.state === "suspended") audioCtx.resume().catch(() => {});
    const rTrack = current.remoteMedia?.getAudioTracks?.()[0];
    if (rTrack && rTrack.id !== remoteTrackId) {
      remoteTrackId = rTrack.id;
      try { remoteProbe?.src?.disconnect(); } catch {}
      remoteProbe = makeProbe(new MediaStream([rTrack]));
    }
    const now = Date.now();
    const loc = measure(localProbe, now, !!current.muted);
    const rem = measure(remoteProbe, now, false);
    if (loc !== !!current.speakingLocal || rem !== !!current.speakingRemote) {
      current.speakingLocal = loc;
      current.speakingRemote = rem;
      publish();
    }
  }, 80);

  current.meterStop = () => {
    clearInterval(timer);
    try { localProbe?.src?.disconnect(); } catch {}
    try { remoteProbe?.src?.disconnect(); } catch {}
    audioCtx?.close().catch(() => {});
  };
}

function stopSpeaking(current) {
  if (current?.meterStop) {
    current.meterStop();
    current.meterStop = null;
  }
}

function signal(action, payload = null, target = call) {
  send({
    type: "call",
    signal: { call_id: target.id, conversation_id: target.conversation, action, video: target.video, payload },
  });
}

// Every way a call ends says why: `message` is shown to the user.
function end(message) {
  if (!call) return;
  const ending = call;
  clearTimeout(ending.timer);
  clearTimeout(ending.retry);
  stopSpeaking(ending);
  ending.screenStream?.getTracks().forEach((t) => t.stop());
  ending.pc?.close();
  ending.local?.getTracks().forEach((t) => t.stop());
  for (const id of ["local-video", "remote-video", "local-screen", "remote-screen"]) {
    const el = document.getElementById(id);
    if (el) el.srcObject = null;
  }
  if (document.fullscreenElement) {
    document.exitFullscreen().catch(() => {});
  }
  call = null;
  try { window.GoofyNative?.call(false, false); } catch (e) { console.warn("[appel] service Android :", e); }
  publish(message || "Appel terminé.");
}

// Ends the call here and tells the other side why.
function stop(reason, message) {
  if (!call) return;
  signal("hangup", reason ? { reason } : null);
  end(message);
}

async function getMedia(current) {
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: { echoCancellation: true, noiseSuppression: true },
    video: current.video ? { facingMode: "user", width: { ideal: tauri ? 1280 : 640 } } : false,
  });
  // The call may have ended while the permission dialog was open.
  if (call !== current) {
    stream.getTracks().forEach((t) => t.stop());
    return false;
  }
  current.local = stream;
  const preview = document.getElementById("local-video");
  if (preview) {
    preview.muted = true; // setting the attribute alone does not mute
    preview.srcObject = stream;
  }
  // Keeps the microphone and camera alive when the app goes to the background.
  try { window.GoofyNative?.call(true, current.video); } catch (e) { console.warn("[appel] service Android :", e); }
  return true;
}

// "relay udp 1.2.3.4:5000" from a candidate line.
function describe(line) {
  const f = String(line || "").replace(/^candidate:/, "").split(" ");
  return `${f[7] || "?"} ${(f[2] || "").toLowerCase()} ${f[4] || ""}:${f[5] || ""}`;
}

function updateRemoteCamera(current) {
  const track = current.remoteCameraTrack;
  const active = Boolean(track && track.readyState === "live" && !track.muted);
  if (active !== !!current.remoteCamera) {
    current.remoteCamera = active;
    publish();
  }
}

function attachRemoteScreen(current, stream) {
  const el = document.getElementById("remote-screen");
  if (el) el.srcObject = stream;
  current.screenRemote = !!stream;
  publish();
}

async function createPeer(current) {
  let iceServers = [];
  try {
    const res = await fetch(server + "/api/ice", { headers: { Authorization: "Bearer " + token } });
    const list = await res.json().catch(() => null);
    if (!res.ok || !Array.isArray(list)) throw list?.error || "HTTP " + res.status;
    iceServers = list.map((s) => ({
      urls: s.urls, username: s.username || undefined, credential: s.credential || undefined,
    }));
  } catch (e) {
    // The call still works without TURN when the two devices can reach each other.
    console.warn("[appel] serveurs STUN/TURN indisponibles :", e);
  }
  if (call !== current) return false;
  trace("serveurs ICE :", iceServers.flatMap((s) => s.urls).join(" ") || "aucun");
  const pc = new RTCPeerConnection({ iceServers });
  current.pc = pc;
  current.remoteMedia = new MediaStream();
  current.remoteStreams = new Map();
  current.local.getTracks().forEach((t) => pc.addTrack(t, current.local));
  pc.onicecandidate = (e) => {
    if (call !== current) return;
    if (!e.candidate) return trace("candidats locaux : collecte terminée");
    trace("candidat local :", describe(e.candidate.candidate));
    signal("ice", e.candidate.toJSON());
  };
  pc.onicecandidateerror = (e) => trace("erreur de candidat :", e.url, e.errorCode, e.errorText);
  pc.onicegatheringstatechange = () => trace("collecte ICE :", pc.iceGatheringState);
  pc.onsignalingstatechange = () => trace("signalisation :", pc.signalingState);
  pc.oniceconnectionstatechange = () => trace("ICE :", pc.iceConnectionState);
  pc.onnegotiationneeded = async () => {
    if (call !== current || !current.readyForReneg) return;
    try {
      current.makingOffer = true;
      await pc.setLocalDescription();
      if (call === current) signal("offer", pc.localDescription.toJSON());
    } catch (e) {
      console.warn("[appel] renégociation :", e);
    } finally {
      current.makingOffer = false;
    }
  };
  pc.ontrack = (e) => {
    if (call !== current) return;
    const track = e.track;
    const stream = e.streams[0] || new MediaStream([track]);
    const sid = stream.id || "";
    if (sid) current.remoteStreams.set(sid, stream);

    if (track.kind === "audio") {
      // Remove any stale audio track before adding the live one.
      for (const old of current.remoteMedia.getAudioTracks()) {
        if (old.id !== track.id) current.remoteMedia.removeTrack(old);
      }
      current.remoteMedia.addTrack(track);
      const rv = document.getElementById("remote-video");
      if (rv && rv.srcObject !== current.remoteMedia) rv.srcObject = current.remoteMedia;
      return;
    }

    // Video track: distinguish screen share from remote camera.
    const announcedScreen = Boolean(current.remoteScreenStreamId && sid === current.remoteScreenStreamId);
    const secondVideoStream = Boolean(
      current.remoteCameraTrack &&
      current.remoteCameraTrack.readyState === "live" &&
      current.remoteCameraTrack.id !== track.id &&
      current.remoteCameraStreamId &&
      sid !== current.remoteCameraStreamId
    );
    const audioCallScreen = Boolean(!current.video && current.remoteScreenStreamId !== null);

    if (announcedScreen || secondVideoStream || audioCallScreen) {
      trace("flux écran distant reçu :", sid);
      attachRemoteScreen(current, stream);
      track.onended = () => {
        if (call === current && current.screenRemote) {
          current.remoteScreenStreamId = null;
          attachRemoteScreen(current, null);
        }
      };
    } else {
      current.remoteCameraTrack = track;
      current.remoteCameraStreamId = sid;
      for (const old of current.remoteMedia.getVideoTracks()) {
        if (old.id !== track.id) current.remoteMedia.removeTrack(old);
      }
      current.remoteMedia.addTrack(track);
      const rv = document.getElementById("remote-video");
      if (rv && rv.srcObject !== current.remoteMedia) rv.srcObject = current.remoteMedia;
      track.onmute = () => { if (call === current) updateRemoteCamera(current); };
      track.onunmute = () => { if (call === current) updateRemoteCamera(current); };
      track.onended = () => {
        if (call === current && current.remoteCameraTrack?.id === track.id) {
          current.remoteCameraTrack = null;
          updateRemoteCamera(current);
        }
      };
      updateRemoteCamera(current);
    }
  };
  pc.onconnectionstatechange = () => { if (call === current) connectionChanged(current); };
  return true;
}

function connectionChanged(current) {
  const pc = current.pc;
  trace("connexion :", pc.connectionState);
  switch (pc.connectionState) {
    case "connected":
      clearTimeout(current.timer);
      clearTimeout(current.retry);
      current.readyForReneg = true;
      logRoute(pc);
      startSpeaking(current);
      if (current.state !== "connected") {
        current.state = "connected";
        publish();
      }
      break;
    case "disconnected":
      // Often comes back by itself (network switch, short loss).
      clearTimeout(current.retry);
      current.retry = setTimeout(() => recover(current), 4000);
      break;
    case "failed":
      clearTimeout(current.retry);
      recover(current);
      break;
  }
}

// Lost or failed connection: the caller restarts ICE (new candidates, new TURN
// allocations), and the call ends with a clear message if nothing comes back.
function recover(current) {
  const pc = current.pc;
  if (call !== current || !pc || pc.connectionState === "connected") return;
  if (current.offerer && (current.restarts || 0) < 2 && pc.signalingState === "stable") {
    current.restarts = (current.restarts || 0) + 1;
    trace("redémarrage ICE", current.restarts);
    current.makingOffer = true;
    pc.createOffer({ iceRestart: true })
      .then((offer) => (call === current ? pc.setLocalDescription(offer) : null))
      .then(() => { if (call === current) signal("offer", pc.localDescription.toJSON()); })
      .catch((e) => console.warn("[appel] redémarrage ICE impossible :", e))
      .finally(() => { current.makingOffer = false; });
  }
  clearTimeout(current.timer);
  current.timer = setTimeout(() => {
    if (call !== current || current.pc?.connectionState === "connected") return;
    stop("failed", current.state === "connected" ? "Connexion perdue." : "La connexion de l'appel a échoué.");
  }, 15000);
}

// Direct or through the TURN relay, for the console.
async function logRoute(pc) {
  try {
    const stats = await pc.getStats();
    const pair = [...stats.values()].find((s) => s.type === "candidate-pair" && s.nominated && s.state === "succeeded");
    const local = pair && stats.get(pair.localCandidateId);
    const remote = pair && stats.get(pair.remoteCandidateId);
    if (local && remote) trace("route :", local.candidateType, local.protocol, "→", remote.candidateType);
  } catch {}
}

// Ringing, then connecting: each stage has its own limit and message.
function expire(current, seconds) {
  clearTimeout(current.timer);
  current.timer = setTimeout(() => {
    if (call !== current) return;
    // A device that is only ringing must not end a call answered elsewhere.
    if (current.state === "incoming") end("Appel manqué.");
    else if (current.state === "calling") stop("", "Pas de réponse.");
    else stop("timeout", "Impossible d'établir la connexion de l'appel.");
  }, seconds * 1000);
}

async function addCandidate(current, candidate) {
  if (current.ignoreOffer) return;
  if (!current.pc?.remoteDescription) return current.queued.push(candidate);
  trace("candidat distant :", describe(candidate?.candidate));
  // One unusable candidate (old ICE generation, unknown address) must not end the call.
  try { await current.pc.addIceCandidate(candidate); } catch (e) { console.warn("[appel] candidat ignoré :", e); }
}

// `avatar` (media id of the callee's photo) and `peerId` are optional.
export async function startCall(conversation, peer, video, avatar, peerId) {
  if (call) return;
  const current = {
    id: crypto.randomUUID(), conversation, peer, video, state: "calling", queued: [],
    avatar: avatar || null, peerId: peerId || 0,
    remoteScreenStreamId: null,
  };
  call = current;
  publish();
  try {
    if (!(await getMedia(current))) return;
  } catch (e) {
    console.warn("[appel] micro ou caméra :", e);
    if (call !== current) return;
    return end("Accès au micro ou à la caméra refusé.");
  }
  signal("invite");
  expire(current, 45);
}

export async function acceptCall() {
  const current = call;
  if (!current || current.state !== "incoming") return;
  current.state = "connecting";
  publish();
  try {
    if (!(await getMedia(current))) return;
  } catch (e) {
    console.warn("[appel] micro ou caméra :", e);
    if (call !== current) return;
    signal("reject", { reason: "media" });
    return end("Accès au micro ou à la caméra refusé.");
  }
  try {
    if (!(await createPeer(current))) return;
  } catch (e) {
    console.error("[appel] connexion impossible à préparer :", e);
    if (call !== current) return;
    signal("reject", { reason: "error" });
    return end("Erreur pendant l'appel.");
  }
  signal("accept");
  expire(current, 30);
}

export function rejectCall() {
  if (!call) return;
  signal("reject");
  end("Appel refusé.");
}

export function hangup() {
  if (!call) return;
  signal("hangup");
  end("Appel terminé.");
}

export function toggleMute() {
  if (!call?.local) return;
  call.muted = !call.muted;
  call.local.getAudioTracks().forEach((t) => (t.enabled = !call.muted));
  if (call.muted) call.speakingLocal = false;
  publish();
}

export function toggleCamera() {
  if (!call?.local) return;
  const tracks = call.local.getVideoTracks();
  if (tracks.length === 0) return;
  call.cameraOff = !call.cameraOff;
  tracks.forEach((t) => (t.enabled = !call.cameraOff));
  publish();
}

export async function startScreenShare() {
  const current = call;
  if (!current || !current.pc || current.state !== "connected" || current.screenLocal) return;
  if (!canShareScreen()) throw "Partage d'écran impossible.";
  let stream;
  try {
    stream = await navigator.mediaDevices.getDisplayMedia({
      video: { frameRate: 30, width: { max: 1920 } },
      audio: false,
    });
  } catch (e) {
    // User cancelled the screen picker: stay silent.
    if (e && (e.name === "NotAllowedError" || e.name === "AbortError")) return;
    console.warn("[appel] partage d'écran :", e);
    throw "Partage d'écran impossible.";
  }
  if (call !== current || !current.pc) {
    stream.getTracks().forEach((t) => t.stop());
    return;
  }
  const track = stream.getVideoTracks()[0];
  if (!track) {
    stream.getTracks().forEach((t) => t.stop());
    throw "Partage d'écran impossible.";
  }
  try { track.contentHint = "detail"; } catch {}
  current.screenStream = stream;
  current.screenLocal = true;
  const el = document.getElementById("local-screen");
  if (el) {
    el.muted = true;
    el.srcObject = stream;
  }
  signal("screen", { on: true, stream: stream.id });
  const sender = current.pc.addTrack(track, stream);
  current.screenSender = sender;
  try {
    const params = sender.getParameters();
    if (!params.encodings || params.encodings.length === 0) params.encodings = [{}];
    params.encodings[0].maxBitrate = 1_500_000;
    await sender.setParameters(params);
  } catch {}
  track.onended = () => {
    if (call === current && current.screenLocal) stopScreenShare();
  };
  publish();
}

export function stopScreenShare() {
  const current = call;
  if (!current || !current.screenLocal) return;
  current.screenLocal = false;
  current.screenStream?.getTracks().forEach((t) => t.stop());
  current.screenStream = null;
  const el = document.getElementById("local-screen");
  if (el) el.srcObject = null;
  if (current.screenSender && current.pc) {
    try { current.pc.removeTrack(current.screenSender); } catch {}
  }
  current.screenSender = null;
  signal("screen", { on: false });
  publish();
}

// Signals are handled one at a time so an ICE candidate never overtakes its offer.
let chain = Promise.resolve();
function onSignal(from, s) {
  chain = chain.then(() => handleSignal(from, s)).catch((e) => {
    console.error("[appel] erreur sur le signal " + s.action + " :", e);
    if (call?.id === s.call_id) stop("error", "Erreur pendant l'appel.");
  });
}

async function handleSignal(from, s) {
  if (s.action !== "ice") trace("signal reçu :", s.action, from.id === myId ? "(mon autre appareil)" : "");
  // Echo of an answer given on another of this user's devices.
  if (from.id === myId) {
    if (call?.id === s.call_id && call.state === "incoming") {
      end(s.action === "accept" ? "Appel pris sur un autre appareil." : "Appel refusé sur un autre appareil.");
    }
    return;
  }
  if (s.action === "invite") {
    if (call?.id === s.call_id) return;
    if (call) {
      trace("déjà en appel, nouvel appel refusé");
      return signal("reject", { reason: "busy" }, { id: s.call_id, conversation: s.conversation_id, video: s.video });
    }
    call = {
      id: s.call_id, conversation: s.conversation_id, video: s.video, peer: from.display_name, state: "incoming", queued: [],
      avatar: from.avatar || null, peerId: from.id, remoteScreenStreamId: null,
    };
    expire(call, 45);
    notifyDesktop(from.display_name, s.video ? "Appel vidéo entrant" : "Appel entrant", true);
    return publish();
  }
  const current = call;
  if (!current || current.id !== s.call_id) return;
  const why = REASONS[s.payload?.reason];
  switch (s.action) {
    case "reject":
      return end(why || "Appel refusé.");
    case "hangup":
      if (why) return end(why);
      if (current.state === "incoming") return end("Appel manqué.");
      return end(current.state === "connected" ? "Appel terminé." : "Votre correspondant a raccroché.");
    case "accept": {
      if (current.state !== "calling") return;
      current.state = "connecting";
      current.offerer = true;
      publish();
      expire(current, 30);
      if (!(await createPeer(current))) return;
      current.makingOffer = true;
      try {
        await current.pc.setLocalDescription(await current.pc.createOffer());
        signal("offer", current.pc.localDescription.toJSON());
      } finally {
        current.makingOffer = false;
      }
      return;
    }
    case "offer": {
      if (!current.pc) return;
      const pc = current.pc;
      const polite = !current.offerer;
      const offerCollision = current.makingOffer || pc.signalingState !== "stable";
      current.ignoreOffer = !polite && offerCollision;
      if (current.ignoreOffer) {
        trace("collision d'offre ignorée (impolite)");
        return;
      }
      if (offerCollision) {
        await pc.setLocalDescription({ type: "rollback" });
      }
      await pc.setRemoteDescription(s.payload);
      for (const c of current.queued.splice(0)) await addCandidate(current, c);
      await pc.setLocalDescription(await pc.createAnswer());
      signal("answer", pc.localDescription.toJSON());
      current.readyForReneg = true;
      return;
    }
    case "answer": {
      if (!current.pc) return;
      if (current.pc.signalingState !== "have-local-offer") return;
      await current.pc.setRemoteDescription(s.payload);
      for (const c of current.queued.splice(0)) await addCandidate(current, c);
      current.readyForReneg = true;
      return;
    }
    case "screen": {
      if (s.payload?.on) {
        const sid = s.payload.stream || "";
        current.remoteScreenStreamId = sid;
        const known = sid && current.remoteStreams?.get(sid);
        if (known) {
          // If the track arrived before the announcement and was placed in remoteMedia, move it.
          for (const vt of known.getVideoTracks()) {
            if (current.remoteCameraTrack?.id === vt.id) {
              current.remoteMedia?.removeTrack(vt);
              current.remoteCameraTrack = null;
              updateRemoteCamera(current);
            }
          }
          attachRemoteScreen(current, known);
        }
      } else {
        current.remoteScreenStreamId = null;
        attachRemoteScreen(current, null);
      }
      return;
    }
    case "ice":
      return addCandidate(current, s.payload);
  }
}
