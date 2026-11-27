//! In-chat camera overlay, Snapchat style (contract 0.4.0 §7.6 and §13.5): tap the shutter
//! for a photo, hold it for a video (30 s max), then "Envoyer" or "Reprendre". The shot is
//! sent as a normal media message of the conversation, kept forever.

use crate::{chat, screens, ui::Icon, Ctx};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use futures_util::StreamExt;
use proto::NewMessage;
use wasm_bindgen::prelude::*;

/// Longest video, as for stories.
const MAX_VIDEO_MS: f64 = 30_000.0;

#[wasm_bindgen(module = "/js/camera.js")]
extern "C" {
    /// Whether the device has a camera (always true on Android).
    #[wasm_bindgen(js_name = cameraAvailable)]
    pub async fn camera_available() -> JsValue;
    #[wasm_bindgen(catch, js_name = cameraOpen)]
    async fn camera_open(video_id: &str, facing: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = cameraFlip)]
    async fn camera_flip() -> Result<JsValue, JsValue>;
    /// Tap / hold handling of the shutter; reports "photo:<url>", "rec", "video:<url>",
    /// "err:<message>". Returns a handle for `js::off_listener`.
    #[wasm_bindgen(js_name = cameraShutter)]
    fn camera_shutter(button_id: &str, max_ms: f64, callback: &JsValue) -> JsValue;
    /// Uploads the shot; resolves to the media id.
    #[wasm_bindgen(catch, js_name = cameraSend)]
    async fn camera_send(on_progress: &JsValue) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = cameraRetake)]
    fn camera_retake();
    #[wasm_bindgen(js_name = cameraClose)]
    fn camera_close();
}

#[derive(Clone, PartialEq)]
enum Phase {
    /// Waiting for the camera (permission dialog).
    Opening,
    Live,
    Recording,
    Preview { url: String, video: bool },
    Failed(String),
}

/// Full-screen camera of a conversation (`Overlay::Camera`).
#[component]
pub fn Camera(conversation_id: i64) -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let mut phase = use_signal(|| Phase::Opening);
    // Upload progress while sending (0..1).
    let mut sending = use_signal(|| None::<f64>);
    // Phones start with the back camera, computers with the front one.
    let back_first = !ctx.desktop && !*ctx.wide.peek();
    let title = ctx
        .conversations
        .read()
        .iter()
        .find(|c| c.id == conversation_id)
        .map(|c| screens::title(c, me))
        .unwrap_or_default();

    let shutter = use_hook(move || {
        let (callback, mut events) = crate::channel();
        spawn(async move {
            while let Some(event) = events.next().await {
                if event == "rec" {
                    phase.set(Phase::Recording);
                } else if let Some(url) = event.strip_prefix("photo:") {
                    phase.set(Phase::Preview { url: url.to_string(), video: false });
                } else if let Some(url) = event.strip_prefix("video:") {
                    phase.set(Phase::Preview { url: url.to_string(), video: true });
                } else if let Some(reason) = event.strip_prefix("err:") {
                    if *phase.peek() == Phase::Recording {
                        phase.set(Phase::Live);
                    }
                    ctx.notify(reason.to_string());
                }
            }
        });
        spawn(async move {
            let facing = if back_first { "environment" } else { "user" };
            match camera_open("camera-video", facing).await {
                Ok(opened) if opened.as_bool().unwrap_or(false) => phase.set(Phase::Live),
                Ok(_) => {}
                Err(e) => phase.set(Phase::Failed(e.as_string().unwrap_or_else(|| "Accès à la caméra refusé.".into()))),
            }
        });
        callback
    });
    // Bound once the shutter button exists; replaced handles are removed.
    let mut bound = use_signal(|| None::<JsValue>);
    use_drop(move || {
        if let Some(handle) = bound.peek().as_ref() {
            crate::js::off_listener(handle);
        }
        camera_close();
    });

    let send = move |_| {
        if sending.peek().is_some() {
            return;
        }
        sending.set(Some(0.0));
        let (progress, mut ticks) = crate::channel();
        spawn(async move {
            while let Some(value) = ticks.next().await {
                if let Ok(f) = value.parse::<f64>() {
                    sending.set(Some(f));
                }
            }
        });
        // Keeps going if the overlay is closed during the upload.
        spawn_forever(async move {
            match camera_send(&progress).await {
                Ok(id) => {
                    let media_id = id.as_string().filter(|s| !s.is_empty());
                    // Closed before the message is posted: the conversation shows it at once.
                    if let Ok(mut s) = sending.try_write() {
                        *s = None;
                        ctx.close_overlay();
                    }
                    if let Some(media_id) = media_id {
                        let message = NewMessage { body: String::new(), media_id: Some(media_id), reply_to: None, track_id: None };
                        chat::post_message(ctx, conversation_id, message).await;
                    }
                }
                Err(e) => {
                    if let Ok(mut s) = sending.try_write() {
                        *s = None;
                    }
                    ctx.notify(e.as_string().unwrap_or_else(|| "Échec de l'envoi.".into()));
                }
            }
        });
    };

    let retake = move |_| {
        if sending.peek().is_some() {
            return;
        }
        camera_retake();
        phase.set(Phase::Live);
    };

    let flip = move |_| {
        spawn(async move {
            if let Err(e) = camera_flip().await {
                ctx.notify(e.as_string().unwrap_or_default());
            }
        });
    };

    let current = phase();
    let live = matches!(current, Phase::Live | Phase::Recording | Phase::Opening);
    let recording = current == Phase::Recording;
    rsx! {
        div { class: if recording { "camera recording" } else { "camera" },
            // Always mounted so the stream survives the preview and "Reprendre".
            video { id: "camera-video", class: if live { "camera-live" } else { "camera-live hidden" },
                autoplay: true, playsinline: true, muted: true }
            if let Phase::Preview { url, video } = &current {
                div { class: "camera-shot",
                    if *video {
                        video { src: "{url}", autoplay: true, r#loop: true, playsinline: true }
                    } else {
                        img { src: "{url}", alt: "" }
                    }
                }
            }
            div { class: "camera-top",
                button { class: "icon camera-btn", title: "Fermer", aria_label: "Fermer la caméra",
                    onclick: move |_| ctx.close_overlay(), Icon { name: "hangup" } }
                if !title.is_empty() {
                    span { class: "camera-to", "À : {title}" }
                }
                if live && !recording && !ctx.desktop {
                    button { class: "icon camera-btn", title: "Changer de caméra", aria_label: "Changer de caméra",
                        onclick: flip, Icon { name: "flip" } }
                } else {
                    span { class: "camera-spacer" }
                }
            }
            match current {
                Phase::Failed(reason) => rsx! {
                    div { class: "camera-error",
                        Icon { name: "camera" }
                        p { "{reason}" }
                        button { class: "ghost", onclick: move |_| ctx.close_overlay(), "Fermer" }
                    }
                },
                Phase::Preview { .. } => rsx! {
                    div { class: "camera-bottom preview",
                        if let Some(p) = sending() {
                            div { class: "camera-progress",
                                span { "Envoi…" }
                                div { class: "bar", div { style: "width: {(p * 100.0).round()}%" } }
                            }
                        } else {
                            button { class: "ghost camera-retake", onclick: retake, "Reprendre" }
                            button { class: "primary camera-send", onclick: send, Icon { name: "send" } "Envoyer" }
                        }
                    }
                },
                _ => rsx! {
                    div { class: "camera-bottom",
                        button { id: "camera-shutter", class: if recording { "camera-shutter recording" } else { "camera-shutter" },
                            aria_label: "Prendre une photo ou maintenir pour filmer",
                            disabled: phase() == Phase::Opening,
                            onmounted: move |_| {
                                if let Some(old) = bound.peek().as_ref() {
                                    crate::js::off_listener(old);
                                }
                                bound.set(Some(camera_shutter("camera-shutter", MAX_VIDEO_MS, &shutter)));
                            },
                            span { class: "camera-ring" }
                        }
                        span { class: "camera-hint",
                            if recording { "Relâchez pour terminer" } else { "Touchez pour une photo, maintenez pour une vidéo" }
                        }
                    }
                },
            }
        }
    }
}
