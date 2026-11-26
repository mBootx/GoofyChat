mod api;
mod chat;
mod js;
mod screens;
mod ui;

use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use futures_channel::mpsc;
use futures_util::StreamExt;
use proto::{Conversation, Message, ServerEvent, Session};
use serde::Deserialize;
use wasm_bindgen::{prelude::Closure, JsValue};

fn main() {
    dioxus::launch(App);
}

#[derive(Clone, PartialEq)]
pub enum Screen {
    Home,
    New,
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
    }

    pub async fn refresh(mut self) {
        match api::get(&self.token(), "/api/conversations").await {
            Ok(list) => self.conversations.set(list),
            Err(e) => self.fail(e),
        }
    }

    pub fn open(mut self, screen: Screen) {
        if *self.screen.peek() == Screen::Home {
            js::push_screen();
        }
        self.messages.set(vec![]);
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

    async fn handle(self, event: ServerEvent) {
        match event {
            ServerEvent::Ready => {
                // (Re)connected: catch up on anything missed.
                let open = self.screen.peek().clone();
                match open {
                    Screen::Chat(id) => self.load(id).await,
                    _ => self.refresh().await,
                }
            }
            ServerEvent::Message { message } => {
                let id = message.conversation_id;
                let open = *self.screen.peek() == Screen::Chat(id);
                self.push_message(message);
                if open {
                    self.read(id).await;
                } else {
                    self.refresh().await;
                }
            }
            ServerEvent::Conversation { .. } => self.refresh().await,
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

    // Live connection, tied to the session.
    use_effect(move || {
        let Some(session) = ctx.session.read().clone() else { return };
        let (callback, mut events) = channel();
        js::connect(api::SERVER, &session.token, session.user.id as f64, &callback);
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
                Screen::Chat(id) => rsx! { chat::Chat { key: "{id}", id } },
            }
        }
        chat::CallOverlay { call: call.read().clone() }
        if !toast.is_empty() {
            div { class: "toast", "{toast}" }
        }
    }
}
