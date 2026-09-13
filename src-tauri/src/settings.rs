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

#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SpeechProvider {
    #[default]
    Vosk,
    Windows,
}

impl SpeechProvider {
    pub fn is_supported(self) -> bool {
        match self {
            SpeechProvider::Vosk => true,
            SpeechProvider::Windows => cfg!(target_os = "windows"),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SpeechProvider::Vosk => "Vosk",
            SpeechProvider::Windows => "Windows Speech Recognition",
        }
    }
}

impl<'de> Deserialize<'de> for SpeechProvider {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "windows" => SpeechProvider::Windows,
            _ => SpeechProvider::Vosk,
        })
    }
}

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
    pub speech_recognition_provider: SpeechProvider,
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
            speech_recognition_provider: SpeechProvider::default(),
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

    if !next.speech_recognition_provider.is_supported() {
        warning = Some(format!(
            "{} isn't available on this platform. Keeping {}.",
            next.speech_recognition_provider.label(),
            previous.speech_recognition_provider.label()
        ));
        next.speech_recognition_provider = previous.speech_recognition_provider;
    }

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

    // Swapping engines under a live recognizer would leave the old backend
    // holding the microphone, so end the current session and let the next
    // start pick up the new provider.
    if next.speech_recognition_provider != previous.speech_recognition_provider {
        crate::stt::stop_active_session(&app);
    }

    match warning {
        Some(w) => Err(w),
        None => Ok(next),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_settings_without_provider_default_to_vosk() {
        let raw = r#"{"hotkey":"Alt+Space","voice":"en-GB-RyanNeural","model":"custom/model"}"#;
        let parsed: Settings = serde_json::from_str(raw).expect("legacy settings should load");
        assert_eq!(parsed.speech_recognition_provider, SpeechProvider::Vosk);
        assert_eq!(parsed.hotkey, "Alt+Space");
        assert_eq!(parsed.voice, "en-GB-RyanNeural");
        assert_eq!(parsed.model, "custom/model");
    }

    #[test]
    fn provider_round_trips_as_camel_case_json() {
        let mut settings = Settings::default();
        settings.speech_recognition_provider = SpeechProvider::Windows;
        let json = serde_json::to_string(&settings).expect("settings should serialize");
        assert!(json.contains(r#""speechRecognitionProvider":"windows""#));

        let parsed: Settings = serde_json::from_str(&json).expect("settings should round-trip");
        assert_eq!(parsed.speech_recognition_provider, SpeechProvider::Windows);
    }

    #[test]
    fn unknown_provider_falls_back_without_discarding_other_settings() {
        let raw = r#"{"speechRecognitionProvider":"whisper","calendarId":"work","useRag":false}"#;
        let parsed: Settings = serde_json::from_str(raw).expect("unknown provider should not fail");
        assert_eq!(parsed.speech_recognition_provider, SpeechProvider::Vosk);
        assert_eq!(parsed.calendar_id, "work");
        assert!(!parsed.use_rag);
    }

    #[test]
    fn vosk_is_supported_everywhere() {
        assert!(SpeechProvider::Vosk.is_supported());
        assert_eq!(
            SpeechProvider::Windows.is_supported(),
            cfg!(target_os = "windows")
        );
    }
}
