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
const NUM_RESULTS_WEB = 3;

interface FirecrawlSearchResult {
  url: string;
  title?: string;
  description?: string;
  markdown?: string;
}

// These sites tend to scrape into enormous, noisy markdown (embedded
// JSON, related-content blocks, tracking data) that regularly blows past
// what a single summarization request will accept — and even when it
// doesn't, the content itself (video/audio pages) isn't usefully
// summarizable as text anyway. Simplest fix: don't scrape them at all.
const BLOCKED_RESULT_DOMAINS = [
  "youtube.com",
  "youtu.be",
  "spotify.com",
  "instagram.com",
];

function isBlockedDomain(url: string): boolean {
  try {
    const hostname = new URL(url).hostname.replace(/^www\./, "");
    return BLOCKED_RESULT_DOMAINS.some(
      (domain) => hostname === domain || hostname.endsWith(`.${domain}`),
    );
  } catch {
    return false;
  }
}

// `scrapeOptions.formats: ["markdown"]` here means Firecrawl already scrapes
// the *full* page for every result, not just a snippet — each result's
// `markdown` field is the whole page. (If it looked truncated in devtools,
// that's just the console collapsing long strings for display; the full
// text is there.) `onlyMainContent` strips nav/header/footer boilerplate
// before it even gets to us.
async function searchTheWeb(
  query: string,
  limit = NUM_RESULTS_WEB,
): Promise<FirecrawlSearchResult[] | string> {
  try {
    // Over-fetch a bit since some results get filtered out afterward —
    // otherwise a query that happens to surface a YouTube/Spotify link
    // would silently return fewer usable results than requested.
    const fetchLimit = limit + 3;
    const res = await fetch("https://api.firecrawl.dev/v1/search", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${FIRECRAWL_API_KEY}`,
      },
      body: JSON.stringify({
        query,
        limit: fetchLimit,
        scrapeOptions: {
          formats: ["markdown"],
          onlyMainContent: true,
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
    const results = json.data as FirecrawlSearchResult[];
    return results.filter((r) => !isBlockedDomain(r.url)).slice(0, limit);
  } catch (error) {
    console.error("Search failed:", error);
    return `Error during search: ${error}`;
  }
}

const client = new Cerebras({
  apiKey: import.meta.env.VITE_CEREBRAS_API_KEY,
  dangerouslyAllowBrowser: true,
});

// Pages like YouTube/Spotify often scrape down to markdown that's mostly
// embedded JSON, related-content lists, and other non-visible cruft — easily
// hundreds of KB. Truncating up front keeps the combined context small; the
// answer we actually need is almost always near the top of the page anyway.
//
// NOTE: this used to run each page through its own Cerebras summarization
// call before folding the result into the main conversation call — up to
// 4 Cerebras requests per turn (3 pages + 1 answer). Cerebras' rate limit
// is a shared tokens-per-minute budget across *all* requests, so that
// pattern burned through it fast and 429'd constantly. Now there's no
// separate summarization step at all: the truncated raw markdown for every
// page is concatenated directly into the system message of the single main
// call, so there's exactly one Cerebras request per turn regardless of how
// many pages were scraped.
//
// Bumped from 4000 -> 9000: long-form articles (best-of listicles,
// comparison posts) often carry a big table of contents / intro before the
// actual useful content (e.g. a comparison table) even starts. At 4000
// chars, that useful part was regularly getting cut off entirely — the
// "[...truncated...]" marker was landing right before the content that
// actually mattered. There's headroom for this since it's one combined
// request instead of one per page now.
const MAX_PAGE_MARKDOWN_CHARS = 9000;

function truncateMarkdown(markdown: string): string {
  if (markdown.length <= MAX_PAGE_MARKDOWN_CHARS) return markdown;
  return markdown.slice(0, MAX_PAGE_MARKDOWN_CHARS) + "\n\n[...truncated...]";
}

// No LLM call here — just formats and truncates. Kept as its own function
// so it's easy to swap back to per-page or batch summarization later if
// rate limits ever stop being the binding constraint.
function buildWebContext(results: FirecrawlSearchResult[]): string {
  const pages = results.filter((r) => r.markdown);
  if (pages.length === 0) return "";

  return pages
    .map((page) => `From ${page.url}:\n${truncateMarkdown(page.markdown!)}`)
    .join("\n\n---\n\n");
}

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
// Broadened beyond plain {role, content} to also carry tool_calls (on
// assistant messages that invoke search_web) and tool_call_id (on the
// resulting tool-role messages) — see the tool-calling flow in ask().
//
// FIX: this used to be `new Date().getDate().toString()`, which is only
// the day-of-month integer (e.g. "17") — no month, no year. The model had
// no way to know it was actually mid-2026, so left to its own devices it
// defaulted to its training-era assumption and searched for "2024" instead
// of the current year. Using a full formatted date (with year) fixes that
// at the source.
const now = new Date();
const currentDateString = now.toLocaleDateString("en-US", {
  weekday: "long",
  year: "numeric",
  month: "long",
  day: "numeric",
});
let conversation: Array<{
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  tool_calls?: any[];
  tool_call_id?: string;
}> = [
    {
      role: "system",
      content:
        `You are a helpful voice assistant. Today's date is ${currentDateString}. Trust this date over ` +
        "whatever year you'd otherwise assume — your training data has a cutoff well before today, so don't " +
        "default to an old year when forming search queries or answering questions about what's current. " +
        "DO NOT INCLUDE FORMATTING. Keep replies short and conversational — they will be read aloud. " +
        "You have a search_web tool available. Call it whenever you want, in fact, use it for most queries. " +
        "Particularly information you don't already know or things that change rapidly — tech, news, prices, " +
        "recent events, specific facts. Don't use it for things you can already answer, general conversation, " +
        "or something already searched earlier in this conversation.",
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

// Set while an Edge TTS websocket is open, so interruptSpeech() (below) can
// close it from outside synthesizeEdgeTTS if the user stops playback while
// audio is still being synthesized (before any of it has started playing).
let activeEdgeWs: WebSocket | null = null;

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
    activeEdgeWs = ws;

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
      if (activeEdgeWs === ws) activeEdgeWs = null;
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
      if (activeEdgeWs === ws) activeEdgeWs = null;
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

// True while the person has asked to stop mid-utterance — checked after
// each await in speak() so a stop doesn't cause a fallback voice to pick
// up where Edge TTS left off.
let stopRequested = false;

// Set only while audio is actively playing (i.e. once we're inside the
// `new Promise` in speakEdgeTTS below). interruptSpeech() calls this to
// resolve that promise immediately — just pausing the <audio> element
// wouldn't fire 'ended' or 'error', so the awaiting promise would otherwise
// hang forever instead of letting the rest of the ask() flow finish.
let stopActivePlayback: (() => void) | null = null;

async function speakEdgeTTS(text: string): Promise<void> {
  if (!ttsAudio) {
    ttsAudio = new Audio();
  }
  ttsAudio.pause();

  const blob = await synthesizeEdgeTTS(text);
  if (stopRequested) return; // stopped while synthesizing, before playback ever started

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
      stopActivePlayback = null;
    };
    stopActivePlayback = () => {
      ttsAudio?.pause();
      cleanup();
      resolve(); // an intentional stop is a normal completion, not an error
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
  stopRequested = false;
  try {
    await speakEdgeTTS(text);
    return;
  } catch (error) {
    if (stopRequested) return; // interrupted mid-synthesis — not a real failure
    console.error("[tts] Edge TTS failed, falling back to web speech:", error);
  }
  if (stopRequested) return;
  await speakWebFallback(text);
}

// Stops whatever speech is currently in flight, however far along it is:
// - mid-synthesis (Edge TTS websocket still streaming, nothing playing yet)
// - mid-playback (Edge TTS audio already playing)
// - the Web Speech fallback utterance
// In every case the corresponding `speak()` call resolves normally rather
// than throwing, so the ask()/speak() flow in the hook below finishes
// cleanly and the assistant is immediately ready for the next turn.
function interruptSpeech(): void {
  stopRequested = true;

  if (activeEdgeWs) {
    try {
      activeEdgeWs.close();
    } catch {
      // already closing/closed
    }
    activeEdgeWs = null;
  }

  if (stopActivePlayback) {
    stopActivePlayback();
  } else if (ttsAudio) {
    ttsAudio.pause();
  }

  if (window.speechSynthesis) {
    window.speechSynthesis.cancel();
  }
}

// Given to the model as a callable tool rather than running automatically
// on every turn. Cerebras' gpt-oss-120b supports OpenAI-style tool calling,
// so the model itself decides — based on the conversation so far — whether
// a given question actually needs a web search, and skips it for anything
// it can already answer (chit-chat, follow-ups, things already searched
// earlier in the same conversation).
const SEARCH_TOOL_DEFINITION = {
  type: "function",
  function: {
    name: "search_web",
    description:
      "Search the web and return content from a few relevant pages. Use whenever you want, in fact, use it for most queries. " +
      "Particularly information you don't already know or things that change rapidly — tech, news, prices, recent events, specific facts. " +
      "Dont use it for things you can already answer, general conversation, or something already searched earlier " +
      "in this conversation.",
    parameters: {
      type: "object",
      properties: {
        query: {
          type: "string",
          description: "The search query.",
        },
      },
      required: ["query"],
    },
  },
};

function extractReplyText(message: any): string {
  const rawContent = message?.content;
  if (typeof rawContent === "string") return rawContent;
  if (Array.isArray(rawContent)) {
    return rawContent
      .filter((part: { type?: string; text?: string }) => part?.type === "text")
      .map((part: { text?: string }) => part?.text ?? "")
      .join("");
  }
  return "";
}

async function ask(userText: string): Promise<string> {
  conversation.push({ role: "user", content: userText });

  // First pass: let the model itself decide whether it needs to search.
  // Most turns don't, so this is normally the ONLY Cerebras request per
  // turn — search is opt-in per-message now instead of running every time.
  let res: any = await client.chat.completions.create({
    model: 'gpt-oss-120b',
    messages: conversation as any,
    tools: [SEARCH_TOOL_DEFINITION],
    tool_choice: "auto",
    parallel_tool_calls: false,
  });

  let message = res.choices[0]?.message;
  

  if (message?.tool_calls?.length) {
    const toolCall = message.tool_calls[0];
    let query = userText;
    try {
      query = JSON.parse(toolCall.function.arguments)?.query || userText;
    } catch {
      // Malformed arguments — fall back to the raw user message as the query.
    }

    console.log("[search_web]", query);
    const searchResult = await searchTheWeb(query);
    const context =
      typeof searchResult === "string" ? "" : buildWebContext(searchResult);
    console.log(context);

    // Record the tool call and its result in history, so the model (and
    // later turns) can see what was already searched and doesn't repeat it.
    conversation.push({
      role: "assistant",
      content: message.content ?? "",
      tool_calls: message.tool_calls,
    });
    conversation.push({
      role: "tool",
      tool_call_id: toolCall.id,
      content: context || "No relevant results were found.",
    });

    // Second pass: hand the model the search results and get its actual
    // answer. Only happens on turns where it chose to search — a plain
    // conversational turn never reaches this second request at all.
    res = await client.chat.completions.create({
      model: 'gpt-oss-120b',
      messages: conversation as any,
    });
    message = res.choices[0]?.message;
  }

  let reply = extractReplyText(message);
  reply = reply.replace(/\*/g, "");
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

  // Lets the UI put up a "stop talking" button. Only meaningful while
  // phase === "speaking", but it's harmless to call otherwise — with
  // nothing actively playing, interruptSpeech() is a no-op.
  const stopSpeaking = useCallback(() => {
    interruptSpeech();
  }, []);

  const status: AssistantStatus = errorMessage
    ? "error"
    : phase !== "idle"
      ? phase
      : isListening
        ? "listening"
        : "standby";

  return {
    status,
    partial,
    transcript,
    errorMessage,
    toggleListening,
    stopSpeaking,
    debugLogs,
  };
}