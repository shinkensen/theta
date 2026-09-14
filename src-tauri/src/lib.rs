

pub mod calendar;
pub mod canvas;
pub mod conversations;
pub mod integrations;
pub mod procs;
pub mod profile;
pub mod rag;
pub mod settings;
pub mod stt;

use tauri::menu::{MenuBuilder, MenuEvent};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const MAIN_WINDOW: &str = "main";

#[tauri::command]
fn read_file_content(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("Failed to read file '{path}': {e}"))
}

#[tauri::command]
fn write_file_content(path: String, content: String) -> Result<String, String> {
    std::fs::write(&path, &content)
        .map(|_| format!("Wrote {} bytes to {path}", content.len()))
        .map_err(|e| format!("Failed to write to '{path}': {e}"))
}

#[tauri::command]
fn list_directory(path: String) -> Result<Vec<String>, String> {
    let entries =
        std::fs::read_dir(&path).map_err(|e| format!("Failed to read directory '{path}': {e}"))?;

    let mut result: Vec<String> = entries
        .filter_map(|entry| {
            entry.ok().map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if e.path().is_dir() {
                    format!("📁 {name}")
                } else {
                    format!("📄 {name}")
                }
            })
        })
        .collect();

    result.sort();
    Ok(result)
}

#[tauri::command]
fn show_window(app: AppHandle) -> Result<(), String> {
    reveal(&app);
    Ok(())
}
#[tauri::command]
fn hide_to_tray(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    stt::stop_active_session(&app);
    app.exit(0);
}

fn reveal(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

fn on_hotkey(app: &AppHandle) {
    let was_visible = app
        .get_webview_window(MAIN_WINDOW)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);

    reveal(app);

    let Some(stt) = app.try_state::<stt::SttState>() else {
        return;
    };
    let auto_listen = app
        .try_state::<settings::SettingsState>()
        .map(|s| s.snapshot().auto_listen_on_show)
        .unwrap_or(true);
    let result = if !was_visible {
        if auto_listen {
            stt::start(app, &stt)
        } else {
            Ok(())
        }
    } else {
        stt::toggle(app, &stt).map(|_| ())
    };

    if let Err(e) = result {
        let _ = app.emit(stt::ERROR_EVENT, e);
    }
}


pub fn rebind_hotkey(app: &AppHandle, previous: &str, next: &str) -> Result<(), String> {
    let shortcuts = app.global_shortcut();

    shortcuts
        .on_shortcut(next, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                on_hotkey(app);
            }
        })
        .map_err(|e| format!("Couldn't bind '{next}': {e}. Keeping '{previous}'."))?;

    if !previous.is_empty() && previous != next {
        let _ = shortcuts.unregister(previous);
    }
    Ok(())
}
pub fn set_autostart(app: &AppHandle, enabled: bool) -> Result<(), String> {
    use tauri_plugin_autostart::ManagerExt;
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| format!("Couldn't change the launch-at-login setting: {e}"))
}

fn on_tray_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "show" => reveal(app),
        "listen" => {
            reveal(app);
            if let Some(stt) = app.try_state::<stt::SttState>() {
                if let Err(e) = stt::toggle(app, &stt) {
                    let _ = app.emit(stt::ERROR_EVENT, e);
                }
            }
        }
        "quit" => {
            stt::stop_active_session(app);
            app.exit(0);
        }
        _ => {}
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let menu = MenuBuilder::new(app)
        .text("show", "Open Theta")
        .text("listen", "Start / stop listening")
        .separator()
        .text("quit", "Quit")
        .build()?;

    let mut builder = TrayIconBuilder::with_id("theta-tray")
        .tooltip("Theta — press Ctrl+Shift+Space to talk")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(on_tray_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                reveal(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
         
            Some(vec!["--hidden"]),
        ))
        .manage(stt::SttState::new())
        .manage(procs::ProcState::new())
        .setup(|app| {
      
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("Couldn't resolve the app data directory: {e}"))?;
            std::fs::create_dir_all(&data_dir)
                .map_err(|e| format!("Couldn't create {}: {e}", data_dir.display()))?;

            let settings = settings::SettingsState::load(&data_dir);
            let hotkey = settings.snapshot().hotkey;

            app.manage(rag::RagState::load(&data_dir));
            app.manage(profile::ProfileState::load(&data_dir));
            app.manage(calendar::CalendarState::load(&data_dir));
            app.manage(canvas::CanvasState::load(&data_dir));
            app.manage(integrations::spotify::SpotifyState::load(&data_dir));
            app.manage(integrations::hackatime::HackatimeState::load(&data_dir));
            app.manage(integrations::github::GithubState::load(&data_dir));
            app.manage(integrations::gmail::GmailState::load(&data_dir));
            app.manage(integrations::notion::NotionState::load(&data_dir));
            app.manage(integrations::minestrator::MineStratorState::load(&data_dir));
            app.manage(integrations::api_keys::ApiKeyState::load(&data_dir));
            app.manage(conversations::ConversationState::new(&data_dir).expect("Failed to initialize conversation state"));
            app.manage(settings);

            let handle = app.handle();
            if let Err(e) = rebind_hotkey(handle, "", &hotkey) {
                
                eprintln!("[theta] {e}");
            }
            if let Err(e) = build_tray(handle) {
                eprintln!("[theta] tray icon unavailable: {e}");
            }

          
            if std::env::args().any(|a| a == "--hidden") {
                if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
                    let _ = win.hide();
                }
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let close_to_tray = window
                    .app_handle()
                    .try_state::<settings::SettingsState>()
                    .map(|s| s.snapshot().close_to_tray)
                    .unwrap_or(true);
                if close_to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
          
            read_file_content,
            write_file_content,
            list_directory,
            show_window,
            hide_to_tray,
            quit_app,
          
            stt::start_listening,
            stt::stop_listening,
            stt::toggle_listening,
            stt::is_listening,
            stt::get_debug_log,
            stt::list_input_devices,
            stt::speech_providers,
            
            procs::list_processes,
            procs::system_stats,
            procs::kill_process,
            procs::listening_ports,
            procs::run_command,
           
            rag::rag_ingest_text,
            rag::rag_ingest_file,
            rag::rag_search,
            rag::rag_stats,
            rag::rag_list,
            rag::rag_forget,
            
            profile::profile_get,
            profile::profile_apply,
            profile::profile_remove,
            profile::profile_clear,
            
            calendar::google_auth_status,
            calendar::google_set_credentials,
            calendar::google_connect,
            calendar::google_disconnect,
            calendar::calendar_list_calendars,
            calendar::calendar_list_events,
            calendar::calendar_create_event,
            calendar::calendar_update_event,
            calendar::calendar_delete_event,
            calendar::calendar_quick_add,
      
            canvas::canvas_status,
            canvas::canvas_save_config,
            canvas::canvas_disconnect,
            canvas::canvas_get_profile,
            canvas::canvas_list_active_courses,
            canvas::canvas_list_assignments,
            canvas::canvas_list_due_dates,
            canvas::canvas_list_modules,
            canvas::canvas_list_module_items,
        
            integrations::spotify::spotify_status,
            integrations::spotify::spotify_set_client_id,
            integrations::spotify::spotify_connect,
            integrations::spotify::spotify_disconnect,
            integrations::spotify::spotify_get_playback,
            integrations::spotify::spotify_list_devices,
            integrations::spotify::spotify_play,
            integrations::spotify::spotify_pause,
            integrations::spotify::spotify_next,
            integrations::spotify::spotify_previous,
            integrations::spotify::spotify_seek,
            integrations::spotify::spotify_set_volume,
         
            integrations::hackatime::hackatime_status,
            integrations::hackatime::hackatime_set_client_id,
            integrations::hackatime::hackatime_connect,
            integrations::hackatime::hackatime_disconnect,
            integrations::hackatime::hackatime_get_profile,
            integrations::hackatime::hackatime_get_hours,
            integrations::hackatime::hackatime_get_streak,
            integrations::hackatime::hackatime_list_projects,
            integrations::hackatime::hackatime_latest_heartbeat,
       
            integrations::github::github_status,
            integrations::github::github_set_client_id,
            integrations::github::github_begin_device_flow,
            integrations::github::github_poll_device_flow,
            integrations::github::github_disconnect,
            integrations::github::github_get_profile,
            integrations::github::github_list_repositories,
            integrations::github::github_list_notifications,
            integrations::github::github_search_issues,
            integrations::github::github_create_issue,
            integrations::github::github_comment_issue,
        
            integrations::gmail::gmail_status,
            integrations::gmail::gmail_set_client_id,
            integrations::gmail::gmail_connect,
            integrations::gmail::gmail_disconnect,
            integrations::gmail::gmail_get_profile,
            integrations::gmail::gmail_search_messages,
            integrations::gmail::gmail_get_message,
            integrations::gmail::gmail_get_thread,
            integrations::gmail::gmail_create_draft,
            integrations::gmail::gmail_send_message,
            integrations::gmail::gmail_modify_message,
        
            integrations::notion::notion_status,
            integrations::notion::notion_connect,
            integrations::notion::notion_disconnect,
            integrations::notion::notion_list_tools,
            integrations::notion::notion_call_tool,
      
            integrations::minestrator::minestrator_status,
            integrations::minestrator::minestrator_save_config,
            integrations::minestrator::minestrator_connect,
            integrations::minestrator::minestrator_disconnect,
            integrations::minestrator::minestrator_list_tools,
            integrations::minestrator::minestrator_call_tool,
        
            integrations::api_keys::openrouter_status,
            integrations::api_keys::openrouter_save_key,
            integrations::api_keys::openrouter_get_key,
            integrations::api_keys::openrouter_clear_key,
            integrations::api_keys::firecrawl_status,
            integrations::api_keys::firecrawl_save_key,
            integrations::api_keys::firecrawl_get_key,
      
            settings::get_settings,
            settings::save_settings,
      
            conversations::conversation_create,
            conversations::conversation_save,
            conversations::conversation_load,
            conversations::conversation_list,
            conversations::conversation_delete,
            conversations::conversation_set_current,
            conversations::conversation_get_current,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
