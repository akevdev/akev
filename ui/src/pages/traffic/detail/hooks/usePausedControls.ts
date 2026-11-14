import { MessageSquareReply, Play, Unplug, Zap } from 'lucide-react';
import { useState } from 'react';
import { errorMessage } from '../../../../api/client';
import type { PausedExchange } from '../../../../api/types';
import { useCommands } from '../../../../app/commands';
import { useShortcut } from '../../../../app/shortcuts';
import { toast } from '../../../../components/ui';
import { useShallow } from 'zustand/react/shallow';
import { useAppStore } from '../../../../state/app';
import { openExchange } from '../../actions/exchangeActions';
import type { BreakpointDraftState } from './useBreakpointDraft';

export type PausedAction = 'resume' | 'close' | 'reset';

export interface PausedControls {
  busy: PausedAction | null;
  /** The first validation problem blocking Resume. */
  invalid: string | null;
  run: (kind: PausedAction) => Promise<void>;
  respondOpen: boolean;
  setRespondOpen: (open: boolean) => void;
  /** Other exchanges paused right now. */
  othersPaused: number;
  openNextPaused: () => void;
}

/**
 * Resume / close / reset for a paused exchange, with the ⌘↵ shortcut and palette commands.
 * `describe` names the exchange in the confirmation toast (e.g. `GET example.com/path`).
 */
export function usePausedControls(paused: PausedExchange, bp: BreakpointDraftState, describe: string): PausedControls {
  const others = useAppStore(useShallow((s) => s.paused.filter((p) => p.id !== paused.id).map((p) => p.id)));
  const [busy, setBusy] = useState<PausedAction | null>(null);
  const [respondOpen, setRespondOpen] = useState(false);
  const invalid = Object.values(bp.errors)[0] ?? null;

  const run = async (kind: PausedAction) => {
    if (busy) return;
    let resume;
    if (kind === 'resume') {
      const payload = bp.build();
      if (!payload) {
        toast.error("Can't resume yet", { description: invalid ?? 'Fix the highlighted fields first.' });
        return;
      }
      resume = payload.kind === 'request' ? ({ action: 'continue', request: payload.request } as const) : ({ action: 'continue', response: payload.response } as const);
    } else {
      resume = { action: kind } as const;
    }
    setBusy(kind);
    const edited = bp.changes;
    try {
      await useAppStore.getState().resumeBreakpoint(paused.id, resume);
      bp.clear();
      if (kind === 'resume') toast.success(edited ? 'Resumed with your edits' : 'Resumed', { description: describe });
      else toast(kind === 'close' ? 'Connection closed' : 'Connection reset', { description: 'The client was disconnected at the breakpoint.' });
    } catch (e) {
      toast.error(kind === 'resume' ? "Couldn't resume" : `Couldn't ${kind} the connection`, { description: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  useShortcut('mod+enter', () => void run('resume'), { allowInInputs: true, description: 'Resume paused exchange', group: 'Breakpoints' });
  useCommands([
    { id: 'breakpoint.resume', title: bp.changes ? 'Resume with edits' : 'Resume paused exchange', group: 'Breakpoints', icon: Play, shortcut: 'mod+enter', run: () => void run('resume') },
    ...(paused.stage === 'request' ? [{ id: 'breakpoint.respond', title: 'Respond to paused request…', group: 'Breakpoints', icon: MessageSquareReply, run: () => setRespondOpen(true) }] : []),
    { id: 'breakpoint.close', title: 'Close paused connection', group: 'Breakpoints', icon: Unplug, run: () => void run('close') },
    { id: 'breakpoint.reset', title: 'Reset paused connection', group: 'Breakpoints', icon: Zap, run: () => void run('reset') },
  ]);

  const openNextPaused = () => {
    const next = others.find((o) => o > paused.id) ?? others[0];
    if (next != null) openExchange(next);
  };

  return { busy, invalid, run, respondOpen, setRespondOpen, othersPaused: others.length, openNextPaused };
}
