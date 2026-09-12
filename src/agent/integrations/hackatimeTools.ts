import { hackatime } from "../../integrations/hackatime";
import { registerIntegrationTools, type IntegrationToolModule } from "../integrationRegistry";

export type HackatimeToolName = "hackatime_get_profile" | "hackatime_get_hours" | "hackatime_get_streak" | "hackatime_list_projects" | "hackatime_latest_heartbeat";
const object = (properties: Record<string, unknown> = {}) => ({ type: "object", properties, additionalProperties: false });
const definition = (name: HackatimeToolName, description: string, parameters = object()) => ({ type: "function" as const, function: { name, description, parameters } });
const tools: IntegrationToolModule<HackatimeToolName> = {
  definitions: [
    definition("hackatime_get_profile", "Read the connected Hackatime profile."),
    definition("hackatime_get_hours", "Read total coding time for an optional date range.", object({ startDate: { type: "string", description: "Start date YYYY-MM-DD" }, endDate: { type: "string", description: "End date YYYY-MM-DD" } })),
    definition("hackatime_get_streak", "Read the current Hackatime coding streak."),
    definition("hackatime_list_projects", "List Hackatime projects and tracked time.", object({ includeArchived: { type: "boolean" } })),
    definition("hackatime_latest_heartbeat", "Read the latest Hackatime coding heartbeat."),
  ],
  async execute(name, args) {
    switch (name) {
      case "hackatime_get_profile": return hackatime.profile();
      case "hackatime_get_hours": return hackatime.hours(typeof args.startDate === "string" ? args.startDate : undefined, typeof args.endDate === "string" ? args.endDate : undefined);
      case "hackatime_get_streak": return hackatime.streak();
      case "hackatime_list_projects": return hackatime.projects(args.includeArchived === true);
      case "hackatime_latest_heartbeat": return hackatime.latestHeartbeat();
    }
  },
};
registerIntegrationTools(tools);
