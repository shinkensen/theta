import { invoke } from "@tauri-apps/api/core";
import type { IntegrationStatus } from "./types";

export const hackatime = {
  status: () => invoke<IntegrationStatus>("hackatime_status"),
  saveClientId: (clientId: string) => invoke<void>("hackatime_set_client_id", { clientId }),
  connect: () => invoke<string>("hackatime_connect"),
  disconnect: () => invoke<void>("hackatime_disconnect"),
  profile: () => invoke<unknown>("hackatime_get_profile"),
  hours: (startDate?: string, endDate?: string) => invoke<unknown>("hackatime_get_hours", { startDate, endDate }),
  streak: () => invoke<unknown>("hackatime_get_streak"),
  projects: (includeArchived = false) => invoke<unknown>("hackatime_list_projects", { includeArchived }),
  latestHeartbeat: () => invoke<unknown>("hackatime_latest_heartbeat"),
};
