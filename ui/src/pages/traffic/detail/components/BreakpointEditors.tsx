import clsx from 'clsx';
import { Braces, FileUp, Info, PencilLine } from 'lucide-react';
import { useMemo, type CSSProperties } from 'react';
import { base64ToBytes, utf8DecodeStrict } from '../../../../lib/encoding';
import { formatBytes } from '../../../../lib/format';
import { KNOWN_HEADER_NAMES } from '../../../../lib/headerDocs';
import { HTTP_METHODS, statusClass, statusColorVar, statusText } from '../../../../lib/http';
import { pickFiles } from '../../../../lib/platform';
import {
  Badge,
  Button,
  CodeEditor,
  Combobox,
  IconButton,
  Input,
  KeyValueEditor,
  NumberInput,
  SegmentedControl,
  Tooltip,
  toast,
  type ComboboxOption,
  type KeyValueRow,
} from '../../../../components/ui';
import { bytesToBase64 } from '../../../../lib/encoding';
import { useLastTab } from '../../../../state/ui';
import { editableBodySize, formatJsonText, languageForContentType, nextStatusMessage, rowHeader } from '../lib/breakpoint';
import { withBody, withUrl, type BodyDraft, type DraftErrors, type RequestDraft, type ResponseDraft } from '../hooks/useBreakpointDraft';
import { SectionPanel, SectionTabs } from './SectionTabs';
import styles from './BreakpointEditors.module.css';

const METHOD_OPTIONS: ComboboxOption[] = HTTP_METHODS.map((m) => ({ value: m }));

type EditTab = 'headers' | 'body';

function EditingBadge() {
  return (
    <Badge size="sm" variant="warning" icon={PencilLine} className={styles.editing}>
      Editing
    </Badge>
  );
}

// =====================================================================================
// Request
// =====================================================================================

export interface RequestEditorProps {
  draft: RequestDraft;
  onChange: (update: (d: RequestDraft) => RequestDraft) => void;
  errors: DraftErrors;
  onSubmit: () => void;
}

/** Paused at request: method, URL, headers and body are editable (DESIGN.md §6.6). */
export function RequestEditor({ draft, onChange, errors, onSubmit }: RequestEditorProps) {
  const [tab, setTab] = useLastTab('traffic.breakpoint.request', 'headers');
  const size = editableBodySize(draft.body.mode, draft.body.value);
  const headerCount = draft.rows.filter((r) => r.enabled && r.name.trim()).length;

  return (
    <SectionTabs<EditTab>
      label="Request"
      badge={<EditingBadge />}
      value={tab as EditTab}
      onValueChange={setTab}
      items={[
        { value: 'headers', label: 'Headers', count: headerCount },
        { value: 'body', label: 'Body', badge: size ? formatBytes(size) : undefined },
      ]}
      toolbar={
        <div className={styles.lineBar}>
          <Combobox
            value={draft.method}
            onValueChange={(method) => onChange((d) => ({ ...d, method: method.toUpperCase() }))}
            options={METHOD_OPTIONS}
            filter={false}
            mono
            size="sm"
            className={styles.method}
            invalid={!!errors.method}
            title={errors.method}
            aria-label="Method"
            spellCheck={false}
          />
          <Input
            value={draft.url}
            onChange={(e) => {
              const url = e.target.value;
              onChange((d) => withUrl(d, url));
            }}
            mono
            size="sm"
            className={styles.url}
            invalid={!!errors.url}
            title={errors.url}
            aria-label="URL"
            spellCheck={false}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                onSubmit();
              }
            }}
          />
        </div>
      }
    >
      <SectionPanel value="headers" scroll>
        <HeadersEditor rows={draft.rows} onChange={(rows) => onChange((d) => ({ ...d, rows }))} error={errors.headers} note="Content-Length and Host follow your body and URL edits automatically." />
      </SectionPanel>
      <SectionPanel value="body">
        <BodyEditor body={draft.body} onChange={(body) => onChange((d) => withBody(d, body))} contentType={rowHeader(draft.rows, 'content-type')} error={errors.body} onSubmit={onSubmit} />
      </SectionPanel>
    </SectionTabs>
  );
}

// =====================================================================================
// Response
// =====================================================================================

export interface ResponseEditorProps {
  draft: ResponseDraft;
  onChange: (update: (d: ResponseDraft) => ResponseDraft) => void;
  errors: DraftErrors;
  onSubmit: () => void;
  /** Section label. */
  label?: string;
  tabGroup?: string;
  /** Inside the Respond dialog: no label / "Editing" badge, top border. */
  compact?: boolean;
  /** id for the status input (autofocus target). */
  statusInputId?: string;
}

/** Paused at response (or the Respond… dialog): status, message, headers and body. */
export function ResponseEditor({ draft, onChange, errors, onSubmit, label = 'Response', tabGroup = 'traffic.breakpoint.response', compact, statusInputId }: ResponseEditorProps) {
  const [tab, setTab] = useLastTab(tabGroup, 'body');
  const size = editableBodySize(draft.body.mode, draft.body.value);
  const headerCount = draft.rows.filter((r) => r.enabled && r.name.trim()).length;

  return (
    <SectionTabs<EditTab>
      label={label}
      hideLabel={compact}
      badge={compact ? undefined : <EditingBadge />}
      className={compact ? styles.compact : undefined}
      toolbarPlacement={compact ? 'above' : 'below'}
      headClassName={compact ? styles.compactHead : undefined}
      value={tab as EditTab}
      onValueChange={setTab}
      items={[
        { value: 'headers', label: 'Headers', count: headerCount },
        { value: 'body', label: 'Body', badge: size ? formatBytes(size) : undefined },
      ]}
      toolbar={
        <div className={clsx(styles.lineBar, compact && styles.lineBarCompact)}>
          {compact && (
            <label className={styles.lineLabel} htmlFor={statusInputId}>
              Status
            </label>
          )}
          <NumberInput
            id={statusInputId}
            value={draft.status}
            onValueChange={(status) => onChange((d) => ({ ...d, status, message: nextStatusMessage(d.status, d.message, status) }))}
            min={100}
            max={999}
            allowEmpty
            size="sm"
            mono
            className={styles.status}
            invalid={!!errors.status}
            title={errors.status}
            aria-label="Status code"
          />
          <StatusDot status={draft.status} />
          <Input
            value={draft.message}
            onChange={(e) => onChange((d) => ({ ...d, message: e.target.value }))}
            size="sm"
            className={styles.url}
            placeholder={draft.status != null ? statusText(draft.status) || 'Reason phrase' : 'Reason phrase'}
            aria-label="Status message"
          />
        </div>
      }
    >
      <SectionPanel value="headers" scroll>
        <HeadersEditor rows={draft.rows} onChange={(rows) => onChange((d) => ({ ...d, rows }))} error={errors.headers} />
      </SectionPanel>
      <SectionPanel value="body">
        <BodyEditor body={draft.body} onChange={(body) => onChange((d) => withBody(d, body))} contentType={rowHeader(draft.rows, 'content-type')} error={errors.body} onSubmit={onSubmit} />
      </SectionPanel>
    </SectionTabs>
  );
}

// =====================================================================================
// Pieces
// =====================================================================================

const CLASS_NAMES: Record<string, string> = { '1xx': 'Informational', '2xx': 'Success', '3xx': 'Redirect', '4xx': 'Client error', '5xx': 'Server error' };

/** Colour of the status class next to the status input (live while typing). */
function StatusDot({ status }: { status: number | null }) {
  const valid = status != null && status >= 100 && status <= 999;
  const cls = valid ? statusClass(status) : null;
  const name = cls ? (CLASS_NAMES[cls] ?? 'Non-standard status') : 'Enter a status code';
  return (
    <Tooltip content={name}>
      <span className={styles.statusDot} style={{ '--dot': valid ? statusColorVar(status) : 'var(--border-strong)' } as CSSProperties} role="img" aria-label={name} />
    </Tooltip>
  );
}

function HeadersEditor({ rows, onChange, error, note = 'Content-Length follows the body automatically.' }: { rows: KeyValueRow[]; onChange: (rows: KeyValueRow[]) => void; error?: string; note?: string }) {
  return (
    <div className={styles.headers}>
      <KeyValueEditor rows={rows} onChange={onChange} nameSuggestions={KNOWN_HEADER_NAMES} namePlaceholder="Header" aria-label="Headers" />
      {error ? (
        <p className={clsx(styles.note, styles.noteError)}>{error}</p>
      ) : (
        <p className={styles.note}>
          <Info size={12} strokeWidth={2} aria-hidden />
          {note}
        </p>
      )}
    </div>
  );
}

export interface BodyEditorProps {
  body: BodyDraft;
  onChange: (body: BodyDraft) => void;
  contentType: string | undefined;
  error?: string;
  onSubmit: () => void;
}

export function BodyEditor({ body, onChange, contentType, error, onSubmit }: BodyEditorProps) {
  const language = languageForContentType(contentType);
  const size = editableBodySize(body.mode, body.value);
  const canFormat = body.mode === 'text' && language === 'json' && body.value.trim() !== '';
  const textFromBase64 = useMemo(() => {
    if (body.mode !== 'base64') return null;
    try {
      return utf8DecodeStrict(base64ToBytes(body.value.replace(/\s+/g, '')));
    } catch {
      return null;
    }
  }, [body.mode, body.value]);

  const setMode = (mode: 'text' | 'base64') => {
    if (mode === body.mode) return;
    if (mode === 'base64') {
      onChange({ ...body, mode, value: body.value ? bytesToBase64(new TextEncoder().encode(body.value)) : '' });
    } else if (textFromBase64 != null) {
      onChange({ ...body, mode, value: textFromBase64 });
    }
  };

  const loadFile = async () => {
    const files = await pickFiles();
    const file = files?.[0];
    if (!file) return;
    const text = utf8DecodeStrict(file.data);
    onChange(text != null ? { mode: 'text', value: text, binary: false } : { mode: 'base64', value: bytesToBase64(file.data), binary: true });
    toast.success('Body replaced', { description: `${file.name} · ${formatBytes(file.data.length)}` });
  };

  return (
    <div className={styles.body}>
      <div className={styles.bodyBar}>
        <SegmentedControl
          size="sm"
          value={body.mode}
          onValueChange={setMode}
          aria-label="Body encoding"
          items={[
            { value: 'text', label: 'Text', disabled: body.mode === 'base64' && textFromBase64 == null, tooltip: body.mode === 'base64' && textFromBase64 == null ? 'The body is binary — not valid UTF-8' : undefined },
            { value: 'base64', label: 'Base64' },
          ]}
        />
        {canFormat && (
          <Tooltip content="Pretty-print JSON">
            <Button
              size="sm"
              variant="ghost"
              icon={Braces}
              onClick={() => {
                const pretty = formatJsonText(body.value);
                if (pretty != null) onChange({ ...body, value: pretty });
                else toast.error('Not valid JSON', { description: 'Fix the syntax first.' });
              }}
            >
              Format
            </Button>
          </Tooltip>
        )}
        <span className={styles.spacer} />
        {error ? <span className={styles.barError}>{error}</span> : <span className={clsx(styles.barText, 'tnum')}>{size == null ? '—' : formatBytes(size)}</span>}
        <IconButton size="sm" label="Load body from file…" icon={FileUp} onClick={() => void loadFile()} />
      </div>
      {body.binary && body.mode === 'base64' && <p className={styles.binaryNote}>Binary body — edit it as base64. It is sent exactly as decoded from this text.</p>}
      <CodeEditor
        value={body.value}
        onChange={(value) => onChange({ ...body, value })}
        language={body.mode === 'base64' ? 'text' : language}
        wrap
        fill
        placeholder="No body — type or paste one"
        onSubmit={onSubmit}
        aria-label="Body"
      />
    </div>
  );
}
