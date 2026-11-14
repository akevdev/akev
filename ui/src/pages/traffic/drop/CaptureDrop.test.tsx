import { describe, expect, test } from 'bun:test';
import { useRef } from 'react';
import { api } from '../../../api/client';
import { act, fireEvent, render, screen, stub, waitFor } from '../../../test/render';
import { CaptureDropOverlay, useCaptureDrop } from './CaptureDrop';

function Zone({ onImported }: { onImported?: (ids: number[]) => void }) {
  const ref = useRef<HTMLDivElement | null>(null);
  const over = useCaptureDrop(ref, onImported);
  return (
    <div ref={ref} data-testid="zone">
      <CaptureDropOverlay visible={over} />
    </div>
  );
}

/** A drag event carrying files (or, without `files`, some other drag such as a column). */
function dataTransfer(files?: File[]) {
  return { types: files ? ['Files'] : ['text/plain'], files: files ?? [], dropEffect: 'none' };
}

describe('dropping capture files on Traffic', () => {
  test('shows the overlay while files are dragged over, not for other drags', () => {
    render(<Zone />);
    const zone = screen.getByTestId('zone');
    fireEvent.dragEnter(zone, { dataTransfer: dataTransfer() });
    expect(screen.queryByText('Drop to import')).toBeNull();
    fireEvent.dragEnter(zone, { dataTransfer: dataTransfer([]) });
    expect(screen.getByText('Drop to import')).toBeTruthy();
    fireEvent.dragLeave(zone, { dataTransfer: dataTransfer([]) });
    expect(screen.queryByText('Drop to import')).toBeNull();
  });

  test('imports each file with the importer its format needs', async () => {
    const har = stub(api.transfer, 'importHar', async () => ({ ids: [7, 8] }));
    const charles = stub(api.transfer, 'importCharles', async () => ({ ids: [9] }));
    const imported: number[][] = [];
    render(<Zone onImported={(ids) => imported.push(ids)} />);
    const zone = screen.getByTestId('zone');
    const files = [new File(['{"log":{"entries":[]}}'], 'checkout.har'), new File(['[{"status":"COMPLETE"}]'], 'charles-export.json')];
    fireEvent.dragEnter(zone, { dataTransfer: dataTransfer(files) });
    await act(async () => {
      fireEvent.drop(zone, { dataTransfer: dataTransfer(files) });
    });
    await waitFor(() => expect(imported).toEqual([[7, 8, 9]]));
    expect(new TextDecoder().decode(har[0]?.[0] as Uint8Array)).toBe('{"log":{"entries":[]}}');
    expect(charles).toHaveLength(1);
    expect(screen.queryByText('Drop to import')).toBeNull();
  });

  test("files that aren't captures are refused without calling the daemon", async () => {
    const har = stub(api.transfer, 'importHar', async () => ({ ids: [1] }));
    const imported: number[][] = [];
    render(<Zone onImported={(ids) => imported.push(ids)} />);
    await act(async () => {
      fireEvent.drop(screen.getByTestId('zone'), { dataTransfer: dataTransfer([new File(['hello'], 'notes.txt')]) });
    });
    await new Promise((r) => setTimeout(r, 20));
    expect(har).toHaveLength(0);
    expect(imported).toEqual([]);
  });
});
