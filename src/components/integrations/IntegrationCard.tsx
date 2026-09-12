import type { FormEventHandler, ReactNode } from "react";

interface IntegrationCardProps {
  name: string;
  category: string;
  description: string;
  connected?: boolean;
  configured?: boolean;
  loading?: boolean;
  className?: string;
  children: ReactNode;
  error?: string;
  message?: string;
  accountLabel?: string;
  onSubmit?: FormEventHandler<HTMLFormElement>;
}

export function IntegrationCard({
  name,
  category,
  description,
  connected = false,
  configured = false,
  loading = false,
  className = "",
  children,
  error,
  message,
  accountLabel,
  onSubmit,
}: IntegrationCardProps) {
  const state = loading ? "Checking…" : connected ? "Connected" : configured ? "Ready" : "Not connected";
  return <form className={`card form-stack integration-card ${className}`.trim()} onSubmit={onSubmit} aria-busy={loading}>
    <div className="integration-head">
      <div><span className="card-kicker">{category}</span><h2>{name}</h2></div>
      <span className={`connection-badge ${connected ? "connected" : ""}`}>{state}</span>
    </div>
    <p className="muted">{description}</p>
    {children}
    {accountLabel && <p className="muted">Signed in as {accountLabel}.</p>}
    {error && <p className="integration-error" role="alert">{error}</p>}
    {message && !error && <p className="integration-message" role="status">{message}</p>}
  </form>;
}

interface CredentialFieldProps {
  label: string;
  value: string;
  onChange(value: string): void;
  placeholder?: string;
  required?: boolean;
  secret?: boolean;
  help?: string;
}

export function CredentialField({ label, value, onChange, placeholder, required, secret = false, help }: CredentialFieldProps) {
  return <label>{label}<input type={secret ? "password" : "text"} value={value} onChange={(event) => onChange(event.target.value)} placeholder={placeholder} autoComplete={secret ? "new-password" : "off"} required={required} />{help && <small>{help}</small>}</label>;
}
