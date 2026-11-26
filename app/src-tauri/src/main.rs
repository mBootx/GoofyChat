// No console window next to the desktop app in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    goofychat_lib::run()
}
