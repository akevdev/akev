import { useMemo, type ComponentType } from 'react';
import type { InterceptorInfo } from '../../api/types';
import { ProBadge } from '../../components/pro';
import { AppIcon, Badge, Button, Sheet } from '../../components/ui';
import { interceptorFeature } from '../../lib/license';
import { deactivateInterceptor, usePending } from './actions';
import { CATEGORY_LABEL, iconKey, panelKindFor, statusOf, type PanelKind } from './catalog';
import { parseMetadata } from './metadata';
import { AndroidPanel } from './panels/AndroidPanel';
import { DockerPanel } from './panels/DockerPanel';
import { ElectronPanel } from './panels/ElectronPanel';
import { GenericPanel } from './panels/GenericPanel';
import { IosPanel } from './panels/IosPanel';
import { JvmPanel } from './panels/JvmPanel';
import { ManualPanel } from './panels/ManualPanel';
import { SystemProxyPanel } from './panels/SystemProxyPanel';
import { TerminalPanel } from './panels/TerminalPanel';
import { WireguardPanel } from './panels/WireguardPanel';
import { TransparentPanel } from './panels/TransparentPanel';
import type { PanelProps } from './panels/types';
import { useProxyInfo } from './useProxyInfo';
import styles from './InterceptorSheet.module.css';

const PANELS: Record<PanelKind, ComponentType<PanelProps>> = {
  android: AndroidPanel,
  docker: DockerPanel,
  terminal: TerminalPanel,
  electron: ElectronPanel,
  manual: ManualPanel,
  ios: IosPanel,
  'system-proxy': SystemProxyPanel,
  jvm: JvmPanel,
  wireguard: WireguardPanel,
  transparent: TransparentPanel,
  generic: GenericPanel,
};

const WIDTH: Partial<Record<PanelKind, number>> = { manual: 680, 'system-proxy': 560, terminal: 540, docker: 560, wireguard: 580, transparent: 680 };

/** Panels whose per-target controls replace the footer's Stop button. */
const TARGETED: ReadonlySet<PanelKind> = new Set(['android', 'docker', 'jvm', 'system-proxy', 'electron', 'wireguard', 'transparent']);

export interface InterceptorSheetProps {
  info: InterceptorInfo | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onOpenOther: (id: string) => void;
}

/** Right-side configuration Sheet for interceptors that need setup (DESIGN.md §8.2). */
export function InterceptorSheet({ info, open, onOpenChange, onOpenOther }: InterceptorSheetProps) {
  const proxy = useProxyInfo();
  const meta = useMemo(() => parseMetadata(info?.metadata), [info?.metadata]);
  const kind = info ? (panelKindFor(info) ?? 'generic') : null;
  const stopping = usePending(info ? `${info.id}:stop` : '');
  const Panel = kind ? PANELS[kind] : null;
  const status = info ? statusOf(info, proxy.platform) : null;
  const feature = info ? interceptorFeature(info.id) : undefined;

  return (
    <Sheet
      open={open && info != null}
      onOpenChange={onOpenChange}
      focusContent
      width={(kind && WIDTH[kind]) ?? 500}
      title={
        info ? (
          <span className={styles.title}>
            <AppIcon name={iconKey(info.icon)} size={32} tile />
            <span className={styles.titleText}>
              <span className={styles.name}>{info.name}</span>
              <span className={styles.meta}>
                {CATEGORY_LABEL[info.category] ?? info.category}
                {feature && <ProBadge feature={feature} />}
                {status && status.tone !== 'ready' && (
                  <Badge size="sm" variant={status.tone === 'active' ? 'success' : status.tone === 'missing' ? 'warning' : 'neutral'} dot={status.tone === 'active'}>
                    {status.label}
                  </Badge>
                )}
                {status?.tone === 'ready' && info.version && <span className="tnum">· {info.version}</span>}
              </span>
            </span>
          </span>
        ) : (
          ''
        )
      }
      description={info?.description}
      footer={
        info ? (
          <>
            {info.active && kind && !TARGETED.has(kind) && (
              <Button variant="subtle-danger" loading={stopping} onClick={() => void deactivateInterceptor(info)} className={styles.stop}>
                Stop intercepting
              </Button>
            )}
            <Button onClick={() => onOpenChange(false)}>Done</Button>
          </>
        ) : undefined
      }
    >
      {info && Panel && (
        <div className={styles.body} key={info.id}>
          <Panel info={info} meta={meta} proxy={proxy} onOpenOther={onOpenOther} />
        </div>
      )}
    </Sheet>
  );
}
