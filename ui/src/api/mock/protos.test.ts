import { describe, expect, test } from 'bun:test';
import type { BodyView, GrpcMessage } from '../types';
import { MockProtoRegistry, mockGrpcTrailers } from './protos';

const view = (over: Partial<BodyView>): BodyView => ({ kind: 'grpc', size: 10, wireSize: 10, encodings: [], text: null, formatted: null, structured: null, decodeError: null, truncated: false, captureTruncated: false, ...over });

describe('mock protobuf registry', () => {
  test('seeded files compile, with services, enums and a line-numbered error', () => {
    const reg = new MockProtoRegistry(Date.now());
    const info = reg.info();
    expect(info.files.map((f) => f.name)).toEqual(['helloworld/helloworld.proto', 'legacy/billing.proto', 'telemetry/v1/trace.proto']);
    const hello = info.files[0];
    expect(hello?.package).toBe('helloworld');
    expect(hello?.services[0]?.methods.map((m) => [m.name, m.inputType, m.outputType, m.serverStreaming])).toEqual([
      ['SayHello', 'helloworld.HelloRequest', 'helloworld.HelloReply', false],
      ['SayHelloStream', 'helloworld.HelloRequest', 'helloworld.HelloReply', true],
    ]);
    expect(hello?.enums).toEqual(['helloworld.HelloReply.Status']);
    expect(info.files[1]?.errors).toEqual([{ file: 'legacy/billing.proto', line: 6, column: 3, message: "name 'strin' is not defined" }]);
    expect(info.messageTypes).toContain('telemetry.v1.ExportRequest');
    expect(reg.source('helloworld/helloworld.proto')).toContain('service Greeter');
  });

  test('uploads, unbalanced braces, descriptor sets and removal', () => {
    const reg = new MockProtoRegistry(Date.now());
    let info = reg.add([{ name: 'x/a.proto', content: 'syntax = "proto3";\npackage a;\nmessage A {\n  string s = 1;\n' }]);
    expect(info.files.find((f) => f.name === 'x/a.proto')?.errors[0]?.message).toContain("expected '}'");
    info = reg.add([{ name: 'x/a.proto', content: 'syntax = "proto3";\npackage a;\nmessage A { string s = 1; }\nmessage Empty {}\n' }]);
    expect(info.files.find((f) => f.name === 'x/a.proto')).toMatchObject({ errors: [], messages: ['a.A', 'a.Empty'] });
    info = reg.add([{ name: 'set.desc', base64: 'CgA=' }]);
    expect(info.files.find((f) => f.name === 'set.desc')?.kind).toBe('descriptor-set');
    info = reg.add([{ name: 'bad.desc', base64: '/w==' }]);
    expect(info.files.find((f) => f.name === 'bad.desc')?.errors.length).toBe(1);
    expect(() => reg.add([{ name: '../x.proto', content: '' }])).toThrow();
    expect(() => reg.remove('nope.proto')).toThrow();
    expect(reg.remove('x/a.proto').files.some((f) => f.name === 'x/a.proto')).toBe(false);
    expect(reg.remove(null).files).toEqual([]);
  });

  test('mappings are validated and normalized', () => {
    const reg = new MockProtoRegistry(Date.now());
    const saved = reg.setMappings([{ id: '', enabled: true, pattern: ' /items/* ', side: 'response', messageType: '.shop.Item' }]);
    expect(saved[0]).toMatchObject({ pattern: '/items/*', messageType: 'shop.Item' });
    expect(saved[0]?.id).not.toBe('');
    expect(() => reg.setMappings([{ id: '', enabled: true, pattern: '', side: 'both', messageType: 'a.B' }])).toThrow();
    expect(() => reg.setMappings([{ id: '', enabled: true, pattern: '/x', side: 'both', messageType: 'not a type' }])).toThrow();
  });

  test('gRPC bodies get typed messages from the method named by the path', () => {
    const reg = new MockProtoRegistry(Date.now());
    const grpc = view({ structured: { messages: [{ compressed: false, decoded: { '1': 'akev', '2': 7, '3': 'Hello', '4': 1, '9': 'x' }, trailers: null, size: 20 }, { compressed: false, decoded: null, trailers: [['grpc-status', '0']] }] } });
    const out = reg.decorate(grpc, { side: 'response', url: 'https://grpc.example.com/helloworld.Greeter/SayHello', path: '/helloworld.Greeter/SayHello', contentType: 'application/grpc' });
    expect(out.schema).toMatchObject({ messageType: 'helloworld.HelloReply', source: 'grpc', method: { service: 'helloworld.Greeter', method: 'SayHello' } });
    const messages = (out.structured as { messages: GrpcMessage[] }).messages;
    expect(messages[0]?.typed).toEqual({ server: 'akev', latency_ms: 7, message: 'Hello', status: 'OK', '9': 'x' });
    expect(messages[0]?.unknownFields).toEqual(['9']);
    expect(messages[1]?.typed).toBeUndefined();
    // Requests decode as the input type; unknown methods aren't decorated.
    const req = reg.decorate(view({ structured: { messages: [{ compressed: false, decoded: { '1': 'Ada' }, trailers: null }] } }), { side: 'request', url: '', path: '/helloworld.Greeter/SayHello', contentType: 'application/grpc' });
    expect((req.structured as { messages: GrpcMessage[] }).messages[0]?.typed).toEqual({ name: 'Ada' });
    const unknown = view({ structured: { messages: [] } });
    expect(reg.decorate(unknown, { side: 'request', url: '', path: '/other.Svc/M', contentType: 'application/grpc' })).toBe(unknown);
  });

  test('protobuf bodies use mappings', () => {
    const reg = new MockProtoRegistry(Date.now());
    const pb = view({ kind: 'protobuf', structured: { '1': 'svc', '2': 3, '4': 2 } });
    const out = reg.decorate(pb, { side: 'request', url: 'https://telemetry.example.io/v1/traces', path: '/v1/traces', contentType: 'application/x-protobuf' });
    expect(out.schema).toMatchObject({ source: 'mapping', mapping: 'telemetry.example.io/v1/traces', typed: { service_name: 'svc', span_count: 3, sampling: 'RATIO' } });
    // The mapping is for requests only.
    expect(reg.decorate(pb, { side: 'response', url: 'https://telemetry.example.io/v1/traces', path: '/v1/traces', contentType: null }).schema).toBeUndefined();
  });

  test('gRPC trailers', () => {
    const base = { id: 1, kind: 'http', state: 'complete', startedAt: 0, method: 'POST', scheme: 'https', host: 'g', port: 443, path: '/a.B/C', httpVersion: 'HTTP/2', status: 200, contentKind: 'grpc', requestSize: 0, responseSize: 0 } as const;
    expect(mockGrpcTrailers({ ...base })).toEqual([
      ['grpc-status', '0'],
      ['grpc-message', ''],
    ]);
    expect(mockGrpcTrailers({ ...base, id: 10 })?.[0]).toEqual(['grpc-status', '5']);
    expect(mockGrpcTrailers({ ...base, contentKind: 'json' })).toBeUndefined();
    expect(mockGrpcTrailers({ ...base, status: undefined })).toBeUndefined();
  });
});
