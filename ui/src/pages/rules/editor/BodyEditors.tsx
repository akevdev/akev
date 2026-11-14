import { useEffect, useMemo, useState } from 'react';
import type { BodySource, BodyTransform, Headers } from '../../../api/types';
import { FileUp } from 'lucide-react';
import { Button, CodeEditor, Input, SegmentedControl, Select, TextArea, toast, Tooltip } from '../../../components/ui';
import { base64ToText, textToBase64 } from '../../../lib/encoding';
import { formatBytes } from '../../../lib/format';
import { getHeader } from '../../../lib/http';
import { pickFiles } from '../../../lib/platform';
import { bodyFromFile, LANGUAGE_CONTENT_TYPES, languageForContentType } from '../../../lib/rules/actions';
import { jsonPatchError, parseJson } from '../../../lib/rules/validate';
import { Explain, FieldError, useIssue, useTouch } from './context';
import { PathPicker } from './PathPicker';
import { JsonField, MatchReplaceList } from './fields';
import styles from './Editor.module.css';

type SourceKind = BodySource['type'];

/** Larger files should be served from disk (File tab) rather than stored in the rule set. */
const MAX_INLINE_BYTES = 8 * 1024 * 1024;

const LANGUAGES = [
  { value: 'text', label: 'Plain text' },
  { value: 'json', label: 'JSON' },
  { value: 'html', label: 'HTML' },
  { value: 'xml', label: 'XML' },
  { value: 'css', label: 'CSS' },
  { value: 'javascript', label: 'JavaScript' },
  { value: 'yaml', label: 'YAML' },
  { value: 'markdown', label: 'Markdown' },
];

/** Convert body content between sources, keeping what the user wrote where possible. */
export function convertBody(body: BodySource, to: SourceKind): BodySource {
  if (body.type === to) return body;
  const asText = (): string => {
    switch (body.type) {
      case 'text':
        return body.text;
      case 'json':
        return JSON.stringify(body.value, null, 2) ?? '';
      case 'base64':
        return base64ToText(body.data) ?? '';
      default:
        return '';
    }
  };
  switch (to) {
    case 'empty':
      return { type: 'empty' };
    case 'text':
      return { type: 'text', text: asText() };
    case 'json': {
      const t = asText();
      const parsed = t.trim() ? parseJson(t) : null;
      return { type: 'json', value: parsed?.ok ? parsed.value : t.trim() ? t : {} };
    }
    case 'base64':
      return { type: 'base64', data: textToBase64(asText()) };
    case 'file':
      return { type: 'file', path: '' };
  }
}

export interface BodySourceEditorProps {
  body: BodySource;
  onChange: (body: BodySource) => void;
  path: string;
  /** Response headers, for the content-type ↔ language sync. */
  headers?: Headers;
  onHeadersChange?: (headers: Headers) => void;
  contentId?: string;
}

/** Body source tabs (none / text / JSON / base64 / file) with a code editor and language picker. */
export function BodySourceEditor({ body, onChange, path, headers, onHeadersChange, contentId }: BodySourceEditorProps) {
  const contentType = headers ? getHeader(headers, 'content-type') : undefined;
  const [language, setLanguage] = useState(() => languageForContentType(contentType));
  useEffect(() => {
    if (contentType) setLanguage(languageForContentType(contentType));
  }, [contentType]);
  const fileErr = useIssue(`${path}.path`);
  const b64Err = useIssue(`${path}.data`);
  const touch = useTouch();

  const setSource = (to: SourceKind) => {
    onChange(convertBody(body, to));
    if (to === 'json' && headers && onHeadersChange && !contentType) onHeadersChange([...headers, ['content-type', 'application/json']]);
  };

  const pickLanguage = (lang: string) => {
    const prev = language;
    setLanguage(lang);
    if (!headers || !onHeadersChange) return;
    const ct = LANGUAGE_CONTENT_TYPES[lang];
    if (!ct) return;
    // Two-way sync: add a content type when missing, or follow the language when the header
    // still matched the previous language.
    if (!contentType) onHeadersChange([...headers, ['content-type', ct]]);
    else if (languageForContentType(contentType) === prev) onHeadersChange(headers.map(([k, v]) => (k.toLowerCase() === 'content-type' ? [k, ct] : [k, v])));
  };

  const loadFile = async () => {
    const files = await pickFiles();
    const file = files?.[0];
    if (!file) return;
    if (file.data.length > MAX_INLINE_BYTES) {
      toast.error(`“${file.name}” is too large to inline`, { description: `It is ${formatBytes(file.data.length)}; inline bodies are limited to ${formatBytes(MAX_INLINE_BYTES)}. Use the File tab to serve it from disk instead.` });
      return;
    }
    const loaded = bodyFromFile(file.name, file.data);
    onChange(loaded.body);
    if (loaded.body.type === 'text') setLanguage(loaded.language);
    if (loaded.contentType && headers && onHeadersChange) {
      onHeadersChange(contentType ? headers.map(([k, v]) => (k.toLowerCase() === 'content-type' ? [k, loaded.contentType as string] : [k, v])) : [...headers, ['content-type', loaded.contentType]]);
    }
    toast.success(`Loaded ${file.name}`, { description: `${formatBytes(file.data.length)} · ${loaded.body.type === 'text' ? 'as text' : 'as base64'}` });
  };

  const b64Size = useMemo(() => (body.type === 'base64' ? Math.floor((body.data.replace(/\s+/g, '').replace(/=+$/, '').length * 3) / 4) : 0), [body]);

  return (
    <div className={styles.stack}>
      <div className={styles.bodyBar}>
        <SegmentedControl<SourceKind>
          size="sm"
          value={body.type}
          onValueChange={setSource}
          items={[
            { value: 'empty', label: 'None' },
            { value: 'text', label: 'Text' },
            { value: 'json', label: 'JSON' },
            { value: 'base64', label: 'Base64' },
            { value: 'file', label: 'File' },
          ]}
          aria-label="Body source"
        />
        {body.type === 'text' && <Select size="sm" value={language} onValueChange={pickLanguage} options={LANGUAGES} className={styles.langSelect} aria-label="Syntax" />}
        {body.type === 'base64' && <span className={styles.bodyMeta}>{formatBytes(b64Size)} decoded</span>}
        <Tooltip content="Inline a file's contents (text, or base64 for binary). To serve a file that is re-read on every request, use the File tab.">
          <Button size="sm" variant="ghost" icon={FileUp} onClick={() => void loadFile()} className={styles.bodyLoad}>
            Load file…
          </Button>
        </Tooltip>
      </div>
      {body.type === 'empty' && <Explain>The response has no body.</Explain>}
      {body.type === 'text' && (
        <div className={styles.codeFrame}>
          <CodeEditor value={body.text} onChange={(text) => onChange({ type: 'text', text })} language={language} minHeight={120} maxHeight={360} contentId={contentId} placeholder="Response body…" aria-label="Body text" transparent />
        </div>
      )}
      {body.type === 'json' && <JsonField value={body.value} onChange={(value) => onChange({ type: 'json', value })} minHeight={120} maxHeight={360} label="JSON body" contentId={contentId} />}
      {body.type === 'base64' && (
        <>
          <TextArea mono autoResize minRows={3} maxRows={10} value={body.data} invalid={!!b64Err} onChange={(e) => onChange({ type: 'base64', data: e.target.value })} placeholder="Base64-encoded bytes" aria-label="Base64 body" spellCheck={false} />
          <FieldError>{b64Err}</FieldError>
        </>
      )}
      {body.type === 'file' && (
        <>
          <div className={styles.pathRow}>
            <Input mono value={body.path} invalid={!!fileErr} onChange={(e) => onChange({ type: 'file', path: e.target.value })} onBlur={() => touch(`${path}.path`)} placeholder="/absolute/path/to/response.json" aria-label="File path" spellCheck={false} />
            <PathPicker pick="file" title="Respond with this file" onPick={(p) => onChange({ type: 'file', path: p })} />
          </div>
          <FieldError>{fileErr}</FieldError>
          <Explain>Read fresh on every request, so edits to the file apply immediately. Paths are on the machine running Akev.</Explain>
        </>
      )}
    </div>
  );
}

// =====================================================================================
// body transforms
// =====================================================================================

type TransformKind = BodyTransform['type'];

function defaultBodyTransform(kind: TransformKind): BodyTransform {
  switch (kind) {
    case 'replace':
      return { type: 'replace', body: { type: 'text', text: '' } };
    case 'match-replace':
      return { type: 'match-replace', replacements: [{ find: '', replace: '', regex: false, caseSensitive: false }] };
    case 'json-merge':
      return { type: 'json-merge', patch: { key: 'value' } };
    case 'json-patch':
      return { type: 'json-patch', operations: [{ op: 'replace', path: '/key', value: 'new value' }] };
  }
}

export function BodyTransformEditor({ body, onChange, path, contentId }: { body: BodyTransform; onChange: (b: BodyTransform) => void; path: string; contentId?: string }) {
  const opsErr = useIssue(`${path}.operations`);
  return (
    <div className={styles.stack}>
      <SegmentedControl<TransformKind>
        size="sm"
        value={body.type}
        onValueChange={(k) => onChange(defaultBodyTransform(k))}
        items={[
          { value: 'replace', label: 'Replace' },
          { value: 'match-replace', label: 'Find & replace' },
          { value: 'json-merge', label: 'Merge JSON' },
          { value: 'json-patch', label: 'JSON Patch' },
        ]}
        aria-label="Body transform"
      />
      {body.type === 'replace' && <BodySourceEditor body={body.body} onChange={(b) => onChange({ type: 'replace', body: b })} path={`${path}.body`} contentId={contentId} />}
      {body.type === 'match-replace' && <MatchReplaceList items={body.replacements} onChange={(replacements) => onChange({ type: 'match-replace', replacements })} path={`${path}.replacements`} />}
      {body.type === 'json-merge' && (
        <>
          <JsonField value={body.patch} onChange={(patch) => onChange({ type: 'json-merge', patch })} minHeight={96} label="JSON merge patch" contentId={contentId} />
          <Explain>RFC 7386 merge patch: objects merge recursively, `null` deletes a key. Applies only when the body is JSON.</Explain>
        </>
      )}
      {body.type === 'json-patch' && (
        <>
          <JsonField value={body.operations} onChange={(ops) => onChange({ type: 'json-patch', operations: ops as unknown[] })} validate={jsonPatchError} minHeight={96} label="JSON patch operations" contentId={contentId} />
          <FieldError>{opsErr}</FieldError>
          <Explain>RFC 6902 operations (add, remove, replace, move, copy, test), applied atomically.</Explain>
        </>
      )}
    </div>
  );
}
