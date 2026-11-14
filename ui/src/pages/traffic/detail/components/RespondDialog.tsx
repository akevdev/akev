import clsx from 'clsx';
import { MessageSquareReply } from 'lucide-react';
import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import { errorMessage } from '../../../../api/client';
import type { ExchangeId } from '../../../../api/types';
import { Button, Dialog, Tooltip, toast } from '../../../../components/ui';
import { statusColorVar } from '../../../../lib/http';
import { useAppStore } from '../../../../state/app';
import { RESPOND_PRESETS, presetResponse, type RespondPreset } from '../lib/breakpoint';
import { buildResponse, responseDraftFrom, type ResponseDraft } from '../hooks/useBreakpointDraft';
import { ResponseEditor } from './BreakpointEditors';
import styles from './RespondDialog.module.css';

export interface RespondDialogProps {
  id: ExchangeId;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called after the breakpoint was answered. */
  onResponded?: () => void;
}

const STATUS_ID = 'akev-respond-status';
const DEFAULT_PRESET = RESPOND_PRESETS[0] as RespondPreset;

function draftFor(preset: RespondPreset): ResponseDraft {
  return responseDraftFrom(presetResponse(preset));
}

/**
 * Answer a paused request without forwarding it (`RequestResume { action: 'respond' }`): quick-start
 * presets (200 JSON, 204, 302, 401, 404, 429, 5xx…), then status, reason, headers
 * (KeyValueEditor) and body (CodeEditor).
 */
export function RespondDialog({ id, open, onOpenChange, onResponded }: RespondDialogProps) {
  const [draft, setDraft] = useState<ResponseDraft>(() => draftFor(DEFAULT_PRESET));
  const [presetId, setPresetId] = useState<string | null>(DEFAULT_PRESET.id);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setDraft(draftFor(DEFAULT_PRESET));
    setPresetId(DEFAULT_PRESET.id);
  }, [open]);

  const { response, errors } = useMemo(() => buildResponse(draft), [draft]);
  const invalid = Object.values(errors)[0] ?? null;

  const applyPreset = (preset: RespondPreset) => {
    setDraft(draftFor(preset));
    setPresetId(preset.id);
  };

  const submit = async () => {
    if (invalid || busy) return;
    setBusy(true);
    try {
      await useAppStore.getState().resumeBreakpoint(id, { action: 'respond', response });
      toast.success(`Responded with ${response.status}`, { description: 'The request was not forwarded upstream.' });
      onOpenChange(false);
      onResponded?.();
    } catch (e) {
      toast.error("Couldn't respond", { description: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Respond to the paused request"
      description="The client gets this response; the request is never forwarded upstream."
      width={760}
      flush
      onOpenAutoFocus={(e) => {
        e.preventDefault();
        document.getElementById(STATUS_ID)?.focus();
      }}
      footer={
        <>
          {invalid && <span className={styles.error}>{invalid}</span>}
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="primary" icon={MessageSquareReply} kbd="mod+enter" loading={busy} disabled={!!invalid} onClick={() => void submit()}>
            Send response
          </Button>
        </>
      }
    >
      {/* biome-ignore lint/a11y/noStaticElementInteractions: scopes keyboard shortcuts for the controls inside it; not a control */}
      <div
        className={styles.editor}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && !e.defaultPrevented) {
            e.preventDefault();
            void submit();
          }
        }}
      >
        <div className={styles.presets} role="group" aria-label="Start from a template">
          <span className={styles.presetsLabel}>Start from</span>
          {RESPOND_PRESETS.map((p) => (
            <Tooltip key={p.id} content={p.description}>
              <button
                type="button"
                className={clsx(styles.preset, presetId === p.id && styles.presetActive)}
                aria-pressed={presetId === p.id}
                onClick={() => applyPreset(p)}
                style={{ '--preset-color': statusColorVar(p.status) } as CSSProperties}
              >
                <span className={styles.presetDot} aria-hidden />
                {p.label}
              </button>
            </Tooltip>
          ))}
        </div>
        <ResponseEditor
          draft={draft}
          onChange={(u) => {
            setPresetId(null);
            setDraft((d) => u(d));
          }}
          errors={errors}
          onSubmit={() => void submit()}
          label="Response"
          tabGroup="traffic.breakpoint.respond"
          compact
          statusInputId={STATUS_ID}
        />
      </div>
    </Dialog>
  );
}
