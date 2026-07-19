use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tauri::{AppHandle, Emitter, State};
use vosk::{CompleteResult, DecodingState, Model, Recognizer};

/// Timestamp string for debug log lines: `HH:MM:SS.mmm` (UTC-ish, good
/// enough for a relative-ordered debug panel).
fn now_stamp() -> String {
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

pub struct ListeningState {
    is_listening: Arc<AtomicBool>,
    model: Arc<Model>,
    /// Last partial text captured by the stream callback, so that when the
    /// user stops listening we can flush it as a final result instead of
    /// silently dropping whatever they were saying.
    last_partial: Arc<Mutex<String>>,
    debug_log: Arc<Mutex<Vec<String>>>,
}

impl ListeningState {
    pub fn new(model_path: &str) -> Self {
        eprintln!("[theta] Loading Vosk model from {model_path}");
        let model = Model::new(model_path).unwrap_or_else(|| {
            panic!(
                "[theta] Failed to load Vosk model from '{model_path}'. \
                Ensure the model directory exists and is a valid Vosk model."
            )
        });
        eprintln!("[theta] Vosk model loaded successfully");
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

    /// Push a debug line to stderr, the in-memory log buffer, and the
    /// `theta-debug` event stream (consumed by the mini console in the UI).
    fn log(&self, app: &AppHandle, msg: impl Into<String>) {
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

#[tauri::command]
fn start_listening(app_handle: AppHandle, state: State<'_, ListeningState>) -> Result<(), String> {
    if state.is_listening.swap(true, Ordering::SeqCst) {
        return Err("Already listening".to_string());
    }

    if let Ok(mut p) = state.last_partial.lock() {
        p.clear();
    }
    state.log(&app_handle, "start_listening: requested");

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
            eprintln!("Listening thread error: {e}");
            let _ = app_for_thread.emit("vosk-error", e.clone());
            let _ = app_for_thread.emit(
                "theta-debug",
                format!("[{}] start_listening error: {e}", now_stamp()),
            );
        }
        is_listening.store(false, Ordering::SeqCst);
        let _ = app_for_thread.emit(
            "theta-debug",
            format!("[{}] listening thread exited", now_stamp()),
        );
    });

    Ok(())
}

#[tauri::command]
fn stop_listening(app_handle: AppHandle, state: State<'_, ListeningState>) {
    state.log(&app_handle, "stop_listening: requested");
    state.is_listening.store(false, Ordering::SeqCst);

    let _ = app_handle.emit("vosk-speech-partial", "");
}

#[tauri::command]
fn get_debug_log(state: State<'_, ListeningState>) -> Vec<String> {
    state
        .debug_log
        .lock()
        .map(|log| log.clone())
        .unwrap_or_default()
}

fn run_listening_loop(
    app_handle: AppHandle,
    model: Arc<Model>,
    is_listening: Arc<AtomicBool>,
    last_partial: Arc<Mutex<String>>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host.default_input_device().ok_or("No input device found")?;
    let supported_config = device
        .default_input_config()
        .map_err(|e| format!("Failed to get input config: {e}"))?;

    // Extract info before supported_config is consumed by .into()
    let device_sample_rate: u32 = supported_config.sample_rate();
    let device_channels = supported_config.channels() as usize;
    let sample_format = supported_config.sample_format();
    // Get device name before device is moved into closures
    let device_name = device.to_string();

    let _ = app_handle.emit(
        "theta-debug",
        format!(
            "[{}] device: {} | ch={} rate={} fmt={:?}",
            now_stamp(),
            device_name,
            device_channels,
            device_sample_rate,
            sample_format,
        ),
    );

    // Vosk requires 16 kHz mono i16 audio.
    // We always feed the recognizer at 16000 Hz regardless of device rate.
    const VOSK_RATE: f32 = 16000.0;
    let mut recognizer = Recognizer::new(&model, VOSK_RATE).ok_or("Failed to create recognizer")?;
    recognizer.set_words(true);

    // Shared recognizer behind a mutex so the stream callback (which may run
    // on a separate audio thread) can safely hand samples to Vosk.
    let recognizer = Arc::new(Mutex::new(recognizer));

    let config: cpal::StreamConfig = supported_config.into();
    let app_for_stream = app_handle.clone();
    let last_partial_for_stream = Arc::clone(&last_partial);
    let level_tick = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let level_tick_stream = Arc::clone(&level_tick);
    let recognizer_stream = Arc::clone(&recognizer);

    // Accumulate fractional resampling error across callbacks.
    let resample_acc = Arc::new(Mutex::new(0.0f64));
    let resample_acc_stream = Arc::clone(&resample_acc);

    // Build the stream using the device's native format.
    // We down-mix to mono and linearly resample to VOSK_RATE on the fly.
    let build_result = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            config.clone(),
            move |data: &[f32], _: &_| {
                let mono: Vec<f32> = if device_channels == 1 {
                    data.to_vec()
                } else {
                    data.chunks(device_channels)
                        .map(|ch| ch.iter().sum::<f32>() / device_channels as f32)
                        .collect()
                };
                let resampled = resample_f32(
                    &mono,
                    device_sample_rate,
                    VOSK_RATE as u32,
                    &resample_acc_stream,
                );
                let tick = level_tick_stream.fetch_add(1, Ordering::Relaxed);
                if tick % 10 == 0 {
                    let rms = (resampled
                        .iter()
                        .map(|s| (*s as f32) * (*s as f32))
                        .sum::<f32>()
                        / resampled.len().max(1) as f32)
                        .sqrt();
                    let _ = app_for_stream.emit(
                        "theta-level",
                        (rms / i16::MAX as f32 * 1000.0).round() as i32,
                    );
                }
                if let Ok(mut rec) = recognizer_stream.lock() {
                    process_samples_i16(
                        &mut rec,
                        &resampled,
                        &app_for_stream,
                        &last_partial_for_stream,
                    );
                }
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            config.clone(),
            move |data: &[i16], _: &_| {
                let f32_data: Vec<f32> = data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                let mono: Vec<f32> = if device_channels == 1 {
                    f32_data
                } else {
                    f32_data
                        .chunks(device_channels)
                        .map(|ch| ch.iter().sum::<f32>() / device_channels as f32)
                        .collect()
                };
                let resampled = resample_f32(
                    &mono,
                    device_sample_rate,
                    VOSK_RATE as u32,
                    &resample_acc_stream,
                );
                let tick = level_tick_stream.fetch_add(1, Ordering::Relaxed);
                if tick % 10 == 0 {
                    let rms = (resampled
                        .iter()
                        .map(|s| (*s as f32) * (*s as f32))
                        .sum::<f32>()
                        / resampled.len().max(1) as f32)
                        .sqrt();
                    let _ = app_for_stream.emit(
                        "theta-level",
                        (rms / i16::MAX as f32 * 1000.0).round() as i32,
                    );
                }
                if let Ok(mut rec) = recognizer_stream.lock() {
                    process_samples_i16(
                        &mut rec,
                        &resampled,
                        &app_for_stream,
                        &last_partial_for_stream,
                    );
                }
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            config,
            move |data: &[u16], _: &_| {
                let f32_data: Vec<f32> = data
                    .iter()
                    .map(|&s| (s as f32 - 32768.0) / 32768.0)
                    .collect();
                let mono: Vec<f32> = if device_channels == 1 {
                    f32_data
                } else {
                    f32_data
                        .chunks(device_channels)
                        .map(|ch| ch.iter().sum::<f32>() / device_channels as f32)
                        .collect()
                };
                let resampled = resample_f32(
                    &mono,
                    device_sample_rate,
                    VOSK_RATE as u32,
                    &resample_acc_stream,
                );
                let tick = level_tick_stream.fetch_add(1, Ordering::Relaxed);
                if tick % 10 == 0 {
                    let rms = (resampled
                        .iter()
                        .map(|s| (*s as f32) * (*s as f32))
                        .sum::<f32>()
                        / resampled.len().max(1) as f32)
                        .sqrt();
                    let _ = app_for_stream.emit(
                        "theta-level",
                        (rms / i16::MAX as f32 * 1000.0).round() as i32,
                    );
                }
                if let Ok(mut rec) = recognizer_stream.lock() {
                    process_samples_i16(
                        &mut rec,
                        &resampled,
                        &app_for_stream,
                        &last_partial_for_stream,
                    );
                }
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        ),
        fmt => return Err(format!("Unsupported sample format: {fmt:?}")),
    };

    let stream = build_result.map_err(|e| format!("Failed to build input stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Failed to start audio stream: {e}"))?;
    let _ = app_handle.emit(
        "theta-debug",
        format!("[{}] stream.play() OK - listening", now_stamp()),
    );

    while is_listening.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    let _ = app_handle.emit(
        "theta-debug",
        format!("[{}] is_listening=false - stopping stream", now_stamp()),
    );
    drop(stream);

    // If the user manually stops, the audio might still be in Vosk's buffer
    // waiting for trailing silence. Flush it out now.
    if let Ok(mut rec) = recognizer.lock() {
        if let CompleteResult::Single(result) = rec.final_result() {
            let text = result.text.trim().to_string();
            if !text.is_empty() {
                let _ = app_handle.emit("vosk-speech-result", text.clone());
                let _ = app_handle.emit(
                    "theta-debug",
                    format!("[{}] FINAL (flushed on stop): {:?}", now_stamp(), text),
                );
                if let Ok(mut p) = last_partial.lock() {
                    p.clear();
                }
            }
        }
    }

    Ok(())
}

/// Linear resampler: converts mono f32 audio from `src_rate` to `dst_rate`.
/// `acc` carries the fractional sample offset across callback invocations so
/// there are no seam clicks at chunk boundaries.
fn resample_f32(input: &[f32], src_rate: u32, dst_rate: u32, acc: &Mutex<f64>) -> Vec<i16> {
    if src_rate == dst_rate {
        return input
            .iter()
            .map(|&s| (s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
            .collect();
    }
    let ratio = src_rate as f64 / dst_rate as f64;
    let mut out = Vec::new();
    let mut pos = acc.lock().unwrap_or_else(|e| e.into_inner()).clone();
    while pos < input.len() as f64 {
        let idx = pos as usize;
        let frac = pos - idx as f64;
        let s0 = input.get(idx).copied().unwrap_or(0.0);
        let s1 = input.get(idx + 1).copied().unwrap_or(s0);
        let sample = s0 + (s1 - s0) * frac as f32;
        out.push((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
        pos += ratio;
    }
    // Store remaining fractional offset for next callback
    if let Ok(mut a) = acc.lock() {
        *a = pos - input.len() as f64;
        if *a < 0.0 {
            *a = 0.0;
        }
    }
    out
}

/// Feeds pre-converted i16 mono 16 kHz samples into the recognizer and emits
/// final or partial results to the frontend.
fn process_samples_i16(
    recognizer: &mut Recognizer,
    data: &[i16],
    app_handle: &AppHandle,
    last_partial: &Mutex<String>,
) {
    match recognizer.accept_waveform(data) {
        Ok(DecodingState::Finalized) => {
            if let CompleteResult::Single(result) = recognizer.result() {
                let text = result.text.to_string();
                if !text.is_empty() {
                    let _ = app_handle.emit("vosk-speech-result", text.clone());
                    let _ = app_handle
                        .emit("theta-debug", format!("[{}] FINAL: {text:?}", now_stamp()));
                    if let Ok(mut p) = last_partial.lock() {
                        p.clear();
                    }
                }
            }
        }
        Ok(_) => {
            let partial = recognizer.partial_result();
            let ptext = partial.partial.to_string();
            if !ptext.is_empty() {
                let _ = app_handle.emit("vosk-speech-partial", ptext.clone());
                if let Ok(mut p) = last_partial.lock() {
                    *p = ptext;
                }
            }
        }
        Err(e) => {
            eprintln!("accept_waveform error: {e}");
            let _ = app_handle.emit(
                "theta-debug",
                format!("[{}] accept_waveform error: {e}", now_stamp()),
            );
        }
    }
}

// ---------------------------------------------------------------------
// Existing commands
// ---------------------------------------------------------------------

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

// ---------------------------------------------------------------------
// App entry point
// ---------------------------------------------------------------------

/// Resolves the Vosk model directory.
///
/// Looks for `vosk-models/vosk-model-small-en-us-0.15` relative to the
/// running executable (works for `cargo run`/`tauri dev`, where the exe
/// lives in `target/<profile>/`, by walking up to find `src-tauri`), and
/// also relative to the crate manifest dir (set by Cargo at build time).
/// Panics with a clear message if the model folder can't be found.
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
        .manage(ListeningState::new(&model_path))
        .invoke_handler(tauri::generate_handler![
            greet,
            get_system_info,
            read_file_content,
            write_file_content,
            list_directory,
            start_listening,
            stop_listening,
            get_debug_log,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub mod temp;
