//! Local speech-to-text on top of Vosk + cpal.
//!
//! Captures from the default input device in whatever native format it
//! offers, down-mixes to mono, linearly resamples to the 16 kHz Vosk wants,
//! and streams partial/final transcripts to the frontend as Tauri events.
//!
//! Events emitted:
//!   `vosk-speech-partial` (String)  — in-progress text, replaces previous
//!   `vosk-speech-result`  (String)  — finalized utterance
//!   `vosk-error`          (String)  — capture/recognizer failure
//!   `theta-level`         (i32)     — 0..1000 mic level, for the waveform
//!   `theta-debug`         (String)  — timestamped log line
//!   `theta-listening`     (bool)    — authoritative listening state

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tauri::{AppHandle, Emitter, State};
use vosk::{CompleteResult, DecodingState, Model, Recognizer};

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

/// Vosk fixes the recognizer sample rate; everything is resampled to this.
const VOSK_RATE: f32 = 16000.0;

/// RMS (in normalized 0..1 units) below which a frame counts as silence.
const SILENCE_RMS: f32 = 0.012;

/// How long the input has to stay quiet before we force-finalize whatever
/// partial text we have. Vosk's own endpointing is conservative, so without
/// this the assistant feels laggy after the user stops talking.
const ENDPOINT_SILENCE_MS: u64 = 900;

pub struct SttState {
    pub is_listening: Arc<AtomicBool>,
    model: Arc<Model>,
    last_partial: Arc<Mutex<String>>,
    debug_log: Arc<Mutex<Vec<String>>>,
}

impl SttState {
    pub fn new(model_path: &str) -> Self {
        eprintln!("[theta] Loading Vosk model from {model_path}");
        let model = Model::new(model_path).unwrap_or_else(|| {
            panic!(
                "[theta] Failed to load Vosk model from '{model_path}'. \
                 Ensure the directory exists and is a valid Vosk model."
            )
        });
        eprintln!("[theta] Vosk model loaded");
        Self {
            is_listening: Arc::new(AtomicBool::new(false)),
            model: Arc::new(model),
            last_partial: Arc::new(Mutex::new(String::new())),
            debug_log: Arc::new(Mutex::new(vec![format!(
                "[{}] model loaded: {model_path}",
                now_stamp()
            )])),
        }
    }

    /// Mirror a line to stderr, the ring buffer, and the `theta-debug` stream.
    pub fn log(&self, app: &AppHandle, msg: impl Into<String>) {
        let entry = format!("[{}] {}", now_stamp(), msg.into());
        eprintln!("[theta] {entry}");
        if let Ok(mut log) = self.debug_log.lock() {
            log.push(entry.clone());
            if log.len() > 500 {
                let drop = log.len() - 500;
                log.drain(0..drop);
            }
        }
        let _ = app.emit("theta-debug", entry);
    }
}

/// Starts capture on a background thread. Idempotent: a second call while
/// already listening is a no-op rather than an error, so a double-tapped
/// hotkey doesn't surface a spurious failure in the UI.
///
/// This is the plain-function form so the tray menu and the Rust-side global
/// shortcut can start listening without a `tauri::State` handle; the
/// `#[tauri::command]` below is a thin wrapper over it.
pub fn start(app_handle: &AppHandle, state: &SttState) -> Result<(), String> {
    if state.is_listening.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    if let Ok(mut p) = state.last_partial.lock() {
        p.clear();
    }
    state.log(app_handle, "start_listening: requested");
    let _ = app_handle.emit("theta-listening", true);

    let is_listening = Arc::clone(&state.is_listening);
    let model = Arc::clone(&state.model);
    let last_partial = Arc::clone(&state.last_partial);
    let app_for_thread = app_handle.clone();

    std::thread::spawn(move || {
        if let Err(e) = run_listening_loop(
            app_for_thread.clone(),
            model,
            Arc::clone(&is_listening),
            last_partial,
        ) {
            eprintln!("[theta] listening thread error: {e}");
            let _ = app_for_thread.emit("vosk-error", e.clone());
            let _ = app_for_thread.emit(
                "theta-debug",
                format!("[{}] start_listening error: {e}", now_stamp()),
            );
        }
        is_listening.store(false, Ordering::SeqCst);
        let _ = app_for_thread.emit("theta-listening", false);
        let _ = app_for_thread.emit(
            "theta-debug",
            format!("[{}] listening thread exited", now_stamp()),
        );
    });

    Ok(())
}

/// Signals the capture thread to wind down. Also plain-function form.
pub fn stop(app_handle: &AppHandle, state: &SttState) {
    state.log(app_handle, "stop_listening: requested");
    state.is_listening.store(false, Ordering::SeqCst);
    let _ = app_handle.emit("vosk-speech-partial", "");
    let _ = app_handle.emit("theta-listening", false);
}

/// Flips listening on or off. Used by the tray menu and global shortcut.
pub fn toggle(app_handle: &AppHandle, state: &SttState) -> Result<bool, String> {
    if state.is_listening.load(Ordering::SeqCst) {
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
    state.is_listening.load(Ordering::SeqCst)
}

#[tauri::command]
pub fn get_debug_log(state: State<'_, SttState>) -> Vec<String> {
    state
        .debug_log
        .lock()
        .map(|log| log.clone())
        .unwrap_or_default()
}

/// Names of available input devices, for the settings panel.
#[tauri::command]
pub fn list_input_devices() -> Result<Vec<String>, String> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|e| format!("Failed to enumerate input devices: {e}"))?;
    Ok(devices.map(|d| d.to_string()).collect())
}

/// Everything the audio callback touches, in one place.
///
/// The original code duplicated this body once per `cpal::SampleFormat`.
/// Each format branch now only converts its native sample type to
/// interleaved `f32` and calls [`Pipeline::feed`].
struct Pipeline {
    app: AppHandle,
    recognizer: Arc<Mutex<Recognizer>>,
    last_partial: Arc<Mutex<String>>,
    channels: usize,
    src_rate: u32,
    /// Carries the fractional resample offset across callbacks so chunk
    /// boundaries don't click.
    resample_acc: Mutex<f64>,
    level_tick: AtomicU32,
    /// Millis-since-epoch of the last frame loud enough to count as speech.
    last_voice_ms: AtomicU64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Pipeline {
    fn feed(&self, interleaved: &[f32]) {
        let mono: Vec<f32> = if self.channels <= 1 {
            interleaved.to_vec()
        } else {
            interleaved
                .chunks(self.channels)
                .map(|frame| frame.iter().sum::<f32>() / self.channels as f32)
                .collect()
        };
        if mono.is_empty() {
            return;
        }

        // RMS on the normalized mono signal, before the i16 conversion, so
        // the silence threshold is a plain 0..1 amplitude.
        let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
        if rms > SILENCE_RMS {
            self.last_voice_ms.store(now_ms(), Ordering::Relaxed);
        }

        // Emit a level roughly 1 in every 3 callbacks — enough for a smooth
        // waveform without flooding the event bridge.
        if self.level_tick.fetch_add(1, Ordering::Relaxed) % 3 == 0 {
            let level = (rms * 4000.0).min(1000.0).round() as i32;
            let _ = self.app.emit("theta-level", level);
        }

        let samples = resample_to_i16(&mono, self.src_rate, VOSK_RATE as u32, &self.resample_acc);
        if samples.is_empty() {
            return;
        }

        let Ok(mut rec) = self.recognizer.lock() else {
            return;
        };

        match rec.accept_waveform(&samples) {
            Ok(DecodingState::Finalized) => {
                if let CompleteResult::Single(result) = rec.result() {
                    self.publish_final(result.text.trim());
                }
            }
            Ok(_) => {
                let ptext = rec.partial_result().partial.trim().to_string();
                if !ptext.is_empty() {
                    let _ = self.app.emit("vosk-speech-partial", ptext.clone());
                    if let Ok(mut p) = self.last_partial.lock() {
                        *p = ptext;
                    }
                }
                // Vosk endpoints conservatively. If we're sitting on text and
                // the room has gone quiet, cut the utterance ourselves.
                let quiet_for = now_ms().saturating_sub(self.last_voice_ms.load(Ordering::Relaxed));
                let have_text = self
                    .last_partial
                    .lock()
                    .map(|p| !p.is_empty())
                    .unwrap_or(false);
                if have_text && quiet_for > ENDPOINT_SILENCE_MS {
                    if let CompleteResult::Single(result) = rec.final_result() {
                        let text = result.text.trim().to_string();
                        self.publish_final(&text);
                    }
                    rec.reset();
                }
            }
            Err(e) => {
                eprintln!("[theta] accept_waveform error: {e}");
                let _ = self.app.emit(
                    "theta-debug",
                    format!("[{}] accept_waveform error: {e}", now_stamp()),
                );
            }
        }
    }

    fn publish_final(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let _ = self.app.emit("vosk-speech-result", text.to_string());
        let _ = self.app.emit("vosk-speech-partial", "");
        let _ = self
            .app
            .emit("theta-debug", format!("[{}] FINAL: {text:?}", now_stamp()));
        if let Ok(mut p) = self.last_partial.lock() {
            p.clear();
        }
    }
}

fn run_listening_loop(
    app_handle: AppHandle,
    model: Arc<Model>,
    is_listening: Arc<AtomicBool>,
    last_partial: Arc<Mutex<String>>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or(
        "No microphone found. Plug one in or pick an input device in Windows sound settings.",
    )?;
    let supported = device
        .default_input_config()
        .map_err(|e| format!("Failed to get input config: {e}"))?;

    let src_rate: u32 = supported.sample_rate();
    let channels = supported.channels() as usize;
    let sample_format = supported.sample_format();
    let device_name = device.to_string();

    let _ = app_handle.emit(
        "theta-debug",
        format!(
            "[{}] device: {device_name} | ch={channels} rate={src_rate} fmt={sample_format:?}",
            now_stamp()
        ),
    );

    let mut recognizer =
        Recognizer::new(&model, VOSK_RATE).ok_or("Failed to create Vosk recognizer")?;
    recognizer.set_words(true);
    let recognizer = Arc::new(Mutex::new(recognizer));

    let pipeline = Arc::new(Pipeline {
        app: app_handle.clone(),
        recognizer: Arc::clone(&recognizer),
        last_partial: Arc::clone(&last_partial),
        channels,
        src_rate,
        resample_acc: Mutex::new(0.0),
        level_tick: AtomicU32::new(0),
        last_voice_ms: AtomicU64::new(now_ms()),
    });

    let config: cpal::StreamConfig = supported.into();
    let on_err = |err| eprintln!("[theta] audio stream error: {err}");

    // One arm per native sample format; each converts to f32 and delegates.
    let stream = match sample_format {
        cpal::SampleFormat::F32 => {
            let p = Arc::clone(&pipeline);
            device.build_input_stream(
                config,
                move |data: &[f32], _: &_| p.feed(data),
                on_err,
                None,
            )
        }
        cpal::SampleFormat::I16 => {
            let p = Arc::clone(&pipeline);
            device.build_input_stream(
                config,
                move |data: &[i16], _: &_| {
                    let f: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                    p.feed(&f);
                },
                on_err,
                None,
            )
        }
        cpal::SampleFormat::U16 => {
            let p = Arc::clone(&pipeline);
            device.build_input_stream(
                config,
                move |data: &[u16], _: &_| {
                    let f: Vec<f32> = data
                        .iter()
                        .map(|&s| (s as f32 - 32768.0) / 32768.0)
                        .collect();
                    p.feed(&f);
                },
                on_err,
                None,
            )
        }
        fmt => return Err(format!("Unsupported sample format: {fmt:?}")),
    }
    .map_err(|e| format!("Failed to build input stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Failed to start audio stream: {e}"))?;
    let _ = app_handle.emit(
        "theta-debug",
        format!("[{}] stream.play() OK — listening", now_stamp()),
    );

    while is_listening.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(80));
    }

    let _ = app_handle.emit(
        "theta-debug",
        format!("[{}] is_listening=false — stopping stream", now_stamp()),
    );
    drop(stream);
    let _ = app_handle.emit("theta-level", 0);

    // A manual stop can leave audio sitting in Vosk's buffer waiting for
    // trailing silence that will never come. Flush it.
    if let Ok(mut rec) = recognizer.lock() {
        if let CompleteResult::Single(result) = rec.final_result() {
            let text = result.text.trim().to_string();
            if !text.is_empty() {
                let _ = app_handle.emit("vosk-speech-result", text.clone());
                let _ = app_handle.emit(
                    "theta-debug",
                    format!("[{}] FINAL (flushed on stop): {text:?}", now_stamp()),
                );
                if let Ok(mut p) = last_partial.lock() {
                    p.clear();
                }
            }
        }
    }

    Ok(())
}

/// Linear resampler, mono f32 in → mono i16 at `dst_rate`.
///
/// `acc` holds the leftover fractional read position between calls, which is
/// what keeps consecutive callbacks from producing a seam.
fn resample_to_i16(input: &[f32], src_rate: u32, dst_rate: u32, acc: &Mutex<f64>) -> Vec<i16> {
    let to_i16 = |s: f32| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;

    if src_rate == dst_rate {
        return input.iter().copied().map(to_i16).collect();
    }

    let ratio = src_rate as f64 / dst_rate as f64;
    let mut guard = match acc.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    let mut pos = *guard;
    let mut out = Vec::with_capacity((input.len() as f64 / ratio).ceil() as usize + 1);
    while pos < input.len() as f64 {
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;
        let s0 = input.get(idx).copied().unwrap_or(0.0);
        let s1 = input.get(idx + 1).copied().unwrap_or(s0);
        out.push(to_i16(s0 + (s1 - s0) * frac));
        pos += ratio;
    }
    *guard = (pos - input.len() as f64).max(0.0);
    out
}
