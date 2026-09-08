import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { runAgent, type AgentMessage } from "../agent/agent";
import { learnProfile } from "../agent/profile";
import { speakWithSystemVoice, streamEdgeSpeech, type SpeechHandle, type TtsDiagnostic } from "../voice/edgeTts";
import type { ConfirmationRequest, ProfileItem, Settings, ToolActivity } from "../agent/tools";

export type AssistantStatus = "standby" | "listening" | "thinking" | "speaking" | "error";
export interface TranscriptEntry { id: string; role: "user" | "assistant"; text: string; timestamp: number }
export interface Toast { id: string; text: string; tone: "info" | "error" | "success" }
type PendingConfirmation = ConfirmationRequest & { resolve: (approved: boolean) => void };
const DEFAULT_SETTINGS: Settings = { hotkey: "CommandOrControl+Shift+Space", closeToTray: true, launchAtLogin: false, autoListenOnShow: true, speakReplies: true, voice: "en-GB-SoniaNeural", model: "openrouter/free", useRag: true, allowWeb: true, autoApprove: false, calendarId: "primary" };
function guid(): string { return crypto.randomUUID().replace(/-/g, ""); }
function errorDetail(error: unknown): string {
  if (error instanceof Error) return error.stack || `${error.name}: ${error.message}`;
  try { return typeof error === "string" ? error : JSON.stringify(error); } catch { return String(error); }
}
async function speak(text: string, voice: string, signal: AbortSignal, speechRef: React.MutableRefObject<SpeechHandle | null>, report: (diagnostic: TtsDiagnostic) => void, onFallback: (message: string) => void): Promise<void> {
  try {
    const handle = await streamEdgeSpeech(text, voice, signal, report); speechRef.current = handle; await handle.done;
  } catch (error) {
    if (signal.aborted) return;
    const detail = error instanceof Error ? error.message : String(error);
    report({ stage: "fallback", detail });
    onFallback("Using the best available system voice.");
    await speakWithSystemVoice(text, signal);
  } finally { speechRef.current = null; }
}

export function useVoiceAssistant() {
  const [isListening, setIsListening] = useState(false), [phase, setPhase] = useState<"idle" | "thinking" | "speaking">("idle");
  const [partial, setPartial] = useState(""), [level, setLevel] = useState(0), [transcript, setTranscript] = useState<TranscriptEntry[]>([]);
  const [errorMessage, setErrorMessage] = useState(""), [debugLogs, setDebugLogs] = useState<string[]>([]), [activities, setActivities] = useState<ToolActivity[]>([]);
  const [confirmation, setConfirmation] = useState<ConfirmationRequest | null>(null), [settings, setSettings] = useState<Settings>(DEFAULT_SETTINGS), [toasts, setToasts] = useState<Toast[]>([]);
  const [speechDiagnostic, setSpeechDiagnostic] = useState<TtsDiagnostic | null>(null), [isTestingVoice, setIsTestingVoice] = useState(false);
  const conversationRef = useRef<AgentMessage[]>([]), processingRef = useRef(false), abortRef = useRef<AbortController | null>(null), speechRef = useRef<SpeechHandle | null>(null), profileRef = useRef<ProfileItem[]>([]), confirmationRef = useRef<PendingConfirmation | null>(null);
  const toast = useCallback((text: string, tone: Toast["tone"] = "info") => { const id = guid(); setToasts((v) => [...v, { id, text, tone }]); window.setTimeout(() => setToasts((v) => v.filter((item) => item.id !== id)), 4500); }, []);
  const addEntry = useCallback((role: TranscriptEntry["role"], text: string) => setTranscript((v) => [...v, { id: guid(), role, text, timestamp: Date.now() }]), []);
  const updateActivity = useCallback((next: ToolActivity) => setActivities((items) => {
    const index = items.findIndex((item) => item.callId === next.callId);
    if (next.protected && (next.status === "success" || next.status === "denied")) return index < 0 ? items : items.filter((item) => item.callId !== next.callId);
    if (index < 0) return [...items, next];
    const copy = [...items]; copy[index] = next; return copy;
  }), []);
  const requestConfirmation = useCallback((request: ConfirmationRequest) => new Promise<boolean>((resolve) => { const pending = { ...request, resolve }; confirmationRef.current = pending; setConfirmation(request); }), []);
  const resolveConfirmation = useCallback((approved: boolean) => { const resolve = confirmationRef.current?.resolve; confirmationRef.current = null; setConfirmation(null); resolve?.(approved); }, []);

  const submitText = useCallback(async (raw: string) => {
    const text = raw.trim(); if (!text || processingRef.current) return;
    processingRef.current = true; setErrorMessage(""); addEntry("user", text); setPhase("thinking");
    const controller = new AbortController(); abortRef.current = controller;
    try {
      const result = await runAgent(text, { settings, messages: conversationRef.current, confirm: requestConfirmation, onActivity: updateActivity, onDebug: (line) => setDebugLogs((v) => [...v.slice(-199), line]), signal: controller.signal });
      conversationRef.current = result.messages; addEntry("assistant", result.reply);
      void learnProfile(text, result.reply, profileRef.current, settings.model).then((profile) => { profileRef.current = profile; }).catch((error) => setDebugLogs((v) => [...v.slice(-49), `Profile update: ${String(error)}`]));
      if (settings.speakReplies && !controller.signal.aborted) {
        setPhase("speaking");
        await speak(result.reply, settings.voice, controller.signal, speechRef, (diagnostic) => {
          setSpeechDiagnostic(diagnostic);
          if (diagnostic.stage === "fallback") setDebugLogs((v) => [...v.slice(-49), `Edge speech: ${diagnostic.detail}`]);
        }, (message) => toast(message, "info"));
      }
    } catch (error) {
      if (!(error instanceof DOMException && error.name === "AbortError")) { const message = error instanceof Error ? error.message : String(error); setDebugLogs((v) => [...v.slice(-199), `[agent] ${errorDetail(error)}`]); setErrorMessage(message); toast(message, "error"); }
    } finally { processingRef.current = false; abortRef.current = null; setPhase("idle"); }
  }, [addEntry, requestConfirmation, settings, toast, updateActivity]);

  const cancel = useCallback(() => { resolveConfirmation(false); abortRef.current?.abort(); speechRef.current?.stop(); window.speechSynthesis.cancel(); processingRef.current = false; setPhase("idle"); }, [resolveConfirmation]);
  const toggleListening = useCallback(async () => { setErrorMessage(""); try { if (isListening) await invoke("stop_listening"); else await invoke("start_listening"); } catch (error) { const message = String(error); setErrorMessage(message); toast(message, "error"); } }, [isListening, toast]);
  const saveSettings = useCallback(async (next: Settings) => { try { const saved = await invoke<Settings>("save_settings", { settings: next }); setSettings(saved); toast("Settings saved.", "success"); return saved; } catch (error) { toast(String(error), "error"); throw error; } }, [toast]);
  const testVoice = useCallback(async (voice = settings.voice) => {
    if (isTestingVoice) return;
    const controller = new AbortController(); abortRef.current?.abort(); abortRef.current = controller; setIsTestingVoice(true); setPhase("speaking");
    try {
      await speak("Hello. I’m Theta, ready whenever you are.", voice, controller.signal, speechRef, (diagnostic) => {
        setSpeechDiagnostic(diagnostic);
        if (diagnostic.stage === "fallback") setDebugLogs((v) => [...v.slice(-49), `Edge speech: ${diagnostic.detail}`]);
      }, (message) => toast(message, "info"));
    } finally { if (abortRef.current === controller) abortRef.current = null; setIsTestingVoice(false); setPhase("idle"); }
  }, [isTestingVoice, settings.voice, toast]);

  useEffect(() => {
    const captureError = (event: ErrorEvent) => setDebugLogs((v) => [...v.slice(-199), `[window] ${event.error ? errorDetail(event.error) : `${event.message} at ${event.filename}:${event.lineno}:${event.colno}`}`]);
    const captureRejection = (event: PromiseRejectionEvent) => setDebugLogs((v) => [...v.slice(-199), `[promise] ${errorDetail(event.reason)}`]);
    window.addEventListener("error", captureError); window.addEventListener("unhandledrejection", captureRejection);
    return () => { window.removeEventListener("error", captureError); window.removeEventListener("unhandledrejection", captureRejection); };
  }, []);
  useEffect(() => { invoke<Settings>("get_settings").then((loaded) => {
    if (loaded.model === "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free") {
      const migrated = { ...loaded, model: "openrouter/free" }; setSettings(migrated);
      void invoke<Settings>("save_settings", { settings: migrated }).catch((error) => setDebugLogs((v) => [...v.slice(-199), `[settings] Model migration: ${errorDetail(error)}`]));
    } else setSettings(loaded);
  }).catch((error) => toast(String(error), "error")); }, [toast]);
  useEffect(() => { invoke<ProfileItem[]>("profile_get").then((profile) => { profileRef.current = profile; }).catch((error) => setDebugLogs((v) => [...v.slice(-49), `Profile load: ${String(error)}`])); }, []);
  useEffect(() => {
    const unsubs: UnlistenFn[] = []; let disposed = false;
    const bind = async <T,>(name: string, handler: (payload: T) => void) => { const off = await listen<T>(name, (event) => handler(event.payload)); if (disposed) off(); else unsubs.push(off); };
    void bind<string>("vosk-speech-partial", setPartial);
    void bind<string>("vosk-speech-result", (text) => { setPartial(""); void submitText(text); });
    void bind<string>("vosk-error", (message) => { setErrorMessage(String(message)); toast(String(message), "error"); });
    void bind<boolean>("theta-listening", setIsListening);
    void bind<number>("theta-level", (value) => setLevel(Math.max(0, Math.min(1, Number(value) / 1000))));
    void bind<boolean>("theta-hotkey", setIsListening);
    void bind<string>("theta-debug", (line) => setDebugLogs((v) => [...v.slice(-49), line]));
    void invoke<boolean>("is_listening").then(setIsListening).catch(() => undefined);
    return () => { disposed = true; unsubs.forEach((off) => off()); abortRef.current?.abort(); confirmationRef.current?.resolve(false); };
  }, [submitText, toast]);

  const status: AssistantStatus = errorMessage ? "error" : phase !== "idle" ? phase : isListening ? "listening" : "standby";
  return { status, isListening, level, partial, transcript, errorMessage, debugLogs, activities, confirmation, settings, toasts, speechDiagnostic, isTestingVoice, submitText, toggleListening, testVoice, approve: () => resolveConfirmation(true), deny: () => resolveConfirmation(false), cancel, stopSpeaking: cancel, saveSettings, setErrorMessage };
}