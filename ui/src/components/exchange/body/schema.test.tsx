import { describe, expect, test } from 'bun:test';
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { BodyView, SchemaInfo } from '../../../api/types';
import { TooltipProvider } from '../../ui/Tooltip';
import { analyzeBody, grpcMessages } from '../model/body';
import { DecodedView } from './Structured';

const render = (node: ReactNode) => renderToStaticMarkup(<TooltipProvider>{node}</TooltipProvider>);

const base: BodyView = { kind: 'protobuf', size: 10, wireSize: 10, encodings: [], text: null, formatted: null, structured: { '1': 'sku-1', '2': 'Teapot' }, decodeError: null, truncated: false, captureTruncated: false };
const schema: SchemaInfo = { messageType: 'shop.v1.Item', source: 'mapping', mapping: '/items/*', typed: { id: 'sku-1', name: 'Teapot' } };

describe('typed protobuf view', () => {
  test('a protobuf body with a schema shows field names, the schema badge and the toggle', () => {
    const html = render(<DecodedView view={{ ...base, schema }} kind="protobuf" />);
    expect(html).toContain('schema:');
    expect(html).toContain('shop.v1.Item');
    expect(html).toContain('mapping /items/*');
    expect(html).toContain('Typed');
    expect(html).toContain('Schemaless');
    // The (virtualised) tree shows the typed decode.
    expect(html).toContain('aria-label="Decoded shop.v1.Item"');
    expect(html).not.toContain('Add schema');
  });

  test('without a schema it offers to add one', () => {
    const html = render(<DecodedView view={base} kind="protobuf" />);
    expect(html).toContain('Decoded without a schema');
    expect(html).toContain('Add schema');
    expect(html).toContain('aria-label="Decoded payload"');
    expect(html).not.toContain('schema:');
  });

  test('a mismatch keeps the schemaless decode and says why', () => {
    const failed: SchemaInfo = { messageType: 'shop.v1.Item', source: 'content-type', error: 'invalid wire type: Varint (expected LengthDelimited)' };
    const html = render(<DecodedView view={{ ...base, schema: failed }} kind="protobuf" />);
    expect(html).toContain('decode as');
    expect(html).toContain('invalid wire type');
    expect(html).not.toContain('Schemaless</'); // no toggle: only one decode
    // Binary content that didn't decode at all.
    const binary = render(<DecodedView view={{ ...base, kind: 'binary', structured: null, schema: failed }} kind="binary" />);
    expect(binary).toContain("Couldn&#x27;t decode this body");
    expect(binary).toContain('shop.v1.Item');
  });

  test('unknown fields are called out', () => {
    const html = render(<DecodedView view={{ ...base, schema: { ...schema, unknownFields: ['21', 'related.99'] } }} kind="protobuf" />);
    expect(html).toContain('21, related.99');
    expect(html).toContain('out of date');
  });
});

describe('typed gRPC view', () => {
  const grpc: BodyView = {
    ...base,
    kind: 'grpc',
    structured: {
      messages: [
        { compressed: false, decoded: { '1': 'Ada' }, trailers: null, size: 5, typed: { name: 'Ada' } },
        { compressed: false, decoded: { '1': 7 }, trailers: null, size: 2, typedError: 'invalid wire type' },
      ],
    },
    schema: { messageType: 'helloworld.HelloRequest', source: 'grpc', method: { service: 'helloworld.Greeter', method: 'SayHello', inputType: 'helloworld.HelloRequest', outputType: 'helloworld.HelloReply', clientStreaming: false, serverStreaming: false } },
  };

  test('messages show their typed decode, mismatches are flagged', () => {
    const html = render(<DecodedView view={grpc} kind="grpc" />);
    expect(html).toContain('helloworld.HelloRequest');
    expect(html).toContain('SayHello request');
    expect(html).toContain('>typed<');
    expect(html).toContain('schema mismatch');
  });

  test('the model keeps typed fields of gRPC messages', () => {
    const messages = grpcMessages(grpc);
    expect(messages[0]?.typed).toEqual({ name: 'Ada' });
    expect(messages[1]?.typedError).toBe('invalid wire type');
    expect(messages[1]?.typed).toBeUndefined();
  });

  test('gRPC without a schema still says schemaless', () => {
    const plain: BodyView = { ...grpc, schema: undefined, structured: { messages: [{ compressed: false, decoded: { '1': 'Ada' }, trailers: null, size: 5 }] } };
    const html = render(<DecodedView view={plain} kind="grpc" />);
    expect(html).toContain('schemaless');
    expect(html).toContain('Add schema');
  });
});

describe('body model with schemas', () => {
  test('a schema that matched a binary body offers the Decoded view (to explain the failure)', () => {
    const model = analyzeBody({ ...base, kind: 'binary', structured: null, schema: { messageType: 'a.B', source: 'mapping', error: 'x' } }, 'application/octet-stream', true);
    expect(model.modes).toContain('decoded');
    expect(model.auto).toBe('hex');
  });

  test('a typed decode alone is enough for the Decoded view', () => {
    const model = analyzeBody({ ...base, structured: null, schema }, 'application/x-protobuf', true);
    expect(model.modes[0]).toBe('decoded');
    expect(model.auto).toBe('decoded');
  });
});
