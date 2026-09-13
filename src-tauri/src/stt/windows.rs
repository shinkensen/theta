//! Speech-to-text on top of the Windows dictation service.
//!
//! Uses `Windows.Media.SpeechRecognition` continuous recognition, which owns
//! microphone capture itself. That keeps the engine entirely inside the Rust
//! process — unlike the webview's Web Speech API, recognition here keeps
//! running while the window is hidden, so the tray and global hotkey behave
//! the same as they do under Vosk.
//!
//! WinRT raises its recognition events on the thread that created the
//! recognizer, and only if that thread is apartment-threaded and keeps
//! pumping window messages — so a session here runs on its own STA message
//! loop. A plain worker thread that sleeps between polls never sees a single
//! event, no matter how healthy the microphone is.
//!
//! The trade-off is that the OS gives us no access to the raw waveform, so
//! this backend reports a flat level and the UI animates from listening state
//! alone.

use super::SessionSink;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::windows::core::{Error as WinError, HSTRING};
use ::windows::Foundation::TypedEventHandler;
use ::windows::Media::SpeechRecognition::{
    SpeechContinuousRecognitionCompletedEventArgs,
    SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionConfidence, SpeechRecognitionHypothesisGeneratedEventArgs,
    SpeechRecognitionResultStatus, SpeechRecognitionScenario, SpeechRecognitionTopicConstraint,
    SpeechRecognizer,
};
use ::windows::Win32::System::WinRT::{
    RoInitialize, RoUninitialize, RO_INIT_SINGLETHREADED,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

const POLL: Duration = Duration::from_millis(50);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(1500);

/// How often the run loop may restart a session the OS ended for silence
/// before it gives up. Speech activity resets the count, so this only trips
/// when recognition keeps timing out without ever hearing anything.
const MAX_RESTARTS: u32 = 100;

const ACCESS_DENIED: i32 = -2147024891;
const CLASS_NOT_REGISTERED: i32 = -2147221164;
const SPERR_NOT_FOUND: i32 = -2147201015; // 0x8004503a - Speech privacy policy not accepted

pub fn run(sink: &Arc<SessionSink>) -> Result<(), String> {
    let initialized = unsafe { RoInitialize(RO_INIT_SINGLETHREADED) }.is_ok();
    let outcome = recognize(sink);
    if initialized {
        unsafe { RoUninitialize() };
    }
    outcome
}

fn recognize(sink: &Arc<SessionSink>) -> Result<(), String> {
    sink.log("Initializing Windows Speech Recognition...");
    let recognizer =
        SpeechRecognizer::new().map_err(|e| failure("open Windows Speech Recognition", &e))?;
    
    sink.log("Windows Speech Recognition will use the system default microphone.");
    sink.log("To change it: Windows Settings > System > Sound > Input > Choose your input device");
    
    let outcome = dictate(sink, &recognizer);
    let _ = recognizer.Close();
    outcome
}

fn dictate(sink: &Arc<SessionSink>, recognizer: &SpeechRecognizer) -> Result<(), String> {
    // Log recognizer state for debugging
    sink.log("Setting up Windows Speech Recognition constraints...");
    
    let constraint = SpeechRecognitionTopicConstraint::Create(
        SpeechRecognitionScenario::Dictation,
        &HSTRING::from("theta"),
    )
    .map_err(|e| failure("prepare Windows dictation", &e))?;

    recognizer
        .Constraints()
        .and_then(|constraints| constraints.Append(&constraint))
        .map_err(|e| failure("prepare Windows dictation", &e))?;

    sink.log("Compiling speech recognition constraints...");
    let compiled = recognizer
        .CompileConstraintsAsync()
        .and_then(|operation| operation.get())
        .map_err(|e| failure("prepare Windows dictation", &e))?;
    let compile_status = compiled
        .Status()
        .map_err(|e| failure("prepare Windows dictation", &e))?;
    
    sink.log(format!("Constraint compilation status: {}", compile_status.0));
    
    if compile_status != SpeechRecognitionResultStatus::Success {
        return Err(status_message(compile_status));
    }

    let session = recognizer
        .ContinuousRecognitionSession()
        .map_err(|e| failure("start a Windows dictation session", &e))?;

    // Set timeouts - increase initial silence timeout for microphone detection
    if let Ok(timeouts) = recognizer.Timeouts() {
        use ::windows::Foundation::TimeSpan;
        let initial_silence = TimeSpan {
            Duration: 10_000_000 * 10, // 10 seconds in 100-nanosecond units
        };
        let end_silence = TimeSpan {
            Duration: 10_000_000 * 1, // 1 second in 100-nanosecond units
        };
        let _ = timeouts.SetInitialSilenceTimeout(initial_silence);
        let _ = timeouts.SetEndSilenceTimeout(end_silence);
        sink.log("Set speech recognition timeouts");
    }
    
    // Check if recognizer has audio input
    sink.log("Checking audio input configuration...");

    let finished = Arc::new(AtomicBool::new(false));
    let final_status = Arc::new(AtomicI32::new(SpeechRecognitionResultStatus::Success.0));

    let hypothesis_sink = Arc::clone(sink);
    let hypothesis_token = recognizer
        .HypothesisGenerated(&TypedEventHandler::<
            SpeechRecognizer,
            SpeechRecognitionHypothesisGeneratedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                if let Ok(text) = args.Hypothesis().and_then(|h| h.Text()) {
                    let text_str = text.to_string();
                    hypothesis_sink.log(format!("Hypothesis: {}", &text_str));
                    hypothesis_sink.partial(&text_str);
                }
            }
            Ok(())
        }))
        .map_err(|e| failure("follow Windows dictation", &e))?;

    let result_sink = Arc::clone(sink);
    let result_token = session
        .ResultGenerated(&TypedEventHandler::<
            SpeechContinuousRecognitionSession,
            SpeechContinuousRecognitionResultGeneratedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                if let Ok(result) = args.Result() {
                    let status = result.Status().unwrap_or(SpeechRecognitionResultStatus::Unknown);
                    let confidence = result.Confidence().unwrap_or(SpeechRecognitionConfidence::Rejected);
                    let text = result.Text().unwrap_or_default().to_string();
                    
                    result_sink.log(format!("Result received - Status: {}, Confidence: {}, Text: '{}'", 
                        status.0, confidence.0, text));
                    
                    let accepted = status == SpeechRecognitionResultStatus::Success
                        && confidence != SpeechRecognitionConfidence::Rejected;
                    
                    if accepted && !text.is_empty() {
                        result_sink.publish_final(&text);
                    } else if !accepted {
                        result_sink.log(format!("Result rejected - Status: {}, Confidence: {}", status.0, confidence.0));
                    }
                }
            }
            Ok(())
        }))
        .map_err(|e| failure("follow Windows dictation", &e))?;

    let completed_flag = Arc::clone(&finished);
    let completed_status = Arc::clone(&final_status);
    let completed_token = session
        .Completed(&TypedEventHandler::<
            SpeechContinuousRecognitionSession,
            SpeechContinuousRecognitionCompletedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                if let Ok(status) = args.Status() {
                    completed_status.store(status.0, Ordering::SeqCst);
                }
            }
            completed_flag.store(true, Ordering::SeqCst);
            Ok(())
        }))
        .map_err(|e| failure("follow Windows dictation", &e))?;

    let started = session
        .StartAsync()
        .and_then(|operation| operation.get())
        .map_err(|e| failure("start Windows dictation", &e));

    if started.is_ok() {
        sink.log("Windows Speech Recognition started");
        sink.log("Listening for audio input... Speak now to test microphone.");

        let mut no_audio_warned = false;
        let mut restarts: u32 = 0;
        let start_time = Instant::now();

        while !sink.is_cancelled() && !finished.load(Ordering::SeqCst) {
            // Events arrive as window messages on this thread; without the
            // pump below, not one hypothesis or result is ever delivered.
            pump_messages();
            std::thread::sleep(POLL);

            // The OS ends the session itself on silence/pause timeouts.
            // That's normal for continuous dictation — start it again so one
            // long pause doesn't silently kill listening until the next toggle.
            if finished.load(Ordering::SeqCst) {
                let status = SpeechRecognitionResultStatus(final_status.load(Ordering::SeqCst));
                if !restartable(status) || restarts >= MAX_RESTARTS {
                    break;
                }
                restarts += 1;
                finished.store(false, Ordering::SeqCst);
                sink.log(format!(
                    "session ended on its own (status {}); restarting ({restarts}/{MAX_RESTARTS})",
                    status.0
                ));
                // A completed session refuses StartAsync until it has been
                // stopped, so transition it back first.
                let _ = session.StopAsync().and_then(|operation| operation.get());
                if let Err(e) = session.StartAsync().and_then(|operation| operation.get()) {
                    sink.log(format!("couldn't restart dictation: {}", e.message()));
                    break;
                }

                // Warn if nothing was heard after the first few rounds
                if !no_audio_warned && start_time.elapsed() > Duration::from_secs(5) {
                    sink.log("No audio detected yet. Check that:");
                    sink.log("1. Your microphone is the Windows default recording device");
                    sink.log("2. The microphone is not muted");
                    sink.log("3. Microphone permissions are enabled in Windows Settings > Privacy & security > Microphone");
                    no_audio_warned = true;
                }
            }
        }

        if !finished.load(Ordering::SeqCst) {
            // A user-initiated stop should still surrender the last utterance,
            // which `StopAsync` waits for. A session that has been superseded
            // has no listener left, so it drops the audio instead.
            let _ = if sink.is_current() {
                session.StopAsync().and_then(|operation| operation.get())
            } else {
                session.CancelAsync().and_then(|operation| operation.get())
            };
            // Keep pumping while the stop settles, or its completion event
            // can never be delivered back to this thread.
            let deadline = Instant::now() + SHUTDOWN_GRACE;
            while !finished.load(Ordering::SeqCst) && Instant::now() < deadline {
                pump_messages();
                std::thread::sleep(POLL);
            }
        }
    }

    let _ = recognizer.RemoveHypothesisGenerated(hypothesis_token);
    let _ = session.RemoveResultGenerated(result_token);
    let _ = session.RemoveCompleted(completed_token);

    started?;

    let status = SpeechRecognitionResultStatus(final_status.load(Ordering::SeqCst));
    if ended_normally(status) {
        sink.log(format!("Windows dictation ended (status {})", status.0));
        Ok(())
    } else {
        Err(status_message(status))
    }
}

/// Drains pending window messages so WinRT can deliver event callbacks on
/// this thread. Recognizer events arrive through this queue, not through
/// the poll loop.
fn pump_messages() {
    let mut msg = MSG::default();
    unsafe {
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// Session endings worth retrying: the OS gave up on silence or a pause
/// limit, but nothing is wrong with the microphone or the language pack.
fn restartable(status: SpeechRecognitionResultStatus) -> bool {
    status == SpeechRecognitionResultStatus::TimeoutExceeded
        || status == SpeechRecognitionResultStatus::PauseLimitExceeded
        || status == SpeechRecognitionResultStatus::UserCanceled
}

fn ended_normally(status: SpeechRecognitionResultStatus) -> bool {
    status == SpeechRecognitionResultStatus::Success
        || status == SpeechRecognitionResultStatus::UserCanceled
        || status == SpeechRecognitionResultStatus::TimeoutExceeded
}

fn status_message(status: SpeechRecognitionResultStatus) -> String {
    let detail = if status == SpeechRecognitionResultStatus::TopicLanguageNotSupported {
        "Windows Speech Recognition doesn't support your speech language. Add a supported one in \
         Windows Settings > Time & language > Speech."
    } else if status == SpeechRecognitionResultStatus::GrammarLanguageMismatch {
        "Your Windows speech language doesn't match the installed recognizer. Check Windows \
         Settings > Time & language > Speech."
    } else if status == SpeechRecognitionResultStatus::GrammarCompilationFailure {
        "Windows couldn't prepare dictation. Check that a speech language is installed in Windows \
         Settings > Time & language > Speech."
    } else if status == SpeechRecognitionResultStatus::AudioQualityFailure {
        "Windows couldn't get usable audio from the microphone. Check the input level and try \
         again."
    } else if status == SpeechRecognitionResultStatus::PauseLimitExceeded {
        "Windows Speech Recognition paused for too long and stopped listening."
    } else if status == SpeechRecognitionResultStatus::NetworkFailure {
        "Windows couldn't reach its online speech service. Check your connection, or switch to \
         Vosk in Settings to stay offline."
    } else if status == SpeechRecognitionResultStatus::MicrophoneUnavailable {
        "Windows can't use the microphone. Check that one is connected and that microphone access \
         is allowed in Windows Settings > Privacy & security > Microphone."
    } else {
        "Windows Speech Recognition stopped unexpectedly."
    };
    detail.to_string()
}

fn failure(action: &str, error: &WinError) -> String {
    match error.code().0 {
        ACCESS_DENIED => "Windows blocked Theta from using speech recognition. Allow microphone \
                          and online speech access in Windows Settings > Privacy & security, then \
                          try again."
            .to_string(),
        CLASS_NOT_REGISTERED => "Windows Speech Recognition isn't available on this system. \
                                 Install a speech language in Windows Settings > Time & language \
                                 > Speech, or switch to Vosk in Settings."
            .to_string(),
        SPERR_NOT_FOUND => "Windows Speech Recognition needs to be set up first. \
                            Open Windows Settings > Privacy & security > Speech, \
                            turn on Online speech recognition, and accept the privacy policy. \
                            Then try again, or switch to Vosk in Settings for offline recognition."
            .to_string(),
        _ => format!("Couldn't {action}: {}", error.message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_timeouts_are_worth_restarting() {
        assert!(restartable(SpeechRecognitionResultStatus::TimeoutExceeded));
        assert!(restartable(SpeechRecognitionResultStatus::PauseLimitExceeded));
        assert!(restartable(SpeechRecognitionResultStatus::UserCanceled));
    }

    #[test]
    fn real_failures_are_not_restarted() {
        assert!(!restartable(SpeechRecognitionResultStatus::MicrophoneUnavailable));
        assert!(!restartable(SpeechRecognitionResultStatus::NetworkFailure));
        assert!(!restartable(
            SpeechRecognitionResultStatus::TopicLanguageNotSupported
        ));
    }

    #[test]
    fn normal_endings_are_not_reported_as_errors() {
        assert!(ended_normally(SpeechRecognitionResultStatus::Success));
        assert!(ended_normally(SpeechRecognitionResultStatus::UserCanceled));
        assert!(ended_normally(SpeechRecognitionResultStatus::TimeoutExceeded));
    }

    #[test]
    fn real_failures_are_reported() {
        assert!(!ended_normally(
            SpeechRecognitionResultStatus::MicrophoneUnavailable
        ));
        assert!(!ended_normally(
            SpeechRecognitionResultStatus::NetworkFailure
        ));
        assert!(!ended_normally(
            SpeechRecognitionResultStatus::TopicLanguageNotSupported
        ));
    }

    #[test]
    fn status_messages_are_actionable() {
        let microphone = status_message(SpeechRecognitionResultStatus::MicrophoneUnavailable);
        assert!(microphone.contains("Privacy & security"));

        let language = status_message(SpeechRecognitionResultStatus::TopicLanguageNotSupported);
        assert!(language.contains("Time & language"));

        let network = status_message(SpeechRecognitionResultStatus::NetworkFailure);
        assert!(network.contains("Vosk"));
    }

    #[test]
    fn unknown_status_still_produces_a_message() {
        let message = status_message(SpeechRecognitionResultStatus(4242));
        assert!(!message.is_empty());
    }

    #[test]
    fn access_denied_points_at_windows_privacy_settings() {
        let error = WinError::from_hresult(::windows::core::HRESULT(ACCESS_DENIED));
        let message = failure("start dictation", &error);
        assert!(message.contains("Privacy & security"));
    }

    #[test]
    fn other_errors_name_the_action() {
        let error = WinError::from_hresult(::windows::core::HRESULT(-2147024809));
        let message = failure("start dictation", &error);
        assert!(message.contains("start dictation"));
    }
}
