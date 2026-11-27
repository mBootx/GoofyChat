//! Profile, blocked people, sign out and account deletion.

use crate::{
    api, js,
    ui::{Avatar, Icon},
    Ctx,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use proto::{DeleteAccount, Profile, User};

async fn save(ctx: Ctx, profile: Profile) {
    match api::put::<_, User>(&ctx.token(), "/api/me", &profile).await {
        Ok(user) => {
            ctx.set_user(user);
            ctx.notify("Profil enregistré.");
        }
        Err(e) => ctx.fail(e),
    }
}

/// Start with Windows, version and updates. Only inside the desktop app.
#[component]
fn DesktopSettings() -> Element {
    let mut ctx = use_context::<Ctx>();
    let mut autostart = use_signal(|| false);
    let mut version = use_signal(String::new);
    let mut checking = use_signal(|| false);
    use_hook(move || {
        spawn(async move {
            autostart.set(js::get_autostart().await.as_bool().unwrap_or(false));
            version.set(js::app_version().await.as_string().unwrap_or_default());
        });
    });

    let toggle = move |_| {
        let wanted = !autostart();
        spawn(async move {
            match js::set_autostart(wanted).await {
                Ok(_) => autostart.set(wanted),
                Err(e) => ctx.notify(e.as_string().unwrap_or_else(|| "Réglage impossible.".into())),
            }
        });
    };

    let check = move |_| {
        if checking() {
            return;
        }
        checking.set(true);
        spawn(async move {
            let found = js::check_update().await.as_string().unwrap_or_default();
            checking.set(false);
            match serde_json::from_str::<crate::Update>(&found) {
                Ok(update) => ctx.update.set(Some(update)),
                Err(_) => ctx.notify("Goofy Chat est à jour."),
            }
        });
    };

    rsx! {
        h3 { "Application" }
        button { class: "switch-row", role: "switch", aria_checked: "{autostart}", onclick: toggle,
            div {
                div { "Lancer au démarrage de Windows" }
                div { class: "hint", "Goofy Chat démarre dans la zone de notification pour recevoir vos messages." }
            }
            span { class: if autostart() { "switch on" } else { "switch" } }
        }
        div { class: "hint", "Fermer la fenêtre garde Goofy Chat actif dans la zone de notification. Pour quitter, utilisez l'icône près de l'horloge." }
        div { class: "version-row",
            span { class: "hint", if version().is_empty() { "Goofy Chat" } else { "Goofy Chat {version}" } }
            button { class: "link", disabled: checking(), onclick: check,
                if checking() { "Recherche…" } else { "Rechercher une mise à jour" }
            }
        }
    }
}

#[component]
pub fn Settings() -> Element {
    let ctx = use_context::<Ctx>();
    let user = ctx.session.read().as_ref().map(|s| s.user.clone());
    let mut name = use_signal(|| ctx.session.peek().as_ref().map(|s| s.user.display_name.clone()).unwrap_or_default());
    let mut password = use_signal(String::new);
    let mut confirm_delete = use_signal(|| false);
    let blocked = ctx.blocked.read().clone();
    let Some(user) = user else { return rsx! {} };

    let current_name = move || ctx.session.peek().as_ref().map(|s| s.user.display_name.clone()).unwrap_or_default();

    let photo_chosen = move |_| {
        let display_name = current_name();
        // Keeps going if the user leaves the screen.
        spawn_forever(async move {
            match js::upload_from_input("avatar-file").await {
                Ok(value) => {
                    if let Some(id) = value.as_string().filter(|s| !s.is_empty()) {
                        save(ctx, Profile { display_name, avatar_id: Some(id), remove_avatar: false }).await;
                    }
                }
                Err(e) => ctx.notify(e.as_string().unwrap_or_else(|| "Échec de l'envoi.".into())),
            }
        });
    };

    let remove_photo = move |_| {
        let profile = Profile { display_name: current_name(), avatar_id: None, remove_avatar: true };
        spawn_forever(save(ctx, profile));
    };

    let save_name = move |_| {
        let profile = Profile { display_name: name(), avatar_id: None, remove_avatar: false };
        spawn_forever(save(ctx, profile));
    };

    let logout = move |_| {
        spawn_forever(async move {
            let _ = api::post_empty(&ctx.token(), "/api/logout").await;
            ctx.sign_out();
        });
    };

    let delete_account = move |_| {
        if password().is_empty() {
            ctx.notify("Saisissez votre mot de passe.");
            return;
        }
        if !confirm_delete() {
            confirm_delete.set(true);
            return;
        }
        let body = DeleteAccount { password: password() };
        spawn_forever(async move {
            match api::post_unit(&ctx.token(), "/api/account/delete", &body).await {
                Ok(()) => {
                    ctx.sign_out();
                    ctx.notify("Compte supprimé.");
                }
                Err(e) => ctx.fail(e),
            }
        });
    };

    rsx! {
        div { class: "screen",
            header {
                button { class: "icon", onclick: move |_| js::pop_screen(), Icon { name: "back" } }
                h2 { "Réglages" small { "@{user.username}" } }
            }
            div { class: "list pad",
                div { class: "profile",
                    Avatar { name: user.display_name.clone(), photo: user.avatar.clone() }
                    input { id: "avatar-file", class: "hidden", r#type: "file", accept: "image/*", onchange: photo_chosen }
                    button { class: "link", onclick: move |_| js::pick_file("avatar-file"), "Changer la photo" }
                    if user.avatar.is_some() {
                        button { class: "link", onclick: remove_photo, "Retirer la photo" }
                    }
                }
                input { class: "field", placeholder: "Nom affiché", value: "{name}",
                    oninput: move |e| name.set(e.value()) }
                button { class: "primary", onclick: save_name, "Enregistrer le nom" }

                h3 { "Personnes bloquées" }
                if blocked.is_empty() {
                    div { class: "hint", "Personne." }
                }
                for u in blocked {
                    div { key: "{u.id}", class: "person",
                        Avatar { name: u.display_name.clone(), photo: u.avatar.clone(), small: true }
                        span { "{u.display_name}" }
                        button { class: "link",
                            onclick: move |_| {
                                spawn_forever(async move {
                                    if let Err(e) = api::delete(&ctx.token(), &format!("/api/blocks/{}", u.id)).await {
                                        ctx.fail(e);
                                    }
                                    ctx.load_blocked().await;
                                });
                            },
                            "Débloquer"
                        }
                    }
                }

                crate::theme::Appearance {}
                crate::theme::SoundSettings {}

                crate::links::LinkedAccounts {}

                if ctx.desktop {
                    DesktopSettings {}
                }

                h3 { "Compte" }
                button { class: "link", onclick: logout, "Se déconnecter" }
                div { class: "hint", "Supprimer le compte efface définitivement vos messages, vos fichiers et vos discussions privées." }
                input { class: "field", r#type: "password", placeholder: "Mot de passe", value: "{password}",
                    oninput: move |e| { password.set(e.value()); confirm_delete.set(false); } }
                button { class: "primary danger", onclick: delete_account,
                    if confirm_delete() { "Confirmer la suppression" } else { "Supprimer mon compte" }
                }
            }
        }
    }
}
