#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use icloud_recovery_desktop::commands::{self, AppState};

fn main() {
    tracing_subscriber::fmt::init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::get_restore_state,
            commands::start_auth,
            commands::reset_session,
            commands::scan_deleted_items,
            commands::start_restore,
            commands::pause_restore,
            commands::retry_failed,
        ])
        .run(tauri::generate_context!())
        .expect("error while running CloudNest");
}
