import { FileDown } from 'lucide-react';
import { useEffect, useRef, useState, type RefObject } from 'react';
import { importCaptureFiles } from '../../../app/sessions/actions';
import type { PickedFile } from '../../../lib/platform';
import styles from './CaptureDrop.module.css';

/** Whether a drag carries files from outside the page (not a column or a rule being moved). */
export function draggingFiles(e: Pick<DragEvent, 'dataTransfer'>): boolean {
  const types = e.dataTransfer?.types;
  return types != null && Array.from(types).includes('Files');
}

async function readFiles(list: FileList | null | undefined): Promise<PickedFile[]> {
  const files = Array.from(list ?? []);
  return Promise.all(files.map(async (f) => ({ name: f.name, data: new Uint8Array(await f.arrayBuffer()) })));
}

/**
 * Drop capture files (HAR, `.akev`, Charles, Fiddler, mitmproxy) on `zone` to import them. The
 * browser and the desktop app both deliver HTML5 drops (the shell disables Tauri's own drag &
 * drop handler). While mounted, a file dropped elsewhere in the window doesn't navigate away.
 * Returns whether files are being dragged over the zone.
 */
export function useCaptureDrop(zone: RefObject<HTMLElement | null>, onImported?: (ids: number[]) => void): boolean {
  const [over, setOver] = useState(false);
  const onImportedRef = useRef(onImported);
  onImportedRef.current = onImported;

  useEffect(() => {
    const el = zone.current;
    if (!el) return;
    let depth = 0;
    const enter = (e: DragEvent) => {
      if (!draggingFiles(e)) return;
      e.preventDefault();
      depth++;
      setOver(true);
    };
    const dragOver = (e: DragEvent) => {
      if (!draggingFiles(e)) return;
      e.preventDefault();
      if (e.dataTransfer) e.dataTransfer.dropEffect = 'copy';
    };
    const leave = (e: DragEvent) => {
      if (!draggingFiles(e)) return;
      depth = Math.max(0, depth - 1);
      if (depth === 0) setOver(false);
    };
    const drop = (e: DragEvent) => {
      if (!draggingFiles(e)) return;
      e.preventDefault();
      depth = 0;
      setOver(false);
      void readFiles(e.dataTransfer?.files).then(async (files) => {
        if (!files.length) return;
        const ids = await importCaptureFiles(files);
        if (ids.length) onImportedRef.current?.(ids);
      });
    };
    // Outside the zone, a dropped file would replace the app with the file's contents.
    const guard = (e: DragEvent) => {
      if (draggingFiles(e) && !e.defaultPrevented) e.preventDefault();
    };
    el.addEventListener('dragenter', enter);
    el.addEventListener('dragover', dragOver);
    el.addEventListener('dragleave', leave);
    el.addEventListener('drop', drop);
    window.addEventListener('dragover', guard);
    window.addEventListener('drop', guard);
    return () => {
      el.removeEventListener('dragenter', enter);
      el.removeEventListener('dragover', dragOver);
      el.removeEventListener('dragleave', leave);
      el.removeEventListener('drop', drop);
      window.removeEventListener('dragover', guard);
      window.removeEventListener('drop', guard);
    };
  }, [zone]);

  return over;
}

/** Shown over the Traffic page while capture files are dragged onto it. */
export function CaptureDropOverlay({ visible }: { visible: boolean }) {
  if (!visible) return null;
  return (
    <div className={styles.overlay} role="status" aria-live="polite">
      <div className={styles.card}>
        <span className={styles.icon} aria-hidden>
          <FileDown size={22} strokeWidth={1.75} />
        </span>
        <span className={styles.title}>Drop to import</span>
        <span className={styles.hint}>HAR, .akev, Charles (.chlsj), Fiddler (.saz) or mitmproxy flows</span>
      </div>
    </div>
  );
}
