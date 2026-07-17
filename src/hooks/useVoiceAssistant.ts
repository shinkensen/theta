import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import Cerebras from '@cerebras/cerebras_cloud_sdk';

// Web search is OPTIONAL context for the model. If the search API is
// unconfigured (no key) or unreachable, we fall back to answering without
// it rather than killing the whole voice loop. Note: this runs in the
// webview, so we read the key from `import.meta.env` (Vite) — `process.env`
// does not exist in the browser.
//
// We call Firecrawl's REST API directly with fetch rather than using the
// `firecrawl` npm package: that SDK is written for Node and imports Node's
// `events` module (EventEmitter) internally. Vite can't polyfill that for a
// browser/webview build — it externalizes the module to a stub, and the SDK
// crashes at import time trying to `extends EventEmitter` against it
// ("Class extends value undefined is not a constructor or null"). Plain
// fetch avoids the problem entirely since the REST API is just JSON over
// HTTPS.
const FIRECRAWL_API_KEY = import.meta.env.VITE_FIRECRAWL_KEY;

async function searchTheWeb(query: string, limit = 1) {
  try {
    const res = await fetch("https://api.firecrawl.dev/v1/search", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${FIRECRAWL_API_KEY}`,
      },
      body: JSON.stringify({
        query,
        limit,
        scrapeOptions: {
          formats: ["markdown"],
        },
      }),
    });

    if (!res.ok) {
      throw new Error(`Firecrawl search returned ${res.status}`);
    }

    const json = await res.json();
    if (!json.success) {
      throw new Error(json.error ?? "Firecrawl search failed");
    }
    return json.data;
  } catch (error) {
    console.error("Search failed:", error);
    return `Error during search: ${error}`;
  }
}

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

// ---------------------------------------------------------------------------
// Edge TTS — free, no API key, uses the same neural voices as Microsoft
// Edge's "Read Aloud" feature (Azure Cognitive Services voices under the
// hood). Noticeably more natural than StreamElements' Polly voices and the
// built-in Web Speech voices. This talks directly to Microsoft's
// synthesize-readaloud websocket endpoint from the browser — no server
// component needed.
//
// Caveat: this is a reverse-engineered, undocumented endpoint (the same one
// the `edge-tts` Python/Node libraries use). Microsoft can change auth
// requirements or rate-limit it without notice, which is why we still keep
// the Web Speech fallback below as a last resort.
//
// Pick any voice from `edge-tts --list-voices` (or the list at
// https://github.com/rany2/edge-tts). A few good ones:
//   en-GB-RyanNeural   — warm British male (used below)
//   en-US-AndrewNeural — warm US male
//   en-US-AvaNeural    — natural US female
//   en-GB-SoniaNeural  — natural British female
const EDGE_TTS_VOICE = "en-GB-RyanNeural";

const EDGE_TRUSTED_CLIENT_TOKEN = import.meta.env.VITE_EDGE_TRUSTED_TOKEN;
const EDGE_WS_BASE =
  "wss://speech.platform.bing.com/consumer/speech/synthesize/readaloud/edge/v1";
const EDGE_CHROMIUM_VERSION = "130.0.2849.68";

function newGuid(): string {
  // crypto.randomUUID() is available in Tauri webviews; strip dashes since
  // Edge's protocol expects the connection/request IDs bare.
  return crypto.randomUUID().replace(/-/g, "");
}

// Microsoft gates the endpoint behind a `Sec-MS-GEC` token: SHA-256 of
// "<windows-file-time-rounded-to-5min><trusted client token>", uppercased.
// Ported from the algorithm used by the edge-tts Python/Node libraries.
async function computeSecMsGec(): Promise<string> {
  const WIN_EPOCH_SECONDS = 11644473600n; // seconds between 1601-01-01 and 1970-01-01
  let seconds = BigInt(Math.floor(Date.now() / 1000)) + WIN_EPOCH_SECONDS;
  seconds -= seconds % 300n; // round down to a 5-minute window
  const windowsTicks = seconds * 10_000_000n; // seconds -> 100ns ticks

  const toHash = `${windowsTicks.toString()}${EDGE_TRUSTED_CLIENT_TOKEN}`;
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(toHash),
  );
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("")
    .toUpperCase();
}

function escapeSsml(text: string): string {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

// Talks to Microsoft's TTS websocket and resolves with a playable audio Blob
// (mp3) once the full utterance has streamed back.
async function synthesizeEdgeTTS(text: string): Promise<Blob> {
  const secMsGec = await computeSecMsGec();
  const connectionId = newGuid();
  const url =
    `${EDGE_WS_BASE}?TrustedClientToken=${EDGE_TRUSTED_CLIENT_TOKEN}` +
    `&Sec-MS-GEC=${secMsGec}` +
    `&Sec-MS-GEC-Version=1-${EDGE_CHROMIUM_VERSION}` +
    `&ConnectionId=${connectionId}`;

  return new Promise<Blob>((resolve, reject) => {
    const ws = new WebSocket(url);
    ws.binaryType = "arraybuffer";

    const audioChunks: Uint8Array[] = [];
    let settled = false;
    const timeout = setTimeout(() => {
      if (!settled) {
        settled = true;
        ws.close();
        reject(new Error("Edge TTS timed out"));
      }
    }, 15000);

    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      ws.close();
      if (audioChunks.length === 0) {
        reject(new Error("Edge TTS returned no audio"));
        return;
      }
      resolve(new Blob(audioChunks as BlobPart[], { type: "audio/mpeg" }));
    };

    const fail = (err: unknown) => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      try {
        ws.close();
      } catch {
        // already closing/closed
      }
      reject(err instanceof Error ? err : new Error(String(err)));
    };

    ws.onopen = () => {
      const timestamp = new Date().toUTCString();

      // 1. Speech config — request mp3 output so it's easy to play back.
      ws.send(
        `X-Timestamp:${timestamp}\r\n` +
          `Content-Type:application/json; charset=utf-8\r\n` +
          `Path:speech.config\r\n\r\n` +
          JSON.stringify({
            context: {
              synthesis: {
                audio: {
                  metadataoptions: {
                    sentenceBoundaryEnabled: "false",
                    wordBoundaryEnabled: "false",
                  },
                  outputFormat: "audio-24khz-48kbitrate-mono-mp3",
                },
              },
            },
          }),
      );

      // 2. SSML request with the actual text to speak.
      const requestId = newGuid();
      const ssml =
        `<speak version='1.0' xmlns='http://www.w3.org/2001/10/synthesis' xml:lang='en-US'>` +
        `<voice name='${EDGE_TTS_VOICE}'>` +
        `<prosody pitch='+0Hz' rate='+0%' volume='+0%'>${escapeSsml(text)}</prosody>` +
        `</voice></speak>`;
      ws.send(
        `X-RequestId:${requestId}\r\n` +
          `Content-Type:application/ssml+xml\r\n` +
          `X-Timestamp:${timestamp}\r\n` +
          `Path:ssml\r\n\r\n${ssml}`,
      );
    };

    ws.onmessage = (event) => {
      if (typeof event.data === "string") {
        if (event.data.includes("Path:turn.end")) {
          finish();
        }
        // "Path:turn.start" / "Path:response" / audio.metadata frames are
        // informational only — nothing else to do with them here.
        return;
      }

      // Binary frame: [2-byte big-endian header length][header text][audio bytes]
      const buffer = event.data as ArrayBuffer;
      if (buffer.byteLength < 2) return;
      const view = new DataView(buffer);
      const headerLength = view.getUint16(0, false);
      const headerBytes = new Uint8Array(buffer, 2, headerLength);
      const header = new TextDecoder().decode(headerBytes);
      if (header.includes("Path:audio")) {
        audioChunks.push(new Uint8Array(buffer, 2 + headerLength));
      }
    };

    ws.onerror = () => fail(new Error("Edge TTS websocket error"));
    ws.onclose = (event) => {
      if (!settled && !event.wasClean) {
        fail(new Error(`Edge TTS websocket closed unexpectedly (${event.code})`));
      }
    };
  });
}

// Module-scoped audio element reused across calls so we don't leak elements.
let ttsAudio: HTMLAudioElement | null = null;

async function speakEdgeTTS(text: string): Promise<void> {
  if (!ttsAudio) {
    ttsAudio = new Audio();
  }
  ttsAudio.pause();

  const blob = await synthesizeEdgeTTS(text);
  const blobUrl = URL.createObjectURL(blob);

  await new Promise<void>((resolve, reject) => {
    if (!ttsAudio) {
      reject(new Error("audio element missing"));
      return;
    }
    ttsAudio.src = blobUrl;
    const cleanup = () => {
      URL.revokeObjectURL(blobUrl);
      if (ttsAudio) {
        ttsAudio.onended = null;
        ttsAudio.onerror = null;
      }
    };
    ttsAudio.onended = () => {
      cleanup();
      console.debug("[tts] Edge TTS playback finished");
      resolve();
    };
    ttsAudio.onerror = () => {
      cleanup();
      reject(new Error("Edge TTS audio playback error"));
    };
    ttsAudio.play().catch((err: unknown) => {
      cleanup();
      reject(err);
    });
  });
}

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

// Last-resort fallback using the built-in (robotic) Web Speech API voices,
// used only if Edge TTS is unreachable or blocked.
async function speakWebFallback(text: string): Promise<void> {
  if (!window.speechSynthesis) {
    console.warn("Speech synthesis not supported in this environment");
    return;
  }
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
    utterance.onend = () => resolve();
    utterance.onerror = () => resolve();
    window.speechSynthesis.speak(utterance);
    setTimeout(() => {
      if (!window.speechSynthesis.speaking) resolve();
    }, 500);
  });
}

async function speak(text: string): Promise<void> {
  try {
    await speakEdgeTTS(text);
    return;
  } catch (error) {
    console.error("[tts] Edge TTS failed, falling back to web speech:", error);
  }
  await speakWebFallback(text);
}

async function ask(userText: string): Promise<string> {
  // Search is best-effort context; never let it abort the conversation.
  const context = await searchTheWeb(userText);
  const userContent = userText;
  conversation.push({ role: "user", content: userContent });
  const tempMessages = [
    ...conversation.slice(0, -1), // all previous messages except the last user message
    {
      role: "system",
      content: `Here is relevant web context to help answer the user's next message: ${JSON.stringify(context)}`,
    },
    conversation[conversation.length - 1], // the user's actual message, last
  ];
  const res: any = await client.chat.completions.create({
    model: 'gpt-oss-120b',
    messages: tempMessages as any,
  });

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