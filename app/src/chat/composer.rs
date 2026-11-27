//! The input bar of a conversation: text (with edit mode and @mention suggestions),
//! attachments, camera, voice messages; and the bar shown instead in a locked
//! conversation (contract 0.4.0 §7.6).

use super::post_message;
use crate::{
    api, js,
    photos::Panel,
    ui::{self, Icon, Name, UserAvatar},
    Ctx, Overlay,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use proto::{EditMessage, FriendRequestOutcome, FriendRequestStatus, Message, NewFriendRequest, NewMessage, User};
use wasm_bindgen::JsValue;

/// The "@query" being typed at the end of the text: byte index of the "@" and the query in
/// lower case.
fn mention_query(text: &str) -> Option<(usize, String)> {
    let at = text.rfind('@')?;
    let before = text[..at].chars().last();
    if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '@') {
        return None;
    }
    let query = &text[at + 1..];
    if query.len() > 24 || !query.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
        return None;
    }
    Some((at, query.to_lowercase()))
}

/// Sends the file produced by an upload or a recording.
async fn send_media(ctx: Ctx, id: i64, reply_to: Option<i64>, uploaded: Result<JsValue, JsValue>) {
    match uploaded {
        Ok(value) => {
            if let Some(media_id) = value.as_string().filter(|s| !s.is_empty()) {
                let message = NewMessage { body: String::new(), media_id: Some(media_id), reply_to, track_id: None };
                post_message(ctx, id, message).await;
            }
        }
        Err(e) => ctx.notify(e.as_string().unwrap_or_else(|| "Échec de l'envoi.".into())),
    }
}

#[component]
pub fn Composer(
    id: i64,
    draft: Signal<String>,
    replying: Signal<Option<Message>>,
    editing: Signal<Option<Message>>,
    recording: Signal<bool>,
    uploading: Signal<bool>,
    panel: Signal<Option<Panel>>,
    /// Other members, offered after "@" (groups only; empty otherwise).
    members: Vec<User>,
) -> Element {
    let ctx = use_context::<Ctx>();
    let mut last_typing = use_signal(|| 0.0);
    let mut camera = use_signal(|| false);
    // Enter sends with a real keyboard; on phones it makes a new line.
    let enter = use_hook(super::enter_sends);
    use_hook(move || {
        spawn(async move {
            let found = crate::camera::camera_available().await.as_bool().unwrap_or(false);
            camera.set(found);
        })
    });
    // Also after a programmatic change (sent, edit started, mention picked).
    use_effect(move || {
        let _ = draft.read();
        super::autosize("composer");
    });

    let mut send = move || {
        let body = draft.peek().trim().to_string();
        let current_edit = editing.peek().clone();
        if let Some(original) = current_edit {
            if body.is_empty() && original.media.is_none() {
                ctx.notify("Message vide.");
                return;
            }
            editing.set(None);
            draft.set(String::new());
            if body == original.body {
                return;
            }
            spawn(async move {
                let path = format!("/api/messages/{}", original.id);
                match api::put::<_, Message>(&ctx.token(), &path, &EditMessage { body: body.clone() }).await {
                    Ok(updated) => {
                        ctx.update_message(updated);
                        // The list preview may show this message.
                        ctx.refresh().await;
                    }
                    Err(e) => {
                        ctx.fail(e);
                        editing.set(Some(original));
                        draft.set(body);
                    }
                }
            });
            return;
        }
        if body.is_empty() {
            return;
        }
        let quoted = replying.peek().clone();
        draft.set(String::new());
        replying.set(None);
        spawn(async move {
            let message = NewMessage { body: body.clone(), media_id: None, reply_to: quoted.as_ref().map(|m| m.id), track_id: None };
            if !post_message(ctx, id, message).await {
                draft.set(body);
                replying.set(quoted);
            }
        });
    };

    let file_chosen = move |_| {
        uploading.set(true);
        let reply_to = replying.peek().as_ref().map(|m| m.id);
        replying.set(None);
        // Keeps going if the user leaves the conversation.
        spawn_forever(async move {
            send_media(ctx, id, reply_to, js::upload_from_input("file").await).await;
            if let Ok(mut busy) = uploading.try_write() {
                *busy = false;
            }
        });
    };

    let record = move |_| {
        if recording() {
            recording.set(false);
            uploading.set(true);
            let reply_to = replying.peek().as_ref().map(|m| m.id);
            replying.set(None);
            spawn_forever(async move {
                send_media(ctx, id, reply_to, js::stop_recording().await).await;
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

    let typed = move |e: FormEvent| {
        draft.set(e.value());
        super::autosize("composer");
        let now = js_sys::Date::now();
        if editing.peek().is_none() && !e.value().is_empty() && now - last_typing() > 3000.0 {
            last_typing.set(now);
            js::send_typing(id as f64);
        }
    };

    let text = draft.read().clone();
    let has_text = !text.trim().is_empty();
    let is_editing = editing.read().is_some();
    let wide = *ctx.wide.read();
    // Phones: the extra buttons make room for the text while typing.
    let extras = wide || !has_text;
    let suggestions: Vec<User> = match mention_query(&text) {
        Some((_, q)) if !members.is_empty() => members
            .iter()
            .filter(|u| u.username.to_lowercase().contains(&q) || u.display_name.to_lowercase().contains(&q))
            .take(6)
            .cloned()
            .collect(),
        _ => vec![],
    };

    rsx! {
        if !suggestions.is_empty() {
            div { class: "mention-list", role: "listbox",
                for u in suggestions {
                    button { key: "{u.id}", class: "mention-option", role: "option",
                        onclick: {
                            let username = u.username.clone();
                            move |_| {
                                let text = draft.peek().clone();
                                if let Some((at, _)) = mention_query(&text) {
                                    draft.set(format!("{}@{username} ", &text[..at]));
                                }
                                js::focus_input("composer");
                            }
                        },
                        UserAvatar { user: u.clone(), small: true }
                        span { class: "mention-names",
                            Name { user: u.clone() }
                            small { "@{u.username}" }
                        }
                    }
                }
            }
        }
        if recording() {
            // Covers the composer (hidden by CSS) while the microphone is on.
            div { class: "recbar",
                button { class: "icon rec-cancel", title: "Annuler l'enregistrement",
                    onclick: move |_| { js::cancel_recording(); recording.set(false); },
                    Icon { name: "trash" } }
                span { class: "rec-dot" }
                span { class: "rec-time", id: "rec-time" }
                canvas { class: "rec-level", id: "rec-level",
                    onmounted: move |_| js::record_meter("rec-level", "rec-time") }
                button { class: "icon rec-send", title: "Envoyer le message vocal", onclick: record,
                    Icon { name: "send" } }
            }
        }
        if let Some(m) = editing() {
            div { class: "replying editing-bar",
                Icon { name: "edit" }
                div { class: "quote",
                    b { "Modification du message" }
                    span { {ui::message_summary(&m)} }
                }
                button { class: "icon", title: "Annuler la modification",
                    onclick: move |_| { editing.set(None); draft.set(String::new()); },
                    Icon { name: "hangup" } }
            }
        } else if let Some(m) = replying() {
            div { class: "replying",
                div { class: "quote",
                    b { "{m.sender.display_name}" }
                    span { {ui::message_summary(&m)} }
                }
                button { class: "icon", title: "Annuler la réponse", onclick: move |_| replying.set(None), Icon { name: "hangup" } }
            }
        }
        div { class: "composer",
            input { id: "file", class: "hidden", r#type: "file", accept: "image/*,video/*,audio/*", onchange: file_chosen }
            if !is_editing {
                button { class: "icon", title: "Joindre", disabled: uploading() || recording(),
                    onclick: move |_| panel.set(Some(Panel::Attach)),
                    Icon { name: "clip" } }
                if extras {
                    if camera() {
                        button { class: "icon", title: "Caméra", disabled: recording(),
                            onclick: move |_| ctx.open_overlay(Overlay::Camera { conversation_id: id }),
                            Icon { name: "camera" } }
                    }
                    button { class: "icon", title: "Partager un morceau", disabled: recording(),
                        onclick: move |_| panel.set(Some(Panel::Music)),
                        Icon { name: "music" } }
                }
            }
            textarea { id: "composer", class: "field", rows: "1", value: "{text}",
                placeholder: if is_editing { "Modifier le message" } else { "Message" },
                aria_label: "Message",
                oninput: typed,
                onkeydown: move |e: KeyboardEvent| {
                    if e.key() == Key::Enter && enter && !e.modifiers().shift() && !e.is_composing() {
                        e.prevent_default();
                        send();
                    }
                },
            }
            if is_editing {
                button { class: "icon send", title: "Enregistrer la modification", onclick: move |_| send(), Icon { name: "check" } }
            } else if has_text {
                button { class: "icon send", title: "Envoyer", onclick: move |_| send(), Icon { name: "send" } }
            } else {
                button { class: if recording() { "icon recording" } else { "icon" }, title: "Message vocal", disabled: uploading(), onclick: record,
                    Icon { name: ui::pick(recording(), "stop", "mic") } }
            }
        }
    }
}

/// Instead of the composer in a locked one-to-one conversation (no longer friends).
#[component]
pub fn LockedBar(peer: Option<User>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut busy = use_signal(|| false);
    let mut sent = use_signal(|| false);
    let Some(peer) = peer else {
        return rsx! { div { class: "locked-bar", Icon { name: "lock" } span { "Vous n'êtes plus amis." } } };
    };
    let (incoming, outgoing) = {
        let requests = ctx.requests.read();
        (
            requests.incoming.iter().any(|r| r.user.id == peer.id),
            requests.outgoing.iter().any(|r| r.user.id == peer.id),
        )
    };
    let outgoing = outgoing || sent();
    let peer_id = peer.id;
    let username = peer.username.clone();

    let add = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        let body = NewFriendRequest { username: username.clone() };
        spawn(async move {
            match api::post::<_, FriendRequestOutcome>(&ctx.token(), "/api/friends/requests", &body).await {
                Ok(outcome) => {
                    if outcome.status == FriendRequestStatus::Sent {
                        sent.set(true);
                    }
                    crate::friends::load(ctx).await;
                    ctx.refresh().await;
                }
                Err(e) => ctx.fail(e),
            }
            busy.set(false);
        });
    };
    let accept = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            let path = format!("/api/friends/requests/{peer_id}/accept");
            match api::post::<_, proto::Friend>(&ctx.token(), &path, &()).await {
                Ok(_) => {
                    crate::friends::load(ctx).await;
                    ctx.refresh().await;
                }
                Err(e) => ctx.fail(e),
            }
            busy.set(false);
        });
    };

    rsx! {
        div { class: "locked-bar",
            Icon { name: "lock" }
            span { "Vous n'êtes plus amis." }
            if incoming {
                button { class: "ghost", disabled: busy(), onclick: accept, "Accepter sa demande" }
            } else if outgoing {
                button { class: "ghost", disabled: true, "Demande envoyée" }
            } else {
                button { class: "ghost", disabled: busy(), onclick: add, "Ajouter en ami" }
            }
        }
    }
}
