import { describe, expect, test } from 'bun:test';
import type { ReactNode } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { BodyView } from '../../../api/types';
import { TooltipProvider } from '../../ui/Tooltip';
import { DecodedView, EventsView, FormView, PartsView } from './Structured';

const render = (node: ReactNode) => renderToStaticMarkup(<TooltipProvider>{node}</TooltipProvider>);

const base: BodyView = {
  kind: 'msg-pack',
  size: 10,
  wireSize: 10,
  encodings: [],
  text: null,
  formatted: null,
  structured: null,
  decodeError: null,
  truncated: false,
  captureTruncated: false,
};

const NOTE = 'Decoded view truncated — showing the first part';

describe('structuredTruncated', () => {
  const cases: [string, BodyView, (v: BodyView) => ReactNode][] = [
    ['decoded', { ...base, structured: { a: 1 } }, (v) => <DecodedView view={v} kind="msg-pack" />],
    ['grpc', { ...base, kind: 'grpc', structured: { messages: [{ compressed: false, decoded: { 1: 'x' }, trailers: null, size: 3 }] } }, (v) => <DecodedView view={v} kind="grpc" />],
    ['parts', { ...base, kind: 'multipart', structured: [{ name: 'a', filename: null, contentType: 'text/plain', headers: [], size: 1, text: 'x' }] }, (v) => <PartsView view={v} />],
    ['events', { ...base, kind: 'event-stream', structured: [{ event: null, id: null, retry: null, data: 'hi' }] }, (v) => <EventsView view={v} contentId="c" />],
    ['form', { ...base, kind: 'form-url-encoded', structured: [['a', '1']] }, (v) => <FormView view={v} />],
  ];

  for (const [name, view, make] of cases) {
    test(`${name}: a banner says the decode was cut short, only when it was`, () => {
      expect(render(make({ ...view, structuredTruncated: true }))).toContain(NOTE);
      expect(render(make(view))).not.toContain(NOTE);
    });
  }
});
