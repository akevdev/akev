/**
 * Tiny CodeMirror StreamLanguage for raw HTTP/1.x messages (request or response):
 *
 *   GET /path?x=1 HTTP/1.1        ← method · target · version
 *   HTTP/1.1 200 OK               ← version · status · reason
 *   Content-Type: application/json ← header name · value
 *                                  ← blank line
 *   {"body": …}                    ← body (plain)
 *
 * Also handles HTTP/2 pseudo-headers (`:authority: example.com`).
 */

import { StreamLanguage, type StreamParser } from '@codemirror/language';

interface HttpState {
  /** 0 = start line, 1 = headers, 2 = body */
  section: 0 | 1 | 2;
  /** Position within the start line / header line. */
  part: number;
  /** Line kind once known. */
  line: 'request' | 'response' | 'header' | null;
}

const METHOD = /^[A-Z][A-Z0-9-]*(?=\s)/;
const VERSION = /^HTTP\/\d(?:\.\d)?/;

export const httpParser: StreamParser<HttpState> = {
  name: 'http',
  startState: () => ({ section: 0, part: 0, line: null }),
  copyState: (s) => ({ ...s }),
  blankLine(state) {
    if (state.section === 1 || state.section === 0) state.section = 2;
  },
  token(stream, state) {
    if (stream.sol()) {
      state.part = 0;
      state.line = null;
      if (state.section === 0 && stream.match(/^\s*$/, false)) {
        stream.skipToEnd();
        return null;
      }
    }

    if (state.section === 2) {
      stream.skipToEnd();
      return null;
    }

    if (state.section === 0) {
      if (stream.eatSpace()) return null;
      if (state.part === 0) {
        state.part = 1;
        if (stream.match(VERSION)) {
          state.line = 'response';
          return 'keyword';
        }
        if (stream.match(METHOD)) {
          state.line = 'request';
          return 'keyword';
        }
        // Not a start line: treat as headers.
        state.section = 1;
      } else if (state.line === 'request') {
        if (state.part === 1) {
          state.part = 2;
          stream.match(/^\S+/);
          return 'string';
        }
        if (stream.match(VERSION)) {
          stream.skipToEnd();
          state.section = 1;
          return 'atom';
        }
        stream.skipToEnd();
        state.section = 1;
        return null;
      } else if (state.line === 'response') {
        if (state.part === 1 && stream.match(/^\d{3}/)) {
          state.part = 2;
          return 'number';
        }
        stream.skipToEnd();
        state.section = 1;
        return 'comment';
      }
    }

    // headers
    if (state.part === 0) {
      state.part = 1;
      if (stream.match(/^:?[^:\s][^:]*/)) return 'propertyName';
      stream.skipToEnd();
      return null;
    }
    if (state.part === 1) {
      if (stream.eat(':')) {
        state.part = 2;
        return 'punctuation';
      }
      stream.skipToEnd();
      return null;
    }
    // Whitespace after the colon is its own (unstyled) token.
    if (stream.eatSpace()) return null;
    stream.skipToEnd();
    return 'string';
  },
  languageData: { commentTokens: {} },
};

export const http = StreamLanguage.define(httpParser);
