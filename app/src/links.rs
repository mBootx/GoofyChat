//! SelfHost-hub accounts linked to Goofy Chat: goofysound (Navidrome) and Photos (FileBrowser).

use crate::{api, ui::Icon, Ctx, Screen};
use dioxus::prelude::*;
use proto::{LinkAccount, Links};

/// French message for a failed link attempt.
fn reason(failure: api::Failure, service: &str) -> String {
    match failure.status {
        // The server already explains wrong passwords and conflicts in French.
        400 | 409 if !failure.message.starts_with("Erreur ") => failure.message,
        400 => format!("Identifiant ou mot de passe {service} incorrect."),
        409 => format!("Ce compte {service} est déjà lié à un autre compte Goofy Chat."),
        404 | 501 => "Ce serveur ne prend pas encore en charge SelfHost-hub.".into(),
        502..=504 => format!("{service} ne répond pas. Réessayez dans un instant."),
        _ => failure.message,
    }
}

/// Settings section listing the linked services.
#[component]
pub fn LinkedAccounts() -> Element {
    let ctx = use_context::<Ctx>();
    let mut links = use_signal(|| None::<Links>);
    let mut unsupported = use_signal(|| false);

    use_hook(move || {
        spawn(async move {
            match api::get::<Links>(&ctx.token(), "/api/links").await {
                Ok(found) => links.set(Some(found)),
                Err(f) if f.status == 401 => ctx.fail(f),
                Err(f) if f.status == 404 => unsupported.set(true),
                // Offline: show both services as unlinked, linking will tell.
                Err(_) => links.set(Some(Links::default())),
            }
        });
    });

    let current = links();
    rsx! {
        h3 { "SelfHost-hub" }
        div { class: "hint", "Le serveur garde cette connexion pour aller chercher votre musique et vos photos." }
        if unsupported() {
            div { class: "hint", "Ce serveur ne prend pas encore en charge SelfHost-hub." }
        } else if let Some(found) = current {
            div { class: "links",
                LinkCard { kind: "navidrome", service: "goofysound", icon: "music",
                    description: "Partagez des morceaux et montrez ce que vous écoutez.",
                    linked: found.navidrome.clone(), links }
                LinkCard { kind: "photos", service: "Photos", icon: "image",
                    description: "Envoyez les photos sauvegardées par SelfHost-hub.",
                    linked: found.photos.clone(), links }
            }
        } else {
            div { class: "hint", "Chargement…" }
        }
    }
}

#[component]
fn LinkCard(
    kind: &'static str,
    service: &'static str,
    icon: &'static str,
    description: &'static str,
    linked: Option<String>,
    links: Signal<Option<Links>>,
) -> Element {
    let ctx = use_context::<Ctx>();
    let mut open = use_signal(|| false);
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| false);
    let mut confirm = use_signal(|| false);
    let path = format!("/api/links/{kind}");

    let submit = {
        let path = path.clone();
        move |e: FormEvent| {
            e.prevent_default();
            if busy() {
                return;
            }
            let body = LinkAccount { username: username().trim().to_string(), password: password() };
            if body.username.is_empty() || body.password.is_empty() {
                error.set("Saisissez l'identifiant et le mot de passe.".into());
                return;
            }
            busy.set(true);
            error.set(String::new());
            let path = path.clone();
            spawn(async move {
                let result = api::post::<_, Links>(&ctx.token(), &path, &body).await;
                busy.set(false);
                match result {
                    Ok(found) => {
                        password.set(String::new());
                        open.set(false);
                        links.set(Some(found));
                        ctx.notify(format!("Compte {service} lié."));
                    }
                    Err(f) if f.status == 401 => ctx.fail(f),
                    Err(f) => error.set(reason(f, service)),
                }
            });
        }
    };

    let unlink = move |_| {
        if !confirm() {
            confirm.set(true);
            return;
        }
        confirm.set(false);
        busy.set(true);
        let path = path.clone();
        spawn(async move {
            let result = api::delete(&ctx.token(), &path).await;
            busy.set(false);
            match result {
                Ok(()) => {
                    if let Some(current) = links.write().as_mut() {
                        if kind == "navidrome" {
                            current.navidrome = None;
                        } else {
                            current.photos = None;
                        }
                    }
                    ctx.notify(format!("Compte {service} délié."));
                }
                Err(f) => ctx.fail(f),
            }
        });
    };

    rsx! {
        div { class: if linked.is_some() { "link-card linked" } else { "link-card" },
            div { class: "link-head",
                div { class: "link-icon", Icon { name: icon } }
                div { class: "link-text",
                    b { "{service}" }
                    if let Some(name) = &linked {
                        span { class: "link-status", span { class: "dot" } "Lié à {name}" }
                    } else {
                        span { "{description}" }
                    }
                }
                if linked.is_some() {
                    button { class: if confirm() { "link danger-link" } else { "link" }, disabled: busy(), onclick: unlink,
                        if confirm() { "Confirmer" } else { "Délier" }
                    }
                } else if !open() {
                    button { class: "ghost small", onclick: move |_| { open.set(true); error.set(String::new()); }, "Lier" }
                }
            }
            if linked.is_none() && open() {
                form { class: "link-form", onsubmit: submit,
                    input { class: "field", placeholder: "Identifiant {service}", autocapitalize: "none",
                        autocomplete: "username", value: "{username}", oninput: move |e| username.set(e.value()) }
                    input { class: "field", r#type: "password", placeholder: "Mot de passe", autocomplete: "current-password",
                        value: "{password}", oninput: move |e| password.set(e.value()) }
                    if !error().is_empty() {
                        div { class: "error", "{error}" }
                    }
                    div { class: "link-actions",
                        button { class: "link", r#type: "button", onclick: move |_| { open.set(false); error.set(String::new()); },
                            "Annuler" }
                        button { class: "primary", r#type: "submit", disabled: busy(),
                            if busy() { "Vérification…" } else { "Lier le compte" }
                        }
                    }
                }
            }
        }
    }
}

/// Shown instead of a feature that needs a linked account.
#[component]
pub fn LinkNeeded(message: String, service: &'static str) -> Element {
    let ctx = use_context::<Ctx>();
    let text = if message.is_empty() { format!("Liez votre compte {service} dans les réglages.") } else { message };
    rsx! {
        div { class: "link-needed",
            div { class: "note-art", Icon { name: "lock" } }
            b { "Compte {service} non lié" }
            p { "{text}" }
            button { class: "primary", onclick: move |_| ctx.open(Screen::Settings), "Ouvrir les réglages" }
        }
    }
}
