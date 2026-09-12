import { useCallback, useEffect, useState, type FormEvent } from "react";
import { firecrawl } from "../../integrations/openrouter-firecrawl";
import type { IntegrationStatus } from "../../integrations/types";
import { CredentialField, IntegrationCard } from "./IntegrationCard";

export function FirecrawlSettingsCard() {
  const [status, setStatus] = useState<IntegrationStatus | null>(null);
  const [key, setKey] = useState("");
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(true);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      setStatus(await firecrawl.status());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const save = async (e: FormEvent) => {
    e.preventDefault();
    setError("");
    try {
      await firecrawl.saveKey(key);
      setKey("");
      setMessage("Firecrawl API key saved successfully.");
      await refresh();
    } catch (r) {
      setError(String(r));
    }
  };

  return (
    <IntegrationCard
      name="Firecrawl"
      category="RESEARCH"
      description="Optional. Enables web search and scraping for agent research."
      configured={status?.configured}
      connected={status?.connected}
      loading={loading}
      error={error}
      message={message}
      onSubmit={save}
    >
      <CredentialField
        secret
        label="Firecrawl API key"
        value={key}
        onChange={setKey}
        placeholder={status?.configured ? "Enter a new key to replace it" : "Paste API key"}
        required={!status?.configured}
        help="Get your API key from firecrawl.dev"
      />
      <div className="button-row">
        <button disabled={loading || !key.trim()}>Save key</button>
      </div>
    </IntegrationCard>
  );
}
