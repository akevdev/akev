import clsx from 'clsx';
import { ArrowDownToLine, ArrowUpFromLine, Plus, X } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import type { BodyTransform, Headers, RequestTransform, ResponseTransform } from '../../../api/types';
import { Button, Checkbox, IconButton, Input, Select } from '../../../components/ui';
import { HTTP_METHODS, statusText } from '../../../lib/http';
import { isEmptyRequestTransform, isEmptyResponseTransform } from '../../../lib/rules/describe';
import { headerNameError } from '../../../lib/rules/validate';
import { BodyTransformEditor } from './BodyEditors';
import { BlockLabel, FieldError, useIssue, useTouch } from './context';
import { HeadersField, MatchReplaceList, StatusField, StringChipsField } from './fields';
import styles from './Editor.module.css';

type ReqPart = 'method' | 'url' | 'query' | 'headers' | 'body';
type ResPart = 'status' | 'headers' | 'body';

/** Drop empty lists / undefined keys so the saved JSON stays minimal. */
function pruneRequest(t: RequestTransform): RequestTransform {
  const out: RequestTransform = {};
  if (t.method != null) out.method = t.method;
  if (t.urlReplace?.length) out.urlReplace = t.urlReplace;
  if (t.setQuery?.length) out.setQuery = t.setQuery;
  if (t.setHeaders?.length) out.setHeaders = t.setHeaders;
  if (t.addHeaders?.length) out.addHeaders = t.addHeaders;
  if (t.removeHeaders?.length) out.removeHeaders = t.removeHeaders;
  if (t.body) out.body = t.body;
  return out;
}

function pruneResponse(t: ResponseTransform): ResponseTransform {
  const out: ResponseTransform = {};
  if (t.status != null) out.status = t.status;
  if (t.statusMessage != null) out.statusMessage = t.statusMessage;
  if (t.setHeaders?.length) out.setHeaders = t.setHeaders;
  if (t.addHeaders?.length) out.addHeaders = t.addHeaders;
  if (t.removeHeaders?.length) out.removeHeaders = t.removeHeaders;
  if (t.body) out.body = t.body;
  return out;
}

export interface TransformEditorProps {
  request: RequestTransform | undefined;
  response: ResponseTransform | undefined;
  onChange: (request: RequestTransform | undefined, response: ResponseTransform | undefined) => void;
  /** Path prefix (`action`). */
  path: string;
  /** Which sides to offer. */
  sides?: ('request' | 'response')[];
  ruleId: string;
}

/** Request/response rewrite editor: parts are added on demand so the form stays calm. */
export function TransformEditor({ request, response, onChange, path, sides = ['request', 'response'], ruleId }: TransformEditorProps) {
  const setReq = (t: RequestTransform) => {
    const p = pruneRequest(t);
    onChange(isEmptyRequestTransform(p) ? undefined : p, response);
  };
  const setRes = (t: ResponseTransform) => {
    const p = pruneResponse(t);
    onChange(request, isEmptyResponseTransform(p) ? undefined : p);
  };
  return (
    <div className={styles.transformSides}>
      {sides.includes('request') && <RequestSide t={request ?? {}} onChange={setReq} path={`${path}.request`} ruleId={ruleId} />}
      {sides.includes('response') && <ResponseSide t={response ?? {}} onChange={setRes} path={`${path}.response`} ruleId={ruleId} />}
    </div>
  );
}

function SideCard({ icon, title, hint, children, empty, addButtons }: { icon: ReactNode; title: string; hint: string; children: ReactNode; empty: boolean; addButtons: ReactNode }) {
  return (
    <section className={styles.sideCard}>
      <header className={styles.sideHead}>
        <span className={styles.sideIcon}>{icon}</span>
        <span className={styles.sideTitle}>{title}</span>
        <span className={styles.sideHint}>{empty ? hint : ''}</span>
      </header>
      {children}
      <div className={clsx(styles.partAdders, empty && styles.partAddersEmpty)}>{addButtons}</div>
    </section>
  );
}

function Part({ label, onRemove, children, hint }: { label: string; onRemove: () => void; children: ReactNode; hint?: ReactNode }) {
  return (
    <div className={styles.part}>
      <div className={styles.partHead}>
        <BlockLabel hint={hint}>{label}</BlockLabel>
        <IconButton label={`Remove ${label.toLowerCase()} change`} icon={X} size="sm" onClick={onRemove} />
      </div>
      {children}
    </div>
  );
}

function AddPart({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <Button size="sm" variant="ghost" icon={Plus} onClick={onClick} className={styles.partAdd}>
      {label}
    </Button>
  );
}

function RequestSide({ t, onChange, path, ruleId }: { t: RequestTransform; onChange: (t: RequestTransform) => void; path: string; ruleId: string }) {
  const [open, setOpen] = useState<Set<ReqPart>>(new Set());
  const has: Record<ReqPart, boolean> = {
    method: t.method != null,
    url: !!t.urlReplace?.length || open.has('url'),
    query: !!t.setQuery?.length || open.has('query'),
    headers: !!(t.setHeaders?.length || t.addHeaders?.length || t.removeHeaders?.length) || open.has('headers'),
    body: t.body != null,
  };
  const show = (p: ReqPart, patch: Partial<RequestTransform>) => {
    setOpen((s) => new Set(s).add(p));
    onChange({ ...t, ...patch });
  };
  const hide = (p: ReqPart, keys: (keyof RequestTransform)[]) => {
    setOpen((s) => {
      const n = new Set(s);
      n.delete(p);
      return n;
    });
    const next = { ...t };
    for (const k of keys) delete next[k];
    onChange(next);
  };
  const methodErr = useIssue(`${path}.method`);
  const empty = !Object.values(has).some(Boolean);
  return (
    <SideCard
      icon={<ArrowUpFromLine size={14} strokeWidth={1.9} aria-hidden />}
      title="Request"
      hint="Change what is sent to the server"
      empty={empty}
      addButtons={
        <>
          {!has.method && <AddPart label="Method" onClick={() => show('method', { method: 'POST' })} />}
          {!has.url && <AddPart label="URL" onClick={() => show('url', { urlReplace: [{ find: '', replace: '', regex: false, caseSensitive: false }] })} />}
          {!has.query && <AddPart label="Query" onClick={() => show('query', { setQuery: [['', '']] })} />}
          {!has.headers && <AddPart label="Headers" onClick={() => show('headers', {})} />}
          {!has.body && <AddPart label="Body" onClick={() => show('body', { body: { type: 'json-merge', patch: { key: 'value' } } })} />}
        </>
      }
    >
      {has.method && (
        <Part label="Method" onRemove={() => hide('method', ['method'])}>
          <Select
            size="sm"
            value={t.method}
            onValueChange={(method) => onChange({ ...t, method })}
            options={[...new Set([...HTTP_METHODS, ...(t.method ? [t.method] : [])])].map((m) => ({ value: m, label: m }))}
            mono
            invalid={!!methodErr}
            className={styles.methodSelect}
            aria-label="New method"
          />
          <FieldError>{methodErr}</FieldError>
        </Part>
      )}
      {has.url && (
        <Part label="URL find & replace" hint="Applied to the full URL — rewrite host, path or query" onRemove={() => hide('url', ['urlReplace'])}>
          <MatchReplaceList items={t.urlReplace ?? []} onChange={(urlReplace) => onChange({ ...t, urlReplace })} path={`${path}.urlReplace`} findPlaceholder="/v1/" replacePlaceholder="/v2/" />
        </Part>
      )}
      {has.query && (
        <Part label="Query parameters" hint="Set or remove" onRemove={() => hide('query', ['setQuery'])}>
          <QuerySetList items={t.setQuery ?? []} onChange={(setQuery) => onChange({ ...t, setQuery })} path={`${path}.setQuery`} />
        </Part>
      )}
      {has.headers && (
        <Part label="Headers" onRemove={() => hide('headers', ['setHeaders', 'addHeaders', 'removeHeaders'])}>
          <HeaderChanges setHeaders={t.setHeaders} addHeaders={t.addHeaders} removeHeaders={t.removeHeaders} path={path} onChange={(p) => onChange({ ...t, ...p })} />
        </Part>
      )}
      {has.body && t.body && (
        <Part label="Body" onRemove={() => hide('body', ['body'])}>
          <BodyTransformEditor body={t.body} onChange={(body: BodyTransform) => onChange({ ...t, body })} path={`${path}.body`} contentId={`${ruleId}:req-body`} />
        </Part>
      )}
    </SideCard>
  );
}

function ResponseSide({ t, onChange, path, ruleId }: { t: ResponseTransform; onChange: (t: ResponseTransform) => void; path: string; ruleId: string }) {
  const [open, setOpen] = useState<Set<ResPart>>(new Set());
  const has: Record<ResPart, boolean> = {
    status: t.status != null || t.statusMessage != null,
    headers: !!(t.setHeaders?.length || t.addHeaders?.length || t.removeHeaders?.length) || open.has('headers'),
    body: t.body != null,
  };
  const hide = (p: ResPart, keys: (keyof ResponseTransform)[]) => {
    setOpen((s) => {
      const n = new Set(s);
      n.delete(p);
      return n;
    });
    const next = { ...t };
    for (const k of keys) delete next[k];
    onChange(next);
  };
  const empty = !Object.values(has).some(Boolean);
  return (
    <SideCard
      icon={<ArrowDownToLine size={14} strokeWidth={1.9} aria-hidden />}
      title="Response"
      hint="Change what the client receives"
      empty={empty}
      addButtons={
        <>
          {!has.status && <AddPart label="Status" onClick={() => onChange({ ...t, status: 200 })} />}
          {!has.headers && (
            <AddPart
              label="Headers"
              onClick={() => {
                setOpen((s) => new Set(s).add('headers'));
                onChange({ ...t });
              }}
            />
          )}
          {!has.body && <AddPart label="Body" onClick={() => onChange({ ...t, body: { type: 'json-merge', patch: { key: 'value' } } })} />}
        </>
      }
    >
      {has.status && (
        <Part label="Status" hint={t.status != null ? `${t.status} ${statusText(t.status)}` : undefined} onRemove={() => hide('status', ['status', 'statusMessage'])}>
          <StatusField status={t.status ?? 200} statusMessage={t.statusMessage} onChange={(status, statusMessage) => onChange({ ...t, status, statusMessage })} path={`${path}.`} />
        </Part>
      )}
      {has.headers && (
        <Part label="Headers" onRemove={() => hide('headers', ['setHeaders', 'addHeaders', 'removeHeaders'])}>
          <HeaderChanges setHeaders={t.setHeaders} addHeaders={t.addHeaders} removeHeaders={t.removeHeaders} path={path} onChange={(p) => onChange({ ...t, ...p })} />
        </Part>
      )}
      {has.body && t.body && (
        <Part label="Body" onRemove={() => hide('body', ['body'])}>
          <BodyTransformEditor body={t.body} onChange={(body) => onChange({ ...t, body })} path={`${path}.body`} contentId={`${ruleId}:res-body`} />
        </Part>
      )}
    </SideCard>
  );
}

function HeaderChanges({ setHeaders, addHeaders, removeHeaders, path, onChange }: { setHeaders?: Headers; addHeaders?: Headers; removeHeaders?: string[]; path: string; onChange: (p: { setHeaders?: Headers; addHeaders?: Headers; removeHeaders?: string[] }) => void }) {
  return (
    <div className={styles.headerChanges}>
      <div>
        <div className={styles.subLabel}>Set — replaces existing values</div>
        <HeadersField headers={setHeaders ?? []} onChange={(h) => onChange({ setHeaders: h })} path={`${path}.setHeaders`} label="Set headers" />
      </div>
      <div>
        <div className={styles.subLabel}>Add — appends, keeping existing values</div>
        <HeadersField headers={addHeaders ?? []} onChange={(h) => onChange({ addHeaders: h })} path={`${path}.addHeaders`} label="Add headers" />
      </div>
      <div>
        <div className={styles.subLabel}>Remove</div>
        <StringChipsField values={removeHeaders ?? []} onChange={(v) => onChange({ removeHeaders: v })} placeholder="header-name, another-header" path={`${path}.removeHeaders`} validate={headerNameError} />
      </div>
    </div>
  );
}

function QuerySetList({ items, onChange, path }: { items: [string, string | null][]; onChange: (items: [string, string | null][]) => void; path: string }) {
  const touch = useTouch();
  const update = (i: number, next: [string, string | null]) => onChange(items.map((x, j) => (j === i ? next : x)));
  return (
    <div className={styles.stack}>
      {items.map(([name, value], i) => (
        <QueryRow key={i} path={`${path}.${i}`} name={name} value={value} onBlur={() => touch(`${path}.${i}`)} onChange={(n) => update(i, n)} onRemove={() => onChange(items.filter((_, j) => j !== i))} />
      ))}
      <div>
        <Button size="sm" variant="ghost" icon={Plus} onClick={() => onChange([...items, ['', '']])}>
          Add parameter
        </Button>
      </div>
    </div>
  );
}

function QueryRow({ path, name, value, onChange, onRemove, onBlur }: { path: string; name: string; value: string | null; onChange: (n: [string, string | null]) => void; onRemove: () => void; onBlur: () => void }) {
  const err = useIssue(path);
  const removing = value === null;
  return (
    <div>
      <div className={styles.queryRow}>
        <Input size="sm" mono value={name} placeholder="param" invalid={!!err} onChange={(e) => onChange([e.target.value, value])} onBlur={onBlur} aria-label="Parameter name" />
        <span className={styles.replaceArrow}>=</span>
        <Input size="sm" mono value={value ?? ''} placeholder={removing ? 'removed' : 'value'} disabled={removing} onChange={(e) => onChange([name, e.target.value])} aria-label="Parameter value" />
        <Checkbox checked={removing} onCheckedChange={(c) => onChange([name, c ? null : ''])} label="Remove" />
        <IconButton label="Delete row" icon={X} size="sm" onClick={onRemove} />
      </div>
      <FieldError>{err}</FieldError>
    </div>
  );
}
