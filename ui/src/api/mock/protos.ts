/**
 * Mock protobuf schema registry (`/api/protos*`) and typed decoding of the mock's gRPC /
 * protobuf bodies. `.proto` files are "compiled" by a small line-based parser that finds
 * packages, messages (with their fields), enums, services and the errors the UI needs to show
 * (unknown field types, unbalanced braces) — enough to exercise the UI, not a real compiler.
 */

import { base64ToBytes } from '../../lib/encoding';
import { grpcCallFromPath, mappingMatchesUrl, validateMappingPattern, validateMessageTypeName } from '../../lib/protobuf';
import type { BodyView, ExchangeSummary, GrpcMessage, Headers, ProtoFileInfo, ProtoMapping, ProtoRegistry, ProtoUploadFile, SchemaInfo, Side } from '../types';

const SCALARS = new Set(['double', 'float', 'int32', 'int64', 'uint32', 'uint64', 'sint32', 'sint64', 'fixed32', 'fixed64', 'sfixed32', 'sfixed64', 'bool', 'string', 'bytes']);

interface Field {
  number: number;
  name: string;
  type: string;
  line: number;
  column: number;
}

interface Parsed {
  info: ProtoFileInfo;
  messages: Map<string, Field[]>;
  enums: Map<string, Map<number, string>>;
}

const SEED: readonly { name: string; content: string }[] = [
  {
    name: 'helloworld/helloworld.proto',
    content: `syntax = "proto3";

package helloworld;

// The greeting service definition.
service Greeter {
  rpc SayHello (HelloRequest) returns (HelloReply);
  rpc SayHelloStream (HelloRequest) returns (stream HelloReply);
}

message HelloRequest {
  string name = 1;
}

message HelloReply {
  enum Status {
    STATUS_UNSPECIFIED = 0;
    OK = 1;
    DEGRADED = 2;
  }
  string server = 1;
  int32 latency_ms = 2;
  string message = 3;
  Status status = 4;
}
`,
  },
  {
    name: 'telemetry/v1/trace.proto',
    content: `syntax = "proto3";

package telemetry.v1;

enum Sampling {
  SAMPLING_UNSPECIFIED = 0;
  ALWAYS = 1;
  RATIO = 2;
}

message ExportRequest {
  string service_name = 1;
  int32 span_count = 2;
  string note = 3;
  Sampling sampling = 4;
}

message ExportResponse {
  int64 accepted_spans = 1;
}
`,
  },
  {
    name: 'legacy/billing.proto',
    content: `syntax = "proto3";

package billing;

message Invoice {
  strin id = 1;
  int64 amount_cents = 2;
}
`,
  },
];

const SEED_MAPPINGS: readonly ProtoMapping[] = [{ id: 'm-telemetry', enabled: true, pattern: 'telemetry.example.io/v1/traces', side: 'request', messageType: 'telemetry.v1.ExportRequest' }];

/** Parse one `.proto` source. */
function parseProto(name: string, content: string, addedAt: number): Parsed {
  const info: ProtoFileInfo = { name, kind: 'proto', size: new TextEncoder().encode(content).length, addedAt, services: [], messages: [], enums: [], errors: [] };
  const messages = new Map<string, Field[]>();
  const enums = new Map<string, Map<number, string>>();
  const pkg = /^\s*package\s+([\w.]+)\s*;/m.exec(content)?.[1] ?? '';
  if (pkg) info.package = pkg;
  const scopes: { kind: 'message' | 'enum' | 'service' | 'oneof' | 'other'; name: string }[] = [];
  const prefix = () => {
    const names = scopes.filter((s) => s.kind === 'message' || s.kind === 'enum').map((s) => s.name);
    return [pkg, ...names].filter(Boolean).join('.');
  };
  const current = (kinds: string[]) => [...scopes].reverse().find((s) => kinds.includes(s.kind));
  const lines = content.split('\n');
  lines.forEach((raw, i) => {
    const line = raw.replace(/\/\/.*$/, '');
    const open = /^\s*(message|enum|service|oneof)\s+(\w+)\s*\{/.exec(line);
    if (open) {
      const kind = open[1] as 'message' | 'enum' | 'service' | 'oneof';
      const full = [prefix(), open[2]].filter(Boolean).join('.');
      scopes.push({ kind, name: open[2] ?? '' });
      if (kind === 'message') {
        messages.set(full, []);
        info.messages.push(full);
      } else if (kind === 'enum') {
        enums.set(full, new Map());
        info.enums.push(full);
      } else if (kind === 'service') info.services.push({ name: full, methods: [] });
      // A one-line block (`message Empty {}`) closes right away.
      for (const ch of line.slice(line.indexOf('{'))) if (ch === '}') scopes.pop();
      return;
    }
    const rpc = /^\s*rpc\s+(\w+)\s*\(\s*(stream\s+)?([\w.]+)\s*\)\s*returns\s*\(\s*(stream\s+)?([\w.]+)\s*\)/.exec(line);
    const service = info.services[info.services.length - 1];
    if (rpc && service && current(['service'])) {
      const q = (t: string) => (t.includes('.') ? t : [pkg, t].filter(Boolean).join('.'));
      service.methods.push({ name: rpc[1] ?? '', path: `/${service.name}/${rpc[1]}`, inputType: q(rpc[3] ?? ''), outputType: q(rpc[5] ?? ''), clientStreaming: !!rpc[2], serverStreaming: !!rpc[4] });
      return;
    }
    const scope = current(['message', 'enum']);
    if (scope?.kind === 'message') {
      const field = /^(\s*(?:repeated\s+|optional\s+)?)(map<[^>]+>|[\w.]+)\s+(\w+)\s*=\s*(\d+)/.exec(line);
      if (field) {
        const fields = messages.get(prefix());
        fields?.push({ number: Number(field[4]), name: field[3] ?? '', type: field[2] ?? '', line: i + 1, column: (field[1]?.length ?? 0) + 1 });
      }
    } else if (scope?.kind === 'enum') {
      const value = /^\s*(\w+)\s*=\s*(-?\d+)/.exec(line);
      if (value) enums.get(prefix())?.set(Number(value[2]), value[1] ?? '');
    }
    for (const ch of line) {
      if (ch === '}') scopes.pop();
    }
  });
  if (scopes.length) info.errors.push({ file: name, line: lines.length, column: 1, message: "unexpected end of file, expected '}'" });
  // Unknown field types.
  const known = new Set([...messages.keys(), ...enums.keys()]);
  for (const fields of messages.values()) {
    for (const f of fields) {
      if (SCALARS.has(f.type) || f.type.startsWith('map<') || f.type.startsWith('google.protobuf.')) continue;
      if (![...known].some((k) => k === f.type || k.endsWith(`.${f.type}`))) info.errors.push({ file: name, line: f.line, column: f.column, message: `name '${f.type}' is not defined` });
    }
  }
  if (info.errors.length) {
    info.services = [];
    info.messages = [];
    info.enums = [];
    messages.clear();
    enums.clear();
  }
  info.messages.sort();
  info.enums.sort();
  return { info, messages, enums };
}

export class MockProtoRegistry {
  private files = new Map<string, Parsed>();
  private sources = new Map<string, string>();
  mappings: ProtoMapping[] = SEED_MAPPINGS.map((m) => ({ ...m }));

  constructor(now: number) {
    for (const f of SEED) this.add([{ name: f.name, content: f.content }], now - 86_400_000);
  }

  info(): ProtoRegistry {
    const files = [...this.files.values()].map((p) => p.info).sort((a, b) => a.name.localeCompare(b.name));
    const messageTypes = [...new Set(files.flatMap((f) => f.messages))].sort();
    return { files, mappings: this.mappings.map((m) => ({ ...m })), messageTypes };
  }

  add(upload: ProtoUploadFile[], now = Date.now()): ProtoRegistry {
    if (!upload.length) throw new Error('no files given');
    for (const f of upload) {
      if (!f.name || f.name.split('/').some((s) => !s || s.startsWith('.'))) throw new Error(`invalid file name "${f.name}": use a relative path like \`shop/v1/catalog.proto\``);
      if (f.content != null) {
        this.sources.set(f.name, f.content);
        this.files.set(f.name, parseProto(f.name, f.content, now));
      } else {
        const bytes = base64ToBytes(f.base64 ?? '');
        const info: ProtoFileInfo = { name: f.name, kind: 'descriptor-set', size: bytes.length, addedAt: now, services: [], messages: [], enums: [], errors: [] };
        if (bytes.length === 0 || bytes[0] !== 0x0a) info.errors.push({ file: f.name, line: null, column: null, message: 'not a protobuf descriptor set (FileDescriptorSet): invalid wire type' });
        else info.contains = [f.name.replace(/\.[^.]+$/, '.proto')];
        this.files.set(f.name, { info, messages: new Map(), enums: new Map() });
      }
    }
    return this.info();
  }

  remove(name: string | null): ProtoRegistry {
    if (name == null) {
      this.files.clear();
      this.sources.clear();
    } else {
      if (!this.files.has(name)) throw Object.assign(new Error(`no schema file named "${name}"`), { notFound: true });
      this.files.delete(name);
      this.sources.delete(name);
    }
    return this.info();
  }

  source(name: string): string | null {
    return this.sources.get(name) ?? null;
  }

  setMappings(next: ProtoMapping[]): ProtoMapping[] {
    if (!Array.isArray(next)) throw new Error('expected a list of mappings');
    this.mappings = next.map((m, i) => {
      const pattern = (m.pattern ?? '').trim();
      const messageType = (m.messageType ?? '').trim().replace(/^\./, '');
      if (validateMappingPattern(pattern)) throw new Error(`mapping ${i + 1}: the URL pattern is empty`);
      if (validateMessageTypeName(messageType)) throw new Error(`mapping ${i + 1}: "${messageType}" is not a message type name (e.g. \`shop.v1.Item\`)`);
      return { id: m.id || crypto.randomUUID().replace(/-/g, ''), enabled: m.enabled ?? true, pattern, side: m.side ?? 'both', messageType };
    });
    return this.mappings.map((m) => ({ ...m }));
  }

  private findMessage(name: string): { fields: Field[]; parsed: Parsed } | null {
    for (const parsed of this.files.values()) {
      const fields = parsed.messages.get(name);
      if (fields) return { fields, parsed };
    }
    return null;
  }

  private resolve(type: string, scope: string): { kind: 'message'; name: string } | { kind: 'enum'; values: Map<number, string> } | null {
    const candidates = type.includes('.') ? [type] : [`${scope}.${type}`, ...[scope.split('.').slice(0, -1).join('.')].map((p) => (p ? `${p}.${type}` : type)), type];
    for (const c of candidates) {
      if (this.findMessage(c)) return { kind: 'message', name: c };
      for (const parsed of this.files.values()) {
        const values = parsed.enums.get(c);
        if (values) return { kind: 'enum', values };
      }
    }
    return null;
  }

  /** Schemaless JSON (`{"1": …}`) → typed JSON for `message`. */
  private typed(message: string, value: unknown, unknown: string[], path = ''): unknown {
    const def = this.findMessage(message);
    if (!def || value == null || typeof value !== 'object' || Array.isArray(value)) return value;
    const out: Record<string, unknown> = {};
    for (const [key, v] of Object.entries(value as Record<string, unknown>)) {
      const field = def.fields.find((f) => String(f.number) === key);
      if (!field) {
        unknown.push(path ? `${path}.${key}` : key);
        out[key] = v;
        continue;
      }
      const target = this.resolve(field.type, message);
      const convert = (x: unknown) => (target?.kind === 'enum' && typeof x === 'number' ? (target.values.get(x) ?? x) : target?.kind === 'message' ? this.typed(target.name, x, unknown, path ? `${path}.${field.name}` : field.name) : x);
      out[field.name] = Array.isArray(v) ? v.map(convert) : convert(v);
    }
    return out;
  }

  /** Add the typed decode the daemon would produce to a mock body view. */
  decorate(view: BodyView, ctx: { side: Side; url: string; path: string; contentType: string | null }): BodyView {
    if (view.size === 0 || view.structured == null) return view;
    let schema: SchemaInfo | null = null;
    if (view.kind === 'grpc' || view.kind === 'grpc-web') {
      const call = grpcCallFromPath(ctx.path);
      const method = call ? this.info().files.flatMap((f) => f.services).find((s) => s.name === call.service)?.methods.find((m) => m.name === call.method) : undefined;
      if (method && call) {
        const messageType = ctx.side === 'request' ? method.inputType : method.outputType;
        schema = { messageType, source: 'grpc', method: { service: call.service, method: method.name, inputType: method.inputType, outputType: method.outputType, clientStreaming: method.clientStreaming, serverStreaming: method.serverStreaming } };
      }
    }
    if (!schema) {
      const mapping = this.mappings.find((m) => m.enabled && (m.side === 'both' || m.side === ctx.side) && mappingMatchesUrl(m.pattern, ctx.url) && this.findMessage(m.messageType));
      if (mapping) schema = { messageType: mapping.messageType, source: 'mapping', mapping: mapping.pattern };
    }
    if (!schema) return view;
    if (view.kind === 'grpc' || view.kind === 'grpc-web') {
      const s = view.structured as { messages?: (GrpcMessage & { size?: number })[] };
      const messages = (s.messages ?? []).map((m) => {
        if (m.trailers || m.decoded == null) return m;
        const unknown: string[] = [];
        const typed = this.typed(schema.messageType, m.decoded, unknown);
        return { ...m, typed, ...(unknown.length ? { unknownFields: unknown } : {}) };
      });
      return { ...view, structured: { ...s, messages }, schema };
    }
    if (view.kind === 'protobuf') {
      const unknown: string[] = [];
      const typed = this.typed(schema.messageType, view.structured, unknown);
      return { ...view, schema: { ...schema, typed, ...(unknown.length ? { unknownFields: unknown } : {}) } };
    }
    return view;
  }
}

/** HTTP/2 trailers of a mock gRPC response: mostly OK, some NOT_FOUND. */
export function mockGrpcTrailers(summary: ExchangeSummary): Headers | undefined {
  if ((summary.contentKind !== 'grpc' && summary.contentKind !== 'grpc-web') || summary.status == null) return undefined;
  return summary.id % 5 === 0
    ? [
        ['grpc-status', '5'],
        ['grpc-message', 'greeter%3A%20no%20such%20person'],
      ]
    : [
        ['grpc-status', '0'],
        ['grpc-message', ''],
      ];
}
