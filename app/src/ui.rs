//! Small shared pieces: icons, avatars, previews, dates.

use crate::api;
use dioxus::prelude::*;
use wasm_bindgen::JsValue;

/// `if c { a } else { b }` for `&'static str` props: written inline in rsx, the
/// literals would become formatted `String`s.
pub fn pick(c: bool, a: &'static str, b: &'static str) -> &'static str {
    if c { a } else { b }
}

#[component]
pub fn Icon(name: &'static str) -> Element {
    let path = match name {
        "back" => "M15 18l-6-6 6-6",
        "plus" => "M12 5v14M5 12h14",
        "logout" => "M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4M16 17l5-5-5-5M21 12H9",
        "send" => "M22 2L11 13M22 2l-7 20-4-9-9-4z",
        "clip" => "M21 11l-9 9a6 6 0 0 1-8.5-8.5l9-9a4 4 0 0 1 5.7 5.7l-9 9a2 2 0 0 1-2.9-2.8l8.4-8.5",
        "mic" => "M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3zM19 10v2a7 7 0 0 1-14 0v-2M12 19v3",
        "micoff" => "M3 3l18 18M9 9v3a3 3 0 0 0 5 2M15 9V5a3 3 0 0 0-6-1M19 10v2a7 7 0 0 1-1 3M5 10v2a7 7 0 0 0 10 6M12 19v3",
        "stop" => "M6 6h12v12H6z",
        "video" => "M23 7l-7 5 7 5zM3 5h11a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z",
        "videooff" => "M3 3l18 18M16 11V7a2 2 0 0 0-2-2h-4M4 5H3a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h11a2 2 0 0 0 2-2M23 7l-7 5 7 5z",
        "phone" => "M22 17v3a2 2 0 0 1-2 2A18 18 0 0 1 2 4a2 2 0 0 1 2-2h3a2 2 0 0 1 2 1.7l.7 3a2 2 0 0 1-.5 2L8 9.9a16 16 0 0 0 6 6l1.3-1.2a2 2 0 0 1 2-.5l3 .7A2 2 0 0 1 22 17z",
        "hangup" => "M6 6l12 12M18 6L6 18",
        "users" => "M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8zM23 21v-2a4 4 0 0 0-3-3.9M16 3.1a4 4 0 0 1 0 7.8",
        "more" => "M12 5v.01M12 12v.01M12 19v.01",
        "download" => "M12 3v12M7 10l5 5 5-5M5 21h14",
        "settings" => "M4 6h10M18 6h2M4 12h2M10 12h10M4 18h12M20 18h0M16 4v4M8 10v4M18 16v4",
        "music" => "M9 18V5l12-2v13M9 18a3 3 0 1 1-6 0 3 3 0 0 1 6 0zM21 16a3 3 0 1 1-6 0 3 3 0 0 1 6 0z",
        "play" => "M7 4.5v15l12.5-7.5z",
        "pause" => "M8 5v14M16 5v14",
        "image" => "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM8.5 10a1.5 1.5 0 1 0 0-3 1.5 1.5 0 0 0 0 3zM21 15l-5-5L5 21",
        "search" => "M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM21 21l-4.3-4.3",
        "check" => "M20 6L9 17l-5-5",
        "lock" => "M5 11h14v10H5zM8 11V7a4 4 0 0 1 8 0v4",
        "chevron" => "M9 18l6-6-6-6",
        "trash" => "M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6M10 11v6M14 11v6",
        "calendar" => "M5 4h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM16 2v4M8 2v4M3 10h18",
        _ => "M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z",
    };
    rsx! { svg { view_box: "0 0 24 24", path { d: path } } }
}

#[component]
pub fn Avatar(
    name: String,
    #[props(default)] photo: Option<String>,
    #[props(default)] small: bool,
    /// Listening to music right now: green ring and dot.
    #[props(default)]
    live: bool,
) -> Element {
    let initials: String = name
        .split_whitespace()
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    let hue = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
    rsx! {
        div {
            class: match (small, live) {
                (true, true) => "avatar small live",
                (true, false) => "avatar small",
                (false, true) => "avatar live",
                (false, false) => "avatar",
            },
            style: "background: hsl({hue} 60% 52%)",
            if let Some(id) = photo {
                img { src: api::media_url(&id), alt: "" }
            } else {
                "{initials}"
            }
        }
    }
}

/// One line describing a message, for previews and quotes.
pub fn summary(body: &str, mime: Option<&str>, deleted: bool) -> String {
    if deleted {
        return "Message supprimé".into();
    }
    if !body.is_empty() {
        return body.into();
    }
    match mime.map(|m| m.split('/').next().unwrap_or_default()) {
        Some("image") => "📷 Photo".into(),
        Some("video") => "🎬 Vidéo".into(),
        Some(_) => "🎤 Audio".into(),
        // No text and no file: a shared song (quotes do not carry the track).
        None => "🎵 Musique".into(),
    }
}

/// [`summary`] of a whole message, naming a shared song.
pub fn message_summary(message: &proto::Message) -> String {
    match &message.track {
        Some(track) if !message.deleted => format!("🎵 {} — {}", track.title, track.artist),
        _ => summary(&message.body, message.media.as_ref().map(|m| m.mime.as_str()), message.deleted),
    }
}

fn date(ms: i64) -> js_sys::Date {
    js_sys::Date::new(&JsValue::from_f64(ms as f64))
}

pub fn time(ms: i64) -> String {
    let d = date(ms);
    format!("{:02}:{:02}", d.get_hours(), d.get_minutes())
}

pub fn day(ms: i64) -> String {
    let d = date(ms);
    format!("{:02}/{:02}/{}", d.get_date(), d.get_month() + 1, d.get_full_year())
}

/// Time for today, date otherwise.
pub fn short(ms: i64) -> String {
    if day(ms) == day(js_sys::Date::now() as i64) {
        time(ms)
    } else {
        day(ms)
    }
}
