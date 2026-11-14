/**
 * Typed protobuf decoding in the body viewer: the schema bar (which message type decoded the
 * body, where it came from, Typed ↔ Schemaless) and the notices around it.
 */

import clsx from 'clsx';
import { Braces, FileCode2 } from 'lucide-react';
import type { SchemaInfo } from '../../../api/types';
import { navigate } from '../../../app/router';
import { schemaSourceText, shortTypeName, unknownFieldsText } from '../../../lib/protobuf';
import { usePagePref } from '../../../state/ui';
import { Button, SegmentedControl, Tooltip } from '../../ui';
import { Banner } from '../parts/Parts';
import styles from './schema.module.css';

export type ProtoViewMode = 'typed' | 'schemaless';

const MODE_PREF = 'exchange.body.protoView';

/** Typed vs schemaless, remembered across bodies. */
export function useProtoViewMode(): [ProtoViewMode, (m: ProtoViewMode) => void] {
  const [mode, setMode] = usePagePref<ProtoViewMode>(MODE_PREF, 'typed');
  return [mode === 'schemaless' ? 'schemaless' : 'typed', setMode];
}

/**
 * The bar above typed decodes: `schema: shop.v1.Item` (source in the tooltip) and the
 * Typed / Schemaless toggle (when both decodes exist).
 */
export function SchemaBar({ schema, mode, onModeChange, canToggle }: { schema: SchemaInfo; mode: ProtoViewMode; onModeChange: (m: ProtoViewMode) => void; canToggle: boolean }) {
  const failed = schema.error != null;
  return (
    <div className={styles.bar} role="toolbar" aria-label="Protobuf schema">
      <Tooltip content={`${failed ? 'Matched' : 'Decoded'} with ${schema.messageType}, chosen by ${schemaSourceText(schema)}`} multiline>
        <span className={clsx(styles.schema, failed && styles.schemaFailed)} data-testid="schema-badge">
          <FileCode2 size={12} strokeWidth={2} aria-hidden />
          <span className={styles.schemaLabel}>schema:</span>
          <span className={styles.schemaType}>{schema.messageType}</span>
        </span>
      </Tooltip>
      <span className={styles.source}>{sourceLabel(schema)}</span>
      <span className={styles.spacer} />
      {canToggle && (
        <SegmentedControl<ProtoViewMode>
          size="sm"
          aria-label="Protobuf view"
          value={mode}
          onValueChange={onModeChange}
          items={[
            { value: 'typed', label: 'Typed' },
            { value: 'schemaless', label: 'Schemaless' },
          ]}
        />
      )}
    </div>
  );
}

function sourceLabel(schema: SchemaInfo): string {
  switch (schema.source) {
    case 'grpc':
      return schema.method ? `${schema.method.method} ${schema.method.inputType === schema.messageType ? 'request' : 'response'}` : 'gRPC';
    case 'content-type':
      return 'from Content-Type';
    case 'mapping':
      return schema.mapping ? `mapping ${schema.mapping}` : 'mapping';
  }
}

/** Notices about a typed decode: mismatch (falls back to schemaless) and unknown fields. */
export function SchemaNotices({ schema, unknownFields, showingTyped }: { schema: SchemaInfo; unknownFields?: string[]; showingTyped: boolean }) {
  const fields = unknownFields ?? schema.unknownFields;
  const unknown = unknownFieldsText(fields);
  const one = fields?.length === 1;
  return (
    <>
      {schema.error && (
        <Banner tone="warning" className={styles.notice}>
          Doesn't decode as <code>{shortTypeName(schema.messageType)}</code>: {schema.error}. Showing the schemaless decode.
        </Banner>
      )}
      {showingTyped && unknown && (
        <Banner tone="info" className={styles.notice}>
          {one ? 'Field' : 'Fields'} <code>{unknown}</code> {one ? "isn't" : "aren't"} in <code>{shortTypeName(schema.messageType)}</code> — the schema may be out of date. {one ? "It's" : "They're"} shown by field number.
        </Banner>
      )}
    </>
  );
}

/** "Decoded without a schema" with a shortcut to Settings → Protobuf schemas. */
export function NoSchemaNotice() {
  return (
    <Banner
      tone="neutral"
      icon={Braces}
      className={styles.notice}
      actions={
        <Button size="sm" variant="ghost" onClick={() => navigate('/settings/protobuf')}>
          Add schema
        </Button>
      }
    >
      Decoded without a schema — keys are protobuf field numbers. Import the <code>.proto</code> files to see names.
    </Banner>
  );
}
