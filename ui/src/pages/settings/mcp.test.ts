import { describe, expect, test } from 'bun:test';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { claudeCodeCommand, claudeDesktopPaths, cursorInstallLink, jsonConfig, maskToken, MCP_TOOLS, mcpUrl, shellQuote, supportsTransport, TOKEN_PLACEHOLDER, vscodeInstallLink, type McpTarget } from './mcp';

const target: McpTarget = { command: 'akev', url: 'http://127.0.0.1:45456/mcp', token: 'tok-123456789' };

describe('MCP client configuration', () => {
  test('the endpoint is /mcp on the API address', () => {
    expect(mcpUrl('http://127.0.0.1:45456')).toBe('http://127.0.0.1:45456/mcp');
    expect(mcpUrl('http://127.0.0.1:45456/')).toBe('http://127.0.0.1:45456/mcp');
  });

  test('Claude Code: claude mcp add, stdio or HTTP', () => {
    expect(claudeCodeCommand(target, 'stdio')).toBe('claude mcp add akev -- akev mcp');
    expect(claudeCodeCommand(target, 'stdio', 'user')).toBe('claude mcp add --scope user akev -- akev mcp');
    expect(claudeCodeCommand({ ...target, command: '/opt/My Tools/akev' }, 'stdio')).toBe("claude mcp add akev -- '/opt/My Tools/akev' mcp");
    expect(claudeCodeCommand(target, 'http')).toBe("claude mcp add --transport http akev http://127.0.0.1:45456/mcp --header 'Authorization: Bearer tok-123456789'");
    expect(claudeCodeCommand({ ...target, token: null }, 'http')).toContain(`Bearer ${TOKEN_PLACEHOLDER}`);
  });

  test('Claude Desktop and Cursor use mcpServers; VS Code uses servers with a type', () => {
    expect(JSON.parse(jsonConfig('claude-desktop', target, 'stdio'))).toEqual({ mcpServers: { akev: { command: 'akev', args: ['mcp'] } } });
    expect(JSON.parse(jsonConfig('cursor', target, 'http'))).toEqual({ mcpServers: { akev: { url: target.url, headers: { Authorization: 'Bearer tok-123456789' } } } });
    expect(JSON.parse(jsonConfig('vscode', target, 'stdio'))).toEqual({ servers: { akev: { type: 'stdio', command: 'akev', args: ['mcp'] } } });
    expect(JSON.parse(jsonConfig('vscode', target, 'http'))).toEqual({ servers: { akev: { type: 'http', url: target.url, headers: { Authorization: 'Bearer tok-123456789' } } } });
  });

  test('Claude Desktop has no HTTP entries', () => {
    expect(supportsTransport('claude-desktop', 'http')).toBe(false);
    expect(supportsTransport('claude-desktop', 'stdio')).toBe(true);
    expect(supportsTransport('cursor', 'http')).toBe(true);
  });

  test("the config path for this computer's OS comes first", () => {
    expect(claudeDesktopPaths('windows')[0]?.path).toBe('%APPDATA%\\Claude\\claude_desktop_config.json');
    expect(claudeDesktopPaths('macos')[0]?.path).toBe('~/Library/Application Support/Claude/claude_desktop_config.json');
    expect(claudeDesktopPaths('linux')).toHaveLength(2);
  });

  test('install links carry the server entry', () => {
    const cursor = new URL(cursorInstallLink(target, 'stdio'));
    expect(cursor.protocol).toBe('cursor:');
    expect(cursor.searchParams.get('name')).toBe('akev');
    expect(JSON.parse(atob(cursor.searchParams.get('config') ?? ''))).toEqual({ command: 'akev', args: ['mcp'] });
    const vscode = vscodeInstallLink(target, 'stdio');
    expect(vscode.startsWith('vscode:mcp/install?')).toBe(true);
    expect(JSON.parse(decodeURIComponent(vscode.slice('vscode:mcp/install?'.length)))).toEqual({ name: 'akev', type: 'stdio', command: 'akev', args: ['mcp'] });
  });

  test('tokens are masked except for the last four characters', () => {
    expect(maskToken('abcdefghijklmnop')).toBe('••••••••••••mnop');
    expect(maskToken('short')).toBe('•••••');
  });

  test('shell quoting', () => {
    expect(shellQuote('akev')).toBe('akev');
    expect(shellQuote('C:\\Tools\\akev.exe')).toBe("'C:\\Tools\\akev.exe'");
    expect(shellQuote("it's")).toBe("'it'\\''s'");
  });
});

describe('MCP tool catalogue', () => {
  // The daemon's tool table (crates/akev-api/src/mcp/tools/) is the source of truth: keep the
  // Settings list in step with it.
  const dir = join(import.meta.dir, '../../../../crates/akev-api/src/mcp/tools');
  const tools = readdirSync(dir)
    .filter((f) => f.endsWith('.rs'))
    .map((f) => readFileSync(join(dir, f), 'utf8'))
    .join('\n');

  test('lists every tool the daemon offers', () => {
    const dispatched = [...tools.matchAll(/^\s*"(akev_[a-z_]+)" =>/gm)].map((m) => m[1]);
    expect(dispatched.length).toBeGreaterThan(10);
    expect(MCP_TOOLS.map((t) => t.name).sort()).toEqual([...new Set(dispatched)].sort() as string[]);
  });

  test('marks exactly the write tools as Pro', () => {
    const block = /MCP_WRITE_TOOLS: &\[&str\] = &\[([\s\S]*?)\];/.exec(tools)?.[1] ?? '';
    const write = [...block.matchAll(/"(akev_[a-z_]+)"/g)].map((m) => m[1]);
    expect(write.length).toBeGreaterThan(3);
    expect(MCP_TOOLS.filter((t) => t.feature === 'mcp-write').map((t) => t.name).sort()).toEqual(write.sort() as string[]);
    expect(MCP_TOOLS.find((t) => t.name === 'akev_llm_conversation')?.feature).toBe('llm-lens');
  });
});
