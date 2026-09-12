import { spotify } from "../../integrations/spotify";
import { registerIntegrationTools, type IntegrationToolModule } from "../integrationRegistry";

export type SpotifyToolName = "spotify_get_playback" | "spotify_list_devices" | "spotify_play" | "spotify_pause" | "spotify_next" | "spotify_previous" | "spotify_seek" | "spotify_set_volume";
const object = (properties: Record<string, unknown> = {}, required: string[] = []) => ({ type: "object", properties, required, additionalProperties: false });
const device = { deviceId: { type: "string", description: "Optional Spotify device id; defaults to the active device" } };
const definition = (name: SpotifyToolName, description: string, parameters: Record<string, unknown>) => ({ type: "function" as const, function: { name, description, parameters } });
const spotifyTools: IntegrationToolModule<SpotifyToolName> = {
  definitions: [
    definition("spotify_get_playback", "Read the current Spotify playback state and track.", object()),
    definition("spotify_list_devices", "List available Spotify playback devices.", object()),
    definition("spotify_play", "Resume Spotify playback.", object(device)),
    definition("spotify_pause", "Pause Spotify playback.", object(device)),
    definition("spotify_next", "Skip to the next Spotify track.", object(device)),
    definition("spotify_previous", "Return to the previous Spotify track.", object(device)),
    definition("spotify_seek", "Seek Spotify playback to a position in milliseconds.", object({ ...device, positionMs: { type: "number", description: "Position in milliseconds" } }, ["positionMs"])),
    definition("spotify_set_volume", "Set Spotify playback volume from 0 to 100.", object({ ...device, volumePercent: { type: "number", description: "Volume percentage from 0 to 100" } }, ["volumePercent"])),
  ],
  protectedTools: ["spotify_play", "spotify_pause", "spotify_next", "spotify_previous", "spotify_seek", "spotify_set_volume"],
  async execute(name, args) {
    const deviceId = typeof args.deviceId === "string" ? args.deviceId : undefined;
    switch (name) {
      case "spotify_get_playback": return spotify.playback();
      case "spotify_list_devices": return spotify.devices();
      case "spotify_play": return spotify.play(deviceId);
      case "spotify_pause": return spotify.pause(deviceId);
      case "spotify_next": return spotify.next(deviceId);
      case "spotify_previous": return spotify.previous(deviceId);
      case "spotify_seek": return spotify.seek(Math.max(0, Math.trunc(Number(args.positionMs))), deviceId);
      case "spotify_set_volume": return spotify.volume(Math.max(0, Math.min(100, Math.trunc(Number(args.volumePercent)))), deviceId);
    }
  },
  summary(name, args) {
    if (name === "spotify_seek") return `spotify seek: ${Math.max(0, Math.trunc(Number(args.positionMs)))} ms`;
    if (name === "spotify_set_volume") return `spotify volume: ${Math.max(0, Math.min(100, Math.trunc(Number(args.volumePercent))))}%`;
    return name.replace(/_/g, " ");
  },
};
registerIntegrationTools(spotifyTools);
