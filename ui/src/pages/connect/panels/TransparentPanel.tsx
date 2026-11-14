import clsx from 'clsx';
import { CircleCheck, CircleOff, Power, RadioTower, TriangleAlert } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type { TransparentBackend, TransparentCaptureMode, TransparentExclusion, TransparentLauncher, TransparentPortScope, TransparentStep } from '../../../api/types';
import { ProCallout } from '../../../components/pro';
import { Badge, Button, Input, SegmentedControl, Spinner, Switch } from '../../../components/ui';
import { useFeature } from '../../../state/license';
import { usePagePref } from '../../../state/ui';
import { CommandBlock } from '../CommandBlock';
import { formatMark, listenerSummary, MODE_HELP, normalizeOptions, planRequestOf, recentRows, SUGGESTED_MARK, type TransparentOptions } from '../transparent';
import { saveTransparent, useTransparentInfo, useTransparentPlan } from '../useTransparent';
import { Callout, PanelSection } from './kit';
import type { PanelProps } from './types';
import styles from './TransparentPanel.module.css';
import kit from './panels.module.css';

const MODES: { value: TransparentCaptureMode; label: string }[] = [
  { value: 'app', label: 'Per app' },
  { value: 'user', label: 'Per user' },
  { value: 'machine', label: 'Whole machine' },
];

const PORTS: { value: TransparentPortScope; label: string }[] = [
  { value: 'web', label: '80 + 443' },
  { value: 'all', label: 'All TCP' },
  { value: 'custom', label: 'Custom' },
];

const BACKENDS: { value: TransparentBackend; label: string }[] = [
  { value: 'nft', label: 'nftables' },
  { value: 'iptables', label: 'iptables' },
];

const LAUNCHERS: { value: TransparentLauncher; label: string }[] = [
  { value: 'systemd', label: 'systemd slice' },
  { value: 'cgroupfs', label: 'Without systemd' },
];

const EXCLUSIONS: { value: TransparentExclusion; label: string }[] = [
  { value: 'uid', label: "Akev's user id" },
  { value: 'mark', label: 'Firewall mark' },
];

function Step({ n, step }: { n?: number; step: TransparentStep }) {
  return (
    <li className={styles.step}>
      {n != null && (
        <span className={styles.stepNum} aria-hidden>
          {n}
        </span>
      )}
      <div className={styles.stepBody}>
        <div className={styles.stepTitle}>
          {step.title}
          <Badge size="sm" variant={step.root ? 'warning' : 'neutral'}>
            {step.root ? 'as root' : 'as you'}
          </Badge>
        </div>
        {step.description && <p className={styles.stepDesc}>{step.description}</p>}
        <CommandBlock code={step.command} wrap={!step.command.includes('\n')} aria-label={step.title} copyToast="Copied — paste it into a terminal" />
      </div>
    </li>
  );
}

/** Connect → Transparent capture (Linux): listener status / redirect check, mode picker, generated commands. */
export function TransparentPanel(_props: PanelProps) {
  const { info, error: statusError } = useTransparentInfo(2000);
  const [stored, setStored] = usePagePref<TransparentOptions>('connect.transparent', normalizeOptions(null));
  const options = useMemo(() => normalizeOptions(stored), [stored]);
  const set = <K extends keyof TransparentOptions>(key: K, value: TransparentOptions[K]) => setStored({ ...options, [key]: value });
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), 5000);
    return () => window.clearInterval(id);
  }, []);
  const [busy, setBusy] = useState(false);
  const licensed = useFeature('transparent');

  const listener = info?.listener ?? null;
  const summary = listenerSummary(listener, now);
  const { request, error: inputError } = planRequestOf(options);
  // Without Pro there's no listener to send redirected connections to: no commands either.
  const { plan, error: planError, loading } = useTransparentPlan(info?.supported === false || !licensed ? null : request, listener?.port ?? listener?.requestedPort ?? null);
  const rows = recentRows(listener, now);
  const system = info?.system;

  const turnOn = async () => {
    setBusy(true);
    await saveTransparent({ enabled: true }, "Couldn't start the transparent listener");
    setBusy(false);
  };
  const applySuggestedMark = async () => {
    setBusy(true);
    await saveTransparent({ mark: SUGGESTED_MARK }, "Couldn't set the firewall mark");
    setBusy(false);
  };

  if (info && !info.supported) {
    return (
      <Callout tone="warning" title="Not available on this system">
        {info.unsupportedReason ?? 'Transparent capture needs Linux.'}
      </Callout>
    );
  }

  const StatusIcon = summary.tone === 'active' ? CircleCheck : summary.tone === 'error' ? TriangleAlert : summary.tone === 'off' ? CircleOff : RadioTower;

  return (
    <>
      <p className={kit.lead}>
        Akev's transparent listener reads each redirected connection's original destination and intercepts it like any proxied request. Akev never changes your firewall itself — it writes the commands, you run them.
      </p>

      {!licensed && (
        <ProCallout feature="transparent" title="Transparent capture is part of Akev Pro">
          {listener?.enabled ? 'The listener is paused until Pro is active; your settings are kept.' : 'Capture apps that ignore proxy settings — per app, per user or the whole machine — with firewall rules Akev writes for you.'}
        </ProCallout>
      )}

      <section className={styles.statusCard} data-tone={summary.tone} aria-label="Transparent listener status">
        <div className={styles.statusHead}>
          <span className={styles.statusIcon} aria-hidden>
            {summary.tone === 'waiting' && listener ? <Spinner size={15} tone="muted" /> : <StatusIcon size={16} strokeWidth={2} />}
          </span>
          <div className={styles.statusText} role="status" aria-live="polite">
            <span className={styles.statusTitle}>{summary.title}</span>
            {summary.detail && <span className={styles.statusDetail}>{summary.detail}</span>}
          </div>
          {listener && !listener.enabled && licensed && (
            <Button size="sm" variant="primary" icon={Power} loading={busy} onClick={() => void turnOn()} className={styles.statusAction}>
              Turn on
            </Button>
          )}
        </div>
        {listener?.listening && (
          <div className={styles.facts}>
            {listener.addresses.map((a) => (
              <Badge key={a} size="sm" mono>
                {a}
              </Badge>
            ))}
            {listener.mark !== 0 && (
              <Badge size="sm" variant={listener.markError ? 'warning' : 'neutral'}>
                mark {formatMark(listener.mark)}
              </Badge>
            )}
            {listener.direct > 0 && <Badge size="sm">{listener.direct} direct</Badge>}
          </div>
        )}
        {rows.length > 0 && (
          <ul className={styles.recent} aria-label="Recent connections">
            {rows.map((r) => (
              <li key={r.key} className={styles.recentRow}>
                <span className={clsx(styles.recentDest, !r.redirected && styles.recentDirect)}>{r.destination}</span>
                <span className={styles.recentWho}>{r.who}</span>
                <span className={styles.recentWhen}>{r.when}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
      {statusError && <Callout tone="danger">{statusError}</Callout>}
      {listener && listener.loops > 0 && (
        <Callout tone="danger" title="Akev's own connections are being redirected back to it">
          {listener.loops} connection{listener.loops === 1 ? '' : 's'} from Akev looped back and {listener.loops === 1 ? 'was' : 'were'} refused: your rules don't exclude Akev. Use per-app capture, or exclude Akev by user id or firewall mark.
        </Callout>
      )}
      {listener?.markError && listener.mark !== 0 && <Callout tone="warning">{listener.markError}</Callout>}

      {licensed && (
        <>
          <PanelSection title="What to capture">
            <SegmentedControl<TransparentCaptureMode> value={options.mode} onValueChange={(v) => set('mode', v)} items={MODES} size="sm" aria-label="Capture mode" className={kit.selfStart} />
            <p className={styles.modeHelp}>{MODE_HELP[options.mode]}</p>
            <div className={styles.options}>
              {options.mode === 'app' && (
                <>
                  <label className={styles.full}>
                    <span className={styles.optionLabel}>Program to launch</span>
                    <Input value={options.command} onChange={(e) => set('command', e.target.value)} placeholder="Empty: a captured shell — or e.g. firefox --new-instance" mono aria-label="Program to launch" />
                  </label>
                  <div>
                    <span className={styles.optionLabel}>Launch with</span>
                    <SegmentedControl<TransparentLauncher> value={options.launcher} onValueChange={(v) => set('launcher', v)} items={LAUNCHERS} size="sm" aria-label="Launcher" />
                  </div>
                  <div>
                    <Switch checked={options.trustCa} onCheckedChange={(v) => set('trustCa', v)} size="sm" label="Trust Akev's CA" description="CA variables on the launch command (curl, Node, Python…)" />
                  </div>
                </>
              )}
              {options.mode === 'user' && (
                <label className={styles.full}>
                  <span className={styles.optionLabel}>User account</span>
                  <Input value={options.user} onChange={(e) => set('user', e.target.value)} placeholder="User name or id, e.g. tester" mono aria-label="User account" />
                </label>
              )}
              {options.mode === 'machine' && (
                <div className={styles.full}>
                  <span className={styles.optionLabel}>Keep Akev's own connections out by</span>
                  <SegmentedControl<TransparentExclusion> value={options.exclude} onValueChange={(v) => set('exclude', v)} items={EXCLUSIONS} size="sm" aria-label="Exclude Akev by" />
                </div>
              )}
              <div>
                <span className={styles.optionLabel}>Destination ports</span>
                <SegmentedControl<TransparentPortScope> value={options.ports} onValueChange={(v) => set('ports', v)} items={PORTS} size="sm" aria-label="Destination ports" />
              </div>
              <div>
                <span className={styles.optionLabel}>Firewall</span>
                <SegmentedControl<TransparentBackend> value={options.backend} onValueChange={(v) => set('backend', v)} items={BACKENDS} size="sm" aria-label="Firewall tool" />
              </div>
              {options.ports === 'custom' && (
                <label className={styles.full}>
                  <span className={styles.optionLabel}>Ports</span>
                  <Input value={options.customPorts} onChange={(e) => set('customPorts', e.target.value)} placeholder="8080, 8443" mono aria-label="Custom ports" />
                </label>
              )}
            </div>
            <details className={styles.details}>
              <summary>More options</summary>
              <div className={clsx(styles.detailsBody, styles.options)}>
                <label className={styles.full}>
                  <span className={styles.optionLabel}>Never redirect these destinations (besides loopback)</span>
                  <Input value={options.excludeDestinations} onChange={(e) => set('excludeDestinations', e.target.value)} placeholder="10.0.0.0/8, 192.168.1.10" mono aria-label="Excluded destinations" />
                </label>
                <div className={styles.full}>
                  <Switch checked={options.sudo} onCheckedChange={(v) => set('sudo', v)} size="sm" label="Prefix root commands with sudo" description="Turn off when you paste them into a root shell." />
                </div>
              </div>
            </details>
            {options.mode === 'machine' && options.exclude === 'mark' && listener?.mark === 0 && (
              <Callout
                tone="info"
                title="Akev needs a firewall mark"
                action={
                  <Button size="sm" loading={busy} onClick={() => void applySuggestedMark()}>
                    Use mark {formatMark(SUGGESTED_MARK)}
                  </Button>
                }
              >
                Akev marks its upstream connections with it so the rules can skip them (Settings → Proxy → Transparent capture).
              </Callout>
            )}
          </PanelSection>

          <PanelSection title="Commands" description={plan?.summary ? `Captures: ${plan.summary}.` : 'Run them in a terminal, in order.'} actions={loading ? <Spinner size={14} tone="muted" /> : undefined}>
            {(inputError || planError) && <Callout tone={inputError ? 'warning' : 'danger'}>{inputError ?? planError}</Callout>}
            {plan && (
              <div className={clsx(loading && styles.planStale)}>
                {plan.warnings.length > 0 && (
                  <Callout tone="warning">
                    <ul className={kit.bullets}>
                      {plan.warnings.map((w) => (
                        <li key={w}>{w}</li>
                      ))}
                    </ul>
                  </Callout>
                )}
                <ol className={styles.steps} aria-label="Setup">
                  {plan.setup.map((s, i) => (
                    <Step key={s.title} n={i + 1} step={s} />
                  ))}
                  {plan.launch && <Step n={plan.setup.length + 1} step={plan.launch} />}
                  <Step n={plan.setup.length + (plan.launch ? 2 : 1)} step={plan.test} />
                </ol>
              </div>
            )}
          </PanelSection>
        </>
      )}

      {plan && (
        <PanelSection title="When you're done" description="Remove the rules — while they're loaded and Akev isn't running, the redirected connections fail.">
          <ol className={styles.steps} aria-label="Teardown">
            {plan.teardown.map((s) => (
              <Step key={s.title} step={s} />
            ))}
          </ol>
        </PanelSection>
      )}

      {plan && (
        <PanelSection title="Good to know">
          <ul className={kit.bullets}>
            {plan.notes.map((n) => (
              <li key={n}>{n}</li>
            ))}
            {system?.user && system.uid != null && (
              <li>
                Akev runs as {system.user} (uid {system.uid}){system.userManagerCgroup ? `; your systemd user manager is ${system.userManagerCgroup}` : ''}.
              </li>
            )}
            <li>macOS (pf) and Windows (WinDivert) transparent capture aren't supported yet — use the system proxy or per-app interceptors there.</li>
          </ul>
        </PanelSection>
      )}
    </>
  );
}
