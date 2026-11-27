//! One message bubble, its attachments and the long-press menu (contract 0.4.0 §7.6).

use crate::{
    api, js,
    ui::{self, Icon},
    Ctx,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use proto::{Conversation, Media, Message, Quote, React, REACTIONS};

/// Adds the reaction, or removes it when it is already the user's.
pub fn toggle_reaction(ctx: Ctx, message: &Message, emoji: String) {
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
pub fn receipt(conversation: &Conversation, me: i64, message: i64) -> String {
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

fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

/// Splits a body into plain runs and "@username" runs of members (with their user id),
/// with the server's rule (contract §3.3.1): "@" at the start or after a character that is
/// not `[A-Za-z0-9_.@]`, then 3 to 24 of `[A-Za-z0-9_.]`, trailing dots stripped.
pub fn mention_runs(body: &str, members: &[(String, i64)]) -> Vec<(String, Option<i64>)> {
    let chars: Vec<(usize, char)> = body.char_indices().collect();
    let mut runs = vec![];
    let mut start = 0;
    let mut i = 0;
    while i < chars.len() {
        let (pos, c) = chars[i];
        let boundary = i == 0 || !(name_char(chars[i - 1].1) || chars[i - 1].1 == '@');
        if c == '@' && boundary {
            let mut j = i + 1;
            while j < chars.len() && name_char(chars[j].1) {
                j += 1;
            }
            let end = chars.get(j).map(|c| c.0).unwrap_or(body.len());
            let name = body[pos + 1..end].trim_end_matches('.');
            if (3..=24).contains(&name.len()) {
                if let Some((_, user)) = members.iter().find(|(u, _)| u.eq_ignore_ascii_case(name)) {
                    if start < pos {
                        runs.push((body[start..pos].to_string(), None));
                    }
                    let stop = pos + 1 + name.len();
                    runs.push((body[pos..stop].to_string(), Some(*user)));
                    start = stop;
                    while i < chars.len() && chars[i].0 < stop {
                        i += 1;
                    }
                    continue;
                }
            }
        }
        i += 1;
    }
    if start < body.len() {
        runs.push((body[start..].to_string(), None));
    }
    runs
}

/// A message of the conversation. `data-msg` lets `js/gestures.js` report long presses,
/// double taps and taps on its photo.
#[component]
pub fn Bubble(
    message: Message,
    group: bool,
    /// Receipt of an own message ("✓✓"), else empty.
    status: String,
    /// Usernames (any case) and ids of the members, for mention highlighting.
    members: Vec<(String, i64)>,
    /// The long-press menu is open on it.
    lifted: bool,
    /// Non-zero: show the double-tap heart (a new value replays it).
    heart: u32,
    /// Locked conversation: reactions cannot change.
    locked: bool,
) -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let m = &message;
    let mine = m.sender.id == me;
    let mut class = String::from(if mine { "bubble mine" } else { "bubble" });
    if m.mentions.contains(&me) && !m.deleted {
        class.push_str(" mentioned");
    }
    if lifted {
        class.push_str(" lifted");
    }
    let reactions = tally(m, me);
    let runs = if m.body.contains('@') { mention_runs(&m.body, &members) } else { vec![(m.body.clone(), None)] };
    let edited = m.edited_at.is_some() && !m.deleted;
    let pops = (heart > 0).then_some(heart);
    rsx! {
        div { class, "data-msg": "{m.id}",
            if group && !mine {
                div { class: "who", ui::Name { user: m.sender.clone() } }
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
                Attachment { media, message: m.id, mine }
            }
            if let Some(track) = m.track.clone().filter(|_| !m.deleted) {
                crate::music::TrackCard { track, slot: format!("msg:{}", m.id) }
            }
            if !m.body.is_empty() {
                div { class: "text",
                    for (text, who) in runs {
                        if let Some(user) = who {
                            span { class: if user == me { "mention me" } else { "mention" }, "{text}" }
                        } else {
                            "{text}"
                        }
                    }
                }
            }
            div { class: "time",
                if edited {
                    span { class: "edited", "modifié" }
                }
                "{ui::time(m.created_at)} {status}"
            }
            if !reactions.is_empty() {
                div { class: "reactions",
                    for (emoji, count, own) in reactions {
                        button { key: "{emoji}", class: if own { "reaction own" } else { "reaction" }, disabled: locked,
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
            for n in pops {
                span { key: "{n}", class: "heart-pop", aria_hidden: "true", "❤️" }
            }
        }
    }
}

#[component]
fn Attachment(media: Media, message: i64, mine: bool) -> Element {
    let url = api::media_url(&media.id);
    // Voice messages are recorded in a WebM container, always declared audio/*.
    match media.mime.split('/').next().unwrap_or("") {
        // A tap opens the viewer (reported by gestures.js once it is not a double tap).
        "image" => rsx! { img { src: "{url}", alt: "{media.name}", draggable: "false", "data-view": "1" } },
        "video" => rsx! { video { src: "{url}#t=0.1", controls: true, preload: "metadata", playsinline: true } },
        _ => rsx! { VoiceNote { media, message, mine } },
    }
}

/// Audio player of a message: a waveform for voice messages, a slim track for
/// other audio files. `js/voice.js` draws it and plays it.
#[component]
fn VoiceNote(media: Media, message: i64, mine: bool) -> Element {
    let id = format!("vm-{message}");
    let voice = media.name.starts_with("message-vocal") || media.mime.contains("webm") || media.mime.contains("ogg");
    let kind = if voice { "voice" } else { "file" };
    let unmount = id.clone();
    use_drop(move || js::voice_unmount(&unmount));
    let (mount, toggle, speed) = (id.clone(), id.clone(), id.clone());
    rsx! {
        div { id: "{id}", class: crate::ui::pick(mine, "vm mine", "vm"),
            "data-media": "{media.id}", "data-url": "{api::media_url(&media.id)}", "data-kind": kind,
            "data-state": "idle",
            onmounted: move |_| js::voice_mount(&mount),
            button { class: "vm-play", title: "Lire", aria_label: "Lire le message vocal",
                onclick: move |_| js::voice_toggle(&toggle),
                span { class: "vm-icon-play", Icon { name: "play" } }
                span { class: "vm-icon-pause", Icon { name: "pause" } }
            }
            div { class: "vm-body",
                if !voice {
                    div { class: "vm-name", "{media.name}" }
                }
                canvas { class: "vm-wave" }
                span { class: "vm-time", "data-t": "0:00" }
            }
            if voice {
                button { class: "vm-speed", title: "Vitesse", "data-s": "1×", onclick: move |_| js::voice_speed(&speed) }
            }
        }
    }
}

/// Icons of the menu that `ui::Icon` does not have.
fn menu_icon(name: &str) -> Element {
    let path = match name {
        "reply" => "M9 14L4 9l5-5M4 9h10a6 6 0 0 1 6 6v5",
        "copy" => "M9 9h11v11H9zM5 15H4V4h11v1",
        _ => "",
    };
    rsx! { svg { view_box: "0 0 24 24", path { d: path } } }
}

/// What the long-press menu asks the conversation to do.
#[derive(Clone, PartialEq, Debug)]
pub enum MenuAction {
    Reply(Message),
    /// Reply prefilled with "@username ".
    Mention(Message),
    Edit(Message),
}

/// Menu opened by a long press (or right click) at the press point: reactions, reply,
/// mention, copy, edit and delete.
#[component]
pub fn MessageMenu(message: Message, x: f64, y: f64, locked: bool, on_close: EventHandler<()>, on_action: EventHandler<MenuAction>) -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let mut confirm_delete = use_signal(|| false);
    let m = message.clone();
    let mine = m.sender.id == me;
    let editable = mine && !m.deleted && m.track.is_none() && !m.body.is_empty() && !locked;
    let copy_text = m.body.clone();
    let delete_path = format!("/api/messages/{}", m.id);
    rsx! {
        div { class: "msg-menu-backdrop", onclick: move |_| on_close.call(()),
            oncontextmenu: move |e| { e.prevent_default(); on_close.call(()); } }
        div { id: "msg-menu", class: "msg-menu", role: "menu",
            style: "left: {x}px; top: {y}px",
            onmounted: move |_| super::place_menu("msg-menu", x, y),
            if !locked && !m.deleted {
                div { class: "msg-menu-emojis",
                    for emoji in REACTIONS {
                        {
                            let own = m.reactions.iter().any(|r| r.user_id == me && r.emoji == emoji);
                            let target = m.clone();
                            rsx! {
                                button { key: "{emoji}", class: if own { "own" } else { "" }, aria_label: "Réagir {emoji}",
                                    onclick: move |_| { toggle_reaction(ctx, &target, emoji.to_string()); on_close.call(()); },
                                    "{emoji}"
                                }
                            }
                        }
                    }
                }
            }
            if !locked && !m.deleted {
                button { class: "msg-menu-item", role: "menuitem",
                    onclick: { let m = m.clone(); move |_| on_action.call(MenuAction::Reply(m.clone())) },
                    {menu_icon("reply")} "Répondre"
                }
                if !mine {
                    button { class: "msg-menu-item", role: "menuitem",
                        onclick: { let m = m.clone(); move |_| on_action.call(MenuAction::Mention(m.clone())) },
                        Icon { name: "at" } "Mentionner"
                    }
                }
            }
            if !copy_text.is_empty() && !m.deleted {
                button { class: "msg-menu-item", role: "menuitem",
                    onclick: move |_| {
                        let text = copy_text.clone();
                        on_close.call(());
                        spawn_forever(async move {
                            let ok = super::copy_text(&text).await.ok().and_then(|v| v.as_bool()).unwrap_or(false);
                            ctx.notify(if ok { "Copié." } else { "Copie impossible." });
                        });
                    },
                    {menu_icon("copy")} "Copier"
                }
            }
            if editable {
                button { class: "msg-menu-item", role: "menuitem",
                    onclick: { let m = m.clone(); move |_| on_action.call(MenuAction::Edit(m.clone())) },
                    Icon { name: "edit" } "Modifier"
                }
            }
            if mine && !m.deleted {
                button { class: "msg-menu-item danger", role: "menuitem",
                    onclick: move |_| {
                        if !confirm_delete() {
                            confirm_delete.set(true);
                            return;
                        }
                        on_close.call(());
                        let path = delete_path.clone();
                        spawn_forever(async move {
                            if let Err(e) = api::delete(&ctx.token(), &path).await {
                                ctx.fail(e);
                            }
                        });
                    },
                    Icon { name: "trash" }
                    if confirm_delete() { "Confirmer la suppression" } else { "Supprimer pour tout le monde" }
                }
            }
        }
    }
}
