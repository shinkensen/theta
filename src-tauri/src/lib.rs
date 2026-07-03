use serde::Serialize;
use std::time::SystemTime;

#[derive(Serialize)]
struct SystemInfo {
    os: String,
    arch: String,
    uptime_secs: u64,
    hostname: String,
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! Welcome to Theta 🚀", name)
}

#[tauri::command]
fn get_system_info() -> Result<SystemInfo, String> {
    let os = std::env::consts::OS.to_string();
    let arch = std::env::consts::ARCH.to_string();
    let uptime_secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    Ok(SystemInfo {
        os,
        arch,
        uptime_secs,
        hostname,
    })
}

#[tauri::command]
fn read_file_content(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("Failed to read file '{}': {}", path, e))
}

#[tauri::command]
fn write_file_content(path: String, content: String) -> Result<String, String> {
    std::fs::write(&path, &content)
        .map(|_| format!("Successfully wrote to {}", path))
        .map_err(|e| format!("Failed to write to '{}': {}", path, e))
}

fn resolve_path(path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")).join(p)
    }
}

#[tauri::command]
fn read_file_bytes(path: String) -> Result<Vec<u8>, String> {
    let resolved = resolve_path(&path);
    std::fs::read(&resolved).map_err(|e| format!("Failed to read file '{}': {}", resolved.display(), e))
}

#[tauri::command]
fn list_directory(path: String) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(&path)
        .map_err(|e| format!("Failed to read directory '{}': {}", path, e))?;

    let mut result: Vec<String> = entries
        .filter_map(|entry| {
            entry.ok().map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() {
                    format!("📁 {}", name)
                } else {
                    format!("📄 {}", name)
                }
            })
        })
        .collect();

    result.sort();
    Ok(result)
}

fn ensure_recordings_dir() {
    let dir = std::path::Path::new("theta_recordings");
    if !dir.exists() {
        let _ = std::fs::create_dir_all(dir);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    ensure_recordings_dir();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_audio_recorder::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            get_system_info,
            read_file_content,
            read_file_bytes,
            write_file_content,
            list_directory,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}