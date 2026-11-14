import { Info, SearchX } from 'lucide-react';
import type { RefObject } from 'react';
import { Button, EmptyState, Tooltip } from '../../components/ui';
import { pluralize } from '../../lib/format';
import type { FlatRow } from '../../lib/rules/tree';
import { RuleTree, type RuleTreeHandle, type RuleTreeProps } from './list/RuleTree';
import styles from './RulesPage.module.css';

type TreeProps = Omit<RuleTreeProps, 'rows' | 'dragDisabled' | 'aria-label'>;

export interface RuleListPaneProps extends TreeProps {
  treeRef: RefObject<RuleTreeHandle | null>;
  rows: FlatRow[];
  query: string;
  searching: boolean;
  matchingRules: number;
  onClearSearch(): void;
}

/** The rule list: how-evaluation-works line, the tree (or "no matches"), search footer. */
export function RuleListPane({ treeRef, rows, query, searching, matchingRules, onClearSearch, ...tree }: RuleListPaneProps) {
  return (
    <aside className={styles.list} aria-label="Rule list">
      <div className={styles.infoLine}>
        <Info size={13} aria-hidden />
        <span className="truncate">
          Top to bottom · <span className={styles.infoModify}>modifiers stack</span> · first <span className={styles.infoRespond}>respond</span> rule wins
        </span>
        <Tooltip
          multiline
          content="Rules are checked in order. Rewrites, delays, throttles, breakpoints, highlights and scripts apply and evaluation continues; the first matching mock / map / pass-through / close rule answers the request. If none answers, it goes to the server unchanged. Disabled rules, disabled groups and rules past their match limit are skipped."
        >
          <button type="button" className={styles.infoMore}>
            How it works
          </button>
        </Tooltip>
      </div>
      {rows.length === 0 && searching ? (
        <EmptyState
          size="sm"
          icon={SearchX}
          title="No matching rules"
          description={<>Nothing matches “{query}”.</>}
          actions={
            <Button size="sm" onClick={onClearSearch}>
              Clear search
            </Button>
          }
        />
      ) : (
        <RuleTree ref={treeRef} rows={rows} dragDisabled={searching} aria-label="Rules, in evaluation order" {...tree} />
      )}
      {searching && rows.length > 0 && (
        <div className={styles.searchNote}>
          <span className="tnum">{pluralize(matchingRules, 'matching rule')}</span> · clear the search to reorder
        </div>
      )}
    </aside>
  );
}
