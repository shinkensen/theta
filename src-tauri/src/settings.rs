

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
 
    pub hotkey: String,

    pub close_to_tray: bool,

    pub launch_at_login: bool,

    pub auto_listen_on_show: bool,
    pub speech_recognition_provider: SpeechProvider,

    pub speak_replies: bool,

    pub voice: String,

    pub model: String,
    
    pub use_rag: bool,
   
    pub allow_web: bool,
   
    pub auto_approve: bool,
    
    pub calendar_id: String,
  
    pub custom_api_url: Option<String>,
  
    pub custom_api_key: Option<String>,
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
            custom_api_url: None,
            custom_api_key: None,
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
