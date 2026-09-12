import { invoke } from "@tauri-apps/api/core";
import type { IntegrationStatus } from "./types";

export type { IntegrationStatus } from "./types";

export interface SpotifyDevice {
  id: string;
  name: string;
  deviceType: string;
  isActive: boolean;
  isRestricted: boolean;
  volumePercent?: number;
}

export interface SpotifyTrack {
  uri: string;
  name: string;
  artists: string[];
  album: string;
  artworkUrl?: string;
  durationMs: number;
}

export interface SpotifyPlayback {
  active: boolean;
  isPlaying: boolean;
  progressMs: number;
  repeatState?: string;
  shuffleState?: boolean;
  device?: SpotifyDevice;
  track?: SpotifyTrack;
}

export const spotify = {
  status: () => invoke<IntegrationStatus>("spotify_status"),
  saveClientId: (clientId: string) => invoke<void>("spotify_set_client_id", { clientId }),
  connect: () => invoke<string>("spotify_connect"),
  disconnect: () => invoke<void>("spotify_disconnect"),
  playback: () => invoke<SpotifyPlayback>("spotify_get_playback"),
  devices: () => invoke<SpotifyDevice[]>("spotify_list_devices"),
  play: (deviceId?: string) => invoke<void>("spotify_play", { deviceId }),
  pause: (deviceId?: string) => invoke<void>("spotify_pause", { deviceId }),
  next: (deviceId?: string) => invoke<void>("spotify_next", { deviceId }),
  previous: (deviceId?: string) => invoke<void>("spotify_previous", { deviceId }),
  seek: (positionMs: number, deviceId?: string) => invoke<void>("spotify_seek", { positionMs, deviceId }),
  volume: (volumePercent: number, deviceId?: string) => invoke<void>("spotify_set_volume", { volumePercent, deviceId }),
};
