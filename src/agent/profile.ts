import OpenAI from "openai";
import { invoke } from "@tauri-apps/api/core";
import type { ProfileItem, ProfileOperation } from "./tools";

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

const CATEGORIES = new Set(["interest", "hobby", "project", "preference", "recurring_topic"]);

function parseOperations(raw: string): ProfileOperation[] {
  const match = raw.match(/\[[\s\S]*\]/);
  if (!match) return [];
  let values: unknown;
  try { values = JSON.parse(match[0]); } catch { return []; }
  if (!Array.isArray(values)) return [];
  return values.slice(0, 8).flatMap((value) => {
    if (!value || typeof value !== "object") return [];
    const row = value as Record<string, unknown>;
    if (!CATEGORIES.has(String(row.category)) || typeof row.text !== "string" || !row.text.trim()) return [];
    return [{ category: String(row.category) as ProfileOperation["category"], text: row.text.trim().slice(0, 180), explicit: row.explicit === true, replaceId: typeof row.replaceId === "string" ? row.replaceId : undefined }];
  });
}

export async function learnProfile(userText: string, assistantReply: string, current: ProfileItem[], model: string, signal?: AbortSignal): Promise<ProfileItem[]> {
  const client = await getClient();
  const prompt = `Update a user's evolving profile from one conversation turn. Return ONLY a JSON array (maximum 8 objects) with category, text, explicit, and optional replaceId.
Categories: interest, hobby, project, preference, recurring_topic.
Explicit is true only when the user directly states the fact. A question alone may create a low-confidence recurring_topic, not an interest. Use replaceId only for a clearly contradicted existing item. Never infer or save credentials, exact addresses, health, religion, politics, sexuality, finances, or other sensitive traits. Keep each text short, third-person, and timeless. Return [] when nothing useful is learned.
Current profile: ${JSON.stringify(current)}
User: ${userText}
Assistant: ${assistantReply}`;
  const response = await client.chat.completions.create({ model, messages: [{ role: "user", content: prompt }], temperature: 0.1 }, { signal });
  const content = response.choices?.[0]?.message?.content;
  const raw = typeof content === "string" ? content : "";
  const operations = parseOperations(raw);
  if (!operations.length) return current;
  return invoke<ProfileItem[]>("profile_apply", { operations });
}

export { parseOperations };
