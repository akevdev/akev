// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus; focus goes to the inputs in them
import clsx from 'clsx';
import { CircleAlert, CircleCheck, FilePlus2, FileUp, Info, Paperclip, Plus, RefreshCw, Trash2, Type, WandSparkles, X } from 'lucide-react';
import { memo, useMemo, useRef, useState, type DragEvent } from 'react';
import { Button, Checkbox, CodeEditor, EmptyState, IconButton, KeyValueEditor, SegmentedControl, Select, Tooltip, makeRow, toast, type KeyValueRow } from '../../components/ui';
import { formatBytes } from '../../lib/format';
import { methodHasBody } from '../../lib/http';
import { pickFiles } from '../../lib/platform';
import { PERSIST_FILE_LIMIT, TEXT_TYPES, type BodyMode, type BodyState, type FileRef, type MultipartRow } from './model';
import { checkJson, fileRef, formatJson, serializeBody } from './request';
import { substitute, type VarMap } from './variables';
import styles from './RequestEditor.module.css';

const MODE_ITEMS: readonly { value: BodyMode; label: string }[] = [
  { value: 'none', label: 'None' },
  { value: 'json', label: 'JSON' },
  { value: 'text', label: 'Text' },
  { value: 'form', label: 'Form' },
  { value: 'multipart', label: 'Multipart' },
  { value: 'binary', label: 'File' },
];

export const BODY_MODE_LABEL: Record<BodyMode, string> = { none: 'None', json: 'JSON', text: 'Text', form: 'Form', multipart: 'Multipart', binary: 'File' };

export interface BodyEditorProps {
  tabId: string;
  body: BodyState;
  method: string;
  vars: VarMap;
  onBody: (update: (b: BodyState) => BodyState) => void;
  onSubmit: () => void;
  onSave: () => void;
}

async function readDropped(e: DragEvent): Promise<FileRef[]> {
  const files = Array.from(e.dataTransfer.files);
  return Promise.all(files.map(async (f) => fileRef(f.name, new Uint8Array(await f.arrayBuffer()), f.type || undefined)));
}

/** Body tab: mode switcher + the matching editor (JSON/Text/Form/Multipart/File). */
export const BodyEditor = memo(function BodyEditor({ tabId, body, method, vars, onBody, onSubmit, onSave }: BodyEditorProps) {
  const json = body.mode === 'json' ? checkJson(body.json) : null;
  const size = useMemo(() => (body.mode === 'none' ? 0 : serializeBody(body, (s) => substitute(s, vars).text).size), [body, vars]);
  const textType = TEXT_TYPES.find((t) => t.value === body.textType);

  const format = () => {
    const pretty = formatJson(body.json);
    if (pretty == null) {
      toast.error('Fix the JSON before formatting', { description: json && !json.ok ? `Line ${json.line}, column ${json.column}: ${json.message}` : undefined });
      return;
    }
    onBody((b) => ({ ...b, json: pretty }));
  };

  return (
    <div className={styles.bodyRoot}>
      <div className={styles.bodyBar}>
        <SegmentedControl size="sm" value={body.mode} onValueChange={(mode) => onBody((b) => ({ ...b, mode }))} items={MODE_ITEMS} aria-label="Body type" />
        <div className={styles.bodyBarRight}>
          {json && body.json.trim() !== '' && (
            <Tooltip content={json.ok ? 'Variables outside strings are allowed' : `Line ${json.line}, column ${json.column}`}>
              {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
              <span className={clsx(styles.jsonStatus, json.ok ? styles.jsonOk : styles.jsonBad)} tabIndex={0}>
                {json.ok ? <CircleCheck size={13} strokeWidth={2} aria-hidden /> : <CircleAlert size={13} strokeWidth={2} aria-hidden />}
                <span className={styles.jsonStatusText}>{json.ok ? 'Valid JSON' : `${json.message} · ${json.line}:${json.column}`}</span>
              </span>
            </Tooltip>
          )}
          {body.mode === 'json' && (
            <Tooltip content="Format JSON" shortcut="shift+alt+f">
              <Button size="sm" variant="ghost" onClick={format} disabled={!body.json.trim()} aria-label="Format JSON">
                <span className={styles.btnInner}>
                  <WandSparkles size={14} strokeWidth={1.75} aria-hidden />
                  <span className={styles.collapsible}>Format</span>
                </span>
              </Button>
            </Tooltip>
          )}
          {body.mode === 'text' && (
            <Select size="sm" variant="ghost" value={body.textType} onValueChange={(textType) => onBody((b) => ({ ...b, textType }))} options={TEXT_TYPES.map((t) => ({ value: t.value, label: t.label, description: t.value }))} aria-label="Text content type" renderValue={() => textType?.label ?? body.textType} />
          )}
          {body.mode !== 'none' && <span className={clsx(styles.bodySize, 'tnum')}>{formatBytes(size)}</span>}
        </div>
      </div>
      {body.mode !== 'none' && !methodHasBody(method) && (
        <div className={styles.notice}>
          <Info size={13} strokeWidth={2} aria-hidden />
          <span>{method.toUpperCase()} requests don't usually carry a body — some servers ignore or reject it.</span>
        </div>
      )}
      <div className={styles.bodyContent}>
        {body.mode === 'none' && (
          <EmptyState
            size="sm"
            icon={Type}
            title="No body"
            description="Pick a body type above. Content is kept per type, so switching never loses work."
            actions={
              <>
                <Button size="sm" onClick={() => onBody((b) => ({ ...b, mode: 'json' }))}>
                  JSON
                </Button>
                <Button size="sm" onClick={() => onBody((b) => ({ ...b, mode: 'form' }))}>
                  Form
                </Button>
              </>
            }
          />
        )}
        {body.mode === 'json' && (
          <CodeEditor
            value={body.json}
            onChange={(v) => onBody((b) => ({ ...b, json: v }))}
            language="json"
            fill
            placeholder={'{\n  "name": "Akev",\n  "id": {{id}}\n}'}
            contentId={`${tabId}:json`}
            onSubmit={onSubmit}
            onSave={onSave}
            aria-label="JSON body"
          />
        )}
        {body.mode === 'text' && (
          <CodeEditor
            value={body.text}
            onChange={(v) => onBody((b) => ({ ...b, text: v }))}
            language={textType?.language ?? 'text'}
            fill
            wrap
            placeholder="Request body"
            contentId={`${tabId}:text`}
            onSubmit={onSubmit}
            onSave={onSave}
            aria-label="Text body"
          />
        )}
        {body.mode === 'form' && (
          <div className={styles.scrollPad}>
            <KeyValueEditor rows={body.form} onChange={(form) => onBody((b) => ({ ...b, form }))} separator="=" namePlaceholder="Field" valuePlaceholder="Value" aria-label="Form fields" />
            <p className={styles.footnote}>Sent as <code>application/x-www-form-urlencoded</code>; values are encoded for you.</p>
          </div>
        )}
        {body.mode === 'multipart' && (
          <div className={styles.scrollPad}>
            <MultipartEditor rows={body.multipart} onChange={(multipart) => onBody((b) => ({ ...b, multipart }))} />
          </div>
        )}
        {body.mode === 'binary' && <FileBody file={body.binary} onChange={(binary) => onBody((b) => ({ ...b, binary }))} />}
      </div>
    </div>
  );
});

// =====================================================================================
// File body
// =====================================================================================

function FileBody({ file, onChange }: { file: FileRef | null; onChange: (f: FileRef | null) => void }) {
  const [over, setOver] = useState(false);
  const depth = useRef(0);
  const choose = async () => {
    const files = await pickFiles();
    const f = files?.[0];
    if (f) onChange(fileRef(f.name, f.data));
  };
  const dropHandlers = {
    onDragEnter: (e: DragEvent) => {
      e.preventDefault();
      depth.current++;
      setOver(true);
    },
    onDragOver: (e: DragEvent) => e.preventDefault(),
    onDragLeave: () => {
      depth.current = Math.max(0, depth.current - 1);
      if (depth.current === 0) setOver(false);
    },
    onDrop: async (e: DragEvent) => {
      e.preventDefault();
      depth.current = 0;
      setOver(false);
      const [f] = await readDropped(e);
      if (f) onChange(f);
    },
  };

  if (!file) {
    return (
      <div className={styles.scrollPad}>
        <button type="button" className={clsx(styles.dropZone, over && styles.dropOver)} onClick={choose} {...dropHandlers}>
          <span className={styles.dropIcon}>
            <FileUp size={20} strokeWidth={1.6} aria-hidden />
          </span>
          <span className={styles.dropTitle}>Drop a file or click to choose</span>
          <span className={styles.dropHint}>Sent as the raw request body. The Content-Type follows the file type.</span>
        </button>
      </div>
    );
  }
  const lost = !file.data && file.size > 0;
  return (
    <div className={styles.scrollPad} {...dropHandlers}>
      <div className={clsx(styles.fileCard, over && styles.dropOver, lost && styles.fileLost)}>
        <span className={styles.fileIcon}>
          <Paperclip size={16} strokeWidth={1.75} aria-hidden />
        </span>
        <div className={styles.fileText}>
          <span className={styles.fileName}>{file.name}</span>
          <span className={styles.fileMeta}>
            {file.type || 'application/octet-stream'} · <span className="tnum">{formatBytes(file.size)}</span>
          </span>
        </div>
        <Button size="sm" icon={RefreshCw} onClick={choose}>
          Replace
        </Button>
        <IconButton size="sm" label="Remove file" icon={X} onClick={() => onChange(null)} />
      </div>
      {lost ? (
        <p className={clsx(styles.footnote, styles.footnoteWarn)}>
          <CircleAlert size={12} strokeWidth={2} aria-hidden /> This file was too large to keep after a reload — choose it again before sending.
        </p>
      ) : file.size > PERSIST_FILE_LIMIT ? (
        <p className={styles.footnote}>Files over {formatBytes(PERSIST_FILE_LIMIT)} aren't kept after a reload.</p>
      ) : null}
    </div>
  );
}

// =====================================================================================
// Multipart
// =====================================================================================

function MultipartEditor({ rows, onChange }: { rows: readonly MultipartRow[]; onChange: (rows: MultipartRow[]) => void }) {
  const update = (id: string, patch: Partial<MultipartRow>) => onChange(rows.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  const add = (kind: 'text' | 'file') => onChange([...rows, { ...makeRow(), kind, file: null }]);
  const attach = async (id: string) => {
    const files = await pickFiles();
    const f = files?.[0];
    if (f) update(id, { file: fileRef(f.name, f.data), kind: 'file' });
  };
  const onDrop = async (e: DragEvent) => {
    e.preventDefault();
    const files = await readDropped(e);
    if (files.length) onChange([...rows, ...files.map((file) => ({ ...makeRow(file.name.replace(/\.[^.]+$/, '') || 'file', ''), kind: 'file' as const, file }))]);
  };

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: drop zone for files; each part also has a file picker button
    <div className={styles.stack} onDragOver={(e) => e.preventDefault()} onDrop={onDrop}>
      {rows.length > 0 && (
        <div className={styles.mpTable} role="table" aria-label="Multipart fields">
          {rows.map((r) => (
            <div key={r.id} role="row" className={clsx(styles.mpRow, !r.enabled && styles.mpDisabled)}>
              <span role="cell" className={styles.autoToggle}>
                <Checkbox checked={r.enabled} onCheckedChange={(c) => update(r.id, { enabled: c })} aria-label={`Send ${r.name || 'field'}`} />
              </span>
              <span role="cell" className={styles.mpCell}>
                <input className={styles.mpInput} value={r.name} placeholder="Name" spellCheck={false} onChange={(e) => update(r.id, { name: e.target.value })} aria-label="Field name" />
              </span>
              <span role="cell" className={clsx(styles.mpCell, styles.mpKind)}>
                <SegmentedControl
                  size="sm"
                  value={r.kind}
                  onValueChange={(kind) => update(r.id, { kind })}
                  items={[
                    { value: 'text', icon: Type, tooltip: 'Text value', 'aria-label': 'Text' },
                    { value: 'file', icon: Paperclip, tooltip: 'File', 'aria-label': 'File' },
                  ]}
                  aria-label="Field type"
                />
              </span>
              <span role="cell" className={styles.mpCell}>
                {r.kind === 'text' ? (
                  <input className={clsx(styles.mpInput, styles.mpValue)} value={r.value} placeholder="Value" spellCheck={false} onChange={(e) => update(r.id, { value: e.target.value })} aria-label="Field value" />
                ) : (
                  <button type="button" className={styles.mpFile} onClick={() => attach(r.id)}>
                    <Paperclip size={12} strokeWidth={2} aria-hidden />
                    {r.file ? (
                      <>
                        <span className={styles.mpFileName}>{r.file.name}</span>
                        <span className={clsx(styles.mpFileSize, 'tnum')}>{r.file.data ? formatBytes(r.file.size) : 'pick again'}</span>
                      </>
                    ) : (
                      <span className={styles.mpFilePlaceholder}>Choose file…</span>
                    )}
                  </button>
                )}
              </span>
              <span role="cell" className={styles.mpActions}>
                <IconButton size="sm" label="Remove field" icon={Trash2} variant="danger" onClick={() => onChange(rows.filter((x) => x.id !== r.id))} />
              </span>
            </div>
          ))}
        </div>
      )}
      <div className={styles.mpButtons}>
        <Button size="sm" icon={Plus} onClick={() => add('text')}>
          Add field
        </Button>
        <Button size="sm" icon={FilePlus2} onClick={() => add('file')}>
          Add file
        </Button>
        <span className={styles.footnote}>Or drop files here. Sent as <code>multipart/form-data</code>.</span>
      </div>
    </div>
  );
}

/** Rows for the Form/Params editors after a change (helper for typing). */
export type Rows = KeyValueRow[];
