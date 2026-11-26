//! Sign up and in, conversation list and new conversation.

use crate::{
    api, js,
    ui::{self, Avatar, Icon},
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

/// Profile photo of the other person in a one-to-one conversation.
pub fn photo(conversation: &Conversation, me: i64) -> Option<String> {
    if conversation.is_group {
        return None;
    }
    conversation.members.iter().find(|u| u.id != me).and_then(|u| u.avatar.clone())
}

fn preview(c: &Conversation) -> String {
    match &c.last_message {
        None => "Aucun message".into(),
        Some(m) => ui::summary(&m.body, m.media.as_ref().map(|f| f.mime.as_str()), m.deleted),
    }
}

#[component]
pub fn Home() -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let name = ctx.session.read().as_ref().map(|s| s.user.display_name.clone()).unwrap_or_default();
    let username = ctx.session.read().as_ref().map(|s| s.user.username.clone()).unwrap_or_default();
    let avatar = ctx.session.read().as_ref().and_then(|s| s.user.avatar.clone());
    let conversations = ctx.conversations.read().clone();
    let mut people = ctx.people_tab;
    let unread: i64 = conversations.iter().map(|c| c.unread).sum();
    let wide = *ctx.wide.read();
    let open = match *ctx.screen.read() {
        Screen::Chat(id) => Some(id),
        _ => None,
    };
    let hint = if ctx.desktop {
        "Aucune discussion. Cliquez sur + ou trouvez vos amis dans Personnes."
    } else {
        "Aucune discussion. Touchez + ou trouvez vos amis dans Personnes."
    };

    rsx! {
        div { class: "screen",
            header {
                Avatar { name: name.clone(), photo: avatar, small: true }
                h2 { if people() { "Personnes" } else { "Discussions" } small { "@{username}" } }
                if wide {
                    button { class: "icon", title: "Nouvelle discussion", onclick: move |_| ctx.open(Screen::New),
                        Icon { name: "plus" } }
                }
                button { class: "icon", title: "Réglages", onclick: move |_| ctx.open(Screen::Settings), Icon { name: "settings" } }
            }
            if people() {
                People {}
            } else {
            div { class: "list",
                if conversations.is_empty() {
                    div { class: "empty", "{hint}" }
                }
                for c in conversations {
                    button { key: "{c.id}", class: if open == Some(c.id) { "row active" } else { "row" },
                        onclick: move |_| ctx.open(Screen::Chat(c.id)),
                        Avatar { name: title(&c, me), photo: photo(&c, me) }
                        div { class: "main",
                            div { class: "title", "{title(&c, me)}" }
                            div { class: "sub", "{preview(&c)}" }
                        }
                        div { class: "side",
                            if let Some(m) = &c.last_message {
                                span { "{ui::short(m.created_at)}" }
                            }
                            if c.unread > 0 {
                                span { class: "badge", "{c.unread}" }
                            }
                        }
                    }
                }
            }
            if !wide {
                button { class: "fab", onclick: move |_| ctx.open(Screen::New), Icon { name: "plus" } }
            }
            }
            nav { class: "tabs",
                button { class: if people() { "tab" } else { "tab on" }, onclick: move |_| people.set(false),
                    Icon { name: "chat" }
                    span { "Discussions" }
                    if unread > 0 {
                        span { class: "badge", "{unread}" }
                    }
                }
                button { class: if people() { "tab on" } else { "tab" }, onclick: move |_| people.set(true),
                    Icon { name: "users" }
                    span { "Personnes" }
                }
            }
        }
    }
}

/// Opens the conversation, creating it when it does not exist yet.
async fn start_conversation(ctx: Ctx, body: NewConversation) {
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

/// Every member, to find friends.
#[component]
fn People() -> Element {
    let ctx = use_context::<Ctx>();
    let mut query = use_signal(String::new);
    let mut users = use_signal(|| None::<Vec<User>>);
    let mut busy = use_signal(|| false);

    let load = move |q: String| {
        spawn(async move {
            let encoded = String::from(js_sys::encode_uri_component(&q));
            match api::get::<Vec<User>>(&ctx.token(), &format!("/api/users?q={encoded}")).await {
                // Ignore answers to an outdated query.
                Ok(list) if *query.peek() == q => users.set(Some(list)),
                Ok(_) => {}
                Err(e) => ctx.fail(e),
            }
        });
    };
    use_hook(move || load(String::new()));

    rsx! {
        div { class: "pad",
            input { class: "field", placeholder: "Rechercher une personne…", autocapitalize: "none", value: "{query}",
                oninput: move |e| {
                    let q = e.value();
                    query.set(q.clone());
                    load(q.trim().to_string());
                },
            }
        }
        div { class: "list",
            if let Some(list) = users() {
                if list.is_empty() {
                    div { class: "empty", "Aucune personne trouvée." }
                }
                for u in list.clone() {
                    button { key: "{u.id}", class: "row", disabled: busy(),
                        onclick: {
                            let id = u.id;
                            move |_| {
                                busy.set(true);
                                spawn(async move {
                                    let body = NewConversation { member_ids: vec![id], name: None };
                                    start_conversation(ctx, body).await;
                                    if let Ok(mut b) = busy.try_write() {
                                        *b = false;
                                    }
                                });
                            }
                        },
                        Avatar { name: u.display_name.clone(), photo: u.avatar.clone() }
                        div { class: "main",
                            div { class: "title", "{u.display_name}" }
                            div { class: "sub", "@{u.username}" }
                        }
                    }
                }
            } else {
                div { class: "empty", "Chargement…" }
            }
        }
    }
}

#[component]
pub fn NewChat() -> Element {
    let ctx = use_context::<Ctx>();
    let mut results = use_signal(Vec::<User>::new);
    let mut chosen = use_signal(Vec::<User>::new);
    let mut group = use_signal(String::new);
    let mut query = use_signal(String::new);
    // A keyboard is at hand on desktop; on a phone it would cover the screen.
    use_hook(move || {
        if ctx.desktop {
            js::focus_input("new-search");
        }
    });

    let search = move |event: FormEvent| {
        let q = event.value();
        query.set(q.clone());
        spawn(async move {
            let encoded = String::from(js_sys::encode_uri_component(&q));
            if let Ok(list) = api::get(&ctx.token(), &format!("/api/users?q={encoded}")).await {
                // Ignore answers to an outdated query.
                if *query.peek() == q {
                    results.set(list);
                }
            }
        });
    };

    let create = move |_| {
        let is_group = chosen.read().len() > 1;
        let body = NewConversation {
            member_ids: chosen.read().iter().map(|u| u.id).collect(),
            name: is_group.then(|| group()),
        };
        spawn(start_conversation(ctx, body));
    };

    rsx! {
        div { class: "screen",
            header {
                button { class: "icon", onclick: move |_| js::pop_screen(), Icon { name: "back" } }
                h2 { "Nouvelle discussion" }
            }
            div { class: "pad",
                input { id: "new-search", class: "field", placeholder: "Rechercher un identifiant…", autocapitalize: "none", oninput: search }
                if !chosen.read().is_empty() {
                    div { class: "chips",
                        for u in chosen() {
                            button { key: "{u.id}", class: "chip",
                                onclick: move |_| chosen.write().retain(|x| x.id != u.id),
                                "{u.display_name} ✕"
                            }
                        }
                    }
                }
                if chosen.read().len() > 1 {
                    input { class: "field", placeholder: "Nom du groupe", value: "{group}",
                        oninput: move |e| group.set(e.value()) }
                }
                if !chosen.read().is_empty() {
                    button { class: "primary", onclick: create,
                        if chosen.read().len() > 1 { "Créer le groupe" } else { "Discuter" }
                    }
                }
            }
            div { class: "list",
                for u in results() {
                    button { key: "{u.id}", class: "row",
                        onclick: {
                            let u = u.clone();
                            move |_| {
                                if !chosen.read().iter().any(|x| x.id == u.id) {
                                    chosen.write().push(u.clone());
                                }
                            }
                        },
                        Avatar { name: u.display_name.clone(), photo: u.avatar.clone() }
                        div { class: "main",
                            div { class: "title", "{u.display_name}" }
                            div { class: "sub", "@{u.username}" }
                        }
                    }
                }
            }
        }
    }
}
