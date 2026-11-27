//! The conversation view (package C, contract 0.4.0 §7.6).
//! Bubbles and the long-press menu live in `chat/bubble.rs`, the input bar in
//! `chat/composer.rs`.

mod bubble;
mod composer;

use crate::{
    api, js,
    photos::Panel,
    screens,
    ui::{self, Avatar, Icon, Name, UserAvatar},
    Ctx, Screen,
};
use bubble::{Bubble, MenuAction, MessageMenu};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use futures_util::StreamExt;
use proto::{Conversation, Message, NewMessage, LIKE};
use wasm_bindgen::prelude::*;

/// Bindings of `js/gestures.js`.
#[wasm_bindgen(module = "/js/gestures.js")]
extern "C" {
    /// Long press / right click, double tap and photo taps on `[data-msg]` elements inside
    /// the container. Returns a handle for `js::off_listener`.
    #[wasm_bindgen(js_name = attachGestures)]
    fn attach_gestures(container_id: &str, callback: &JsValue) -> JsValue;
    /// Grows the composer up to 5 lines.
    #[wasm_bindgen(js_name = autosize)]
    fn autosize(textarea_id: &str);
    /// Enter sends (real keyboard) rather than inserting a new line.
    #[wasm_bindgen(js_name = enterSends)]
    fn enter_sends() -> bool;
    #[wasm_bindgen(js_name = placeMenu)]
    fn place_menu(menu_id: &str, x: f64, y: f64);
    #[wasm_bindgen(catch, js_name = copyText)]
    async fn copy_text(text: &str) -> Result<JsValue, JsValue>;
}

pub(crate) async fn post_message(ctx: Ctx, id: i64, message: NewMessage) -> bool {
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

/// Desktop notification for a message; the shell skips it while the window has the focus.
/// A message mentioning the user is announced even then (as a toast inside the window),
/// unless its conversation is open and active.
pub fn announce(ctx: Ctx, message: &Message) {
    let group = ctx
        .conversations
        .peek()
        .iter()
        .find(|c| c.id == message.conversation_id && c.is_group)
        .map(|c| c.name.clone());
    let body = ui::message_summary(message);
    if message.mentions.contains(&ctx.my_id()) {
        let open = *ctx.screen.peek() == Screen::Chat(message.conversation_id);
        if open && js::is_active() {
            return;
        }
        let mut title = format!("{} vous a mentionné", message.sender.display_name);
        if let Some(name) = group {
            title.push_str(&format!(" · {name}"));
        }
        if js::is_active() {
            ctx.notify(format!("{title} : {body}"));
        } else {
            js::notify_desktop(&title, &body, false);
        }
        return;
    }
    let title = match group {
        Some(name) => format!("{} · {name}", message.sender.display_name),
        None => message.sender.display_name.clone(),
    };
    js::notify_desktop(&title, &body, false);
}

/// "Vu" / "Envoyé" / "Vu par n" line of a conversation row, shown instead of the time.
/// None: show the time (last message not mine, deleted, or a locked conversation).
pub fn row_status(conversation: &Conversation, me: i64) -> Option<String> {
    if conversation.locked {
        return None;
    }
    let last = conversation.last_message.as_ref().filter(|m| m.sender.id == me && !m.deleted)?;
    let seen = conversation
        .members
        .iter()
        .filter(|u| u.id != me)
        .filter(|u| conversation.reads.iter().any(|r| r.user_id == u.id && r.message_id >= last.id))
        .count();
    if conversation.is_group {
        (seen >= 1).then(|| format!("Vu par {seen}"))
    } else {
        Some(if seen >= 1 { "Vu" } else { "Envoyé" }.to_string())
    }
}

/// Where the long-press menu is open: message id and press point.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Pressed {
    id: i64,
    x: f64,
    y: f64,
}

#[component]
pub fn Chat(id: i64) -> Element {
    let mut ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let mut draft = use_signal(String::new);
    let recording = use_signal(|| false);
    let uploading = use_signal(|| false);
    let mut viewer = use_signal(|| None::<String>);
    let mut exhausted = use_signal(|| false);
    let mut replying = use_signal(|| None::<Message>);
    let mut editing = use_signal(|| None::<Message>);
    let mut pressed = use_signal(|| None::<Pressed>);
    // Double-tap heart: message id and a counter that replays the animation.
    let mut heart = use_signal(|| None::<(i64, u32)>);
    let mut menu = use_signal(|| false);
    // Attach menu, photo picker or music sheet.
    let mut panel = use_signal(|| None::<Panel>);

    let conversation = ctx.conversations.read().iter().find(|c| c.id == id).cloned();
    let title = conversation.as_ref().map(|c| screens::title(c, me)).unwrap_or_default();
    let group = conversation.as_ref().is_some_and(|c| c.is_group);
    let locked = conversation.as_ref().is_some_and(|c| c.locked);
    // The other person of a one-to-one conversation.
    let peer = conversation
        .as_ref()
        .filter(|c| !c.is_group)
        .and_then(|c| c.members.iter().find(|u| u.id != me).cloned());
    let subtitle = conversation
        .as_ref()
        .filter(|c| c.is_group)
        .map(|c| format!("{} membres", c.members.len()))
        .or_else(|| {
            let track = crate::activity::peer_track(ctx, conversation.as_ref()?, me)?;
            Some(format!("🎵 écoute {} — {}", track.title, track.artist))
        })
        .unwrap_or_default();
    // The other person, when this user blocked them.
    let blocked = peer.as_ref().filter(|u| ctx.blocked.read().iter().any(|b| b.id == u.id)).map(|u| u.id);
    // Usernames for mention highlighting; other members for "@" suggestions (groups).
    let members: Vec<(String, i64)> = conversation
        .iter()
        .flat_map(|c| c.members.iter())
        .map(|u| (u.username.clone(), u.id))
        .collect();
    let suggest: Vec<proto::User> = conversation
        .iter()
        .filter(|c| c.is_group)
        .flat_map(|c| c.members.iter())
        .filter(|u| u.id != me)
        .cloned()
        .collect();
    let (typing, typing_names) = {
        let who = ctx.typing.read();
        let names: Vec<String> = conversation
            .iter()
            .flat_map(|c| c.members.iter())
            .filter(|u| u.id != me && who.iter().any(|t| t.0 == u.id))
            .map(|u| u.display_name.clone())
            .collect();
        let label = match names.len() {
            0 => String::new(),
            1 => format!("{} écrit…", names[0]),
            _ => format!("{} écrivent…", names.join(", ")),
        };
        (!names.is_empty(), label)
    };
    let messages = ctx.messages.read().clone();
    let menu_for = pressed().and_then(|p| messages.iter().find(|m| m.id == p.id && !m.deleted).cloned().map(|m| (m, p)));

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

    let act = move |action: MenuAction| {
        pressed.set(None);
        match action {
            MenuAction::Reply(m) => {
                editing.set(None);
                replying.set(Some(m));
            }
            MenuAction::Mention(m) => {
                editing.set(None);
                let prefix = format!("@{} ", m.sender.username);
                let current = draft.peek().clone();
                if !current.starts_with(&prefix) {
                    draft.set(format!("{prefix}{current}"));
                }
                replying.set(Some(m));
            }
            MenuAction::Edit(m) => {
                replying.set(None);
                draft.set(m.body.clone());
                editing.set(Some(m));
            }
        }
        js::focus_input("composer");
    };

    // Keyboard and desktop extras: Escape closes the topmost layer (and, in two
    // panes, the conversation); dropped or pasted files are sent; message gestures.
    let listeners = use_hook(move || {
        let (callback, mut keys) = crate::channel();
        let escape = js::on_escape(&callback);
        spawn(async move {
            while keys.next().await.is_some() {
                // An overlay (camera) is closed by the App first.
                if ctx.overlay.peek().is_some() {
                    continue;
                }
                if viewer.peek().is_some() {
                    viewer.set(None);
                } else if pressed.peek().is_some() {
                    pressed.set(None);
                } else if *menu.peek() {
                    menu.set(false);
                } else if panel.peek().is_some() {
                    panel.set(None);
                } else if editing.peek().is_some() {
                    editing.set(None);
                    draft.set(String::new());
                } else if replying.peek().is_some() {
                    replying.set(None);
                } else if *ctx.wide.peek() {
                    js::pop_screen();
                }
            }
        });
        let (callback, mut attached) = crate::channel();
        let attach = js::on_attach(&callback);
        let mut uploading = uploading;
        spawn(async move {
            let mut pending = 0;
            while let Some(event) = attached.next().await {
                if event == "start" {
                    pending += 1;
                    uploading.set(true);
                    continue;
                }
                pending -= 1;
                if pending <= 0 {
                    pending = 0;
                    uploading.set(false);
                }
                if let Some(media_id) = event.strip_prefix("ok:") {
                    let reply_to = replying.peek().as_ref().map(|m| m.id);
                    replying.set(None);
                    let message = NewMessage { body: String::new(), media_id: Some(media_id.to_string()), reply_to, track_id: None };
                    spawn_forever(async move {
                        post_message(ctx, id, message).await;
                    });
                } else if let Some(reason) = event.strip_prefix("err:") {
                    ctx.notify(reason.to_string());
                }
            }
        });
        let (callback, mut gestures) = crate::channel();
        let gesture = attach_gestures("messages", &callback);
        spawn(async move {
            let mut pops = 0u32;
            while let Some(event) = gestures.next().await {
                let parts: Vec<&str> = event.split(':').collect();
                let Some(mid) = parts.get(1).and_then(|s| s.parse::<i64>().ok()) else { continue };
                let Some(message) = ctx.messages.peek().iter().find(|m| m.id == mid && !m.deleted).cloned() else { continue };
                let coord = |i: usize| parts.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or_default();
                match parts[0] {
                    "long" => {
                        let locked = ctx.conversations.peek().iter().any(|c| c.id == id && c.locked);
                        if locked && message.body.is_empty() && message.sender.id != me {
                            continue;
                        }
                        pressed.set(Some(Pressed { id: mid, x: coord(2), y: coord(3) }));
                    }
                    "double" => {
                        let locked = ctx.conversations.peek().iter().any(|c| c.id == id && c.locked);
                        if locked {
                            continue;
                        }
                        let liked = message.reactions.iter().any(|r| r.user_id == me && r.emoji == LIKE);
                        bubble::toggle_reaction(ctx, &message, LIKE.to_string());
                        if !liked {
                            pops += 1;
                            let n = pops;
                            heart.set(Some((mid, n)));
                            spawn(async move {
                                gloo_timers::future::TimeoutFuture::new(1200).await;
                                if heart.peek().is_some_and(|h| h.1 == n) {
                                    heart.set(None);
                                }
                            });
                        }
                    }
                    "tap" => {
                        if let Some(media) = message.media.as_ref().filter(|m| m.mime.starts_with("image/")) {
                            viewer.set(Some(api::media_url(&media.id)));
                        }
                    }
                    _ => {}
                }
            }
        });
        if ctx.desktop || *ctx.wide.peek() {
            js::focus_input("composer");
        }
        (escape, attach, gesture)
    });
    use_drop(move || {
        js::off_listener(&listeners.0);
        js::off_listener(&listeners.1);
        js::off_listener(&listeners.2);
    });

    let wide = *ctx.wide.read();
    let side_open = *ctx.side_open.read();
    // Shown big on the call screen.
    let call_photo = peer.as_ref().and_then(|u| u.avatar.clone()).unwrap_or_default();
    let call_peer = peer.as_ref().map(|u| u.id as f64).unwrap_or_default();
    let peer_id = peer.as_ref().map(|u| u.id);
    let online = peer_id.is_some_and(|p| ctx.is_online(p));
    let pressed_id = pressed().map(|p| p.id);
    let heart_on = heart();
    let mut last_day = String::new();
    rsx! {
        div { class: "screen chat",
            header {
                if !wide {
                    button { class: "icon", title: "Retour", onclick: move |_| js::pop_screen(), Icon { name: "back" } }
                }
                // One-to-one: the profile; group: the members.
                button { class: "chat-who", title: if group { "Membres" } else { "Profil" },
                    onclick: move |_| match peer_id {
                        Some(user) => ctx.open(Screen::Profile(user)),
                        None => menu.set(true),
                    },
                    if let Some(user) = peer.clone() {
                        UserAvatar { user, small: true, online }
                    } else {
                        Avatar { name: title.clone(), small: true }
                    }
                    h2 {
                        if let Some(user) = peer.clone() {
                            Name { user }
                        } else {
                            "{title}"
                        }
                        if !subtitle.is_empty() { small { "{subtitle}" } }
                    }
                }
                if !group && blocked.is_none() && !locked && conversation.is_some() {
                    button { class: "icon", title: "Appel vocal",
                        onclick: { let (t, p) = (title.clone(), call_photo.clone()); move |_| js::start_call_with(id as f64, &t, false, &p, call_peer) },
                        Icon { name: "phone" } }
                    button { class: "icon", title: "Appel vidéo",
                        onclick: { let (t, p) = (title.clone(), call_photo.clone()); move |_| js::start_call_with(id as f64, &t, true, &p, call_peer) },
                        Icon { name: "video" } }
                }
                if wide {
                    button { class: if side_open { "icon side-toggle on" } else { "icon side-toggle" },
                        title: if group { "Membres" } else { "Profil" },
                        aria_pressed: "{side_open}",
                        onclick: move |_| ctx.set_side_open(!side_open),
                        Icon { name: ui::pick(group, "users", "eye") } }
                }
                button { class: "icon", title: "Options", onclick: move |_| menu.set(true), Icon { name: "more" } }
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
                        let status = conversation
                            .as_ref()
                            .filter(|_| mine && !m.deleted)
                            .map(|c| bubble::receipt(c, me, m.id))
                            .unwrap_or_default();
                        let lifted = pressed_id == Some(m.id);
                        let pop = heart_on.filter(|h| h.0 == m.id).map(|h| h.1).unwrap_or(0);
                        rsx! {
                            Fragment { key: "{m.id}",
                                if new_day {
                                    div { class: "day", "{day}" }
                                }
                                Bubble { message: m.clone(), group, status, members: members.clone(), lifted, heart: pop, locked }
                            }
                        }
                    }
                }
                if typing {
                    div { class: "typing-row", aria_label: "{typing_names}",
                        if group {
                            div { class: "typing-names", "{typing_names}" }
                        }
                        div { class: "typing-dots", span {} span {} span {} }
                    }
                }
            }
            if uploading() {
                div { class: "status", "Envoi en cours…" }
            }
            if let Some(user) = blocked {
                div { class: "status",
                    "Vous avez bloqué cette personne."
                    button { class: "link", onclick: move |_| set_blocked(ctx, user, false), "Débloquer" }
                }
            } else if locked {
                composer::LockedBar { peer: peer.clone() }
            } else {
                composer::Composer { id, draft, replying, editing, recording, uploading, panel, members: suggest }
            }
            if let Some((m, p)) = menu_for {
                MessageMenu { key: "{m.id}", message: m, x: p.x, y: p.y, locked,
                    on_close: move |_| pressed.set(None),
                    on_action: act,
                }
            }
            if menu() {
                if let Some(c) = conversation.clone() {
                    ConversationMenu { conversation: c, on_close: move |_| menu.set(false) }
                }
            }
            crate::photos::ComposerPanel { id, panel, replying }
            if let Some(url) = viewer() {
                div { class: "viewer", onclick: move |_| viewer.set(None),
                    img { src: "{url}" }
                    if ctx.desktop {
                        button { class: "viewer-close", title: "Fermer (Échap)", Icon { name: "hangup" } }
                    }
                }
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
                                UserAvatar { user: u.clone(), small: true }
                                button { class: "person-name", title: "Voir le profil",
                                    onclick: move |_| { on_close.call(()); ctx.open(Screen::Profile(user)); },
                                    Name { user: u.clone() }
                                }
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
