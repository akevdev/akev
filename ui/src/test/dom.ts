/**
 * Test preload (`bunfig.toml`): registers happy-dom's browser globals (`window`, `document`,
 * `localStorage`…) so component tests can render with Testing Library. The URL matches a
 * local daemon, like the UI served by `akev start`.
 */
import { GlobalRegistrator } from '@happy-dom/global-registrator';

if (typeof document === 'undefined') GlobalRegistrator.register({ url: 'http://127.0.0.1:45456/', width: 1440, height: 900 });
