/**
 * The "LLM" response tab: a recognised LLM API call shown as a conversation — provider, model,
 * token usage and latency up top, then the system prompt, tool definitions, the messages sent
 * and the model's reply (streamed responses reassembled by the daemon, filling in live).
 */

import clsx from 'clsx';
import { Braces, ChevronRight, ChevronsUp, Copy, FileText, Pilcrow, Radio, ScrollText, Sparkles, TriangleAlert, Unplug, Wrench } from 'lucide-react';
import { memo, useMemo, useState, type ReactNode } from 'react';
import type { ExchangeSummary, LlmExchange, LlmTool } from '../../../../../api/types';
import { ProviderMark } from '../../../../../components/exchange/llm/ProviderMark';
import { ProCallout } from '../../../../../components/pro';
import { Banner } from '../../../../../components/exchange/parts/Parts';
import { Badge, Button, CopyButton, EmptyState, IconButton, Menu, Popover, PopoverContent, PopoverTrigger, Skeleton, Spinner, toast, type BadgeVariant } from '../../../../../components/ui';
import { formatDuration, pluralize } from '../../../../../lib/format';
import { charCount, conversationJson, conversationMarkdown, conversationStats, formatTokens, formatTokensPerSecond, paramItems, stopReasonInfo, toolCallNames, usageItems, type Tone } from '../../../../../lib/llm/conversation';
import { apiLabel, providerInfo } from '../../../../../lib/llm/providers';
import { copyText } from '../../../../../lib/platform';
import { useFeature } from '../../../../../state/license';
import { usePagePref } from '../../../../../state/ui';
import { llmVersion, useLlmExchange } from '../../hooks/useLlmExchange';
import { JsonBlock, MessageBubble } from './LlmParts';
import styles from './LlmView.module.css';

const LIVE: ReadonlySet<string> = new Set(['requesting', 'waiting', 'responding', 'streaming']);

/** Above this many request messages only the latest are shown until asked. */
const FOLD_MESSAGES = 14;
const SHOWN_WHEN_FOLDED = 10;

const TONE_BADGE: Record<Tone, BadgeVariant> = { success: 'success', info: 'info', warning: 'warning', danger: 'danger', neutral: 'neutral' };

export interface LlmViewProps {
  summary: ExchangeSummary;
}

/**
 * Fetches `GET /api/exchanges/:id/llm` (live while streaming) and renders the conversation.
 * Without Akev Pro it shows what the view does instead — and never asks the daemon.
 */
export function LlmView({ summary }: LlmViewProps) {
  const licensed = useFeature('llm-lens');
  if (!licensed) return <LlmLocked summary={summary} />;
  return <LlmConversationView summary={summary} />;
}

function LlmLocked({ summary }: LlmViewProps) {
  const provider = summary.llm ? providerInfo(summary.llm.provider) : null;
  const what = provider ? `${provider.name} call${summary.llm?.model ? ` to ${summary.llm.model}` : ''}` : 'LLM API call';
  return (
    <div className={styles.center}>
      <ProCallout feature="llm-lens" variant="panel" title="Read this call as a conversation" className={styles.locked}>
        The LLM lens shows this {what} as a chat — system prompt, tools, messages, the streamed reply, tokens and latency. It's part of Akev Pro; the other tabs show the raw exchange.
      </ProCallout>
    </div>
  );
}

function LlmConversationView({ summary }: LlmViewProps) {
  const live = LIVE.has(summary.state) && !summary.paused;
  const { data, error, notLlm, locked, reload } = useLlmExchange(summary.id, llmVersion(summary), live);
  if (locked) return <LlmLocked summary={summary} />;
  if (!data) {
    if (error)
      return (
        <div className={styles.center}>
          <EmptyState icon={Unplug} tone="danger" size="sm" title="Couldn't load the conversation" description={error} actions={<Button size="sm" onClick={reload}>Retry</Button>} />
        </div>
      );
    if (notLlm)
      return (
        <div className={styles.center}>
          <EmptyState icon={Sparkles} size="sm" title="Not an LLM API call" description="Akev couldn't read this exchange as a call to a supported LLM API. The Body tab shows it as captured." />
        </div>
      );
    return <LoadingConversation />;
  }
  return <Conversation x={data} live={live} requestedModel={summary.llm?.model} />;
}

function LoadingConversation() {
  return (
    <div className={styles.root} aria-busy>
      <div className={styles.hero}>
        <Skeleton width="40%" height={14} />
        <Skeleton width="60%" height={18} />
        <Skeleton lines={2} height={10} />
      </div>
      <Skeleton lines={6} height={10} />
    </div>
  );
}

// =====================================================================================
// Conversation
// =====================================================================================

const Conversation = memo(function Conversation({ x, live, requestedModel }: { x: LlmExchange; live: boolean; requestedModel?: string }) {
  const [markdown, setMarkdown] = usePagePref<boolean>('traffic.llm.markdown', true);
  const names = useMemo(() => toolCallNames(x), [x]);
  const stats = useMemo(() => conversationStats(x), [x]);
  const messages = x.request.messages;
  const [showAll, setShowAll] = useState(false);
  const hidden = !showAll && messages.length > FOLD_MESSAGES ? messages.length - SHOWN_WHEN_FOLDED : 0;

  return (
    <div className={styles.root}>
      <Hero x={x} live={live} requestedModel={requestedModel} markdown={markdown} onMarkdown={setMarkdown} />

      {x.error && <ErrorNotice x={x} />}
      {x.warnings && x.warnings.length > 0 && (
        <Banner tone="warning" icon={TriangleAlert} className={styles.banner}>
          {x.warnings.length === 1 ? x.warnings[0] : (
            <ul className={styles.warnList}>
              {x.warnings.map((w) => (
                <li key={w}>{w}</li>
              ))}
            </ul>
          )}
        </Banner>
      )}

      {x.request.system && <SystemPrompt text={x.request.system} />}
      {x.request.tools.length > 0 && <ToolsList tools={x.request.tools} />}

      <SectionDivider
        label="Conversation"
        meta={[pluralize(stats.messages, 'message'), stats.toolCalls ? pluralize(stats.toolCalls, 'tool call') : null, stats.images ? pluralize(stats.images, 'image') : null].filter(Boolean).join(' · ')}
      />
      <div className={styles.thread}>
        {hidden > 0 && (
          <button type="button" className={styles.earlier} onClick={() => setShowAll(true)}>
            <ChevronsUp size={13} strokeWidth={2} aria-hidden />
            Show {pluralize(hidden, 'earlier message')}
          </button>
        )}
        {messages.slice(hidden).map((m, i) => (
          <MessageBubble key={hidden + i} m={m} index={hidden + i} markdown={markdown} toolNames={names} />
        ))}
        {messages.length === 0 && <div className={styles.quiet}>No messages in the request.</div>}
      </div>

      <ResponseSection x={x} live={live} markdown={markdown} names={names} />
    </div>
  );
});

// =====================================================================================
// Hero: provider, model, metrics, parameters
// =====================================================================================

function Hero({ x, live, requestedModel, markdown, onMarkdown }: { x: LlmExchange; live: boolean; requestedModel?: string; markdown: boolean; onMarkdown: (v: boolean) => void }) {
  const info = providerInfo(x.provider);
  const stop = stopReasonInfo(x.response?.stopReason);
  const model = x.model ?? requestedModel;
  const copy = async (text: string, what: string) => {
    if (await copyText(text)) toast.success(`Conversation copied as ${what}`);
    else toast.error('Copy failed', { description: 'Clipboard unavailable' });
  };
  return (
    <header className={styles.hero}>
      <div className={styles.heroTop}>
        <ProviderMark provider={x.provider} size={24} />
        <div className={styles.heroTitle}>
          <span className={styles.providerName}>
            {info.name}
            {info.local && <span className={styles.localTag}>local</span>}
          </span>
          <span className={styles.apiLabel}>{apiLabel(x.api)}</span>
        </div>
        <div className={styles.heroBadges}>
          {x.stream && (
            <Badge size="sm" variant={live ? 'accent' : 'neutral'} pulse={live} icon={live ? undefined : Radio}>
              {live ? 'Streaming' : 'Streamed'}
            </Badge>
          )}
          {stop && (
            <Badge size="sm" variant={TONE_BADGE[stop.tone]} title={`Stop reason: ${stop.raw}`}>
              {stop.label}
            </Badge>
          )}
          {x.error && (
            <Badge size="sm" variant="danger">
              Error{x.error.status ? ` ${x.error.status}` : ''}
            </Badge>
          )}
        </div>
        <div className={styles.heroActions}>
          <IconButton label={markdown ? 'Show replies as plain text' : 'Render replies as Markdown'} icon={Pilcrow} active={markdown} onClick={() => onMarkdown(!markdown)} />
          <Menu
            align="end"
            trigger={<IconButton label="Copy conversation" icon={Copy} tooltip={false} />}
            items={[
              { label: 'Copy as Markdown', icon: FileText, onSelect: () => void copy(conversationMarkdown(x), 'Markdown') },
              { label: 'Copy as JSON', icon: Braces, onSelect: () => void copy(conversationJson(x), 'JSON') },
            ]}
          />
        </div>
      </div>
      <div className={styles.modelRow}>
        <span className={styles.model} title={model}>
          {model ?? 'Unknown model'}
        </span>
        {requestedModel && model && requestedModel !== model && <span className={styles.requested}>requested {requestedModel}</span>}
      </div>
      <Metrics x={x} />
      <Params params={x.request.params} />
    </header>
  );
}

interface Metric {
  key: string;
  label: string;
  value: string;
  sub?: string;
  hint: string;
  accent?: boolean;
}

function Metrics({ x }: { x: LlmExchange }) {
  const usage = x.response?.usage;
  const t = x.timing;
  const metrics: Metric[] = [];
  for (const u of usageItems(usage)) {
    if (u.key === 'reasoningTokens') continue;
    let sub: string | undefined;
    if (u.key === 'inputTokens' && usage?.cacheReadTokens && u.value > 0) sub = `${Math.round((usage.cacheReadTokens / u.value) * 100)}% cached`;
    if (u.key === 'outputTokens' && usage?.reasoningTokens) sub = `${formatTokens(usage.reasoningTokens)} reasoning`;
    metrics.push({ key: u.key, label: u.label, value: formatTokens(u.value), sub, hint: u.hint });
  }
  if (t.ttftMs != null) metrics.push({ key: 'ttft', label: 'TTFT', value: formatDuration(t.ttftMs), hint: 'Time to first token, from the request being sent', accent: true });
  if (t.tokensPerSecond != null) metrics.push({ key: 'tps', label: 'Speed', value: formatTokensPerSecond(t.tokensPerSecond), hint: 'Output tokens per second of generation' });
  if (t.totalMs != null) metrics.push({ key: 'total', label: 'Total', value: formatDuration(t.totalMs), sub: t.chunks ? pluralize(t.chunks, 'chunk') : undefined, hint: 'From the request being sent to the response being complete' });
  if (metrics.length === 0) return null;
  return (
    <dl className={styles.metrics}>
      {metrics.map((m) => (
        <div key={m.key} className={clsx(styles.metric, m.accent && styles.metricAccent)} title={m.hint}>
          <dt className={styles.metricLabel}>{m.label}</dt>
          <dd className={styles.metricValue}>{m.value}</dd>
          {m.sub && <dd className={styles.metricSub}>{m.sub}</dd>}
        </div>
      ))}
    </dl>
  );
}

function Params({ params }: { params: Record<string, unknown> }) {
  const items = useMemo(() => paramItems(params), [params]);
  if (items.length === 0) return null;
  return (
    <div className={styles.params} role="group" aria-label="Request parameters">
      {items.map((p) =>
        p.complex ? (
          <Popover key={p.key}>
            <PopoverTrigger asChild>
              <button type="button" className={clsx(styles.param, styles.paramComplex)}>
                <span className={styles.paramKey}>{p.key}</span>
                <span className={styles.paramValue}>{p.text}</span>
              </button>
            </PopoverTrigger>
            <PopoverContent width={420} align="start" className={styles.paramPopover}>
              <div className={styles.popoverHead}>
                <span className={styles.toolName}>{p.key}</span>
                <CopyButton value={() => JSON.stringify(p.value, null, 2)} label="Copy value" />
              </div>
              <JsonBlock value={p.value} maxLines={30} />
            </PopoverContent>
          </Popover>
        ) : (
          <span key={p.key} className={styles.param}>
            <span className={styles.paramKey}>{p.key}</span>
            <span className={styles.paramValue}>{p.text}</span>
          </span>
        ),
      )}
    </div>
  );
}

function ErrorNotice({ x }: { x: LlmExchange }) {
  const e = x.error;
  if (!e) return null;
  const detail = [e.kind, e.code && e.code !== String(e.status) ? e.code : null].filter(Boolean).join(' · ');
  return (
    <Banner tone="danger" className={styles.banner}>
      <span className={styles.errorTitle}>
        {e.status ? `HTTP ${e.status} — ` : ''}
        {e.message}
      </span>
      {detail && <span className={styles.errorDetail}>{detail}</span>}
    </Banner>
  );
}

// =====================================================================================
// System prompt & tools
// =====================================================================================

function Collapsible({ icon: Icon, title, meta, defaultOpen, actions, preview, children }: { icon: typeof ScrollText; title: string; meta?: string; defaultOpen: boolean; actions?: ReactNode; preview?: ReactNode; children: ReactNode }) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section className={clsx(styles.block, open && styles.blockOpen)}>
      <div className={styles.blockHead}>
        <button type="button" className={styles.blockToggle} onClick={() => setOpen((v) => !v)} aria-expanded={open}>
          <ChevronRight size={13} strokeWidth={2} className={clsx(styles.chev, open && styles.chevOpen)} aria-hidden />
          <Icon size={13} strokeWidth={2} aria-hidden className={styles.blockIcon} />
          <span className={styles.blockTitle}>{title}</span>
          {meta && <span className={styles.subtleMeta}>{meta}</span>}
        </button>
        {actions}
      </div>
      {open ? <div className={styles.blockBody}>{children}</div> : preview}
    </section>
  );
}

function SystemPrompt({ text }: { text: string }) {
  const short = text.length <= 600 && text.split('\n').length <= 8;
  return (
    <Collapsible
      icon={ScrollText}
      title="System prompt"
      meta={charCount(text)}
      defaultOpen={short}
      actions={<CopyButton value={text} label="Copy system prompt" className={styles.blockCopy} />}
      preview={<div className={styles.systemPreview}>{text.slice(0, 400)}</div>}
    >
      <div className={styles.systemText}>{text}</div>
    </Collapsible>
  );
}

function ToolsList({ tools }: { tools: readonly LlmTool[] }) {
  return (
    <Collapsible
      icon={Wrench}
      title="Tools"
      meta={String(tools.length)}
      defaultOpen={false}
      preview={
        <div className={styles.toolPreview}>
          {tools.map((t, i) => (
            <span key={`${t.name}-${i}`} className={styles.toolChip}>
              {t.name}
            </span>
          ))}
        </div>
      }
    >
      <ul className={styles.toolList}>
        {tools.map((t, i) => (
          <ToolRow key={`${t.name}-${i}`} t={t} />
        ))}
      </ul>
    </Collapsible>
  );
}

function ToolRow({ t }: { t: LlmTool }) {
  const [open, setOpen] = useState(false);
  const hasSchema = t.schema != null;
  return (
    <li className={styles.toolRow}>
      <button type="button" className={styles.toolRowHead} onClick={() => setOpen((v) => !v)} aria-expanded={open} disabled={!hasSchema}>
        {hasSchema ? <ChevronRight size={12} strokeWidth={2} className={clsx(styles.chev, open && styles.chevOpen)} aria-hidden /> : <span className={styles.chevSpacer} />}
        <span className={styles.toolName}>{t.name}</span>
        {t.kind && <span className={styles.kindTag}>{t.kind}</span>}
        {t.description && <span className={styles.toolDesc}>{t.description}</span>}
      </button>
      {open && hasSchema && <JsonBlock value={t.schema} className={styles.schema} maxLines={30} />}
    </li>
  );
}

// =====================================================================================
// Response
// =====================================================================================

function SectionDivider({ label, meta, accent }: { label: string; meta?: string; accent?: boolean }) {
  return (
    <div className={clsx(styles.divider, accent && styles.dividerAccent)} role="separator" aria-label={label}>
      <span className={styles.dividerLabel}>{label}</span>
      {meta && <span className={styles.dividerMeta}>{meta}</span>}
    </div>
  );
}

function ResponseSection({ x, live, markdown, names }: { x: LlmExchange; live: boolean; markdown: boolean; names: ReadonlyMap<string, string> }) {
  const r = x.response;
  const out = r?.usage?.outputTokens;
  const meta = [out != null ? `${formatTokens(out)} tokens` : null, x.timing.totalMs != null ? formatDuration(x.timing.totalMs) : null, r?.stopReason ?? null].filter(Boolean).join(' · ');
  if (!r && x.error) return null;
  return (
    <>
      <SectionDivider label="Response" meta={meta} accent />
      <div className={styles.thread}>
        {!r || r.messages.length === 0 ? (
          live ? (
            <div className={styles.waiting}>
              <Spinner size={13} tone="muted" />
              {r ? 'Streaming — waiting for the first token…' : 'Waiting for the model…'}
            </div>
          ) : (
            <div className={styles.quiet}>{r ? 'The model returned no content.' : 'No response was received.'}</div>
          )
        ) : (
          r.messages.map((m, i) => <MessageBubble key={i} m={m} markdown={markdown} toolNames={names} response streaming={live && i === r.messages.length - 1} />)
        )}
      </div>
      {r?.id && (
        <div className={styles.responseId}>
          <span>{r.id}</span>
          <CopyButton value={r.id} label="Copy response id" />
        </div>
      )}
    </>
  );
}

