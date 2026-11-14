import { Dialog, Kbd } from '../components/ui';
import { useShortcutDocList, type ShortcutDoc } from './shortcuts';
import { setShortcutsOverlayOpen, useShortcutsOverlayOpen } from './shortcutsOverlay';
import styles from './ShortcutsOverlay.module.css';

const ORDER = ['General', 'Navigation', 'Traffic', 'Traffic list', 'Detail', 'Rules', 'Compose', 'Connect', 'Breakpoints', 'Appearance'];

/** `?` overlay listing every documented shortcut, grouped. */
export function ShortcutsOverlay() {
  const isOpen = useShortcutsOverlayOpen();
  const docs = useShortcutDocList();
  // One row per action: bindings with the same description are listed as alternatives.
  const groups = new Map<string, ShortcutDoc[]>();
  for (const d of docs) {
    const list = groups.get(d.group) ?? [];
    const same = list.find((x) => x.description === d.description);
    if (same) same.combo = `${same.combo} or ${d.combo}`;
    else list.push({ ...d });
    groups.set(d.group, list);
  }
  const sorted = [...groups.entries()].sort(([a], [b]) => {
    const ia = ORDER.indexOf(a);
    const ib = ORDER.indexOf(b);
    return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.localeCompare(b);
  });

  return (
    <Dialog open={isOpen} onOpenChange={setShortcutsOverlayOpen} title="Keyboard shortcuts" width={720} flush focusContent>
      <div className={styles.grid}>
        {sorted.map(([group, items]) => (
          <section key={group} className={styles.group}>
            <h3 className={styles.heading}>{group}</h3>
            <ul className={styles.list}>
              {items.map((d) => (
                <li key={`${d.combo}-${d.description}`} className={styles.row}>
                  <span className={styles.desc}>{d.description}</span>
                  <span className={styles.keys}>
                    {d.combo.split(/\s+or\s+/).map((combo, i) => (
                      <span key={combo} className={styles.alt}>
                        {i > 0 && <span className={styles.or}>or</span>}
                        <Kbd combo={combo} literal />
                      </span>
                    ))}
                  </span>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </Dialog>
  );
}

export default ShortcutsOverlay;
