import clsx from 'clsx';
import { FileJson, FolderOpen, FolderPlus, Import, Save } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import { Badge, Button, Dialog, Field, Input, Select, TextArea } from '../../components/ui';
import type { SavedFolder } from './model';
import { utf8Decode } from '../../lib/encoding';
import { pickFiles } from '../../lib/platform';
import { collectionSize, importCollection, parseCollectionText, type ImportedCollection } from './importers';
import { detectPaste, pasteKindLabel, splitCurlCommands, splitFetchCalls, splitPowerShellCommands, type PasteKind } from './paste';
import styles from './Dialogs.module.css';

const TOP = '__top__';
const NEW = '__new__';

export interface SaveDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title?: string;
  initialName: string;
  folders: readonly SavedFolder[];
  initialFolderId?: string | null;
  /** `newFolderName` is set when the user chose to create a folder. */
  onSave: (name: string, folderId: string | null, newFolderName?: string) => void;
}

/** Name + folder picker for saving a request. */
export function SaveDialog({ open, onOpenChange, title = 'Save request', initialName, folders, initialFolderId, onSave }: SaveDialogProps) {
  const [name, setName] = useState(initialName);
  const [folder, setFolder] = useState<string>(TOP);
  const [newFolder, setNewFolder] = useState('');
  const nameRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (!open) return;
    setName(initialName);
    setFolder(initialFolderId && folders.some((f) => f.id === initialFolderId) ? initialFolderId : TOP);
    setNewFolder('');
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, [open]);

  const submit = (e?: FormEvent | KeyboardEvent) => {
    e?.preventDefault();
    if (!name.trim()) return nameRef.current?.focus();
    if (folder === NEW) onSave(name.trim(), null, newFolder.trim() || 'New folder');
    else onSave(name.trim(), folder === TOP ? null : folder);
    onOpenChange(false);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={title}
      width={440}
      onOpenAutoFocus={(e) => {
        e.preventDefault();
        nameRef.current?.focus();
        nameRef.current?.select();
      }}
      footer={
        <>
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button variant="primary" icon={Save} onClick={() => submit()} disabled={!name.trim()}>
            Save
          </Button>
        </>
      }
    >
      <form className={styles.form} onSubmit={submit}>
        <Field label="Name">
          <Input
            ref={nameRef}
            value={name}
            onChange={(e) => setName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.nativeEvent.isComposing) submit(e);
            }}
            placeholder="Get user"
          />
        </Field>
        <Field label="Folder">
          <Select
            value={folder}
            onValueChange={setFolder}
            options={[
              { value: TOP, label: 'Top level' },
              ...folders.map((f) => ({ value: f.id, label: f.name })),
              { value: NEW, label: 'New folder…', icon: FolderPlus },
            ]}
            aria-label="Folder"
          />
        </Field>
        {folder === NEW && (
          <Field label="Folder name">
            <Input
              value={newFolder}
              onChange={(e) => setNewFolder(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && !e.nativeEvent.isComposing) submit(e);
              }}
              placeholder="New folder"
              autoFocus
            />
          </Field>
        )}
        <button type="submit" hidden />
      </form>
    </Dialog>
  );
}

export interface ImportDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initialText?: string;
  /** Import cURL / fetch() commands; resolves true when done (the dialog then closes). */
  onImport: (text: string) => Promise<boolean>;
  /** Import a HAR file / Postman collection into Saved. */
  onImportCollection: (collection: ImportedCollection) => void;
}

type Detected =
  | { kind: Exclude<PasteKind, null>; count: number }
  | { kind: 'collection'; collection: ImportedCollection; fileName?: string }
  | { kind: 'error'; message: string };

function describeCollection(c: ImportedCollection): string {
  const n = collectionSize(c);
  const what = `${n} request${n === 1 ? '' : 's'}`;
  const folders = c.folders.length ? ` in ${c.folders.length + (c.requests.length ? 1 : 0)} folders` : '';
  const vars = c.variables.length ? ` · ${c.variables.length} variable${c.variables.length === 1 ? '' : 's'}` : '';
  const label = c.format === 'har' ? 'HAR' : c.format === 'postman' ? 'Postman' : 'OpenAPI';
  return `${label} · ${what}${folders}${vars}`;
}

/**
 * Import requests: paste cURL / fetch() / PowerShell commands (one tab each), or pick / drop /
 * paste a HAR file, Postman collection or OpenAPI spec (saved as folders).
 */
export function ImportDialog({ open, onOpenChange, initialText = '', onImport, onImportCollection }: ImportDialogProps) {
  const [text, setText] = useState(initialText);
  const [file, setFile] = useState<Detected | null>(null);
  const [busy, setBusy] = useState(false);
  const [over, setOver] = useState(false);
  useEffect(() => {
    if (!open) return;
    setText(initialText);
    setFile(null);
    setOver(false);
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, [open]);

  const fromText = useMemo<Detected | null>(() => {
    const kind = detectPaste(text);
    if (kind === 'curl') return { kind, count: splitCurlCommands(text).length };
    if (kind === 'fetch') return { kind, count: splitFetchCalls(text).length };
    if (kind === 'powershell') return { kind, count: splitPowerShellCommands(text).length };
    const doc = parseCollectionText(text);
    if (doc) {
      try {
        return { kind: 'collection', collection: importCollection(doc.json) };
      } catch (e) {
        return { kind: 'error', message: e instanceof Error ? e.message : String(e) };
      }
    }
    return null;
  }, [text]);
  const detected = file ?? fromText;

  const loadFile = (name: string, bytes: Uint8Array) => {
    const content = utf8Decode(bytes);
    const doc = parseCollectionText(content);
    if (doc) {
      try {
        setFile({ kind: 'collection', collection: importCollection(doc.json, name), fileName: name });
      } catch (e) {
        setFile({ kind: 'error', message: e instanceof Error ? e.message : String(e) });
      }
      return;
    }
    if (detectPaste(content)) {
      setFile(null);
      setText(content);
      return;
    }
    setFile({ kind: 'error', message: `${name} isn't a HAR file, Postman collection, OpenAPI spec or command` });
  };

  const choose = async () => {
    const picked = await pickFiles({ accept: ['.har', '.json', 'application/json', '.txt', '.sh'] });
    const f = picked?.[0];
    if (f) loadFile(f.name, f.data);
  };

  const run = async () => {
    if (!detected || busy || detected.kind === 'error') return;
    if (detected.kind === 'collection') {
      if (collectionSize(detected.collection) === 0) return;
      onImportCollection(detected.collection);
      onOpenChange(false);
      return;
    }
    setBusy(true);
    try {
      if (await onImport(text)) onOpenChange(false);
    } finally {
      setBusy(false);
    }
  };

  const ready = !!detected && detected.kind !== 'error' && (detected.kind !== 'collection' || collectionSize(detected.collection) > 0);
  const label = !detected || detected.kind === 'error' ? 'Import' : detected.kind === 'collection' ? 'Import to Saved' : detected.count > 1 ? `Open ${detected.count} tabs` : 'Import';

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Import requests"
      description="Paste cURL, fetch() or PowerShell commands (DevTools → Copy as …) — each opens in a tab. HAR files, Postman collections and OpenAPI specs (JSON) are saved as folders."
      width={640}
      footer={
        <>
          <span className={styles.detect}>
            {detected?.kind === 'collection' ? (
              <Badge variant="accent" size="sm">
                {describeCollection(detected.collection)}
              </Badge>
            ) : detected?.kind === 'error' ? (
              <span className={styles.detectError}>{detected.message}</span>
            ) : detected ? (
              <Badge variant="accent" size="sm">
                {detected.count} {pasteKindLabel(detected.kind, detected.count)}
              </Badge>
            ) : text.trim() ? (
              <span className={styles.detectNone}>Not a cURL, fetch() or PowerShell command, or a collection</span>
            ) : null}
          </span>
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button variant="primary" icon={Import} loading={busy} disabled={!ready} onClick={run} kbd="mod+enter">
            {label}
          </Button>
        </>
      }
    >
      {/* biome-ignore lint/a11y/noStaticElementInteractions: drop zone for files; the same import is available from a button */}
      <div
        className={clsx(styles.importBody, over && styles.importOver)}
        onDragEnter={(e) => {
          if (e.dataTransfer.types.includes('Files')) {
            e.preventDefault();
            setOver(true);
          }
        }}
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes('Files')) e.preventDefault();
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
        }}
        onDrop={async (e) => {
          const f = e.dataTransfer.files[0];
          if (!f) return;
          e.preventDefault();
          setOver(false);
          loadFile(f.name, new Uint8Array(await f.arrayBuffer()));
        }}
      >
        {file?.kind === 'collection' && file.fileName ? (
          <div className={styles.fileChip}>
            <FileJson size={18} strokeWidth={1.6} aria-hidden className={styles.fileChipIcon} />
            <div className={styles.fileChipText}>
              <span className={styles.fileChipName}>{file.fileName}</span>
              <span className={styles.fileChipMeta}>{file.collection.name}{file.collection.skipped.length ? ` · ${file.collection.skipped.length} skipped` : ''}</span>
            </div>
            <Button size="sm" variant="ghost" onClick={() => setFile(null)}>
              Clear
            </Button>
          </div>
        ) : (
          <TextArea
            mono
            autoFocus
            className={styles.textarea}
            value={text}
            onChange={(e) => {
              setFile(null);
              setText(e.target.value);
            }}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                void run();
              }
            }}
            placeholder={"curl 'https://api.example.com/v1/users' \\\n  -H 'accept: application/json'"}
            spellCheck={false}
            aria-label="Commands to import"
          />
        )}
        <div className={styles.importFoot}>
          <Button size="sm" variant="secondary" icon={FolderOpen} onClick={choose}>
            Choose file…
          </Button>
          <span className={styles.importHint}>or drop a .har, Postman or OpenAPI .json file here</span>
        </div>
      </div>
    </Dialog>
  );
}
