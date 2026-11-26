//! Desktop shell: tray icon, single instance, notifications, start with the
//! system, camera and microphone permission, and self-update from GitHub.
//!
//! Closing the window only hides it, so the page and its WebSocket keep running
//! and can announce messages and calls without any push service.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Builder, Manager, Runtime, State, UserAttentionType, WebviewWindow, WindowEvent, Wry};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const LATEST: &str = "https://api.github.com/repos/mBootx/GoofyChat/releases/latest";
const ASSET: &str = "GoofyChat-setup.exe";
/// Passed by the system when the app starts with the session: stay in the tray.
const AUTOSTART: &str = "--autostart";
const TRAY: &str = "main";

#[derive(Default)]
struct Shell {
    /// The installer found by the last update check; the only one we will run.
    update: Mutex<Option<Release>>,
    /// The "still running in the tray" hint is shown once per run.
    hinted: AtomicBool,
    installing: AtomicBool,
}

#[derive(Clone, Serialize)]
struct Release {
    version: String,
    url: String,
}

pub fn setup(builder: Builder<Wry>) -> Builder<Wry> {
    builder
        // Must come first: a second launch only brings this window back.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| show(app)))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, Some(vec![AUTOSTART])))
        .manage(Shell::default())
        .invoke_handler(tauri::generate_handler![
            notify,
            set_unread,
            check_update,
            install_update,
            autostart_enabled,
            set_autostart,
            app_version
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                let shell = window.state::<Shell>();
                if !shell.hinted.swap(true, Ordering::Relaxed) {
                    toast(
                        window.app_handle(),
                        "Goofy Chat reste actif",
                        "Il continue de recevoir vos messages dans la zone de notification. Clic droit sur l'icône pour quitter.",
                        false,
                    );
                }
            }
        })
        .setup(|app| {
            let open = MenuItem::with_id(app, "open", "Ouvrir Goofy Chat", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quitter", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id(TRAY)
                .tooltip("Goofy Chat")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                        show(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            if let Some(window) = app.get_webview_window("main") {
                #[cfg(windows)]
                allow_media(&window);
                if !std::env::args().any(|a| a == AUTOSTART) {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            }
            Ok(())
        })
}

fn main_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window("main")
}

/// Brings the window back from the tray, the taskbar or behind other windows.
fn show<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = main_window(app) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Announces a message or a call, unless the user is looking at the window.
#[tauri::command]
fn notify(app: AppHandle, title: String, body: String, call: bool) {
    let Some(window) = main_window(&app) else { return };
    let visible = window.is_visible().unwrap_or(false) && !window.is_minimized().unwrap_or(false);
    if visible && window.is_focused().unwrap_or(false) {
        return;
    }
    if visible || window.is_minimized().unwrap_or(false) {
        let kind = if call { UserAttentionType::Critical } else { UserAttentionType::Informational };
        let _ = window.request_user_attention(Some(kind));
    }
    toast(&app, &title, &body, call);
}

#[cfg(windows)]
fn toast<R: Runtime>(app: &AppHandle<R>, title: &str, body: &str, call: bool) {
    use tauri_winrt_notification::{Duration as Shown, Sound, Toast};
    // Toasts need the AppUserModelID of a Start menu shortcut: the installer
    // creates one for our identifier. Unbundled builds borrow PowerShell's.
    let installed = std::env::current_exe()
        .map(|p| !p.components().any(|c| c.as_os_str() == "target"))
        .unwrap_or(false);
    let id = if installed { app.config().identifier.clone() } else { Toast::POWERSHELL_APP_ID.to_string() };
    let handle = app.clone();
    let mut toast = Toast::new(&id)
        .title(title)
        .text1(body)
        .on_activated(move |_| {
            show(&handle);
            Ok(())
        });
    toast = if call {
        // The page rings by itself; the toast stays longer.
        toast.duration(Shown::Long).sound(None).add_button("Ouvrir", "open")
    } else {
        toast.duration(Shown::Short).sound(Some(Sound::Default))
    };
    let _ = toast.show();
}

#[cfg(not(windows))]
fn toast<R: Runtime>(app: &AppHandle<R>, _title: &str, _body: &str, _call: bool) {
    if let Some(window) = main_window(app) {
        let _ = window.request_user_attention(Some(UserAttentionType::Informational));
    }
}

/// Unread count in the window title, the tray tooltip and a taskbar badge.
#[tauri::command]
fn set_unread(app: AppHandle, count: i64) {
    let label = match count {
        0 => "Goofy Chat".to_string(),
        1 => "Goofy Chat (1 message non lu)".to_string(),
        n => format!("Goofy Chat ({n} messages non lus)"),
    };
    if let Some(tray) = app.tray_by_id(TRAY) {
        let _ = tray.set_tooltip(Some(&label));
    }
    if let Some(window) = main_window(&app) {
        let _ = window.set_title(if count > 0 { &label } else { "Goofy Chat" });
        #[cfg(windows)]
        let _ = window.set_overlay_icon((count > 0).then(badge));
    }
}

/// A small dot in the accent colour, drawn over the taskbar button.
#[cfg(windows)]
fn badge() -> tauri::image::Image<'static> {
    const SIZE: u32 = 20;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    let center = (SIZE as f32 - 1.0) / 2.0;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let d = ((x as f32 - center).powi(2) + (y as f32 - center).powi(2)).sqrt();
            // White ring around a red dot, anti-aliased on the edge.
            let alpha = (center + 0.5 - d).clamp(0.0, 1.0);
            let (r, g, b) = if d < center - 2.0 { (0xe5, 0x48, 0x4d) } else { (0xff, 0xff, 0xff) };
            rgba.extend_from_slice(&[r, g, b, (alpha * 255.0) as u8]);
        }
    }
    tauri::image::Image::new_owned(rgba, SIZE, SIZE)
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[tauri::command]
fn autostart_enabled(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let launcher = app.autolaunch();
    let result = if enabled { launcher.enable() } else { launcher.disable() };
    result.map_err(|e| format!("Réglage impossible : {e}"))
}

// ---- self-update ----

fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .user_agent("GoofyChat-desktop")
        .build())
}

/// "v1.2.3" or "1.2.3" as numbers; a suffix such as "-beta" is ignored.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let core = text.trim().trim_start_matches(['v', 'V']);
    let core = core.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

fn latest(current: &str) -> Result<Option<Release>, String> {
    let response = agent()?
        .get(LATEST)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    let release: serde_json::Value =
        serde_json::from_reader(response.into_reader()).map_err(|e| e.to_string())?;
    let tag = release["tag_name"].as_str().unwrap_or_default();
    let (Some(found), Some(mine)) = (parse_version(tag), parse_version(current)) else { return Ok(None) };
    if found <= mine || release["draft"].as_bool() == Some(true) || release["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let url = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"].as_str() == Some(ASSET))
        .and_then(|a| a["browser_download_url"].as_str());
    Ok(url.map(|url| Release { version: tag.trim_start_matches(['v', 'V']).to_string(), url: url.to_string() }))
}

/// The newer release with a Windows installer, if any.
#[tauri::command]
async fn check_update(app: AppHandle, shell: State<'_, Shell>) -> Result<Option<Release>, String> {
    let current = app.package_info().version.to_string();
    let found = tauri::async_runtime::spawn_blocking(move || latest(&current))
        .await
        .map_err(|e| e.to_string())??;
    *shell.update.lock().unwrap() = found.clone();
    Ok(found)
}

fn download(url: &str, version: &str) -> Result<std::path::PathBuf, String> {
    let dir = std::env::temp_dir().join("GoofyChat-update");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(format!("GoofyChat-setup-{version}.exe"));
    let partial = dir.join("download.part");
    let response = agent()?.get(url).call().map_err(|e| e.to_string())?;
    let mut file = std::fs::File::create(&partial).map_err(|e| e.to_string())?;
    let written = std::io::copy(&mut response.into_reader(), &mut file).map_err(|e| e.to_string())?;
    drop(file);
    if written < 100_000 {
        return Err("Fichier d'installation incomplet.".into());
    }
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&partial, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Downloads the installer found by `check_update`, starts it and quits: the
/// installer replaces the app and starts the new version.
#[tauri::command]
async fn install_update(app: AppHandle, shell: State<'_, Shell>, url: String) -> Result<(), String> {
    let release = shell.update.lock().unwrap().clone();
    let Some(release) = release.filter(|r| r.url == url) else {
        return Err("Mise à jour inconnue, relancez la recherche.".into());
    };
    if shell.installing.swap(true, Ordering::SeqCst) {
        return Err("Installation déjà en cours.".into());
    }
    let result = tauri::async_runtime::spawn_blocking(move || {
        let installer = download(&release.url, &release.version)?;
        // Tauri's NSIS installer: /P passive (progress bar only, closes the running
        // app), /UPDATE keeps shortcuts and the start-with-Windows entry, /R
        // starts the new version at the end.
        std::process::Command::new(&installer)
            .args(["/P", "/UPDATE", "/R"])
            .spawn()
            .map_err(|e| format!("Impossible de lancer l'installation : {e}"))
    })
    .await
    .map_err(|e| e.to_string())?;
    match result {
        Ok(_) => {
            app.exit(0);
            Ok(())
        }
        Err(e) => {
            shell.installing.store(false, Ordering::SeqCst);
            Err(e)
        }
    }
}

// ---- WebView2 ----

/// Grants the microphone and camera to the app's own page, instead of WebView2
/// asking again with its own dialog.
#[cfg(windows)]
fn allow_media(window: &WebviewWindow) {
    let _ = window.with_webview(|webview| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CAMERA, COREWEBVIEW2_PERMISSION_KIND_MICROPHONE,
            COREWEBVIEW2_PERMISSION_STATE_ALLOW,
        };
        use webview2_com::PermissionRequestedEventHandler;

        let Ok(core) = webview.controller().CoreWebView2() else { return };
        let handler = PermissionRequestedEventHandler::create(Box::new(|_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            args.PermissionKind(&mut kind)?;
            let mut uri = windows::core::PWSTR::null();
            args.Uri(&mut uri)?;
            let uri = webview2_com::take_pwstr(uri);
            let own = uri.starts_with("http://tauri.localhost") || uri.starts_with("https://tauri.localhost");
            if own && (kind == COREWEBVIEW2_PERMISSION_KIND_MICROPHONE || kind == COREWEBVIEW2_PERMISSION_KIND_CAMERA) {
                args.SetState(COREWEBVIEW2_PERMISSION_STATE_ALLOW)?;
            }
            Ok(())
        }));
        let mut token = 0i64;
        let _ = core.add_PermissionRequested(&handler, &mut token);
    });
}
