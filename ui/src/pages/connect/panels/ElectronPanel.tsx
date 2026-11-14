import { AppWindow, Play, Square, X } from 'lucide-react';
import { useState, type FormEvent } from 'react';
import type { Platform } from '../../../api/types';
import { Badge, Button, Field, IconButton, Input } from '../../../components/ui';
import { usePagePref } from '../../../state/ui';
import { activateInterceptor, deactivateInterceptor, pendingKey, usePendingSet } from '../actions';
import { Callout, PanelSection, TargetList, TargetRow } from './kit';
import { useRefreshInterceptors, type PanelProps } from './types';
import styles from './panels.module.css';

const PLACEHOLDER: Record<Platform, string> = {
  macos: '/Applications/Slack.app',
  windows: 'C:\\Users\\you\\AppData\\Local\\slack\\slack.exe',
  linux: '/usr/bin/code',
};

const EXAMPLES: Record<Platform, { label: string; path: string }[]> = {
  macos: [
    { label: 'Slack', path: '/Applications/Slack.app' },
    { label: 'VS Code', path: '/Applications/Visual Studio Code.app' },
    { label: 'Discord', path: '/Applications/Discord.app' },
    { label: 'Postman', path: '/Applications/Postman.app' },
  ],
  linux: [
    { label: 'VS Code', path: '/usr/share/code/code' },
    { label: 'Slack', path: '/usr/lib/slack/slack' },
    { label: 'Discord', path: '/usr/share/discord/Discord' },
  ],
  windows: [],
};

const NOUN: Record<Platform, string> = { macos: 'application', windows: '.exe', linux: 'binary' };

/** Readable app name from a path (`/Applications/Slack.app` → `Slack`). */
export function appNameFromPath(path: string): string {
  const parts = path.replace(/[\\/]+$/, '').split(/[\\/]/).filter(Boolean);
  const app = parts.find((p) => /\.app$/i.test(p));
  if (app) return app.replace(/\.app$/i, '');
  const meaningful = parts.filter((p) => !/^(bin|run|MacOS)$/i.test(p));
  const last = meaningful[meaningful.length - 1] ?? path;
  return last.replace(/\.exe$/i, '');
}

function validatePath(p: string): string | null {
  const s = p.trim();
  if (!s) return 'Enter the path to the app';
  if (!/^(\/|~\/|[a-z]:[\\/]|\\\\)/i.test(s)) return 'Use the full path, starting with / or a drive letter';
  return null;
}

/** Electron: launch an app binary with interception; lists running apps; remembers recent paths. */
export function ElectronPanel({ info, meta, proxy }: PanelProps) {
  const platform = proxy.platform ?? 'linux';
  useRefreshInterceptors(5000); // running apps disappear when they quit
  const [recent, setRecent] = usePagePref<string[]>('connect.electronPaths', []);
  const recentList = [...new Set([...(Array.isArray(recent) ? recent : []), ...meta.recentPaths])].slice(0, 5);
  const [path, setPath] = useState('');
  const [touched, setTouched] = useState(false);
  const isPending = usePendingSet();
  const error = touched ? validatePath(path) : null;
  const running = meta.running;
  const runningPaths = new Set(running.map((r) => r.path));

  const launch = async (p: string) => {
    const res = await activateInterceptor(info, { path: p }, { key: pendingKey(info.id, p), target: appNameFromPath(p) });
    if (res?.success) {
      setRecent([p, ...recentList.filter((x) => x !== p)].slice(0, 5));
      if (p === path.trim()) setPath('');
      setTouched(false);
    }
  };

  const onSubmit = (e: FormEvent) => {
    e.preventDefault();
    setTouched(true);
    const p = path.trim();
    if (validatePath(p)) return;
    void launch(p);
  };

  return (
    <>
      {!info.available && info.unavailableReason && <Callout tone="warning">{info.unavailableReason}</Callout>}
      <PanelSection
        title="Launch an Electron app"
        description="Akev starts the app with its proxy and certificate settings. Slack, VS Code, Discord, Postman, Figma and most other Electron apps work. Quit the app first if it's already running."
      >
        <form className={styles.pathForm} onSubmit={onSubmit} noValidate>
          <Field label={`Path to the ${NOUN[platform]}`} error={error ?? undefined} className={styles.pathField} hint={platform === 'macos' ? 'The .app bundle is enough.' : undefined}>
            <Input value={path} onChange={(e) => setPath(e.target.value)} onBlur={() => path && setTouched(true)} placeholder={PLACEHOLDER[platform]} mono autoFocus spellCheck={false} />
          </Field>
          <Button type="submit" variant="primary" icon={Play} loading={isPending(pendingKey(info.id, path.trim()))} disabled={!info.available} className={styles.launchButton}>
            Launch
          </Button>
        </form>
        {EXAMPLES[platform].length > 0 && (
          <div className={styles.examples}>
            <span className={styles.shellLabel}>Try</span>
            {EXAMPLES[platform].map((ex) => (
              <button
                key={ex.path}
                type="button"
                className={styles.example}
                title={ex.path}
                onClick={() => {
                  setPath(ex.path);
                  setTouched(false);
                }}
              >
                {ex.label}
              </button>
            ))}
          </div>
        )}
      </PanelSection>

      {running.length > 0 && (
        <PanelSection title="Running" description="Launched from Akev — their traffic shows up in Traffic.">
          <TargetList label="Running Electron apps">
            {running.map((app) => {
              const stopKey = pendingKey(`${info.id}:stop`, app.pid);
              return (
                <TargetRow
                  key={app.pid}
                  icon={<AppWindow size={16} strokeWidth={1.75} />}
                  title={app.name}
                  subtitle={`PID ${app.pid}${app.path ? ` · ${app.path}` : ''}`}
                  badge={
                    <Badge variant="success" size="sm" dot>
                      Intercepted
                    </Badge>
                  }
                  active
                  action={
                    <Button size="sm" variant="ghost" icon={Square} loading={isPending(stopKey)} onClick={() => void deactivateInterceptor(info, { pid: app.pid }, { key: stopKey, target: app.name })}>
                      Quit
                    </Button>
                  }
                />
              );
            })}
          </TargetList>
        </PanelSection>
      )}

      {recentList.length > 0 && (
        <PanelSection title="Recent">
          <TargetList label="Recent Electron apps">
            {recentList.map((p) => (
              <TargetRow
                key={p}
                icon={<AppWindow size={16} strokeWidth={1.75} />}
                title={appNameFromPath(p)}
                subtitle={p}
                action={
                  <>
                    <IconButton label={`Forget ${appNameFromPath(p)}`} icon={X} size="sm" onClick={() => setRecent(recentList.filter((x) => x !== p))} />
                    <Button size="sm" icon={Play} loading={isPending(pendingKey(info.id, p))} disabled={!info.available || runningPaths.has(p)} onClick={() => void launch(p)}>
                      {runningPaths.has(p) ? 'Running' : 'Launch'}
                    </Button>
                  </>
                }
              />
            ))}
          </TargetList>
        </PanelSection>
      )}
    </>
  );
}
