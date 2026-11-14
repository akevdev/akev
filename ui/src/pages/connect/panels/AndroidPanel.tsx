import { CircleCheck, MonitorSmartphone, RefreshCw, Smartphone, TriangleAlert, Unplug } from 'lucide-react';
import { useMemo } from 'react';
import type { InterceptorInfo } from '../../../api/types';
import { Badge, Button, IconButton } from '../../../components/ui';
import { activateInterceptor, clearActivationResult, deactivateInterceptor, pendingKey, useActivationResult, usePending } from '../actions';
import { CommandBlock } from '../CommandBlock';
import { metaStepNodes, Steps } from '../GuideView';
import { describeDevice, describeDeviceState, parseActivation, type AdbDevice } from '../metadata';
import { Callout, PanelEmpty, PanelSection, ResultBlock, TargetList, TargetRow } from './kit';
import { useRefreshInterceptors, type PanelProps } from './types';
import styles from './panels.module.css';

const CONNECT_STEPS = [
  'On the device, open Settings → About phone and tap “Build number” seven times to enable Developer options.',
  'Settings → System → Developer options → turn on “USB debugging”.',
  'Connect the device with a USB cable and accept the “Allow USB debugging?” prompt.',
];

const CHECK_LABEL: Record<string, string> = {
  'push-ca': 'Certificate copied',
  'reverse-tunnel': 'USB tunnel',
  'http-proxy': 'Proxy set',
};

function DeviceRow({ info, device: d, soleDevice }: { info: InterceptorInfo; device: AdbDevice; soleDevice: boolean }) {
  const state = describeDeviceState(d.state);
  const key = pendingKey(info.id, d.id);
  const stopKey = pendingKey(`${info.id}:stop`, d.id);
  const starting = usePending(key);
  const stopping = usePending(stopKey);
  const result = useActivationResult(key);
  const details = useMemo(() => (result ? parseActivation(result.metadata) : null), [result]);
  const active = d.active || (soleDevice && info.active);
  const hardware = describeDevice(d);

  return (
    <TargetRow
      icon={d.emulator ? <MonitorSmartphone size={16} strokeWidth={1.75} /> : <Smartphone size={16} strokeWidth={1.75} />}
      title={d.name}
      subtitle={hardware ? `${d.id} · ${hardware}` : d.id}
      badge={
        active ? (
          <Badge variant="success" size="sm" dot>
            Intercepted
          </Badge>
        ) : !state.ready ? (
          <Badge variant="warning" size="sm">
            {state.label}
          </Badge>
        ) : d.emulator ? (
          <Badge size="sm">Emulator</Badge>
        ) : undefined
      }
      hint={!active ? state.hint : undefined}
      active={active}
      disabled={!state.ready && !active}
      action={
        active ? (
          <Button size="sm" variant="ghost" icon={Unplug} loading={stopping} onClick={() => void deactivateInterceptor(info, { deviceId: d.id }, { key: stopKey, target: d.name })}>
            Stop
          </Button>
        ) : (
          <Button size="sm" variant="primary" disabled={!state.ready || !info.available} loading={starting} onClick={() => void activateInterceptor(info, { deviceId: d.id }, { key, target: d.name, verb: 'intercept', quiet: true })}>
            Intercept
          </Button>
        )
      }
    >
      {result && details && (
        <ResultBlock message={result.message ?? `${d.name} now uses the Akev proxy.`} onDismiss={() => clearActivationResult(key)}>
          {details.checks.length > 0 && (
            <ul className={styles.checks} aria-label="Setup steps">
              {details.checks.map((c) => (
                <li key={c.step} className={styles.check} data-ok={c.ok}>
                  {c.ok ? <CircleCheck size={12} strokeWidth={2} aria-hidden /> : <TriangleAlert size={12} strokeWidth={2} aria-hidden />}
                  <span title={c.detail}>{CHECK_LABEL[c.step] ?? c.step}</span>
                </li>
              ))}
            </ul>
          )}
          {details.installSteps.length > 0 && <Steps steps={details.installSteps} />}
          {details.manualRecovery && <CommandBlock code={details.manualRecovery} title="If the device loses connectivity after Akev quits" />}
        </ResultBlock>
      )}
    </TargetRow>
  );
}

/** Android over ADB: device list from metadata, activate per device. */
export function AndroidPanel({ info, meta }: PanelProps) {
  const { refreshing, refresh } = useRefreshInterceptors(4000);
  const devices = meta.devices;
  // Without any device the daemon reports "unavailable" with an explanation; that's the empty state, not an error.
  const adbMissing = !info.available && devices.length === 0 && /adb|platform-tools/i.test(info.unavailableReason ?? '') && !/no devices/i.test(info.unavailableReason ?? '');

  return (
    <>
      {adbMissing && (
        <Callout tone="warning" title="ADB isn't available">
          {info.unavailableReason ?? 'Install the Android SDK platform tools and make sure adb is on your PATH.'}
        </Callout>
      )}
      <PanelSection
        title="Devices"
        description="Akev points the device at the proxy over a USB tunnel and copies its certificate to the device for you to install."
        actions={<IconButton label="Refresh devices" icon={RefreshCw} size="sm" loading={refreshing} onClick={refresh} />}
      >
        {devices.length === 0 ? (
          <PanelEmpty
            icon={Smartphone}
            title="No devices connected"
            action={
              <Button size="sm" icon={RefreshCw} loading={refreshing} onClick={refresh}>
                Check again
              </Button>
            }
          >
            Connect a phone with USB debugging enabled, or start an emulator, then check again.
          </PanelEmpty>
        ) : (
          <TargetList label="Android devices">
            {devices.map((d) => (
              <DeviceRow key={d.id} info={info} device={d} soleDevice={devices.length === 1} />
            ))}
          </TargetList>
        )}
      </PanelSection>
      <PanelSection title="Connecting a device">
        <Steps steps={meta.steps.length > 0 ? metaStepNodes(meta.steps) : CONNECT_STEPS} />
        <p className={styles.footnote}>
          Emulators from Android Studio work without any setup. Apps targeting Android 7+ only trust Akev on emulators, rooted devices, or debug builds that allow user certificates.
        </p>
      </PanelSection>
    </>
  );
}
