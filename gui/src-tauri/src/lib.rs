mod commands;
use commands::{
    agent_run_steps, export_integrated_pdf, get_correlation_html, get_html_report, get_report,
    get_result_json, get_snapshot, integrated_report_html, open_integrated_report, pdf_available,
    register_account, save_text_file, start_test, stop_test, AppState,
};
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(AppState::default()))
        .invoke_handler(tauri::generate_handler![
            start_test,
            stop_test,
            get_snapshot,
            get_report,
            get_html_report,
            get_result_json,
            integrated_report_html,
            export_integrated_pdf,
            open_integrated_report,
            pdf_available,
            get_correlation_html,
            register_account,
            agent_run_steps,
            save_text_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
