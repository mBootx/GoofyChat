//! The conversation view and the call overlay.

use crate::{
    api, js, screens,
    ui::{self, Avatar, Icon},
    CallView, Ctx,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use proto::{Conversation, Media, Message, NewMessage, Quote, React, REACTIONS};
use wasm_bindgen::JsValue;

async fn post_message(ctx: Ctx, id: i64, message: NewMessage) -> bool {
    let path = format!("/api/conversations/{id}/messages");
    match api::post::<_, Message>(&ctx.token(), &path, &message).await {
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
async fn send_media(ctx: Ctx, id: i64, reply_to: Option<i64>, uploaded: Result<JsValue, JsValue>) {
    match uploaded {
        Ok(value) => {
            if let Some(media_id) = value.as_string().filter(|s| !s.is_empty()) {
                let message = NewMessage { body: String::new(), media_id: Some(media_id), reply_to };
                post_message(ctx, id, message).await;
            }
        }
        Err(e) => ctx.notify(e.as_string().unwrap_or_else(|| "Échec de l'envoi.".into())),
    }
}

/// Adds the reaction, or removes it when it is already the user's.
fn toggle_reaction(ctx: Ctx, message: &Message, emoji: String) {
    let me = ctx.my_id();
    let same = message.reactions.iter().any(|r| r.user_id == me && r.emoji == emoji);
    let body = React { emoji: (!same).then_some(emoji) };
    let path = format!("/api/messages/{}/reaction", message.id);
    spawn_forever(async move {
        if let Err(e) = api::put_unit(&ctx.token(), &path, &body).await {
            ctx.fail(e);
        }
    });
}

/// Delivery state of one of the user's own messages.
fn receipt(conversation: &Conversation, me: i64, message: i64) -> String {
    let others = conversation.members.iter().filter(|u| u.id != me);
    let total = others.clone().count();
    let read = others
        .filter(|u| conversation.reads.iter().any(|r| r.user_id == u.id && r.message_id >= message))
        .count();
    match read {
        0 => "✓".into(),
        n if n == total => "✓✓".into(),
        n => format!("✓ Lu par {n}"),
    }
}

/// Reactions grouped by emoji, with their count and whether the user is in them.
fn tally(message: &Message, me: i64) -> Vec<(&'static str, usize, bool)> {
    REACTIONS
        .iter()
        .filter_map(|&emoji| {
            let given = message.reactions.iter().filter(|r| r.emoji == emoji);
            let count = given.clone().count();
            let mine = given.clone().any(|r| r.user_id == me);
            (count > 0).then_some((emoji, count, mine))
        })
        .collect()
}

fn quoted(quote: &Quote) -> String {
    ui::summary(&quote.body, quote.mime.as_deref(), quote.deleted)
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
    let mut replying = use_signal(|| None::<Message>);
    let mut selected = use_signal(|| None::<i64>);
    let mut confirm_delete = use_signal(|| false);
    let mut menu = use_signal(|| false);
    let mut last_typing = use_signal(|| 0.0);

    let conversation = ctx.conversations.read().iter().find(|c| c.id == id).cloned();
    let title = conversation.as_ref().map(|c| screens::title(c, me)).unwrap_or_default();
    let photo = conversation.as_ref().and_then(|c| screens::photo(c, me));
    let group = conversation.as_ref().is_some_and(|c| c.is_group);
    let subtitle = conversation
        .as_ref()
        .filter(|c| c.is_group)
        .map(|c| format!("{} membres", c.members.len()))
        .unwrap_or_default();
    // The other person, when this user blocked them.
    let blocked = conversation
        .as_ref()
        .filter(|c| !c.is_group)
        .and_then(|c| c.members.iter().find(|u| u.id != me))
        .filter(|u| ctx.blocked.read().iter().any(|b| b.id == u.id))
        .map(|u| u.id);
    let typing = {
        let who = ctx.typing.read();
        let names: Vec<String> = conversation
            .iter()
            .flat_map(|c| c.members.iter())
            .filter(|u| who.iter().any(|t| t.0 == u.id))
            .map(|u| u.display_name.clone())
            .collect();
        match names.len() {
            0 => String::new(),
            1 => format!("{} écrit…", names[0]),
            _ => format!("{} écrivent…", names.join(", ")),
        }
    };
    let messages = ctx.messages.read().clone();
    let chosen = selected().and_then(|s| messages.iter().find(|m| m.id == s && !m.deleted).cloned());

    let mut send_text = move || {
        let body = draft().trim().to_string();
        if body.is_empty() {
            return;
        }
        let quoted = replying();
        draft.set(String::new());
        replying.set(None);
        spawn(async move {
            let message = NewMessage { body: body.clone(), media_id: None, reply_to: quoted.as_ref().map(|m| m.id) };
            if !post_message(ctx, id, message).await {
                draft.set(body);
                replying.set(quoted);
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
        let reply_to = replying().map(|m| m.id);
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
            let reply_to = replying().map(|m| m.id);
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
        let now = js_sys::Date::now();
        if !e.value().is_empty() && now - last_typing() > 3000.0 {
            last_typing.set(now);
            js::send_typing(id as f64);
        }
    };

    // Leaving the screen while recording releases the microphone.
    use_drop(js::cancel_recording);

    let mut last_day = String::new();
    rsx! {
        div { class: "screen",
            header {
                button { class: "icon", onclick: move |_| js::pop_screen(), Icon { name: "back" } }
                Avatar { name: title.clone(), photo, small: true }
                h2 { "{title}" if !subtitle.is_empty() { small { "{subtitle}" } } }
                if !group && blocked.is_none() && conversation.is_some() {
                    button { class: "icon", onclick: { let t = title.clone(); move |_| js::start_call(id as f64, &t, false) },
                        Icon { name: "phone" } }
                    button { class: "icon", onclick: { let t = title.clone(); move |_| js::start_call(id as f64, &t, true) },
                        Icon { name: "video" } }
                }
                button { class: "icon", onclick: move |_| menu.set(true), Icon { name: "more" } }
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
                        let message_id = m.id;
                        let status = conversation
                            .as_ref()
                            .filter(|_| mine && !m.deleted)
                            .map(|c| receipt(c, me, m.id))
                            .unwrap_or_default();
                        let reactions = tally(&m, me);
                        rsx! {
                            Fragment { key: "{m.id}",
                                if new_day {
                                    div { class: "day", "{day}" }
                                }
                                div { class: if mine { "bubble mine" } else { "bubble" },
                                    onclick: move |_| { confirm_delete.set(false); selected.set(Some(message_id)); },
                                    if group && !mine {
                                        div { class: "who", "{m.sender.display_name}" }
                                    }
                                    if let Some(quote) = &m.reply {
                                        div { class: "quote",
                                            b { "{quote.sender_name}" }
                                            span { "{quoted(quote)}" }
                                        }
                                    }
                                    if m.deleted {
                                        div { class: "gone", "Message supprimé" }
                                    }
                                    if let Some(media) = m.media.clone() {
                                        // Using the player must not open the message menu.
                                        div { onclick: move |e| e.stop_propagation(),
                                            Attachment { media, on_view: move |url| viewer.set(Some(url)) }
                                        }
                                    }
                                    if !m.body.is_empty() {
                                        div { "{m.body}" }
                                    }
                                    div { class: "time", "{ui::time(m.created_at)} {status}" }
                                    if !reactions.is_empty() {
                                        div { class: "reactions",
                                            for (emoji, count, own) in reactions {
                                                button { key: "{emoji}", class: if own { "reaction own" } else { "reaction" },
                                                    onclick: {
                                                        let m = m.clone();
                                                        move |e: MouseEvent| {
                                                            e.stop_propagation();
                                                            toggle_reaction(ctx, &m, emoji.to_string());
                                                        }
                                                    },
                                                    "{emoji} {count}"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !typing.is_empty() {
                div { class: "status", "{typing}" }
            }
            if uploading() {
                div { class: "status", "Envoi en cours…" }
            }
            if recording() {
                div { class: "status", "Enregistrement… touchez ■ pour envoyer." }
            }
            if let Some(m) = replying() {
                div { class: "replying",
                    div { class: "quote",
                        b { "{m.sender.display_name}" }
                        span { {ui::summary(&m.body, m.media.as_ref().map(|f| f.mime.as_str()), false)} }
                    }
                    button { class: "icon", onclick: move |_| replying.set(None), Icon { name: "hangup" } }
                }
            }
            if let Some(user) = blocked {
                div { class: "status",
                    "Vous avez bloqué cette personne."
                    button { class: "link", onclick: move |_| set_blocked(ctx, user, false), "Débloquer" }
                }
            } else {
                div { class: "composer",
                    input { id: "file", class: "hidden", r#type: "file", accept: "image/*,video/*,audio/*", onchange: file_chosen }
                    button { class: "icon", disabled: uploading() || recording(), onclick: move |_| js::pick_file("file"),
                        Icon { name: "clip" } }
                    input { class: "field", placeholder: "Message", value: "{draft}",
                        oninput: typed,
                        onkeydown: move |e| if e.key() == Key::Enter { send_text() },
                    }
                    if draft().trim().is_empty() {
                        button { class: if recording() { "icon recording" } else { "icon" }, disabled: uploading(), onclick: record,
                            Icon { name: { if recording() { "stop" } else { "mic" } } } }
                    } else {
                        button { class: "icon send", onclick: move |_| send_text(), Icon { name: "send" } }
                    }
                }
            }
            if let Some(m) = chosen {
                div { class: "backdrop", onclick: move |_| selected.set(None),
                    div { class: "sheet", onclick: move |e| e.stop_propagation(),
                        div { class: "emojis",
                            for emoji in REACTIONS {
                                button { key: "{emoji}",
                                    onclick: {
                                        let m = m.clone();
                                        move |_| { toggle_reaction(ctx, &m, emoji.to_string()); selected.set(None); }
                                    },
                                    "{emoji}"
                                }
                            }
                        }
                        button { class: "action",
                            onclick: {
                                let m = m.clone();
                                move |_| { replying.set(Some(m.clone())); selected.set(None); }
                            },
                            "Répondre"
                        }
                        if m.sender.id == me {
                            button { class: "action danger",
                                onclick: {
                                    let path = format!("/api/messages/{}", m.id);
                                    move |_| {
                                        if !confirm_delete() {
                                            confirm_delete.set(true);
                                            return;
                                        }
                                        selected.set(None);
                                        let path = path.clone();
                                        spawn_forever(async move {
                                            if let Err(e) = api::delete(&ctx.token(), &path).await {
                                                ctx.fail(e);
                                            }
                                        });
                                    }
                                },
                                if confirm_delete() { "Confirmer la suppression" } else { "Supprimer pour tout le monde" }
                            }
                        }
                    }
                }
            }
            if menu() {
                if let Some(c) = conversation.clone() {
                    ConversationMenu { conversation: c, on_close: move |_| menu.set(false) }
                }
            }
            if let Some(url) = viewer() {
                div { class: "viewer", onclick: move |_| viewer.set(None), img { src: "{url}" } }
            }
        }
    }
}

fn set_blocked(ctx: Ctx, user: i64, block: bool) {
    spawn_forever(async move {
        let path = format!("/api/blocks/{user}");
        let result = if block {
            api::put_unit(&ctx.token(), &path, &()).await
        } else {
            api::delete(&ctx.token(), &path).await
        };
        if let Err(e) = result {
            ctx.fail(e);
        }
        ctx.load_blocked().await;
    });
}

/// Members, blocking and leaving.
#[component]
fn ConversationMenu(conversation: Conversation, on_close: EventHandler<()>) -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let id = conversation.id;
    let mut confirm_leave = use_signal(|| false);
    let blocked: Vec<i64> = ctx.blocked.read().iter().map(|u| u.id).collect();

    let leave = move |_| {
        if !confirm_leave() {
            confirm_leave.set(true);
            return;
        }
        on_close.call(());
        spawn_forever(async move {
            match api::post_empty(&ctx.token(), &format!("/api/conversations/{id}/leave")).await {
                // Drops the conversation and returns to the list.
                Ok(()) => ctx.refresh().await,
                Err(e) => ctx.fail(e),
            }
        });
    };

    rsx! {
        div { class: "backdrop", onclick: move |_| on_close.call(()),
            div { class: "sheet", onclick: move |e| e.stop_propagation(),
                for u in conversation.members.iter().filter(|u| u.id != me).cloned() {
                    {
                        let is_blocked = blocked.contains(&u.id);
                        let user = u.id;
                        rsx! {
                            div { key: "{u.id}", class: "person",
                                Avatar { name: u.display_name.clone(), photo: u.avatar.clone(), small: true }
                                span { "{u.display_name}" }
                                button { class: "link", onclick: move |_| set_blocked(ctx, user, !is_blocked),
                                    if is_blocked { "Débloquer" } else { "Bloquer" }
                                }
                            }
                        }
                    }
                }
                if conversation.is_group {
                    button { class: "action danger", onclick: leave,
                        if confirm_leave() { "Confirmer : quitter le groupe" } else { "Quitter le groupe" }
                    }
                }
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
                        Icon { name: { if call.video { "video" } else { "phone" } } } }
                } else {
                    button { class: if call.muted { "round on" } else { "round" }, onclick: move |_| js::toggle_mute(),
                        Icon { name: { if call.muted { "micoff" } else { "mic" } } } }
                    button { class: "round red", onclick: move |_| js::hangup(), Icon { name: "hangup" } }
                    if call.video {
                        button { class: if call.camera_off { "round on" } else { "round" }, onclick: move |_| js::toggle_camera(),
                            Icon { name: { if call.camera_off { "videooff" } else { "video" } } } }
                    }
                }
            }
        }
    }
}
