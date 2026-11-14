/**
 * Searchable index of individual settings (Settings → search box). Each entry's `id` matches
 * the `id` of its `SettingRow` (`#setting-<id>`). The sections themselves are in `sections.ts`.
 */

import { customTitleBarAvailable } from '../../lib/windowChrome';
import { rankByQuery } from '../connect/search';
import { SECTIONS, type SectionId } from './sections';

export interface SettingEntry {
  id: string;
  section: SectionId;
  title: string;
  keywords?: string;
  /** Only searchable where the setting is shown (e.g. desktop-only rows). */
  available?: () => boolean;
}

export const SETTINGS_INDEX: readonly SettingEntry[] = [
  { id: 'theme', section: 'general', title: 'Theme', keywords: 'dark light midnight oled high contrast appearance colour color mode' },
  { id: 'density', section: 'general', title: 'Density', keywords: 'compact comfortable row height spacing' },
  { id: 'scale', section: 'general', title: 'Text size', keywords: 'ui scale zoom font size bigger smaller' },
  { id: 'mono-paths', section: 'general', title: 'Monospace paths', keywords: 'font traffic list url' },
  { id: 'detail-position', section: 'general', title: 'Detail pane position', keywords: 'layout split right bottom' },
  { id: 'title-bar', section: 'general', title: 'Title bar', keywords: 'window decorations buttons close minimize maximize gnome wayland linux windows desktop', available: customTitleBarAvailable },
  { id: 'start-page', section: 'general', title: 'Start page', keywords: 'open launch default home' },
  { id: 'setup-guide', section: 'general', title: 'Setup guide', keywords: 'first run onboarding welcome getting started tour trust certificate launch browser' },
  { id: 'port', section: 'proxy', title: 'Proxy port', keywords: 'listen address 8000 rebind listening' },
  { id: 'lan', section: 'proxy', title: 'Allow other devices', keywords: 'lan network wifi bind 0.0.0.0 phone remote' },
  { id: 'bind', section: 'proxy', title: 'Bind address', keywords: 'listen interface ip 127.0.0.1' },
  { id: 'http2', section: 'proxy', title: 'HTTP/2', keywords: 'h2 alpn protocol' },
  { id: 'socks', section: 'proxy', title: 'SOCKS proxy', keywords: 'socks5 socks4 protocol' },
  { id: 'header-case', section: 'proxy', title: 'Preserve header case', keywords: 'headers casing http1' },
  { id: 'http3-upstream', section: 'proxy', title: 'HTTP/3 to servers', keywords: 'h3 quic alt-svc udp protocol prefer' },
  { id: 'http3-listen', section: 'proxy', title: 'Accept HTTP/3 from clients', keywords: 'h3 quic udp listener protocol' },
  { id: 'http3-port', section: 'proxy', title: 'QUIC port', keywords: 'h3 http/3 udp listener port' },
  { id: 'tls-passthrough', section: 'proxy', title: 'TLS passthrough', keywords: 'pinning bypass skip decrypt tunnel exclude hosts' },
  { id: 'tls-profile', section: 'proxy', title: 'Upstream TLS fingerprint', keywords: 'impersonate impersonation browser ja3 ja4 cloudflare datadome akamai bot blocked challenge chrome firefox safari edge boringssl' },
  { id: 'tls-profile-overrides', section: 'proxy', title: 'TLS fingerprint per host', keywords: 'impersonate override hosts chrome firefox safari edge' },
  { id: 'upstream', section: 'proxy', title: 'Upstream proxy', keywords: 'corporate parent chain pac system direct' },
  { id: 'no-proxy', section: 'proxy', title: 'Bypass upstream proxy for', keywords: 'no_proxy exclude hosts' },
  { id: 'dns-overrides', section: 'proxy', title: 'DNS overrides', keywords: 'hosts file resolve ip mapping' },
  { id: 'doh', section: 'proxy', title: 'DNS-over-HTTPS', keywords: 'doh resolver cloudflare google quad9 dns' },
  { id: 'connect-timeout', section: 'proxy', title: 'Connect timeout', keywords: 'connection seconds' },
  { id: 'response-timeout', section: 'proxy', title: 'Response timeout', keywords: 'headers wait seconds' },
  { id: 'body-idle-timeout', section: 'proxy', title: 'Body idle timeout', keywords: 'stream stall idle wait seconds download' },
  { id: 'max-paused', section: 'proxy', title: 'Max paused at once', keywords: 'breakpoints pause limit waiting requests' },
  { id: 'auto-release', section: 'proxy', title: 'Continue paused exchanges automatically', keywords: 'breakpoints auto release resume timeout seconds' },
  { id: 'transparent', section: 'proxy', title: 'Transparent capture', keywords: 'transparent listener redirect iptables nftables firewall linux per-app cgroup so_original_dst mark 8001' },
  { id: 'ca', section: 'certificates', title: 'CA certificate', keywords: 'fingerprint download pem der expiry root' },
  { id: 'regenerate', section: 'certificates', title: 'Regenerate CA', keywords: 'new certificate rotate reset' },
  { id: 'trust', section: 'certificates', title: 'Trust the CA', keywords: 'install keychain certificate store macos windows linux firefox android ios' },
  { id: 'client-certs', section: 'certificates', title: 'Client certificates', keywords: 'mtls mutual tls pkcs12 pfx p12' },
  { id: 'trusted-cas', section: 'certificates', title: 'Trusted CA certificates', keywords: 'upstream custom root internal corporate' },
  { id: 'ignore-cert-errors', section: 'certificates', title: 'Ignore certificate errors', keywords: 'insecure self-signed invalid tls expired' },
  { id: 'max-body', section: 'capture', title: 'Max body size', keywords: 'truncate limit large' },
  { id: 'memory', section: 'capture', title: 'Memory budget', keywords: 'ram spill disk limit' },
  { id: 'max-exchanges', section: 'capture', title: 'Max exchanges', keywords: 'limit history count oldest evict' },
  { id: 'rule-body-budget', section: 'capture', title: 'Memory for rule bodies', keywords: 'rules scripts breakpoints buffer large body budget ram limit' },
  { id: 'spill-max', section: 'capture', title: 'Disk space for bodies', keywords: 'spill disk limit storage gigabytes' },
  { id: 'event-queue', section: 'capture', title: 'Capture queue', keywords: 'overload backpressure dropped bodies queue memory' },
  { id: 'ignore-hosts', section: 'capture', title: 'Ignored hosts', keywords: 'exclude hide noise telemetry' },
  { id: 'record-tunnels', section: 'capture', title: 'Record tunnels', keywords: 'connect passthrough' },
  { id: 'record-tls-failures', section: 'capture', title: 'Record TLS failures', keywords: 'handshake errors pinning' },
  { id: 'detect-processes', section: 'capture', title: 'Detect processes', keywords: 'source app pid' },
  { id: 'proto-files', section: 'protobuf', title: 'Protobuf schemas', keywords: 'proto grpc grpc-web descriptor set protoc buf import upload message types decode' },
  { id: 'proto-mappings', section: 'protobuf', title: 'Protobuf URL mappings', keywords: 'proto message type url pattern decode octet-stream' },
  { id: 'mcp-transport', section: 'mcp', title: 'Connect an AI agent', keywords: 'mcp model context protocol claude code desktop cursor vs code vscode copilot agent llm ai stdio akev mcp' },
  { id: 'mcp-token', section: 'mcp', title: 'MCP token and endpoint', keywords: 'mcp http bearer token api url streamable' },
  { id: 'mcp-allow-launch', section: 'mcp', title: 'Let agents launch programs', keywords: 'mcp allow launch electron safety prompt injection' },
  { id: 'mcp-reveal-secrets', section: 'mcp', title: 'Secrets AI agents see', keywords: 'mcp reveal secrets redact redaction tokens cookies authorization agent' },
  { id: 'mcp-allow-path', section: 'mcp', title: 'Folders AI agents can use', keywords: 'mcp allow path folder files exports har map local agent' },
  { id: 'shortcuts', section: 'shortcuts', title: 'Keyboard shortcuts', keywords: 'keys hotkeys keymap rebind remap change customize bindings' },
  { id: 'export', section: 'data', title: 'Export configuration', keywords: 'backup settings rules json download' },
  { id: 'import', section: 'data', title: 'Import configuration', keywords: 'restore settings rules json upload http toolkit' },
  { id: 'reset-ui', section: 'data', title: 'Reset interface preferences', keywords: 'columns layout defaults' },
  { id: 'clear-traffic', section: 'data', title: 'Clear captured traffic', keywords: 'delete exchanges history' },
  { id: 'autosave', section: 'data', title: 'Autosave traffic', keywords: 'sessions save restart keep persist history crash' },
  { id: 'autosave-interval', section: 'data', title: 'Autosave interval', keywords: 'sessions save every seconds minutes' },
  { id: 'restore-on-startup', section: 'data', title: 'Restore last session on startup', keywords: 'sessions autosave reopen launch restart' },
  { id: 'autosave-size', section: 'data', title: 'Autosave size limit', keywords: 'sessions disk megabytes exchanges bound' },
  { id: 'saved-sessions', section: 'data', title: 'Saved sessions', keywords: 'sessions open rename delete charles mitmproxy library' },
  { id: 'license-plan', section: 'license', title: 'Plan', keywords: 'pro free upgrade buy purchase subscription license licence plan paid' },
  { id: 'license-key-input', section: 'license', title: 'License key', keywords: 'licence key activate enter paste register serial unlock pro' },
  { id: 'license-trial', section: 'license', title: 'Pro trial', keywords: 'trial free 14 days try pro evaluate' },
  { id: 'license-remove', section: 'license', title: 'Remove license', keywords: 'licence deactivate uninstall key free' },
  { id: 'license-features', section: 'license', title: 'Pro features', keywords: 'pro wireguard transparent jvm tls fingerprint script protobuf llm mcp unlock' },
  { id: 'version', section: 'about', title: 'Version', keywords: 'build release update' },
  { id: 'update-status', section: 'about', title: 'Check for updates', keywords: 'update upgrade new version release download' },
  { id: 'update-automatic', section: 'about', title: 'Check for updates automatically', keywords: 'update auto background privacy' },
  { id: 'trust-this-computer', section: 'certificates', title: 'Trust the CA on this computer', keywords: 'trust install keychain certificate store root ca system' },
  { id: 'data-dir', section: 'about', title: 'Data directory', keywords: 'folder path storage config' },
  { id: 'copy-diagnostics', section: 'about', title: 'Copy diagnostics', keywords: 'bug report support logs warnings crash debug info' },
  { id: 'report-problem', section: 'about', title: 'Report a problem', keywords: 'bug issue github feedback support crash' },
  { id: 'licenses', section: 'about', title: 'Open-source licenses', keywords: 'licences credits third party' },
];

export function searchSettings(query: string, limit = 8): SettingEntry[] {
  if (!query.trim()) return [];
  const label = (s: SectionId) => SECTIONS.find((x) => x.id === s)?.label ?? s;
  const entries = SETTINGS_INDEX.filter((e) => e.available?.() ?? true);
  return rankByQuery(entries, query, (e) => [
    { text: e.title, weight: 1 },
    { text: e.keywords ?? '', weight: 0.7 },
    { text: label(e.section), weight: 0.5 },
  ]).slice(0, limit);
}
