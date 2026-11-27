//! Sign up and in, conversation list and new conversation.

use crate::{
    activity, api, chat, js,
    ui::{self, Avatar, Icon, Name, UserAvatar},
    Ctx, Screen,
};
use dioxus::prelude::*;
use proto::{Conversation, Login, NewConversation, Register, Session, User};

#[component]
pub fn Auth() -> Element {
    let ctx = use_context::<Ctx>();
    let mut registering = use_signal(|| false);
    let mut username = use_signal(String::new);
    let mut display = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut invite = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let submit = move |event: FormEvent| {
        event.prevent_default();
        if busy() {
            return;
        }
        busy.set(true);
        error.set(String::new());
        let mut session = ctx.session;
        spawn(async move {
            let result: Result<Session, api::Failure> = if registering() {
                let body = Register {
                    username: username(),
                    display_name: display(),
                    password: password(),
                    invite: invite(),
                };
                api::post("", "/api/register", &body).await
            } else {
                let body = Login { username: username(), password: password() };
                api::post("", "/api/login", &body).await
            };
            busy.set(false);
            match result {
                Ok(s) => {
                    if let Ok(json) = serde_json::to_string(&s) {
                        js::save_session(&json);
                    }
                    session.set(Some(s));
                }
                Err(e) => error.set(e.message),
            }
        });
    };

    rsx! {
        form { class: "screen auth", onsubmit: submit,
            div { class: "logo", Icon { name: "chat" } }
            h1 { "Goofy Chat" }
            p { if registering() { "Créez votre compte" } else { "Heureux de vous revoir" } }
            input { class: "field", placeholder: "Identifiant", autocapitalize: "none", autocomplete: "username",
                value: "{username}", oninput: move |e| username.set(e.value()) }
            if registering() {
                input { class: "field", placeholder: "Nom affiché", value: "{display}",
                    oninput: move |e| display.set(e.value()) }
            }
            input { class: "field", r#type: "password", placeholder: "Mot de passe (8 caractères min.)",
                value: "{password}", oninput: move |e| password.set(e.value()) }
            if registering() {
                input { class: "field", placeholder: "Code d'invitation (si demandé)", autocapitalize: "none",
                    value: "{invite}", oninput: move |e| invite.set(e.value()) }
            }
            if !error().is_empty() {
                div { class: "error", "{error}" }
            }
            button { class: "primary", r#type: "submit", disabled: busy(),
                if registering() { "Créer le compte" } else { "Se connecter" }
            }
            button { class: "link", r#type: "button",
                onclick: move |_| { registering.toggle(); error.set(String::new()); },
                if registering() { "J'ai déjà un compte" } else { "Créer un compte" }
            }
        }
    }
}

pub fn title(conversation: &Conversation, me: i64) -> String {
    if conversation.is_group {
        return conversation.name.clone();
    }
    conversation
        .members
        .iter()
        .find(|u| u.id != me)
        .map(|u| u.display_name.clone())
        .unwrap_or_default()
}

fn preview(c: &Conversation) -> String {
    match &c.last_message {
        None => "Aucun message".into(),
        Some(m) => ui::message_summary(m),
    }
}

/// The other person of a one-to-one conversation.
pub fn peer(conversation: &Conversation, me: i64) -> Option<User> {
    if conversation.is_group {
        return None;
    }
    conversation.members.iter().find(|u| u.id != me).cloned()
}

/// The "Messages" tab of the mobile home: the conversation list, the friends button and
/// the "+" button.
#[component]
pub fn Conversations() -> Element {
    let ctx = use_context::<Ctx>();
    let requests = ctx.requests.read().incoming.len();
    rsx! {
        div { class: "screen conversations-screen",
            header {
                h2 { "Messages" }
                button { class: "icon with-badge", title: "Amis", onclick: move |_| ctx.open(Screen::Friends),
                    Icon { name: "userplus" }
                    if requests > 0 {
                        span { class: "badge", "{requests}" }
                    }
                }
            }
            ConversationList {}
            button { class: "fab", title: "Nouvelle discussion", onclick: move |_| ctx.open(Screen::New), Icon { name: "plus" } }
        }
    }
}

/// The conversations, newest first (mobile Messages tab and desktop side navigation).
#[component]
pub fn ConversationList() -> Element {
    let ctx = use_context::<Ctx>();
    let conversations = ctx.conversations.read().clone();
    let open = match *ctx.screen.read() {
        Screen::Chat(id) => Some(id),
        _ => None,
    };
    let hint = if ctx.desktop {
        "Aucune discussion. Cliquez sur + pour écrire à un ami."
    } else {
        "Aucune discussion. Touchez + pour écrire à un ami."
    };
    rsx! {
        div { class: "list conversations",
            if conversations.is_empty() {
                div { class: "empty",
                    p { "{hint}" }
                    button { class: "ghost", onclick: move |_| ctx.open(Screen::Friends), "Ajouter des amis" }
                }
            }
            for c in conversations {
                ConversationRow { key: "{c.id}", active: open == Some(c.id), conversation: c }
            }
        }
    }
}

#[component]
fn ConversationRow(conversation: Conversation, active: bool) -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let c = conversation;
    let id = c.id;
    let other = peer(&c, me);
    let track = activity::peer_track(ctx, &c, me);
    let online = other.as_ref().is_some_and(|u| ctx.is_online(u.id));
    let status = if c.locked { None } else { chat::row_status(&c, me) };
    let mut class = String::from("row");
    if active {
        class.push_str(" active");
    }
    if c.unread > 0 {
        class.push_str(" unread");
    }
    rsx! {
        button { class, onclick: move |_| ctx.open(Screen::Chat(id)),
            if let Some(user) = other.clone() {
                UserAvatar { user, live: track.is_some(), online }
            } else {
                Avatar { name: c.name.clone() }
            }
            div { class: "main",
                div { class: "title",
                    if let Some(user) = other {
                        Name { user }
                    } else {
                        "{c.name}"
                    }
                }
                div { class: "sub", "{preview(&c)}" }
                if let Some(t) = &track {
                    div { class: "listening", "{activity::line(t)}" }
                }
            }
            div { class: "side",
                if c.locked {
                    span { class: "row-lock", title: "Vous n'êtes plus amis", Icon { name: "lock" } }
                } else if let Some(s) = status {
                    span { class: "row-status", "{s}" }
                } else if let Some(m) = &c.last_message {
                    span { "{ui::short(m.created_at)}" }
                }
                if c.unread > 0 {
                    span { class: "badge", "{c.unread}" }
                }
            }
        }
    }
}

/// Opens the conversation, creating it when it does not exist yet.
pub(crate) async fn start_conversation(ctx: Ctx, body: NewConversation) {
    #[derive(serde::Deserialize)]
    struct Created {
        id: i64,
    }
    match api::post::<_, Created>(&ctx.token(), "/api/conversations", &body).await {
        Ok(created) => {
            ctx.refresh().await;
            ctx.open(Screen::Chat(created.id));
        }
        Err(e) => ctx.fail(e),
    }
}

fn matches(user: &User, query: &str) -> bool {
    let q = query.trim().trim_start_matches('@').to_lowercase();
    q.is_empty() || user.username.to_lowercase().contains(&q) || user.display_name.to_lowercase().contains(&q)
}

/// A new conversation with one friend, or a group of friends (a name when two or more).
#[component]
pub fn NewChat() -> Element {
    let ctx = use_context::<Ctx>();
    let mut chosen = use_signal(Vec::<User>::new);
    let mut group = use_signal(String::new);
    let mut query = use_signal(String::new);
    let mut busy = use_signal(|| false);
    // A keyboard is at hand on desktop; on a phone it would cover the screen.
    use_hook(move || {
        if ctx.desktop {
            js::focus_input("new-search");
        }
    });
    let friends: Vec<User> = ctx.friends.read().iter().map(|f| f.user.clone()).collect();
    let no_friends = friends.is_empty();
    let shown: Vec<User> = friends.into_iter().filter(|u| matches(u, &query())).collect();
    let count = chosen.read().len();

    let create = move |_| {
        if busy() {
            return;
        }
        let ids: Vec<i64> = chosen.read().iter().map(|u| u.id).collect();
        if ids.len() == 1 {
            crate::friends::open_direct(ctx, ids[0]);
            return;
        }
        let name = group().trim().to_string();
        if name.is_empty() {
            ctx.notify("Donnez un nom au groupe.");
            return;
        }
        busy.set(true);
        let body = NewConversation { member_ids: ids, name: Some(name) };
        spawn(async move {
            start_conversation(ctx, body).await;
            if let Ok(mut b) = busy.try_write() {
                *b = false;
            }
        });
    };

    rsx! {
        div { class: "screen",
            header {
                button { class: "icon", title: "Retour", onclick: move |_| ctx.back(), Icon { name: "back" } }
                h2 { "Nouvelle discussion" small { "Avec vos amis" } }
            }
            div { class: "pad",
                input { id: "new-search", class: "field", placeholder: "Rechercher un ami…", autocapitalize: "none",
                    value: "{query}", oninput: move |e| query.set(e.value()) }
                if count > 0 {
                    div { class: "chips",
                        for u in chosen() {
                            button { key: "{u.id}", class: "chip",
                                onclick: move |_| chosen.write().retain(|x| x.id != u.id),
                                "{u.display_name} ✕"
                            }
                        }
                    }
                }
                if count > 1 {
                    input { class: "field", placeholder: "Nom du groupe", value: "{group}",
                        oninput: move |e| group.set(e.value()) }
                }
                if count > 0 {
                    button { class: "primary", disabled: busy(), onclick: create,
                        if count > 1 { "Créer le groupe" } else { "Discuter" }
                    }
                }
            }
            div { class: "list",
                if no_friends {
                    div { class: "empty",
                        p { "Vous ne pouvez écrire qu'à vos amis. Ajoutez-en avec leur identifiant." }
                        button { class: "ghost", onclick: move |_| ctx.open(Screen::Friends), "Ajouter des amis" }
                    }
                } else if shown.is_empty() {
                    div { class: "empty", "Aucun ami trouvé." }
                }
                for u in shown {
                    {
                        let picked = chosen.read().iter().any(|x| x.id == u.id);
                        let online = ctx.is_online(u.id);
                        rsx! {
                            button { key: "{u.id}", class: if picked { "row picked" } else { "row" },
                                onclick: {
                                    let u = u.clone();
                                    move |_| {
                                        if chosen.read().iter().any(|x| x.id == u.id) {
                                            chosen.write().retain(|x| x.id != u.id);
                                        } else {
                                            chosen.write().push(u.clone());
                                        }
                                    }
                                },
                                UserAvatar { user: u.clone(), online }
                                div { class: "main",
                                    div { class: "title", Name { user: u.clone() } }
                                    div { class: "sub", "@{u.username}" }
                                }
                                span { class: if picked { "pick on" } else { "pick" }, Icon { name: "check" } }
                            }
                        }
                    }
                }
            }
        }
    }
}
