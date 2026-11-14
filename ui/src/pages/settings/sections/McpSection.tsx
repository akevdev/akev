import { Download, Eye, EyeOff, Info, ShieldAlert, TriangleAlert } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { api } from '../../../api/client';
import type { Platform } from '../../../api/types';
import { ProBadge, ProCallout } from '../../../components/pro';
import { Badge, Button, CopyButton, IconButton, Input, SegmentedControl, Switch, TabList, Tabs, TabsContent } from '../../../components/ui';
import { isDesktop, openExternal } from '../../../lib/platform';
import { useAppStore } from '../../../state/app';
import { useFeature } from '../../../state/license';
import { useLastTab, usePagePref } from '../../../state/ui';
import { CommandBlock } from '../../connect/CommandBlock';
import {
  claudeCodeCommand,
  claudeDesktopPaths,
  CLI_DOWNLOAD_URL,
  cursorInstallLink,
  jsonConfig,
  maskToken,
  MCP_CLIENTS,
  MCP_DOCS_URL,
  MCP_TOOLS,
  mcpUrl,
  supportsTransport,
  vscodeInstallLink,
  type McpClientId,
  type McpTarget,
  type McpTransport,
} from '../mcp';
import { SettingRow, SettingsCard } from '../SettingRow';
import mcpStyles from './mcp.module.css';
import styles from './sections.module.css';

function ExternalLink({ href, children }: { href: string; children: ReactNode }) {
  return (
    <a
      href={href}
      target="_blank"
      rel="noreferrer"
      onClick={(e) => {
        e.preventDefault();
        openExternal(href);
      }}
    >
      {children}
    </a>
  );
}

/**
 * Settings → AI agents (MCP): copyable configurations for Claude Code, Claude Desktop, Cursor and
 * VS Code (stdio through `akev mcp`, or HTTP with the token), which tools need Pro, and the
 * safety switches.
 */
export function McpSection() {
  const apiUrl = useAppStore((s) => s.status?.apiUrl ?? null);
  const platform = useAppStore((s) => s.status?.platform);
  const desktop = useAppStore((s) => s.status?.desktop ?? false) || isDesktop();
  const [transport, setTransport] = usePagePref<McpTransport>('settings.mcp.transport', 'stdio');
  const [command, setCommand] = usePagePref<string>('settings.mcp.command', 'akev');
  const [client, setClient] = useLastTab('settings.mcp.client', 'claude-code');
  const [reveal, setReveal] = useState(false);

  const token = api.getConfig().token;
  const url = mcpUrl(apiUrl ?? window.location.origin);
  const exe = command.trim() || 'akev';
  const real: McpTarget = { command: exe, url, token };
  const shown: McpTarget = { ...real, token: token && !reveal ? maskToken(token) : token };

  return (
    <>
      <SettingsCard
        id="mcp"
        title="Connect an AI agent"
        description="Claude Code, Claude Desktop, Cursor, VS Code and other MCP clients can read captured traffic, search it, replay requests and add mocks."
      >
        <SettingRow
          id="mcp-transport"
          title="Connection"
          description={
            transport === 'stdio' ? (
              <>
                The client starts <code>akev mcp</code>, which connects to this Akev and reconnects when it restarts. Recommended.
              </>
            ) : (
              'The client talks to Akev over HTTP with the API token.'
            )
          }
          control={
            <SegmentedControl<McpTransport>
              value={transport}
              onValueChange={setTransport}
              items={[
                { value: 'stdio', label: 'akev mcp' },
                { value: 'http', label: 'HTTP' },
              ]}
              aria-label="Connection"
            />
          }
        >
          {transport === 'stdio' && desktop && (
            <div className={`${styles.callout} ${styles.calloutInfo}`}>
              <Info size={14} aria-hidden />
              <span>
                The desktop app doesn't install the <code>akev</code> command. <ExternalLink href={CLI_DOWNLOAD_URL}>Install the command-line tool</ExternalLink> too, then come back here.
              </span>
            </div>
          )}
          {transport === 'http' && (
            <div className={`${styles.callout} ${styles.calloutWarning}`}>
              <TriangleAlert size={14} aria-hidden />
              <span>
                {desktop
                  ? 'The desktop app picks a new port and token every time it starts, so this configuration stops working after a restart. Use akev mcp for one that keeps working.'
                  : 'The token changes every time Akev starts, unless you start it with --token or AKEV_TOKEN. Use akev mcp for a configuration that keeps working.'}
              </span>
            </div>
          )}
        </SettingRow>

        {transport === 'stdio' ? (
          <SettingRow
            id="mcp-command"
            title="Command"
            htmlFor="mcp-command-input"
            description={
              <>
                Apps started from a desktop menu may not see your shell's <code>PATH</code>. If a client can't start <code>akev</code>, enter its full path: <code>which akev</code> (macOS, Linux) or <code>where akev</code> (Windows) prints it.
              </>
            }
            control={<Input id="mcp-command-input" mono value={command} onChange={(e) => setCommand(e.target.value)} placeholder="akev" spellCheck={false} className={mcpStyles.commandInput} />}
          />
        ) : (
          <>
            <SettingRow
              id="mcp-endpoint"
              title="Endpoint"
              description="Streamable HTTP. Send the token as a bearer token."
              control={
                <span className={styles.inlineGroup}>
                  <span className={styles.addr}>{url}</span>
                  <CopyButton value={url} label="Copy endpoint" toastMessage="Endpoint copied" />
                </span>
              }
            />
            <SettingRow
              id="mcp-token"
              title="Token"
              description={
                token ? (
                  'Anyone with the token can read your traffic. Keep it out of shared files.'
                ) : (
                  <>
                    This window has no token. Open Akev from the link <code>akev start</code> prints.
                  </>
                )
              }
              control={
                token ? (
                  <span className={styles.inlineGroup}>
                    <span className={styles.addr} data-testid="mcp-token">
                      {reveal ? token : maskToken(token)}
                    </span>
                    <IconButton label={reveal ? 'Hide token' : 'Show token'} icon={reveal ? EyeOff : Eye} onClick={() => setReveal(!reveal)} />
                    <CopyButton value={token} label="Copy token" toastMessage="Token copied" />
                  </span>
                ) : (
                  '—'
                )
              }
            />
          </>
        )}

        <div className={mcpStyles.clients}>
          <Tabs value={client} onValueChange={setClient}>
            <TabList<McpClientId> items={MCP_CLIENTS.map((c) => ({ value: c.id, label: c.label }))} aria-label="MCP client" />
            <TabsContent value="claude-code" className={mcpStyles.panel}>
              <ClaudeCodePanel shown={shown} real={real} transport={transport} />
            </TabsContent>
            <TabsContent value="claude-desktop" className={mcpStyles.panel}>
              <ClaudeDesktopPanel shown={shown} real={real} transport={transport} platform={platform} />
            </TabsContent>
            <TabsContent value="cursor" className={mcpStyles.panel}>
              <JsonClientPanel client="cursor" shown={shown} real={real} transport={transport} desktop={desktop} />
            </TabsContent>
            <TabsContent value="vscode" className={mcpStyles.panel}>
              <JsonClientPanel client="vscode" shown={shown} real={real} transport={transport} desktop={desktop} />
            </TabsContent>
          </Tabs>
        </div>

        <p className={mcpStyles.docs}>
          More clients and details in the <ExternalLink href={MCP_DOCS_URL}>MCP guide</ExternalLink>.
        </p>
      </SettingsCard>

      <ToolsCard />
      <SafetyCard />
    </>
  );
}

interface PanelProps {
  shown: McpTarget;
  real: McpTarget;
  transport: McpTransport;
}

function ClaudeCodePanel({ shown, real, transport }: PanelProps) {
  const [allProjects, setAllProjects] = useState(false);
  const scope = allProjects ? 'user' : 'local';
  return (
    <>
      <p className={mcpStyles.step}>Run this in a terminal:</p>
      <CommandBlock code={claudeCodeCommand(shown, transport, scope)} copyValue={claudeCodeCommand(real, transport, scope)} copyToast="Command copied" aria-label="Claude Code command" />
      <Switch checked={allProjects} onCheckedChange={setAllProjects} size="sm" label="All my projects" description={allProjects ? 'Added with --scope user.' : 'Added to the current project, for you only.'} />
      <p className={styles.hint}>
        <code>claude mcp list</code> shows whether Claude Code can connect.
      </p>
    </>
  );
}

function ClaudeDesktopPanel({ shown, real, transport, platform }: PanelProps & { platform: Platform | undefined }) {
  if (!supportsTransport('claude-desktop', transport)) {
    return (
      <div className={`${styles.callout} ${styles.calloutInfo}`}>
        <Info size={14} aria-hidden />
        <span>Claude Desktop only starts local servers from its config file. Switch the connection to akev mcp.</span>
      </div>
    );
  }
  const paths = claudeDesktopPaths(platform);
  return (
    <>
      <p className={mcpStyles.step}>
        In Claude Desktop, open <strong>Settings → Developer → Edit Config</strong>, or edit the file directly:
      </p>
      <ul className={mcpStyles.paths}>
        {paths.map((p) => (
          <li key={p.platform}>
            <span className={mcpStyles.pathOs}>
              {p.label}
              {p.platform === platform && (
                <Badge variant="accent" size="sm">
                  This computer
                </Badge>
              )}
            </span>
            <span className={styles.inlineGroup}>
              <code className={styles.addr}>{p.path}</code>
              <CopyButton value={p.path} label={`Copy the ${p.label} path`} toastMessage="Path copied" />
            </span>
          </li>
        ))}
      </ul>
      <p className={mcpStyles.step}>Add Akev to it:</p>
      <CommandBlock code={jsonConfig('claude-desktop', shown, transport)} copyValue={jsonConfig('claude-desktop', real, transport)} copyToast="Configuration copied" wrap={false} aria-label="Claude Desktop configuration" />
      <p className={styles.hint}>Use the full path to akev in "command": Claude Desktop may not see your shell's PATH. On Windows, double the backslashes. Quit and reopen Claude Desktop to load the change.</p>
    </>
  );
}

function JsonClientPanel({ client, shown, real, transport, desktop }: PanelProps & { client: 'cursor' | 'vscode'; desktop: boolean }) {
  const cursor = client === 'cursor';
  const name = cursor ? 'Cursor' : 'VS Code';
  // The desktop app's webview only opens web links, so the install links are browser-only.
  const link = desktop ? null : cursor ? cursorInstallLink(real, transport) : vscodeInstallLink(real, transport);
  return (
    <>
      {link && (
        <div className={mcpStyles.install}>
          <Button
            variant="primary"
            icon={Download}
            onClick={() => {
              window.location.href = link;
            }}
          >
            Add to {name}
          </Button>
          <span className={styles.hint}>Opens {name}, which asks before adding Akev.</span>
        </div>
      )}
      <p className={mcpStyles.step}>
        {link ? 'Or add it by hand: save' : 'Save'} this as{' '}
        {cursor ? (
          <>
            <code>~/.cursor/mcp.json</code> for all your projects, or <code>.cursor/mcp.json</code> in a project:
          </>
        ) : (
          <>
            <code>.vscode/mcp.json</code> in your workspace (VS Code says <code>servers</code>, not <code>mcpServers</code>):
          </>
        )}
      </p>
      <CommandBlock code={jsonConfig(client, shown, transport)} copyValue={jsonConfig(client, real, transport)} copyToast="Configuration copied" wrap={false} aria-label={`${name} configuration`} />
    </>
  );
}

function ToolsCard() {
  const write = useFeature('mcp-write');
  const llm = useFeature('llm-lens');
  const entitled = (f: string | null) => f == null || (f === 'mcp-write' ? write : llm);
  return (
    <SettingsCard id="mcp-tools" title="Tools" description="Reading, searching and exporting are free. Tools that change something need Akev Pro or its trial; without it they return an error that names the feature.">
      {!write && <ProCallout feature="mcp-write" className={mcpStyles.callout} />}
      <table className={mcpStyles.tools}>
        <thead>
          <tr>
            <th scope="col">Tool</th>
            <th scope="col">What it does</th>
            <th scope="col">Plan</th>
          </tr>
        </thead>
        <tbody>
          {MCP_TOOLS.map((t) => (
            <tr key={t.name}>
              <td>
                <code className={mcpStyles.toolName}>{t.name}</code>
              </td>
              <td className={mcpStyles.toolPurpose}>{t.purpose}</td>
              <td className={mcpStyles.toolPlan}>
                {t.feature == null ? (
                  <span className={styles.muted}>Free</span>
                ) : entitled(t.feature) ? (
                  <Badge variant="success" size="sm">
                    Pro · active
                  </Badge>
                ) : (
                  <ProBadge always />
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </SettingsCard>
  );
}

function SafetyCard() {
  const dataDir = useAppStore((s) => s.status?.dataDir ?? null);
  const exportsDir = dataDir ? `${dataDir}${dataDir.includes('\\') ? '\\' : '/'}exports` : null;
  return (
    <SettingsCard id="mcp-safety" title="Safety" description="Agents read captured traffic, and a server can put instructions into it. Akev limits what an agent can do on this computer. These switches are options of the Akev you start, so they apply to every agent connected to it.">
      <SettingRow
        id="mcp-reveal-secrets"
        title="Secrets stay hidden"
        description={
          <>
            Agents see credential headers, cookies, secret query parameters and tokens in bodies as <code>[redacted]</code>. To let an agent see them when it asks, start Akev with <code>akev start --mcp-reveal-secrets</code>, or add <code>--reveal-secrets</code> to <code>akev mcp</code> when it runs its own Akev.
          </>
        }
      />
      <SettingRow
        id="mcp-allow-path"
        title="Files agents can use"
        description={
          <>
            HAR exports and the files an agent's rules serve stay in {exportsDir ? <code>{exportsDir}</code> : <>the <code>exports</code> folder of Akev's data directory</>}. To give agents another folder, start Akev with <code>--mcp-allow-path &lt;folder&gt;</code> (repeat it for more), or add <code>--allow-path &lt;folder&gt;</code> to <code>akev mcp</code> when it runs its own Akev. Rules you make yourself can use any file.
          </>
        }
      />
      <SettingRow
        id="mcp-allow-launch"
        title="Launching programs"
        description={
          <>
            Agents can't use the Electron interceptor, which starts any program, unless you opt in: start Akev with <code>akev start --mcp-allow-launch</code>, or add <code>--allow-launch</code> to <code>akev mcp</code> when it runs its own Akev. Without it, launch apps from the Connect page.
          </>
        }
      />
      <SettingRow
        id="mcp-approve"
        title="Approve tool calls yourself"
        description="Changing the rules, clearing traffic and launching interceptors are marked destructive, so clients ask before running them. Only approve calls you asked for, never because captured content asks you to."
        control={<ShieldAlert size={18} className={mcpStyles.shield} aria-hidden />}
      />
    </SettingsCard>
  );
}
