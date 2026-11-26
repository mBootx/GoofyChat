#[cfg(desktop)]
mod desktop;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = desktop::setup(builder);
    builder
        .run(tauri::generate_context!())
        .expect("failed to start the application");
}
