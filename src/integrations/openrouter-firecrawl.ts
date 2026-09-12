import { invoke } from "@tauri-apps/api/core";
import type { IntegrationStatus } from "./types";

export const openrouter = {
  status: () => invoke<IntegrationStatus>("openrouter_status"),
  saveKey: (apiKey: string) => invoke<void>("openrouter_save_key", { apiKey }),
  getKey: () => invoke<string>("openrouter_get_key"),
  clearKey: () => invoke<void>("openrouter_clear_key"),
};

export const firecrawl = {
  status: () => invoke<IntegrationStatus>("firecrawl_status"),
  saveKey: (apiKey: string) => invoke<void>("firecrawl_save_key", { apiKey }),
  getKey: () => invoke<string>("firecrawl_get_key"),
  clearKey: () => invoke<void>("firecrawl_clear_key"),
};
