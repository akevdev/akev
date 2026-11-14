import clsx from 'clsx';
import { ArrowDown, ArrowDownUp, ArrowUp, CirclePause, FlaskConical, Info, MessagesSquare, RotateCcw } from 'lucide-react';
import { type CSSProperties, type ReactNode } from 'react';
import type { Action, TagColor, WsBreakpointDirection } from '../../../api/types';
import { TAG_COLORS } from '../../../api/types';
import { ProCallout } from '../../../components/pro';
import { Button, Checkbox, CodeEditor, Input, Kbd, NumberInput, SegmentedControl, Switch } from '../../../components/ui';
import { formatDuration } from '../../../lib/format';
import { useFeature } from '../../../state/license';
import { tagColorVar } from '../../../lib/http';
import { bpsToKbps, detectPreset, kbpsToBps, presetValues, SCRIPT_TEMPLATE, THROTTLE_PRESETS, type ThrottlePresetId } from '../../../lib/rules/actions';
import { onRadioGroupKeyDown, radioTabIndex } from '../radioKeys';
import { BodySourceEditor } from './BodyEditors';
import { Block, Explain, FieldError, useIssue, useTouch } from './context';
import { HeadersField, StatusField } from './fields';
import { PatternEditor } from './Matchers';
import { PathPicker } from './PathPicker';
import { TlsProfileForm } from './TlsProfileForm';
import { TransformEditor } from './TransformEditor';
import styles from './Editor.module.css';

export interface ActionFormProps {
  action: Action;
  onChange: (a: Action) => void;
  ruleId: string;
  /** Opens the test panel (script "Test with selected exchange"). */
  onTest: () => void;
}

/** The per-action settings form under the action picker. */
export function ActionForm({ action, onChange, ruleId, onTest }: ActionFormProps) {
  switch (action.type) {
    case 'mock':
      return <MockForm action={action} onChange={onChange} ruleId={ruleId} />;
    case 'forward':
      return <ForwardForm action={action} onChange={onChange} ruleId={ruleId} />;
    case 'map-local':
      return <MapLocalForm action={action} onChange={onChange} />;
    case 'transform':
      return (
        <div className={styles.stack}>
          <TransformEditor request={action.request} response={action.response} onChange={(request, response) => onChange(withTransforms({ type: 'transform' }, request, response))} path="action" ruleId={ruleId} />
          <Explain icon={<Info size={13} aria-hidden />}>Rewrites stack: later rules still run, and the request is then handled by the first matching respond rule (or sent upstream).</Explain>
        </div>
      );
    case 'passthrough':
      return (
        <div className={styles.stack}>
          <Explain>Matching requests go to their original destination and no later rule is evaluated. Optionally rewrite them on the way:</Explain>
          <TransformEditor request={action.request} response={action.response} onChange={(request, response) => onChange(withTransforms({ type: 'passthrough' }, request, response))} path="action" ruleId={ruleId} />
        </div>
      );
    case 'breakpoint':
      return <BreakpointForm action={action} onChange={onChange} />;
    case 'delay':
      return <DelayForm action={action} onChange={onChange} />;
    case 'throttle':
      return <ThrottleForm action={action} onChange={onChange} />;
    case 'highlight':
      return <HighlightForm action={action} onChange={onChange} />;
    case 'script':
      return <ScriptForm action={action} onChange={onChange} ruleId={ruleId} onTest={onTest} />;
    case 'ws-reject':
      return (
        <div className={styles.stack}>
          <Block label="Status">
            <StatusField status={action.status} statusMessage={undefined} allowMessage={false} onChange={(status) => onChange({ ...action, status })} path="action." />
          </Block>
          <Block label="Body">
            <BodySourceEditor body={action.body} onChange={(body) => onChange({ ...action, body })} path="action.body" contentId={`${ruleId}:ws-body`} />
          </Block>
          <Explain>WebSocket upgrade requests get this HTTP response instead of a connection.</Explain>
        </div>
      );
    case 'close':
      return <Explain icon={<Info size={13} aria-hidden />}>The connection is closed cleanly without sending any response — clients usually report it as an empty reply or network error.</Explain>;
    case 'reset':
      return <Explain icon={<Info size={13} aria-hidden />}>The connection is aborted with a TCP RST (or an HTTP/2 RST_STREAM), simulating a crashed server or a flaky network.</Explain>;
    case 'timeout':
      return <Explain icon={<Info size={13} aria-hidden />}>The request is accepted and never answered; the connection stays open until the client gives up.</Explain>;
    case 'ws-echo':
      return <Explain icon={<Info size={13} aria-hidden />}>WebSocket upgrades are accepted by Akev itself and every message is echoed straight back — handy for testing clients without a server.</Explain>;
    case 'ws-breakpoint':
      return <WsBreakpointForm action={action} onChange={onChange} />;
    case 'tls-profile':
      return <TlsProfileForm action={action} onChange={onChange} />;
  }
}

function withTransforms<T extends { type: 'transform' | 'passthrough' }>(base: T, request: unknown, response: unknown): Action {
  return { ...base, ...(request ? { request } : {}), ...(response ? { response } : {}) } as Action;
}

// =====================================================================================

function MockForm({ action, onChange, ruleId }: { action: Extract<Action, { type: 'mock' }>; onChange: (a: Action) => void; ruleId: string }) {
  return (
    <div className={styles.stack}>
      <Block label="Status">
        <StatusField status={action.status} statusMessage={action.statusMessage} onChange={(status, statusMessage) => onChange(statusMessage === undefined ? withoutKey({ ...action, status }, 'statusMessage') : { ...action, status, statusMessage })} path="action." />
      </Block>
      <Block label="Headers" hint="Content-Length and Date are added automatically">
        <HeadersField headers={action.headers} onChange={(headers) => onChange({ ...action, headers })} path="action.headers" />
      </Block>
      <Block label="Body">
        <BodySourceEditor body={action.body} onChange={(body) => onChange({ ...action, body })} path="action.body" headers={action.headers} onHeadersChange={(headers) => onChange({ ...action, headers })} contentId={`${ruleId}:mock-body`} />
      </Block>
    </div>
  );
}

function withoutKey<T extends object, K extends keyof T>(obj: T, key: K): Omit<T, K> & T {
  const copy = { ...obj };
  delete copy[key];
  return copy;
}

function ForwardForm({ action, onChange, ruleId }: { action: Extract<Action, { type: 'forward' }>; onChange: (a: Action) => void; ruleId: string }) {
  const targetErr = useIssue('action.target');
  const prefixErr = useIssue('action.stripPrefix');
  const touch = useTouch();
  return (
    <div className={styles.stack}>
      <Block label="Target" hint="Base URL or host[:port]" error={targetErr} htmlFor={`${ruleId}-target`}>
        <Input id={`${ruleId}-target`} mono value={action.target} invalid={!!targetErr} placeholder="https://staging.example.com" onChange={(e) => onChange({ ...action, target: e.target.value })} onBlur={() => touch('action.target')} spellCheck={false} />
      </Block>
      <div className={styles.twoCol}>
        <Block label="Strip path prefix" hint="Optional" error={prefixErr}>
          <Input mono value={action.stripPrefix ?? ''} invalid={!!prefixErr} placeholder="/api" onChange={(e) => onChange(e.target.value ? { ...action, stripPrefix: e.target.value } : withoutKey(action, 'stripPrefix'))} spellCheck={false} />
        </Block>
        <Block label="Host header">
          <Switch checked={action.preserveHost} onCheckedChange={(preserveHost) => onChange({ ...action, preserveHost })} label="Keep the original Host" description={action.preserveHost ? 'Upstream sees the original Host header' : 'Rewritten to the target host (recommended)'} />
        </Block>
      </div>
      <Explain>
        A request to <code>/api/users</code> {action.stripPrefix ? <>loses <code>{action.stripPrefix}</code> and </> : null}goes to <code className={styles.codeStrong}>{previewForward(action.target, action.stripPrefix)}</code>, keeping its method, query and body.
      </Explain>
      <details className={styles.disclosure} open={!!(action.request || action.response)}>
        <summary>Also rewrite the request or response</summary>
        <TransformEditor request={action.request} response={action.response} onChange={(request, response) => onChange({ ...withoutKey(withoutKey(action, 'request'), 'response'), ...(request ? { request } : {}), ...(response ? { response } : {}) })} path="action" ruleId={ruleId} />
      </details>
    </div>
  );
}

function previewForward(target: string, strip: string | undefined): string {
  const t = target.trim().replace(/\/+$/, '') || 'https://target';
  let path = '/api/users';
  if (strip && path.startsWith(strip)) path = path.slice(strip.length) || '/';
  return `${t}${path.startsWith('/') ? path : `/${path}`}`;
}

function MapLocalForm({ action, onChange }: { action: Extract<Action, { type: 'map-local' }>; onChange: (a: Action) => void }) {
  const pathErr = useIssue('action.path');
  const touch = useTouch();
  return (
    <div className={styles.stack}>
      <Block label="File or folder" error={pathErr} htmlFor="map-local-path">
        <div className={styles.pathRow}>
          <Input id="map-local-path" mono value={action.path} invalid={!!pathErr} placeholder="/Users/me/project/dist" onChange={(e) => onChange({ ...action, path: e.target.value })} onBlur={() => touch('action.path')} spellCheck={false} />
          <PathPicker pick="either" title="Serve this file or folder" onPick={(path) => onChange({ ...action, path })} />
        </div>
      </Block>
      <div className={styles.twoCol}>
        <Block label="Strip path prefix" hint="Optional">
          <Input mono value={action.stripPrefix ?? ''} placeholder="/assets" onChange={(e) => onChange(e.target.value ? { ...action, stripPrefix: e.target.value } : withoutKey(action, 'stripPrefix'))} spellCheck={false} />
        </Block>
        <Block label="Index file" hint="For folder requests">
          <Input mono value={action.index ?? ''} placeholder="index.html" onChange={(e) => onChange(e.target.value ? { ...action, index: e.target.value } : withoutKey(action, 'index'))} spellCheck={false} />
        </Block>
      </div>
      <Switch
        checked={action.fallbackToUpstream}
        onCheckedChange={(fallbackToUpstream) => onChange({ ...action, fallbackToUpstream })}
        label="Fall back to the server"
        description={action.fallbackToUpstream ? 'Missing files are fetched from the real server' : 'Missing files get a 404'}
      />
      <Block label="Extra response headers">
        <HeadersField headers={action.headers} onChange={(headers) => onChange({ ...action, headers })} path="action.headers" />
      </Block>
      <Explain>Files are read on every request and served with a Content-Type guessed from the extension. When the path is a folder, the request path (minus the prefix) is resolved inside it.</Explain>
    </div>
  );
}

function BreakpointForm({ action, onChange }: { action: Extract<Action, { type: 'breakpoint' }>; onChange: (a: Action) => void }) {
  const err = useIssue('action.request');
  return (
    <div className={styles.stack}>
      <div className={styles.checkCards}>
        <label className={clsx(styles.checkCard, action.request && styles.checkCardOn)}>
          <Checkbox checked={action.request} onCheckedChange={(request) => onChange({ ...action, request })} aria-label="Pause the request" />
          <span>
            <span className={styles.checkTitle}>Request</span>
            <span className={styles.checkDesc}>Before it is sent — edit the method, URL, headers or body, or answer it yourself.</span>
          </span>
        </label>
        <label className={clsx(styles.checkCard, action.response && styles.checkCardOn)}>
          <Checkbox checked={action.response} onCheckedChange={(response) => onChange({ ...action, response })} aria-label="Pause the response" />
          <span>
            <span className={styles.checkTitle}>Response</span>
            <span className={styles.checkDesc}>Before the client receives it — edit the status, headers or body.</span>
          </span>
        </label>
      </div>
      <FieldError>{err}</FieldError>
      <Explain icon={<CirclePause size={13} aria-hidden />}>Paused exchanges appear in Traffic with an amber badge; edit them there and resume with <Kbd combo="mod+enter" size="sm" />.</Explain>
    </div>
  );
}

const WS_PATTERN_OPS = ['contains', 'equals', 'starts-with', 'ends-with', 'regex'] as const;

function WsBreakpointForm({ action, onChange }: { action: Extract<Action, { type: 'ws-breakpoint' }>; onChange: (a: Action) => void }) {
  const filtered = action.pattern != null;
  return (
    <div className={styles.stack}>
      <Block label="Messages">
        <SegmentedControl<WsBreakpointDirection>
          value={action.direction}
          onValueChange={(direction) => onChange({ ...action, direction })}
          items={[
            { value: 'both', icon: ArrowDownUp, label: 'Both directions' },
            { value: 'sent', icon: ArrowUp, label: 'Sent', tooltip: 'Client → server' },
            { value: 'received', icon: ArrowDown, label: 'Received', tooltip: 'Server → client' },
          ]}
          aria-label="Which messages to pause"
        />
      </Block>
      <Switch
        checked={filtered}
        onCheckedChange={(on) => onChange(on ? { ...action, pattern: { op: 'contains', value: '', caseSensitive: false } } : withoutKey(action, 'pattern'))}
        label="Only messages whose payload matches"
        description={filtered ? 'Binary payloads are matched as (lossy) UTF-8 text' : 'Every text and binary message is paused'}
      />
      {action.pattern && <PatternEditor pattern={action.pattern} path="action.pattern" placeholder={'"type":"auth"'} ops={WS_PATTERN_OPS} mono onChange={(pattern) => onChange({ ...action, pattern })} />}
      <Explain icon={<MessagesSquare size={13} aria-hidden />}>
        Applies to WebSocket upgrades matching the conditions above. Paused messages show up in the exchange's <strong>Messages</strong> tab: edit, drop or forward each one (<Kbd combo="mod+enter" size="sm" /> forwards). Only that direction waits — pings and the other direction keep flowing. Disabling the rule stops pausing open connections too.
      </Explain>
    </div>
  );
}

function DelayForm({ action, onChange }: { action: Extract<Action, { type: 'delay' }>; onChange: (a: Action) => void }) {
  const lo = action.ms;
  const hi = action.ms + action.jitterMs;
  return (
    <div className={styles.stack}>
      <div className={styles.twoCol}>
        <Block label="Delay">
          <NumberInput value={action.ms} onValueChange={(v) => onChange({ ...action, ms: v ?? 0 })} min={0} max={600_000} step={100} unit="ms" stepper />
        </Block>
        <Block label="Random jitter" hint="Optional">
          <NumberInput value={action.jitterMs} onValueChange={(v) => onChange({ ...action, jitterMs: v ?? 0 })} min={0} max={600_000} step={100} unit="ms" stepper />
        </Block>
      </div>
      <Explain>
        Each matching request waits <strong>{action.jitterMs > 0 ? `${formatDuration(lo)} – ${formatDuration(hi)}` : formatDuration(lo)}</strong> before it is handled. Delays from several matching rules add up.
      </Explain>
    </div>
  );
}

function ThrottleForm({ action, onChange }: { action: Extract<Action, { type: 'throttle' }>; onChange: (a: Action) => void }) {
  const preset = detectPreset(action);
  return (
    <div className={styles.stack}>
      <SegmentedControl<ThrottlePresetId>
        value={preset}
        onValueChange={(p) => {
          if (p !== 'custom') onChange({ ...action, ...presetValues(p) });
        }}
        items={[...THROTTLE_PRESETS.map((p) => ({ value: p.id, label: p.label })), { value: 'custom' as const, label: 'Custom' }]}
        aria-label="Network preset"
      />
      <div className={styles.threeCol}>
        <Block label="Download">
          <NumberInput value={bpsToKbps(action.downloadBps)} onValueChange={(v) => onChange({ ...action, downloadBps: kbpsToBps(v ?? 0) })} min={0} step={100} unit="kbit/s" />
        </Block>
        <Block label="Upload">
          <NumberInput value={bpsToKbps(action.uploadBps)} onValueChange={(v) => onChange({ ...action, uploadBps: kbpsToBps(v ?? 0) })} min={0} step={100} unit="kbit/s" />
        </Block>
        <Block label="Latency">
          <NumberInput value={action.latencyMs} onValueChange={(v) => onChange({ ...action, latencyMs: v ?? 0 })} min={0} step={10} unit="ms" />
        </Block>
      </div>
      <Explain>0 means unlimited. When several throttle rules match, the most restrictive values win.</Explain>
    </div>
  );
}

const COLOR_LABELS: Record<TagColor, string> = { red: 'Red', orange: 'Orange', yellow: 'Yellow', green: 'Green', blue: 'Blue', purple: 'Purple', grey: 'Grey' };

function HighlightForm({ action, onChange }: { action: Extract<Action, { type: 'highlight' }>; onChange: (a: Action) => void }) {
  return (
    <div className={styles.stack}>
      <Block label="Colour">
        <div className={styles.swatches} role="radiogroup" aria-label="Highlight colour" onKeyDown={onRadioGroupKeyDown}>
          {TAG_COLORS.map((c, i) => (
            <button
              key={c}
              type="button"
              role="radio"
              aria-checked={action.color === c}
              tabIndex={radioTabIndex(action.color === c, i === 0, (TAG_COLORS as readonly string[]).includes(action.color))}
              className={clsx(styles.swatch, action.color === c && styles.swatchOn)}
              style={{ '--sw': tagColorVar(c) } as CSSProperties}
              onClick={() => onChange({ ...action, color: c })}
            >
              <span className={styles.swatchDot} aria-hidden />
              {COLOR_LABELS[c]}
            </button>
          ))}
        </div>
      </Block>
      <Block label="Note" hint="Shown on matching exchanges">
        <Input value={action.note ?? ''} placeholder="e.g. Payments" onChange={(e) => onChange(e.target.value ? { ...action, note: e.target.value } : withoutKey(action, 'note'))} />
      </Block>
      <HighlightPreview color={action.color} note={action.note} />
    </div>
  );
}

function HighlightPreview({ color, note }: { color: string; note?: string }): ReactNode {
  return (
    <div className={styles.hlPreview} style={{ '--sw': tagColorVar(color) } as CSSProperties} aria-hidden>
      <span className={styles.hlStripe} />
      <span className={styles.hlMethod}>POST</span>
      <span className={styles.hlStatus}>200</span>
      <span className={clsx(styles.hlUrl, 'truncate')}>api.stripe.com/v1/payment_intents</span>
      {note && <span className={styles.hlNote}>{note}</span>}
    </div>
  );
}

function ScriptForm({ action, onChange, ruleId, onTest }: { action: Extract<Action, { type: 'script' }>; onChange: (a: Action) => void; ruleId: string; onTest: () => void }) {
  const err = useIssue('action.source');
  const licensed = useFeature('scripting');
  return (
    <div className={styles.stack}>
      {!licensed && (
        <ProCallout feature="scripting" title="Script rules need Akev Pro">
          This rule is saved but inactive: matching requests pass through unchanged until Pro is active. You can keep editing and testing the script.
        </ProCallout>
      )}
      <div className={clsx(styles.codeFrame, err && styles.codeFrameInvalid)}>
        <CodeEditor value={action.source} onChange={(source) => onChange({ ...action, source })} language="javascript" minHeight={260} maxHeight={560} contentId={`${ruleId}:script`} aria-label="Script source" transparent />
      </div>
      <FieldError>{err}</FieldError>
      <div className={styles.scriptBar}>
        <Button size="sm" icon={FlaskConical} onClick={onTest}>
          Test with selected exchange
        </Button>
        <Button size="sm" variant="ghost" icon={RotateCcw} onClick={() => onChange({ ...action, source: SCRIPT_TEMPLATE })} disabled={action.source === SCRIPT_TEMPLATE}>
          Reset to template
        </Button>
        <span className={styles.scriptHint}>QuickJS sandbox · 2 s CPU · 64 MB · no network or file access</span>
      </div>
    </div>
  );
}
