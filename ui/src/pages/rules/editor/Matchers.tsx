import clsx from 'clsx';
import { Ellipsis, Plus, X } from 'lucide-react';
import { useEffect, useRef, useState, type CSSProperties } from 'react';
import type { KeyValueMatch, Matcher, MatcherType, MatchOp, StringMatch } from '../../../api/types';
import {
  Button,
  Combobox,
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
  IconButton,
  Input,
  Select,
  Switch,
} from '../../../components/ui';
import { KNOWN_HEADER_NAMES } from '../../../lib/headerDocs';
import { HTTP_METHODS, methodColorVar } from '../../../lib/http';
import { defaultMatcher, HTTP_VERSIONS, kvList, MATCHER_GROUPS, MATCHERS, matcherInfo, OP_LABELS, parsePorts, patternOf, PROTOCOLS, stringMatch, withKvList } from '../../../lib/rules/matchers';
import { iconFor } from '../icons';
import { FieldError, useFieldError, useIssue, useTouch } from './context';
import { CaseToggle, JsonField, NegateToggle } from './fields';
import styles from './Editor.module.css';

const HEADER_SUGGESTIONS = KNOWN_HEADER_NAMES.map((n) => ({ value: n.toLowerCase() }));

// =====================================================================================
// add-condition menu
// =====================================================================================

export function AddConditionMenu({ onAdd, label = 'Add condition', size = 'sm', exclude = [] }: { onAdd: (type: MatcherType) => void; label?: string; size?: 'sm' | 'md'; exclude?: MatcherType[] }) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <Button size={size} variant="ghost" icon={Plus} className={styles.addButton}>
          {label}
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className={styles.richMenu}>
        {MATCHER_GROUPS.map((group, gi) => {
          const items = MATCHERS.filter((m) => m.group === group && !exclude.includes(m.type));
          if (items.length === 0) return null;
          return (
            <div key={group} role="group" aria-label={group}>
              {gi > 0 && <DropdownMenuSeparator />}
              <DropdownMenuLabel>{group}</DropdownMenuLabel>
              <div className={styles.richGrid}>
                {items.map((m) => {
                  const Icon = iconFor(m.icon);
                  return (
                    <DropdownMenuItem key={m.type} className={styles.richItem} onSelect={() => onAdd(m.type)}>
                      <span className={styles.richIcon} aria-hidden>
                        <Icon size={14} strokeWidth={1.8} />
                      </span>
                      <span className={styles.richText}>
                        <span className={styles.richTitle}>{m.label}</span>
                        <span className={styles.richDesc}>{m.description}</span>
                      </span>
                    </DropdownMenuItem>
                  );
                })}
              </div>
            </div>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

// =====================================================================================
// list
// =====================================================================================

export interface MatcherListProps {
  matchers: Matcher[];
  onChange: (matchers: Matcher[]) => void;
  /** Path prefix for validation (`matchers`, `matchers.2.matchers`). */
  path: string;
  /** Connector word between rows. */
  joiner: 'and' | 'or';
  depth?: number;
}

/** A list of AND-ed (or OR-ed, inside "Any of") conditions with an add menu. */
export function MatcherList({ matchers, onChange, path, joiner, depth = 0 }: MatcherListProps) {
  const [focusIndex, setFocusIndex] = useState<number | null>(null);
  const add = (type: MatcherType) => {
    setFocusIndex(matchers.length);
    onChange([...matchers, defaultMatcher(type)]);
  };
  return (
    <div className={clsx(styles.matcherList, depth > 0 && styles.matcherListNested)} style={{ '--depth': depth } as CSSProperties}>
      {matchers.map((m, i) => (
        <MatcherRow
          key={i}
          matcher={m}
          path={`${path}.${i}`}
          connector={i === 0 ? (depth === 0 ? 'where' : joiner === 'or' ? 'either' : 'both') : joiner}
          autoFocus={focusIndex === i}
          depth={depth}
          onChange={(next) => onChange(matchers.map((x, j) => (j === i ? next : x)))}
          onRemove={() => {
            setFocusIndex(null);
            onChange(matchers.filter((_, j) => j !== i));
          }}
        />
      ))}
      <div className={styles.addRow}>
        <AddConditionMenu onAdd={add} label={depth > 0 ? `Add ${joiner === 'or' ? 'alternative' : 'nested condition'}` : 'Add condition'} />
      </div>
    </div>
  );
}

// =====================================================================================
// row
// =====================================================================================

interface MatcherRowProps {
  matcher: Matcher;
  path: string;
  connector: string;
  autoFocus: boolean;
  depth: number;
  onChange: (m: Matcher) => void;
  onRemove: () => void;
}

function MatcherRow({ matcher, path, connector, autoFocus, depth, onChange, onRemove }: MatcherRowProps) {
  const info = matcherInfo(matcher.type);
  const Icon = iconFor(info.icon);
  const rowRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!autoFocus) return;
    const el = rowRef.current?.querySelector<HTMLElement>('input, textarea, [role="combobox"], .cm-content, button[aria-pressed]');
    el?.focus();
  }, [autoFocus]);
  const pattern = patternOf(matcher);
  const isLogic = matcher.type === 'any' || matcher.type === 'all' || matcher.type === 'not';

  return (
    <div ref={rowRef} className={clsx(styles.matcherRow, isLogic && styles.matcherRowLogic)} data-type={matcher.type}>
      <span className={styles.connector}>{connector}</span>
      <span className={styles.matcherLabel} title={info.description}>
        <Icon size={13} strokeWidth={1.8} aria-hidden />
        <span className="truncate">{info.label}</span>
      </span>
      <div className={styles.matcherBody}>
        {pattern ? (
          <PatternEditor
            pattern={pattern}
            path={`${path}.pattern`}
            placeholder={info.placeholder}
            ops={info.ops ?? []}
            mono={matcher.type !== 'source' && matcher.type !== 'graphql-operation'}
            caseLocked={matcher.type === 'host'}
            onChange={(p) => onChange({ ...matcher, pattern: p } as Matcher)}
          />
        ) : (
          <ValueEditor matcher={matcher} path={path} depth={depth} onChange={onChange} />
        )}
      </div>
      <IconButton label="Remove condition" icon={X} size="sm" onClick={onRemove} className={styles.removeButton} />
    </div>
  );
}

// =====================================================================================
// string pattern
// =====================================================================================

export function PatternEditor({
  pattern,
  path,
  placeholder,
  ops,
  mono,
  caseLocked,
  onChange,
  compact,
}: {
  pattern: StringMatch;
  path: string;
  placeholder?: string;
  ops: readonly MatchOp[];
  mono: boolean;
  caseLocked?: boolean;
  onChange: (p: StringMatch) => void;
  compact?: boolean;
}) {
  const err = useIssue(`${path}.value`);
  const touch = useTouch();
  const opOptions = ops.map((op) => ({ value: op, label: OP_LABELS[op] }));
  const valuePlaceholder = pattern.op === 'regex' ? '^/api/v\\d+/' : pattern.op === 'glob' ? (placeholder?.includes('*') ? placeholder : `${placeholder ?? ''}*`) : placeholder;
  return (
    <div className={styles.patternWrap}>
      <div className={clsx(styles.patternRow, compact && styles.patternRowCompact)}>
        <Select
          value={pattern.op}
          onValueChange={(op) => onChange({ ...pattern, op, caseSensitive: op === 'regex' && !pattern.value ? true : pattern.caseSensitive })}
          options={opOptions}
          size="sm"
          className={styles.opSelect}
          aria-label="Operator"
        />
        {pattern.op !== 'exists' ? (
          <Input
            size="sm"
            mono={mono || pattern.op === 'regex'}
            value={pattern.value}
            placeholder={valuePlaceholder}
            invalid={!!err}
            onChange={(e) => onChange({ ...pattern, value: e.target.value })}
            onBlur={() => touch(`${path}.value`)}
            className={styles.patternInput}
            aria-label="Value"
            spellCheck={false}
          />
        ) : (
          <span className={styles.patternInput} />
        )}
        <CaseToggle caseSensitive={!caseLocked && pattern.caseSensitive} onChange={(caseSensitive) => onChange({ ...pattern, caseSensitive })} disabled={caseLocked || pattern.op === 'exists'} />
        <NegateToggle negate={!!pattern.negate} onChange={(negate) => onChange(negate ? { ...pattern, negate } : withoutNegate(pattern))} />
      </div>
      <FieldError>{err}</FieldError>
    </div>
  );
}

function withoutNegate(p: StringMatch): StringMatch {
  const { negate: _n, ...rest } = p;
  return rest;
}

// =====================================================================================
// other value kinds
// =====================================================================================

function ValueEditor({ matcher, path, depth, onChange }: { matcher: Matcher; path: string; depth: number; onChange: (m: Matcher) => void }) {
  switch (matcher.type) {
    case 'method':
      return <MethodChips methods={matcher.methods} path={`${path}.methods`} onChange={(methods) => onChange({ ...matcher, methods })} />;
    case 'port':
      return <PortsInput ports={matcher.ports} path={`${path}.ports`} onChange={(ports) => onChange({ ...matcher, ports })} />;
    case 'protocol':
      return <ChipSet options={PROTOCOLS} values={matcher.protocols} path={`${path}.protocols`} onChange={(protocols) => onChange({ ...matcher, protocols })} mono />;
    case 'http-version':
      return <ChipSet options={HTTP_VERSIONS} values={matcher.versions} path={`${path}.versions`} onChange={(versions) => onChange({ ...matcher, versions })} mono />;
    case 'query':
    case 'header':
    case 'cookie':
    case 'form': {
      const list = kvList(matcher) ?? [];
      const sub = matcher.type === 'query' ? 'params' : matcher.type === 'header' ? 'headers' : matcher.type === 'cookie' ? 'cookies' : 'fields';
      return <KeyValueMatches list={list} path={`${path}.${sub}`} type={matcher.type} onChange={(l) => onChange(withKvList(matcher, l))} />;
    }
    case 'json-body':
      return (
        <div className={styles.stack}>
          <JsonField value={matcher.value} onChange={(value) => onChange({ ...matcher, value })} minHeight={64} maxHeight={240} label="JSON body to match" />
          <Switch
            size="sm"
            checked={matcher.partial}
            onCheckedChange={(partial) => onChange({ ...matcher, partial })}
            label="Partial match"
            description={matcher.partial ? 'The body must contain these fields (extra fields are fine)' : 'The body must equal this value exactly'}
          />
        </div>
      );
    case 'any':
    case 'all':
      return <MatcherList matchers={matcher.matchers} onChange={(matchers) => onChange({ ...matcher, matchers })} path={`${path}.matchers`} joiner={matcher.type === 'any' ? 'or' : 'and'} depth={depth + 1} />;
    case 'not':
      return <NotEditor matcher={matcher} path={path} depth={depth} onChange={onChange} />;
    default:
      return null;
  }
}

function NotEditor({ matcher, path, depth, onChange }: { matcher: Extract<Matcher, { type: 'not' }>; path: string; depth: number; onChange: (m: Matcher) => void }) {
  return (
    <div className={clsx(styles.matcherList, styles.matcherListNested)} style={{ '--depth': depth + 1 } as CSSProperties}>
      <MatcherRowInner matcher={matcher.matcher} path={`${path}.matcher`} depth={depth + 1} onChange={(inner) => onChange({ ...matcher, matcher: inner })} />
    </div>
  );
}

/** The wrapped condition of a "Not": same row, but its type can be swapped instead of removed. */
function MatcherRowInner({ matcher, path, depth, onChange }: { matcher: Matcher; path: string; depth: number; onChange: (m: Matcher) => void }) {
  const info = matcherInfo(matcher.type);
  const Icon = iconFor(info.icon);
  const pattern = patternOf(matcher);
  return (
    <div className={styles.matcherRow} data-type={matcher.type}>
      <span className={styles.connector}>not</span>
      <span className={styles.matcherLabel} title={info.description}>
        <Icon size={13} strokeWidth={1.8} aria-hidden />
        <span className="truncate">{info.label}</span>
      </span>
      <div className={styles.matcherBody}>
        {pattern ? (
          <PatternEditor pattern={pattern} path={`${path}.pattern`} placeholder={info.placeholder} ops={info.ops ?? []} mono caseLocked={matcher.type === 'host'} onChange={(p) => onChange({ ...matcher, pattern: p } as Matcher)} />
        ) : (
          <ValueEditor matcher={matcher} path={path} depth={depth} onChange={onChange} />
        )}
      </div>
      <SwapConditionMenu onPick={(type) => onChange(defaultMatcher(type))} />
    </div>
  );
}

function SwapConditionMenu({ onPick }: { onPick: (type: MatcherType) => void }) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <IconButton label="Change condition type" icon={Ellipsis} size="sm" tooltip={false} className={styles.removeButton} />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuLabel>Change condition</DropdownMenuLabel>
        {MATCHERS.filter((m) => m.type !== 'not').map((m) => {
          const Icon = iconFor(m.icon);
          return (
            <DropdownMenuItem key={m.type} icon={Icon} onSelect={() => onPick(m.type)}>
              {m.label}
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

const PRIMARY_METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS'];

function MethodChips({ methods, path, onChange }: { methods: string[]; path: string; onChange: (m: string[]) => void }) {
  const err = useIssue(path, true);
  const extra = methods.filter((m) => !PRIMARY_METHODS.includes(m));
  const [custom, setCustom] = useState('');
  const toggle = (m: string) => onChange(methods.includes(m) ? methods.filter((x) => x !== m) : [...methods, m]);
  const others = HTTP_METHODS.filter((m) => !PRIMARY_METHODS.includes(m));
  return (
    <div>
      <div className={styles.chipSet} role="group" aria-label="Methods">
        {[...PRIMARY_METHODS, ...extra].map((m) => {
          const on = methods.includes(m);
          return (
            <button
              key={m}
              type="button"
              aria-pressed={on}
              className={clsx(styles.methodChip, on && styles.methodChipOn)}
              style={{ '--m': methodColorVar(m) } as CSSProperties}
              onClick={() => toggle(m)}
            >
              {m}
            </button>
          );
        })}
        <DropdownMenu modal={false}>
          <DropdownMenuTrigger asChild>
            <button type="button" className={styles.methodMore} aria-label="More methods">
              <Ellipsis size={14} aria-hidden />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            <DropdownMenuLabel>Other methods</DropdownMenuLabel>
            {others.map((m) => (
              <DropdownMenuItem key={m} onSelect={() => toggle(m)} hint={methods.includes(m) ? '✓' : undefined}>
                {m}
              </DropdownMenuItem>
            ))}
            <DropdownMenuSeparator />
            {/* biome-ignore lint/a11y/noStaticElementInteractions: keeps typing in the custom method field from reaching the list shortcuts */}
            <div className={styles.customMethod} onKeyDown={(e) => e.stopPropagation()}>
              <Input
                size="sm"
                mono
                value={custom}
                placeholder="Custom (e.g. PROPFIND)"
                onChange={(e) => setCustom(e.target.value.toUpperCase().replace(/[^A-Z0-9!#$%&'*+\-.^_`|~]/g, ''))}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && custom) {
                    if (!methods.includes(custom)) onChange([...methods, custom]);
                    setCustom('');
                  }
                }}
                aria-label="Custom method"
              />
            </div>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
      <FieldError>{err}</FieldError>
    </div>
  );
}

function ChipSet({ options, values, path, onChange, mono }: { options: readonly string[]; values: string[]; path: string; onChange: (v: string[]) => void; mono?: boolean }) {
  const err = useIssue(path, true);
  const lower = values.map((v) => v.toLowerCase());
  return (
    <div>
      <div className={styles.chipSet} role="group">
        {options.map((o) => {
          const on = lower.includes(o.toLowerCase());
          return (
            <button
              key={o}
              type="button"
              aria-pressed={on}
              className={clsx(styles.optionChip, on && styles.optionChipOn, mono && 'mono')}
              onClick={() => onChange(on ? values.filter((v) => v.toLowerCase() !== o.toLowerCase()) : [...values, o])}
            >
              {o}
            </button>
          );
        })}
      </div>
      <FieldError>{err}</FieldError>
    </div>
  );
}

function PortsInput({ ports, path, onChange }: { ports: number[]; path: string; onChange: (p: number[]) => void }) {
  const [text, setText] = useState(ports.join(', '));
  const last = useRef(ports);
  useEffect(() => {
    if (ports.join(',') !== last.current.join(',')) {
      last.current = ports;
      setText(ports.join(', '));
    }
  }, [ports]);
  const parsed = parsePorts(text);
  const localErr = parsed === null ? 'Ports are numbers from 1 to 65535, separated by commas' : null;
  useFieldError(localErr);
  const err = useIssue(path);
  return (
    <div>
      <Input
        size="sm"
        mono
        value={text}
        placeholder="443, 8443"
        invalid={!!(localErr ?? err)}
        onChange={(e) => {
          setText(e.target.value);
          const p = parsePorts(e.target.value);
          if (p) {
            last.current = p;
            onChange(p);
          }
        }}
        className={styles.portsInput}
        aria-label="Ports"
      />
      <FieldError>{localErr ?? err}</FieldError>
    </div>
  );
}

// =====================================================================================
// key/value matches (query, header, cookie, form)
// =====================================================================================

const KV_LABEL: Record<string, { add: string; key: string }> = {
  query: { add: 'Add parameter', key: 'param' },
  header: { add: 'Add header', key: 'header-name' },
  cookie: { add: 'Add cookie', key: 'cookie' },
  form: { add: 'Add field', key: 'field' },
};

function KeyValueMatches({ list, path, type, onChange }: { list: KeyValueMatch[]; path: string; type: string; onChange: (l: KeyValueMatch[]) => void }) {
  const err = useIssue(path);
  const labels = KV_LABEL[type] ?? { add: 'Add', key: 'name' };
  const [focusLast, setFocusLast] = useState(false);
  return (
    <div className={styles.kvMatches}>
      {list.map((kv, i) => (
        <KeyValueMatchRow
          key={i}
          kv={kv}
          path={`${path}.${i}`}
          type={type}
          keyPlaceholder={labels.key}
          autoFocus={focusLast && i === list.length - 1}
          onChange={(next) => onChange(list.map((x, j) => (j === i ? next : x)))}
          onRemove={list.length > 1 ? () => onChange(list.filter((_, j) => j !== i)) : undefined}
        />
      ))}
      <div>
        <Button
          size="sm"
          variant="ghost"
          icon={Plus}
          className={styles.addButton}
          onClick={() => {
            setFocusLast(true);
            onChange([...list, { key: '', value: stringMatch('equals') }]);
          }}
        >
          {labels.add}
        </Button>
      </div>
      <FieldError>{err}</FieldError>
    </div>
  );
}

const KV_OPS: MatchOp[] = ['exists', 'equals', 'contains', 'starts-with', 'ends-with', 'glob', 'regex'];

function KeyValueMatchRow({ kv, path, type, keyPlaceholder, autoFocus, onChange, onRemove }: { kv: KeyValueMatch; path: string; type: string; keyPlaceholder: string; autoFocus: boolean; onChange: (kv: KeyValueMatch) => void; onRemove?: () => void }) {
  const keyErr = useIssue(`${path}.key`);
  const valueErr = useIssue(`${path}.value.value`);
  const touch = useTouch();
  const value = kv.value ?? { op: 'exists' as MatchOp, value: '', caseSensitive: false };
  // `{ key }` without a value = "present"; `exists` + negate = "absent".
  const op = value.op;
  const setValue = (v: StringMatch) => onChange(v.op === 'exists' && !v.negate ? { key: kv.key } : { key: kv.key, value: v });
  const keyInput =
    type === 'header' ? (
      <Combobox
        size="sm"
        mono
        value={kv.key}
        onValueChange={(key) => onChange({ ...kv, key })}
        options={HEADER_SUGGESTIONS}
        openOnFocus={false}
        placeholder={keyPlaceholder}
        invalid={!!keyErr}
        onBlur={() => touch(`${path}.key`)}
        autoFocus={autoFocus}
        className={styles.kvKey}
        aria-label="Name"
      />
    ) : (
      <Input size="sm" mono value={kv.key} onChange={(e) => onChange({ ...kv, key: e.target.value })} placeholder={keyPlaceholder} invalid={!!keyErr} onBlur={() => touch(`${path}.key`)} autoFocus={autoFocus} className={styles.kvKey} aria-label="Name" />
    );
  return (
    <div>
      <div className={styles.kvRow}>
        {keyInput}
        <Select
          value={op}
          onValueChange={(o) => setValue({ ...value, op: o, caseSensitive: o === 'regex' && !value.value ? true : value.caseSensitive })}
          options={KV_OPS.map((o) => ({ value: o, label: o === 'exists' ? (value.negate ? 'is absent' : 'is present') : OP_LABELS[o] }))}
          size="sm"
          className={styles.kvOp}
          aria-label="Operator"
        />
        {op !== 'exists' ? (
          <Input size="sm" mono value={value.value} onChange={(e) => setValue({ ...value, value: e.target.value })} placeholder={op === 'regex' ? 'regex' : 'value'} invalid={!!valueErr} className={styles.kvValue} aria-label="Value" spellCheck={false} />
        ) : (
          <span className={styles.kvValue} />
        )}
        <CaseToggle caseSensitive={value.caseSensitive} onChange={(caseSensitive) => setValue({ ...value, caseSensitive })} disabled={op === 'exists'} />
        <NegateToggle negate={!!value.negate} onChange={(negate) => setValue(negate ? { ...value, negate } : withoutNegate(value))} />
        {onRemove ? <IconButton label="Remove" icon={X} size="sm" onClick={onRemove} /> : <span className={styles.kvRemoveSpacer} />}
      </div>
      <FieldError>{keyErr ?? valueErr}</FieldError>
    </div>
  );
}
