//! The conversation view and the call overlay.

use crate::{
    api, js, screens,
    ui::{self, Avatar, Icon},
    CallView, Ctx,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use proto::{Media, Message, NewMessage};
use wasm_bindgen::JsValue;

async fn post_message(ctx: Ctx, id: i64, body: String, media_id: Option<String>) -> bool {
    let path = format!("/api/conversations/{id}/messages");
    match api::post::<_, Message>(&ctx.token(), &path, &NewMessage { body, media_id }).await {
        Ok(message) => {
            ctx.push_message(message);
            true
        }
        Err(e) => {
            ctx.fail(e);
            false
        }
    }
}

/// Sends the file produced by an upload or a recording.
async fn send_media(ctx: Ctx, id: i64, uploaded: Result<JsValue, JsValue>) {
    match uploaded {
        Ok(value) => {
            if let Some(media_id) = value.as_string().filter(|s| !s.is_empty()) {
                post_message(ctx, id, String::new(), Some(media_id)).await;
            }
        }
        Err(e) => ctx.notify(e.as_string().unwrap_or_else(|| "Échec de l'envoi.".into())),
    }
}

#[component]
pub fn Chat(id: i64) -> Element {
    let mut ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let mut draft = use_signal(String::new);
    let mut recording = use_signal(|| false);
    let mut uploading = use_signal(|| false);
    let mut viewer = use_signal(|| None::<String>);
    let mut exhausted = use_signal(|| false);

    let conversation = ctx.conversations.read().iter().find(|c| c.id == id).cloned();
    let title = conversation.as_ref().map(|c| screens::title(c, me)).unwrap_or_default();
    let group = conversation.as_ref().is_some_and(|c| c.is_group);
    let subtitle = conversation
        .as_ref()
        .filter(|c| c.is_group)
        .map(|c| format!("{} membres", c.members.len()))
        .unwrap_or_default();
    let messages = ctx.messages.read().clone();

    let mut send_text = move || {
        let body = draft().trim().to_string();
        if body.is_empty() {
            return;
        }
        draft.set(String::new());
        spawn(async move {
            if !post_message(ctx, id, body.clone(), None).await {
                draft.set(body);
            }
        });
    };

    let older = move |_| {
        let Some(first) = ctx.messages.peek().first().map(|m| m.id) else { return };
        spawn(async move {
            let path = format!("/api/conversations/{id}/messages?before={first}");
            match api::get::<Vec<Message>>(&ctx.token(), &path).await {
                Ok(mut list) => {
                    exhausted.set(list.len() < 50);
                    list.extend(ctx.messages.peek().iter().cloned());
                    ctx.messages.set(list);
                }
                Err(e) => ctx.fail(e),
            }
        });
    };

    let file_chosen = move |_| {
        uploading.set(true);
        // Keeps going if the user leaves the conversation.
        spawn_forever(async move {
            send_media(ctx, id, js::upload_from_input("file").await).await;
            if let Ok(mut busy) = uploading.try_write() {
                *busy = false;
            }
        });
    };

    let record = move |_| {
        if recording() {
            recording.set(false);
            uploading.set(true);
            spawn_forever(async move {
                send_media(ctx, id, js::stop_recording().await).await;
                if let Ok(mut busy) = uploading.try_write() {
                    *busy = false;
                }
            });
        } else {
            spawn(async move {
                match js::start_recording().await {
                    Ok(started) => recording.set(started.as_bool().unwrap_or(false)),
                    Err(e) => ctx.notify(e.as_string().unwrap_or_default()),
                }
            });
        }
    };

    // Leaving the screen while recording releases the microphone.
    use_drop(js::cancel_recording);


    let mut last_day = String::new();
    rsx! {
        div { class: "screen",
            header {
                button { class: "icon", onclick: move |_| js::pop_screen(), Icon { name: "back" } }
                Avatar { name: title.clone(), small: true }
                h2 { "{title}" if !subtitle.is_empty() { small { "{subtitle}" } } }
                if !group && conversation.is_some() {
                    button { class: "icon", onclick: { let t = title.clone(); move |_| js::start_call(id as f64, &t, false) },
                        Icon { name: "phone" } }
                    button { class: "icon", onclick: { let t = title.clone(); move |_| js::start_call(id as f64, &t, true) },
                        Icon { name: "video" } }
                }
            }
            div { class: "messages", id: "messages",
                if messages.len() >= 50 && !exhausted() {
                    button { class: "link", onclick: older, "Messages précédents" }
                }
                if messages.is_empty() {
                    div { class: "empty", "Dites bonjour 👋" }
                }
                for m in messages {
                    {
                        let day = ui::day(m.created_at);
                        let new_day = day != last_day;
                        last_day = day.clone();
                        let mine = m.sender.id == me;
                        rsx! {
                            Fragment { key: "{m.id}",
                                if new_day {
                                    div { class: "day", "{day}" }
                                }
                                div { class: if mine { "bubble mine" } else { "bubble" },
                                    if group && !mine {
                                        div { class: "who", "{m.sender.display_name}" }
                                    }
                                    if let Some(media) = m.media.clone() {
                                        Attachment { media, on_view: move |url| viewer.set(Some(url)) }
                                    }
                                    if !m.body.is_empty() {
                                        div { "{m.body}" }
                                    }
                                    div { class: "time", "{ui::time(m.created_at)}" }
                                }
                            }
                        }
                    }
                }
            }
            if uploading() {
                div { class: "status", "Envoi en cours…" }
            }
            if recording() {
                div { class: "status", "Enregistrement… touchez ■ pour envoyer." }
            }
            div { class: "composer",
                input { id: "file", class: "hidden", r#type: "file", accept: "image/*,video/*,audio/*", onchange: file_chosen }
                button { class: "icon", disabled: uploading() || recording(), onclick: move |_| js::pick_file("file"),
                    Icon { name: "clip" } }
                input { class: "field", placeholder: "Message", value: "{draft}",
                    oninput: move |e| draft.set(e.value()),
                    onkeydown: move |e| if e.key() == Key::Enter { send_text() },
                }
                if draft().trim().is_empty() {
                    button { class: if recording() { "icon recording" } else { "icon" }, disabled: uploading(), onclick: record,
                        Icon { name: if recording() { "stop" } else { "mic" } } }
                } else {
                    button { class: "icon send", onclick: move |_| send_text(), Icon { name: "send" } }
                }
            }
            if let Some(url) = viewer() {
                div { class: "viewer", onclick: move |_| viewer.set(None), img { src: "{url}" } }
            }
        }
    }
}

#[component]
fn Attachment(media: Media, on_view: EventHandler<String>) -> Element {
    let url = api::media_url(&media.id);
    // Voice messages are recorded in a WebM container, always declared audio/*.
    match media.mime.split('/').next().unwrap_or("") {
        "image" => {
            let full = url.clone();
            rsx! { img { src: "{url}", alt: "{media.name}", onclick: move |_| on_view.call(full.clone()) } }
        }
        "video" => rsx! { video { src: "{url}#t=0.1", controls: true, preload: "metadata", playsinline: true } },
        _ => rsx! { audio { src: "{url}", controls: true, preload: "metadata" } },
    }
}

#[component]
pub fn CallOverlay(call: CallView) -> Element {
    let active = !call.state.is_empty() && call.state != "idle";
    let class = match (active, call.video) {
        (false, _) => "call hidden",
        (true, true) => "call",
        (true, false) => "call audio",
    };
    let status = match call.state.as_str() {
        "calling" => "Appel en cours…",
        "incoming" if call.video => "Appel vidéo entrant",
        "incoming" => "Appel entrant",
        "connecting" => "Connexion…",
        _ => "En communication",
    };
    // The video elements stay mounted so the streams can be attached at any time.
    rsx! {
        div { class,
            video { id: "remote-video", autoplay: true, playsinline: true }
            video { id: "local-video", autoplay: true, playsinline: true, muted: true }
            div { class: "who",
                h2 { "{call.peer}" }
                div { "{status}" }
            }
            div { class: "buttons",
                if call.state == "incoming" {
                    button { class: "round red", onclick: move |_| js::reject_call(), Icon { name: "hangup" } }
                    button { class: "round green", onclick: move |_| js::accept_call(),
                        Icon { name: if call.video { "video" } else { "phone" } } }
                } else {
                    button { class: if call.muted { "round on" } else { "round" }, onclick: move |_| js::toggle_mute(),
                        Icon { name: if call.muted { "micoff" } else { "mic" } } }
                    button { class: "round red", onclick: move |_| js::hangup(), Icon { name: "hangup" } }
                    if call.video {
                        button { class: if call.camera_off { "round on" } else { "round" }, onclick: move |_| js::toggle_camera(),
                            Icon { name: if call.camera_off { "videooff" } else { "video" } } }
                    }
                }
            }
        }
    }
}
