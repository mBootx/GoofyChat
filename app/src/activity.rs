//! What friends are listening to on goofysound, "like Discord": the state kept in
//! `Ctx::activities`, status lines for lists and headers; the Activity page (mobile tab)
//! and the desktop right pane (contract 0.4.0 §7.3, §7.4).

use crate::{
    api, music, profile, screens, stories,
    ui::{Icon, Name, UserAvatar},
    Ctx, Screen,
};
use dioxus::prelude::*;
use gloo_timers::future::TimeoutFuture;
use proto::{Activity, Conversation, Track, User};

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

/// The current time, refreshed every second (listening progress bars).
fn use_clock() -> Signal<f64> {
    let mut now = use_signal(js_sys::Date::now);
    use_hook(move || {
        spawn(async move {
            loop {
                TimeoutFuture::new(1000).await;
                now.set(js_sys::Date::now());
            }
        });
    });
    now
}

/// The "Activité" tab of the mobile home.
#[component]
pub fn ActivityPage() -> Element {
    rsx! {
        div { class: "screen activity-page",
            header { h2 { "Activité" } }
            div { class: "list activity-feed-wrap",
                ActivityFeed {}
            }
        }
    }
}

/// Right pane of the wide layout (shown while `Ctx::side_open`): the profile of the other
/// person of a one-to-one conversation, the members of a group, else the activity.
#[component]
pub fn SidePane() -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let open = match *ctx.screen.read() {
        Screen::Chat(id) => Some(id),
        _ => None,
    };
    let conversation = open.and_then(|id| ctx.conversations.read().iter().find(|c| c.id == id).cloned());
    let content = match conversation {
        Some(c) if c.is_group => rsx! { MembersPane { conversation: c } },
        Some(c) => match screens::peer(&c, me) {
            Some(user) => rsx! { profile::ProfilePane { key: "{user.id}", user_id: user.id } },
            None => rsx! { ActivityPane {} },
        },
        None => rsx! { ActivityPane {} },
    };
    rsx! {
        aside { class: "right-pane",
            button { class: "icon pane-close", title: "Masquer le panneau", onclick: move |_| ctx.set_side_open(false),
                Icon { name: "hangup" }
            }
            {content}
        }
    }
}

/// Members of the open group, online ones first; tap opens a profile.
#[component]
fn MembersPane(conversation: Conversation) -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let mut members = conversation.members.clone();
    let online: Vec<i64> = members.iter().filter(|u| u.id == me || ctx.is_online(u.id)).map(|u| u.id).collect();
    members.sort_by_key(|u| (!online.contains(&u.id), u.display_name.to_lowercase()));
    rsx! {
        div { class: "pane-head",
            h3 { "Membres — {members.len()}" }
        }
        div { class: "pane-list",
            for u in members {
                {
                    let id = u.id;
                    let live = track_of(ctx, id).is_some();
                    rsx! {
                        button { key: "{id}", class: "row pane-person", onclick: move |_| ctx.open(Screen::Profile(id)),
                            UserAvatar { user: u.clone(), small: true, online: online.contains(&id), live }
                            div { class: "main",
                                div { class: "title", Name { user: u.clone() } }
                                if let Some(t) = track_of(ctx, id) {
                                    div { class: "listening", "{line(&t)}" }
                                } else {
                                    div { class: "sub", "@{u.username}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The activity in the desktop right pane.
#[component]
pub fn ActivityPane() -> Element {
    rsx! {
        div { class: "pane-head", h3 { "Activité" } }
        div { class: "pane-list activity-feed-wrap",
            ActivityFeed {}
        }
    }
}

/// Friends only: who is online, what they listen to, their recent stories.
#[component]
fn ActivityFeed() -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let now = use_clock();
    let friends = ctx.friends.read().clone();
    let online: Vec<User> = friends.iter().filter(|f| ctx.is_online(f.user.id)).map(|f| f.user.clone()).collect();
    let mut listening: Vec<Activity> = ctx
        .activities
        .read()
        .iter()
        .filter(|a| a.user_id != me && friends.iter().any(|f| f.user.id == a.user_id))
        .cloned()
        .collect();
    listening.sort_by(|a, b| b.since.cmp(&a.since));
    let entries: Vec<(Activity, User)> = listening
        .into_iter()
        .filter_map(|a| {
            let user = friends.iter().find(|f| f.user.id == a.user_id)?.user.clone();
            Some((a, user))
        })
        .collect();
    let stories = ctx
        .stories
        .read()
        .iter()
        .any(|r| r.user.id != me && r.stories.iter().any(|s| s.state == proto::StoryState::Live));

    if online.is_empty() && entries.is_empty() && !stories {
        return rsx! {
            div { class: "activity-empty",
                div { class: "note-art", Icon { name: "activity" } }
                p { "C'est calme ici — quand vos amis sont en ligne, écoutent de la musique ou publient une story, ça s'affiche ici." }
            }
        };
    }
    rsx! {
        div { class: "activity-feed",
            if !online.is_empty() {
                section { class: "feed-section",
                    h3 { class: "section-title", "En ligne — {online.len()}" }
                    div { class: "online-strip",
                        for u in online {
                            {
                                let id = u.id;
                                rsx! {
                                    button { key: "{id}", class: "online-friend", title: "{u.display_name}",
                                        onclick: move |_| ctx.open(Screen::Profile(id)),
                                        UserAvatar { user: u.clone(), online: true, live: track_of(ctx, id).is_some() }
                                        span { class: "online-name", Name { user: u.clone() } }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            if !entries.is_empty() {
                section { class: "feed-section",
                    h3 { class: "section-title", "En écoute — {entries.len()}" }
                    div { class: "activity-list",
                        for (activity, user) in entries {
                            ActivityCard { key: "{user.id}", activity, user, now: now() }
                        }
                    }
                }
            }
            if stories {
                section { class: "feed-section",
                    h3 { class: "section-title", "Stories récentes" }
                    stories::RecentStories {}
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
            button { class: "activity-who", title: "Profil de {user.display_name}", onclick: move |_| ctx.open(Screen::Profile(user_id)),
                UserAvatar { user: user.clone(), small: true, live: true }
                div { class: "activity-name",
                    b { Name { user: user.clone() } }
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
