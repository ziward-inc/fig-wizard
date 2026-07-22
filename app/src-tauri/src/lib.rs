pub mod commands;
pub mod detect;
pub mod pdf;
pub mod pipeline;
pub mod verify;

use commands::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::open_pdf,
            commands::pick_pdf_file,
            commands::pick_output_dir,
            commands::model_status,
            commands::download_model,
            commands::run_extraction,
            commands::cancel_extraction,
            commands::list_results,
            commands::reveal_in_finder,
            commands::codex_status,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
