/// <reference types="vite/client" />

/// Keys read from `.env` at build time. Declaring them here means a typo in an
/// `import.meta.env.VITE_…` name is a compile error rather than `undefined` at
/// runtime.
interface ImportMetaEnv {
  readonly VITE_OPENROUTER_API_KEY: string;
  readonly VITE_FIRECRAWL_KEY: string;
  readonly VITE_EDGE_TRUSTED_TOKEN: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
