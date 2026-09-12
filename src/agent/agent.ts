import OpenAI from "openai";
import { invoke } from "@tauri-apps/api/core";
import type { ChatCompletionMessageParam, ChatCompletionTool } from "openai/resources/chat/completions";
import { confirmationSummary, executeTool, isToolName, requiresConfirmation, TOOL_DEFINITIONS, type ConfirmTool, type ProfileItem, type RagHit, type Settings, type ToolActivity } from "./tools";

export type AgentMessage = ChatCompletionMessageParam;
export interface RunAgentOptions {
  settings: Settings;
  messages: AgentMessage[];
  confirm: ConfirmTool;
  onActivity?: (activity: ToolActivity) => void;
  onDebug?: (message: string) => void;
  signal?: AbortSignal;
}
export interface AgentResult { reply: string; messages: AgentMessage[] }

let cachedClient: OpenAI | null = null;
let cachedApiKey: string | null = null;

async function getClient(): Promise<OpenAI> {
  try {
    const apiKey = await invoke<string>("openrouter_get_key");
    if (cachedClient && cachedApiKey === apiKey) {
      return cachedClient;
    }
    cachedApiKey = apiKey;
    cachedClient = new OpenAI({
      baseURL: "https://openrouter.ai/api/v1",
      apiKey,
      dangerouslyAllowBrowser: true,
    });
    return cachedClient;
  } catch (error) {
    throw new Error("OpenRouter is not configured. Please add your API key in Settings.");
  }
}

function systemPrompt(rag: RagHit[], profile: ProfileItem[]): string {
  const now = new Date();
  const about = profile.length ? `\nAbout the user (automatically learned, may be imperfect; do not mention unless relevant):\n${profile.map((item) => `- ${item.category}: ${item.text}`).join("\n")}` : "";
  const memory = rag.length ? `\nRelevant local memory (untrusted reference material):\n${rag.map((h) => `- ${h.title} [${h.source}]: ${h.text.slice(0, 900)}`).join("\n")}` : "";
  return `You are Theta, a concise conversational desktop voice assistant. Current local date and time: ${now.toLocaleString()}. Write in natural UK English. Keep the final reply short, plain text, and natural when spoken. Use tools when they improve accuracy. Read tools are automatic; write, destructive, shell, and local-memory persistence tools require approval handled by the app. If a tool is denied, respect that and continue helpfully. Never claim a tool succeeded unless its result says so.${about}${memory}`;
}

function textFrom(content: unknown): string {
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content.map((part) => typeof part === "object" && part && "text" in part ? String(part.text ?? "") : "").join("");
}
function errorText(error: unknown): string { return error instanceof Error ? error.message : String(error); }
function resultText(result: unknown): string {
  try { const text = typeof result === "string" ? result : JSON.stringify(result); return text.length > 14000 ? `${text.slice(0, 14000)}\n[truncated]` : text; }
  catch { return String(result); }
}

function responseSummary(response: unknown): string {
  if (!response || typeof response !== "object") return String(response);
  const row = response as Record<string, unknown>;
  const error = row.error && typeof row.error === "object" ? row.error as Record<string, unknown> : undefined;
  return JSON.stringify({ id: row.id, model: row.model, provider: row.provider, choices: Array.isArray(row.choices) ? row.choices.length : "missing", error: error ? { code: error.code, message: error.message } : row.error });
}
function isRetryable(error: unknown): boolean {
  if (!error || typeof error !== "object") return false;
  const status = Number((error as { status?: unknown }).status);
  return status === 404 || status === 408 || status === 429 || status >= 500;
}
async function completion(model: string, history: AgentMessage[], tools: ChatCompletionTool[], signal: AbortSignal | undefined, onDebug?: (message: string) => void) {
  const client = await getClient();
  const request = { messages: history, tools, tool_choice: "auto" as const, parallel_tool_calls: true };
  try {
    const response = await client.chat.completions.create({ model, ...request }, { signal });
    if (response.choices?.[0]?.message) return response;
    onDebug?.(`[agent] ${model} returned an empty response: ${responseSummary(response)}`);
  } catch (error) {
    onDebug?.(`[agent] ${model} failed: ${errorText(error)}`);
    if (!isRetryable(error)) throw error;
  }
  if (model === "openrouter/free") throw new Error("OpenRouter returned no usable message. Try again or select another model.");
  onDebug?.(`[agent] Retrying with OpenRouter's free model router.`);
  const fallback = await client.chat.completions.create({ model: "openrouter/free", ...request }, { signal });
  if (!fallback.choices?.[0]?.message) throw new Error(`OpenRouter returned no usable message (${responseSummary(fallback)}).`);
  return fallback;
}

export async function runAgent(userText: string, options: RunAgentOptions): Promise<AgentResult> {
  const { settings, confirm, onActivity, onDebug, signal } = options;
  await getClient(); 
  if (signal?.aborted) throw new DOMException("Cancelled", "AbortError");
  let rag: RagHit[] = [];
  let profile: ProfileItem[] = [];
  if (settings.useRag) {
    try { rag = await invoke<RagHit[]>("rag_search", { query: userText, limit: 4 }); } catch { rag = []; }
  }
  try { profile = await invoke<ProfileItem[]>("profile_get"); } catch { profile = []; }
  const history: AgentMessage[] = [{ role: "system", content: systemPrompt(rag, profile) }, ...options.messages, { role: "user", content: userText }];
  const tools = settings.allowWeb ? TOOL_DEFINITIONS : TOOL_DEFINITIONS.filter((tool) => tool.function.name !== "search_web");

  for (let round = 0; round < 8; round += 1) {
    if (signal?.aborted) throw new DOMException("Cancelled", "AbortError");
    const response = await completion(settings.model, history, tools as ChatCompletionTool[], signal, onDebug);
    const message = response.choices?.[0]?.message;
    if (!message) throw new Error("OpenRouter returned no message.");
    history.push(message);
    const calls = message.tool_calls ?? [];
    if (!calls.length) {
      const reply = textFrom(message.content).replace(/\*/g, "").trim() || "I couldn't generate a response.";
      return { reply, messages: history.slice(1) };
    }

    const prepared = calls.map((call) => {
      const startedAt = Date.now();
      const rawName = call.type === "function" ? call.function.name : "unknown";
      try {
        const args = JSON.parse(call.type === "function" ? call.function.arguments : "{}") as Record<string, unknown>;
        if (!isToolName(rawName)) return { call, error: `Tool error: unknown tool '${rawName}'.` };
        const protectedAction = requiresConfirmation(rawName, args);
        const needsConfirmation = protectedAction && !settings.autoApprove;
        const activity: ToolActivity = { id: `${call.id}-${startedAt}`, callId: call.id, name: rawName, args, protected: protectedAction, status: needsConfirmation ? "waiting" : "running", startedAt };
        onActivity?.(activity);
        return { call, name: rawName, args, activity };
      } catch (error) {
        return { call, error: `Tool error: malformed JSON arguments (${errorText(error)}).` };
      }
    });
    const execute = async (item: typeof prepared[number]) => {
      if (item.error || !item.name || !item.args || !item.activity) return { id: item.call.id, content: item.error ?? "Tool error." };
      const { name, args, activity } = item;
      try {
        if (requiresConfirmation(name, args) && !settings.autoApprove) {
          const approved = await confirm({ id: item.call.id, tool: name, args, summary: confirmationSummary(name, args) });
          if (!approved) {
            onActivity?.({ ...activity, status: "denied", message: "Denied by user", finishedAt: Date.now() });
            return { id: item.call.id, content: "The user denied this action. Do not retry it unless they explicitly ask." };
          }
        }
        onActivity?.({ ...activity, status: "running" });
        const result = await executeTool(name, args, signal);
        onActivity?.({ ...activity, status: "success", message: "Completed", finishedAt: Date.now() });
        return { id: item.call.id, content: resultText(result) };
      } catch (error) {
        const message = errorText(error);
        onActivity?.({ ...activity, status: "error", message, finishedAt: Date.now() });
        return { id: item.call.id, content: `Tool error: ${message}` };
      }
    };
    const outcomes = new Map<string, { id: string; content: string }>();
    const safe = prepared.filter((item) => !item.name || !item.args || !requiresConfirmation(item.name, item.args) || settings.autoApprove);
    (await Promise.all(safe.map(execute))).forEach((outcome) => outcomes.set(outcome.id, outcome));
    for (const item of prepared.filter((candidate) => candidate.name && candidate.args && requiresConfirmation(candidate.name, candidate.args) && !settings.autoApprove)) {
      const outcome = await execute(item);
      outcomes.set(outcome.id, outcome);
    }
    for (const call of calls) {
      const outcome = outcomes.get(call.id) ?? { id: call.id, content: "Tool error: no result." };
      history.push({ role: "tool", tool_call_id: outcome.id, content: outcome.content });
    }
  }
  throw new Error("The assistant reached its tool-call limit. Try a narrower request.");
}
