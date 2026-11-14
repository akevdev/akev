/**
 * Settings → AI agents (MCP): the client configurations, config file locations and the tool
 * catalogue. Pure helpers (tested in `mcp.test.ts`); the section is `sections/McpSection.tsx`.
 *
 * Agents connect two ways (crates/akev-cli/src/mcp.rs, crates/akev-api/src/mcp):
 * - stdio: the client starts `akev mcp`, which bridges to the running Akev (found through
 *   `daemon.json`) and reconnects when it restarts, or starts a headless one.
 * - HTTP: `POST <api>/mcp` with `Authorization: Bearer <token>` (Streamable HTTP). The token
 *   and, in the desktop app, the port change on every start.
 */

import type { Feature, Platform } from '../../api/types';

export type McpClientId = 'claude-code' | 'claude-desktop' | 'cursor' | 'vscode';
export type McpTransport = 'stdio' | 'http';

export const MCP_CLIENTS: readonly { id: McpClientId; label: string }[] = [
  { id: 'claude-code', label: 'Claude Code' },
  { id: 'claude-desktop', label: 'Claude Desktop' },
  { id: 'cursor', label: 'Cursor' },
  { id: 'vscode', label: 'VS Code' },
];

export const MCP_DOCS_URL = 'https://akev.dev/docs/mcp/';
export const CLI_DOWNLOAD_URL = 'https://akev.dev/download#cli';

/** What a client needs to reach Akev. */
export interface McpTarget {
  /** The `akev` executable (a full path when the client doesn't see the shell's PATH). */
  command: string;
  /** `<api>/mcp`. */
  url: string;
  /** The API token, or `null` when the UI doesn't know it. */
  token: string | null;
}

/** Placeholder written into HTTP snippets when the token is unknown. */
export const TOKEN_PLACEHOLDER = '<token>';

/** The MCP endpoint of an API base URL (`http://127.0.0.1:45456` → `…/mcp`). */
export function mcpUrl(apiUrl: string): string {
  return `${apiUrl.replace(/\/+$/, '')}/mcp`;
}

/** Mask a secret for display: `••••••••` plus its last four characters. */
export function maskToken(token: string): string {
  return token.length <= 8 ? '•'.repeat(token.length) : `${'•'.repeat(12)}${token.slice(-4)}`;
}

/** Quote one shell argument when it needs it (POSIX shells; PowerShell takes the same quoting for these). */
export function shellQuote(arg: string): string {
  return /^[\w@%+=:,./-]+$/.test(arg) ? arg : `'${arg.replace(/'/g, `'\\''`)}'`;
}

function bearer(token: string | null): string {
  return `Bearer ${token ?? TOKEN_PLACEHOLDER}`;
}

/** `claude mcp add …` for Claude Code. */
export function claudeCodeCommand(t: McpTarget, transport: McpTransport, scope: 'local' | 'user' = 'local'): string {
  const scopeArg = scope === 'user' ? ' --scope user' : '';
  if (transport === 'http') return `claude mcp add --transport http${scopeArg} akev ${t.url} --header ${shellQuote(`Authorization: ${bearer(t.token)}`)}`;
  return `claude mcp add${scopeArg} akev -- ${shellQuote(t.command)} mcp`;
}

/** The server entry (stdio or HTTP) as a client's JSON config expects it. */
function serverEntry(t: McpTarget, transport: McpTransport, client: 'claude-desktop' | 'cursor' | 'vscode'): Record<string, unknown> {
  if (transport === 'http') {
    const headers = { Authorization: bearer(t.token) };
    return client === 'vscode' ? { type: 'http', url: t.url, headers } : { url: t.url, headers };
  }
  return client === 'vscode' ? { type: 'stdio', command: t.command, args: ['mcp'] } : { command: t.command, args: ['mcp'] };
}

/** `claude_desktop_config.json`, `.cursor/mcp.json` or `.vscode/mcp.json` (VS Code says `servers`). */
export function jsonConfig(client: 'claude-desktop' | 'cursor' | 'vscode', t: McpTarget, transport: McpTransport): string {
  const key = client === 'vscode' ? 'servers' : 'mcpServers';
  return JSON.stringify({ [key]: { akev: serverEntry(t, transport, client) } }, null, 2);
}

/** Claude Desktop only starts local servers from its config file (no HTTP entries). */
export function supportsTransport(client: McpClientId, transport: McpTransport): boolean {
  return !(client === 'claude-desktop' && transport === 'http');
}

/** Where Claude Desktop keeps its config, per OS (it has no Linux build). */
export const CLAUDE_DESKTOP_CONFIG: readonly { platform: Platform; label: string; path: string }[] = [
  { platform: 'macos', label: 'macOS', path: '~/Library/Application Support/Claude/claude_desktop_config.json' },
  { platform: 'windows', label: 'Windows', path: '%APPDATA%\\Claude\\claude_desktop_config.json' },
];

/** The config paths with this computer's first. */
export function claudeDesktopPaths(platform: Platform | undefined): typeof CLAUDE_DESKTOP_CONFIG {
  return [...CLAUDE_DESKTOP_CONFIG].sort((a, b) => Number(b.platform === platform) - Number(a.platform === platform));
}

/** Cursor's "Add to Cursor" deep link (`config` is the server entry, base64). */
export function cursorInstallLink(t: McpTarget, transport: McpTransport): string {
  const config = btoa(JSON.stringify(serverEntry(t, transport, 'cursor')));
  return `cursor://anysphere.cursor-deeplink/mcp/install?name=akev&config=${encodeURIComponent(config)}`;
}

/** VS Code's MCP install link (`vscode:mcp/install?<server JSON>`). */
export function vscodeInstallLink(t: McpTarget, transport: McpTransport): string {
  return `vscode:mcp/install?${encodeURIComponent(JSON.stringify({ name: 'akev', ...serverEntry(t, transport, 'vscode') }))}`;
}

// =====================================================================================
// Tools (crates/akev-api/src/mcp/tools/mod.rs: `MCP_WRITE_TOOLS`, `pro_feature`)
// =====================================================================================

export interface McpTool {
  name: string;
  purpose: string;
  /** The Pro feature the tool needs (`null`: free). */
  feature: Feature | null;
}

export const MCP_TOOLS: readonly McpTool[] = [
  { name: 'akev_status', purpose: 'Proxy address, CA, statistics and the environment variables that route a program through Akev', feature: null },
  { name: 'akev_list_exchanges', purpose: 'List captured exchanges, optionally filtered', feature: null },
  { name: 'akev_get_exchange', purpose: 'One exchange in full, with decoded bodies', feature: null },
  { name: 'akev_get_body', purpose: 'Read a request or response body, or a slice of it', feature: null },
  { name: 'akev_search', purpose: 'Full-text search of URLs, headers and bodies', feature: null },
  { name: 'akev_wait_for', purpose: 'Wait until a matching exchange is captured', feature: null },
  { name: 'akev_get_rules', purpose: 'Read the rules, with match counts', feature: null },
  { name: 'akev_test_rules', purpose: 'Dry-run rules against an exchange or an ad-hoc request', feature: null },
  { name: 'akev_export_har', purpose: 'Export traffic as HAR, inline or to a file', feature: null },
  { name: 'akev_snippet', purpose: 'Code that reproduces a captured request', feature: null },
  { name: 'akev_list_breakpoints', purpose: 'Exchanges paused at a breakpoint', feature: null },
  { name: 'akev_list_interceptors', purpose: 'Browsers, terminals and other clients Akev can launch', feature: null },
  { name: 'akev_deactivate_interceptor', purpose: 'Stop an interceptor (always allowed, so an agent can undo a launch)', feature: null },
  { name: 'akev_filter_help', purpose: 'The filter language reference', feature: null },
  { name: 'akev_llm_conversation', purpose: 'The conversation of an LLM API call: messages, tool calls, tokens', feature: 'llm-lens' },
  { name: 'akev_send_request', purpose: 'Send a request, like Compose', feature: 'mcp-write' },
  { name: 'akev_replay', purpose: 'Replay a captured request, optionally modified', feature: 'mcp-write' },
  { name: 'akev_set_rules', purpose: 'Replace the rules', feature: 'mcp-write' },
  { name: 'akev_add_mock', purpose: 'Add a mock response rule', feature: 'mcp-write' },
  { name: 'akev_resume_breakpoint', purpose: 'Resume a paused exchange, optionally edited', feature: 'mcp-write' },
  { name: 'akev_set_recording', purpose: 'Pause or resume recording', feature: 'mcp-write' },
  { name: 'akev_clear_traffic', purpose: 'Clear captured traffic', feature: 'mcp-write' },
  { name: 'akev_activate_interceptor', purpose: 'Launch or configure a browser, terminal or other client', feature: 'mcp-write' },
];
