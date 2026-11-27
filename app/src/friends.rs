//! Friends: the list, requests, adding by username and presence (contract 0.4.0 §3.2, §7).
//!
//! `load` and the event hooks keep `Ctx::friends`, `Ctx::requests` and `Ctx::online` in
//! step; the helpers below are shared with the profile screens.

use crate::{
    api, js, screens,
    ui::{Icon, Name, UserAvatar},
    Ctx, Screen,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use proto::{
    Friend, FriendNotice, FriendNoticeKind, FriendRequestOutcome, FriendRequestStatus, FriendRequests,
    NewConversation, NewFriendRequest, User,
};

/// Initial state, on (re)connection and after a `friends` event. Older servers have no
/// such routes: errors other than a bad session are ignored.
pub async fn load(mut ctx: Ctx) {
    match api::get::<Vec<Friend>>(&ctx.token(), "/api/friends").await {
        Ok(list) => {
            ctx.online.set(list.iter().filter(|f| f.online).map(|f| f.user.id).collect());
            ctx.friends.set(list);
        }
        Err(f) if f.status == 401 => return ctx.fail(f),
        Err(_) => {}
    }
    match api::get::<FriendRequests>(&ctx.token(), "/api/friends/requests").await {
        Ok(requests) => ctx.requests.set(requests),
        Err(f) if f.status == 401 => ctx.fail(f),
        Err(_) => {}
    }
}

/// `ServerEvent::Friends`: reload, and tell the user about a new request or acceptance.
pub async fn on_friends(ctx: Ctx, notice: Option<FriendNotice>) {
    load(ctx).await;
    if let Some(notice) = notice {
        let name = notice.user.display_name;
        ctx.notify(match notice.kind {
            FriendNoticeKind::Request => format!("{name} vous a envoyé une demande d'ami."),
            FriendNoticeKind::Accepted => format!("{name} a accepté votre demande d'ami."),
        });
    }
}

/// `ServerEvent::Presence`.
pub fn on_presence(mut ctx: Ctx, user_id: i64, online: bool) {
    let mut set = ctx.online.write();
    if online {
        set.insert(user_id);
    } else {
        set.remove(&user_id);
    }
}

/// `ServerEvent::UserChanged`: replaces the user wherever the app keeps a copy.
pub fn on_user_changed(mut ctx: Ctx, user: User) {
    if user.id == ctx.my_id() {
        ctx.set_user(user.clone());
    }
    for friend in ctx.friends.write().iter_mut().filter(|f| f.user.id == user.id) {
        friend.user = user.clone();
    }
    {
        let mut requests = ctx.requests.write();
        let r = &mut *requests;
        for request in r.incoming.iter_mut().chain(r.outgoing.iter_mut()) {
            if request.user.id == user.id {
                request.user = user.clone();
            }
        }
    }
    for conversation in ctx.conversations.write().iter_mut() {
        for member in conversation.members.iter_mut().filter(|m| m.id == user.id) {
            *member = user.clone();
        }
    }
    for message in ctx.messages.write().iter_mut().filter(|m| m.sender.id == user.id) {
        message.sender = user.clone();
    }
    for reel in ctx.stories.write().iter_mut().filter(|r| r.user.id == user.id) {
        reel.user = user.clone();
    }
}

/// Sends a friend request by username. True when it was sent or made you friends.
pub async fn send_request(ctx: Ctx, username: String) -> bool {
    let body = NewFriendRequest { username };
    match api::post::<_, FriendRequestOutcome>(&ctx.token(), "/api/friends/requests", &body).await {
        Ok(outcome) => {
            let name = outcome.user.display_name;
            let became_friends = outcome.status == FriendRequestStatus::Friends;
            ctx.notify(match outcome.status {
                FriendRequestStatus::Sent => format!("Demande envoyée à {name}."),
                FriendRequestStatus::Friends => format!("Vous êtes maintenant amis avec {name}."),
            });
            load(ctx).await;
            if became_friends {
                ctx.refresh().await;
            }
            true
        }
        Err(e) => {
            ctx.fail(e);
            false
        }
    }
}

/// What to do with a pending request or a friendship.
#[derive(Clone, Copy, PartialEq)]
pub enum Change {
    Accept,
    Decline,
    /// Cancel one's own outgoing request.
    Cancel,
    /// Remove a friend.
    Remove,
}

/// Applies a [`Change`] and reloads the lists (the server's `friends` event follows).
pub async fn change(ctx: Ctx, user_id: i64, what: Change) {
    let token = ctx.token();
    let result = match what {
        Change::Accept => {
            api::post::<_, Friend>(&token, &format!("/api/friends/requests/{user_id}/accept"), &()).await.map(|_| ())
        }
        Change::Decline => api::post_empty(&token, &format!("/api/friends/requests/{user_id}/decline")).await,
        Change::Cancel => api::delete(&token, &format!("/api/friends/requests/{user_id}")).await,
        Change::Remove => api::delete(&token, &format!("/api/friends/{user_id}")).await,
    };
    match result {
        Ok(()) => {
            if what == Change::Remove {
                ctx.notify("Ami retiré.");
            }
            load(ctx).await;
            // A direct conversation may have been locked or unlocked.
            ctx.refresh().await;
        }
        Err(e) => ctx.fail(e),
    }
}

/// The one-to-one conversation with this user, when the app knows it.
pub fn direct_with(ctx: Ctx, user_id: i64) -> Option<i64> {
    if let Some(id) = ctx.friend(user_id).and_then(|f| f.direct_id) {
        return Some(id);
    }
    let me = ctx.my_id();
    ctx.conversations
        .peek()
        .iter()
        .find(|c| !c.is_group && c.members.iter().any(|u| u.id == user_id) && c.members.iter().any(|u| u.id == me))
        .map(|c| c.id)
}

/// Opens the one-to-one conversation with a friend, creating it when needed.
pub fn open_direct(ctx: Ctx, user_id: i64) {
    match direct_with(ctx, user_id) {
        Some(id) => ctx.open(Screen::Chat(id)),
        None => {
            spawn_forever(screens::start_conversation(ctx, NewConversation { member_ids: vec![user_id], name: None }));
        }
    }
}

/// Friends, requests and "add by username".
#[component]
pub fn FriendsScreen() -> Element {
    let ctx = use_context::<Ctx>();
    let mut username = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let friends = ctx.friends.read().clone();
    let requests = ctx.requests.read().clone();
    let online: Vec<i64> = friends.iter().filter(|f| ctx.is_online(f.user.id)).map(|f| f.user.id).collect();
    use_hook(move || {
        // Fresh lists each time the screen opens (cheap, and catches missed events).
        spawn(load(ctx));
        if ctx.desktop {
            js::focus_input("friend-username");
        }
    });

    let submit = move |event: FormEvent| {
        event.prevent_default();
        let name = username().trim().to_string();
        if name.is_empty() || busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            if send_request(ctx, name).await {
                username.set(String::new());
            }
            busy.set(false);
        });
    };

    let count_online = online.len();
    rsx! {
        div { class: "screen friends",
            header {
                button { class: "icon", title: "Retour", onclick: move |_| ctx.back(), Icon { name: "back" } }
                h2 { "Amis" small { "{friends.len()} ami(s), {count_online} en ligne" } }
            }
            div { class: "list friends-body",
                form { class: "add-friend", onsubmit: submit,
                    label { r#for: "friend-username", "Ajouter un ami" }
                    p { class: "hint", "Saisissez l'identifiant exact de la personne (par exemple @camille)." }
                    div { class: "add-friend-row",
                        input { id: "friend-username", class: "field", placeholder: "@identifiant", autocapitalize: "none",
                            autocomplete: "off", spellcheck: "false", value: "{username}",
                            oninput: move |e| username.set(e.value()) }
                        button { class: "primary", r#type: "submit", disabled: busy() || username().trim().is_empty(),
                            if busy() { "Envoi…" } else { "Envoyer" }
                        }
                    }
                }

                if !requests.incoming.is_empty() {
                    h3 { class: "section-title", "Demandes reçues — {requests.incoming.len()}" }
                    for r in requests.incoming.clone() {
                        PersonRow { key: "in-{r.user.id}", user: r.user.clone(),
                            div { class: "row-actions",
                                button { class: "small-btn accent", onclick: { let id = r.user.id; move |e: MouseEvent| { e.stop_propagation(); spawn_forever(change(ctx, id, Change::Accept)); } },
                                    "Accepter" }
                                button { class: "small-btn", onclick: { let id = r.user.id; move |e: MouseEvent| { e.stop_propagation(); spawn_forever(change(ctx, id, Change::Decline)); } },
                                    "Refuser" }
                            }
                        }
                    }
                }

                if !requests.outgoing.is_empty() {
                    h3 { class: "section-title", "Demandes envoyées — {requests.outgoing.len()}" }
                    for r in requests.outgoing.clone() {
                        PersonRow { key: "out-{r.user.id}", user: r.user.clone(),
                            div { class: "row-actions",
                                button { class: "small-btn", onclick: { let id = r.user.id; move |e: MouseEvent| { e.stop_propagation(); spawn_forever(change(ctx, id, Change::Cancel)); } },
                                    "Annuler" }
                            }
                        }
                    }
                }

                h3 { class: "section-title", "Amis — {friends.len()}" }
                if friends.is_empty() {
                    div { class: "empty small", "Pas encore d'amis. Envoyez une demande avec leur identifiant ci-dessus." }
                }
                for f in friends {
                    PersonRow { key: "f-{f.user.id}", user: f.user.clone(), online: online.contains(&f.user.id),
                        div { class: "row-actions",
                            button { class: "icon", title: "Écrire à {f.user.display_name}",
                                onclick: { let id = f.user.id; move |e: MouseEvent| { e.stop_propagation(); open_direct(ctx, id); } },
                                Icon { name: "chat" } }
                        }
                    }
                }
            }
        }
    }
}

/// A person in a list: avatar, name, @username; tap opens the profile.
#[component]
pub fn PersonRow(user: User, #[props(default)] online: bool, children: Element) -> Element {
    let ctx = use_context::<Ctx>();
    let id = user.id;
    rsx! {
        div { class: "row person-row", role: "button", tabindex: "0",
            onclick: move |_| ctx.open(Screen::Profile(id)),
            onkeydown: move |e| {
                if matches!(e.key(), Key::Enter) {
                    ctx.open(Screen::Profile(id));
                }
            },
            UserAvatar { user: user.clone(), online }
            div { class: "main",
                div { class: "title", Name { user: user.clone() } }
                div { class: "sub", "@{user.username}" }
            }
            {children}
        }
    }
}
