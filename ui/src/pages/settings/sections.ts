/**
 * The Settings sections, in nav order. Kept apart from the settings search index so the app
 * shell (palette commands) can list them without loading the Settings page.
 */

export type SectionId = 'general' | 'proxy' | 'certificates' | 'capture' | 'protobuf' | 'mcp' | 'shortcuts' | 'data' | 'license' | 'about';

export const SECTIONS: readonly { id: SectionId; label: string; description: string }[] = [
  { id: 'general', label: 'General', description: 'Appearance, layout and start page.' },
  { id: 'proxy', label: 'Proxy', description: 'Listening address, protocols (HTTP/2, HTTP/3), upstream proxy, DNS and timeouts. Changes apply immediately — no restart needed.' },
  { id: 'certificates', label: 'Certificates', description: 'The Akev certificate authority, how to trust it, and certificates for upstream servers.' },
  { id: 'capture', label: 'Capture', description: 'What Akev records and how much memory it may use.' },
  { id: 'protobuf', label: 'Protobuf schemas', description: 'Import .proto files or descriptor sets to decode protobuf, gRPC and gRPC-Web bodies with real field and message names.' },
  { id: 'mcp', label: 'AI agents (MCP)', description: 'Connect Claude Code, Claude Desktop, Cursor, VS Code and other MCP clients so they can read, replay and mock your traffic.' },
  { id: 'shortcuts', label: 'Shortcuts', description: 'Every keyboard shortcut in Akev, and the keys you changed. Press ? anywhere to see the ones available on the current page.' },
  { id: 'data', label: 'Data', description: 'Saved sessions and autosave, moving your configuration between machines, resetting preferences and clearing captured traffic.' },
  { id: 'license', label: 'License', description: 'Your plan (Free or Pro), the license key, the 14-day trial and what Akev Pro includes.' },
  { id: 'about', label: 'About', description: 'Version, environment and open-source licenses.' },
];

export function isSectionId(v: string | undefined): v is SectionId {
  return SECTIONS.some((s) => s.id === v);
}
