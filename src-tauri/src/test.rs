use serde::Serialize;
use std::time::SystemTime;
#[derive(Serialize)]

#[tauri::command]
pub fn get_time_now()->u64{
    let now = SystemTime::now();
    let secs: u64 =now.duration_since(SystemTime::UNIX_EPOCH).map_err(|e| e.to_string()).map(|d| d.as_secs()).expect("Getting the current system time failed"); //this is like the template litteral of rust
    secs
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            get_system_info,
            read_file_content,
            write_file_content,
            list_directory,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}