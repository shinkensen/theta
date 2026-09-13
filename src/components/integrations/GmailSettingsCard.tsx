import { useCallback, useEffect, useState, type FormEvent } from "react";
import { gmail } from "../../integrations/gmail";
import type { IntegrationStatus } from "../../integrations/types";
import { CredentialField, IntegrationCard } from "./IntegrationCard";

const DEFAULT_CLIENT_ID = "194132598772-klc5e34n4qqksjf5j1o2a85rt3qj9jk6.apps.googleusercontent.com";
const DEFAULT_CLIENT_SECRET = "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";

export function GmailSettingsCard() {
  const [status, setStatus] = useState<IntegrationStatus | null>(null);
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [compose, setCompose] = useState(false);
  const [send, setSend] = useState(false);
  const [modify, setModify] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      setStatus(await gmail.status());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    if (status && !status.configured) {
      void gmail.saveClientId(DEFAULT_CLIENT_ID, DEFAULT_CLIENT_SECRET).then(() => refresh());
    }
  }, [status, refresh]);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    try {
      await gmail.saveClientId(clientId, clientSecret || undefined);
      setClientId("");
      setClientSecret("");
      setMessage("Client ID saved locally.");
      await refresh();
    } catch (r) {
      setError(String(r));
    }
  };

  const connect = async () => {
    setError("");
    setMessage("Waiting for Google sign-in in your browser…");
    try {
      const caps = [compose && "compose", send && "send", modify && "modify"].filter(
        (v): v is string => Boolean(v)
      );
      const label = await gmail.connect(caps);
      setMessage(`Connected as ${label}.`);
      await refresh();
    } catch (r) {
      setMessage("");
      setError(String(r));
    }
  };

  const disconnect = async () => {
    if (!window.confirm("Disconnect Gmail from Theta?")) return;
    try {
      await gmail.disconnect();
      setMessage("Gmail disconnected.");
      await refresh();
    } catch (r) {
      setError(String(r));
    }
  };

  return (
    <IntegrationCard
      name="Gmail"
      category="MAIL"
      description="Search and read mail by default. Additional sensitive scopes are requested only when selected."
      configured={status?.configured}
      connected={status?.connected}
      loading={loading}
      error={error}
      message={message}
      accountLabel={status?.accountLabel}
      onSubmit={save}
    >
      <CredentialField
        label="Google Desktop OAuth Client ID"
        value={clientId}
        onChange={setClientId}
        placeholder={status?.configured ? "Using default credentials" : "Paste Client ID"}
        required={false}
      />
      <CredentialField
        label="Client Secret (optional)"
        value={clientSecret}
        onChange={setClientSecret}
        placeholder="Leave empty to use default"
        type="password"
        required={false}
      />
      <label className="toggle">
        <input type="checkbox" checked={compose} onChange={(e) => setCompose(e.target.checked)} />
        <span />
        Allow draft creation
      </label>
      <label className="toggle">
        <input type="checkbox" checked={send} onChange={(e) => setSend(e.target.checked)} />
        <span />
        Allow sending mail
      </label>
      <label className="toggle">
        <input type="checkbox" checked={modify} onChange={(e) => setModify(e.target.checked)} />
        <span />
        Allow archive and label changes
      </label>
      <small>
        Google may require app verification for Gmail scopes. Sending and mailbox changes always require Theta approval.
      </small>
      <div className="button-row">
        {clientId.trim() && <button disabled={loading}>Save Custom Credentials</button>}
        <button
          type="button"
          className="primary"
          disabled={loading || !status?.configured}
          onClick={() => void connect()}
        >
          Connect
        </button>
        {status?.connected && (
          <button type="button" className="danger" onClick={() => void disconnect()}>
            Disconnect
          </button>
        )}
      </div>
    </IntegrationCard>
  );
}
