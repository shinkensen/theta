import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  startRecording,
  stopRecording,
  type RecordingResult,
} from "tauri-plugin-audio-recorder-api";
import { OpenRouter } from "@openrouter/sdk";
import * as SpeechSDK from "microsoft-cognitiveservices-speech-sdk";

// Keys come from .env (gitignored). See .env.example.
const HC_API_KEY = import.meta.env.VITE_HC_API_KEY as string | undefined;
const AZURE_SPEECH_KEY = import.meta.env.VITE_AZURE_SPEECH_KEY as
  | string
  | undefined;
const AZURE_SPEECH_REGION =
  (import.meta.env.VITE_AZURE_SPEECH_REGION as string | undefined) ?? "eastus";

const TRANSCRIBE_URL = "http://68.183.25.147:8000/transcribe";

// Lazily create the OpenRouter client so the module doesn't crash if the key
// is missing during build/preview.
function createClient(): OpenRouter {
  if (!HC_API_KEY) {
    throw new Error(
      "VITE_HC_API_KEY is not set. Copy .env.example to .env and fill it in.",
    );
  }
  return new OpenRouter({
    apiKey: HC_API_KEY,
    serverURL: "https://ai.hackclub.com/proxy/v1",
  });
}

type ChatMessage = { role: "system" | "user" | "assistant"; content: string };
const conversation: ChatMessage[] = [
  { role: "system", content: "You are a helpful assistant." },
];

async function ask(userText: string): Promise<string> {
  const client = createClient();
  conversation.push({ role: "user", content: userText });
  const res = await client.chat.send({
    chatRequest: {
      model: "openai/gpt-oss-120b:free",
      // The SDK's message type is stricter than our loose history; cast.
      messages: conversation as never,
    },
  });
  const raw = res.choices?.[0]?.message?.content;
  const reply =
    typeof raw === "string"
      ? raw
      : Array.isArray(raw)
        ? raw
            .map((p: { text?: string }) => p?.text ?? "")
            .join("")
        : "";
  conversation.push({ role: "assistant", content: reply });
  return reply;
}

/** Read a recorded file via the Tauri backend (no node:fs in the webview). */
async function readFileAsBlob(path: string): Promise<Blob> {
  const bytes = await invoke<number[]>("read_file_bytes", { path });
  const u8 = new Uint8Array(bytes);
  // WAV is RIFF; browsers infer audio/wav but we set it explicitly.
  return new Blob([u8], { type: "audio/wav" });
}

async function getAudioTranscription(filepath: string): Promise<string> {
  try {
    const fileBlob = await readFileAsBlob(filepath);
    const formData = new FormData();
    const basename = filepath.split(/[\\/]/).pop() ?? "audio.wav";
    formData.append("file", fileBlob, basename);

    console.log("Sending file for transcription:", filepath);
    const res = await fetch(TRANSCRIBE_URL, {
      method: "POST",
      body: formData,
    });
    if (!res.ok) {
      const errorData = await res.json().catch(() => ({}));
      throw new Error(
        (errorData as { error?: string }).error ||
          `Error Code: ${res.status} Message: ${res.statusText}`,
      );
    }
    const data = (await res.json()) as { text?: string };
    return data.text ?? "";
  } catch (error) {
    console.error("Transcription failed:", error);
    return "";
  }
}

function speakText(text: string): Promise<void> {
  return new Promise((resolve, reject) => {
    if (!AZURE_SPEECH_KEY) {
      console.warn("VITE_AZURE_SPEECH_KEY not set; skipping TTS.");
      resolve();
      return;
    }
    const speechConfig = SpeechSDK.SpeechConfig.fromSubscription(
      AZURE_SPEECH_KEY,
      AZURE_SPEECH_REGION,
    );
    speechConfig.speechSynthesisVoiceName = "en-US-AvaNeural";
    const audioConfig = SpeechSDK.AudioConfig.fromDefaultSpeakerOutput();
    const synthesizer = new SpeechSDK.SpeechSynthesizer(
      speechConfig,
      audioConfig,
    );
    synthesizer.speakTextAsync(
      text,
      (result) => {
        synthesizer.close();
        if (
          result.reason === SpeechSDK.ResultReason.SynthesizingAudioCompleted
        ) {
          console.log("Speech synthesis finished.");
          resolve();
        } else {
          console.error("Speech synthesis canceled:", result.errorDetails);
          reject(new Error(result.errorDetails ?? "synthesis canceled"));
        }
      },
      (err: unknown) => {
        synthesizer.close();
        console.error("TTS error:", err);
        reject(
          err instanceof Error
            ? err
            : new Error(typeof err === "string" ? err : String(err)),
        );
      },
    );
  });
}

async function runPipeline(filepath: string): Promise<void> {
  const text = await getAudioTranscription(filepath);
  if (!text) return;
  const reply = await ask(text);
  if (reply) await speakText(reply);
}

export default function Recording() {
  const [recording, setRecording] = useState(false);
  const [processing, setProcessing] = useState(false);

  // Refs so async chains see the latest values without stale closures.
  const recordingRef = useRef(false);
  const stopTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  async function startAndScheduleStop() {
    setRecording(true);
    recordingRef.current = true;

    // Build a filesystem-safe filename; the recorder writes to outputPath.
    const ts = new Date().toISOString().replace(/[:.]/g, "-");
    const outputPath = `theta_recordings/voice_sample_${ts}.wav`;

    await startRecording({
      outputPath,
      format: "wav",
      quality: "high",
      maxDuration: 0,
    });
    console.log("Recording started ->", outputPath);

    // Auto-stop after 5s (uses the ref so it reflects the live state).
    stopTimerRef.current = setTimeout(async () => {
      if (!recordingRef.current) return;
      await doStop();
    }, 5000);
  }

  async function doStop(): Promise<void> {
    if (stopTimerRef.current) {
      clearTimeout(stopTimerRef.current);
      stopTimerRef.current = null;
    }
    if (!recordingRef.current) return;
    recordingRef.current = false;
    setRecording(false);

    let result: RecordingResult | null = null;
    try {
      result = await stopRecording();
      console.log("Recording saved:", result.filePath);
    } catch (error) {
      console.error("Failed to stop recording:", error);
      return;
    }

    if (result?.filePath) {
      setProcessing(true);
      try {
        await runPipeline(result.filePath);
      } catch (error) {
        console.error("Pipeline failed:", error);
      } finally {
        setProcessing(false);
      }
    }
  }

  async function handleClick() {
    if (processing) return; // ignore clicks while transcribing/speaking
    if (recordingRef.current) {
      await doStop();
    } else {
      await startAndScheduleStop();
    }
  }

  // Cleanup on unmount.
  useEffect(() => {
    return () => {
      if (stopTimerRef.current) clearTimeout(stopTimerRef.current);
      if (recordingRef.current) {
        stopRecording().catch((e) =>
          console.error("cleanup stop failed", e),
        );
      }
    };
  }, []);

  const bg = recording ? "#e53935" : processing ? "#ffb300" : "#ffffff";

  return (
    <div
      onClick={handleClick}
      title={recording ? "Stop" : processing ? "Processing…" : "Record"}
      style={{
        width: 50,
        height: 50,
        border: "1px solid #333",
        background: bg,
        borderRadius: "50%",
        cursor: processing ? "wait" : "pointer",
        opacity: processing ? 0.7 : 1,
      }}
    />
  );
}