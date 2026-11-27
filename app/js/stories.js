// Stories (package D): camera capture, gallery pick, photo normalisation, upload, and the
// viewer's video controls. The in-chat camera (camera.js) has its own copy of the camera
// code on purpose (parallel work, contract 0.4.0 §7.9). Uploads go through
// window.GoofyBridge.uploadBlob (bridge.js). Errors are French strings.

const MAX_SIDE = 1920;            // proto::limits::STORY_PHOTO_SIDE
const JPEG_QUALITY = 0.85;
const MAX_VIDEO_BYTES = 60 * 1024 * 1024; // proto::limits::STORY_VIDEO_BYTES
const MAX_VIDEO_MS = 30000;       // proto::limits::STORY_VIDEO_MS
const MIN_RECORD_MS = 700;        // shorter holds become a photo
const VIDEO_TYPES = ["video/mp4", "video/webm", "video/quicktime"];

let stream = null;     // live camera (and microphone when allowed)
let preview = null;    // <video> showing the camera
let facing = "environment";
let opening = 0;       // bumped on close so a late getUserMedia is dropped
let pending = null;    // { blob, name, url, video }
let recorder = null;
let chunks = [];
let recordDone = null;
let recordStart = 0;
let tickTimer = null;
let stopTimer = null;

function mobile() { return /Android|iPhone|iPad|iPod|Mobile/i.test(navigator.userAgent); }

function stopStream() {
  if (stream) stream.getTracks().forEach((t) => t.stop());
  stream = null;
  if (preview) preview.srcObject = null;
}

function cameraError(e) {
  switch (e && e.name) {
    case "NotAllowedError":
    case "SecurityError":
      return "Accès à la caméra refusé.";
    case "NotFoundError":
    case "OverconstrainedError":
      return "Aucune caméra trouvée.";
    case "NotReadableError":
      return "La caméra est utilisée par une autre application.";
    default:
      return "Caméra indisponible.";
  }
}

async function getStream(mode) {
  const video = { facingMode: { ideal: mode }, width: { ideal: 1920 }, height: { ideal: 1080 } };
  try {
    // The microphone is for held-shutter videos; photos work without it.
    return await navigator.mediaDevices.getUserMedia({ video, audio: true });
  } catch (first) {
    try {
      return await navigator.mediaDevices.getUserMedia({ video, audio: false });
    } catch {
      throw first;
    }
  }
}

// Starts the camera in the <video id=videoId>. facing: "user", "environment" or "" (back
// camera on phones, front camera elsewhere). Resolves to the facing in use ("" when the
// composer was closed meanwhile).
export async function storyCameraOpen(videoId, mode) {
  const ticket = ++opening;
  if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) throw "Caméra indisponible.";
  facing = mode || (mobile() ? "environment" : "user");
  stopStream();
  let s;
  try {
    s = await getStream(facing);
  } catch (e) {
    throw cameraError(e);
  }
  const el = document.getElementById(videoId);
  if (ticket !== opening || !el) {
    s.getTracks().forEach((t) => t.stop());
    return "";
  }
  stream = s;
  preview = el;
  el.muted = true;
  el.playsInline = true;
  el.srcObject = s;
  try { await el.play(); } catch {}
  return facing;
}

// Switches between the front and back cameras. Resolves to the new facing.
export async function storyFlip() {
  if (!preview || recorder) return facing;
  const id = preview.id;
  return storyCameraOpen(id, facing === "user" ? "environment" : "user");
}

function setPending(blob, name, video) {
  storyDiscard();
  pending = { blob, name, video, url: URL.createObjectURL(blob) };
  return pending.url;
}

function toJpeg(canvas) {
  return new Promise((resolve, reject) =>
    canvas.toBlob((b) => (b ? resolve(b) : reject("Image illisible.")), "image/jpeg", JPEG_QUALITY));
}

function fit(w, h) {
  const scale = Math.min(1, MAX_SIDE / Math.max(w, h));
  return [Math.max(1, Math.round(w * scale)), Math.max(1, Math.round(h * scale))];
}

// Takes a photo from the camera. Resolves to an object URL of the JPEG preview.
export async function storyShoot() {
  if (!preview || !preview.videoWidth) throw "La caméra n'est pas encore prête.";
  const [w, h] = fit(preview.videoWidth, preview.videoHeight);
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const g = canvas.getContext("2d");
  if (facing === "user") {
    // Keep what the user saw: the front camera preview is mirrored.
    g.translate(w, 0);
    g.scale(-1, 1);
  }
  g.drawImage(preview, 0, 0, w, h);
  return setPending(await toJpeg(canvas), "story.jpg", false);
}

function recorderType() {
  if (typeof MediaRecorder === "undefined") return null;
  const choices = ["video/mp4;codecs=avc1,mp4a", "video/mp4", "video/webm;codecs=vp9,opus", "video/webm;codecs=vp8,opus", "video/webm"];
  return choices.find((t) => MediaRecorder.isTypeSupported && MediaRecorder.isTypeSupported(t)) || "";
}

function clearTimers() {
  clearInterval(tickTimer);
  clearTimeout(stopTimer);
  tickTimer = stopTimer = null;
}

// Starts recording a video (held shutter). onTick receives the elapsed milliseconds as a
// string, then "end" when the recording stopped by itself at maxMs.
export function storyRecordStart(maxMs, onTick) {
  if (!stream || recorder) throw "La caméra n'est pas encore prête.";
  const type = recorderType();
  if (type === null) throw "L'enregistrement vidéo n'est pas pris en charge ici.";
  const options = { videoBitsPerSecond: 2500000 };
  if (type) options.mimeType = type;
  let rec;
  try {
    rec = new MediaRecorder(stream, options);
  } catch {
    throw "L'enregistrement vidéo n'est pas pris en charge ici.";
  }
  recorder = rec;
  chunks = [];
  rec.ondataavailable = (e) => { if (e.data && e.data.size) chunks.push(e.data); };
  recordDone = new Promise((resolve) => { rec.onstop = () => resolve(); });
  rec.start(250);
  recordStart = Date.now();
  const limit = Math.min(maxMs || MAX_VIDEO_MS, MAX_VIDEO_MS);
  tickTimer = setInterval(() => onTick && onTick(String(Date.now() - recordStart)), 100);
  stopTimer = setTimeout(() => {
    clearTimers();
    if (rec.state !== "inactive") rec.stop();
    if (onTick) onTick("end");
  }, limit);
}

// Ends the recording. Resolves to an object URL of the video, or "" when it was too short
// to be a video (the caller takes a photo instead).
export async function storyRecordStop() {
  const rec = recorder;
  if (!rec) throw "Aucun enregistrement en cours.";
  clearTimers();
  const elapsed = Date.now() - recordStart;
  if (rec.state !== "inactive") rec.stop();
  await recordDone;
  recorder = null;
  const parts = chunks;
  chunks = [];
  if (elapsed < MIN_RECORD_MS || !parts.length) return "";
  // A plain type (no codecs) so the server recognises it.
  const base = (rec.mimeType || parts[0].type || "video/webm").split(";")[0];
  const mime = base === "video/mp4" ? "video/mp4" : "video/webm";
  const blob = new Blob(parts, { type: mime });
  if (blob.size > MAX_VIDEO_BYTES) throw "Vidéo trop lourde (60 Mo max).";
  return setPending(blob, mime === "video/mp4" ? "story.mp4" : "story.webm", true);
}

function videoDuration(file) {
  return new Promise((resolve) => {
    const v = document.createElement("video");
    const url = URL.createObjectURL(file);
    let done = false;
    const finish = (ms) => {
      if (done) return;
      done = true;
      URL.revokeObjectURL(url);
      v.removeAttribute("src");
      resolve(ms);
    };
    v.preload = "metadata";
    v.muted = true;
    v.onloadedmetadata = () => finish(isFinite(v.duration) ? v.duration * 1000 : NaN);
    v.onerror = () => finish(NaN);
    setTimeout(() => finish(NaN), 10000);
    v.src = url;
  });
}

async function decodeImage(file) {
  if (typeof createImageBitmap === "function") {
    try {
      // Applies the EXIF orientation.
      return await createImageBitmap(file, { imageOrientation: "from-image" });
    } catch {}
  }
  return new Promise((resolve, reject) => {
    const img = new Image();
    const url = URL.createObjectURL(file);
    img.onload = () => { URL.revokeObjectURL(url); resolve(img); };
    img.onerror = () => { URL.revokeObjectURL(url); reject("Image illisible."); };
    img.src = url;
  });
}

async function normalisePhoto(file) {
  const image = await decodeImage(file);
  const [w, h] = fit(image.width || image.naturalWidth, image.height || image.naturalHeight);
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  canvas.getContext("2d").drawImage(image, 0, 0, w, h);
  if (image.close) image.close();
  return toJpeg(canvas);
}

// The file chosen in <input type=file id=inputId>. Photos are re-encoded (JPEG, long side
// ≤ 1920, EXIF dropped); videos are checked (type, 60 MB, 30 s). Resolves to an object URL
// of the preview, or "" when nothing was chosen.
export async function storyPick(inputId) {
  const input = document.getElementById(inputId);
  const file = input && input.files && input.files[0];
  if (input) input.value = "";
  if (!file) return "";
  let type = (file.type || "").toLowerCase();
  if (!type && file.name) {
    const ext = file.name.split(".").pop().toLowerCase();
    if (ext === "mp4" || ext === "m4v") type = "video/mp4";
    else if (ext === "webm") type = "video/webm";
    else if (ext === "mov") type = "video/quicktime";
  }
  if (type.startsWith("video/")) {
    if (!VIDEO_TYPES.includes(type)) throw "Format vidéo non pris en charge (MP4, WebM ou MOV).";
    if (file.size > MAX_VIDEO_BYTES) throw "Vidéo trop lourde (60 Mo max).";
    const ms = await videoDuration(file);
    // Unreadable metadata: the server's worker checks the length.
    if (ms > MAX_VIDEO_MS + 500) throw "Vidéo trop longue (30 s max).";
    const ext = type === "video/webm" ? "webm" : type === "video/quicktime" ? "mov" : "mp4";
    return setPending(file, "story." + ext, true);
  }
  if (type.startsWith("image/") || !type) {
    return setPending(await normalisePhoto(file), "story.jpg", false);
  }
  throw "Choisissez une photo ou une vidéo.";
}

// The prepared file is a video.
export function storyPendingVideo() { return !!(pending && pending.video); }

// Uploads the prepared file. onProgress receives the fraction sent (0..1) as a string.
// Resolves to the media id.
export async function storyPublish(onProgress) {
  if (!pending) throw "Rien à publier.";
  const bridge = window.GoofyBridge;
  if (!bridge || !bridge.uploadBlob) throw "Échec de l'envoi.";
  return bridge.uploadBlob(pending.blob, pending.name, (f) => { if (onProgress) onProgress(String(f)); });
}

// Forgets the prepared photo or video (retake). The camera keeps running.
export function storyDiscard() {
  if (pending) URL.revokeObjectURL(pending.url);
  pending = null;
}

// Stops everything: recording, camera, prepared file.
export function storyCameraClose() {
  opening++;
  clearTimers();
  if (recorder) {
    try { if (recorder.state !== "inactive") recorder.stop(); } catch {}
    recorder = null;
  }
  chunks = [];
  storyDiscard();
  stopStream();
  preview = null;
}

// ---- viewer ----

// Plays a story video with sound; falls back to muted when the browser refuses sound.
export function storyVideoStart(id) {
  const v = document.getElementById(id);
  if (!v) return;
  v.muted = false;
  const p = v.play();
  if (p && p.catch) {
    p.catch(() => {
      v.muted = true;
      v.play().catch(() => {});
    });
  }
}

// Pauses (hold to pause, menus) or resumes the story video.
export function storyVideoPaused(id, paused) {
  const v = document.getElementById(id);
  if (!v) return;
  if (paused) v.pause();
  else if (v.paused && !v.ended) v.play().catch(() => {});
}

// Short haptic feedback via the shared bridge (no-op where unsupported).
export function storyVibrate(ms) {
  window.GoofyBridge?.vibrate?.(ms);
}
