// In-chat camera, Snapchat style (package C): live preview, tap the shutter for a photo,
// hold it for a video (30 s max, MediaRecorder), preview, then send as a normal media
// message through window.GoofyBridge.uploadBlob. Errors are French strings.
// The story camera (stories.js) has its own copy of this logic on purpose.

const HOLD_MS = 280;      // a press longer than this starts a video
const MAX_SIDE = 1920;    // photos: long side
const QUALITY = 0.85;

let stream = null;
let facing = "user";
let videoId = "";
let generation = 0;       // bumped by close/reopen, to drop late permission answers
let shot = null;          // { blob, url, name, video }
let recorder = null;      // { rec, chunks, mime, started, timer, raf, done }

function stopStream() {
  stream?.getTracks().forEach((t) => t.stop());
  stream = null;
}

async function getStream(mode) {
  if (!navigator.mediaDevices?.getUserMedia) throw "Caméra indisponible sur cet appareil.";
  const video = { facingMode: { ideal: mode }, width: { ideal: 1920 }, height: { ideal: 1080 } };
  try {
    return await navigator.mediaDevices.getUserMedia({ video, audio: true });
  } catch (e) {
    // The microphone may be refused or missing: photos and silent videos still work.
    try {
      return await navigator.mediaDevices.getUserMedia({ video, audio: false });
    } catch (e2) {
      const name = e2?.name || e?.name || "";
      if (name === "NotFoundError" || name === "OverconstrainedError") throw "Aucune caméra trouvée.";
      throw "Accès à la caméra refusé.";
    }
  }
}

async function attach() {
  const el = document.getElementById(videoId);
  if (!el) return;
  el.muted = true;
  el.srcObject = stream;
  el.dataset.facing = facing;
  try { await el.play(); } catch {}
}

export async function cameraAvailable() {
  if (window.GoofyNative) return true;
  try {
    const devices = await navigator.mediaDevices.enumerateDevices();
    return devices.some((d) => d.kind === "videoinput");
  } catch {
    return false;
  }
}

export async function cameraOpen(id, mode) {
  const mine = ++generation;
  videoId = id;
  facing = mode === "environment" ? "environment" : "user";
  stopStream();
  const s = await getStream(facing);
  if (mine !== generation) {
    s.getTracks().forEach((t) => t.stop());
    return false;
  }
  stream = s;
  await attach();
  return true;
}

export async function cameraFlip() {
  if (recorder || !stream) return;
  const mine = ++generation;
  const next = facing === "user" ? "environment" : "user";
  stopStream();
  const s = await getStream(next);
  if (mine !== generation) {
    s.getTracks().forEach((t) => t.stop());
    return;
  }
  facing = next;
  stream = s;
  await attach();
}

function setShot(blob, name, video) {
  if (shot) URL.revokeObjectURL(shot.url);
  shot = { blob, url: URL.createObjectURL(blob), name, video };
  document.getElementById(videoId)?.pause();
  return shot.url;
}

export async function cameraShoot() {
  const el = document.getElementById(videoId);
  if (!el || !el.videoWidth) throw "La caméra n'est pas prête.";
  const w = el.videoWidth, h = el.videoHeight;
  const scale = Math.min(1, MAX_SIDE / Math.max(w, h));
  const canvas = document.createElement("canvas");
  canvas.width = Math.round(w * scale);
  canvas.height = Math.round(h * scale);
  canvas.getContext("2d").drawImage(el, 0, 0, canvas.width, canvas.height);
  const blob = await new Promise((done) => canvas.toBlob(done, "image/jpeg", QUALITY));
  if (!blob) throw "La photo n'a pas pu être prise.";
  return setShot(blob, "photo.jpg", false);
}

function recordingType() {
  if (typeof MediaRecorder === "undefined") return null;
  const types = [
    "video/mp4;codecs=avc1,mp4a", "video/mp4",
    "video/webm;codecs=vp9,opus", "video/webm;codecs=vp8,opus", "video/webm",
  ];
  return types.find((t) => MediaRecorder.isTypeSupported?.(t)) ?? "";
}

// Starts a video; onTick(fraction 0..1 as a string) while it runs; stops by itself at maxMs
// and then calls onAuto(url).
export function cameraRecordStart(maxMs, onTick, onAuto) {
  if (recorder || !stream) return false;
  const type = recordingType();
  if (type === null) throw "Vidéo impossible sur cet appareil.";
  const options = { videoBitsPerSecond: 2_500_000, audioBitsPerSecond: 128_000 };
  if (type) options.mimeType = type;
  let rec;
  try { rec = new MediaRecorder(stream, options); } catch { throw "Vidéo impossible sur cet appareil."; }
  const current = { rec, chunks: [], mime: rec.mimeType || type || "video/webm", started: Date.now() };
  rec.ondataavailable = (e) => { if (e.data?.size) current.chunks.push(e.data); };
  rec.start(500);
  recorder = current;
  const tick = () => {
    if (recorder !== current) return;
    onTick?.(String(Math.min(1, (Date.now() - current.started) / maxMs)));
    current.raf = requestAnimationFrame(tick);
  };
  tick();
  current.timer = setTimeout(() => {
    if (recorder !== current) return;
    cameraRecordStop().then((url) => onAuto?.(url), () => onAuto?.(""));
  }, maxMs);
  return true;
}

export async function cameraRecordStop() {
  const current = recorder;
  if (!current) return "";
  recorder = null;
  clearTimeout(current.timer);
  cancelAnimationFrame(current.raf);
  await new Promise((done) => {
    current.rec.onstop = done;
    try { current.rec.stop(); } catch { done(); }
  });
  const mime = current.mime.split(";")[0] || "video/webm";
  const blob = new Blob(current.chunks, { type: mime });
  if (!blob.size || Date.now() - current.started < 300) throw "Vidéo trop courte.";
  return setShot(blob, mime === "video/mp4" ? "video.mp4" : "video.webm", true);
}

// Tap = photo, hold = video, release = end of the video. Reports "photo:<url>", "rec",
// "video:<url>" or "err:<message>"; sets --p (0..1) on the button while recording.
// Returns a function that removes the listeners.
export function cameraShutter(buttonId, maxMs, callback) {
  const button = document.getElementById(buttonId);
  if (!button) return () => {};
  let hold = null;
  let pressed = false;
  let busy = false;
  const fail = (e) => callback("err:" + (typeof e === "string" ? e : "Échec de la capture."));
  const finish = () => {
    button.style.setProperty("--p", "0");
    button.classList.remove("recording");
  };
  const down = (e) => {
    if (busy || shot || !stream || (e.pointerType === "mouse" && e.button !== 0)) return;
    e.preventDefault();
    pressed = true;
    try { button.setPointerCapture(e.pointerId); } catch {}
    hold = setTimeout(() => {
      hold = null;
      if (!pressed) return;
      try {
        const started = cameraRecordStart(
          maxMs,
          (p) => button.style.setProperty("--p", p),
          (url) => {
            pressed = false;
            finish();
            if (url) callback("video:" + url);
            else fail("La vidéo n'a pas pu être enregistrée.");
          },
        );
        if (started) {
          button.classList.add("recording");
          window.GoofyBridge?.vibrate(12);
          callback("rec");
        }
      } catch (err) { fail(err); }
    }, HOLD_MS);
  };
  const up = async () => {
    if (!pressed) return;
    pressed = false;
    busy = true;
    try {
      if (hold) {
        clearTimeout(hold);
        hold = null;
        callback("photo:" + await cameraShoot());
      } else if (recorder) {
        finish();
        callback("video:" + await cameraRecordStop());
      }
    } catch (err) {
      finish();
      fail(err);
    } finally {
      busy = false;
    }
  };
  const context = (e) => e.preventDefault();
  button.addEventListener("pointerdown", down);
  button.addEventListener("pointerup", up);
  button.addEventListener("pointercancel", up);
  button.addEventListener("contextmenu", context);
  return () => {
    clearTimeout(hold);
    button.removeEventListener("pointerdown", down);
    button.removeEventListener("pointerup", up);
    button.removeEventListener("pointercancel", up);
    button.removeEventListener("contextmenu", context);
  };
}

// Uploads the shot; onProgress(fraction as a string). Resolves to the media id.
export async function cameraSend(onProgress) {
  const current = shot;
  if (!current) throw "Rien à envoyer.";
  if (!window.GoofyBridge) throw "Échec de l'envoi.";
  return window.GoofyBridge.uploadBlob(current.blob, current.name, (f) => onProgress?.(String(f)));
}

// Back to the live preview.
export function cameraRetake() {
  if (shot) URL.revokeObjectURL(shot.url);
  shot = null;
  const el = document.getElementById(videoId);
  if (el && stream) el.play().catch(() => {});
}

// Releases the camera and the microphone. An upload in progress keeps its own blob.
export function cameraClose() {
  generation++;
  const current = recorder;
  recorder = null;
  if (current) {
    clearTimeout(current.timer);
    cancelAnimationFrame(current.raf);
    try { current.rec.stop(); } catch {}
  }
  stopStream();
  if (shot) URL.revokeObjectURL(shot.url);
  shot = null;
}
