import { describe, expect, test } from 'bun:test';
import type { ProtoFileInfo, ProtoRegistry, SchemaInfo } from '../api/types';
import { base64ToBytes, utf8Encode } from './encoding';
import {
  countMatches,
  fileMatches,
  grpcCallFromPath,
  isGrpcContentType,
  mappingMatchesUrl,
  mappingsValid,
  mappingTypeWarning,
  normalizeMappings,
  normalizeProtoName,
  protoErrorLocation,
  schemaSourceText,
  shortTypeName,
  stripCommonRoot,
  summarizeProtoFile,
  summarizeRegistry,
  summaryUrl,
  toUploadFiles,
  unknownFieldsText,
  validateMappingPattern,
  validateMessageTypeName,
  wildcardMatch,
} from './protobuf';

const file = (over: Partial<ProtoFileInfo> = {}): ProtoFileInfo => ({
  name: 'shop/v1/catalog.proto',
  kind: 'proto',
  size: 420,
  addedAt: 1,
  package: 'shop.v1',
  services: [{ name: 'shop.v1.Catalog', methods: [{ name: 'GetItem', path: '/shop.v1.Catalog/GetItem', inputType: 'shop.v1.GetItemRequest', outputType: 'shop.v1.Item', clientStreaming: false, serverStreaming: false }, { name: 'ListItems', path: '/shop.v1.Catalog/ListItems', inputType: 'shop.v1.ListItemsRequest', outputType: 'shop.v1.Item', clientStreaming: false, serverStreaming: true }] }],
  messages: ['shop.v1.GetItemRequest', 'shop.v1.Item', 'shop.v1.Item.Variant'],
  enums: ['shop.v1.Item.Status'],
  errors: [],
  ...over,
});

describe('gRPC calls', () => {
  test('paths name service and method', () => {
    expect(grpcCallFromPath('/shop.v1.Catalog/GetItem')).toEqual({ service: 'shop.v1.Catalog', shortService: 'Catalog', package: 'shop.v1', method: 'GetItem' });
    expect(grpcCallFromPath('/api/grpc/helloworld.Greeter/SayHello?x=1')?.service).toBe('helloworld.Greeter');
    expect(grpcCallFromPath('/Greeter/SayHello')).toEqual({ service: 'Greeter', shortService: 'Greeter', package: '', method: 'SayHello' });
    for (const bad of ['', '/', '/GetItem', '/shop.v1.Catalog/', '/a b/C', '/.a/B', '/a./B', '/a/b.c', null, undefined]) expect(grpcCallFromPath(bad)).toBeNull();
  });

  test('content types', () => {
    expect(isGrpcContentType('application/grpc')).toBe(true);
    expect(isGrpcContentType(' Application/gRPC-Web+proto')).toBe(true);
    expect(isGrpcContentType('application/x-protobuf')).toBe(false);
    expect(isGrpcContentType(null)).toBe(false);
  });
});

describe('schema labels', () => {
  test('type names and sources', () => {
    expect(shortTypeName('shop.v1.Item')).toBe('Item');
    expect(shortTypeName('Item')).toBe('Item');
    const base: SchemaInfo = { messageType: 'shop.v1.Item', source: 'grpc' };
    expect(schemaSourceText({ ...base, method: { service: 'shop.v1.Catalog', method: 'GetItem', inputType: 'a', outputType: 'b', clientStreaming: false, serverStreaming: false } })).toBe('gRPC method shop.v1.Catalog/GetItem');
    expect(schemaSourceText({ ...base, source: 'content-type' })).toBe('the Content-Type header');
    expect(schemaSourceText({ ...base, source: 'mapping', mapping: '/items/*' })).toBe('the mapping /items/*');
  });

  test('unknown fields', () => {
    expect(unknownFieldsText(undefined)).toBeNull();
    expect(unknownFieldsText([])).toBeNull();
    expect(unknownFieldsText(['21', 'related.99'])).toBe('21, related.99');
    expect(unknownFieldsText(['1', '2', '3', '4', '5', '6'], 4)).toBe('1, 2, 3, 4 and 2 more');
  });
});

describe('uploads', () => {
  test('names are normalized like the daemon expects', () => {
    expect(normalizeProtoName('./shop\\v1/catalog.proto')).toBe('shop/v1/catalog.proto');
    expect(normalizeProtoName('/abs/../x.proto')).toBe('abs/x.proto');
    expect(normalizeProtoName('my file (1).proto')).toBe('my_file__1_.proto');
    expect(normalizeProtoName('.hidden.proto')).toBe('_hidden.proto');
  });

  test('sources are sent as text, descriptor sets as base64, other files skipped', () => {
    const { upload, skipped } = toUploadFiles([
      { name: 'catalog.proto', data: utf8Encode('syntax = "proto3";') },
      { name: 'api.desc', data: new Uint8Array([0x0a, 0x00]) },
      { name: 'README.md', data: utf8Encode('# hi') },
    ]);
    expect(upload).toEqual([
      { name: 'catalog.proto', content: 'syntax = "proto3";' },
      { name: 'api.desc', base64: 'CgA=' },
    ]);
    expect(base64ToBytes(upload[1]?.base64 ?? '')).toEqual(new Uint8Array([0x0a, 0x00]));
    expect(skipped).toEqual(['README.md']);
  });
});

describe('files', () => {
  test('summaries', () => {
    expect(summarizeProtoFile(file()).text).toBe('1 service · 2 methods · 3 messages · 1 enum');
    expect(summarizeProtoFile(file({ services: [], messages: [], enums: [] })).text).toBe('No definitions');
    expect(summarizeProtoFile(file({ services: [], messages: [], enums: [], errors: [{ file: 'x', line: 1, column: 1, message: 'm' }] })).text).toBe('Not compiled');
    const registry: ProtoRegistry = { files: [file(), file({ name: 'bad.proto', errors: [{ file: 'bad.proto', line: 4, column: 3, message: 'x' }] })], mappings: [], messageTypes: ['a.B', 'c.D'] };
    expect(summarizeRegistry(registry)).toEqual({ files: 2, failing: 1, services: 2, messages: 2 });
    expect(summarizeRegistry(null)).toEqual({ files: 0, failing: 0, services: 0, messages: 0 });
  });

  test('error locations', () => {
    expect(protoErrorLocation({ file: 'a.proto', line: 4, column: 3, message: '' })).toBe('a.proto:4:3');
    expect(protoErrorLocation({ file: 'a.proto', line: 4, column: null, message: '' })).toBe('a.proto:4');
    expect(protoErrorLocation({ file: 'a.desc', line: null, column: null, message: '' })).toBe('a.desc');
  });

  test('search', () => {
    expect(fileMatches(file(), '')).toBe(true);
    expect(fileMatches(file(), 'listitems')).toBe(true);
    expect(fileMatches(file(), 'Variant')).toBe(true);
    expect(fileMatches(file(), 'status')).toBe(true);
    expect(fileMatches(file(), 'nope')).toBe(false);
  });
});

describe('mappings', () => {
  test('validation', () => {
    expect(validateMappingPattern('')).not.toBeNull();
    expect(validateMappingPattern('a b')).not.toBeNull();
    expect(validateMappingPattern('api.example.com/*')).toBeNull();
    expect(validateMessageTypeName('')).not.toBeNull();
    expect(validateMessageTypeName('shop..Item')).not.toBeNull();
    expect(validateMessageTypeName('1shop.Item')).not.toBeNull();
    expect(validateMessageTypeName('.shop.v1.Item')).toBeNull();
    expect(mappingTypeWarning('shop.v1.Item', ['shop.v1.Item'])).toBeNull();
    expect(mappingTypeWarning('shop.v1.Missing', ['shop.v1.Item'])).toContain("isn't defined");
    expect(mappingTypeWarning('', [])).toBeNull();
    const ok = { id: '', enabled: true, pattern: ' /a ', side: 'both' as const, messageType: '.a.B' };
    expect(mappingsValid([ok])).toBe(true);
    expect(mappingsValid([ok, { ...ok, pattern: '' }])).toBe(false);
    expect(normalizeMappings([ok])).toEqual([{ ...ok, pattern: '/a', messageType: 'a.B' }]);
  });

  test('wildcards', () => {
    expect(wildcardMatch('*', '')).toBe(true);
    expect(wildcardMatch('a*c', 'ABBC')).toBe(true);
    expect(wildcardMatch('a*c', 'acb')).toBe(false);
    expect(wildcardMatch('a**b', 'ab')).toBe(true);
    expect(wildcardMatch('?x', 'ax')).toBe(false);
  });

  test('pattern forms match like the daemon', () => {
    const url = 'https://API.example.com/v1/items/42?fields=name';
    for (const yes of ['/v1/items/*', '/v1/items/42?fields=name', 'https://api.example.com/v1/*', '*://api.example.com/*', 'api.example.com/v1/items/*', '*.example.com', 'api.example.com']) expect(mappingMatchesUrl(yes, url)).toBe(true);
    for (const no of ['/v1/items', '/v2/*', 'http://api.example.com/*', 'other.example.com/*', 'example.com', 'api.example.com/v1']) expect(mappingMatchesUrl(no, url)).toBe(false);
    expect(mappingMatchesUrl('localhost:8080/rpc', 'http://localhost:8080/rpc')).toBe(true);
    expect(mappingMatchesUrl('localhost', 'http://localhost:8080/rpc')).toBe(true);
    expect(mappingMatchesUrl('localhost/rpc', 'http://localhost:8080/rpc')).toBe(false);
  });
});

describe('traffic matching', () => {
  const s = (scheme: string, host: string, port: number, path: string) => ({ scheme: scheme as 'https', host, port, path });
  test('summary URLs', () => {
    expect(summaryUrl(s('https', 'api.example.com', 443, '/v1/items?x=1'))).toBe('https://api.example.com/v1/items?x=1');
    expect(summaryUrl(s('http', 'localhost', 3000, '/rpc'))).toBe('http://localhost:3000/rpc');
    expect(summaryUrl(s('wss', '::1', 443, '/ws'))).toBe('https://[::1]/ws');
  });
  test('counts', () => {
    const list = [s('https', 'api.example.com', 443, '/v1/items/1'), s('https', 'api.example.com', 443, '/v1/users/1'), s('http', 'localhost', 3000, '/v1/items/2')];
    expect(countMatches('/v1/items/*', list)).toBe(2);
    expect(countMatches('api.example.com/*', list)).toBe(2);
    expect(countMatches('localhost:3000/*', list)).toBe(1);
    expect(countMatches('', list)).toBe(0);
  });
  test('folder drops lose their common root', () => {
    expect(stripCommonRoot(['protos/a.proto', 'protos/shop/b.proto'])).toEqual(['a.proto', 'shop/b.proto']);
    expect(stripCommonRoot(['a.proto', 'protos/b.proto'])).toEqual(['a.proto', 'protos/b.proto']);
    expect(stripCommonRoot(['x/a.proto', 'y/b.proto'])).toEqual(['x/a.proto', 'y/b.proto']);
    expect(stripCommonRoot([])).toEqual([]);
  });
});
