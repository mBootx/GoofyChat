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
      if (event.type === "ready") outbox.splice(0).forEach(send);
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
  if (call) hangup();
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

// Ring tone for incoming calls on desktop, where no system ringer exists.
let ringing = null;
function ringer(on) {
  if (!tauri) return;
  if (!on) {
    if (ringing) { clearInterval(ringing.timer); ringing.audio.close().catch(() => {}); ringing = null; }
    return;
  }
  if (ringing) return;
  try {
    const audio = new AudioContext();
    const beep = () => {
      const t = audio.currentTime;
      [[0, 784], [0.18, 988], [0.36, 784], [0.54, 988]].forEach(([at, freq]) => {
        const osc = audio.createOscillator();
        const gain = audio.createGain();
        osc.type = "sine";
        osc.frequency.value = freq;
        gain.gain.setValueAtTime(0.0001, t + at);
        gain.gain.exponentialRampToValueAtTime(0.18, t + at + 0.02);
        gain.gain.exponentialRampToValueAtTime(0.0001, t + at + 0.16);
        osc.connect(gain).connect(audio.destination);
        osc.start(t + at);
        osc.stop(t + at + 0.17);
      });
    };
    beep();
    ringing = { audio, timer: setInterval(beep, 2200) };
  } catch {}
}

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
  const active = recorder;
  recorder = null;
  if (!active) return;
  active.stop();
  active.stream.getTracks().forEach((t) => t.stop());
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
  return true;
}

export async function stopRecording() {
  const active = recorder;
  if (!active) return "";
  recorder = null;
  await new Promise((done) => { active.onstop = done; active.stop(); });
  active.stream.getTracks().forEach((t) => t.stop());
  const type = (active.mimeType || "audio/webm").split(";")[0];
  return upload(new Blob(chunks, { type }), "message-vocal.webm");
}

// ---- calls ----

let call = null; // { id, conversation, video, peer, state, pc, local, queued, timer }
let report = () => {};

export function onCallState(callback) { report = callback; }

function publish(message = "") {
  ringer(call?.state === "incoming");
  report(JSON.stringify(call
    ? { state: call.state, video: call.video, peer: call.peer, muted: !!call.muted, cameraOff: !!call.cameraOff, message }
    : { state: "idle", video: false, peer: "", muted: false, cameraOff: false, message }));
}

function signal(action, payload = null, target = call) {
  send({
    type: "call",
    signal: { call_id: target.id, conversation_id: target.conversation, action, video: target.video, payload },
  });
}

function end(message = "") {
  if (!call) return;
  clearTimeout(call.timer);
  call.pc?.close();
  call.local?.getTracks().forEach((t) => t.stop());
  for (const id of ["local-video", "remote-video"]) {
    const el = document.getElementById(id);
    if (el) el.srcObject = null;
  }
  call = null;
  window.GoofyNative?.call(false, false);
  publish(message);
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
  preview.muted = true; // setting the attribute alone does not mute
  preview.srcObject = stream;
  // Keeps the microphone and camera alive when the app goes to the background.
  window.GoofyNative?.call(true, current.video);
  return true;
}

async function createPeer(current) {
  let iceServers = [];
  try {
    const res = await fetch(server + "/api/ice", { headers: { Authorization: "Bearer " + token } });
    iceServers = (await res.json()).map((s) => ({
      urls: s.urls, username: s.username || undefined, credential: s.credential || undefined,
    }));
  } catch {}
  if (call !== current) return false;
  const pc = new RTCPeerConnection({ iceServers });
  current.pc = pc;
  current.local.getTracks().forEach((t) => pc.addTrack(t, current.local));
  pc.onicecandidate = (e) => { if (e.candidate && call === current) signal("ice", e.candidate.toJSON()); };
  pc.ontrack = (e) => { document.getElementById("remote-video").srcObject = e.streams[0]; };
  pc.onconnectionstatechange = () => {
    if (call !== current) return;
    if (pc.connectionState === "connected") {
      clearTimeout(current.timer);
      current.state = "connected";
      publish();
    } else if (pc.connectionState === "failed") {
      signal("hangup");
      end("La connexion de l'appel a échoué.");
    }
  };
  return true;
}

function expire(current, seconds) {
  clearTimeout(current.timer);
  current.timer = setTimeout(() => {
    if (call !== current) return;
    // A device that is only ringing must not end a call answered elsewhere.
    if (current.state !== "incoming") signal("hangup");
    end("Pas de réponse.");
  }, seconds * 1000);
}

export async function startCall(conversation, peer, video) {
  if (call) return;
  const current = { id: crypto.randomUUID(), conversation, peer, video, state: "calling", queued: [] };
  call = current;
  publish();
  try {
    if (!(await getMedia(current))) return;
  } catch {
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
    if (!(await getMedia(current)) || !(await createPeer(current))) return;
  } catch {
    if (call !== current) return;
    signal("reject");
    return end("Accès au micro ou à la caméra refusé.");
  }
  signal("accept");
  expire(current, 30);
}

export function rejectCall() {
  if (!call) return;
  signal("reject");
  end();
}

export function hangup() {
  if (!call) return;
  signal("hangup");
  end();
}

export function toggleMute() {
  if (!call?.local) return;
  call.muted = !call.muted;
  call.local.getAudioTracks().forEach((t) => (t.enabled = !call.muted));
  publish();
}

export function toggleCamera() {
  if (!call?.local) return;
  call.cameraOff = !call.cameraOff;
  call.local.getVideoTracks().forEach((t) => (t.enabled = !call.cameraOff));
  publish();
}

// Signals are handled one at a time so an ICE candidate never overtakes its offer.
let chain = Promise.resolve();
function onSignal(from, s) {
  chain = chain.then(() => handleSignal(from, s)).catch(() => {
    if (call) { signal("hangup"); end("Erreur pendant l'appel."); }
  });
}

async function handleSignal(from, s) {
  // Echo of an answer given on another of this user's devices.
  if (from.id === myId) {
    if (call?.id === s.call_id && call.state === "incoming") end();
    return;
  }
  if (s.action === "invite") {
    if (call?.id === s.call_id) return;
    if (call) return signal("reject", null, { id: s.call_id, conversation: s.conversation_id, video: s.video });
    call = { id: s.call_id, conversation: s.conversation_id, video: s.video, peer: from.display_name, state: "incoming", queued: [] };
    expire(call, 45);
    notifyDesktop(from.display_name, s.video ? "Appel vidéo entrant" : "Appel entrant", true);
    return publish();
  }
  const current = call;
  if (!current || current.id !== s.call_id) return;
  switch (s.action) {
    case "reject":
      return end("Appel refusé.");
    case "hangup":
      return end("Appel terminé.");
    case "accept": {
      if (current.state !== "calling") return;
      current.state = "connecting";
      publish();
      expire(current, 30);
      if (!(await createPeer(current))) return;
      await current.pc.setLocalDescription(await current.pc.createOffer());
      return signal("offer", current.pc.localDescription.toJSON());
    }
    case "offer":
    case "answer": {
      if (!current.pc) return;
      await current.pc.setRemoteDescription(s.payload);
      for (const c of current.queued.splice(0)) await current.pc.addIceCandidate(c);
      if (s.action === "offer") {
        await current.pc.setLocalDescription(await current.pc.createAnswer());
        signal("answer", current.pc.localDescription.toJSON());
      }
      return;
    }
    case "ice":
      if (current.pc?.remoteDescription) await current.pc.addIceCandidate(s.payload);
      else current.queued.push(s.payload);
  }
}
