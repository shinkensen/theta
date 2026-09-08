import { invoke } from "@tauri-apps/api/core";

export interface Settings {
  hotkey: string;
  closeToTray: boolean;
  launchAtLogin: boolean;
  autoListenOnShow: boolean;
  speakReplies: boolean;
  voice: string;
  model: string;
  useRag: boolean;
  allowWeb: boolean;
  autoApprove: boolean;
  calendarId: string;
}

export interface EventInput {
  summary?: string;
  description?: string;
  location?: string;
  start?: string;
  end?: string;
  all_day?: boolean;
  attendees?: string[];
}
export interface CalEvent { id: string; summary: string; description?: string; location?: string; start: string; end: string; all_day: boolean; calendar_id: string; html_link?: string; status?: string; attendees: string[] }
export interface CalendarSummary { id: string; summary: string; primary: boolean; access_role?: string }
export interface ProcessInfo { pid: number; parent_pid?: number; name: string; cmd: string; exe?: string; cpu: number; memory_mb: number; run_time: number; status: string }
export interface SystemStats { os: string; os_version: string; kernel: string; hostname: string; arch: string; cpu_brand: string; cpu_count: number; cpu_usage: number; mem_total_mb: number; mem_used_mb: number; mem_percent: number; swap_total_mb: number; swap_used_mb: number; uptime_secs: number; process_count: number }
export interface PortInfo { port: number; proto: string; address: string; pid: number; process: string }
export interface RagHit { id: string; title: string; source: string; text: string; score: number; created: number }
export interface RagDoc { id: string; title: string; source: string; created: number; preview: string; tags: string[] }
export type ProfileCategory = "interest" | "hobby" | "project" | "preference" | "recurring_topic";
export interface ProfileItem { id: string; category: ProfileCategory; text: string; confidence: number; evidenceCount: number; explicit: boolean; created: number; lastSeen: number }
export interface ProfileOperation { category: ProfileCategory; text: string; explicit?: boolean; replaceId?: string }
export interface CommandOutput { stdout: string; stderr: string; exit_code: number; timed_out: boolean }

export type ToolName = "search_web" | "rag_search" | "rag_ingest_text" | "rag_ingest_file" | "rag_list" | "rag_forget" | "calendar_list_calendars" | "calendar_list_events" | "calendar_create_event" | "calendar_update_event" | "calendar_delete_event" | "calendar_quick_add" | "list_processes" | "system_stats" | "listening_ports" | "kill_process" | "run_command";
export interface ToolCall { id: string; type: "function"; function: { name: ToolName | string; arguments: string } }
export interface ToolActivity { id: string; callId: string; name: string; args: unknown; protected: boolean; status: "waiting" | "running" | "success" | "denied" | "error"; message?: string; startedAt: number; finishedAt?: number }
export interface ConfirmationRequest { id: string; tool: ToolName; args: unknown; summary: string }
export type ConfirmTool = (request: ConfirmationRequest) => Promise<boolean>;

const objectSchema = (properties: Record<string, unknown>, required: string[] = []) => ({ type: "object", properties, required, additionalProperties: false });
const str = (description: string) => ({ type: "string", description });
const num = (description: string) => ({ type: "number", description });
const eventSchema = (description: string) => ({
  type: "object", description, additionalProperties: false,
  properties: {
    summary: str("Event title"), description: str("Event description"), location: str("Event location"),
    start: str("Start as RFC3339, YYYY-MM-DDTHH:MM, or YYYY-MM-DD"),
    end: str("End in the same format; defaults to one hour after start"),
    all_day: { type: "boolean", description: "Whether this is an all-day event" },
    attendees: { type: "array", items: { type: "string" }, description: "Attendee email addresses" },
  },
});
export const TOOL_DEFINITIONS: Array<{ type: "function"; function: { name: string; description: string; parameters: Record<string, unknown> } }> = ([
  ["search_web", "Search current web pages for information.", objectSchema({ query: str("Search query"), limit: num("Maximum results, 1-8") }, ["query"])],
  ["rag_search", "Search local memory.", objectSchema({ query: str("Search query"), limit: num("Maximum hits") }, ["query"])],
  ["rag_ingest_text", "Save text into local memory.", objectSchema({ text: str("Text to save"), title: str("Title"), source: str("Source label"), tags: { type: "array", items: { type: "string" } } }, ["text"])],
  ["rag_ingest_file", "Read a UTF-8 file and save it into local memory.", objectSchema({ path: str("Absolute file path") }, ["path"])],
  ["rag_list", "List local memory documents.", objectSchema({ limit: num("Maximum documents") })],
  ["rag_forget", "Delete a local memory document or source.", objectSchema({ id: str("Document id"), source: str("Source to delete") })],
  ["calendar_list_calendars", "List Google calendars.", objectSchema({})],
  ["calendar_list_events", "List calendar events in a time window.", objectSchema({ timeMin: str("RFC3339 or YYYY-MM-DD"), timeMax: str("RFC3339 or YYYY-MM-DD"), calendarId: str("Calendar id"), query: str("Text filter"), maxResults: num("Maximum events") })],
  ["calendar_create_event", "Create a calendar event. Pass start and end as strings, never as dateTime/date objects or JSON strings.", objectSchema({ event: eventSchema("Structured event fields"), calendarId: str("Calendar id") }, ["event"])],
  ["calendar_update_event", "Update a calendar event. Pass start and end as strings, never as dateTime/date objects or JSON strings.", objectSchema({ eventId: str("Event id"), event: eventSchema("Partial structured event fields"), calendarId: str("Calendar id") }, ["eventId", "event"])],
  ["calendar_delete_event", "Delete a calendar event.", objectSchema({ eventId: str("Event id"), calendarId: str("Calendar id") }, ["eventId"])],
  ["calendar_quick_add", "Create an event from natural language.", objectSchema({ text: str("Event description"), calendarId: str("Calendar id") }, ["text"])],
  ["list_processes", "List running processes.", objectSchema({ query: { type: "object", properties: { filter: str("Name or command filter"), sort_by: str("cpu, memory, name, or pid"), limit: num("Maximum rows") } } })],
  ["system_stats", "Read system CPU and memory statistics.", objectSchema({})],
  ["listening_ports", "List local listening network ports.", objectSchema({})],
  ["kill_process", "Force stop a process.", objectSchema({ pid: num("Process id") }, ["pid"])],
  ["run_command", "Run a shell command and capture output.", objectSchema({ command: str("Shell command"), cwd: str("Working directory"), timeoutSecs: num("Timeout seconds") }, ["command"])],
] as Array<[string, string, Record<string, unknown>]>).map(([name, description, parameters]) => ({ type: "function" as const, function: { name, description, parameters } }));

export const REQUIRES_CONFIRMATION = new Set<ToolName>([
  "kill_process", "run_command", "calendar_create_event", "calendar_update_event",
  "calendar_delete_event", "calendar_quick_add", "rag_forget", "rag_ingest_text", "rag_ingest_file",
]);

export function confirmationSummary(name: ToolName, args: Record<string, unknown>): string {
  const target = name === "kill_process" ? `PID ${String(args.pid)}`
    : name === "run_command" ? String(args.command)
    : name === "rag_ingest_file" ? String(args.path)
    : name === "rag_ingest_text" ? String(args.title ?? "this text")
    : name === "rag_forget" ? String(args.id ?? args.source ?? "memory")
    : name.startsWith("calendar_") ? String((args.event as EventInput | undefined)?.summary ?? args.text ?? args.eventId ?? "calendar event")
    : JSON.stringify(args);
  return `${name.replace(/_/g, " ")}: ${target}`;
}

function abortError(): DOMException { return new DOMException("The operation was cancelled.", "AbortError"); }
function ensureActive(signal?: AbortSignal): void { if (signal?.aborted) throw abortError(); }
function asArgs(value: unknown): Record<string, unknown> { if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("Tool arguments must be a JSON object."); return value as Record<string, unknown>; }
function optionalString(v: unknown): string | undefined { return typeof v === "string" ? v : undefined; }
function calendarEvent(value: unknown): Record<string, unknown> {
  let event = value;
  if (typeof event === "string") {
    try { event = JSON.parse(event); } catch { throw new Error("Calendar event must be a JSON object with string start and end fields."); }
  }
  const row = asArgs(event);
  const time = (value: unknown): unknown => {
    if (!value || typeof value !== "object" || Array.isArray(value)) return value;
    const node = value as Record<string, unknown>;
    return node.dateTime ?? node.date ?? value;
  };
  return { ...row, start: time(row.start), end: time(row.end) };
}
function optionalNumber(v: unknown): number | undefined { return typeof v === "number" && Number.isFinite(v) ? v : undefined; }
export async function executeTool(name: ToolName, rawArgs: unknown, signal?: AbortSignal): Promise<unknown> {
  ensureActive(signal);
  const a = asArgs(rawArgs);
  let result: unknown;
  switch (name) {
    case "search_web": result = await searchWeb(String(a.query ?? ""), optionalNumber(a.limit), signal); break;
    case "rag_search": result = await invoke<RagHit[]>(name, { query: String(a.query ?? ""), limit: optionalNumber(a.limit) }); break;
    case "rag_ingest_text": result = await invoke(name, { text: String(a.text ?? ""), title: optionalString(a.title), source: optionalString(a.source), tags: Array.isArray(a.tags) ? a.tags : undefined }); break;
    case "rag_ingest_file": result = await invoke(name, { path: String(a.path ?? "") }); break;
    case "rag_list": result = await invoke<RagDoc[]>(name, { limit: optionalNumber(a.limit) }); break;
    case "rag_forget": result = await invoke<number>(name, { id: optionalString(a.id), source: optionalString(a.source) }); break;
    case "calendar_list_calendars": result = await invoke<CalendarSummary[]>(name); break;
    case "calendar_list_events": result = await invoke<CalEvent[]>(name, { timeMin: optionalString(a.timeMin), timeMax: optionalString(a.timeMax), calendarId: optionalString(a.calendarId), query: optionalString(a.query), maxResults: optionalNumber(a.maxResults) }); break;
    case "calendar_create_event": result = await invoke<CalEvent>(name, { event: calendarEvent(a.event), calendarId: optionalString(a.calendarId) }); break;
    case "calendar_update_event": result = await invoke<CalEvent>(name, { eventId: String(a.eventId ?? ""), event: calendarEvent(a.event), calendarId: optionalString(a.calendarId) }); break;
    case "calendar_delete_event": result = await invoke<string>(name, { eventId: String(a.eventId ?? ""), calendarId: optionalString(a.calendarId) }); break;
    case "calendar_quick_add": result = await invoke<CalEvent>(name, { text: String(a.text ?? ""), calendarId: optionalString(a.calendarId) }); break;
    case "list_processes": result = await invoke<ProcessInfo[]>(name, { query: a.query }); break;
    case "system_stats": result = await invoke<SystemStats>(name); break;
    case "listening_ports": result = await invoke<PortInfo[]>(name); break;
    case "kill_process": result = await invoke<string>(name, { pid: Number(a.pid) }); break;
    case "run_command": result = await invoke<CommandOutput>(name, { command: String(a.command ?? ""), cwd: optionalString(a.cwd), timeoutSecs: optionalNumber(a.timeoutSecs) }); break;
    default: throw new Error(`Unknown tool: ${String(name)}`);
  }
  ensureActive(signal);
  return result;
}

interface WebResult { url: string; title?: string; description?: string; markdown?: string }
async function searchWeb(query: string, requestedLimit = 3, signal?: AbortSignal): Promise<WebResult[]> {
  const key = import.meta.env.VITE_FIRECRAWL_KEY;
  if (!key) throw new Error("Web search is not configured.");
  const limit = Math.max(1, Math.min(8, requestedLimit));
  const response = await fetch("https://api.firecrawl.dev/v1/search", {
    method: "POST", signal,
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${key}` },
    body: JSON.stringify({ query, limit: limit + 3, scrapeOptions: { formats: ["markdown"], onlyMainContent: true } }),
  });
  if (!response.ok) throw new Error(`Web search returned ${response.status}.`);
  const body = await response.json() as { success?: boolean; error?: string; data?: WebResult[] };
  if (!body.success) throw new Error(body.error ?? "Web search failed.");
  const blocked = ["youtube.com", "youtu.be", "spotify.com", "instagram.com"];
  return (body.data ?? []).filter((item) => {
    try { const host = new URL(item.url).hostname.replace(/^www\./, ""); return !blocked.some((domain) => host === domain || host.endsWith(`.${domain}`)); }
    catch { return true; }
  }).slice(0, limit).map((item) => ({ ...item, markdown: item.markdown?.slice(0, 7000) }));
}

export function isToolName(name: string): name is ToolName {
  return TOOL_DEFINITIONS.some((tool) => tool.function.name === name);
}
