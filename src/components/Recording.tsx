import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  startRecording,
  stopRecording,
  type RecordingResult,
} from "tauri-plugin-audio-recorder-api";
import { OpenRouter } from "@openrouter/sdk";
import type {
  ChatFunctionTool,
  ChatMessages,
  ChatToolCall,
} from "@openrouter/sdk/models";
import * as SpeechSDK from "microsoft-cognitiveservices-speech-sdk";

// Keys come from .env (gitignored). See .env.example.
const HC_API_KEY = import.meta.env.VITE_HC_API_KEY as string | undefined;
const AZURE_SPEECH_KEY = import.meta.env.VITE_AZURE_SPEECH_KEY as
  | string
  | undefined;
const AZURE_SPEECH_REGION =
  (import.meta.env.VITE_AZURE_SPEECH_REGION as string | undefined) ?? "eastus";
const TRANSCRIBE_URL =
  (import.meta.env.VITE_TRANSCRIBE_URL as string | undefined) ??
  "http://68.183.25.147:8000/transcribe";

const MODEL = "openai/gpt-oss-120b:free";
const MAX_TOOL_ITERATIONS = 6; // hard cap so a confused model can't loop forever
const MAX_HISTORY_MESSAGES = 24; // keep the request small & cheap
const RECORD_SAFETY_LIMIT_MS = 60_000; // hard stop even if the user forgets

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

// ---------------------------------------------------------------------------
// Agent tools — every tool here maps 1:1 to a #[tauri::command] in lib.rs.
// Adding a new capability for the assistant means: write the Rust command,
// register it in the invoke_handler, then describe it here.
// ---------------------------------------------------------------------------

const TOOLS: ChatFunctionTool[] = [
  {
    type: "function",
    function: {
      name: "list_directory",
      description:
        "List the files and folders inside a directory on the user's computer.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Absolute or relative directory path to list.",
          },
        },
        required: ["path"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "read_file_content",
      description: "Read the full text contents of a file on the user's computer.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Absolute or relative path to the file to read.",
          },
        },
        required: ["path"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "write_file_content",
      description:
        "Create or overwrite a text file on the user's computer with the given content.",
      parameters: {
        type: "object",
        properties: {
          path: {
            type: "string",
            description: "Absolute or relative path of the file to write.",
          },
          content: {
            type: "string",
            description: "The full text content to write to the file.",
          },
        },
        required: ["path", "content"],
      },
    },
  },
  {
    type: "function",
    function: {
      name: "get_system_info",
      description:
        "Get basic information about the user's computer: OS, architecture, hostname, current unix time.",
      parameters: { type: "object", properties: {} },
    },
  },
];

async function executeTool(call: ChatToolCall): Promise<string> {
  let args: Record<string, unknown> = {};
  try {
    args = call.function.arguments ? JSON.parse(call.function.arguments) : {};
  } catch {
    return `Error: could not parse arguments for "${call.function.name}"`;
  }

  try {
    switch (call.function.name) {
      case "list_directory": {
        const entries = await invoke<string[]>("list_directory", {
          path: String(args.path ?? "."),
        });
        return JSON.stringify(entries);
      }
      case "read_file_content": {
        return await invoke<string>("read_file_content", {
          path: String(args.path ?? ""),
        });
      }
      case "write_file_content": {
        return await invoke<string>("write_file_content", {
          path: String(args.path ?? ""),
          content: String(args.content ?? ""),
        });
      }
      case "get_system_info": {
        const info = await invoke("get_system_info");
        return JSON.stringify(info);
      }
      default:
        return `Error: unknown tool "${call.function.name}"`;
    }
  } catch (error) {
    return `Error: ${error instanceof Error ? error.message : String(error)}`;
  }
}

// ---------------------------------------------------------------------------
// Conversation state + agent loop
// ---------------------------------------------------------------------------

const SYSTEM_PROMPT: ChatMessages = {
  role: "system",
  content:
    "You are Theta, a voice-controlled desktop agent running locally on the " +
    "user's Windows machine. You can inspect the system and read/write text " +
    "files or list directories via tools when a request needs it. Only call " +
    "a tool when it's actually necessary to complete the request; otherwise " +
    "just answer. Your replies are read aloud by text-to-speech, so keep " +
    "them short and conversational \u2014 a sentence or two unless the user " +
    "asked for something longer.",
};

let conversation: ChatMessages[] = [SYSTEM_PROMPT];

function resetConversation() {
  conversation = [SYSTEM_PROMPT];
}

/** Keep the request small: system message + last MAX_HISTORY_MESSAGES. */
function trimConversation() {
  const overflow = conversation.length - (MAX_HISTORY_MESSAGES + 1);
  if (overflow > 0) {
    conversation.splice(1, overflow);
  }
}

type AssistantMessage = { content?: string | null; toolCalls?: ChatToolCall[] };

async function ask(
  userText: string,
  onStatus: (s: string) => void,
): Promise<string> {
  const client = createClient();
  conversation.push({ role: "user", content: userText });
  trimConversation();

  for (let i = 0; i < MAX_TOOL_ITERATIONS; i++) {
    const res = await client.chat.send({
      chatRequest: {
        model: MODEL,
        messages: conversation as never,
        tools: TOOLS,
      },
    });

    const message = res.choices?.[0]?.message as unknown as
      | AssistantMessage
      | undefined;
    const toolCalls = message?.toolCalls ?? [];

    if (toolCalls.length > 0) {
      conversation.push({
        role: "assistant",
        content: message?.content ?? "",
        toolCalls,
      } as never);

      onStatus(
        `Using ${toolCalls.length} tool${toolCalls.length > 1 ? "s" : ""}\u2026`,
      );
      for (const call of toolCalls) {
        onStatus(`\u2192 ${call.function.name}(${call.function.arguments})`);
        const result = await executeTool(call);
        conversation.push({
          role: "tool",
          toolCallId: call.id,
          content: result,
        } as never);
      }
      trimConversation();
      continue; // let the model see the tool results and respond/continue
    }

    const raw = message?.content;
    const reply = typeof raw === "string" ? raw : "";
    conversation.push({ role: "assistant", content: reply });
    return reply;
  }

  return "I got stuck using tools and couldn't finish that one \u2014 try rephrasing.";
}

/** Read a recorded file via the Tauri backend (no node:fs in the webview). */
async function readFileAsBlob(path: string): Promise<Blob> {
  const bytes = await invoke<number[]>("read_file_bytes", { path });
  const u8 = new Uint8Array(bytes);
  // WAV is RIFF; browsers infer audio/wav but we set it explicitly.
  return new Blob([u8], { type: "audio/wav" });
}

async function getAudioTranscription(filepath: string): Promise<string> {
  const fileBlob = await readFileAsBlob(filepath);
  const formData = new FormData();
  const basename = filepath.split(/[\\/]/).pop() ?? "audio.wav";
  formData.append("file", fileBlob, basename);

  // The transcription box is a remote VPS; don't hang forever if it's down.
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 20_000);
  try {
    const res = await fetch(TRANSCRIBE_URL, {
      method: "POST",
      body: formData,
      signal: controller.signal,
    });
    if (!res.ok) {
      const errorData = await res.json().catch(() => ({}));
      throw new Error(
        (errorData as { error?: string }).error ||
          `Transcription server error ${res.status}: ${res.statusText}`,
      );
    }
    const data = (await res.json()) as { text?: string };
    return data.text ?? "";
  } catch (error) {
    if (error instanceof DOMException && error.name === "AbortError") {
      throw new Error("Transcription server timed out after 20s.");
    }
    throw error;
  } finally {
    clearTimeout(timeout);
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
          resolve();
        } else {
          reject(new Error(result.errorDetails ?? "synthesis canceled"));
        }
      },
      (err: unknown) => {
        synthesizer.close();
        reject(
          err instanceof Error
            ? err
            : new Error(typeof err === "string" ? err : String(err)),
        );
      },
    );
  });
}

type PipelineHooks = {
  setTranscript: (s: string) => void;
  setReply: (s: string) => void;
  addStatus: (s: string) => void;
  setError: (s: string) => void;
};

async function runPipeline(filepath: string, hooks: PipelineHooks): Promise<void> {
  hooks.setError("");
  hooks.setReply("");
  hooks.addStatus("Transcribing\u2026");

  const text = await getAudioTranscription(filepath);
  if (!text.trim()) {
    hooks.setError(
      "Got an empty transcription \u2014 the recording may have been silent, " +
        "or the transcription server didn't understand it.",
    );
    return;
  }
  hooks.setTranscript(text);

  hooks.addStatus("Thinking\u2026");
  const reply = await ask(text, hooks.addStatus);

  if (reply) {
    hooks.setReply(reply);
    hooks.addStatus("Speaking\u2026");
    await speakText(reply);
  }
  hooks.addStatus("Done.");
}

export default function Recording() {
  const [recording, setRecording] = useState(false);
  const [processing, setProcessing] = useState(false);
  const [transcript, setTranscript] = useState("");
  const [reply, setReply] = useState("");
  const [statusLog, setStatusLog] = useState<string[]>([]);
  const [error, setError] = useState("");

  // Refs so async chains see the latest values without stale closures.
  const recordingRef = useRef(false);
  const stopTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  function addStatus(s: string) {
    setStatusLog((prev) => [...prev.slice(-6), s]);
  }

  async function startAndScheduleStop() {
    setError("");
    setStatusLog([]);
    setRecording(true);
    recordingRef.current = true;

    // Build a filesystem-safe filename; the recorder writes to outputPath.
    const ts = new Date().toISOString().replace(/[:.]/g, "-");
    const outputPath = `theta_recordings/voice_sample_${ts}.wav`;

    try {
      await startRecording({
        outputPath,
        format: "wav",
        quality: "high",
        maxDuration: 0,
      });
    } catch (e) {
      recordingRef.current = false;
      setRecording(false);
      setError(
        `Couldn't start recording: ${e instanceof Error ? e.message : String(e)}`,
      );
      return;
    }

    // Safety net only \u2014 the primary way to stop is clicking again.
    stopTimerRef.current = setTimeout(async () => {
      if (!recordingRef.current) return;
      await doStop();
    }, RECORD_SAFETY_LIMIT_MS);
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
    } catch (e) {
      setError(
        `Failed to stop recording: ${e instanceof Error ? e.message : String(e)}`,
      );
      return;
    }

    if (result?.filePath) {
      setProcessing(true);
      try {
        await runPipeline(result.filePath, {
          setTranscript,
          setReply,
          addStatus,
          setError,
        });
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        setProcessing(false);
      }
    }
  }

  async function handleClick() {
    if (processing) return; // ignore clicks while transcribing/thinking/speaking
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
    <div style={{ display: "flex", flexDirection: "column", gap: 12, maxWidth: 640 }}>
      <div style={{ display: "flex", gap: 16, alignItems: "center" }}>
        <div
          onClick={handleClick}
          title={
            recording
              ? "Click to stop"
              : processing
                ? "Processing\u2026"
                : "Click to talk to Theta"
          }
          style={{
            width: 50,
            height: 50,
            flexShrink: 0,
            border: "1px solid #333",
            background: bg,
            borderRadius: "50%",
            cursor: processing ? "wait" : "pointer",
            opacity: processing ? 0.7 : 1,
          }}
        />
        <span className="result-msg">
          {recording
            ? "Listening\u2026 click again to stop."
            : processing
              ? "Working on it\u2026"
              : "Click once to start · click again to stop."}
        </span>
        <button
          type="button"
          onClick={() => {
            resetConversation();
            setTranscript("");
            setReply("");
            setStatusLog([]);
            setError("");
          }}
          disabled={recording || processing}
          style={{ marginLeft: "auto" }}
        >
          New conversation
        </button>
      </div>

      {transcript && (
        <div className="card">
          <strong>You said:</strong> {transcript}
        </div>
      )}

      {statusLog.length > 0 && (
        <div className="card" style={{ fontFamily: "monospace", fontSize: 13 }}>
          {statusLog.map((s, i) => (
            <div key={i}>{s}</div>
          ))}
        </div>
      )}

      {reply && (
        <div className="card">
          <strong>Theta:</strong> {reply}
        </div>
      )}

      {error && <p className="error-msg">{error}</p>}
    </div>
  );
}
