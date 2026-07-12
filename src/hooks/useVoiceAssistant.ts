import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import Cerebras from '@cerebras/cerebras_cloud_sdk';
// Web search is OPTIONAL context for the model. If the search API is
// unconfigured (no key) or unreachable, we fall back to answering without
// it rather than killing the whole voice loop. Note: this runs in the
// webview, so we read the key from `import.meta.env` (Vite) — `process.env`
// does not exist in the browser.
const client = new Cerebras({
  apiKey: import.meta.env.VITE_CEREBRAS_API_KEY,
  dangerouslyAllowBrowser: true,
});
export type AssistantStatus =
  | "standby"
  | "listening"
  | "thinking"
  | "speaking"
  | "error";

export interface TranscriptEntry {
  id: string;
  role: "user" | "assistant";
  text: string;
  timestamp: number;
}

// Module-level so it survives re-renders without extra state plumbing.
let conversation: Array<{
  role: "system" | "user" | "assistant";
  content: string;
}> = [
    {
      role: "system",
      content:
        "You are a helpful voice assistant. Keep replies short and conversational — they will be read aloud.",
    },
  ];

// Chromium-based webviews (which Tauri uses on Windows/Linux) populate the
// voice list asynchronously — on first call `getVoices()` often returns []
// and the utterance gets silently dropped rather than throwing. This waits
// for the `voiceschanged` event (with a timeout fallback) before speaking.
function getVoicesAsync(): Promise<SpeechSynthesisVoice[]> {
  return new Promise((resolve) => {
    const existing = window.speechSynthesis.getVoices();
    if (existing.length > 0) {
      resolve(existing);
      return;
    }
    const timeout = setTimeout(() => {
      window.speechSynthesis.onvoiceschanged = null;
      resolve(window.speechSynthesis.getVoices());
    }, 1000);
    window.speechSynthesis.onvoiceschanged = () => {
      clearTimeout(timeout);
      window.speechSynthesis.onvoiceschanged = null;
      resolve(window.speechSynthesis.getVoices());
    };
  });
}

async function speak(text: string): Promise<void> {
  if (!window.speechSynthesis) {
    console.warn("Speech synthesis not supported in this environment");
    return;
  }

  // Clear anything stuck in the queue (e.g. from StrictMode double-invokes
  // or overlapping replies) — otherwise speak() below just queues silently
  // behind a dead utterance and nothing audible happens.
  window.speechSynthesis.cancel();

  const voices = await getVoicesAsync();

  return new Promise((resolve) => {
    const utterance = new SpeechSynthesisUtterance(text);
    if (voices.length > 0) {
      const preferred =
        voices.find((v) => v.lang?.startsWith("en") && v.localService) ??
        voices.find((v) => v.lang?.startsWith("en")) ??
        voices[0];
      utterance.voice = preferred;
    }
    utterance.rate = 1;
    utterance.pitch = 1;
    utterance.volume = 1;

    utterance.onstart = () => console.debug("[tts] speaking started");
    utterance.onend = () => {
      console.debug("[tts] speaking finished");
      resolve();
    };
    utterance.onerror = (e) => {
      console.error("Speech synthesis error:", e.error ?? e);
      resolve();
    };

    window.speechSynthesis.speak(utterance);

    // Some webviews (esp. WebKitGTK on Linux) silently no-op speak() without
    // ever firing onstart/onend — bail out after a timeout so the UI isn't
    // stuck in "speaking" forever.
    setTimeout(() => {
      if (!window.speechSynthesis.speaking) {
        console.warn("[tts] speak() produced no audio — resolving anyway");
        resolve();
      }
    }, 500);
  });
}

async function ask(userText: string): Promise<string> {
  // Search is best-effort context; never let it abort the conversation.
  let context = ""
  const userContent = userText;
  conversation.push({ role: "user", content: userContent });
  const res: any = await client.chat.completions.create({
    model: 'gpt-oss-120b',
    messages: conversation
  });
  ;

  // content can be string | Array<...> | null — extract plain text safely
  const rawContent = res.choices[0]?.message?.content;
  let reply: string;
  if (typeof rawContent === "string") {
    reply = rawContent;
  } else if (Array.isArray(rawContent)) {
    reply = rawContent
      .filter((part: { type?: string; text?: string }) => part?.type === "text")
      .map((part: { text?: string }) => part?.text ?? "")
      .join("");
  } else {
    reply = "Sorry, I couldn't generate a response.";
  }

  if (!reply.trim()) {
    reply = "Sorry, I couldn't generate a response.";
  }

  conversation.push({ role: "assistant", content: reply });
  return reply;
}

type Phase = "idle" | "thinking" | "speaking";

export function useVoiceAssistant() {
  const [isListening, setIsListening] = useState(false);
  const [phase, setPhase] = useState<Phase>("idle");
  const [partial, setPartial] = useState("");
  const [transcript, setTranscript] = useState<TranscriptEntry[]>([]);
  const [errorMessage, setErrorMessage] = useState("");
  const [debugLogs, setDebugLogs] = useState<string[]>([]);
  const processingRef = useRef(false);

  const addEntry = useCallback((role: TranscriptEntry["role"], text: string) => {
    setTranscript((prev) => [
      ...prev,
      {
        id: `${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
        role,
        text,
        timestamp: Date.now(),
      },
    ]);
  }, []);

  useEffect(() => {
    let unlistenPartial: UnlistenFn | undefined;
    let unlistenResult: UnlistenFn | undefined;
    let unlistenError: UnlistenFn | undefined;
    let unlistenDebug: UnlistenFn | undefined;
    let cancelled = false;

    (async () => {
      const p = await listen<string>("vosk-speech-partial", (event) => {
        setPartial(event.payload);
      });

      const r = await listen<string>("vosk-speech-result", async (event) => {
        const text = event.payload.trim();
        setPartial("");
        if (!text || processingRef.current) return;

        processingRef.current = true;
        addEntry("user", text);
        setPhase("thinking");
        try {
          const reply = await ask(text);
          addEntry("assistant", reply);
          setPhase("speaking");
          await speak(reply);
        } catch (error) {
          console.error("Failed to get a response:", error);
          setErrorMessage("Could not reach the assistant. Check your connection.");
        } finally {
          processingRef.current = false;
          setPhase("idle");
        }
      });

      const e = await listen<string>("vosk-error", (event) => {
        console.error("Vosk error:", event.payload);
        setErrorMessage(String(event.payload));
        setIsListening(false);
      });

      const d = await listen<string>("theta-debug", (event) => {
        setDebugLogs((prev) => {
          const next = [...prev, event.payload];
          if (next.length > 50) return next.slice(next.length - 50); // Keep last 50 logs
          return next;
        });
      });

      if (cancelled) {
        p();
        r();
        e();
        d();
      } else {
        unlistenPartial = p;
        unlistenResult = r;
        unlistenError = e;
        unlistenDebug = d;
      }
    })();

    return () => {
      cancelled = true;
      unlistenPartial?.();
      unlistenResult?.();
      unlistenError?.();
      unlistenDebug?.();
    };
  }, [addEntry]);

  const toggleListening = useCallback(async () => {
    if (phase !== "idle") return; // ignore taps while thinking/speaking
    setErrorMessage("");
    try {
      if (isListening) {
        await invoke("stop_listening");
        setIsListening(false);
        setPartial("");
      } else {
        await invoke("start_listening");
        setIsListening(true);
      }
    } catch (error) {
      console.error("Failed to toggle listening:", error);
      setIsListening(false);
      setErrorMessage(String(error));
    }
  }, [isListening, phase]);

  const status: AssistantStatus = errorMessage
    ? "error"
    : phase !== "idle"
      ? phase
      : isListening
        ? "listening"
        : "standby";

  return { status, partial, transcript, errorMessage, toggleListening, debugLogs };
}