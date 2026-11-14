// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus
import clsx from 'clsx';
import { Keyboard, RotateCcw, TriangleAlert } from 'lucide-react';
import { useDeferredValue, useEffect, useMemo, useState } from 'react';
import { comboFromEvent, shortcutLabel, useShortcutDocList } from '../../../app/shortcuts';
import { Button, EmptyState, IconButton, Kbd, SearchInput, Tooltip, undoToast } from '../../../components/ui';
import { pluralize } from '../../../lib/format';
import { isDesktop, isMac } from '../../../lib/platform';
import { useUiStore } from '../../../state/ui';
import { SettingsCard } from '../SettingRow';
import { findConflicts, groupShortcuts, referenceDocs, type CatalogBinding, type CatalogEntry } from '../shortcutCatalog';
import styles from './sections.module.css';

function keyLabel(combo: string): string {
  try {
    return shortcutLabel(combo);
  } catch {
    return combo;
  }
}

/** Rebind (or, with the default keys, reset) a shortcut. */
function assign(binding: string, combo: string, defaultCombo: string): void {
  useUiStore.getState().setShortcut(binding, combo === defaultCombo ? null : combo);
}

interface Pending {
  binding: string;
  combo: string;
  conflicts: CatalogEntry[];
}

/** One key chip: click to record new keys for a rebindable shortcut. */
function BindingChip({
  b,
  description,
  recording,
  onRecord,
  onReset,
}: {
  b: CatalogBinding;
  description: string;
  recording: boolean;
  onRecord: () => void;
  onReset: () => void;
}) {
  if (!b.binding) {
    return (
      <Tooltip content="Built in: this key can't be changed">
        <span className={styles.fixedKeys} tabIndex={-1}>
          {b.combo.split(/\s+or\s+/).map((combo, i) => (
            <span key={combo} className={styles.inlineGroup}>
              {i > 0 && <span className={styles.or}>or</span>}
              <Kbd combo={combo} literal />
            </span>
          ))}
        </span>
      </Tooltip>
    );
  }
  const changed = b.defaultCombo != null && b.combo !== b.defaultCombo;
  return (
    <span className={styles.inlineGroup}>
      <button
        type="button"
        className={clsx(styles.bindingChip, changed && styles.bindingChanged, recording && styles.bindingRecording)}
        onClick={onRecord}
        aria-label={recording ? `Recording new keys for ${description}. Press keys, or Escape to cancel` : `${description}: ${keyLabel(b.combo)}. Change`}
        aria-pressed={recording}
      >
        {recording ? <span className={styles.recordingText}>Press keys…</span> : <Kbd combo={b.combo} literal />}
      </button>
      {changed && !recording && b.defaultCombo && (
        <IconButton size="sm" icon={RotateCcw} label={`Reset to ${keyLabel(b.defaultCombo)}`} onClick={onReset} />
      )}
    </span>
  );
}

/** Settings → Shortcuts: every shortcut, grouped and searchable; click keys to rebind them. */
export function ShortcutsSection() {
  const live = useShortcutDocList();
  const overrides = useUiStore((s) => s.shortcuts);
  const [query, setQuery] = useState('');
  const deferred = useDeferredValue(query);
  const reference = useMemo(() => referenceDocs(overrides, isDesktop()), [overrides]);
  const all = useMemo(() => groupShortcuts(live, reference), [live, reference]);
  const groups = useMemo(() => (deferred.trim() ? groupShortcuts(live, reference, deferred, keyLabel) : all), [live, reference, deferred, all]);
  const total = groups.reduce((n, g) => n + g.items.length, 0);
  const changedCount = Object.keys(overrides).length;

  const [recording, setRecording] = useState<{ binding: string; group: string; defaultCombo: string } | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);

  // While recording, every key press is captured here first (before the app's shortcuts).
  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      const plain = !e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey;
      if (e.key === 'Tab' && plain) {
        setRecording(null);
        return;
      }
      e.preventDefault();
      e.stopPropagation();
      if (e.key === 'Escape' && plain) {
        setRecording(null);
        return;
      }
      const combo = comboFromEvent(e);
      if (!combo) return;
      setRecording(null);
      const conflicts = findConflicts(combo, recording.binding, recording.group, all.flatMap((g) => g.items), isMac);
      if (conflicts.length) setPending({ binding: recording.binding, combo, conflicts });
      else assign(recording.binding, combo, recording.defaultCombo);
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [recording, all]);

  const resetAll = () => {
    const previous = { ...useUiStore.getState().shortcuts };
    useUiStore.getState().resetShortcuts();
    undoToast(`${pluralize(Object.keys(previous).length, 'shortcut')} reset`, () => {
      for (const [binding, combo] of Object.entries(previous)) useUiStore.getState().setShortcut(binding, combo);
    });
  };

  const defaultOf = (binding: string): string => {
    for (const g of all) for (const e of g.items) for (const b of e.bindings) if (b.binding === binding && b.defaultCombo) return b.defaultCombo;
    return '';
  };

  return (
    <>
      <div className={styles.shortcutToolbar} id="setting-shortcuts" data-setting="shortcuts">
        <SearchInput value={query} onValueChange={setQuery} placeholder="Search actions or keys…" className={styles.shortcutSearch} aria-label="Search shortcuts" />
        <span className={`${styles.shortcutCount} tnum`}>{pluralize(total, 'shortcut')}</span>
        <span className={styles.shortcutSpacer} />
        <Button size="sm" variant="ghost" icon={RotateCcw} disabled={changedCount === 0} onClick={resetAll}>
          Reset all
        </Button>
      </div>
      <p className={styles.shortcutHint}>Click a key to change it, then press the new keys (Escape cancels). Keys the lists handle themselves can't be changed.</p>
      {pending && (
        <div className={styles.shortcutConflict} role="alert">
          <TriangleAlert size={14} aria-hidden />
          <span className={styles.shortcutConflictText}>
            <Kbd combo={pending.combo} literal size="sm" /> is already used by {pending.conflicts.map((c) => `${c.description} (${c.group})`).join(', ')}. Use it anyway? Where both apply, the most recently opened view wins.
          </span>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => {
              assign(pending.binding, pending.combo, defaultOf(pending.binding));
              setPending(null);
            }}
          >
            Use anyway
          </Button>
          <Button size="sm" variant="ghost" onClick={() => setPending(null)}>
            Cancel
          </Button>
        </div>
      )}
      {groups.length === 0 ? (
        <EmptyState icon={Keyboard} title={`No shortcut matches “${deferred.trim()}”`} description="Try an action like “clear” or a key like “mod+k”." className={styles.empty} />
      ) : (
        groups.map((g) => (
          <SettingsCard key={g.group} title={g.group}>
            <div className={styles.shortcutGroup} role="table" aria-label={`${g.group} shortcuts`}>
              {g.items.map((e) => (
                <div key={e.description} className={styles.shortcutRow} role="row">
                  <span className={styles.shortcutDesc} role="cell">
                    {e.description}
                  </span>
                  <span className={styles.shortcutKeys} role="cell">
                    {e.bindings.map((b, i) => (
                      <span key={b.binding ?? b.combo} className={styles.inlineGroup}>
                        {i > 0 && <span className={styles.or}>or</span>}
                        <BindingChip
                          b={b}
                          description={e.description}
                          recording={recording?.binding === b.binding && b.binding != null}
                          onRecord={() => {
                            setPending(null);
                            if (b.binding && b.defaultCombo) setRecording(recording?.binding === b.binding ? null : { binding: b.binding, group: e.group, defaultCombo: b.defaultCombo });
                          }}
                          onReset={() => b.binding && useUiStore.getState().setShortcut(b.binding, null)}
                        />
                      </span>
                    ))}
                  </span>
                </div>
              ))}
            </div>
          </SettingsCard>
        ))
      )}
    </>
  );
}
