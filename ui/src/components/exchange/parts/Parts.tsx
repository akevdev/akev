/**
 * Small shared building blocks for the exchange composites: notice banners, key/value info
 * grids, content-kind icons, chips and the safe Markdown renderer.
 */

import clsx from 'clsx';
import {
  AlertTriangle,
  Binary,
  Braces,
  CircleAlert,
  Code2,
  File,
  FileText,
  Film,
  Globe,
  Image as ImageIcon,
  Info,
  Layers,
  ListTree,
  Music,
  Radio,
  Table2,
  Type,
  type LucideIcon,
} from 'lucide-react';
import { Fragment, memo, useMemo, type MouseEvent, type ReactNode } from 'react';
import type { ContentKind } from '../../../api/types';
import { contentKindColorVar } from '../../../lib/http';
import { openExternal } from '../../../lib/platform';
import { CopyButton } from '../../ui';
import { parseInline, type Block, type Inline } from '../model/markdown';
import styles from './parts.module.css';

// =====================================================================================
// Banner
// =====================================================================================

export interface BannerProps {
  tone?: 'warning' | 'info' | 'danger' | 'neutral';
  icon?: LucideIcon;
  children: ReactNode;
  actions?: ReactNode;
  className?: string;
}

const BANNER_ICONS = { warning: AlertTriangle, info: Info, danger: CircleAlert, neutral: Info } as const;

/** Full-width notice row (decode failures, truncation, previews). */
export function Banner({ tone = 'info', icon, children, actions, className }: BannerProps) {
  const Icon = icon ?? BANNER_ICONS[tone];
  return (
    <div className={clsx(styles.banner, styles[tone], className)} role={tone === 'danger' || tone === 'warning' ? 'alert' : 'status'}>
      <Icon size={14} strokeWidth={2} aria-hidden />
      <div className={styles.bannerText}>{children}</div>
      {actions && <div className={styles.bannerActions}>{actions}</div>}
    </div>
  );
}

// =====================================================================================
// InfoGrid
// =====================================================================================

export interface InfoRow {
  label: ReactNode;
  value: ReactNode;
  /** Text copied by the hover copy button. */
  copy?: string;
  mono?: boolean;
  /** Muted trailing hint. */
  hint?: ReactNode;
  /** Hide the row when the value is missing. */
  hidden?: boolean;
}

/** Two-column label/value list with hover copy buttons. */
export function InfoGrid({ rows, className }: { rows: readonly InfoRow[]; className?: string }) {
  const visible = rows.filter((r) => !r.hidden);
  return (
    <dl className={clsx(styles.grid, className)}>
      {visible.map((r, i) => (
        <div key={i} className={styles.gridRow}>
          <dt className={styles.gridLabel}>{r.label}</dt>
          <dd className={clsx(styles.gridValue, r.mono && styles.mono)}>
            <span className={styles.gridValueInner}>
              {r.value ?? <span className={styles.empty}>—</span>}
              {r.hint != null && <span className={styles.gridHint}>{r.hint}</span>}
            </span>
            {r.copy != null && r.copy !== '' && <CopyButton value={r.copy} label={`Copy ${typeof r.label === 'string' ? r.label.toLowerCase() : 'value'}`} className={styles.gridCopy} />}
          </dd>
        </div>
      ))}
    </dl>
  );
}

// =====================================================================================
// Kind icon & chips
// =====================================================================================

const KIND_ICONS: Partial<Record<ContentKind, LucideIcon>> = {
  json: Braces,
  'json-lines': Braces,
  'graph-ql': Braces,
  xml: Code2,
  html: Globe,
  javascript: Code2,
  css: Code2,
  markdown: FileText,
  yaml: FileText,
  csv: Table2,
  text: FileText,
  'form-url-encoded': ListTree,
  multipart: Layers,
  image: ImageIcon,
  audio: Music,
  video: Film,
  font: Type,
  pdf: FileText,
  protobuf: Binary,
  grpc: Binary,
  'grpc-web': Binary,
  'msg-pack': Binary,
  cbor: Binary,
  wasm: Binary,
  binary: Binary,
  'event-stream': Radio,
};

/** Lucide icon for a content kind, tinted with its kind colour. */
export function KindIcon({ kind, size = 13, className }: { kind: ContentKind; size?: number; className?: string }) {
  const Icon = KIND_ICONS[kind] ?? File;
  return <Icon size={size} strokeWidth={2} className={clsx(styles.kindIcon, className)} style={{ color: contentKindColorVar(kind) }} aria-hidden />;
}

export function Chip({ children, accent, className, title }: { children: ReactNode; accent?: boolean; className?: string; title?: string }) {
  return (
    <span className={clsx(styles.chip, accent && styles.chipAccent, className)} title={title}>
      {children}
    </span>
  );
}

// =====================================================================================
// Markdown
// =====================================================================================

function onLinkClick(e: MouseEvent<HTMLAnchorElement>, href: string) {
  if (href.startsWith('#')) return;
  e.preventDefault();
  openExternal(href);
}

function InlineNodes({ nodes }: { nodes: readonly Inline[] }) {
  return (
    <>
      {nodes.map((n, i) => {
        switch (n.t) {
          case 'text':
            return <Fragment key={i}>{n.v}</Fragment>;
          case 'code':
            return <code key={i}>{n.v}</code>;
          case 'strong':
            return (
              <strong key={i}>
                <InlineNodes nodes={n.c} />
              </strong>
            );
          case 'em':
            return (
              <em key={i}>
                <InlineNodes nodes={n.c} />
              </em>
            );
          case 'del':
            return (
              <del key={i}>
                <InlineNodes nodes={n.c} />
              </del>
            );
          case 'br':
            return <br key={i} />;
          case 'image':
            return (
              <span key={i} className={styles.mdImage} title={n.src ? `Image not loaded: ${n.src}` : 'Image not loaded'}>
                <ImageIcon size={12} aria-hidden />
                {n.alt || 'image'}
              </span>
            );
          case 'link':
            return n.href ? (
              <a key={i} href={n.href} onClick={(e) => onLinkClick(e, n.href as string)} rel="noopener noreferrer" title={n.href}>
                <InlineNodes nodes={n.c} />
              </a>
            ) : (
              <span key={i} className={styles.mdUnsafe} title="Link disabled: unsupported URL scheme">
                <InlineNodes nodes={n.c} />
              </span>
            );
          default:
            return null;
        }
      })}
    </>
  );
}

function Blocks({ blocks }: { blocks: readonly Block[] }) {
  return (
    <>
      {blocks.map((b, i) => {
        switch (b.t) {
          case 'heading': {
            const H = `h${b.level}` as 'h1';
            return (
              <H key={i}>
                <InlineNodes nodes={b.c} />
              </H>
            );
          }
          case 'paragraph':
            return (
              <p key={i}>
                <InlineNodes nodes={b.c} />
              </p>
            );
          case 'code':
            return (
              <pre key={i} data-lang={b.lang || undefined}>
                <code>{b.v}</code>
              </pre>
            );
          case 'quote':
            return (
              <blockquote key={i}>
                <Blocks blocks={b.c} />
              </blockquote>
            );
          case 'hr':
            return <hr key={i} />;
          case 'list': {
            const items = b.items.map((it, k) => (
              <li key={k} className={it.task != null ? styles.task : undefined}>
                {it.task != null && (
                  <span className={clsx(styles.taskBox, it.task && styles.checked)} aria-label={it.task ? 'Done' : 'Not done'} role="img">
                    {it.task && (
                      <svg width="9" height="9" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="4" aria-hidden>
                        <path d="M5 12l5 5L20 7" />
                      </svg>
                    )}
                  </span>
                )}
                <div>
                  <Blocks blocks={it.c} />
                </div>
              </li>
            ));
            return b.ordered ? (
              <ol key={i} start={b.start}>
                {items}
              </ol>
            ) : (
              <ul key={i}>{items}</ul>
            );
          }
          case 'table':
            return (
              <table key={i}>
                <thead>
                  <tr>
                    {b.head.map((h, k) => (
                      <th key={k} style={{ textAlign: b.align[k] ?? undefined }}>
                        <InlineNodes nodes={h} />
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {b.rows.map((r, k) => (
                    <tr key={k}>
                      {r.map((c, j) => (
                        <td key={j} style={{ textAlign: b.align[j] ?? undefined }}>
                          <InlineNodes nodes={c} />
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            );
          default:
            return null;
        }
      })}
    </>
  );
}

/** Renders one line of inline Markdown (`code`, **bold**, links) — e.g. finding summaries. */
export const MarkdownInline = memo(function MarkdownInline({ text, className }: { text: string; className?: string }) {
  const nodes = useMemo(() => parseInline(text), [text]);
  return (
    <span className={clsx(styles.mdInline, className)}>
      <InlineNodes nodes={nodes} />
    </span>
  );
});

/** Renders a parsed Markdown AST. All text goes through React (escaped); links are vetted. */
export const Markdown = memo(function Markdown({ blocks, compact, className }: { blocks: readonly Block[]; compact?: boolean; className?: string }) {
  return (
    <div className={clsx(styles.md, compact && styles.compact, className)}>
      <Blocks blocks={blocks} />
    </div>
  );
});
