//! Navigation shell: the mobile home with its tab bar, and the desktop left pane with its
//! user panel (contract 0.4.0 §7.2, §7.3).

use crate::{
    activity, home, profile, screens,
    ui::{Icon, Name, UserAvatar},
    Ctx, Screen, Tab,
};
use dioxus::prelude::*;

/// `Screen::Home` on a narrow window: the current tab above the tab bar.
#[component]
pub fn MobileHome() -> Element {
    let tab = *use_context::<Ctx>().tab.read();
    rsx! {
        div { class: "mobile-home",
            div { class: "tab-content", key: "{tab:?}",
                match tab {
                    Tab::Home => rsx! { home::HomeTab {} },
                    Tab::Messages => rsx! { screens::Conversations {} },
                    Tab::Activity => rsx! { activity::ActivityPage {} },
                    Tab::Me => rsx! { profile::MyProfile {} },
                }
            }
            TabBar {}
        }
    }
}

/// Accueil, Messages (unread badge), Activité, Profil (requests badge).
#[component]
pub fn TabBar() -> Element {
    let ctx = use_context::<Ctx>();
    let current = *ctx.tab.read();
    let unread: i64 = ctx.conversations.read().iter().map(|c| c.unread).sum();
    let requests = ctx.requests.read().incoming.len();
    let me = ctx.session.read().as_ref().map(|s| s.user.clone());
    let class = move |tab: Tab| if current == tab { "tabbar-item on" } else { "tabbar-item" };
    rsx! {
        nav { class: "tabbar",
            button { class: class(Tab::Home), onclick: move |_| ctx.go_tab(Tab::Home),
                Icon { name: "home" }
                span { "Accueil" }
            }
            button { class: class(Tab::Messages), onclick: move |_| ctx.go_tab(Tab::Messages),
                Icon { name: "chat" }
                span { "Messages" }
                if unread > 0 {
                    span { class: "badge", "{unread}" }
                }
            }
            button { class: class(Tab::Activity), onclick: move |_| ctx.go_tab(Tab::Activity),
                Icon { name: "activity" }
                span { "Activité" }
            }
            button { class: class(Tab::Me), onclick: move |_| ctx.go_tab(Tab::Me),
                if let Some(user) = me {
                    UserAvatar { user, small: true }
                } else {
                    Icon { name: "users" }
                }
                span { "Profil" }
                if requests > 0 {
                    span { class: "badge", "{requests}" }
                }
            }
        }
    }
}

/// The left pane of the desktop layout: Accueil, Amis, the conversations, the user panel.
#[component]
pub fn SideNav() -> Element {
    let ctx = use_context::<Ctx>();
    let screen = ctx.screen.read().clone();
    let requests = ctx.requests.read().incoming.len();
    let side_open = *ctx.side_open.read();
    let class = move |on: bool| if on { "nav-item on" } else { "nav-item" };
    rsx! {
        div { class: "side-nav",
            div { class: "nav-top",
                button { class: class(screen == Screen::Home), onclick: move |_| go_home(ctx),
                    Icon { name: "home" }
                    span { "Accueil" }
                }
                button { class: class(screen == Screen::Friends), onclick: move |_| ctx.open(Screen::Friends),
                    Icon { name: "users" }
                    span { "Amis" }
                    if requests > 0 {
                        span { class: "badge", "{requests}" }
                    }
                }
            }
            div { class: "nav-section",
                span { "Messages" }
                button { class: "icon", title: "Nouvelle discussion", onclick: move |_| ctx.open(Screen::New), Icon { name: "plus" } }
            }
            screens::ConversationList {}
            UserPanel { side_open }
        }
    }
}

/// Back to the desktop home (its history entry is the one below the open screen).
fn go_home(ctx: Ctx) {
    if *ctx.screen.peek() != Screen::Home {
        ctx.back();
    }
}

/// Bottom of the side navigation, like Discord's: own avatar, name and username (opens the
/// own profile), right pane toggle and settings.
#[component]
fn UserPanel(side_open: bool) -> Element {
    let ctx = use_context::<Ctx>();
    let Some(me) = ctx.session.read().as_ref().map(|s| s.user.clone()) else { return rsx! {} };
    let id = me.id;
    rsx! {
        div { class: "user-panel",
            button { class: "user-panel-me", title: "Mon profil", onclick: move |_| ctx.open(Screen::Profile(id)),
                UserAvatar { user: me.clone(), small: true, online: true }
                div { class: "user-panel-names",
                    Name { user: me.clone() }
                    span { class: "user-panel-handle", "@{me.username}" }
                }
            }
            button { class: if side_open { "icon on" } else { "icon" },
                title: if side_open { "Masquer le panneau de droite" } else { "Afficher le panneau de droite" },
                onclick: move |_| ctx.set_side_open(!side_open),
                Icon { name: "activity" }
            }
            button { class: "icon", title: "Réglages", onclick: move |_| ctx.open(Screen::Settings), Icon { name: "settings" } }
        }
    }
}
