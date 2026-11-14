// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus
import clsx from 'clsx';
import { ArrowRight, Braces, ChevronRight, CircleAlert, Eye, FileCode2, FolderOpen, Layers, ListTree, Plus, Trash2, Upload } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState, type DragEvent } from 'react';
import { api, errorMessage } from '../../../api/client';
import type { ProtoFileInfo, ProtoMapping, ProtoMappingSide, ProtoRegistry } from '../../../api/types';
import { ProBadge, ProCallout, ProGate } from '../../../components/pro';
import { Badge, Button, CodeEditor, Combobox, confirm, Dialog, EmptyState, IconButton, Input, SearchInput, Select, Skeleton, Switch, toast, Tooltip, undoToast } from '../../../components/ui';
import { forgetBodies } from '../../../components/exchange/body/data';
import { bytesToBase64, utf8Decode } from '../../../lib/encoding';
import { formatBytes, formatNumber, pluralize } from '../../../lib/format';
import { pickFiles, type PickedFile } from '../../../lib/platform';
import {
  countMatches,
  fileMatches,
  isProtoSource,
  MAPPING_SIDES,
  mappingsValid,
  mappingTypeWarning,
  newMapping,
  normalizeMappings,
  PROTO_ACCEPT,
  protoErrorLocation,
  stripCommonRoot,
  summarizeProtoFile,
  summarizeRegistry,
  toUploadFiles,
  validateMappingPattern,
  validateMessageTypeName,
} from '../../../lib/protobuf';
import { useFeature } from '../../../state/license';
import { trafficStore } from '../../../state/traffic';
import { SettingsCard } from '../SettingRow';
import styles from './protobuf.module.css';

/** Most dropped files read from folders (a whole repository dropped by mistake stays cheap). */
const MAX_DROPPED_FILES = 2000;
/** Exchanges a mapping pattern is tried against for its match count. */
const MATCH_SAMPLE = 2000;

// =====================================================================================
// Reading dropped files and folders
// =====================================================================================

async function readEntry(entry: FileSystemEntry, out: PickedFile[]): Promise<void> {
  if (out.length >= MAX_DROPPED_FILES) return;
  if (entry.isFile) {
    const file = await new Promise<File>((ok, err) => (entry as FileSystemFileEntry).file(ok, err));
    const lower = file.name.toLowerCase();
    if (PROTO_ACCEPT.some((ext) => lower.endsWith(ext))) out.push({ name: entry.fullPath.replace(/^\/+/, ''), data: new Uint8Array(await file.arrayBuffer()) });
    return;
  }
  if (entry.isDirectory) {
    const reader = (entry as FileSystemDirectoryEntry).createReader();
    for (;;) {
      const batch = await new Promise<FileSystemEntry[]>((ok, err) => reader.readEntries(ok, err));
      if (!batch.length) break;
      for (const child of batch) await readEntry(child, out);
    }
  }
}

/** Files of a drop, folders included (their relative paths kept). */
async function droppedFiles(e: DragEvent): Promise<PickedFile[]> {
  const entries = Array.from(e.dataTransfer.items ?? [])
    .map((item) => item.webkitGetAsEntry?.() ?? null)
    .filter((x): x is FileSystemEntry => x != null);
  if (entries.length) {
    const out: PickedFile[] = [];
    for (const entry of entries) await readEntry(entry, out);
    if (out.length > 1) {
      const names = stripCommonRoot(out.map((f) => f.name));
      return out.map((f, i) => ({ ...f, name: names[i] ?? f.name }));
    }
    return out.map((f) => ({ ...f, name: f.name.split('/').pop() ?? f.name }));
  }
  return Promise.all(Array.from(e.dataTransfer.files).map(async (f) => ({ name: f.name, data: new Uint8Array(await f.arrayBuffer()) })));
}

// =====================================================================================
// Section
// =====================================================================================

/** Settings → Protobuf schemas: imported `.proto` files / descriptor sets and URL mappings. */
export function ProtobufSection() {
  const [registry, setRegistry] = useState<ProtoRegistry | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const load = useCallback(() => {
    setLoadError(null);
    api.protos.list().then(setRegistry, (e: unknown) => setLoadError(errorMessage(e)));
  }, []);
  useEffect(load, [load]);

  /** Apply a registry returned by a change: body views decode with the new schemas. */
  const applied = useCallback((next: ProtoRegistry) => {
    setRegistry(next);
    forgetBodies();
  }, []);

  if (loadError) {
    return (
      <SettingsCard id="proto-files" title="Schemas">
        <EmptyState icon={CircleAlert} size="sm" tone="danger" title="Couldn't load the schemas" description={loadError} actions={<Button size="sm" onClick={load}>Retry</Button>} />
      </SettingsCard>
    );
  }
  return (
    <>
      <FilesCard registry={registry} onChange={applied} />
      <MappingsCard registry={registry} onChange={applied} />
    </>
  );
}

// =====================================================================================
// Files
// =====================================================================================

function FilesCard({ registry, onChange }: { registry: ProtoRegistry | null; onChange: (r: ProtoRegistry) => void }) {
  const [busy, setBusy] = useState(false);
  const [over, setOver] = useState(false);
  const [query, setQuery] = useState('');
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
  const [viewing, setViewing] = useState<ProtoFileInfo | null>(null);
  const summary = summarizeRegistry(registry);
  const files = registry?.files ?? [];
  const shown = useMemo(() => files.filter((f) => fileMatches(f, query)), [files, query]);
  const licensed = useFeature('proto-schemas');

  const toggle = (name: string) =>
    setExpanded((cur) => {
      const next = new Set(cur);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });

  const upload = async (picked: PickedFile[]) => {
    const { upload: files, skipped } = toUploadFiles(picked);
    if (!files.length) {
      toast.error('No schema files', { description: skipped.length ? `${skipped.slice(0, 3).join(', ')} ${skipped.length === 1 ? "isn't" : "aren't"} .proto files or descriptor sets.` : 'Choose .proto files or descriptor sets (.desc, .pb, .binpb, .protoset).' });
      return;
    }
    setBusy(true);
    try {
      const next = await api.protos.upload(files);
      onChange(next);
      const names = new Set(files.map((f) => f.name));
      const failing = next.files.filter((f) => names.has(f.name) && f.errors.length > 0);
      if (failing.length) {
        setExpanded((cur) => new Set([...cur, ...failing.map((f) => f.name)]));
        toast.warning(`Imported ${pluralize(files.length, 'file')} — ${failing.length} with errors`, { description: `${protoErrorLocation(failing[0]?.errors[0] ?? { file: failing[0]?.name ?? '', line: null, column: null, message: '' })}: ${failing[0]?.errors[0]?.message ?? ''}` });
      } else {
        const added = next.files.filter((f) => names.has(f.name));
        const services = added.reduce((n, f) => n + f.services.length, 0);
        const messages = added.reduce((n, f) => n + f.messages.length, 0);
        toast.success(`Imported ${pluralize(files.length, 'file')}`, { description: [services && pluralize(services, 'service'), messages && pluralize(messages, 'message type')].filter(Boolean).join(' · ') || undefined });
      }
      if (skipped.length) toast(`Skipped ${pluralize(skipped.length, 'file')}`, { description: `${skipped.slice(0, 3).join(', ')}${skipped.length > 3 ? '…' : ''} — not .proto files or descriptor sets.` });
    } catch (e) {
      toast.error("Couldn't import the schemas", { description: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  };

  const choose = async () => {
    const picked = await pickFiles({ accept: [...PROTO_ACCEPT], multiple: true });
    if (picked) await upload(picked);
  };

  const remove = async (file: ProtoFileInfo) => {
    let restore: { name: string; content?: string; base64?: string } | null = null;
    try {
      const bytes = await api.protos.bytes(file.name);
      restore = isProtoSource(file.name) ? { name: file.name, content: utf8Decode(bytes) } : { name: file.name, base64: bytesToBase64(bytes) };
    } catch {
      restore = null;
    }
    try {
      onChange(await api.protos.remove(file.name));
      // Restoring is an upload, which needs Pro.
      const undo = licensed ? restore : null;
      if (undo) undoToast(`Removed ${file.name}`, () => void api.protos.upload([undo]).then(onChange, (e: unknown) => toast.error("Couldn't restore the file", { description: errorMessage(e) })));
      else toast(`Removed ${file.name}`);
    } catch (e) {
      toast.error(`Couldn't remove ${file.name}`, { description: errorMessage(e) });
    }
  };

  const removeAll = async () => {
    const ok = await confirm({ title: 'Remove every schema?', message: `${pluralize(files.length, 'file')} will be removed. Bodies are decoded without names again; URL mappings are kept.`, destructive: true, confirmLabel: 'Remove all' });
    if (!ok) return;
    try {
      onChange(await api.protos.removeAll());
    } catch (e) {
      toast.error("Couldn't remove the schemas", { description: errorMessage(e) });
    }
  };

  const dropProps = {
    onDragEnter: (e: DragEvent) => {
      if (e.dataTransfer.types.includes('Files')) {
        e.preventDefault();
        setOver(true);
      }
    },
    onDragOver: (e: DragEvent) => {
      if (e.dataTransfer.types.includes('Files')) e.preventDefault();
    },
    onDragLeave: (e: DragEvent) => {
      if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
    },
    onDrop: async (e: DragEvent) => {
      if (!e.dataTransfer.types.includes('Files')) return;
      e.preventDefault();
      setOver(false);
      await upload(await droppedFiles(e));
    },
  };

  return (
    <SettingsCard
      id="proto-files"
      title="Schemas"
      description={
        <>
          Import <code>.proto</code> files or descriptor sets (<code>protoc --descriptor_set_out</code>, <code>buf build -o</code>). Akev compiles them and decodes protobuf, gRPC and gRPC-Web bodies with field, enum and message names.
        </>
      }
      actions={
        files.length > 0 ? (
          <Button variant="subtle-danger" size="sm" icon={Trash2} onClick={() => void removeAll()}>
            Remove all
          </Button>
        ) : undefined
      }
    >
      <div id="setting-proto-files" data-setting="proto-files" className={styles.filesBody} {...(licensed ? dropProps : {})}>
        {!licensed ? (
          <ProCallout feature="proto-schemas" title="Your own schemas are part of Akev Pro">
            {files.length > 0
              ? 'The schemas below are kept but not applied until Pro is active: protobuf and gRPC bodies are decoded without names meanwhile.'
              : 'Protobuf and gRPC bodies are still decoded without a schema. Import .proto files or descriptor sets to see real field, enum and message names.'}
          </ProCallout>
        ) : (
          <div className={clsx(styles.drop, over && styles.dropOver, files.length > 0 && styles.dropCompact)} aria-busy={busy || undefined}>
            <Upload size={files.length ? 16 : 22} strokeWidth={1.6} aria-hidden className={styles.dropIcon} />
            <div className={styles.dropText}>
              <span className={styles.dropTitle}>{over ? 'Drop to import' : 'Drop .proto files, folders or descriptor sets here'}</span>
              {!files.length && <span className={styles.dropHint}>Imports resolve by path; files dropped without their folders are matched by file name. google/protobuf well-known types are built in.</span>}
            </div>
            <Button size="sm" variant={files.length ? 'secondary' : 'primary'} icon={FolderOpen} loading={busy} onClick={() => void choose()}>
              Choose files…
            </Button>
          </div>
        )}

        {registry == null ? (
          <div className={styles.list}>
            {[0, 1].map((i) => (
              <div key={i} className={styles.skeletonRow}>
                <Skeleton width="40%" height={12} />
                <Skeleton width="25%" height={11} />
              </div>
            ))}
          </div>
        ) : files.length > 0 ? (
          <>
            <div className={styles.listBar}>
              <span className={clsx(styles.totals, 'tnum')}>
                {pluralize(summary.files, 'file')} · {pluralize(summary.services, 'service')} · {pluralize(summary.messages, 'message type')}
                {summary.failing > 0 && (
                  <Badge size="sm" variant="danger" className={styles.failingBadge}>
                    {summary.failing} with errors
                  </Badge>
                )}
              </span>
              {files.length > 5 && <SearchInput size="sm" value={query} onValueChange={setQuery} placeholder="Find a file, service or message" className={styles.search} aria-label="Find in schemas" />}
            </div>
            <ul className={styles.list} aria-label="Schema files">
              {shown.map((f) => (
                <FileRow key={f.name} file={f} open={expanded.has(f.name)} onToggle={() => toggle(f.name)} onView={() => setViewing(f)} onRemove={() => void remove(f)} />
              ))}
              {shown.length === 0 && <li className={styles.noMatch}>Nothing matches “{query}”.</li>}
            </ul>
          </>
        ) : null}
      </div>
      <SourceDialog file={viewing} onClose={() => setViewing(null)} />
    </SettingsCard>
  );
}

function FileRow({ file, open, onToggle, onView, onRemove }: { file: ProtoFileInfo; open: boolean; onToggle: () => void; onView: () => void; onRemove: () => void }) {
  const summary = summarizeProtoFile(file);
  const failed = file.errors.length > 0;
  return (
    <li className={clsx(styles.file, failed && styles.fileFailed)}>
      <div className={styles.fileHead}>
        <button type="button" className={styles.fileToggle} onClick={onToggle} aria-expanded={open}>
          <ChevronRight size={14} className={clsx(styles.chev, open && styles.chevOpen)} aria-hidden />
          {file.kind === 'proto' ? <FileCode2 size={15} strokeWidth={1.7} aria-hidden className={styles.fileIcon} /> : <Layers size={15} strokeWidth={1.7} aria-hidden className={styles.fileIcon} />}
          <span className={styles.fileName}>{file.name}</span>
          {file.importName && (
            <Tooltip content={`Imported by other files as ${file.importName}`}>
              <span className={styles.importAs}>as {file.importName}</span>
            </Tooltip>
          )}
        </button>
        <span className={styles.fileMeta}>
          {failed ? (
            <Badge size="sm" variant="danger" icon={CircleAlert}>
              {pluralize(file.errors.length, 'error')}
            </Badge>
          ) : (
            <span className={clsx(styles.fileSummary, 'tnum')}>{summary.text}</span>
          )}
          {file.kind === 'descriptor-set' && (
            <Badge size="sm" variant="neutral">
              descriptor set
            </Badge>
          )}
          <span className={clsx(styles.fileSize, 'tnum')}>{formatBytes(file.size)}</span>
        </span>
        <span className={styles.fileActions}>
          {file.kind === 'proto' && <IconButton label="View source" icon={Eye} size="sm" onClick={onView} />}
          <IconButton label={`Remove ${file.name}`} icon={Trash2} size="sm" variant="danger" onClick={onRemove} />
        </span>
      </div>
      {open && <FileDetails file={file} onView={onView} />}
    </li>
  );
}

function FileDetails({ file, onView }: { file: ProtoFileInfo; onView: () => void }) {
  return (
    <div className={styles.details}>
      {file.errors.length > 0 && (
        <ul className={styles.errors} aria-label="Compile errors">
          {file.errors.map((e, i) => (
            <li key={i} className={styles.error}>
              <CircleAlert size={13} strokeWidth={2} aria-hidden className={styles.errorIcon} />
              <span className={styles.errorText}>
                {e.line != null ? (
                  <button type="button" className={styles.errorLoc} onClick={file.kind === 'proto' && e.file === file.name ? onView : undefined} disabled={!(file.kind === 'proto' && e.file === file.name)}>
                    {e.file === file.name ? `line ${e.line}${e.column != null ? `:${e.column}` : ''}` : protoErrorLocation(e)}
                  </button>
                ) : (
                  e.file !== file.name && <span className={styles.errorLoc}>{e.file}</span>
                )}
                <span className={styles.errorMessage}>{e.message}</span>
              </span>
            </li>
          ))}
        </ul>
      )}
      {file.package && (
        <div className={styles.detailRow}>
          <span className={styles.detailKey}>Package</span>
          <span className={styles.mono}>{file.package}</span>
        </div>
      )}
      {file.contains && file.contains.length > 0 && (
        <div className={styles.detailRow}>
          <span className={styles.detailKey}>Contains</span>
          <span className={styles.chips}>
            {file.contains.map((c) => (
              <span key={c} className={styles.chip}>
                {c}
              </span>
            ))}
          </span>
        </div>
      )}
      {file.services.map((s) => (
        <div key={s.name} className={styles.detailRow}>
          <span className={styles.detailKey}>Service</span>
          <div className={styles.service}>
            <span className={clsx(styles.mono, styles.serviceName)}>{s.name}</span>
            <ul className={styles.methods}>
              {s.methods.map((m) => (
                <li key={m.name} className={styles.method}>
                  <Tooltip content={`Requests to ${m.path} decode as ${m.inputType}, responses as ${m.outputType}`} multiline>
                    <span className={styles.methodSig}>
                      <span className={styles.methodName}>{m.name}</span>
                      <span className={styles.punct}>(</span>
                      {m.clientStreaming && <span className={styles.stream}>stream </span>}
                      <span className={styles.typeName}>{shortName(m.inputType)}</span>
                      <span className={styles.punct}>)</span>
                      <ArrowRight size={11} strokeWidth={2} aria-hidden className={styles.arrow} />
                      {m.serverStreaming && <span className={styles.stream}>stream </span>}
                      <span className={styles.typeName}>{shortName(m.outputType)}</span>
                    </span>
                  </Tooltip>
                </li>
              ))}
            </ul>
          </div>
        </div>
      ))}
      {file.messages.length > 0 && (
        <div className={styles.detailRow}>
          <span className={styles.detailKey}>Messages</span>
          <span className={styles.chips}>
            {file.messages.map((m) => (
              <span key={m} className={styles.chip} title={m}>
                {stripPackage(m, file.package)}
              </span>
            ))}
          </span>
        </div>
      )}
      {file.enums.length > 0 && (
        <div className={styles.detailRow}>
          <span className={styles.detailKey}>Enums</span>
          <span className={styles.chips}>
            {file.enums.map((m) => (
              <span key={m} className={clsx(styles.chip, styles.enumChip)} title={m}>
                {stripPackage(m, file.package)}
              </span>
            ))}
          </span>
        </div>
      )}
      {!file.errors.length && !file.services.length && !file.messages.length && !file.enums.length && <p className={styles.empty}>This file defines no messages, enums or services.</p>}
    </div>
  );
}

function shortName(full: string): string {
  return full.slice(full.lastIndexOf('.') + 1);
}

function stripPackage(full: string, pkg: string | undefined): string {
  return pkg && full.startsWith(`${pkg}.`) ? full.slice(pkg.length + 1) : full;
}

function SourceDialog({ file, onClose }: { file: ProtoFileInfo | null; onClose: () => void }) {
  const [text, setText] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const name = file?.name ?? null;
  useEffect(() => {
    if (!name) return;
    setText(null);
    setError(null);
    const ctl = new AbortController();
    api.protos.source(name, ctl.signal).then(setText, (e: unknown) => {
      if (!ctl.signal.aborted) setError(errorMessage(e));
    });
    return () => ctl.abort();
  }, [name]);
  return (
    <Dialog open={file != null} onOpenChange={(o) => !o && onClose()} title={file?.name ?? ''} description={file?.errors.length ? `${pluralize(file.errors.length, 'error')}: ${file.errors.map((e) => `line ${e.line ?? '?'}${e.column != null ? `:${e.column}` : ''} ${e.message}`).join(' · ')}` : file?.package ? `package ${file.package}` : undefined} width={760} flush>
      <div className={styles.source}>
        {error ? (
          <p className={styles.sourceError}>{error}</p>
        ) : text == null ? (
          <div className={styles.sourceLoading}>
            <Skeleton lines={8} height={11} />
          </div>
        ) : (
          <CodeEditor value={text} language="protobuf" readOnly fill contentId={name ?? ''} aria-label={`Source of ${name}`} />
        )}
      </div>
    </Dialog>
  );
}

// =====================================================================================
// Mappings
// =====================================================================================

type SaveState = { phase: 'idle' | 'saving' | 'saved' | 'incomplete' } | { phase: 'error'; message: string };

function MappingsCard({ registry, onChange }: { registry: ProtoRegistry | null; onChange: (r: ProtoRegistry) => void }) {
  const [rows, setRows] = useState<ProtoMapping[] | null>(null);
  const [save, setSave] = useState<SaveState>({ phase: 'idle' });
  const loaded = useRef(false);
  const latest = useRef<ProtoMapping[] | null>(null);
  const registryRef = useRef(registry);
  registryRef.current = registry;

  useEffect(() => {
    if (registry && !loaded.current) {
      loaded.current = true;
      setRows(registry.mappings);
    }
  }, [registry]);

  const licensed = useFeature('proto-schemas');
  const messageTypes = registry?.messageTypes ?? [];
  const typeOptions = useMemo(() => messageTypes.map((t) => ({ value: t, label: shortName(t), detail: t.slice(0, Math.max(0, t.lastIndexOf('.'))) })), [messageTypes]);

  // Debounced save of valid edits.
  useEffect(() => {
    if (!rows || !loaded.current) return;
    if (!mappingsValid(rows)) return;
    const current = registryRef.current;
    const next = normalizeMappings(rows);
    if (current && JSON.stringify(current.mappings) === JSON.stringify(next)) return;
    latest.current = rows;
    const timer = window.setTimeout(() => {
      setSave({ phase: 'saving' });
      api.protos.setMappings(next).then(
        (saved) => {
          const base = registryRef.current;
          if (base) onChange({ ...base, mappings: saved });
          setSave({ phase: 'saved' });
        },
        (e: unknown) => setSave({ phase: 'error', message: errorMessage(e) }),
      );
    }, 500);
    return () => window.clearTimeout(timer);
  }, [rows, onChange]);

  const update = (i: number, patch: Partial<ProtoMapping>) => setRows((cur) => (cur ? cur.map((r, j) => (j === i ? { ...r, ...patch } : r)) : cur));
  const add = () => setRows((cur) => [...(cur ?? []), { ...newMapping(), id: crypto.randomUUID().replace(/-/g, '').slice(0, 16) }]);
  const removeRow = (i: number) => setRows((cur) => (cur ? cur.filter((_, j) => j !== i) : cur));

  const recent = useMemo(() => {
    const snap = trafficStore.getSnapshot();
    const ids = snap.order.slice(-MATCH_SAMPLE);
    return ids.map((id) => snap.summaries.get(id)).filter((s): s is NonNullable<typeof s> => s != null);
    // Recomputed when the card (re)mounts; match counts are a hint, not live data.
  }, []);

  return (
    <SettingsCard
      id="proto-mappings"
      title="URL mappings"
      description={
        <>
          For protobuf bodies that don't say which message they hold. gRPC calls are matched to their method automatically, and a <code>messageType=</code> or <code>proto=</code> Content-Type parameter always wins.
        </>
      }
      actions={licensed ? <SaveStatus state={rows && !mappingsValid(rows) ? { phase: 'incomplete' } : save} /> : <ProBadge feature="proto-schemas" always />}
    >
      {/* Mappings stay visible without Pro; editing them is locked (the Schemas card explains why). */}
      <ProGate feature="proto-schemas" hideCallout>
        <div id="setting-proto-mappings" data-setting="proto-mappings" className={styles.mappings}>
          {rows == null ? (
            <Skeleton lines={2} height={11} />
          ) : (
            <>
              {rows.length > 0 && (
                <div className={styles.mapTable} role="table" aria-label="URL mappings">
                  <div className={clsx(styles.mapRow, styles.mapHead)} role="row">
                    <span role="columnheader" className={styles.th}>
                      On
                    </span>
                    <span role="columnheader" className={styles.th}>
                      URL pattern
                    </span>
                    <span role="columnheader" className={styles.th}>
                      Body
                    </span>
                    <span role="columnheader" className={styles.th}>
                      Message type
                    </span>
                    <span role="columnheader" className="sr-only">
                      Actions
                    </span>
                  </div>
                  {rows.map((m, i) => (
                    <MappingRow key={m.id || i} mapping={m} messageTypes={messageTypes} typeOptions={typeOptions} matches={countMatches(m.pattern, recent)} sample={recent.length} onChange={(patch) => update(i, patch)} onRemove={() => removeRow(i)} />
                  ))}
                </div>
              )}
              {rows.length === 0 && (
                <p className={styles.mapEmpty}>
                  <ListTree size={14} strokeWidth={1.8} aria-hidden />
                  No mappings. Add one to decode e.g. <code>api.example.com/v1/items/*</code> responses as <code>shop.v1.Item</code>.
                </p>
              )}
              <div className={styles.mapFoot}>
                <Button size="sm" icon={Plus} onClick={add}>
                  Add mapping
                </Button>
                <span className={styles.patternHelp}>
                  <Braces size={12} strokeWidth={2} aria-hidden /> <code>*</code> matches anything. <code>/path</code>, <code>host/path</code>, <code>https://host/path</code> or a bare host.
                </span>
              </div>
            </>
          )}
        </div>
      </ProGate>
    </SettingsCard>
  );
}

function MappingRow({
  mapping,
  messageTypes,
  typeOptions,
  matches,
  sample,
  onChange,
  onRemove,
}: {
  mapping: ProtoMapping;
  messageTypes: readonly string[];
  typeOptions: { value: string; label: string; detail: string }[];
  matches: number;
  sample: number;
  onChange: (patch: Partial<ProtoMapping>) => void;
  onRemove: () => void;
}) {
  const [touched, setTouched] = useState({ pattern: mapping.pattern !== '', type: mapping.messageType !== '' });
  const patternError = touched.pattern ? validateMappingPattern(mapping.pattern) : null;
  const typeError = touched.type ? validateMessageTypeName(mapping.messageType) : null;
  const typeWarning = !typeError ? mappingTypeWarning(mapping.messageType, messageTypes) : null;
  const message = patternError ?? typeError ?? typeWarning;
  return (
    <div className={clsx(styles.mapRowWrap, !mapping.enabled && styles.mapDisabled)} role="row">
      <div className={styles.mapRow}>
        <span role="cell" className={styles.cellCenter}>
          <Switch size="sm" checked={mapping.enabled} onCheckedChange={(enabled) => onChange({ enabled })} aria-label={mapping.enabled ? 'Disable mapping' : 'Enable mapping'} />
        </span>
        <span role="cell" className={styles.cell}>
          <Input
            size="sm"
            mono
            value={mapping.pattern}
            invalid={!!patternError}
            placeholder="api.example.com/v1/items/*"
            onChange={(e) => onChange({ pattern: e.target.value })}
            onBlur={() => setTouched((t) => ({ ...t, pattern: true }))}
            aria-label="URL pattern"
            trailing={
              mapping.pattern.trim() && !patternError && sample > 0 ? (
                <Tooltip content={`Matches ${formatNumber(matches)} of the last ${formatNumber(sample)} captured exchanges`}>
                  <span className={clsx(styles.matchCount, matches > 0 && styles.matchCountHit, 'tnum')}>{formatNumber(matches)}</span>
                </Tooltip>
              ) : undefined
            }
          />
        </span>
        <span role="cell" className={styles.cell}>
          <Select<ProtoMappingSide> size="sm" value={mapping.side} onValueChange={(side) => onChange({ side })} options={MAPPING_SIDES.map((s) => ({ value: s.value, label: s.label }))} aria-label="Which bodies" />
        </span>
        <span role="cell" className={styles.cell}>
          <Combobox
            size="sm"
            mono
            value={mapping.messageType}
            onValueChange={(messageType) => onChange({ messageType })}
            options={typeOptions}
            invalid={!!typeError}
            placeholder="shop.v1.Item"
            emptyText={messageTypes.length ? undefined : 'Import a schema to pick its message types'}
            onBlur={() => setTouched((t) => ({ ...t, type: true }))}
            aria-label="Message type"
          />
        </span>
        <span role="cell" className={styles.cellCenter}>
          <IconButton label="Remove mapping" icon={Trash2} size="sm" variant="danger" onClick={onRemove} />
        </span>
      </div>
      {message && (
        <p className={clsx(styles.rowMessage, (patternError || typeError) && styles.rowError)} role={patternError || typeError ? 'alert' : undefined}>
          {message}
        </p>
      )}
    </div>
  );
}

function SaveStatus({ state }: { state: SaveState }) {
  switch (state.phase) {
    case 'saving':
      return <span className={styles.saveStatus}>Saving…</span>;
    case 'saved':
      return <span className={clsx(styles.saveStatus, styles.saved)}>Saved</span>;
    case 'incomplete':
      return (
        <Tooltip content="Mappings are saved once every row has a URL pattern and a message type." multiline>
          <span className={clsx(styles.saveStatus, styles.saveIncomplete)}>Not saved — incomplete row</span>
        </Tooltip>
      );
    case 'error':
      return (
        <Tooltip content={state.message} multiline>
          <span className={clsx(styles.saveStatus, styles.saveError)}>
            <CircleAlert size={12} strokeWidth={2} aria-hidden /> Not saved
          </span>
        </Tooltip>
      );
    default:
      return null;
  }
}
