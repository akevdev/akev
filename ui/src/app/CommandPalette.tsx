import { Command } from 'cmdk';
import { CornerDownLeft, Search } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { Kbd } from '../components/ui';
import { scoreCommand } from './commandScore';
import { closePalette, GROUP_ORDER, useCommandList, useCommandProviders, usePaletteState, type Command as Cmd } from './commands';
import styles from './CommandPalette.module.css';

function groupRank(group: string): number {
  const i = GROUP_ORDER.indexOf(group);
  return i < 0 ? GROUP_ORDER.length : i;
}

function CommandRow({ cmd, onRun, forceMount }: { cmd: Cmd; onRun: (cmd: Cmd) => void; forceMount?: boolean }) {
  const Icon = cmd.icon;
  return (
    <Command.Item
      value={`${cmd.group}:${cmd.id}`}
      // keywords[0] is the title; the rest is secondary text (see `paletteFilter`).
      keywords={[cmd.title, cmd.subtitle ?? '', cmd.group, ...(cmd.keywords ?? [])]}
      onSelect={() => onRun(cmd)}
      disabled={cmd.disabled}
      forceMount={forceMount}
      className={styles.item}
    >
      <span className={styles.icon}>{cmd.iconNode ?? (Icon ? <Icon size={15} strokeWidth={1.75} aria-hidden /> : null)}</span>
      <span className={styles.title}>{cmd.title}</span>
      {cmd.subtitle && <span className={styles.subtitle}>{cmd.subtitle}</span>}
      <span className={styles.spacer} />
      {cmd.shortcut && <Kbd combo={cmd.shortcut} size="sm" />}
    </Command.Item>
  );
}

/** cmdk filter: rank by {@link scoreCommand} (title first, then secondary text, fuzzy last). */
function paletteFilter(_value: string, search: string, keywords?: string[]): number {
  const [title = '', ...secondary] = keywords ?? [];
  return scoreCommand(search, title, secondary);
}

/** ⌘K command palette (cmdk) over the command registry and dynamic providers. */
export function CommandPalette() {
  const { open, initialSearch } = usePaletteState();
  const commands = useCommandList();
  const providers = useCommandProviders();
  const [search, setSearch] = useState('');

  useEffect(() => {
    if (open) setSearch(initialSearch);
  }, [open, initialSearch]);

  const groups = useMemo(() => {
    const visible = commands.filter((c) => !c.when || c.when());
    const byGroup = new Map<string, Cmd[]>();
    for (const c of visible) {
      const list = byGroup.get(c.group) ?? [];
      list.push(c);
      byGroup.set(c.group, list);
    }
    return [...byGroup.entries()].sort(([a], [b]) => groupRank(a) - groupRank(b) || a.localeCompare(b));
    // `when` predicates are re-evaluated each time the palette opens.
  }, [commands, open]);

  const dynamic = useMemo(() => (open && search.trim() ? providers.flatMap((p) => p.provide(search.trim())) : []), [providers, search, open]);

  const run = (cmd: Cmd) => {
    if (!cmd.keepOpen) closePalette();
    // Let the dialog close (and restore focus) before running, so commands can move focus.
    setTimeout(() => void cmd.run(), 0);
  };

  return (
    <Command.Dialog
      open={open}
      onOpenChange={(o) => !o && closePalette()}
      label="Command palette"
      overlayClassName={styles.overlay}
      contentClassName={styles.content}
      className={styles.root}
      filter={paletteFilter}
      loop
    >
      <div className={styles.inputRow}>
        <Search size={16} strokeWidth={1.75} className={styles.searchIcon} aria-hidden />
        <Command.Input value={search} onValueChange={setSearch} placeholder="Type a command, page, or #id to jump to an exchange…" className={styles.input} />
        <Kbd size="sm">esc</Kbd>
      </div>
      <Command.List className={styles.list}>
        <Command.Empty className={styles.empty}>No matching commands</Command.Empty>
        {dynamic.length > 0 && (
          <Command.Group heading={dynamic[0]?.group ?? 'Results'} className={styles.group} forceMount>
            {dynamic.map((cmd) => (
              <CommandRow key={cmd.id} cmd={cmd} onRun={run} forceMount />
            ))}
          </Command.Group>
        )}
        {groups.map(([group, cmds]) => (
          <Command.Group key={group} heading={group} className={styles.group}>
            {cmds.map((cmd) => (
              <CommandRow key={cmd.id} cmd={cmd} onRun={run} />
            ))}
          </Command.Group>
        ))}
      </Command.List>
      <div className={styles.footer}>
        <span className={styles.hint}>
          <Kbd size="sm">↑</Kbd>
          <Kbd size="sm">↓</Kbd> navigate
        </span>
        <span className={styles.hint}>
          <Kbd size="sm">
            <CornerDownLeft size={10} strokeWidth={2} aria-hidden />
          </Kbd>{' '}
          run
        </span>
        <span className={styles.spacer} />
        <span className={styles.hint}>
          <Kbd combo="mod+k" size="sm" /> toggle
        </span>
      </div>
    </Command.Dialog>
  );
}

export default CommandPalette;
