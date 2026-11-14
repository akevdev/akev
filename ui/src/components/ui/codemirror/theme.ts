/**
 * CodeMirror theme driven entirely by CSS custom properties (DESIGN.md §5.1), so switching
 * the app theme restyles every editor instantly without reconfiguring CodeMirror.
 */

import { HighlightStyle, syntaxHighlighting } from '@codemirror/language';
import type { Extension } from '@codemirror/state';
import { EditorView } from '@codemirror/view';
import { tags as t } from '@lezer/highlight';

const editorTheme = EditorView.theme({
  '&': {
    color: 'var(--text)',
    backgroundColor: 'var(--cm-bg, var(--bg-panel))',
    fontSize: 'var(--fs-code)',
    height: '100%',
  },
  '&.cm-focused': { outline: 'none' },
  '.cm-scroller': {
    fontFamily: 'var(--font-mono)',
    lineHeight: 'var(--lh-code)',
    fontFeatureSettings: 'normal',
    // No programming ligatures: `://`, `>=`, `!=` must render exactly as captured.
    fontVariantLigatures: 'none',
    overflow: 'auto',
  },
  '.cm-content': {
    padding: '8px 0',
    caretColor: 'var(--accent)',
  },
  '.cm-line': { padding: '0 12px 0 8px' },
  '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)', borderLeftWidth: '2px' },
  '&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection': {
    backgroundColor: 'var(--accent-soft-strong) !important',
  },
  '.cm-activeLine': { backgroundColor: 'var(--code-active-line)' },
  '.cm-gutters': {
    backgroundColor: 'var(--cm-bg, var(--bg-panel))',
    color: 'var(--text-disabled)',
    border: 'none',
    paddingLeft: '4px',
  },
  '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--text-subtle)' },
  '.cm-lineNumbers .cm-gutterElement': { padding: '0 6px 0 8px', minWidth: '28px', fontFeatureSettings: '"tnum"' },
  '.cm-foldGutter .cm-gutterElement': {
    padding: '0 2px',
    color: 'var(--text-subtle)',
    cursor: 'pointer',
    transition: 'color var(--dur-fast) var(--ease)',
  },
  '.cm-foldGutter .cm-gutterElement:hover': { color: 'var(--text)' },
  '.cm-foldPlaceholder': {
    backgroundColor: 'var(--bg-active)',
    border: 'none',
    color: 'var(--text-muted)',
    borderRadius: '4px',
    padding: '0 6px',
    margin: '0 2px',
    fontFamily: 'var(--font-ui)',
    fontSize: 'var(--fs-xs)',
  },
  '.cm-placeholder': { color: 'var(--text-subtle)', fontStyle: 'normal' },
  '.cm-matchingBracket, .cm-nonmatchingBracket': {
    backgroundColor: 'var(--bg-active)',
    outline: '1px solid var(--border-strong)',
    borderRadius: '2px',
  },
  '.cm-searchMatch': { backgroundColor: 'var(--code-match)', borderRadius: '2px' },
  '.cm-searchMatch.cm-searchMatch-selected': { backgroundColor: 'var(--code-match-active)' },
  '.cm-selectionMatch': { backgroundColor: 'var(--accent-soft)' },
  '.cm-tooltip': {
    backgroundColor: 'var(--bg-elevated)',
    color: 'var(--text)',
    border: 'none',
    borderRadius: 'var(--r-sm)',
    boxShadow: 'var(--shadow-md)',
    fontFamily: 'var(--font-ui)',
    fontSize: 'var(--fs-xs)',
  },
  // ---- search panel
  '.cm-panels': {
    backgroundColor: 'var(--bg-subtle)',
    color: 'var(--text)',
    fontFamily: 'var(--font-ui)',
  },
  '.cm-panels.cm-panels-top': { borderBottom: '1px solid var(--border)' },
  '.cm-panels.cm-panels-bottom': { borderTop: '1px solid var(--border)' },
  '.cm-search': {
    display: 'flex',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: '6px',
    padding: '6px 32px 6px 8px',
    fontSize: 'var(--fs-sm)',
  },
  '.cm-search br': { display: 'none' },
  '.cm-search label': {
    display: 'inline-flex',
    alignItems: 'center',
    gap: '4px',
    color: 'var(--text-muted)',
    fontSize: 'var(--fs-xs)',
    textTransform: 'none',
  },
  '.cm-search input[type=checkbox]': { accentColor: 'var(--accent)', margin: 0 },
  '.cm-textfield': {
    height: '24px',
    padding: '0 8px',
    backgroundColor: 'var(--bg-input)',
    color: 'var(--text)',
    border: '1px solid var(--border-input)',
    borderRadius: '5px',
    fontFamily: 'var(--font-mono)',
    fontVariantLigatures: 'none',
    fontSize: 'var(--fs-sm)',
    outline: 'none',
    margin: 0,
  },
  '.cm-textfield:focus': { borderColor: 'var(--border-focus)', boxShadow: '0 0 0 3px var(--accent-soft)' },
  '.cm-button': {
    height: '24px',
    padding: '0 8px',
    backgroundImage: 'none',
    backgroundColor: 'var(--bg-elevated)',
    color: 'var(--text)',
    border: '1px solid var(--border-strong)',
    borderRadius: '5px',
    fontFamily: 'var(--font-ui)',
    fontSize: 'var(--fs-xs)',
    textTransform: 'capitalize',
    cursor: 'pointer',
    margin: 0,
  },
  '.cm-button:hover': { backgroundColor: 'var(--bg-active)' },
  '.cm-button:active': { backgroundImage: 'none', backgroundColor: 'var(--bg-active)' },
  '.cm-panel.cm-search [name=close]': {
    position: 'absolute',
    top: '6px',
    right: '6px',
    width: '22px',
    height: '22px',
    borderRadius: '5px',
    color: 'var(--text-subtle)',
    fontSize: '16px',
    lineHeight: '20px',
    cursor: 'pointer',
    padding: 0,
  },
  '.cm-panel.cm-search [name=close]:hover': { backgroundColor: 'var(--bg-hover)', color: 'var(--text)' },
});

const highlight = HighlightStyle.define([
  { tag: [t.propertyName, t.definition(t.propertyName), t.attributeName], color: 'var(--syn-key)' },
  { tag: [t.string, t.special(t.string), t.regexp, t.character], color: 'var(--syn-string)' },
  { tag: [t.number, t.integer, t.float], color: 'var(--syn-number)' },
  { tag: [t.bool, t.null, t.atom, t.special(t.variableName)], color: 'var(--syn-bool)' },
  { tag: [t.keyword, t.controlKeyword, t.operatorKeyword, t.definitionKeyword, t.moduleKeyword, t.modifier], color: 'var(--syn-keyword)' },
  { tag: [t.tagName, t.angleBracket], color: 'var(--syn-tag)' },
  { tag: [t.attributeValue], color: 'var(--syn-string)' },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: 'var(--syn-fn)' },
  { tag: [t.typeName, t.className, t.namespace], color: 'var(--syn-type)' },
  { tag: [t.meta, t.processingInstruction, t.documentMeta, t.annotation], color: 'var(--syn-meta)' },
  { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], color: 'var(--syn-comment)', fontStyle: 'italic' },
  { tag: [t.punctuation, t.separator, t.bracket, t.squareBracket, t.brace, t.paren, t.operator, t.derefOperator], color: 'var(--syn-punct)' },
  { tag: [t.heading, t.heading1, t.heading2, t.heading3], color: 'var(--syn-key)', fontWeight: '600' },
  { tag: [t.link, t.url], color: 'var(--syn-link)', textDecoration: 'underline' },
  { tag: t.emphasis, fontStyle: 'italic' },
  { tag: t.strong, fontWeight: '600' },
  { tag: t.strikethrough, textDecoration: 'line-through' },
  { tag: t.invalid, color: 'var(--danger)' },
  { tag: [t.variableName], color: 'var(--text)' },
]);

/** Theme + syntax highlighting extension. */
export const akevTheme: Extension = [editorTheme, syntaxHighlighting(highlight)];
