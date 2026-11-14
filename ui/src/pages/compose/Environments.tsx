import clsx from 'clsx';
import { Check, ChevronDown, Copy, Globe, Layers, Plus, Settings2, Trash2 } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { Button, Dialog, IconButton, Input, KeyValueEditor, Menu, Tooltip, undoToast, makeRow, type KeyValueRow } from '../../components/ui';
import { uniqueName } from './library';
import { uid, type EnvState, type Environment } from './model';
import { envActions, useComposeStore } from './store';
import { DYNAMIC_VARS } from './variables';
import styles from './Environments.module.css';

/** Toolbar selector: active environment + "Manage environments…". */
export function EnvMenu({ onManage }: { onManage: () => void }) {
  const env = useComposeStore((s) => s.env);
  const active = env.envs.find((e) => e.id === env.activeId);
  const count = active ? active.vars.filter((v) => v.enabled && v.name.trim()).length : env.globals.filter((v) => v.enabled && v.name.trim()).length;
  return (
    <Menu
      align="end"
      trigger={
        <button type="button" className={clsx(styles.trigger, active && styles.triggerActive)} aria-label={`Environment: ${active?.name ?? 'none'}`}>
          <Layers size={14} strokeWidth={1.75} aria-hidden />
          <span className={styles.triggerName}>{active?.name ?? 'No environment'}</span>
          {count > 0 && <span className={clsx(styles.triggerCount, 'tnum')}>{count}</span>}
          <ChevronDown size={12} strokeWidth={2} aria-hidden className={styles.triggerChevron} />
        </button>
      }
      items={[
        { type: 'label', label: 'Environment' },
        {
          type: 'radio',
          value: env.activeId ?? '__none__',
          onValueChange: (v) => envActions.setActive(v === '__none__' ? null : v),
          options: [{ value: '__none__', label: 'No environment' }, ...env.envs.map((e) => ({ value: e.id, label: e.name }))],
        },
        { type: 'separator' },
        // Open after the menu has closed and restored focus, so the two overlays don't fight.
        { label: 'Manage environments…', icon: Settings2, onSelect: () => setTimeout(onManage, 0) },
      ]}
    />
  );
}

const GLOBALS = '__globals__';

export interface EnvDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Environment to select when opening. */
  initialId?: string | null;
}

/** Manage environments: globals + named environments with `{{name}}` variables. Edits apply live. */
export function EnvDialog({ open, onOpenChange, initialId }: EnvDialogProps) {
  const env = useComposeStore((s) => s.env);
  const [selected, setSelected] = useState<string>(GLOBALS);

  useEffect(() => {
    if (open) setSelected(initialId && env.envs.some((e) => e.id === initialId) ? initialId : (env.activeId ?? GLOBALS));
    // Only when (re)opening.
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, [open]);

  const current = env.envs.find((e) => e.id === selected);
  const rows = current ? current.vars : env.globals;
  const setEnv = (next: EnvState) => envActions.set(next);
  const setRows = (vars: KeyValueRow[]) => (current ? setEnv({ ...env, envs: env.envs.map((e) => (e.id === current.id ? { ...e, vars } : e)) }) : setEnv({ ...env, globals: vars }));

  const add = (from?: Environment) => {
    const id = uid('e');
    const name = uniqueName(from ? `${from.name} copy` : 'New environment', env.envs.map((e) => e.name));
    const vars = from ? from.vars.map((v) => makeRow(v.name, v.value, v.enabled)) : [makeRow('baseUrl', 'http://localhost:3000')];
    setEnv({ ...env, envs: [...env.envs, { id, name, vars }], activeId: env.activeId ?? id });
    setSelected(id);
  };
  const remove = (e: Environment) => {
    const prev = env;
    setEnv({ ...env, envs: env.envs.filter((x) => x.id !== e.id), activeId: env.activeId === e.id ? null : env.activeId });
    setSelected(GLOBALS);
    undoToast(`Deleted environment “${e.name}”`, () => envActions.set(prev));
  };
  const rename = (name: string) => current && setEnv({ ...env, envs: env.envs.map((e) => (e.id === current.id ? { ...e, name } : e)) });

  const varCount = (vars: readonly KeyValueRow[]) => vars.filter((v) => v.enabled && v.name.trim()).length;
  const overridden = useMemo(() => {
    if (!current) return new Set<string>();
    const names = new Set(current.vars.filter((v) => v.enabled).map((v) => v.name.trim()));
    return new Set(env.globals.filter((g) => g.enabled && names.has(g.name.trim())).map((g) => g.name.trim()));
  }, [current, env.globals]);

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Environments"
      description="Use variables anywhere in a request as {{name}}. They're substituted when the request is sent."
      width={780}
      flush
      onOpenAutoFocus={(e) => {
        // Start in the variables table rather than on the close button.
        const target = e.target instanceof HTMLElement ? e.target.querySelector<HTMLInputElement>('[data-env-editor] input') : null;
        if (target) {
          e.preventDefault();
          target.focus({ preventScroll: true });
        }
      }}
      footer={
        <>
          <span className={styles.footHint}>
            Built-in: {DYNAMIC_VARS.filter((d) => d.name !== '$guid').map((d) => (
              <Tooltip key={d.name} content={d.description}>
                {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
                <code className={styles.dyn} tabIndex={0}>{`{{${d.name}}}`}</code>
              </Tooltip>
            ))}
          </span>
          <Button variant="primary" onClick={() => onOpenChange(false)}>
            Done
          </Button>
        </>
      }
    >
      <div className={styles.layout}>
        <nav className={styles.list} aria-label="Environments">
          <button type="button" className={clsx(styles.item, selected === GLOBALS && styles.itemSelected)} onClick={() => setSelected(GLOBALS)}>
            <Globe size={14} strokeWidth={1.75} aria-hidden className={styles.itemIcon} />
            <span className={styles.itemName}>Globals</span>
            <span className={clsx(styles.itemCount, 'tnum')}>{varCount(env.globals) || ''}</span>
          </button>
          <div className={styles.listLabel}>Environments</div>
          {env.envs.map((e) => (
            <button key={e.id} type="button" className={clsx(styles.item, selected === e.id && styles.itemSelected)} onClick={() => setSelected(e.id)}>
              <Layers size={14} strokeWidth={1.75} aria-hidden className={styles.itemIcon} />
              <span className={styles.itemName}>{e.name || 'Untitled'}</span>
              {env.activeId === e.id ? (
                <Tooltip content="Active">
                  <Check size={13} strokeWidth={2.25} className={styles.itemActive} aria-label="Active" />
                </Tooltip>
              ) : (
                <span className={clsx(styles.itemCount, 'tnum')}>{varCount(e.vars) || ''}</span>
              )}
            </button>
          ))}
          <button type="button" className={clsx(styles.item, styles.addItem)} onClick={() => add()}>
            <Plus size={14} strokeWidth={1.75} aria-hidden className={styles.itemIcon} />
            <span className={styles.itemName}>New environment</span>
          </button>
        </nav>
        <div className={styles.editor}>
          <div className={styles.editorHead}>
            {current ? (
              <Input value={current.name} onChange={(e) => rename(e.target.value)} onBlur={(e) => !e.target.value.trim() && rename('Untitled')} aria-label="Environment name" className={styles.nameInput} />
            ) : (
              <div className={styles.globalsTitle}>
                <span className={styles.globalsName}>Globals</span>
                <span className={styles.globalsHint}>Always available; an active environment overrides them.</span>
              </div>
            )}
            {current && (
              <div className={styles.editorActions}>
                {env.activeId === current.id ? (
                  <Button size="sm" variant="ghost" onClick={() => envActions.setActive(null)}>
                    Deactivate
                  </Button>
                ) : (
                  <Button size="sm" variant="secondary" icon={Check} onClick={() => envActions.setActive(current.id)}>
                    Use this environment
                  </Button>
                )}
                <IconButton size="sm" label="Duplicate environment" icon={Copy} onClick={() => add(current)} />
                <IconButton size="sm" label="Delete environment" icon={Trash2} variant="danger" onClick={() => remove(current)} />
              </div>
            )}
          </div>
          <div className={styles.vars} data-env-editor>
            <KeyValueEditor key={selected} rows={rows} onChange={setRows} separator="=" namePlaceholder="Variable" valuePlaceholder="Value" aria-label="Variables" />
            {overridden.size > 0 && <p className={styles.overrideNote}>Overrides globals: {[...overridden].join(', ')}</p>}
            {env.envs.length === 0 && !current && (
              <div className={styles.callout}>
                <span>Create environments to switch between e.g. local, staging and production with one click.</span>
                <Button size="sm" icon={Plus} onClick={() => add()}>
                  New environment
                </Button>
              </div>
            )}
          </div>
        </div>
      </div>
    </Dialog>
  );
}
