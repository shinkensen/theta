//! Theta — a local-first voice assistant.
//!
//! The Rust side owns everything that has to work whether or not the window is
//! visible, plus everything that would be slow or unsafe in the webview:
//!
//! | module      | what it does                                              |
//! |-------------|-----------------------------------------------------------|
//! | [`stt`]     | Vosk speech-to-text over a cpal capture stream            |
//! | [`rag`]     | local hybrid retrieval (BM25 + character trigrams)        |
//! | [`procs`]   | process / system / port inspection, shell commands        |
//! | [`calendar`]| Google Calendar OAuth + v3 CRUD                           |
//! | [`settings`]| persisted preferences (hotkey, voice, model, …)            |
//!
//! The frontend is a chat UI plus an agent loop that calls these as tools.
//!
//! Background operation is the reason the global shortcut and tray live here
//! rather than in the webview: a shortcut registered from JS only fires while
//! the window has focus, which defeats the point.

pub mod calendar;
pub mod procs;
pub mod profile;
pub mod rag;
pub mod settings;
pub mod stt;

use tauri::menu::{MenuBuilder, MenuEvent};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// The one window Theta has. Used everywhere instead of `get_focused_window`,
/// which is `None` exactly when we care most (hidden or unfocused).
const MAIN_WINDOW: &str = "main";

// ---------------------------------------------------------------------------
// Misc commands the frontend still uses
// ---------------------------------------------------------------------------

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

/// Brings the window back from the tray. Also used by the hotkey handler.
#[tauri::command]
fn show_window(app: AppHandle) -> Result<(), String> {
    reveal(&app);
    Ok(())
}

/// Hides to tray without quitting — what the titlebar's close button calls.
#[tauri::command]
fn hide_to_tray(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        win.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    app.exit(0);
}

// ---------------------------------------------------------------------------
// Window / tray / hotkey plumbing
// ---------------------------------------------------------------------------

/// Show + unminimize + focus, in that order. Windows will not focus a hidden
/// window, and `set_focus` on a minimized one is a no-op, so all three are
/// needed for a reliable "summon" from the tray or hotkey.
fn reveal(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(MAIN_WINDOW) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// The hotkey's job: bring Theta up and start listening in one press.
///
/// If the window is already up *and* the mic is live, the same press stops
/// listening — so it acts as push-to-talk you don't have to hold.
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

    // Revealing a hidden window shouldn't also stop an in-flight recording,
    // so only toggle when the window was already on screen.
    let start_only = !was_visible;
    let result = if start_only {
        if auto_listen {
            stt::start(app, &stt).map(|_| true)
        } else {
            Ok(false)
        }
    } else {
        stt::toggle(app, &stt)
    };

    match result {
        Ok(listening) => {
            let _ = app.emit("theta-hotkey", listening);
        }
        Err(e) => {
            let _ = app.emit("vosk-error", e);
        }
    }
}

/// Re-registers the global shortcut when the user changes it in Settings.
/// Called from [`settings::save_settings`].
pub fn rebind_hotkey(app: &AppHandle, previous: &str, next: &str) -> Result<(), String> {
    let shortcuts = app.global_shortcut();

    // Register first: if `next` is unparseable or already owned by another
    // app, we bail out with the old binding still live.
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

/// Flips the OS "run at login" entry.
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
                match stt::toggle(app, &stt) {
                    Ok(listening) => {
                        let _ = app.emit("theta-hotkey", listening);
                    }
                    Err(e) => {
                        let _ = app.emit("vosk-error", e);
                    }
                }
            }
        }
        "quit" => app.exit(0),
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
        // Left click summons the window; the menu stays on right click, which
        // is what people expect from a tray icon on Windows.
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

// ---------------------------------------------------------------------------
// App entry point
// ---------------------------------------------------------------------------

/// Resolves the Vosk model directory.
///
/// Looks for `vosk-models/vosk-model-small-en-us-0.15` relative to the running
/// executable (works for `cargo run`/`tauri dev`, where the exe lives in
/// `target/<profile>/`, by walking up to find `src-tauri`), and also relative
/// to the crate manifest dir. Panics with a clear message if not found.
fn resolve_vosk_model_path() -> String {
    const MODEL_DIR_NAME: &str = "vosk-model-small-en-us-0.15";
    const RELATIVE_PATHS: &[&str] = &[
        "src-tauri/vosk-models",
        "vosk-models",
        "../src-tauri/vosk-models",
    ];

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            for up in 0..=3 {
                let mut base = exe_dir.to_path_buf();
                for _ in 0..up {
                    if !base.pop() {
                        break;
                    }
                }
                for rel in RELATIVE_PATHS {
                    let candidate = base.join(rel).join(MODEL_DIR_NAME);
                    if candidate.is_dir() {
                        return candidate.to_string_lossy().into_owned();
                    }
                }
            }
        }
    }

    if let Some(manifest) = std::option_env!("CARGO_MANIFEST_DIR") {
        let candidate = std::path::Path::new(manifest)
            .join("vosk-models")
            .join(MODEL_DIR_NAME);
        if candidate.is_dir() {
            return candidate.to_string_lossy().into_owned();
        }
    }

    panic!(
        "Vosk model '{MODEL_DIR_NAME}' not found. Expected it under \
         `src-tauri/vosk-models/{MODEL_DIR_NAME}` (containing am/, conf/, \
         graph/, ivector/). Download it from \
         https://alphacephei.com/vosk/models and extract it there."
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let model_path = resolve_vosk_model_path();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            // Launching at login should land in the tray, not pop a window in
            // the user's face; the frontend reads this argv flag.
            Some(vec!["--hidden"]),
        ))
        .manage(stt::SttState::new(&model_path))
        .manage(procs::ProcState::new())
        .setup(|app| {
            // RAG and OAuth tokens are per-user state, so they live in the
            // platform app-data dir rather than next to the binary.
            let data_dir = app.path().app_data_dir().map_err(|e| {
                format!("Couldn't resolve the app data directory: {e}")
            })?;
            std::fs::create_dir_all(&data_dir).map_err(|e| {
                format!("Couldn't create {}: {e}", data_dir.display())
            })?;

            let settings = settings::SettingsState::load(&data_dir);
            let hotkey = settings.snapshot().hotkey;

            app.manage(rag::RagState::load(&data_dir));
            app.manage(profile::ProfileState::load(&data_dir));
            app.manage(calendar::CalendarState::load(&data_dir));
            app.manage(settings);

            let handle = app.handle();
            if let Err(e) = rebind_hotkey(handle, "", &hotkey) {
                // A taken hotkey is annoying, not fatal — the in-app mic
                // button still works.
                eprintln!("[theta] {e}");
            }
            if let Err(e) = build_tray(handle) {
                eprintln!("[theta] tray icon unavailable: {e}");
            }

            // `--hidden` comes from the autostart entry.
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
            // files + window
            read_file_content,
            write_file_content,
            list_directory,
            show_window,
            hide_to_tray,
            quit_app,
            // speech
            stt::start_listening,
            stt::stop_listening,
            stt::toggle_listening,
            stt::is_listening,
            stt::get_debug_log,
            stt::list_input_devices,
            // processes / terminal
            procs::list_processes,
            procs::system_stats,
            procs::kill_process,
            procs::listening_ports,
            procs::run_command,
            // retrieval
            rag::rag_ingest_text,
            rag::rag_ingest_file,
            rag::rag_search,
            rag::rag_stats,
            rag::rag_list,
            rag::rag_forget,
            // evolving user profile
            profile::profile_get,
            profile::profile_apply,
            profile::profile_remove,
            profile::profile_clear,
            // calendar
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
            // settings
            settings::get_settings,
            settings::save_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
