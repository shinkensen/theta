import { useCallback, useEffect, useState, type FormEvent } from "react";
import { spotify, type IntegrationStatus } from "../../integrations/spotify";
import { CredentialField, IntegrationCard } from "./IntegrationCard";

export function SpotifySettingsCard() {
  const [status, setStatus] = useState<IntegrationStatus | null>(null);
  const [clientId, setClientId] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const refresh = useCallback(async () => {
    setLoading(true); setError("");
    try { setStatus(await spotify.status()); } catch (reason) { setError(String(reason)); }
    finally { setLoading(false); }
  }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  const save = async (event: FormEvent) => {
    event.preventDefault(); setError(""); setMessage("");
    try { await spotify.saveClientId(clientId); setClientId(""); setMessage("Client ID saved locally."); await refresh(); }
    catch (reason) { setError(String(reason)); }
  };
  const connect = async () => {
    setError(""); setMessage("Waiting for Spotify sign-in in your browser…");
    try { const account = await spotify.connect(); setMessage(`Connected as ${account}.`); await refresh(); }
    catch (reason) { setMessage(""); setError(String(reason)); }
  };
  const disconnect = async () => {
    if (!window.confirm("Disconnect Spotify from Theta?")) return;
    try { await spotify.disconnect(); setMessage("Spotify disconnected."); await refresh(); }
    catch (reason) { setError(String(reason)); }
  };
  const copyRedirect = () => void navigator.clipboard.writeText(status?.redirectUri ?? "http://127.0.0.1:8754/callback");
  return <IntegrationCard name="Spotify" category="MUSIC" description="Control the active Spotify player from Theta. Spotify Premium and an active device may be required." connected={status?.connected} configured={status?.configured} loading={loading} className="spotify-card" error={error} message={message} accountLabel={status?.accountLabel} onSubmit={save}>
    <label>OAuth redirect URI<div className="inline-field"><input readOnly value={status?.redirectUri ?? "http://127.0.0.1:8754/callback"} /><button type="button" onClick={copyRedirect}>Copy</button></div></label>
    <CredentialField label="Spotify Client ID" value={clientId} onChange={setClientId} placeholder={status?.configured ? "Enter a new ID to replace it" : "Paste Client ID"} required={!status?.configured} />
    <div className="button-row"><button disabled={loading || !clientId.trim()}>Save Client ID</button><button type="button" className="primary" disabled={loading || !status?.configured} onClick={() => void connect()}>Connect</button>{status?.connected && <button type="button" className="danger" onClick={() => void disconnect()}>Disconnect</button>}</div>
  </IntegrationCard>;
}
