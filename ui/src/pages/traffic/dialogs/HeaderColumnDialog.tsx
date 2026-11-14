import { Columns3 } from 'lucide-react';
import { useMemo, useState } from 'react';
import { Button, Combobox, Dialog, Field, SegmentedControl, type ComboboxOption } from '../../../components/ui';
import { KNOWN_HEADER_NAMES } from '../../../lib/headerDocs';
import { headerColumnId, isHeaderColumn, MAX_HEADER_COLUMNS, useUiStore } from '../../../state/ui';
import { showHeaderColumn } from '../actions/headerColumns';
import styles from './TrafficDialogs.module.css';

type Side = 'request' | 'response';

/** One-click picks for each side. */
const POPULAR: Readonly<Record<Side, readonly string[]>> = {
  request: ['X-Request-Id', 'User-Agent', 'Authorization', 'Referer', 'Origin', 'Accept-Language'],
  response: ['X-Request-Id', 'Cache-Control', 'Content-Encoding', 'Server', 'Age', 'X-Cache'],
};

/** Add a list column showing a request or response header's value (lazy: loads the header names). */
export function HeaderColumnDialog({ onClose }: { onClose: () => void }) {
  const [side, setSide] = useState<Side>('response');
  const [name, setName] = useState('');
  const columns = useUiStore((s) => s.columns);
  const used = columns.filter((c) => isHeaderColumn(c.id)).length;
  const options = useMemo<ComboboxOption[]>(() => KNOWN_HEADER_NAMES.map((n) => ({ value: n })), []);
  const trimmed = name.trim();
  const valid = trimmed !== '' && isHeaderColumn(headerColumnId(side, trimmed));
  const full = used >= MAX_HEADER_COLUMNS;
  const add = (n = trimmed) => {
    if (showHeaderColumn(side, n)) onClose();
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title="Add a header column"
      description="Show a header's value for every exchange in the traffic list. Values load for the rows on screen."
      width={480}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" icon={Columns3} disabled={!valid || full} onClick={() => add()}>
            Add column
          </Button>
        </>
      }
    >
      <div className={styles.headerColumnForm}>
        <SegmentedControl<Side>
          value={side}
          onValueChange={setSide}
          items={[
            { value: 'response', label: 'Response header' },
            { value: 'request', label: 'Request header' },
          ]}
          aria-label="Which headers"
        />
        <Field
          label="Header name"
          error={trimmed && !valid ? 'Header names are letters, digits and - _ . ! # $ % & \' * + ^ ` | ~ (no spaces).' : full ? `Up to ${MAX_HEADER_COLUMNS} header columns can be shown. Remove one from the column menu first.` : undefined}
        >
          <Combobox
            autoFocus
            value={name}
            onValueChange={setName}
            options={options}
            maxResults={12}
            openOnFocus={false}
            placeholder="X-Request-Id"
            spellCheck={false}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && valid && !full && !e.defaultPrevented) {
                e.preventDefault();
                add();
              }
            }}
          />
        </Field>
        <div className={styles.headerColumnPicks}>
          {POPULAR[side].map((n) => (
            <Button key={n} size="sm" variant="secondary" disabled={full || columns.some((c) => c.id === headerColumnId(side, n))} onClick={() => add(n)}>
              {n}
            </Button>
          ))}
        </div>
      </div>
    </Dialog>
  );
}

export default HeaderColumnDialog;
