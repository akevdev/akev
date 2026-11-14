/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Absolute daemon URL for dev against a remote/non-default daemon (disables the dev proxy). */
  readonly VITE_AKEV_URL?: string;
  /** `1` = start with the in-browser mock backend. */
  readonly VITE_MOCK?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

/** Open-source packages bundled into the UI (generated from node_modules by vite.config.ts). */
declare module 'virtual:licenses' {
  const licenses: readonly { name: string; license: string; packages: number }[];
  export default licenses;
}
