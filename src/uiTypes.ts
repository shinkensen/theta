export type { Settings, SpeechProvider, SpeechProviderCapability, CalEvent, CalendarSummary, ProcessInfo, SystemStats, PortInfo, RagDoc, ProfileItem, ProfileCategory, CanvasProfile, CanvasCourse, CanvasSubmission, CanvasAssignmentDate, CanvasAssignment, CanvasDueDate, CanvasModule, CanvasModuleItem } from "./agent/tools";
export interface AuthStatus { connected: boolean; has_credentials: boolean; email?: string }

export interface ConversationMessage {
  id: string;
  role: string;
  text: string;
  timestamp: number;
}

export interface Conversation {
  id: string;
  title: string;
  created: number;
  updated: number;
  messages: ConversationMessage[];
}

export interface ConversationSummary {
  id: string;
  title: string;
  created: number;
  updated: number;
  messageCount: number;
}
