import { useCallback, useEffect, useState } from "react";
import { spotify, type IntegrationStatus, type SpotifyDevice, type SpotifyPlayback } from "../../integrations/spotify";

function clock(milliseconds: number): string {
  const seconds = Math.max(0, Math.floor(milliseconds / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

export function SpotifyPlayer() {
  const [status, setStatus] = useState<IntegrationStatus | null>(null);
  const [playback, setPlayback] = useState<SpotifyPlayback | null>(null);
  const [devices, setDevices] = useState<SpotifyDevice[]>([]);
  const [deviceId, setDeviceId] = useState("");
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(async () => {
    try {
      const nextStatus = await spotify.status(); setStatus(nextStatus);
      if (!nextStatus.connected) { setPlayback(null); setDevices([]); return; }
      const [nextPlayback, nextDevices] = await Promise.all([spotify.playback(), spotify.devices()]);
      setPlayback(nextPlayback); setDevices(nextDevices);
      setDeviceId((current) => current || nextPlayback.device?.id || nextDevices.find((device) => device.isActive)?.id || nextDevices[0]?.id || "");
      setError("");
    } catch (reason) { setError(String(reason)); }
  }, []);
  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 5000);
    return () => window.clearInterval(timer);
  }, [refresh]);
  const act = async (action: () => Promise<void>) => {
    setBusy(true); setError("");
    try { await action(); await new Promise((resolve) => window.setTimeout(resolve, 250)); await refresh(); }
    catch (reason) { setError(String(reason)); }
    finally { setBusy(false); }
  };
  const duration = playback?.track?.durationMs ?? 0;
  const volume = playback?.device?.volumePercent ?? devices.find((device) => device.id === deviceId)?.volumePercent ?? 50;
  if (status && !status.connected) return <section className="panel"><div className="panel-head"><div><h1>Spotify</h1><p>Playback control from Theta</p></div></div><div className="empty">Connect Spotify from Settings to control your music.</div></section>;
  return <section className="panel spotify-panel"><div className="panel-head"><div><h1>Spotify</h1><p>{status?.accountLabel ? `Connected as ${status.accountLabel}` : "Playback control from Theta"}</p></div><button onClick={() => void refresh()}>Refresh</button></div>
    {error && <div className="notice error" role="alert">{error}</div>}
    <div className="spotify-player card">
      {playback?.track?.artworkUrl ? <img className="spotify-art" src={playback.track.artworkUrl} alt={`${playback.track.album} cover`} /> : <div className="spotify-art placeholder" aria-hidden="true">♫</div>}
      <div className="spotify-now"><span className="card-kicker">{playback?.active ? "NOW PLAYING" : "PLAYER IDLE"}</span><h2>{playback?.track?.name ?? "Nothing playing"}</h2><p>{playback?.track ? `${playback.track.artists.join(", ")} · ${playback.track.album}` : "Start playback in Spotify, then refresh."}</p>
        <label className="spotify-device">Device<select value={deviceId} onChange={(event) => setDeviceId(event.target.value)}>{devices.length ? devices.map((device) => <option key={device.id} value={device.id}>{device.name} · {device.deviceType}{device.isRestricted ? " (restricted)" : ""}</option>) : <option value="">No devices found</option>}</select></label>
      </div>
      <div className="spotify-controls">
        <div className="transport"><button disabled={busy || !deviceId} aria-label="Previous track" onClick={() => void act(() => spotify.previous(deviceId))}>‹‹</button><button className="primary play-button" disabled={busy || !deviceId} aria-label={playback?.isPlaying ? "Pause" : "Play"} onClick={() => void act(() => playback?.isPlaying ? spotify.pause(deviceId) : spotify.play(deviceId))}>{playback?.isPlaying ? "Ⅱ" : "▶"}</button><button disabled={busy || !deviceId} aria-label="Next track" onClick={() => void act(() => spotify.next(deviceId))}>››</button></div>
        <label className="range-row"><span>{clock(playback?.progressMs ?? 0)}</span><input type="range" min="0" max={Math.max(1, duration)} value={Math.min(playback?.progressMs ?? 0, duration)} disabled={busy || !duration} aria-label="Playback position" onChange={(event) => setPlayback((current) => current ? { ...current, progressMs: Number(event.target.value) } : current)} onMouseUp={(event) => void act(() => spotify.seek(Number(event.currentTarget.value), deviceId))} onKeyUp={(event) => { if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) void act(() => spotify.seek(Number(event.currentTarget.value), deviceId)); }} /><span>{clock(duration)}</span></label>
        <label className="volume-row"><span>Volume</span><input type="range" min="0" max="100" defaultValue={volume} disabled={busy || !deviceId} onChange={(event) => setPlayback((current) => current?.device ? { ...current, device: { ...current.device, volumePercent: Number(event.target.value) } } : current)} onMouseUp={(event) => void act(() => spotify.volume(Number(event.currentTarget.value), deviceId))} /><output>{volume}%</output></label>
      </div>
    </div><p className="muted spotify-footnote" aria-live="polite">Controls target the selected device. Spotify Premium and an active Spotify app may be required.</p>
  </section>;
}
