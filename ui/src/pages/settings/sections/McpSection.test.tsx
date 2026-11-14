import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { api } from '../../../api/client';
import { MockLicense } from '../../../api/mock/license';
import { useAppStore } from '../../../state/app';
import { licenseReducers } from '../../../state/license';
import { useUiStore } from '../../../state/ui';
import { render, screen, stub, testStatus, userEvent, within } from '../../../test/render';
import { McpSection } from './McpSection';

const TOKEN = 'k3y-0123456789abcdef-wxyz';

beforeEach(() => {
  useAppStore.setState({ status: testStatus({ apiUrl: 'http://127.0.0.1:45999' }) });
  licenseReducers.set(new MockLicense('free').status());
  stub(api, 'getConfig', () => ({ baseUrl: '', token: TOKEN }));
});

afterEach(() => {
  const ui = useUiStore.getState();
  ui.setPagePref('settings.mcp.transport', undefined);
  ui.setPagePref('settings.mcp.command', undefined);
  ui.setLastTab('settings.mcp.client', 'claude-code');
  delete (window as { __AKEV__?: unknown }).__AKEV__;
});

function codeBlock(name: string): string {
  return screen.getByRole('region', { name }).textContent ?? '';
}

describe('Settings → AI agents (MCP)', () => {
  test('Claude Code gets the akev mcp command, following the command path', async () => {
    render(<McpSection />);
    expect(codeBlock('Claude Code command')).toBe('claude mcp add akev -- akev mcp');
    const input = screen.getByRole('textbox', { name: 'Command' });
    await userEvent.clear(input);
    await userEvent.type(input, '/opt/akev/bin/akev');
    expect(codeBlock('Claude Code command')).toBe('claude mcp add akev -- /opt/akev/bin/akev mcp');
    await userEvent.click(screen.getByRole('switch', { name: /All my projects/ }));
    expect(codeBlock('Claude Code command')).toBe('claude mcp add --scope user akev -- /opt/akev/bin/akev mcp');
  });

  test('HTTP shows the endpoint and masks the token until revealed', async () => {
    render(<McpSection />);
    await userEvent.click(screen.getByRole('radio', { name: 'HTTP' }));
    expect(screen.getByText('http://127.0.0.1:45999/mcp')).toBeTruthy();
    const token = screen.getByTestId('mcp-token');
    expect(token.textContent).not.toContain(TOKEN);
    expect(token.textContent?.endsWith('wxyz')).toBe(true);
    // Snippets show the masked token too.
    expect(codeBlock('Claude Code command')).not.toContain(TOKEN);
    await userEvent.click(screen.getByRole('button', { name: 'Show token' }));
    expect(screen.getByTestId('mcp-token').textContent).toBe(TOKEN);
    expect(codeBlock('Claude Code command')).toContain(`Authorization: Bearer ${TOKEN}`);
    await userEvent.click(screen.getByRole('button', { name: 'Hide token' }));
    expect(screen.getByTestId('mcp-token').textContent).not.toContain(TOKEN);
  });

  test('the desktop app warns that HTTP settings change on restart', async () => {
    (window as { __AKEV__?: unknown }).__AKEV__ = { desktop: true };
    render(<McpSection />);
    expect(screen.getByText(/doesn't install the/)).toBeTruthy();
    await userEvent.click(screen.getByRole('radio', { name: 'HTTP' }));
    expect(screen.getByText(/picks a new port and token every time it starts/)).toBeTruthy();
  });

  test('Claude Desktop: config file per OS and the JSON snippet', async () => {
    useAppStore.setState({ status: testStatus({ platform: 'windows' }) });
    render(<McpSection />);
    await userEvent.click(screen.getByRole('tab', { name: 'Claude Desktop' }));
    const paths = screen.getAllByRole('listitem');
    expect(paths[0]?.textContent).toContain('%APPDATA%\\Claude\\claude_desktop_config.json');
    expect(within(paths[0] as HTMLElement).getByText('This computer')).toBeTruthy();
    expect(JSON.parse(codeBlock('Claude Desktop configuration'))).toEqual({ mcpServers: { akev: { command: 'akev', args: ['mcp'] } } });
    await userEvent.click(screen.getByRole('radio', { name: 'HTTP' }));
    expect(screen.getByText(/only starts local servers/)).toBeTruthy();
  });

  test('Cursor and VS Code: config files, and install links in the browser', async () => {
    render(<McpSection />);
    await userEvent.click(screen.getByRole('tab', { name: 'Cursor' }));
    expect(screen.getByRole('button', { name: 'Add to Cursor' })).toBeTruthy();
    expect(JSON.parse(codeBlock('Cursor configuration'))).toEqual({ mcpServers: { akev: { command: 'akev', args: ['mcp'] } } });
    await userEvent.click(screen.getByRole('tab', { name: 'VS Code' }));
    expect(screen.getByText('.vscode/mcp.json')).toBeTruthy();
    expect(JSON.parse(codeBlock('VS Code configuration'))).toEqual({ servers: { akev: { type: 'stdio', command: 'akev', args: ['mcp'] } } });
  });

  test('lists free and Pro tools, and offers Pro when the write tools are locked', () => {
    render(<McpSection />);
    const table = screen.getByRole('table');
    const row = (name: string) => within(table).getByText(name).closest('tr') as HTMLElement;
    expect(within(row('akev_search')).getByText('Free')).toBeTruthy();
    expect(within(row('akev_add_mock')).getByText('Pro')).toBeTruthy();
    expect(within(row('akev_llm_conversation')).getByText('Pro')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Unlock' })).toBeTruthy();
  });

  test('with Pro, the write tools are active', () => {
    licenseReducers.set(new MockLicense('pro').status());
    render(<McpSection />);
    const table = screen.getByRole('table');
    const row = within(table).getByText('akev_add_mock').closest('tr') as HTMLElement;
    expect(within(row).getByText('Pro · active')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Unlock' })).toBeNull();
  });

  test('explains the safety switches: launching, secrets, folders', () => {
    render(<McpSection />);
    expect(screen.getByText('akev start --mcp-allow-launch')).toBeTruthy();
    expect(screen.getByText('akev start --mcp-reveal-secrets')).toBeTruthy();
    expect(screen.getByText('--mcp-allow-path <folder>')).toBeTruthy();
    // The default folder agents may write to, in this Akev's data directory.
    expect(screen.getByText('/home/me/.local/share/akev/exports')).toBeTruthy();
  });
});
