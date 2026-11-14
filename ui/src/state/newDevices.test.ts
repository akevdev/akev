import { describe, expect, test } from 'bun:test';
import { mockSettings } from '../api/mock/data';
import { validateIpRange } from '../pages/settings/sections/AccessCard';
import { withBlockedClient } from './newDevices';

describe('new devices', () => {
  test('blocking adds the address once and keeps the rest of the settings', () => {
    const base = mockSettings();
    const blocked = withBlockedClient(base, '192.168.1.23');
    expect(blocked.proxy.access?.block).toEqual(['192.168.1.23']);
    expect(blocked.proxy.access?.policy).toBe(base.proxy.access?.policy ?? 'private');
    expect(blocked.proxy.port).toBe(base.proxy.port);
    expect(withBlockedClient(blocked, '192.168.1.23')).toBe(blocked);
    expect(withBlockedClient(blocked, '10.0.0.5').proxy.access?.block).toEqual(['192.168.1.23', '10.0.0.5']);
  });

  test('allow and block entries are IP addresses or CIDR ranges', () => {
    for (const ok of ['192.168.1.23', '10.0.0.0/8', 'fd00::/8', '::1', '0.0.0.0/0']) expect(validateIpRange(ok)).toBeNull();
    for (const bad of ['', 'phone', '10.0.0.0/33', 'fd00::/129', '1.2.3.4/8/9', '10.0.0.0/x']) expect(validateIpRange(bad)).not.toBeNull();
  });
});
