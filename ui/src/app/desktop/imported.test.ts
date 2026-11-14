import { describe, expect, test } from 'bun:test';
import { toast } from '../../components/ui/toast';
import { stub } from '../../test/render';
import { getRoute } from '../router';
import { announceImported } from './imported';

describe('files opened with Akev (akev://imported)', () => {
  test('a toast with a Show action', () => {
    const shown = stub(toast, 'success', (() => 'id') as unknown as typeof toast.success);
    announceImported({ file: 'checkout.har', count: 12, firstId: 41 });
    expect(shown[0]?.[0]).toBe('Imported 12 exchanges');
    const options = shown[0]?.[1] as { description: string; action: { label: string; onClick: () => void } };
    expect(options.description).toBe('checkout.har');
    expect(options.action.label).toBe('Show');
    options.action.onClick();
    expect(getRoute()).toMatchObject({ page: 'traffic', params: { id: '41' } });
  });

  test('the error, and what to try next', () => {
    const errors = stub(toast, 'error', (() => 'id') as unknown as typeof toast.error);
    announceImported({ file: 'broken.saz', error: "couldn't import broken.saz: not a zip archive." });
    expect(errors[0]?.[0]).toBe("Couldn't open broken.saz");
    expect((errors[0]?.[1] as { description: string }).description).toBe("couldn't import broken.saz: not a zip archive. Check that the file is complete, or export it again.");
  });

  test('ignores payloads it does not understand', () => {
    const shown = stub(toast, 'success', (() => 'id') as unknown as typeof toast.success);
    announceImported('nonsense');
    announceImported(null);
    expect(shown).toHaveLength(0);
  });
});
