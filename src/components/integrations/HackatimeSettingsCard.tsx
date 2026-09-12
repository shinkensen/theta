import { useCallback, useEffect, useState, type FormEvent } from "react";
import { hackatime } from "../../integrations/hackatime";
import type { IntegrationStatus } from "../../integrations/types";
import { CredentialField, IntegrationCard } from "./IntegrationCard";

export function HackatimeSettingsCard() {
  const [status, setStatus] = useState<IntegrationStatus | null>(null);
  const [clientId, setClientId] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);
  const refresh = useCallback(async () => { setLoading(true); setError(""); try { setStatus(await hackatime.status()); } catch (reason) { setError(String(reason)); } finally { setLoading(false); } }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  const save = async (event: FormEvent) => { event.preventDefault(); setError(""); try { await hackatime.saveClientId(clientId); setClientId(""); setMessage("Client ID saved locally."); await refresh(); } catch (reason) { setError(String(reason)); } };
  const connect = async () => { setError(""); setMessage("Waiting for Hackatime sign-in in your browser…"); try { const label = await hackatime.connect(); setMessage(`Connected as ${label}.`); await refresh(); } catch (reason) { setMessage(""); setError(String(reason)); } };
  const disconnect = async () => { if (!window.confirm("Disconnect Hackatime from Theta?")) return; try { await hackatime.disconnect(); setMessage("Hackatime disconnected."); await refresh(); } catch (reason) { setError(String(reason)); } };
  return <IntegrationCard name="Hackatime" category="CODING TIME" description="Read coding hours, project totals, streaks, and your latest heartbeat." configured={status?.configured} connected={status?.connected} loading={loading} error={error} message={message} accountLabel={status?.accountLabel} onSubmit={save}>
    <label>OAuth redirect URI<input readOnly value={status?.redirectUri ?? "http://127.0.0.1/callback"} /><small>Register this loopback URI in your Hackatime OAuth app.</small></label>
    <CredentialField label="Hackatime Client ID" value={clientId} onChange={setClientId} placeholder={status?.configured ? "Enter a new ID to replace it" : "Paste Client ID"} required={!status?.configured} />
    <div className="button-row"><button disabled={loading || !clientId.trim()}>Save Client ID</button><button type="button" className="primary" disabled={loading || !status?.configured} onClick={() => void connect()}>Connect</button>{status?.connected && <button type="button" className="danger" onClick={() => void disconnect()}>Disconnect</button>}</div>
  </IntegrationCard>;
}
