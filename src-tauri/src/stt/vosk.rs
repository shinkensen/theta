

use super::SessionSink;
use ::vosk::{CompleteResult, DecodingState, Model, Recognizer};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

const VOSK_RATE: f32 = 16000.0;

const SILENCE_RMS: f32 = 0.012;
const ENDPOINT_SILENCE_MS: u64 = 900;

const MODEL_DIR_NAME: &str = "vosk-model-small-en-us-0.15";

pub struct ModelCache {
    model: Mutex<Option<Arc<Model>>>,
}

impl ModelCache {
    pub fn new() -> Self {
        Self {
            model: Mutex::new(None),
        }
    }

    fn load(&self, sink: &SessionSink) -> Result<Arc<Model>, String> {
        let mut cached = self
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(model) = cached.as_ref() {
            return Ok(Arc::clone(model));
        }

        let path = resolve_model_path()?;
        let display = path.to_string_lossy().into_owned();
        sink.log(format!("loading Vosk model from {display}"));

        let model = Model::new(display.as_str()).ok_or_else(|| {
            format!(
                "Couldn't load the Vosk model at '{display}'. Check that the folder is a complete \
                 model (am/, conf/, graph/, ivector/), or switch to Windows Speech Recognition in \
                 Settings."
            )
        })?;

        let model = Arc::new(model);
        *cached = Some(Arc::clone(&model));
        sink.log("Vosk model loaded");
        Ok(model)
    }
}

impl Default for ModelCache {
    fn default() -> Self {
        Self::new()
    }
}

fn resolve_model_path() -> Result<PathBuf, String> {
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
                        return Ok(candidate);
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
            return Ok(candidate);
        }
    }

    Err(format!(
        "Vosk model '{MODEL_DIR_NAME}' not found. Expected it under \
         `src-tauri/vosk-models/{MODEL_DIR_NAME}` (containing am/, conf/, graph/, ivector/). \
         Download it from https://alphacephei.com/vosk/models and extract it there, or switch to \
         Windows Speech Recognition in Settings."
    ))
}

pub fn list_input_devices() -> Result<Vec<String>, String> {
    let host = cpal::default_host();
    let devices = host
        .input_devices()
        .map_err(|e| format!("Failed to enumerate input devices: {e}"))?;
    Ok(devices.map(|d| d.to_string()).collect())
}


struct Pipeline {
    sink: Arc<SessionSink>,
    recognizer: Arc<Mutex<Recognizer>>,
    channels: usize,
    src_rate: u32,
  
    resample_acc: Mutex<f64>,
    level_tick: AtomicU32,

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

        let rms = (mono.iter().map(|s| s * s).sum::<f32>() / mono.len() as f32).sqrt();
        if rms > SILENCE_RMS {
            self.last_voice_ms.store(now_ms(), Ordering::Relaxed);
        }

        if self.level_tick.fetch_add(1, Ordering::Relaxed) % 3 == 0 {
            let level = (rms * 4000.0).min(1000.0).round() as i32;
            self.sink.level(level);
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
                    self.sink.publish_final(result.text);
                }
            }
            Ok(_) => {
                self.sink.partial(rec.partial_result().partial);
                let quiet_for = now_ms().saturating_sub(self.last_voice_ms.load(Ordering::Relaxed));
                if self.sink.has_partial() && quiet_for > ENDPOINT_SILENCE_MS {
                    if let CompleteResult::Single(result) = rec.final_result() {
                        self.sink.publish_final(result.text);
                    }
                    rec.reset();
                }
            }
            Err(e) => {
                self.sink.log(format!("accept_waveform error: {e}"));
            }
        }
    }
}

pub fn run(sink: &Arc<SessionSink>, models: &ModelCache) -> Result<(), String> {
    let model = models.load(sink)?;

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

    sink.log(format!(
        "device: {device_name} | ch={channels} rate={src_rate} fmt={sample_format:?}"
    ));

    let mut recognizer =
        Recognizer::new(&model, VOSK_RATE).ok_or("Failed to create Vosk recognizer")?;
    recognizer.set_words(true);
    let recognizer = Arc::new(Mutex::new(recognizer));

    let pipeline = Arc::new(Pipeline {
        sink: Arc::clone(sink),
        recognizer: Arc::clone(&recognizer),
        channels,
        src_rate,
        resample_acc: Mutex::new(0.0),
        level_tick: AtomicU32::new(0),
        last_voice_ms: AtomicU64::new(now_ms()),
    });

    let config: cpal::StreamConfig = supported.into();
    let on_err = |err| eprintln!("[theta] audio stream error: {err}");

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
    sink.log("stream.play() OK — listening");

    while !sink.is_cancelled() {
        std::thread::sleep(std::time::Duration::from_millis(80));
    }

    sink.log("cancelled — stopping stream");
    drop(stream);
    sink.level(0);

    if let Ok(mut rec) = recognizer.lock() {
        if let CompleteResult::Single(result) = rec.final_result() {
            sink.publish_final(result.text);
        }
    }

    Ok(())
}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_resample_preserves_sample_count() {
        let acc = Mutex::new(0.0);
        let input = vec![0.0, 0.5, -0.5, 1.0];
        let out = resample_to_i16(&input, 16000, 16000, &acc);
        assert_eq!(out.len(), input.len());
        assert_eq!(out[0], 0);
        assert_eq!(out[3], i16::MAX);
    }

    #[test]
    fn downsampling_halves_the_sample_count() {
        let acc = Mutex::new(0.0);
        let input: Vec<f32> = (0..32).map(|i| (i as f32 / 32.0) - 0.5).collect();
        let out = resample_to_i16(&input, 32000, 16000, &acc);
        assert_eq!(out.len(), 16);
    }

    #[test]
    fn resampling_carries_its_offset_between_chunks() {
        let whole: Vec<f32> = (0..64).map(|i| (i as f32 * 0.05).sin()).collect();

        let contiguous_acc = Mutex::new(0.0);
        let contiguous = resample_to_i16(&whole, 44100, 16000, &contiguous_acc);

        let streamed_acc = Mutex::new(0.0);
        let mut streamed = resample_to_i16(&whole[..32], 44100, 16000, &streamed_acc);
        let carried = *streamed_acc.lock().expect("accumulator should not be poisoned");
        streamed.extend(resample_to_i16(&whole[32..], 44100, 16000, &streamed_acc));

        assert!(
            carried.fract() > 0.0,
            "a rate that does not divide evenly must leave a fractional offset"
        );
        assert_eq!(
            contiguous.len(),
            streamed.len(),
            "splitting the input across callbacks must not add or drop samples"
        );
    }

    #[test]
    fn resampling_clamps_out_of_range_input() {
        let acc = Mutex::new(0.0);
        let out = resample_to_i16(&[4.0, -4.0], 16000, 16000, &acc);
        assert_eq!(out, vec![i16::MAX, -i16::MAX]);
    }
}
