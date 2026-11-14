import { useCallback, useState, type RefObject } from 'react';
import { api, errorMessage } from '../../../api/client';
import type { RuleId, RuleSet } from '../../../api/types';
import { toast, undoToast } from '../../../components/ui';
import { utf8Decode } from '../../../lib/encoding';
import { pluralize } from '../../../lib/format';
import { downloadUrl, pickFiles, saveData } from '../../../lib/platform';
import { insertItem, newId, reidentifyAll, setAllEnabled, walkRules, type GroupNode } from '../../../lib/rules/tree';
import type { RuleEditorHandle } from '../editor/RuleEditor';
import type { ImportMode, PendingImport } from '../ImportDialog';
import type { RulesModel } from '../useRulesModel';
import type { Mutate } from './useRuleActions';
import type { RevealFocus } from './useRuleNavigation';

interface Options {
  model: RulesModel;
  mutate: Mutate;
  reveal(id: RuleId, focus?: RevealFocus): boolean;
  editorRef: RefObject<RuleEditorHandle | null>;
}

/** Export the rule set as JSON, and import Akev JSON or HTTP Toolkit `.htkrules` files. */
export function useRuleTransfer({ model, mutate, reveal, editorRef }: Options) {
  const { getRules, saveState } = model;
  const [pendingImport, setPendingImport] = useState<PendingImport | null>(null);

  const exportRules = useCallback(async () => {
    editorRef.current?.flush();
    try {
      if (saveState === 'idle') await downloadUrl(api.rules.exportUrl(), 'akev-rules.json');
      else await saveData(`${JSON.stringify(getRules(), null, 2)}\n`, 'akev-rules.json', 'application/json');
    } catch (e) {
      toast.error("Couldn't export rules", { description: errorMessage(e) });
    }
  }, [editorRef, getRules, saveState]);

  const startImport = useCallback(async () => {
    const files = await pickFiles({ accept: ['.json', '.htkrules', 'application/json'] });
    const file = files?.[0];
    if (!file) return;
    const text = utf8Decode(file.data);
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch (e) {
      toast.error(`“${file.name}” isn't valid JSON`, { description: e instanceof Error ? e.message : String(e) });
      return;
    }
    const obj = parsed as { version?: unknown; items?: { type?: unknown }[] } | null;
    const isAkev = !!obj && typeof obj === 'object' && typeof obj.version === 'number' && Array.isArray(obj.items) && obj.items.every((i) => i && (i.type === 'rule' || i.type === 'group'));
    try {
      const converted = await api.rules.import(parsed);
      setPendingImport({ fileName: file.name, rules: converted, format: isAkev ? 'akev' : 'htk' });
    } catch (e) {
      toast.error(`Couldn't import “${file.name}”`, { description: errorMessage(e) });
    }
  }, []);

  const finishImport = useCallback(
    (mode: ImportMode, enabled: boolean) => {
      const pending = pendingImport;
      if (!pending) return;
      setPendingImport(null);
      const imported = pending.rules;
      const n = walkRules(imported).length;
      let focus: RuleId | null = null;
      const entry = mutate(mode === 'group' ? 'Import rules' : 'Replace all rules', (cur) => {
        if (mode === 'replace') {
          const set: RuleSet = { version: cur.version, items: imported.items };
          const out = enabled ? set : setAllEnabled(set, false);
          focus = out.items[0]?.id ?? null;
          return out;
        }
        const title = pending.fileName.replace(/\.(json|htkrules)$/i, '') || 'Imported rules';
        const group: GroupNode = { type: 'group', id: newId(), title, enabled, collapsed: false, items: reidentifyAll(imported.items) };
        focus = group.id;
        return insertItem(cur, group, cur.items[0] ? { type: 'before', targetId: cur.items[0].id } : { type: 'root-end' });
      });
      if (!entry) return;
      if (focus) reveal(focus, 'row');
      undoToast(mode === 'group' ? `Imported ${pluralize(n, 'rule')}` : `Replaced all rules with ${pluralize(n, 'imported rule')}`, () => model.undoEntry(entry.id));
    },
    [model, mutate, pendingImport, reveal],
  );

  const cancelImport = useCallback(() => setPendingImport(null), []);

  return { exportRules, startImport, finishImport, cancelImport, pendingImport };
}
