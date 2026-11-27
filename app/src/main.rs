mod activity;
mod api;
mod call;
mod camera;
mod chat;
mod friends;
mod home;
mod links;
mod music;
mod nav;
mod photos;
mod js;
mod profile;
mod screens;
mod settings;
mod stories;
mod theme;
mod ui;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use futures_channel::mpsc;
use futures_util::StreamExt;
use proto::{Conversation, Friend, FriendRequests, Message, ServerEvent, Session, StoryReel, User};
use serde::Deserialize;
use std::collections::HashSet;
use wasm_bindgen::{prelude::Closure, JsValue};

fn main() {
    dioxus::launch(App);
}

/// Tab shown at start and after signing out (Accueil).
const DEFAULT_TAB: Tab = Tab::Home;

/// The four tabs of the mobile home (`nav::MobileHome`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Home,
    Messages,
    Activity,
    Me,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Screen {
    /// Root. Narrow: the current tab (`Ctx::tab`). Wide: the main pane's home.
    Home,
    New,
    Settings,
    Chat(i64),
    /// A user's profile, full screen (own id = own profile as others see it).
    Profile(i64),
    EditProfile,
    Friends,
    StoryArchive,
}

/// Full-screen layers above every screen and the tab bar. Opened with
/// `Ctx::open_overlay`, closed with `Ctx::close_overlay` (or back / Escape).
#[derive(Clone, PartialEq, Debug)]
pub enum Overlay {
    /// Story viewer, starting at this author's reel (first unseen story, else the first).
    Stories { user_id: i64 },
    /// Capture or pick, preview, publish.
    StoryComposer,
    /// Snapchat-style camera of a conversation.
    Camera { conversation_id: i64 },
    /// Full-screen viewer for photos/videos exchanged with a user.
    Media { user_id: i64, message_id: i64 },
}

/// A newer desktop release, found by the desktop shell.
#[derive(Clone, Deserialize, PartialEq)]
pub struct Update {
    pub version: String,
    pub url: String,
}

/// State shared by every screen.
#[derive(Clone, Copy)]
pub struct Ctx {
    pub session: Signal<Option<Session>>,
    pub screen: Signal<Screen>,
    pub conversations: Signal<Vec<Conversation>>,
    /// Messages of the open conversation.
    pub messages: Signal<Vec<Message>>,
    pub toast: Signal<String>,
    /// People this user blocked.
    pub blocked: Signal<Vec<User>>,
    /// Who is typing in the open conversation, with the time they last did.
    pub typing: Signal<Vec<(i64, f64)>>,
    /// Three-pane desktop layout.
    pub wide: Signal<bool>,
    /// Running inside the Windows (desktop) app.
    pub desktop: bool,
    pub update: Signal<Option<Update>>,
    /// What visible members are listening to on goofysound.
    pub activities: Signal<Vec<proto::Activity>>,
    // ---- 0.4.0: navigation and social state (contract §7.1) ----
    /// Current tab of the mobile home.
    pub tab: Signal<Tab>,
    /// Narrow layout: the screens below the current one, oldest first.
    pub back_stack: Signal<Vec<Screen>>,
    pub overlay: Signal<Option<Overlay>>,
    pub friends: Signal<Vec<Friend>>,
    pub requests: Signal<FriendRequests>,
    /// Ids of the friends who are online (`friends::on_presence` keeps it current).
    pub online: Signal<HashSet<i64>>,
    /// Own reel first, then friends' reels (`stories::load`, `on_story`).
    pub stories: Signal<Vec<StoryReel>>,
    /// Desktop right pane shown; persisted with `music::pref("side.open")`.
    pub side_open: Signal<bool>,
}

impl Ctx {
    pub fn token(&self) -> String {
        self.session.peek().as_ref().map(|s| s.token.clone()).unwrap_or_default()
    }

    pub fn my_id(&self) -> i64 {
        self.session.peek().as_ref().map(|s| s.user.id).unwrap_or_default()
    }

    pub fn notify(mut self, text: impl Into<String>) {
        let text = text.into();
        self.toast.set(text.clone());
        spawn_forever(async move {
            gloo_timers::future::TimeoutFuture::new(3500).await;
            if *self.toast.peek() == text {
                self.toast.set(String::new());
            }
        });
    }

    /// Reports an error, signing out when the session is no longer valid.
    pub fn fail(self, failure: api::Failure) {
        if failure.status == 401 {
            self.sign_out();
        }
        self.notify(failure.message);
    }

    pub fn sign_out(mut self) {
        js::disconnect();
        js::logout_native();
        js::clear_session();
        self.session.set(None);
        self.screen.set(Screen::Home);
        self.conversations.set(vec![]);
        self.messages.set(vec![]);
        self.blocked.set(vec![]);
        self.typing.set(vec![]);
        self.activities.set(vec![]);
        self.tab.set(DEFAULT_TAB);
        self.back_stack.set(vec![]);
        self.overlay.set(None);
        self.friends.set(vec![]);
        self.requests.set(FriendRequests::default());
        self.online.set(HashSet::new());
        self.stories.set(vec![]);
        music::stop("");
    }

    pub async fn refresh(mut self) {
        match api::get::<Vec<Conversation>>(&self.token(), "/api/conversations").await {
            Ok(list) => {
                let open = self.screen.peek().clone();
                // The open conversation may be gone: left, or its other member deleted.
                let gone = matches!(open, Screen::Chat(id) if !list.iter().any(|c| c.id == id));
                self.conversations.set(list);
                if gone {
                    // Set first so a second refresh does not go back twice; the
                    // popstate handler then lands on the previous screen.
                    self.screen.set(Screen::Home);
                    js::pop_screen();
                }
            }
            Err(e) => self.fail(e),
        }
    }

    pub async fn load_blocked(mut self) {
        match api::get(&self.token(), "/api/blocks").await {
            Ok(list) => self.blocked.set(list),
            Err(e) => self.fail(e),
        }
    }

    /// Keeps the stored session in step with a profile change.
    pub fn set_user(mut self, user: User) {
        let Some(mut session) = self.session.peek().clone() else { return };
        session.user = user;
        if let Ok(json) = serde_json::to_string(&session) {
            js::save_session(&json);
        }
        self.session.set(Some(session));
    }

    // ---- navigation (contract §7.1) ----
    // Browser history is the source of truth for "back": every screen or overlay that can
    // be left with the back button / Android back has one history entry, and the
    // `popstate` handler in `App` (see `Ctx::popped`) moves the state accordingly.

    /// Shows a screen. Narrow: the current one goes on `back_stack` and a history entry is
    /// pushed. Wide: an entry is pushed only when leaving `Home`, so back always returns
    /// to `Home`.
    pub fn open(mut self, screen: Screen) {
        let current = self.screen.peek().clone();
        if current == screen {
            return;
        }
        if *self.wide.peek() {
            if current == Screen::Home {
                js::push_screen();
            }
        } else {
            self.back_stack.write().push(current);
            js::push_screen();
        }
        self.show(screen);
    }

    /// Sets the screen, clearing the open conversation's state and loading a chat.
    fn show(mut self, screen: Screen) {
        self.messages.set(vec![]);
        self.typing.set(vec![]);
        self.screen.set(screen.clone());
        if let Screen::Chat(id) = screen {
            // Outlives the screen that asked for it.
            spawn_forever(self.load(id));
        }
    }

    /// Goes back one step (the `popstate` handler does the rest).
    pub fn back(self) {
        js::pop_screen();
    }

    /// Narrow layout: switches the tab of the mobile home and returns to it.
    pub fn go_tab(mut self, tab: Tab) {
        self.tab.set(tab);
        if *self.screen.peek() != Screen::Home {
            self.back_stack.set(vec![]);
            self.show(Screen::Home);
        }
    }

    /// Shows a full-screen overlay above everything (one history entry).
    pub fn open_overlay(mut self, overlay: Overlay) {
        if self.overlay.peek().is_none() {
            js::push_screen();
        }
        self.overlay.set(Some(overlay));
    }

    /// Closes the open overlay by going back in history (the `popstate` handler clears it).
    pub fn close_overlay(self) {
        if self.overlay.peek().is_some() {
            js::pop_screen();
        }
    }

    /// Shows or hides the desktop right pane, remembered on this device.
    pub fn set_side_open(mut self, open: bool) {
        self.side_open.set(open);
        music::set_pref("side.open", if open { "1" } else { "0" });
    }

    /// The browser went back (`popstate`): close the overlay, else leave the screen.
    async fn popped(mut self) {
        if self.overlay.peek().is_some() {
            self.overlay.set(None);
            return;
        }
        if *self.wide.peek() {
            self.back_stack.set(vec![]);
            self.show(Screen::Home);
        } else {
            let previous = self.back_stack.write().pop().unwrap_or(Screen::Home);
            self.show(previous);
        }
        if self.session.peek().is_some() {
            self.refresh().await;
        }
    }

    pub fn is_friend(&self, id: i64) -> bool {
        self.friends.peek().iter().any(|f| f.user.id == id)
    }

    /// The friend with this user id (does not subscribe the caller).
    pub fn friend(&self, id: i64) -> Option<Friend> {
        self.friends.peek().iter().find(|f| f.user.id == id).cloned()
    }

    /// Whether the user is online (subscribes the caller to presence changes).
    pub fn is_online(&self, id: i64) -> bool {
        self.online.read().contains(&id)
    }

    pub async fn load(mut self, id: i64) {
        let path = format!("/api/conversations/{id}/messages");
        match api::get::<Vec<Message>>(&self.token(), &path).await {
            Ok(list) => {
                if *self.screen.peek() != Screen::Chat(id) {
                    return;
                }
                self.messages.set(list);
                js::scroll_bottom("messages");
                if js::is_active() {
                    self.read(id).await;
                } else {
                    self.refresh().await;
                }
            }
            Err(e) => self.fail(e),
        }
    }

    async fn read(self, id: i64) {
        let _ = api::post_empty(&self.token(), &format!("/api/conversations/{id}/read")).await;
        self.refresh().await;
    }

    /// Adds a message to the open conversation, ignoring duplicates.
    pub fn push_message(mut self, message: Message) {
        if *self.screen.peek() != Screen::Chat(message.conversation_id)
            || self.messages.peek().iter().any(|m| m.id == message.id)
        {
            return;
        }
        self.messages.write().push(message);
        js::scroll_bottom("messages");
    }

    /// Applies a reaction or a deletion to the open conversation.
    pub fn update_message(mut self, updated: Message) {
        if *self.screen.peek() != Screen::Chat(updated.conversation_id) {
            return;
        }
        for message in self.messages.write().iter_mut() {
            if message.id == updated.id {
                *message = updated.clone();
            } else if updated.deleted {
                if let Some(quote) = message.reply.as_mut().filter(|q| q.id == updated.id) {
                    quote.deleted = true;
                    quote.body.clear();
                }
            }
        }
    }

    fn typing_from(mut self, user_id: i64) {
        let now = js_sys::Date::now();
        self.typing.write().retain(|t| t.0 != user_id);
        self.typing.write().push((user_id, now));
        spawn_forever(async move {
            gloo_timers::future::TimeoutFuture::new(5000).await;
            // A later keystroke has a later time and keeps the indicator.
            if self.typing.peek().contains(&(user_id, now)) {
                self.typing.write().retain(|t| t.0 != user_id);
            }
        });
    }

    async fn handle(mut self, event: ServerEvent) {
        match event {
            ServerEvent::Ready => {
                // (Re)connected: catch up on anything missed.
                self.load_blocked().await;
                activity::load(self).await;
                friends::load(self).await;
                stories::load(self).await;
                let open = self.screen.peek().clone();
                match open {
                    Screen::Chat(id) => {
                        self.refresh().await;
                        self.load(id).await
                    }
                    _ => self.refresh().await,
                }
            }
            ServerEvent::Message { message } => {
                let id = message.conversation_id;
                let open = *self.screen.peek() == Screen::Chat(id);
                let sender = message.sender.id;
                if self.desktop && sender != self.my_id() {
                    chat::announce(self, &message);
                }
                // Optional sound (Apparence › Sons) when the message is not in view.
                if sender != self.my_id() && !(open && js::is_active()) {
                    js::message_sound();
                }
                self.typing.write().retain(|t| t.0 != sender);
                self.push_message(message);
                // A window in the tray or behind others has not shown the message.
                if open && js::is_active() {
                    self.read(id).await;
                } else {
                    self.refresh().await;
                }
            }
            ServerEvent::Update { message } => {
                let deleted = message.deleted;
                self.update_message(message);
                // Previews and unread counts only change on deletion.
                if deleted {
                    self.refresh().await;
                }
            }
            ServerEvent::Conversation { id } => {
                self.load_blocked().await;
                self.refresh().await;
                // Members or their profiles changed.
                if *self.screen.peek() == Screen::Chat(id) {
                    self.load(id).await;
                }
            }
            ServerEvent::Read { conversation_id, mark } => {
                if let Some(c) = self.conversations.write().iter_mut().find(|c| c.id == conversation_id) {
                    c.reads.retain(|r| r.user_id != mark.user_id);
                    c.reads.push(mark);
                };
            }
            ServerEvent::Typing { conversation_id, user_id } => {
                if *self.screen.peek() == Screen::Chat(conversation_id) {
                    self.typing_from(user_id);
                }
            }
            ServerEvent::Activity { user_id, activity } => activity::apply(self, user_id, activity),
            ServerEvent::Call { .. } | ServerEvent::Pong => {}
            // 0.4.0 events: the handlers live in their owners' files (contract §7.1).
            ServerEvent::Friends { notice } => {
                friends::on_friends(self, notice).await;
                stories::load(self).await;
            }
            ServerEvent::Presence { user_id, online } => friends::on_presence(self, user_id, online),
            ServerEvent::UserChanged { user } => friends::on_user_changed(self, user),
            ServerEvent::Story { story, user } => stories::on_story(self, story, user),
            ServerEvent::StoryGone { id, user_id } => stories::on_story_gone(self, id, user_id),
            // Anything a newer server adds.
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    /// The window came back to the front: the open conversation is now seen.
    async fn activated(self) {
        if let Screen::Chat(id) = *self.screen.peek() {
            self.read(id).await;
        }
    }
}

/// A JavaScript function that forwards strings into a channel read by Rust.
pub fn channel() -> (JsValue, mpsc::UnboundedReceiver<String>) {
    let (tx, rx) = mpsc::unbounded::<String>();
    let closure = Closure::<dyn FnMut(JsValue)>::new(move |value: JsValue| {
        let _ = tx.unbounded_send(value.as_string().unwrap_or_default());
    });
    (closure.into_js_value(), rx)
}

#[component]
fn App() -> Element {
    let ctx = use_context_provider(|| Ctx {
        session: Signal::new(serde_json::from_str(&js::load_session()).ok()),
        screen: Signal::new(Screen::Home),
        conversations: Signal::new(vec![]),
        messages: Signal::new(vec![]),
        toast: Signal::new(String::new()),
        blocked: Signal::new(vec![]),
        typing: Signal::new(vec![]),
        wide: Signal::new(js::is_wide()),
        desktop: js::is_desktop(),
        update: Signal::new(None),
        activities: Signal::new(vec![]),
        tab: Signal::new(DEFAULT_TAB),
        back_stack: Signal::new(vec![]),
        overlay: Signal::new(None),
        friends: Signal::new(vec![]),
        requests: Signal::new(FriendRequests::default()),
        online: Signal::new(HashSet::new()),
        stories: Signal::new(vec![]),
        side_open: Signal::new(music::pref("side.open") != "0"),
    });
    let mut call = use_signal(call::CallView::default);

    // Desktop: layout width, Escape, window focus, updates and the unread count.
    use_hook(move || {
        let (callback, mut changes) = channel();
        js::on_wide(&callback);
        let mut wide = ctx.wide;
        spawn(async move {
            while let Some(value) = changes.next().await {
                wide.set(!value.is_empty());
            }
        });
        let (callback, mut keys) = channel();
        js::on_escape(&callback);
        spawn(async move {
            while keys.next().await.is_some() {
                // An overlay closes first. The conversation handles its own sheets.
                if ctx.overlay.peek().is_some() {
                    ctx.close_overlay();
                } else if matches!(
                    *ctx.screen.peek(),
                    Screen::New | Screen::Settings | Screen::Profile(_) | Screen::EditProfile | Screen::Friends | Screen::StoryArchive
                ) {
                    js::pop_screen();
                }
            }
        });
        if !ctx.desktop {
            return;
        }
        let (callback, mut focus) = channel();
        js::on_active(&callback);
        spawn(async move {
            while focus.next().await.is_some() {
                if ctx.session.peek().is_some() {
                    ctx.activated().await;
                }
            }
        });
        let mut update = ctx.update;
        spawn_forever(async move {
            loop {
                let found = js::check_update().await.as_string().unwrap_or_default();
                if let Ok(found) = serde_json::from_str::<Update>(&found) {
                    update.set(Some(found));
                }
                gloo_timers::future::TimeoutFuture::new(6 * 3600 * 1000).await;
            }
        });
    });
    use_effect(move || {
        let unread: i64 = ctx.conversations.read().iter().map(|c| c.unread).sum();
        if ctx.desktop {
            js::set_unread(unread as f64);
        }
    });

    // Back button (popstate): closes an overlay or leaves the screen, see `Ctx::popped`.
    use_hook(move || {
        let (callback, mut back) = channel();
        js::on_back(&callback);
        spawn(async move {
            while back.next().await.is_some() {
                ctx.popped().await;
            }
        });
    });

    // Call state, wired once.
    use_hook(move || {
        let (callback, mut states) = channel();
        js::on_call_state(&callback);
        spawn(async move {
            while let Some(json) = states.next().await {
                if let Ok(view) = serde_json::from_str::<call::CallView>(&json) {
                    if !view.message.is_empty() {
                        ctx.notify(view.message.clone());
                    }
                    call.set(view);
                }
            }
        });
    });

    // Live connection, tied to the session. A profile change must not reconnect,
    // which would end a call, so only the identity is watched.
    let identity = use_memo(move || ctx.session.read().as_ref().map(|s| (s.token.clone(), s.user.id)));
    use_effect(move || {
        let Some((token, user_id)) = identity() else { return };
        let (callback, mut events) = channel();
        js::connect(api::SERVER, &token, user_id as f64, &callback);
        spawn_forever(async move {
            while let Some(json) = events.next().await {
                if let Ok(event) = serde_json::from_str::<ServerEvent>(&json) {
                    ctx.handle(event).await;
                }
            }
        });
        spawn(ctx.refresh());
    });

    let signed_in = ctx.session.read().is_some();
    let screen = ctx.screen.read().clone();
    let toast = ctx.toast.read().clone();
    let wide = *ctx.wide.read();
    let update = ctx.update.read().clone();
    let overlay = ctx.overlay.read().clone();
    let side_open = *ctx.side_open.read();
    rsx! {
        div { class: if wide { "app wide" } else { "app" },
            if !signed_in {
                screens::Auth {}
            } else if wide {
                // Three panes: navigation, main (with desktop call stage), then profile / members / activity.
                div { class: "split",
                    aside { class: "side-pane", nav::SideNav {} }
                    section { class: "main-pane",
                        {show_screen(screen, true)}
                        call::CallOverlay { call, wide: true }
                    }
                    if side_open {
                        activity::SidePane {}
                    }
                }
            } else {
                {show_screen(screen, false)}
            }
            if signed_in {
                if let Some(overlay) = overlay {
                    div { class: "overlay",
                        match overlay {
                            Overlay::Stories { user_id } => rsx! { stories::StoryViewer { key: "{user_id}", user_id } },
                            Overlay::StoryComposer => rsx! { stories::StoryComposer {} },
                            Overlay::Camera { conversation_id } => rsx! { camera::Camera { key: "{conversation_id}", conversation_id } },
                            Overlay::Media { user_id, message_id } => rsx! { profile::MediaViewer { key: "{user_id}-{message_id}", user_id, message_id } },
                        }
                    }
                }
            }
            if let Some(found) = update {
                UpdateBanner { update: found }
            }
            if !(signed_in && wide) {
                call::CallOverlay { call, wide }
            }
            if !toast.is_empty() {
                div { class: "toast", "{toast}" }
            }
        }
    }
}

/// The screen of the main area: the whole window on a narrow layout, the middle pane on a
/// wide one. `Home` differs: the tab bar page (narrow) or the desktop home (wide).
fn show_screen(screen: Screen, wide: bool) -> Element {
    match screen {
        Screen::Home if wide => rsx! { home::DesktopHome {} },
        Screen::Home => rsx! { nav::MobileHome {} },
        Screen::New => rsx! { screens::NewChat {} },
        Screen::Settings => rsx! { settings::Settings {} },
        Screen::Chat(id) => rsx! { chat::Chat { key: "{id}", id } },
        Screen::Profile(user_id) => rsx! { profile::ProfileScreen { key: "{user_id}", user_id } },
        Screen::EditProfile => rsx! { profile::EditProfile {} },
        Screen::Friends => rsx! { friends::FriendsScreen {} },
        Screen::StoryArchive => rsx! { stories::StoryArchive {} },
    }
}

/// Offers a newer desktop release, then installs it and restarts.
#[component]
fn UpdateBanner(update: Update) -> Element {
    let mut ctx = use_context::<Ctx>();
    let mut busy = use_signal(|| false);
    let url = update.url.clone();
    let install = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        let url = url.clone();
        spawn(async move {
            // On success the app closes while the installer runs.
            if let Err(e) = js::install_update(&url).await {
                busy.set(false);
                let reason = e.as_string().unwrap_or_default();
                ctx.notify(format!("Échec de la mise à jour. {reason}"));
            }
        });
    };
    rsx! {
        div { class: "update",
            div { class: "update-icon", ui::Icon { name: "download" } }
            div { class: "update-text",
                b { "Mise à jour disponible" }
                span {
                    if busy() { "Téléchargement de la version {update.version}…" } else { "La version {update.version} de Goofy Chat est prête." }
                }
            }
            if !busy() {
                button { class: "ghost", onclick: move |_| ctx.update.set(None), "Plus tard" }
            }
            button { class: "primary", disabled: busy(), onclick: install,
                if busy() { "Installation…" } else { "Installer" }
            }
        }
    }
}
