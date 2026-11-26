mod api;
mod chat;
mod js;
mod screens;
mod settings;
mod ui;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use futures_channel::mpsc;
use futures_util::StreamExt;
use proto::{Conversation, Message, ServerEvent, Session, User};
use serde::Deserialize;
use wasm_bindgen::{prelude::Closure, JsValue};

fn main() {
    dioxus::launch(App);
}

#[derive(Clone, PartialEq)]
pub enum Screen {
    Home,
    New,
    Settings,
    Chat(i64),
}

#[derive(Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CallView {
    pub state: String,
    pub video: bool,
    pub peer: String,
    pub muted: bool,
    pub camera_off: bool,
    pub message: String,
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
    }

    pub async fn refresh(mut self) {
        match api::get::<Vec<Conversation>>(&self.token(), "/api/conversations").await {
            Ok(list) => {
                let open = self.screen.peek().clone();
                // The open conversation may be gone: left, or its other member deleted.
                let gone = matches!(open, Screen::Chat(id) if !list.iter().any(|c| c.id == id));
                self.conversations.set(list);
                if gone {
                    // Set first so a second refresh does not go back twice.
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

    pub fn open(mut self, screen: Screen) {
        if *self.screen.peek() == Screen::Home {
            js::push_screen();
        }
        self.messages.set(vec![]);
        self.typing.set(vec![]);
        self.screen.set(screen.clone());
        if let Screen::Chat(id) = screen {
            // Outlives the screen that asked for it.
            spawn_forever(self.load(id));
        }
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
                self.read(id).await;
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
                self.typing.write().retain(|t| t.0 != sender);
                self.push_message(message);
                if open {
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
            ServerEvent::Call { .. } | ServerEvent::Pong => {}
        }
    }
}

/// A JavaScript function that forwards strings into a channel read by Rust.
fn channel() -> (JsValue, mpsc::UnboundedReceiver<String>) {
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
    });
    let mut call = use_signal(CallView::default);

    // Back button and call state, wired once.
    use_hook(move || {
        let (callback, mut back) = channel();
        js::on_back(&callback);
        let mut screen = ctx.screen;
        spawn(async move {
            while back.next().await.is_some() {
                screen.set(Screen::Home);
                if ctx.session.peek().is_some() {
                    ctx.refresh().await;
                }
            }
        });
        let (callback, mut states) = channel();
        js::on_call_state(&callback);
        spawn(async move {
            while let Some(json) = states.next().await {
                if let Ok(view) = serde_json::from_str::<CallView>(&json) {
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
    rsx! {
        if !signed_in {
            screens::Auth {}
        } else {
            match screen {
                Screen::Home => rsx! { screens::Home {} },
                Screen::New => rsx! { screens::NewChat {} },
                Screen::Settings => rsx! { settings::Settings {} },
                Screen::Chat(id) => rsx! { chat::Chat { key: "{id}", id } },
            }
        }
        chat::CallOverlay { call: call.read().clone() }
        if !toast.is_empty() {
            div { class: "toast", "{toast}" }
        }
    }
}
