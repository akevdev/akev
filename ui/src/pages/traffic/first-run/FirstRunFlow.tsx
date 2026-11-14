import clsx from 'clsx';
import { ArrowLeft, ArrowRight, BookOpen, Check, Circle, Radio, ShieldAlert, ShieldCheck, Smartphone } from 'lucide-react';
import { useEffect, useMemo } from 'react';
import { Logo } from '../../../app/Logo';
import { navigate } from '../../../app/router';
import { Badge, Button, MethodBadge, Skeleton, Spinner, StatusBadge, Switch, toast } from '../../../components/ui';
import { errorMessage } from '../../../api/client';
import { openExternal } from '../../../lib/platform';
import { proxyAddress, useAppStore } from '../../../state/app';
import { trustWithConfirm, useCaTrust } from '../../../state/caTrust';
import { useExchangeSummary, useTraffic } from '../../../state/traffic';
import { CommandBlock } from '../../connect/CommandBlock';
import { toggleRecording } from '../actions/exchangeActions';
import { InterceptorTile, pickTop, ProxyAddressCard } from '../empty/EmptyStates';
import { closeFirstRun, FIRST_RUN_STEPS, firstNewExchange, setFirstRunStep, useFirstRun, type FirstRunStep } from './firstRun';
import styles from './FirstRunFlow.module.css';

export const GETTING_STARTED_URL = 'https://akev.dev/docs/getting-started/';

/** A request to send through the proxy from a terminal (plain HTTP: no certificate needed). */
export function curlCommand(address: string | null): string {
  return `curl -x http://${address ?? '127.0.0.1:8000'} http://example.com/`;
}

/**
 * The first-run setup in place of the empty Traffic list: trust the CA, pick what to capture,
 * then wait (live) for the first request. Every step can be skipped; "Skip setup" closes it
 * for good (it can be reopened from the palette and Settings → General).
 */
export default function FirstRunFlow() {
  const step = useFirstRun((s) => s.step);
  const baseline = useFirstRun((s) => s.baseline);
  const firstId = useTraffic((s) => firstNewExchange(s.order, baseline));
  const trusted = useCaTrust((s) => s.report?.trusted === true);
  const anyActive = useAppStore((s) => s.interceptors.some((i) => i.active));

  const done: Record<FirstRunStep, boolean> = { 0: trusted, 1: anyActive || firstId != null, 2: firstId != null };
  const go = (next: FirstRunStep) => setFirstRunStep(next);
  const next = () => go(Math.min(2, step + 1) as FirstRunStep);

  return (
    <div className={styles.wrap}>
      <section className={styles.panel} aria-labelledby="first-run-title">
        <header className={styles.header}>
          <Logo size={30} className={styles.logo} />
          <div className={styles.headerText}>
            <h2 id="first-run-title" className={styles.title}>
              Set up Akev
            </h2>
            <p className={styles.subtitle}>Three quick steps to your first captured request. Skip any you don't need.</p>
          </div>
          <Button variant="ghost" size="sm" onClick={closeFirstRun}>
            Skip setup
          </Button>
        </header>

        <ol className={styles.steps} aria-label="Setup steps">
          {FIRST_RUN_STEPS.map((s) => {
            const current = s.id === step;
            return (
              <li key={s.id} className={styles.stepItem}>
                <button
                  type="button"
                  className={clsx(styles.step, current && styles.stepCurrent, done[s.id] && styles.stepDone)}
                  aria-current={current ? 'step' : undefined}
                  onClick={() => go(s.id)}
                >
                  <span className={styles.stepNumber} aria-hidden>
                    {done[s.id] ? <Check size={12} strokeWidth={2.75} /> : s.id + 1}
                  </span>
                  <span className={styles.stepLabel}>{s.label}</span>
                  {done[s.id] && <span className="sr-only"> (done)</span>}
                </button>
              </li>
            );
          })}
        </ol>

        <div className={styles.body}>
          {step === 0 && <TrustStep onTrusted={next} />}
          {step === 1 && <CaptureStep onLaunched={() => go(2)} />}
          {step === 2 && <WaitStep firstId={firstId} />}
        </div>

        <footer className={styles.footer}>
          {step > 0 ? (
            <Button variant="ghost" icon={ArrowLeft} onClick={() => go((step - 1) as FirstRunStep)}>
              Back
            </Button>
          ) : (
            <span />
          )}
          {step < 2 ? (
            <Button variant={done[step] ? 'primary' : 'secondary'} iconRight={ArrowRight} onClick={next}>
              {done[step] ? 'Continue' : 'Skip this step'}
            </Button>
          ) : (
            <Button onClick={closeFirstRun}>
              {done[2] ? 'Finish' : 'Close setup'}
            </Button>
          )}
        </footer>
      </section>
    </div>
  );
}

// =====================================================================================
// Step 1: trust the CA
// =====================================================================================

function TrustStep({ onTrusted }: { onTrusted: () => void }) {
  const report = useCaTrust((s) => s.report);
  const busy = useCaTrust((s) => s.busy);

  useEffect(() => {
    void useCaTrust.getState().refresh();
  }, []);

  const main = report?.stores.find((s) => s.canChange && s.trusted !== true) ?? report?.stores[0];

  return (
    <div className={styles.stepBody}>
      <h3 className={styles.stepTitle}>Trust the Akev certificate on this computer</h3>
      <p className={styles.text}>
        Akev reads HTTPS traffic with its own certificate authority. Browsers and terminals that Akev launches trust it automatically. Trust it on this computer too to inspect other apps.
      </p>
      {!report ? (
        <div className={styles.statusCard} aria-busy="true">
          <Skeleton width={220} height={12} />
        </div>
      ) : report.trusted === true ? (
        <div className={clsx(styles.statusCard, styles.statusGood)} role="status">
          <ShieldCheck size={18} aria-hidden className={styles.statusIcon} />
          <div className={styles.statusText}>
            <span className={styles.statusTitle}>Already trusted on this computer</span>
            <span className={styles.muted}>Apps here accept the certificates Akev makes. Nothing to do.</span>
          </div>
          <Badge variant="success" dot>
            Trusted
          </Badge>
        </div>
      ) : main?.canChange ? (
        <div className={styles.statusCard}>
          <ShieldAlert size={18} aria-hidden className={clsx(styles.statusIcon, styles.statusIconWarn)} />
          <div className={styles.statusText}>
            <span className={styles.statusTitle}>Not trusted yet</span>
            <span className={styles.muted}>
              {main.label}: {main.description}.{main.needsAdmin ? ' Your system asks for your password.' : ''}
            </span>
          </div>
          <Button
            variant="primary"
            icon={ShieldCheck}
            loading={busy === main.id}
            disabled={busy != null}
            onClick={async () => {
              if (await trustWithConfirm(main)) onTrusted();
            }}
          >
            Trust
          </Button>
        </div>
      ) : (
        <div className={styles.statusCard}>
          <ShieldAlert size={18} aria-hidden className={clsx(styles.statusIcon, styles.statusIconWarn)} />
          <div className={styles.statusText}>
            <span className={styles.statusTitle}>Akev can't change the trust store here</span>
            <span className={styles.muted}>{main?.unavailableReason ?? 'Follow the steps in Settings → Certificates to trust it by hand.'}</span>
          </div>
          <Button onClick={() => navigate('/settings/certificates')}>Open instructions</Button>
        </div>
      )}
      <p className={styles.hint}>Only capturing apps that Akev launches? Skip this step.</p>
    </div>
  );
}

// =====================================================================================
// Step 2: pick what to capture
// =====================================================================================

function CaptureStep({ onLaunched }: { onLaunched: () => void }) {
  const interceptors = useAppStore((s) => s.interceptors);
  const status = useAppStore((s) => s.status);
  const top = useMemo(() => pickTop(interceptors), [interceptors]);

  return (
    <div className={styles.stepBody}>
      <h3 className={styles.stepTitle}>Choose what to capture</h3>
      <p className={styles.text}>Launch a browser or a terminal that already goes through Akev. Nothing else on your computer changes.</p>
      {top.length > 0 ? (
        <div className={styles.grid} role="list" aria-label="Intercept an app">
          {top.map((i) => (
            <div role="listitem" key={i.id} className={styles.gridItem}>
              <InterceptorTile info={i} onActivated={onLaunched} />
            </div>
          ))}
        </div>
      ) : (
        <div className={styles.statusCard} aria-busy="true">
          <Spinner size={14} tone="muted" label="Loading apps" />
          <span className={styles.muted}>Looking for browsers and terminals…</span>
        </div>
      )}
      <div className={styles.deviceCard}>
        <Smartphone size={18} aria-hidden className={styles.statusIcon} />
        <div className={styles.statusText}>
          <span className={styles.statusTitle}>A phone, container or another computer</span>
          <span className={styles.muted}>Connect has guides for Android, iOS, Docker, WireGuard and manual setup.</span>
        </div>
        <Button iconRight={ArrowRight} onClick={() => navigate('/connect')}>
          Open Connect
        </Button>
      </div>
      <ProxyAddressCard address={proxyAddress(status)} />
    </div>
  );
}

// =====================================================================================
// Step 3: wait for the first request
// =====================================================================================

function WaitStep({ firstId }: { firstId: number | null }) {
  const summary = useExchangeSummary(firstId);
  const status = useAppStore((s) => s.status);
  const recording = useAppStore((s) => s.recording);

  return (
    <div className={styles.stepBody}>
      {summary ? (
        <div className={styles.arrived} role="status">
          <span className={styles.arrivedIcon} aria-hidden>
            <Check size={20} strokeWidth={2.5} />
          </span>
          <h3 className={styles.stepTitle}>Your first request arrived</h3>
          <div className={styles.exchange}>
            <MethodBadge method={summary.method} />
            <StatusBadge summary={summary} />
            <span className={styles.exchangeUrl}>
              {summary.host}
              <span className={styles.muted}>{summary.path}</span>
            </span>
          </div>
          <Button
            variant="primary"
            iconRight={ArrowRight}
            onClick={() => {
              closeFirstRun();
              navigate(`/traffic/${summary.id}`);
            }}
          >
            Show my traffic
          </Button>
        </div>
      ) : (
        <div className={styles.waiting}>
          <div className={clsx(styles.beacon, !recording && styles.beaconPaused)} aria-hidden>
            <span className={styles.ring} />
            <span className={styles.ring} />
            <span className={styles.beaconIcon}>{recording ? <Radio size={20} strokeWidth={1.75} /> : <Circle size={18} strokeWidth={2} />}</span>
          </div>
          <h3 className={styles.stepTitle} role="status">
            {recording ? 'Waiting for your first request…' : 'Recording is paused'}
          </h3>
          <p className={styles.text}>
            {recording
              ? 'Open a website in the browser you launched, or send a request from any terminal:'
              : "Requests still go through Akev, but they aren't recorded. Resume recording to see them here."}
          </p>
          {recording ? (
            <CommandBlock code={curlCommand(proxyAddress(status))} aria-label="Command that sends a request through Akev" className={styles.command} />
          ) : (
            <Button variant="primary" icon={Circle} onClick={toggleRecording}>
              Resume recording
            </Button>
          )}
        </div>
      )}
      <AutosaveOffer />
      <a
        className={styles.docsLink}
        href={GETTING_STARTED_URL}
        target="_blank"
        rel="noreferrer"
        onClick={(e) => {
          e.preventDefault();
          openExternal(GETTING_STARTED_URL);
        }}
      >
        <BookOpen size={13} aria-hidden /> Read the getting started guide
      </a>
    </div>
  );
}

/** "Keep traffic when Akev restarts" (`settings.sessions.autosave`). */
function AutosaveOffer() {
  const settings = useAppStore((s) => s.settings);
  if (!settings) return null;
  const { sessions } = settings;
  const set = async (autosave: boolean) => {
    try {
      const latest = useAppStore.getState().settings ?? settings;
      await useAppStore.getState().saveSettings({ ...latest, sessions: { ...latest.sessions, autosave } });
      if (autosave) toast.success('Autosave is on', { description: 'Captured traffic is kept when Akev restarts. Change it in Settings → Data.' });
    } catch (e) {
      toast.error("Couldn't change autosave", { description: errorMessage(e) });
    }
  };
  return (
    <div className={styles.autosave}>
      <Switch
        checked={sessions.autosave}
        onCheckedChange={(v) => void set(v)}
        label="Keep traffic when Akev restarts"
        description={sessions.autosave ? 'Autosave is on: captured traffic is saved as you go.' : 'Turn on autosave to reopen your traffic next time.'}
      />
    </div>
  );
}
