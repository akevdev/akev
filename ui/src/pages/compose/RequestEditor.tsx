import { memo, useCallback, useMemo } from 'react';
import { CodeView } from '../../components/exchange';
import { KeyValueEditor, TabList, Tabs, TabsContent, type KeyValueRow } from '../../components/ui';
import { AuthEditor, AUTH_LABEL } from './AuthEditor';
import { BodyEditor, BODY_MODE_LABEL } from './BodyEditor';
import { HeadersEditor } from './HeadersEditor';
import { useDebouncedValue } from './hooks';
import type { BodyState, ComposeTab, ParamRow, RequestDraft, RequestTabId } from './model';
import { buildRequest } from './request';
import { tabsActions } from './store';
import { urlWithParams } from './urlParams';
import type { VarMap } from './variables';
import styles from './RequestEditor.module.css';

export interface RequestEditorProps {
  tab: ComposeTab;
  vars: VarMap;
  onSubmit: () => void;
  onSave: () => void;
}

/** Params · Headers · Body · Auth · Code for the active tab. */
export const RequestEditor = memo(function RequestEditor({ tab, vars, onSubmit, onSave }: RequestEditorProps) {
  const { id, draft } = tab;
  const update = useCallback((fn: (d: RequestDraft) => RequestDraft) => tabsActions.updateDraft(id, fn), [id]);

  const onParams = useCallback((rows: KeyValueRow[]) => update((d) => ({ ...d, params: rows as ParamRow[], url: urlWithParams(d.url, rows as ParamRow[]) })), [update]);
  const onHeaders = useCallback((headers: KeyValueRow[]) => update((d) => ({ ...d, headers })), [update]);
  const onToggleAuto = useCallback(
    (key: string, enabled: boolean) => update((d) => ({ ...d, disabledAuto: enabled ? d.disabledAuto.filter((k) => k !== key) : [...d.disabledAuto.filter((k) => k !== key), key] })),
    [update],
  );
  const onBody = useCallback((fn: (b: BodyState) => BodyState) => update((d) => ({ ...d, body: fn(d.body) })), [update]);
  const onAuth = useCallback((auth: RequestDraft['auth']) => update((d) => ({ ...d, auth })), [update]);
  const setTab = useCallback((requestTab: RequestTabId) => tabsActions.patch(id, { requestTab }), [id]);

  const paramCount = draft.params.filter((p) => p.enabled && (p.name || p.value)).length;
  const headerCount = draft.headers.filter((h) => h.enabled && h.name.trim()).length;

  // The Code tab regenerates on a debounced, resolved request.
  const showCode = tab.requestTab === 'code';
  const debouncedDraft = useDebouncedValue(draft, 300);
  const codeRequest = useMemo(() => (showCode ? buildRequest(debouncedDraft, vars).request : null), [showCode, debouncedDraft, vars]);

  return (
    <Tabs value={tab.requestTab} onValueChange={setTab} className={styles.tabs}>
      <TabList
        aria-label="Request"
        className={styles.tabList}
        items={[
          { value: 'params', label: 'Params', count: paramCount || undefined },
          { value: 'headers', label: 'Headers', count: headerCount || undefined },
          { value: 'body', label: 'Body', badge: draft.body.mode !== 'none' ? BODY_MODE_LABEL[draft.body.mode] : undefined },
          { value: 'auth', label: 'Auth', badge: draft.auth.type !== 'none' ? AUTH_LABEL[draft.auth.type] : undefined },
          { value: 'code', label: 'Code' },
        ]}
      />
      <TabsContent value="params" className={styles.panel}>
        <div className={styles.scrollPad}>
          <div className={styles.block}>
            <div className={styles.blockHead}>
              <span className={styles.blockTitle}>Query parameters</span>
              <span className={styles.blockHint}>Synced with the URL both ways. Unchecked params stay here but aren't sent.</span>
            </div>
            <KeyValueEditor rows={draft.params} onChange={onParams} separator="=" namePlaceholder="Parameter" valuePlaceholder="Value" aria-label="Query parameters" />
          </div>
        </div>
      </TabsContent>
      <TabsContent value="headers" className={styles.panel}>
        <div className={styles.scrollPad}>
          <HeadersEditor draft={draft} vars={vars} onHeaders={onHeaders} onToggleAuto={onToggleAuto} onOpenAuth={() => setTab('auth')} onOpenBody={() => setTab('body')} />
        </div>
      </TabsContent>
      <TabsContent value="body" className={styles.panel}>
        <BodyEditor tabId={id} body={draft.body} method={draft.method} vars={vars} onBody={onBody} onSubmit={onSubmit} onSave={onSave} />
      </TabsContent>
      <TabsContent value="auth" className={styles.panel}>
        <AuthEditor auth={draft.auth} vars={vars} onAuth={onAuth} />
      </TabsContent>
      <TabsContent value="code" className={styles.panel}>
        <div className={styles.codePane}>{codeRequest && <CodeView request={codeRequest} className={styles.codeView} />}</div>
      </TabsContent>
    </Tabs>
  );
});
