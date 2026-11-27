//! Blocked people, appearance, sounds, SelfHost-hub links, desktop app, sign out and
//! account deletion (contract 0.4.0 §7.5; profile editing lives in `profile::EditProfile`).

use crate::{
    api, js,
    ui::{Icon, Name, UserAvatar},
    Ctx,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use proto::DeleteAccount;

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
    let mut password = use_signal(String::new);
    let mut confirm_delete = use_signal(|| false);
    let blocked = ctx.blocked.read().clone();
    let Some(user) = user else { return rsx! {} };

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
                button { class: "icon", title: "Retour", onclick: move |_| ctx.back(), Icon { name: "back" } }
                h2 { "Réglages" small { "@{user.username}" } }
            }
            div { class: "list pad",
                h3 { "Personnes bloquées" }
                if blocked.is_empty() {
                    div { class: "hint", "Personne." }
                }
                for u in blocked {
                    div { key: "{u.id}", class: "person",
                        UserAvatar { user: u.clone(), small: true }
                        span { Name { user: u.clone() } }
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
