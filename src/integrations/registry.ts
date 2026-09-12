import type { ComponentType } from "react";
import { GmailSettingsCard } from "../components/integrations/GmailSettingsCard";
import { GitHubSettingsCard } from "../components/integrations/GitHubSettingsCard";
import { HackatimeSettingsCard } from "../components/integrations/HackatimeSettingsCard";
import { MineStratorSettingsCard } from "../components/integrations/MineStratorSettingsCard";
import { NotionSettingsCard } from "../components/integrations/NotionSettingsCard";
import { SpotifySettingsCard } from "../components/integrations/SpotifySettingsCard";

export interface IntegrationCardRegistration {
  id: string;
  component: ComponentType;
}

const cards: IntegrationCardRegistration[] = [
  { id: "spotify", component: SpotifySettingsCard },
  { id: "hackatime", component: HackatimeSettingsCard },
  { id: "github", component: GitHubSettingsCard },
  { id: "gmail", component: GmailSettingsCard },
  { id: "notion", component: NotionSettingsCard },
  { id: "minestrator", component: MineStratorSettingsCard },
];

export function registerIntegrationCard(registration: IntegrationCardRegistration): void {
  if (cards.some(({ id }) => id === registration.id)) throw new Error(`Duplicate integration card: ${registration.id}`);
  cards.push(registration);
}

export function integrationCards(): readonly IntegrationCardRegistration[] {
  return cards;
}
