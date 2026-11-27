//! What friends are listening to on goofysound, "like Discord": the state kept in
//! `Ctx::activities`, status lines for lists and headers, and the desktop pane.

use crate::{
    api, music,
    screens,
    ui::{Avatar, Icon},
    Ctx, Screen,
};
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use proto::{Activity, Conversation, NewConversation, Track, User};

/// Initial state, on (re)connection.
pub async fn load(mut ctx: Ctx) {
    match api::get::<Vec<Activity>>(&ctx.token(), "/api/activity").await {
        Ok(list) => ctx.activities.set(list),
        Err(f) if f.status == 401 => ctx.fail(f),
        // Older servers and unlinked setups simply show no activity.
        Err(_) => {}
    }
}

/// `ServerEvent::Activity`: someone started, changed or stopped listening.
pub fn apply(mut ctx: Ctx, user_id: i64, activity: Option<Activity>) {
    let mut list = ctx.activities.write();
    list.retain(|a| a.user_id != user_id);
    if let Some(activity) = activity {
        list.push(activity);
    }
}

/// What this member is listening to (subscribes the caller to changes).
pub fn track_of(ctx: Ctx, user_id: i64) -> Option<Track> {
    ctx.activities.read().iter().find(|a| a.user_id == user_id).map(|a| a.track.clone())
}

/// What the other person of a one-to-one conversation is listening to.
pub fn peer_track(ctx: Ctx, conversation: &Conversation, me: i64) -> Option<Track> {
    if conversation.is_group {
        return None;
    }
    let other = conversation.members.iter().find(|u| u.id != me)?;
    track_of(ctx, other.id)
}

/// "🎵 Title — Artist"
pub fn line(track: &Track) -> String {
    if track.artist.is_empty() {
        format!("🎵 {}", track.title)
    } else {
        format!("🎵 {} — {}", track.title, track.artist)
    }
}

/// A member known from the conversations.
fn person(ctx: Ctx, id: i64) -> Option<User> {
    ctx.conversations.read().iter().flat_map(|c| c.members.iter()).find(|u| u.id == id).cloned()
}

/// Opens the one-to-one conversation with this member, creating it when needed.
fn open_with(ctx: Ctx, user: i64) {
    let me = ctx.my_id();
    let existing = ctx
        .conversations
        .peek()
        .iter()
        .find(|c| !c.is_group && c.members.len() == 2 && c.members.iter().any(|u| u.id == user) && c.members.iter().any(|u| u.id == me))
        .map(|c| c.id);
    match existing {
        Some(id) => ctx.open(Screen::Chat(id)),
        None => {
            spawn(screens::start_conversation(ctx, NewConversation { member_ids: vec![user], name: None }));
        }
    }
}

/// Third column of the wide layout. Hidden by CSS on narrower windows.
#[component]
pub fn ActivityPane() -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let mut collapsed = use_signal(|| music::pref("activity.collapsed") == "1");
    let mut now = use_signal(js_sys::Date::now);
    use_hook(move || {
        spawn(async move {
            loop {
                TimeoutFuture::new(1000).await;
                now.set(js_sys::Date::now());
            }
        });
    });

    let mut list: Vec<Activity> = ctx.activities.read().iter().filter(|a| a.user_id != me).cloned().collect();
    list.sort_by(|a, b| b.since.cmp(&a.since));
    let entries: Vec<(Activity, User)> = list
        .into_iter()
        .filter_map(|a| {
            let user = person(ctx, a.user_id)?;
            Some((a, user))
        })
        .collect();
    let count = entries.len();
    let toggle = move |_| {
        let value = !collapsed();
        collapsed.set(value);
        music::set_pref("activity.collapsed", if value { "1" } else { "" });
    };

    if collapsed() {
        return rsx! {
            aside { class: "activity-pane collapsed",
                button { class: "icon activity-toggle", title: "Afficher l'activité", onclick: toggle,
                    span { class: "flip", Icon { name: "chevron" } }
                }
                div { class: "activity-rail",
                    if count == 0 {
                        div { class: "rail-empty", title: "Personne n'écoute de musique", Icon { name: "music" } }
                    }
                    for (activity, user) in entries {
                        button { key: "{user.id}", class: "rail-person",
                            title: "{user.display_name} · {line(&activity.track)}",
                            onclick: { let id = user.id; move |_| open_with(ctx, id) },
                            Avatar { name: user.display_name.clone(), photo: user.avatar.clone(), small: true, live: true }
                        }
                    }
                }
            }
        };
    }

    rsx! {
        aside { class: "activity-pane",
            div { class: "activity-head",
                h3 { "Activité" }
                if count > 0 {
                    span { class: "activity-count", "{count}" }
                }
                button { class: "icon activity-toggle", title: "Masquer l'activité", onclick: toggle, Icon { name: "chevron" } }
            }
            div { class: "activity-list",
                if count == 0 {
                    div { class: "activity-empty",
                        div { class: "note-art", Icon { name: "music" } }
                        b { "C'est calme ici" }
                        p { "Quand vos amis écoutent de la musique sur goofysound, ça s'affiche ici." }
                    }
                }
                for (activity, user) in entries {
                    ActivityCard { key: "{user.id}", activity, user, now: now() }
                }
            }
        }
    }
}

#[component]
fn ActivityCard(activity: Activity, user: User, now: f64) -> Element {
    let ctx = use_context::<Ctx>();
    let slot = format!("activity:{}", user.id);
    let state = music::use_slot(slot.clone());
    let current = state();
    let playing = current.as_ref().is_some_and(|p| p.playing || p.loading);
    let track = activity.track.clone();
    let total = (track.duration as f64).max(1.0);
    let elapsed = ((now - activity.since as f64) / 1000.0).clamp(0.0, total);
    let percent = elapsed / total * 100.0;
    let user_id = user.id;
    let listen = {
        let track = track.clone();
        move |e: MouseEvent| {
            e.stop_propagation();
            music::toggle(&slot, &track, state.peek().as_ref());
        }
    };
    rsx! {
        div { class: "activity-card",
            button { class: "activity-who", title: "Écrire à {user.display_name}", onclick: move |_| open_with(ctx, user_id),
                Avatar { name: user.display_name.clone(), photo: user.avatar.clone(), small: true, live: true }
                div { class: "activity-name",
                    b { "{user.display_name}" }
                    span { "sur {activity.player}" }
                }
                music::Eq {}
            }
            div { class: "activity-track",
                music::Cover { track: track.clone(), size: 96 }
                div { class: "activity-song",
                    span { class: "activity-label", "Écoute" }
                    b { title: "{track.title}", "{track.title}" }
                    span { title: "{track.artist}", "{track.artist}" }
                }
            }
            div { class: "activity-progress",
                div { class: "bar", div { style: "width: {percent}%" } }
                div { class: "track-times",
                    span { "{music::clock(elapsed)}" }
                    span { "{music::clock(total)}" }
                }
            }
            button { class: if playing { "ghost listen on" } else { "ghost listen" }, onclick: listen,
                Icon { name: crate::ui::pick(playing, "pause", "play") }
                if playing { "En écoute" } else { "Écouter aussi" }
            }
        }
    }
}
