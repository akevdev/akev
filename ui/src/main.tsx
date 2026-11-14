/**
 * Bootstrap: fonts & styles → auth/base URL → (optional) mock backend → stream → render.
 */
import './styles/fonts';
import './styles/tokens.css';
import './styles/global.css';
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { checkStoredToken, resolveBrowserAuth } from './api/auth';
import { api, isApiError } from './api/client';
import { App } from './app/App';
import { useAppStore } from './state/app';
import { installExternalLinkHandling } from './lib/platform';
import { startConnection } from './state/connection';

/**
 * The in-browser mock backend ships only in dev builds (`bun run dev`) and in builds made with
 * `VITE_MOCK=1`. Both are compile-time constants, so production builds drop the mock chunk.
 */
const MOCK_BUILD = import.meta.env.DEV || import.meta.env.VITE_MOCK === '1';

/** `?mock`, `?mock=1` or `#/route?mock` enable the mock backend in a build that has it;
 *  a `VITE_MOCK=1` build always uses it. */
function mockRequested(): boolean {
  if (import.meta.env.VITE_MOCK === '1') return true;
  const flag = (params: URLSearchParams) => params.has('mock') && params.get('mock') !== '0' && params.get('mock') !== 'false';
  if (flag(new URLSearchParams(window.location.search))) return true;
  const qi = window.location.hash.indexOf('?');
  return qi >= 0 && flag(new URLSearchParams(window.location.hash.slice(qi + 1)));
}

/** Whether Akev accepts the configured token: `false` on 401, throws when it can't tell. */
async function tokenAccepted(): Promise<boolean> {
  try {
    await api.status(AbortSignal.timeout(2500));
    return true;
  } catch (e) {
    if (isApiError(e) && e.isUnauthorized) return false;
    throw e;
  }
}

/** `?showcase` (with `?mock`): hide the mock backend's development labels. */
function showcaseRequested(): boolean {
  const hash = window.location.hash.split('?')[1] ?? '';
  return new URLSearchParams(window.location.search).has('showcase') || new URLSearchParams(hash).has('showcase');
}

async function boot(): Promise<void> {
  const auth = resolveBrowserAuth();
  api.configure(auth.config);
  let mock = false;
  if (MOCK_BUILD && mockRequested()) {
    const { installMock } = await import('./api/mock');
    installMock(api);
    // `?showcase`: product screenshots, without the "Mock data" development labels.
    useAppStore.setState({ mock: !showcaseRequested() });
    mock = true;
  }
  // A token remembered from an earlier visit is stale once Akev restarts with a new one:
  // check it first, so the user gets an explanation instead of a long "Connecting…".
  if (!mock && (await checkStoredToken(auth, tokenAccepted)) === 'stale') {
    api.configure({ token: null });
    useAppStore.setState({ connection: 'unauthorized', retryAt: null });
  } else {
    startConnection();
  }
  installExternalLinkHandling();
  const root = document.getElementById('root');
  if (!root) throw new Error('#root element missing');
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

void boot();
