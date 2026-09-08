//! Persisted user preferences.
//!
//! Kept in `<app-data>/settings.json`. Anything the user can change in the
//! Settings panel and expects to survive a restart lives here — notably the
//! global hotkey, which is registered on the Rust side so it fires even when
//! the window is hidden.
//!
//! Secrets (OpenRouter key, Firecrawl key) deliberately do *not* live here:
//! they stay in `.env` and are read by Vite at build time.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A "Ctrl+Shift+Space"-style accelerator, parsed by the global-shortcut
/// plugin. `CommandOrControl` maps to Ctrl on Windows/Linux, Cmd on macOS.
pub const DEFAULT_HOTKEY: &str = "CommandOrControl+Shift+Space";

/// `#[serde(default)]` on the struct means a settings file written by an older
/// build still loads — missing keys fall back to these.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Global accelerator that reveals the window and toggles the mic.
    pub hotkey: String,
    /// Closing the window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    /// Start Theta when the user logs in.
    pub launch_at_login: bool,
    /// Begin listening as soon as the hotkey reveals the window.
    pub auto_listen_on_show: bool,
    /// Read replies out loud.
    pub speak_replies: bool,
    /// Edge TTS voice short name.
    pub voice: String,
    /// OpenRouter model id used by the agent loop.
    pub model: String,
    /// Inject RAG hits into the system prompt before each turn.
    pub use_rag: bool,
    /// Allow the model to call `search_web`.
    pub allow_web: bool,
    /// Run approval-sensitive tools without prompting.
    pub auto_approve: bool,
    /// Default calendar id for reads and writes.
    pub calendar_id: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            hotkey: DEFAULT_HOTKEY.to_string(),
            close_to_tray: true,
            launch_at_login: false,
            auto_listen_on_show: true,
            speak_replies: true,
            voice: "en-GB-SoniaNeural".to_string(),
            model: "openrouter/free".to_string(),
            use_rag: true,
            allow_web: true,
            auto_approve: false,
            calendar_id: "primary".to_string(),
        }
    }
}

pub struct SettingsState {
    inner: Mutex<Settings>,
    path: PathBuf,
}

impl SettingsState {
    pub fn load(dir: &Path) -> Self {
        let path = dir.join("settings.json");
        let inner = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Settings>(&raw).ok())
            .unwrap_or_default();
        Self {
            inner: Mutex::new(inner),
            path,
        }
    }

    pub fn snapshot(&self) -> Settings {
        self.inner
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }

    fn persist(&self, next: &Settings) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Couldn't create the settings folder: {e}"))?;
        }
        let json = serde_json::to_string_pretty(next).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, json)
            .map_err(|e| format!("Couldn't save settings to {}: {e}", self.path.display()))
    }
}

#[tauri::command]
pub fn get_settings(state: tauri::State<'_, SettingsState>) -> Settings {
    state.snapshot()
}

/// Saves settings and applies the side effects that live outside the file:
/// re-registering the global hotkey and flipping the OS autostart entry.
///
/// A bad accelerator is reported but doesn't block the save — the rest of the
/// settings still apply, and the previously working hotkey stays registered.
#[tauri::command]
pub fn save_settings(
    settings: Settings,
    app: tauri::AppHandle,
    state: tauri::State<'_, SettingsState>,
) -> Result<Settings, String> {
    let previous = state.snapshot();
    let mut next = settings;
    next.hotkey = next.hotkey.trim().to_string();
    if next.hotkey.is_empty() {
        next.hotkey = DEFAULT_HOTKEY.to_string();
    }

    let mut warning: Option<String> = None;

    if next.hotkey != previous.hotkey {
        match crate::rebind_hotkey(&app, &previous.hotkey, &next.hotkey) {
            Ok(()) => {}
            Err(e) => {
                // Keep the old, working binding rather than leaving the user
                // with no hotkey at all.
                next.hotkey = previous.hotkey.clone();
                warning = Some(e);
            }
        }
    }

    if next.launch_at_login != previous.launch_at_login {
        if let Err(e) = crate::set_autostart(&app, next.launch_at_login) {
            next.launch_at_login = previous.launch_at_login;
            warning = Some(e);
        }
    }

    state.persist(&next)?;
    if let Ok(mut guard) = state.inner.lock() {
        *guard = next.clone();
    }

    match warning {
        Some(w) => Err(w),
        None => Ok(next),
    }
}
