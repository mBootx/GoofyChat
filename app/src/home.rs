//! Home: the mobile "Accueil" tab and the desktop main pane when nothing is open
//! (contract 0.4.0 §7.2, §7.3).

use crate::{
    stories,
    ui::{Icon, UserAvatar},
    Ctx, Screen, Tab,
};
use dioxus::prelude::*;

/// The mobile "Accueil" tab: header with own avatar, stories row, then the posts placeholder.
#[component]
pub fn HomeTab() -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.session.read().as_ref().map(|s| s.user.clone());
    rsx! {
        div { class: "screen home-tab",
            header {
                if let Some(user) = me {
                    button { class: "home-me", title: "Mon profil", onclick: move |_| ctx.go_tab(Tab::Me),
                        UserAvatar { user, small: true }
                    }
                }
                h2 { "Accueil" }
            }
            div { class: "home-scroll",
                stories::StoriesRow {}
                div { class: "home-body",
                    PostsPlaceholder {}
                }
            }
        }
    }
}

/// The main pane of the desktop layout when no conversation is open (former `Welcome`).
#[component]
pub fn DesktopHome() -> Element {
    let ctx = use_context::<Ctx>();
    let name = ctx.session.read().as_ref().map(|s| s.user.display_name.clone()).unwrap_or_default();
    let first = name.split_whitespace().next().unwrap_or_default().to_string();
    let requests = ctx.requests.read().incoming.len();
    let greeting = if first.is_empty() { "Bonjour !".to_string() } else { format!("Bonjour {first} !") };
    rsx! {
        div { class: "screen desktop-home",
            div { class: "desktop-home-inner",
                section { class: "home-stories-card",
                    stories::StoriesRow {}
                }
                PostsPlaceholder {}
                section { class: "home-welcome",
                    div { class: "home-welcome-head",
                        div { class: "home-welcome-logo", Icon { name: "chat" } }
                        div { class: "home-welcome-copy",
                            h1 { "{greeting}" }
                            p { "Choisissez une discussion à gauche, ou démarrez-en une nouvelle." }
                        }
                    }
                    div { class: "home-welcome-actions",
                        button { class: "primary", onclick: move |_| ctx.open(Screen::New),
                            Icon { name: "plus" } "Nouvelle discussion"
                        }
                        button { class: "ghost", onclick: move |_| ctx.open(Screen::Friends),
                            Icon { name: "users" } "Amis"
                            if requests > 0 {
                                span { class: "badge", "{requests}" }
                            }
                        }
                        button { class: "ghost", onclick: move |_| ctx.open(Screen::Settings),
                            Icon { name: "settings" } "Réglages"
                        }
                    }
                    div { class: "keys",
                        span { kbd { "Entrée" } " envoyer" }
                        span { kbd { "Échap" } " fermer" }
                        span { kbd { "Ctrl" } "+" kbd { "V" } " coller une image" }
                    }
                }
            }
        }
    }
}

/// Placeholder for future friend posts (no API calls).
#[component]
fn PostsPlaceholder() -> Element {
    rsx! {
        section { class: "posts-card",
            div { class: "posts-art", aria_hidden: "true",
                svg { view_box: "0 0 180 112",
                    // Back card
                    rect { x: "18", y: "18", width: "92", height: "72", rx: "12", class: "posts-art-back" }
                    // Front photo post card
                    rect { x: "48", y: "10", width: "114", height: "90", rx: "14", class: "posts-art-front" }
                    // Author header inside card
                    circle { cx: "66", cy: "26", r: "7", class: "posts-art-accent" }
                    line { x1: "79", y1: "23", x2: "114", y2: "23", class: "posts-art-line" }
                    line { x1: "79", y1: "29", x2: "100", y2: "29", class: "posts-art-line-soft" }
                    // Photo frame
                    rect { x: "58", y: "39", width: "94", height: "42", rx: "8", class: "posts-art-frame" }
                    circle { cx: "76", cy: "53", r: "5", class: "posts-art-accent" }
                    path { d: "M64 75l19-16 14 12 15-18 34 22", class: "posts-art-hill" }
                    // Footer lines
                    line { x1: "60", y1: "90", x2: "96", y2: "90", class: "posts-art-line-soft" }
                    circle { cx: "145", cy: "90", r: "4", class: "posts-art-accent" }
                }
            }
            div { class: "posts-text",
                h3 { "Publications" }
                p { "Bientôt ici : les photos et les nouvelles de vos amis." }
            }
        }
    }
}
