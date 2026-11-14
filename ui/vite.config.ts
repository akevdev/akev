import { join } from 'node:path';
import { defineConfig, loadEnv, type Plugin, type ProxyOptions } from 'vite';
import react from '@vitejs/plugin-react';
import { collectLicenses } from './scripts/licenses';

/** `virtual:licenses`: the bundled packages' licenses, read from node_modules at build time. */
function licenses(): Plugin {
  const id = 'virtual:licenses';
  return {
    name: 'akev-licenses',
    resolveId: (source) => (source === id ? `\0${id}` : null),
    load: (resolved) => (resolved === `\0${id}` ? `export default ${JSON.stringify(collectLicenses(import.meta.dirname))};` : null),
  };
}

/**
 * Akev UI build configuration.
 *
 * - `base: './'` keeps every asset URL relative so the same `dist/` works when embedded in the
 *   Rust daemon (served from `/`) and inside the Tauri webview (`tauri://localhost`).
 * - The dev server reads `VITE_` settings from the repository's `.env` (see `.env.example`).
 *   Builds don't read it, so a development setting like `VITE_MOCK=1` never ends up in one;
 *   a build still takes `VITE_` variables from the environment (the visual tests' mock build).
 * - In dev, `/api` and `/mcp` (including the `/api/stream` WebSocket) are proxied to the daemon
 *   at `http://127.0.0.1:45456` unless `VITE_AKEV_URL` points the client somewhere else.
 *   The proxy rewrites `Origin` to the daemon's own origin so the API's drive-by protection
 *   accepts dev-server requests without `--allow-origin`.
 * - `virtual:licenses` lists the bundled open-source packages (Settings → About).
 */
export default defineConfig(({ command, mode }) => {
  const envDir = command === 'serve' ? join(import.meta.dirname, '..') : false;
  const env = loadEnv(mode, envDir, 'VITE_');
  const target = 'http://127.0.0.1:45456';
  const proxyEntry = (ws: boolean): ProxyOptions => ({
    target,
    ws,
    changeOrigin: true,
    configure(proxy) {
      proxy.on('proxyReq', (req) => {
        if (req.getHeader('origin')) req.setHeader('origin', target);
      });
      proxy.on('proxyReqWs', (req) => {
        if (req.getHeader('origin')) req.setHeader('origin', target);
      });
    },
  });

  return {
    base: './',
    envDir,
    plugins: [react(), licenses()],
    server: {
      port: 5173,
      strictPort: false,
      proxy: env.VITE_AKEV_URL
        ? undefined
        : {
            '/api': proxyEntry(true),
            '/mcp': proxyEntry(false),
          },
    },
    preview: { port: 4173 },
    build: {
      outDir: 'dist',
      emptyOutDir: true,
      target: 'es2022',
      sourcemap: false,
      assetsInlineLimit: 4096,
    },
  };
});
