use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::SystemTime;
use tauri::{AppHandle, Emitter, State};
use vosk::{CompleteResult, DecodingState, Model, Recognizer};
pub struct ListeningState {
    is_listening: Arc<AtomicBool>,
    model: Arc<Model>,
}

impl ListeningState {
    pub fn new(model_path: &str) -> Self {
        let model = Model::new(model_path)
            .unwrap_or_else(|| panic!("Failed to load Vosk model at '{model_path}'"));
        Self {
            is_listening: Arc::new(AtomicBool::new(false)),
            model: Arc::new(model),
        }
    }
}

#[tauri::command]
fn start_listening(app_handle: AppHandle, state: State<'_, ListeningState>) -> Result<(), String> {
    if state.is_listening.swap(true, Ordering::SeqCst) {
        return Err("Already listening".to_string());
    }

    let is_listening = Arc::clone(&state.is_listening);
    let model = Arc::clone(&state.model);
    let app_for_thread = app_handle.clone();

    std::thread::spawn(move || {
        if let Err(e) = run_listening_loop(app_for_thread.clone(), model, Arc::clone(&is_listening)) {
            eprintln!("Listening thread error: {e}");
            let _ = app_for_thread.emit("vosk-error", e);
        }
        is_listening.store(false, Ordering::SeqCst);
    });

    Ok(())
}

#[tauri::command]
fn stop_listening(state: State<'_, ListeningState>) {
    state.is_listening.store(false, Ordering::SeqCst);
}

fn run_listening_loop(
    app_handle: AppHandle,
    model: Arc<Model>,
    is_listening: Arc<AtomicBool>,
) -> Result<(), String> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or("No input device found")?;
    let supported_config = device
        .default_input_config()
        .map_err(|e| format!("Failed to get input config: {e}"))?;

    // cpal will hand you whatever format the OS default device happens
    // to use (F32, I16, or U16) — it does NOT convert for you. Check it
    // instead of assuming F32, or build_input_stream::<f32,_,_> will
    // fail at runtime on devices that don't use F32.
    let sample_format = supported_config.sample_format();
    if sample_format != cpal::SampleFormat::F32 {
        return Err(format!(
            "Unsupported input sample format: {sample_format:?}. \
             Add a matching build_input_stream::<T,_,_> branch for this format."
        ));
    }
    let config: cpal::StreamConfig = supported_config.into();

    let mut recognizer = Recognizer::new(&model, 16000.0).ok_or("Failed to create recognizer")?;
    recognizer.set_words(true);

    let app_for_stream = app_handle.clone();
    let stream = device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &_| {
                process_samples(&mut recognizer, data, &app_for_stream);
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        )
        .map_err(|e| format!("Failed to build input stream: {e}"))?;

    stream
        .play()
        .map_err(|e| format!("Failed to start audio stream: {e}"))?;

    while is_listening.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    drop(stream); // stops audio capture
    Ok(())
}

/// Feeds one chunk of audio into the recognizer and emits either a
/// final result or a partial result to the frontend.
fn process_samples(recognizer: &mut Recognizer, data: &[f32], app_handle: &AppHandle) {
    let i16_buffer: Vec<i16> = data
        .iter()
        .map(|&sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16)
        .collect();

    match recognizer.accept_waveform(&i16_buffer) {
        Ok(DecodingState::Finalized) => {
            if let CompleteResult::Single(result) = recognizer.result() {
                if !result.text.is_empty() {
                    if let Err(e) = app_handle.emit("vosk-speech-result", result.text.to_string()) {
                        eprintln!("Failed to emit vosk-speech-result: {e}");
                    }
                }
            }
        }
        Ok(_) => {
            let partial = recognizer.partial_result();
            if !partial.partial.is_empty() {
                if let Err(e) = app_handle.emit("vosk-speech-partial", partial.partial.to_string()) {
                    eprintln!("Failed to emit vosk-speech-partial: {e}");
                }
            }
        }
        Err(e) => eprintln!("accept_waveform error: {e}"),
    }
}

// ---------------------------------------------------------------------
// Your existing commands, unchanged
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // TODO: replace with the real path to your unzipped Vosk model,
    // or resolve it from Tauri's resource dir (see note below).
    const MODEL_PATH: &str = "path/to/vosk-model-small-en-us-0.18";

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_audio_recorder::init())
        .manage(ListeningState::new(MODEL_PATH))
        .invoke_handler(tauri::generate_handler![
            greet,
            get_system_info,
            read_file_content,
            write_file_content,
            list_directory,
            start_listening,
            stop_listening,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub mod temp;