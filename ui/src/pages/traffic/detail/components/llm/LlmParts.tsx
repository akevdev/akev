/**
 * Renderers for the pieces of an LLM conversation: message bubbles, text (Markdown or plain),
 * thinking, tool calls / results, images, files, refusals, and a lightweight highlighted JSON
 * block. Everything is rendered through React (escaped); Markdown links are vetted by the
 * shared parser and remote images are never loaded.
 */

import clsx from 'clsx';
import { Brain, ChevronDown, ChevronRight, CornerDownRight, FileText, ImageOff, Lock, Paperclip, Settings2, ShieldAlert, Sparkles, User, Wrench } from 'lucide-react';
import { memo, useMemo, useState, type ReactNode } from 'react';
import type { LlmMessage, LlmPart } from '../../../../../api/types';
import { CopyButton } from '../../../../../components/ui';
import { parseMarkdown } from '../../../../../components/exchange/model/markdown';
import { Markdown } from '../../../../../components/exchange/parts/Parts';
import { formatBytes } from '../../../../../lib/format';
import { charCount, chatBreaks, displayJson, highlightJson, imageSrc, isLongText, messageText, messageTone, parseJsonText, prettyValue, roleLabel, type MessageTone } from '../../../../../lib/llm/conversation';
import styles from './LlmView.module.css';

/** Characters of a long text rendered before "Show all". */
const PREVIEW_CHARS = 1400;

// =====================================================================================
// Collapsible long content
// =====================================================================================

function useExpand(long: boolean) {
  const [open, setOpen] = useState(false);
  return { expanded: !long || open, toggle: () => setOpen((v) => !v) };
}

function ShowMore({ expanded, onToggle, total }: { expanded: boolean; onToggle: () => void; total: string }) {
  return (
    <button type="button" className={styles.showMore} onClick={onToggle} aria-expanded={expanded}>
      {expanded ? 'Show less' : `Show all · ${total}`}
      <ChevronDown size={12} strokeWidth={2} aria-hidden className={clsx(expanded && styles.flip)} />
    </button>
  );
}

// =====================================================================================
// JSON
// =====================================================================================

/** Highlighted, pretty JSON (or any value); long blocks collapse. */
export const JsonBlock = memo(function JsonBlock({ value, className, maxLines = 22 }: { value: unknown; className?: string; maxLines?: number }) {
  const text = useMemo(() => displayJson(value), [value]);
  const long = isLongText(text, 2400, maxLines);
  const { expanded, toggle } = useExpand(long);
  const shown = expanded ? text : text.split('\n').slice(0, maxLines).join('\n');
  const tokens = useMemo(() => highlightJson(shown), [shown]);
  return (
    <div className={clsx(styles.jsonWrap, className)}>
      <pre className={clsx(styles.json, !expanded && styles.clipped)}>
        {tokens.map((t, i) =>
          t.k === 't' ? (
            t.v
          ) : (
            <span key={i} className={styles[`j${t.k}`]}>
              {t.v}
            </span>
          ),
        )}
      </pre>
      {long && <ShowMore expanded={expanded} onToggle={toggle} total={`${text.split('\n').length} lines`} />}
    </div>
  );
});

// =====================================================================================
// Text
// =====================================================================================

function TextPart({ text, markdown }: { text: string; markdown: boolean }) {
  const long = isLongText(text);
  const { expanded, toggle } = useExpand(long);
  const shown = expanded ? text : `${text.slice(0, PREVIEW_CHARS).trimEnd()}…`;
  const blocks = useMemo(() => (markdown ? parseMarkdown(chatBreaks(shown)) : null), [markdown, shown]);
  return (
    <div className={styles.textPart}>
      {blocks ? <Markdown blocks={blocks} compact className={styles.md} /> : <div className={styles.plain}>{shown}</div>}
      {long && <ShowMore expanded={expanded} onToggle={toggle} total={charCount(text)} />}
    </div>
  );
}

function ThinkingPart({ text, redacted, markdown }: { text: string; redacted?: boolean; markdown: boolean }) {
  const [open, setOpen] = useState(false);
  const blocks = useMemo(() => (open && markdown && text ? parseMarkdown(chatBreaks(text)) : null), [open, markdown, text]);
  if (redacted || !text) {
    return (
      <div className={clsx(styles.thinking, styles.thinkingRedacted)}>
        <span className={styles.thinkingHead}>
          <Lock size={12} strokeWidth={2} aria-hidden />
          Thinking (encrypted — not readable)
        </span>
      </div>
    );
  }
  return (
    <div className={styles.thinking}>
      <button type="button" className={styles.thinkingHead} onClick={() => setOpen((v) => !v)} aria-expanded={open}>
        <ChevronRight size={12} strokeWidth={2} className={clsx(styles.chev, open && styles.chevOpen)} aria-hidden />
        <Brain size={12} strokeWidth={2} aria-hidden />
        Thinking
        <span className={styles.subtleMeta}>{charCount(text)}</span>
      </button>
      {open && (blocks ? <Markdown blocks={blocks} compact className={clsx(styles.md, styles.thinkingMd)} /> : <div className={styles.thinkingBody}>{text}</div>)}
    </div>
  );
}

// =====================================================================================
// Tools
// =====================================================================================

function ToolCallCard({ p }: { p: Extract<LlmPart, { type: 'tool-call' }> }) {
  return (
    <div className={styles.toolCard}>
      <div className={styles.toolHead}>
        <span className={styles.toolIcon}>
          <Wrench size={11} strokeWidth={2.2} aria-hidden />
        </span>
        <span className={styles.toolName}>{p.name}</span>
        {p.id && (
          <span className={styles.toolId} title={p.id}>
            {p.id}
          </span>
        )}
        <CopyButton value={() => prettyValue(p.arguments)} label="Copy arguments" className={styles.cardCopy} />
      </div>
      <JsonBlock value={p.arguments} />
    </div>
  );
}

function ToolResultCard({ p, toolName, markdown }: { p: Extract<LlmPart, { type: 'tool-result' }>; toolName?: string; markdown: boolean }) {
  const name = p.name ?? toolName;
  const single = p.content.length === 1 && p.content[0]?.type === 'text' ? p.content[0].text : null;
  const json = single != null ? parseJsonText(single) : undefined;
  return (
    <div className={clsx(styles.toolCard, styles.resultCard, p.isError && styles.errorCard)}>
      <div className={styles.toolHead}>
        <span className={clsx(styles.toolIcon, styles.resultIcon)}>
          <CornerDownRight size={11} strokeWidth={2.2} aria-hidden />
        </span>
        <span className={styles.resultLabel}>{p.isError ? 'Error' : 'Result'}</span>
        {name && <span className={styles.toolName}>{name}</span>}
        {p.id && (
          <span className={styles.toolId} title={p.id}>
            {p.id}
          </span>
        )}
        <CopyButton value={() => p.content.map((c) => (c.type === 'text' ? c.text : messageText({ role: 'tool', content: [c] }))).join('\n\n')} label="Copy result" className={styles.cardCopy} />
      </div>
      {p.content.length === 0 ? (
        <div className={styles.cardEmpty}>No content</div>
      ) : json !== undefined ? (
        <JsonBlock value={json} />
      ) : single != null ? (
        <CodeText text={single} />
      ) : (
        <div className={styles.cardParts}>
          {p.content.map((c, i) => (
            <PartView key={i} p={c} tone="tool" markdown={markdown} />
          ))}
        </div>
      )}
    </div>
  );
}

/** Monospace text (tool output, code) with a long-content fold. */
function CodeText({ text }: { text: string }) {
  const long = isLongText(text, 2400, 22);
  const { expanded, toggle } = useExpand(long);
  const shown = expanded ? text : text.split('\n').slice(0, 22).join('\n').slice(0, 2400);
  return (
    <div className={styles.jsonWrap}>
      <pre className={clsx(styles.code, !expanded && styles.clipped)}>{shown}</pre>
      {long && <ShowMore expanded={expanded} onToggle={toggle} total={charCount(text)} />}
    </div>
  );
}

// =====================================================================================
// Media & misc
// =====================================================================================

function ImagePart({ p }: { p: Extract<LlmPart, { type: 'image' }> }) {
  const [big, setBig] = useState(false);
  const src = imageSrc(p);
  const meta = [p.mediaType?.replace('image/', '').toUpperCase(), p.size != null ? formatBytes(p.size) : null].filter(Boolean).join(' · ');
  if (!src) {
    return (
      <div className={styles.fileChip} title={p.url}>
        <ImageOff size={13} strokeWidth={2} aria-hidden />
        <span className={styles.fileLabel}>{p.url ? 'Remote image' : 'Image'}</span>
        <span className={styles.fileName}>{p.url ?? (meta || 'not included')}</span>
        {p.url && <CopyButton value={p.url} label="Copy image URL" className={styles.cardCopy} />}
      </div>
    );
  }
  return (
    <figure className={clsx(styles.image, big && styles.imageBig)}>
      <button type="button" className={styles.imageButton} onClick={() => setBig((v) => !v)} aria-label={big ? 'Shrink image' : 'Enlarge image'}>
        <img src={src} alt={`Attachment${meta ? ` (${meta})` : ''}`} draggable={false} />
      </button>
      {meta && <figcaption className={styles.imageMeta}>{meta}</figcaption>}
    </figure>
  );
}

function FilePart({ p }: { p: Extract<LlmPart, { type: 'file' }> }) {
  return (
    <div className={styles.fileChip} title={p.url}>
      {p.label === 'document' ? <FileText size={13} strokeWidth={2} aria-hidden /> : <Paperclip size={13} strokeWidth={2} aria-hidden />}
      <span className={styles.fileLabel}>{p.label.charAt(0).toUpperCase() + p.label.slice(1)}</span>
      {(p.name || p.url) && <span className={styles.fileName}>{p.name ?? p.url}</span>}
      <span className={styles.subtleMeta}>{[p.mediaType, p.size != null ? formatBytes(p.size) : null].filter(Boolean).join(' · ')}</span>
    </div>
  );
}

function OtherPart({ p }: { p: Extract<LlmPart, { type: 'other' }> }) {
  const [open, setOpen] = useState(false);
  return (
    <div className={styles.toolCard}>
      <button type="button" className={clsx(styles.toolHead, styles.otherHead)} onClick={() => setOpen((v) => !v)} aria-expanded={open}>
        <ChevronRight size={12} strokeWidth={2} className={clsx(styles.chev, open && styles.chevOpen)} aria-hidden />
        <span className={styles.toolName}>{p.label}</span>
        <span className={styles.subtleMeta}>block</span>
      </button>
      {open && <JsonBlock value={p.value} />}
    </div>
  );
}

// =====================================================================================
// Parts & messages
// =====================================================================================

export interface PartViewProps {
  p: LlmPart;
  tone: MessageTone;
  markdown: boolean;
  toolNames?: ReadonlyMap<string, string>;
}

export function PartView({ p, tone, markdown, toolNames }: PartViewProps) {
  switch (p.type) {
    case 'text':
      return <TextPart text={p.text} markdown={markdown && tone === 'assistant'} />;
    case 'thinking':
      return <ThinkingPart text={p.text} redacted={p.redacted} markdown={markdown} />;
    case 'tool-call':
      return <ToolCallCard p={p} />;
    case 'tool-result':
      return <ToolResultCard p={p} toolName={p.id ? toolNames?.get(p.id) : undefined} markdown={markdown} />;
    case 'image':
      return <ImagePart p={p} />;
    case 'file':
      return <FilePart p={p} />;
    case 'refusal':
      return (
        <div className={styles.refusal}>
          <ShieldAlert size={13} strokeWidth={2} aria-hidden />
          <span>
            <strong>Refused:</strong> {p.text}
          </span>
        </div>
      );
    case 'other':
      return <OtherPart p={p} />;
  }
}

const TONE_ICONS = { user: User, assistant: Sparkles, tool: Wrench, system: Settings2 } as const;

export interface MessageBubbleProps {
  m: LlmMessage;
  index?: number;
  markdown: boolean;
  toolNames?: ReadonlyMap<string, string>;
  /** The model's reply (highlighted). */
  response?: boolean;
  /** Still streaming: show a caret. */
  streaming?: boolean;
  /** Extra content in the head row (stop reason…). */
  aside?: ReactNode;
}

export const MessageBubble = memo(function MessageBubble({ m, index, markdown, toolNames, response, streaming, aside }: MessageBubbleProps) {
  const tone = response ? 'assistant' : messageTone(m);
  const Icon = TONE_ICONS[tone];
  const label = tone === 'tool' && m.role !== 'tool' ? 'Tool results' : roleLabel(m.role);
  return (
    <article className={clsx(styles.msg, styles[`tone_${tone}`], response && styles.response)} aria-label={`${label} message`}>
      <header className={styles.msgHead}>
        <span className={styles.avatar}>
          <Icon size={11} strokeWidth={2.2} aria-hidden />
        </span>
        <span className={styles.role}>{label}</span>
        {m.name && <span className={styles.msgName}>{m.name}</span>}
        {response ? <span className={styles.responsePill}>Response</span> : index != null && <span className={styles.msgIndex}>#{index + 1}</span>}
        {aside}
        <CopyButton value={() => messageText(m)} label="Copy message" className={styles.msgCopy} />
      </header>
      <div className={styles.bubble}>
        {m.content.length === 0 && !streaming ? <span className={styles.emptyMsg}>Empty message</span> : m.content.map((p, i) => <PartView key={i} p={p} tone={tone} markdown={markdown} toolNames={toolNames} />)}
        {streaming && <span className={styles.caret} role="img" aria-label="Streaming" />}
      </div>
    </article>
  );
});
