//! Speech-to-text lifecycle, shared by every recognition backend.
//!
//! Capture and recognition live in the backend modules; this module owns what
//! must behave the same whichever engine is selected: one session at a time,
//! generation guarding so a winding-down engine cannot talk over its
//! replacement, and the event names the frontend subscribes to.
//!
//! Events emitted:
//!   `theta-speech-partial` (String)  — in-progress text, replaces previous
//!   `theta-speech-result`  (String)  — finalized utterance
//!   `theta-speech-error`   (String)  — capture/recognizer failure
//!   `theta-level`          (i32)     — 0..1000 mic level, for the waveform
//!   `theta-debug`          (String)  — timestamped log line
//!   `theta-listening`      (bool)    — authoritative listening state

pub mod vosk;

#[cfg(target_os = "windows")]
pub mod windows;

use crate::settings::{SettingsState, SpeechProvider};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;
use tauri::{AppHandle, Emitter, Manager, State};

pub const PARTIAL_EVENT: &str = "theta-speech-partial";
pub const RESULT_EVENT: &str = "theta-speech-result";
pub const ERROR_EVENT: &str = "theta-speech-error";
pub const LEVEL_EVENT: &str = "theta-level";
pub const DEBUG_EVENT: &str = "theta-debug";
pub const LISTENING_EVENT: &str = "theta-listening";

const DEBUG_LOG_LIMIT: usize = 500;

/// `HH:MM:SS.mmm` for debug lines — relative ordering is all we need.
pub fn now_stamp() -> String {
    let dur = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs() as i64;
    let millis = dur.subsec_millis();
    let h = ((secs / 3600) % 24) as u32;
    let m = ((secs / 60) % 60) as u32;
    let s = (secs % 60) as u32;
    format!("{h:02}:{m:02}:{s:02}.{millis:03}")
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Serialize, Clone, Copy, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCapability {
    pub id: SpeechProvider,
    pub label: &'static str,
    pub supported: bool,
}

struct Session {
    active: bool,
    cancel: Option<Arc<AtomicBool>>,
}

pub struct SttState {
    session: Mutex<Session>,
    generation: Arc<AtomicU64>,
    last_partial: Arc<Mutex<String>>,
    debug_log: Arc<Mutex<Vec<String>>>,
    capture: Arc<Mutex<()>>,
    models: Arc<vosk::ModelCache>,
}

impl SttState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(Session {
                active: false,
                cancel: None,
            }),
            generation: Arc::new(AtomicU64::new(0)),
            last_partial: Arc::new(Mutex::new(String::new())),
            debug_log: Arc::new(Mutex::new(Vec::new())),
            capture: Arc::new(Mutex::new(())),
            models: Arc::new(vosk::ModelCache::new()),
        }
    }

    /// Mirror a line to stderr, the ring buffer, and the `theta-debug` stream.
    pub fn log(&self, app: &AppHandle, msg: impl Into<String>) {
        push_log(app, &self.debug_log, msg.into());
    }

    fn is_active(&self) -> bool {
        guard(&self.session).active
    }
}

impl Default for SttState {
    fn default() -> Self {
        Self::new()
    }
}

fn trim_log(lines: &mut Vec<String>) {
    if lines.len() > DEBUG_LOG_LIMIT {
        let excess = lines.len() - DEBUG_LOG_LIMIT;
        lines.drain(0..excess);
    }
}

fn session_is_current(current: &AtomicU64, generation: u64) -> bool {
    current.load(Ordering::SeqCst) == generation
}

fn session_is_cancelled(cancel: &AtomicBool, current: &AtomicU64, generation: u64) -> bool {
    cancel.load(Ordering::SeqCst) || !session_is_current(current, generation)
}

fn push_log(app: &AppHandle, log: &Mutex<Vec<String>>, msg: String) {
    let entry = format!("[{}] {}", now_stamp(), msg);
    eprintln!("[theta] {entry}");
    {
        let mut lines = guard(log);
        lines.push(entry.clone());
        trim_log(&mut lines);
    }
    let _ = app.emit(DEBUG_EVENT, entry);
}

/// The handle a backend uses to report progress.
///
/// Every emit is gated on the session still being the current one, so a
/// recognizer that is still shutting down cannot overwrite transcripts or the
/// listening state belonging to its successor.
pub struct SessionSink {
    app: AppHandle,
    generation: u64,
    current: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    last_partial: Arc<Mutex<String>>,
    debug_log: Arc<Mutex<Vec<String>>>,
}

impl SessionSink {
    pub fn is_current(&self) -> bool {
        session_is_current(&self.current, self.generation)
    }

    pub fn is_cancelled(&self) -> bool {
        session_is_cancelled(&self.cancel, &self.current, self.generation)
    }

    pub fn log(&self, msg: impl Into<String>) {
        push_log(&self.app, &self.debug_log, msg.into());
    }

    pub fn partial(&self, text: &str) {
        if !self.is_current() {
            return;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let _ = self.app.emit(PARTIAL_EVENT, trimmed.to_string());
        *guard(&self.last_partial) = trimmed.to_string();
    }

    pub fn has_partial(&self) -> bool {
        !guard(&self.last_partial).is_empty()
    }

    pub fn publish_final(&self, text: &str) {
        if !self.is_current() {
            return;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        let _ = self.app.emit(RESULT_EVENT, trimmed.to_string());
        let _ = self.app.emit(PARTIAL_EVENT, "");
        guard(&self.last_partial).clear();
        self.log(format!("FINAL: {trimmed:?}"));
    }

    pub fn level(&self, level: i32) {
        if !self.is_current() {
            return;
        }
        let _ = self.app.emit(LEVEL_EVENT, level);
    }

    pub fn error(&self, message: &str) {
        self.log(format!("error: {message}"));
        if !self.is_current() {
            return;
        }
        let _ = self.app.emit(ERROR_EVENT, message.to_string());
    }
}

fn provider_for(app: &AppHandle) -> SpeechProvider {
    app.try_state::<SettingsState>()
        .map(|settings| settings.snapshot().speech_recognition_provider)
        .unwrap_or_default()
}

fn unsupported(provider: SpeechProvider) -> String {
    format!("{} isn't available on this platform.", provider.label())
}

/// Starts capture on a background thread. Idempotent: a second call while
/// already listening is a no-op rather than an error, so a double-tapped
/// hotkey doesn't surface a spurious failure in the UI.
///
/// This is the plain-function form so the tray menu and the Rust-side global
/// shortcut can start listening without a `tauri::State` handle; the
/// `#[tauri::command]` below is a thin wrapper over it.
pub fn start(app_handle: &AppHandle, state: &SttState) -> Result<(), String> {
    let provider = provider_for(app_handle);
    if !provider.is_supported() {
        return Err(unsupported(provider));
    }

    let (generation, cancel) = {
        let mut session = guard(&state.session);
        if session.active {
            return Ok(());
        }
        let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        session.active = true;
        session.cancel = Some(Arc::clone(&cancel));
        (generation, cancel)
    };

    guard(&state.last_partial).clear();
    state.log(
        app_handle,
        format!("start_listening: requested via {}", provider.label()),
    );
    let _ = app_handle.emit(LISTENING_EVENT, true);

    let sink = Arc::new(SessionSink {
        app: app_handle.clone(),
        generation,
        current: Arc::clone(&state.generation),
        cancel,
        last_partial: Arc::clone(&state.last_partial),
        debug_log: Arc::clone(&state.debug_log),
    });
    let capture = Arc::clone(&state.capture);
    let models = Arc::clone(&state.models);
    let app_for_thread = app_handle.clone();

    std::thread::spawn(move || {
        // Sessions take turns on the microphone: a replacement engine waits
        // for its predecessor to let go rather than fighting it for the
        // device. By the time the lock is free the older session has already
        // been superseded, so it exits without touching any hardware.
        let held = guard(&capture);
        let outcome = if sink.is_cancelled() {
            Ok(())
        } else {
            run_backend(provider, &sink, &models)
        };
        drop(held);

        if let Err(message) = outcome {
            sink.error(&message);
        }
        sink.log("listening thread exited");
        finish_session(&app_for_thread, generation);
    });

    Ok(())
}

fn run_backend(
    provider: SpeechProvider,
    sink: &Arc<SessionSink>,
    models: &vosk::ModelCache,
) -> Result<(), String> {
    match provider {
        SpeechProvider::Vosk => vosk::run(sink, models),
        SpeechProvider::Windows => run_windows(sink),
    }
}

#[cfg(target_os = "windows")]
fn run_windows(sink: &Arc<SessionSink>) -> Result<(), String> {
    windows::run(sink)
}

#[cfg(not(target_os = "windows"))]
fn run_windows(_sink: &Arc<SessionSink>) -> Result<(), String> {
    Err(unsupported(SpeechProvider::Windows))
}

/// Signals the capture thread to wind down. Also plain-function form.
pub fn stop(app_handle: &AppHandle, state: &SttState) {
    let cancel = {
        let mut session = guard(&state.session);
        session.active = false;
        session.cancel.take()
    };
    if let Some(cancel) = cancel {
        cancel.store(true, Ordering::SeqCst);
        state.log(app_handle, "stop_listening: requested");
    }
    guard(&state.last_partial).clear();
    let _ = app_handle.emit(PARTIAL_EVENT, "");
    let _ = app_handle.emit(LISTENING_EVENT, false);
}

/// Ends whatever session is running, for callers that only hold an
/// `AppHandle` — notably a settings save that switches engines.
pub fn stop_active_session(app_handle: &AppHandle) {
    if let Some(state) = app_handle.try_state::<SttState>() {
        stop(app_handle, &state);
    }
}

fn finish_session(app_handle: &AppHandle, generation: u64) {
    let Some(state) = app_handle.try_state::<SttState>() else {
        return;
    };
    if state.generation.load(Ordering::SeqCst) != generation {
        return;
    }
    {
        let mut session = guard(&state.session);
        session.active = false;
        session.cancel = None;
    }
    guard(&state.last_partial).clear();
    let _ = app_handle.emit(PARTIAL_EVENT, "");
    let _ = app_handle.emit(LEVEL_EVENT, 0);
    let _ = app_handle.emit(LISTENING_EVENT, false);
}

/// Flips listening on or off. Used by the tray menu and global shortcut.
pub fn toggle(app_handle: &AppHandle, state: &SttState) -> Result<bool, String> {
    let active = state.is_active();
    if active {
        stop(app_handle, state);
        Ok(false)
    } else {
        start(app_handle, state)?;
        Ok(true)
    }
}

#[tauri::command]
pub fn start_listening(app_handle: AppHandle, state: State<'_, SttState>) -> Result<(), String> {
    start(&app_handle, &state)
}

#[tauri::command]
pub fn stop_listening(app_handle: AppHandle, state: State<'_, SttState>) {
    stop(&app_handle, &state)
}

#[tauri::command]
pub fn toggle_listening(app_handle: AppHandle, state: State<'_, SttState>) -> Result<bool, String> {
    toggle(&app_handle, &state)
}

#[tauri::command]
pub fn is_listening(state: State<'_, SttState>) -> bool {
    state.is_active()
}

#[tauri::command]
pub fn get_debug_log(state: State<'_, SttState>) -> Vec<String> {
    guard(&state.debug_log).clone()
}

/// Names of available input devices, for the settings panel.
#[tauri::command]
pub fn list_input_devices() -> Result<Vec<String>, String> {
    vosk::list_input_devices()
}

/// Which engines this build can actually run, so the UI can disable the rest.
#[tauri::command]
pub fn speech_providers() -> Vec<ProviderCapability> {
    [SpeechProvider::Vosk, SpeechProvider::Windows]
        .into_iter()
        .map(|id| ProviderCapability {
            id,
            label: id.label(),
            supported: id.is_supported(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_log_keeps_the_most_recent_lines() {
        let mut lines: Vec<String> = (0..(DEBUG_LOG_LIMIT + 25))
            .map(|index| format!("line {index}"))
            .collect();
        trim_log(&mut lines);
        assert_eq!(lines.len(), DEBUG_LOG_LIMIT);
        assert_eq!(lines[0], "line 25");
        assert_eq!(lines[DEBUG_LOG_LIMIT - 1], format!("line {}", DEBUG_LOG_LIMIT + 24));
    }

    #[test]
    fn trim_log_leaves_short_logs_alone() {
        let mut lines = vec!["only".to_string()];
        trim_log(&mut lines);
        assert_eq!(lines, vec!["only".to_string()]);
    }

    #[test]
    fn a_superseded_session_is_never_current() {
        let current = AtomicU64::new(4);
        assert!(session_is_current(&current, 4));
        current.store(5, Ordering::SeqCst);
        assert!(!session_is_current(&current, 4));
    }

    #[test]
    fn a_superseded_session_reads_as_cancelled() {
        let current = AtomicU64::new(7);
        let cancel = AtomicBool::new(false);
        assert!(!session_is_cancelled(&cancel, &current, 7));

        current.store(8, Ordering::SeqCst);
        assert!(
            session_is_cancelled(&cancel, &current, 7),
            "a replaced session must stop even though nobody set its cancel flag"
        );
    }

    #[test]
    fn an_explicit_stop_cancels_the_current_session() {
        let current = AtomicU64::new(2);
        let cancel = AtomicBool::new(false);
        cancel.store(true, Ordering::SeqCst);
        assert!(session_is_cancelled(&cancel, &current, 2));
        assert!(
            session_is_current(&current, 2),
            "a stopped session is still the current one, so its final flush publishes"
        );
    }

    #[test]
    fn providers_report_platform_support() {
        let providers = speech_providers();
        assert_eq!(providers.len(), 2);

        let vosk = providers
            .iter()
            .find(|provider| matches!(provider.id, SpeechProvider::Vosk))
            .expect("vosk should be listed");
        assert!(vosk.supported);

        let windows = providers
            .iter()
            .find(|provider| matches!(provider.id, SpeechProvider::Windows))
            .expect("windows should be listed");
        assert_eq!(windows.supported, cfg!(target_os = "windows"));
    }

    #[test]
    fn a_new_state_is_idle() {
        let state = SttState::new();
        assert!(!state.is_active());
        assert_eq!(state.generation.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn unsupported_message_names_the_provider() {
        assert!(unsupported(SpeechProvider::Windows).contains("Windows Speech Recognition"));
    }
}
