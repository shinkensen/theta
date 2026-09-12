import type { ReactNode } from "react";

export interface IntegrationStatus {
  id: string;
  name: string;
  configured: boolean;
  connected: boolean;
  accountLabel?: string;
  redirectUri?: string;
  message?: string;
}

export interface IntegrationCardDescriptor {
  id: string;
  name: string;
  category: string;
  description: string;
  component: () => ReactNode;
}
