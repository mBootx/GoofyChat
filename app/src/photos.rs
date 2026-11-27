//! "Envoyer depuis mes photos": the attach menu of the composer and a picker for the
//! photos and videos SelfHost-hub backed up to FileBrowser.

use crate::{api, chat, js, links::LinkNeeded, music, ui::Icon, Ctx, Overlay};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use proto::{ImportPhoto, Media, Message, NewMessage, Photo};
use serde::Deserialize;
use wasm_bindgen::JsValue;

/// At most this many photos per send.
const MAX: usize = 10;

/// The layer the composer opened above the conversation.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Panel {
    Attach,
    Photos,
    Music,
}

/// Whichever composer layer is open. `panel` and `replying` belong to the conversation.
#[component]
pub fn ComposerPanel(id: i64, panel: Signal<Option<Panel>>, replying: Signal<Option<Message>>) -> Element {
    let reply_to = replying.read().as_ref().map(|m| m.id);
    match panel() {
        None => rsx! {},
        Some(Panel::Attach) => rsx! { AttachMenu { id, panel } },
        Some(Panel::Photos) => rsx! { PhotoPicker { id, panel, replying } },
        Some(Panel::Music) => rsx! {
            music::MusicSheet { id, reply_to,
                on_close: move |sent: bool| {
                    panel.set(None);
                    if sent {
                        replying.set(None);
                    }
                },
            }
        },
    }
}

/// The clip button: the camera, a file of the device, or a backed-up photo.
#[component]
fn AttachMenu(id: i64, panel: Signal<Option<Panel>>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut camera = use_signal(|| false);
    use_hook(move || {
        spawn(async move {
            let found = crate::camera::camera_available().await.as_bool().unwrap_or(false);
            camera.set(found);
        })
    });
    rsx! {
        div { class: "backdrop", onclick: move |_| panel.set(None),
            div { class: "sheet attach-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "sheet-head",
                    div { class: "sheet-title", b { "Joindre" } }
                    button { class: "icon", title: "Fermer", onclick: move |_| panel.set(None), Icon { name: "hangup" } }
                }
                if camera() {
                    button { class: "attach-option",
                        onclick: move |_| {
                            panel.set(None);
                            ctx.open_overlay(Overlay::Camera { conversation_id: id });
                        },
                        div { class: "attach-icon camera", Icon { name: "camera" } }
                        div { class: "attach-text",
                            b { "Caméra" }
                            span { "Touchez pour une photo, maintenez pour une vidéo" }
                        }
                        Icon { name: "chevron" }
                    }
                }
                button { class: "attach-option",
                    onclick: move |_| {
                        panel.set(None);
                        // Still inside the click, so the browser opens the picker.
                        js::pick_file("file");
                    },
                    div { class: "attach-icon", Icon { name: "clip" } }
                    div { class: "attach-text",
                        b { "Fichier de l'appareil" }
                        span { "Photo, vidéo ou son de cet appareil" }
                    }
                    Icon { name: "chevron" }
                }
                button { class: "attach-option", onclick: move |_| panel.set(Some(Panel::Photos)),
                    div { class: "attach-icon photos", Icon { name: "image" } }
                    div { class: "attach-text",
                        b { "Mes photos SelfHost-hub" }
                        span { "Les photos et vidéos sauvegardées depuis votre téléphone" }
                    }
                    Icon { name: "chevron" }
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct Page {
    photos: Vec<Photo>,
    #[serde(default)]
    next: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Quick {
    All,
    Today,
    Week,
    Month,
    Year,
}

const QUICK: [(Quick, &str); 5] = [
    (Quick::All, "Toutes"),
    (Quick::Today, "Aujourd'hui"),
    (Quick::Week, "Cette semaine"),
    (Quick::Month, "Ce mois-ci"),
    (Quick::Year, "Cette année"),
];

const MONTHS: [&str; 12] = [
    "Janvier", "Février", "Mars", "Avril", "Mai", "Juin", "Juillet", "Août", "Septembre", "Octobre", "Novembre", "Décembre",
];

fn date(ms: f64) -> js_sys::Date {
    js_sys::Date::new(&JsValue::from_f64(ms))
}

/// "2026-11-26", local time.
fn iso(d: &js_sys::Date) -> String {
    format!("{:04}-{:02}-{:02}", d.get_full_year(), d.get_month() + 1, d.get_date())
}

/// from/to of a quick filter.
fn range(quick: Quick) -> (String, String) {
    let now = date(js_sys::Date::now());
    let today = iso(&now);
    let from = match quick {
        Quick::All => return (String::new(), String::new()),
        Quick::Today => today.clone(),
        Quick::Week => {
            // Weeks start on Monday.
            let back = (now.get_day() + 6) % 7;
            iso(&date(now.get_time() - back as f64 * 86_400_000.0))
        }
        Quick::Month => format!("{:04}-{:02}-01", now.get_full_year(), now.get_month() + 1),
        Quick::Year => format!("{:04}-01-01", now.get_full_year()),
    };
    (from, today)
}

fn month_label(ms: i64) -> String {
    let d = date(ms as f64);
    format!("{} {}", MONTHS[d.get_month() as usize % 12], d.get_full_year())
}

fn thumb_url(photo: &Photo) -> String {
    let encode = |v: &str| String::from(js_sys::encode_uri_component(v));
    format!("{}/api/photos/thumb?path={}&k={}", api::SERVER, encode(&photo.path), encode(&photo.key))
}

/// Full-screen grid of the backed-up photos, newest first.
#[component]
fn PhotoPicker(id: i64, panel: Signal<Option<Panel>>, replying: Signal<Option<Message>>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut photos = use_signal(Vec::<Photo>::new);
    let mut next = use_signal(|| None::<String>);
    let mut loaded = use_signal(|| false);
    let mut loading = use_signal(|| false);
    let mut locked = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut from = use_signal(String::new);
    let mut to = use_signal(String::new);
    let mut quick = use_signal(|| Some(Quick::All));
    let mut selected = use_signal(Vec::<Photo>::new);
    let mut progress = use_signal(|| None::<(usize, usize)>);
    let mut generation = use_signal(|| 0u32);

    // First page (reset) or the next one.
    let mut load = move |reset: bool| {
        if !reset && (*loading.peek() || next.peek().is_none()) {
            return;
        }
        let n = if reset { *generation.peek() + 1 } else { *generation.peek() };
        generation.set(n);
        let cursor = if reset { None } else { next.peek().clone() };
        if reset {
            photos.set(vec![]);
            next.set(None);
            loaded.set(false);
            error.set(String::new());
        }
        let mut query = vec![];
        if !from.peek().is_empty() {
            query.push(format!("from={}", from.peek()));
        }
        if !to.peek().is_empty() {
            query.push(format!("to={}", to.peek()));
        }
        if let Some(cursor) = cursor {
            query.push(format!("cursor={}", String::from(js_sys::encode_uri_component(&cursor))));
        }
        let path = if query.is_empty() { "/api/photos".to_string() } else { format!("/api/photos?{}", query.join("&")) };
        loading.set(true);
        spawn(async move {
            let result = api::get::<Page>(&ctx.token(), &path).await;
            // A newer filter replaced this list.
            if *generation.peek() != n {
                return;
            }
            loading.set(false);
            match result {
                Ok(page) => {
                    photos.write().extend(page.photos);
                    next.set(page.next);
                    loaded.set(true);
                }
                Err(f) if f.status == 403 => locked.set(f.message),
                Err(f) if f.status == 401 => ctx.fail(f),
                Err(f) => error.set(f.message),
            }
        });
    };
    use_hook(move || load(true));

    let mut choose_quick = move |q: Quick| {
        let (f, t) = range(q);
        from.set(f);
        to.set(t);
        quick.set(Some(q));
        load(true);
    };

    let mut toggle = move |photo: Photo| {
        if progress.peek().is_some() {
            return;
        }
        let mut list = selected.write();
        if let Some(i) = list.iter().position(|p| p.path == photo.path) {
            list.remove(i);
        } else if list.len() >= MAX {
            ctx.notify(format!("{MAX} photos au maximum par envoi."));
        } else {
            list.push(photo);
        }
    };

    let send = move |_| {
        let list = selected.peek().clone();
        if list.is_empty() || progress.peek().is_some() {
            return;
        }
        let total = list.len();
        let reply_to = replying.peek().as_ref().map(|m| m.id);
        replying.set(None);
        progress.set(Some((0, total)));
        // Keeps going if the picker or the conversation is closed.
        spawn_forever(async move {
            let mut failed = 0;
            for (i, photo) in list.into_iter().enumerate() {
                let body = ImportPhoto { path: photo.path.clone() };
                match api::post::<_, Media>(&ctx.token(), "/api/photos/import", &body).await {
                    Ok(media) => {
                        let message = NewMessage {
                            body: String::new(),
                            media_id: Some(media.id),
                            reply_to: if i == 0 { reply_to } else { None },
                            track_id: None,
                        };
                        if !chat::post_message(ctx, id, message).await {
                            failed += 1;
                        }
                    }
                    Err(f) if f.status == 401 => {
                        ctx.fail(f);
                        return;
                    }
                    Err(f) => {
                        failed += 1;
                        ctx.notify(format!("{} : {}", photo.name, f.message));
                    }
                }
                if let Ok(mut p) = progress.try_write() {
                    *p = Some((i + 1, total));
                }
            }
            if failed > 0 {
                ctx.notify(if failed == 1 { "Un envoi a échoué.".to_string() } else { format!("{failed} envois ont échoué.") });
            }
            if let Ok(mut p) = panel.try_write() {
                if *p == Some(Panel::Photos) {
                    *p = None;
                }
            }
        });
    };

    let scrolled = move |e: ScrollEvent| {
        let bottom = e.scroll_top() + e.client_height() as f64;
        if bottom > e.scroll_height() as f64 - 800.0 {
            load(false);
        }
    };

    // Month sections, in the order of the list.
    let groups = {
        let mut groups: Vec<(String, Vec<Photo>)> = vec![];
        for photo in photos.read().iter() {
            let label = month_label(photo.taken);
            match groups.last_mut() {
                Some((last, items)) if *last == label => items.push(photo.clone()),
                _ => groups.push((label, vec![photo.clone()])),
            }
        }
        groups
    };
    let chosen: Vec<String> = selected.read().iter().map(|p| p.path.clone()).collect();
    let count = chosen.len();
    let sending = progress();
    let filtered = !from().is_empty() || !to().is_empty();

    rsx! {
        div { class: "backdrop photo-backdrop", onclick: move |_| panel.set(None),
            div { class: "photo-picker", onclick: move |e| e.stop_propagation(),
                header { class: "photo-head",
                    button { class: "icon", title: "Fermer", onclick: move |_| panel.set(None), Icon { name: "back" } }
                    h2 { "Mes photos" small { "SelfHost-hub" } }
                    if count > 0 && sending.is_none() {
                        button { class: "link", onclick: move |_| selected.set(vec![]), "Désélectionner" }
                    }
                }
                if !locked().is_empty() {
                    LinkNeeded { message: locked(), service: "Photos" }
                } else {
                    div { class: "photo-filters",
                        div { class: "photo-chips",
                            for (q, label) in QUICK {
                                button { key: "{label}", class: if quick() == Some(q) { "filter-chip on" } else { "filter-chip" },
                                    onclick: move |_| choose_quick(q), "{label}" }
                            }
                        }
                        div { class: "photo-dates",
                            Icon { name: "calendar" }
                            label { "Du"
                                input { r#type: "date", value: "{from}", max: "{to}",
                                    onchange: move |e| { from.set(e.value()); quick.set(None); load(true); } }
                            }
                            label { "au"
                                input { r#type: "date", value: "{to}", min: "{from}",
                                    onchange: move |e| { to.set(e.value()); quick.set(None); load(true); } }
                            }
                        }
                    }
                    div { class: "photo-scroll", onscroll: scrolled,
                        if !error().is_empty() {
                            div { class: "sheet-note error-note", "{error}"
                                button { class: "link", onclick: move |_| load(true), "Réessayer" }
                            }
                        }
                        if loaded() && groups.is_empty() {
                            div { class: "sheet-note",
                                div { class: "note-art", Icon { name: "image" } }
                                if filtered { "Aucune photo sur cette période." } else { "Aucune photo sauvegardée pour l'instant." }
                            }
                        }
                        for (label, items) in groups {
                            section { key: "{label}", class: "photo-month",
                                h4 { "{label}" }
                                div { class: "photo-grid",
                                    for photo in items {
                                        PhotoTile { key: "{photo.path}",
                                            order: chosen.iter().position(|p| *p == photo.path).map(|i| i + 1),
                                            photo: photo.clone(), on_toggle: move |p| toggle(p) }
                                    }
                                }
                            }
                        }
                        if loading() {
                            div { class: "photo-loading", span { class: "spinner" } "Chargement…" }
                        } else if next().is_some() {
                            button { class: "ghost more", onclick: move |_| load(false), "Afficher plus" }
                        }
                    }
                    div { class: if count > 0 || sending.is_some() { "photo-bar shown" } else { "photo-bar" },
                        if let Some((done, total)) = sending {
                            div { class: "photo-progress",
                                span { {format!("Envoi {}/{total}…", (done + 1).min(total))} }
                                div { class: "bar", div { style: {format!("width: {}%", done as f64 / total.max(1) as f64 * 100.0)} } }
                            }
                        } else {
                            span { class: "photo-count", b { "{count}" } " / {MAX} sélectionnée" if count > 1 { "s" } }
                            button { class: "primary", disabled: count == 0, onclick: send, "Envoyer ({count})" }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn PhotoTile(photo: Photo, order: Option<usize>, on_toggle: EventHandler<Photo>) -> Element {
    let mut broken = use_signal(|| false);
    let url = thumb_url(&photo);
    let class = if order.is_some() { "photo-tile on" } else { "photo-tile" };
    let picked = photo.clone();
    rsx! {
        button { class, title: "{photo.name}", aria_pressed: "{order.is_some()}", onclick: move |_| on_toggle.call(picked.clone()),
            if broken() {
                div { class: "photo-missing", Icon { name: crate::ui::pick(photo.is_video, "video", "image") } }
            } else {
                img { src: "{url}", alt: "", loading: "lazy", decoding: "async", draggable: "false",
                    onerror: move |_| broken.set(true) }
            }
            if photo.is_video {
                span { class: "video-badge", Icon { name: "play" } "Vidéo" }
            }
            span { class: "photo-check",
                if let Some(n) = order { "{n}" }
            }
        }
    }
}
