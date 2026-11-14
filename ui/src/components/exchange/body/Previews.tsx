// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus (CSV preview)
/**
 * Preview renderers for BodyViewer: images/SVG (checkerboard, dimensions, fit/100%), sandboxed
 * HTML, safe Markdown, CSV tables, audio/video players, PDF and font specimens.
 */

import { useVirtualizer } from '@tanstack/react-virtual';
import clsx from 'clsx';
import { ExternalLink, FileText, ImageOff, Lock, Music, Type } from 'lucide-react';
import { useEffect, useId, useMemo, useRef, useState } from 'react';
import type { ImageInfo } from '../../../api/types';
import { formatBytes, formatDuration, formatNumber } from '../../../lib/format';
import { openExternal } from '../../../lib/platform';
import { EmptyState, IconButton, SegmentedControl, Spinner } from '../../ui';
import { hardenHtmlPreview, parseCsv, PREVIEW_CSP } from '../model/body';
import { parseMarkdown } from '../model/markdown';
import { Banner, Markdown } from '../parts/Parts';
import styles from './body.module.css';

// =====================================================================================
// Image
// =====================================================================================

export interface ImagePreviewProps {
  url: string;
  info: ImageInfo | null;
  size: number;
  contentType?: string | null;
  onShowHex?: () => void;
}

export function ImagePreview({ url, info, size, contentType, onShowHex }: ImagePreviewProps) {
  const [zoom, setZoom] = useState<'fit' | 'actual'>('fit');
  const [natural, setNatural] = useState<{ w: number; h: number } | null>(null);
  const [failed, setFailed] = useState(false);
  const [fits, setFits] = useState(true);
  const stageRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    setFailed(false);
    setNatural(null);
    setZoom('fit');
  }, [url]);

  // Whether the image already fits at 100% (then zooming does nothing).
  useEffect(() => {
    const el = stageRef.current;
    if (!el || !natural) return;
    const measure = () => setFits(natural.w <= el.clientWidth - 48 && natural.h <= el.clientHeight - 80);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [natural]);

  if (failed) {
    return (
      <div className={styles.center}>
        <EmptyState
          icon={ImageOff}
          size="sm"
          tone="warning"
          title="Couldn't render this image"
          description="The bytes may be truncated, corrupted, or in a format this browser can't display."
          actions={onShowHex ? <IconButton label="Show raw bytes" icon={FileText} variant="subtle" onClick={onShowHex} /> : undefined}
        />
      </div>
    );
  }

  const w = natural?.w ?? info?.width ?? null;
  const h = natural?.h ?? info?.height ?? null;
  const format = (info?.format ?? contentType?.split('/')[1]?.replace('svg+xml', 'svg') ?? '').toUpperCase();
  const small = natural != null && natural.w <= 64 && natural.h <= 64;
  return (
    <div className={clsx(styles.fill, styles.checker)}>
      <div
        ref={stageRef}
        className={clsx(styles.imageStage, zoom === 'actual' && styles.actual, small && zoom === 'actual' && styles.pixelated, fits && styles.noZoom)}
      >
        {/* biome-ignore lint/a11y/useKeyWithClickEvents: click-to-zoom shortcut; the Fit / 100% control below does the same from the keyboard */}
        <img
          src={url}
          alt={`${format || 'Image'} preview`}
          className={styles.image}
          draggable={false}
          onLoad={(e) => setNatural({ w: e.currentTarget.naturalWidth, h: e.currentTarget.naturalHeight })}
          onError={() => setFailed(true)}
          onClick={() => !fits && setZoom((z) => (z === 'fit' ? 'actual' : 'fit'))}
        />
      </div>
      <div className={styles.floatBar}>
        {w != null && h != null && (
          <strong className="tnum">
            {formatNumber(w)} × {formatNumber(h)}
          </strong>
        )}
        {format && <span>{format}</span>}
        <span className="tnum">{formatBytes(size)}</span>
        <span className={styles.floatSep} aria-hidden />
        <SegmentedControl
          size="sm"
          aria-label="Zoom"
          value={zoom}
          onValueChange={setZoom}
          items={[
            { value: 'fit', label: 'Fit', 'aria-label': 'Fit to the pane' },
            { value: 'actual', label: '100%', 'aria-label': 'Actual size', disabled: fits && zoom === 'fit' },
          ]}
        />
      </div>
    </div>
  );
}

// =====================================================================================
// HTML (sandboxed)
// =====================================================================================

/**
 * Attributes of the preview iframe. `sandbox=""`: no scripts, forms, popups, same-origin access
 * or top navigation. `csp` (CSP Embedded Enforcement, Chromium) imposes {@link PREVIEW_CSP} on
 * the frame before parsing starts; engines without it rely on the CSP `<meta>` that
 * {@link hardenHtmlPreview} puts first in the document.
 */
export function htmlPreviewFrameProps(html: string) {
  return {
    sandbox: '',
    srcDoc: hardenHtmlPreview(html),
    csp: PREVIEW_CSP,
    referrerPolicy: 'no-referrer' as const,
    title: 'HTML preview',
  };
}

export function HtmlPreview({ html }: { html: string }) {
  const frame = useMemo(() => htmlPreviewFrameProps(html), [html]);
  return (
    <div className={styles.frameWrap}>
      <iframe className={styles.frame} {...frame} />
      <span className={styles.frameBadge} title="Scripts, forms and network requests are blocked in previews">
        <Lock size={11} strokeWidth={2.25} aria-hidden />
        Sandboxed · no scripts or network
      </span>
    </div>
  );
}

// =====================================================================================
// Markdown
// =====================================================================================

const MARKDOWN_LIMIT = 1024 * 1024;

export function MarkdownPreview({ text }: { text: string }) {
  const capped = text.length > MARKDOWN_LIMIT;
  const blocks = useMemo(() => parseMarkdown(capped ? text.slice(0, MARKDOWN_LIMIT) : text), [text, capped]);
  return (
    <div className={styles.fill}>
      {capped && <Banner tone="info">Previewing the first {formatBytes(MARKDOWN_LIMIT)} — switch to Raw for the whole document.</Banner>}
      <div className={styles.scroll}>
        <article className={styles.article}>
          <Markdown blocks={blocks} />
        </article>
      </div>
    </div>
  );
}

// =====================================================================================
// CSV
// =====================================================================================

const CSV_ROW_H = 26;

export function CsvPreview({ text }: { text: string }) {
  const { rows, truncated } = useMemo(() => parseCsv(text, 5000), [text]);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const header = rows[0] ?? [];
  const body = rows.slice(1);
  const cols = Math.max(1, ...rows.slice(0, 200).map((r) => r.length));
  const template = useMemo(() => {
    // Widths in characters (+1 for breathing room); cells add 20px of padding on top.
    const widths: number[] = Array.from({ length: cols }, () => 3);
    for (const r of rows.slice(0, 300)) r.forEach((c, i) => (widths[i] = Math.max(widths[i] ?? 3, Math.min(48, c.length + 1))));
    const numW = Math.max(2, String(body.length).length);
    return `calc(${numW}ch + 20px) ${widths.map((w) => `minmax(calc(${Math.min(w, 12)}ch + 20px), calc(${w}ch + 20px))`).join(' ')}`;
  }, [rows, cols, body.length]);
  const virtualizer = useVirtualizer({ count: body.length, getScrollElement: () => scrollRef.current, estimateSize: () => CSV_ROW_H, overscan: 20 });
  const items = virtualizer.getVirtualItems();
  return (
    <div className={styles.fill}>
      {truncated && <Banner tone="info">Showing the first {formatNumber(rows.length)} rows — switch to Raw for the full file.</Banner>}
      <div ref={scrollRef} className={styles.csv} role="table" aria-label="CSV table" aria-rowcount={rows.length}>
        <div className={styles.csvSizer} style={{ height: virtualizer.getTotalSize() + CSV_ROW_H }}>
          <div className={clsx(styles.csvRow, styles.csvHead)} style={{ gridTemplateColumns: template }} role="row">
            <span className={styles.csvNum} role="columnheader">
              #
            </span>
            {Array.from({ length: cols }, (_, i) => (
              <span key={i} role="columnheader" title={header[i]}>
                {header[i] ?? ''}
              </span>
            ))}
          </div>
          {items.map((it) => {
            const r = body[it.index] ?? [];
            return (
              <div key={it.key} className={styles.csvRow} role="row" style={{ gridTemplateColumns: template, position: 'absolute', left: 0, right: 0, top: CSV_ROW_H + it.start }}>
                <span className={clsx(styles.csvNum, 'tnum')}>{it.index + 1}</span>
                {Array.from({ length: cols }, (_, i) => (
                  <span key={i} role="cell" title={(r[i] ?? '').length > 24 ? r[i] : undefined}>
                    {r[i] ?? ''}
                  </span>
                ))}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}

// =====================================================================================
// Audio / video / PDF
// =====================================================================================

export function AudioPreview({ url, title, size, contentType }: { url: string; title: string; size: number; contentType?: string | null }) {
  const [duration, setDuration] = useState<number | null>(null);
  const [failed, setFailed] = useState(false);
  return (
    <div className={styles.center}>
      <div className={styles.mediaCard}>
        <span className={styles.mediaIcon} aria-hidden>
          <Music size={22} strokeWidth={1.75} />
        </span>
        <div className={styles.mediaTitle}>{title}</div>
        <div className={clsx(styles.mediaMeta, 'tnum')}>
          {[contentType, formatBytes(size), duration != null && Number.isFinite(duration) ? formatDuration(duration * 1000) : null].filter(Boolean).join(' · ')}
        </div>
        {failed ? (
          <span className={styles.mediaMeta}>This browser can't play this audio format.</span>
        ) : (
          // biome-ignore lint/a11y/useMediaCaption: plays captured traffic, which has no caption track
          <audio className={styles.audio} controls preload="metadata" src={url} onLoadedMetadata={(e) => setDuration(e.currentTarget.duration)} onError={() => setFailed(true)} />
        )}
      </div>
    </div>
  );
}

export function VideoPreview({ url, size, contentType }: { url: string; size: number; contentType?: string | null }) {
  const [meta, setMeta] = useState<{ w: number; h: number; d: number } | null>(null);
  const [failed, setFailed] = useState(false);
  if (failed) {
    return (
      <div className={styles.center}>
        <EmptyState icon={ImageOff} size="sm" tone="warning" title="Can't play this video" description="The format isn't supported by this browser, or the captured bytes are incomplete." />
      </div>
    );
  }
  return (
    <div className={styles.fill}>
      <div className={styles.videoStage}>
        {/* biome-ignore lint/a11y/useMediaCaption: plays captured traffic, which has no caption track */}
        <video
          className={styles.video}
          controls
          preload="metadata"
          src={url}
          onLoadedMetadata={(e) => setMeta({ w: e.currentTarget.videoWidth, h: e.currentTarget.videoHeight, d: e.currentTarget.duration })}
          onError={() => setFailed(true)}
        />
      </div>
      <div className={styles.floatBar} style={{ paddingRight: 'var(--sp-3)' }}>
        {meta && meta.w > 0 && (
          <strong className="tnum">
            {meta.w} × {meta.h}
          </strong>
        )}
        {meta && Number.isFinite(meta.d) && <span className="tnum">{formatDuration(meta.d * 1000)}</span>}
        <span>{contentType}</span>
        <span className="tnum">{formatBytes(size)}</span>
      </div>
    </div>
  );
}

export function PdfPreview({ url }: { url: string }) {
  return (
    <div className={styles.frameWrap}>
      <iframe className={styles.frame} src={url} title="PDF preview" referrerPolicy="no-referrer" />
      <span className={styles.frameBadge}>
        PDF
        <IconButton label="Open in browser" icon={ExternalLink} size="sm" onClick={() => openExternal(url)} style={{ width: 18, height: 18, margin: '0 -4px 0 0' }} />
      </span>
    </div>
  );
}

// =====================================================================================
// Font specimen
// =====================================================================================

const PANGRAM = 'The quick brown fox jumps over the lazy dog';

export function FontPreview({ url, name }: { url: string; name: string }) {
  const uid = useId().replace(/[^a-zA-Z0-9]/g, '');
  const family = `akev-preview-${uid}`;
  const [state, setState] = useState<'loading' | 'ready' | 'error'>('loading');
  useEffect(() => {
    let cancelled = false;
    setState('loading');
    let face: FontFace | null = null;
    try {
      face = new FontFace(family, `url("${url.replace(/"/g, '%22')}")`);
    } catch {
      setState('error');
      return;
    }
    face.load().then(
      (f) => {
        if (cancelled) return;
        document.fonts.add(f);
        setState('ready');
      },
      () => !cancelled && setState('error'),
    );
    return () => {
      cancelled = true;
      if (face && document.fonts.has(face)) document.fonts.delete(face);
    };
  }, [url, family]);

  if (state === 'loading') {
    return (
      <div className={styles.loadingInline}>
        <Spinner size={16} /> Loading font…
      </div>
    );
  }
  if (state === 'error') {
    return (
      <div className={styles.center}>
        <EmptyState icon={Type} size="sm" tone="warning" title="Couldn't load this font" description="The file may be truncated or in a format this browser doesn't support. Switch to Hex to inspect the bytes." />
      </div>
    );
  }
  return (
    <div className={styles.scroll}>
      <div className={styles.specimen} style={{ fontFamily: `"${family}", var(--font-ui)` }}>
        <div className={styles.specimenHero}>Aa Gg Rr 0123</div>
        <div className={styles.specimenLine} style={{ fontSize: 20 }}>
          ABCDEFGHIJKLMNOPQRSTUVWXYZ
          <br />
          abcdefghijklmnopqrstuvwxyz
          <br />
          0123456789 !@#$%^&amp;*()[]{'{}'}?/
        </div>
        {[12, 16, 24, 36].map((px) => (
          <div key={px} className={styles.specimenSize}>
            <span>{px}px</span>
            <span className={styles.specimenLine} style={{ fontSize: px }}>
              {PANGRAM}
            </span>
          </div>
        ))}
        <div className={styles.mediaMeta} style={{ fontFamily: 'var(--font-ui)' }}>
          {name}
        </div>
      </div>
    </div>
  );
}
