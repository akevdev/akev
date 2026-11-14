import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap } from '@codemirror/autocomplete';
import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
import { bracketMatching, codeFolding, foldAll, foldGutter, foldKeymap, indentOnInput, unfoldAll } from '@codemirror/language';
import { highlightSelectionMatches, openSearchPanel, search, searchKeymap } from '@codemirror/search';
import { Annotation, Compartment, EditorSelection, EditorState, type Extension } from '@codemirror/state';
import {
  drawSelection,
  dropCursor,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers as lineNumbersExt,
  placeholder as placeholderExt,
  rectangularSelection,
} from '@codemirror/view';
import { useEffect, useImperativeHandle, useRef, type CSSProperties } from 'react';
import type { CodeEditorProps } from '../CodeEditor';
import { FOLDABLE, loadLanguage, normalizeLanguage, type CodeLanguage } from './languages';
import { akevTheme } from './theme';

/** Marks transactions that originate from prop updates (not the user). */
const External = Annotation.define<boolean>();

function chevron(open: boolean): HTMLElement {
  const span = document.createElement('span');
  span.setAttribute('aria-hidden', 'true');
  span.innerHTML = `<svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.25" stroke-linecap="round" stroke-linejoin="round" style="transform:rotate(${open ? 0 : -90}deg);transition:transform 120ms"><path d="m6 9 6 6 6-6"/></svg>`;
  span.style.display = 'flex';
  span.style.alignItems = 'center';
  span.style.height = '100%';
  return span;
}

function foldingExt(enabled: boolean): Extension {
  return enabled ? [codeFolding(), foldGutter({ markerDOM: chevron })] : [];
}

function gutterExt(enabled: boolean): Extension {
  return enabled ? [lineNumbersExt(), highlightActiveLineGutter()] : [];
}

function readOnlyExt(readOnly: boolean): Extension {
  // Read-only editors stay focusable so users can select, search (⌘F) and copy.
  return readOnly ? [EditorState.readOnly.of(true)] : [highlightActiveLine(), closeBrackets(), autocompletion({ activateOnTyping: false })];
}

/** The real CodeMirror-backed editor (loaded lazily by `CodeEditor`). */
export default function CodeEditorImpl({
  value,
  onChange,
  language = 'text',
  readOnly = false,
  wrap = false,
  lineNumbers = true,
  folding,
  placeholder,
  className,
  autoFocus,
  contentId,
  onSubmit,
  onSave,
  minHeight,
  maxHeight,
  fill,
  transparent,
  ref,
  ...aria
}: CodeEditorProps) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const viewRef = useRef<EditorView | null>(null);
  const compartments = useRef({
    language: new Compartment(),
    readOnly: new Compartment(),
    wrap: new Compartment(),
    gutter: new Compartment(),
    folding: new Compartment(),
    placeholder: new Compartment(),
    aria: new Compartment(),
  });
  const callbacks = useRef({ onChange, onSubmit, onSave });
  useEffect(() => {
    callbacks.current = { onChange, onSubmit, onSave };
  });

  const lang: CodeLanguage = normalizeLanguage(language);
  const foldOn = folding ?? FOLDABLE.has(lang);

  useImperativeHandle(
    ref,
    () => ({
      get view() {
        return viewRef.current;
      },
      focus: () => viewRef.current?.focus(),
      openSearch: () => {
        const v = viewRef.current;
        if (v) openSearchPanel(v);
      },
      foldAll: () => {
        const v = viewRef.current;
        if (v) foldAll(v);
      },
      unfoldAll: () => {
        const v = viewRef.current;
        if (v) unfoldAll(v);
      },
      getValue: () => viewRef.current?.state.doc.toString() ?? value,
    }),
    [value],
  );

  // Create the view once.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const c = compartments.current;
    const view = new EditorView({
      parent: host,
      state: EditorState.create({
        doc: value,
        extensions: [
          c.gutter.of(gutterExt(lineNumbers)),
          c.folding.of(foldingExt(foldOn)),
          highlightSpecialChars(),
          drawSelection(),
          dropCursor(),
          history(),
          indentOnInput(),
          bracketMatching(),
          rectangularSelection(),
          highlightSelectionMatches(),
          search({ top: true }),
          keymap.of([
            {
              key: 'Mod-Enter',
              run: () => {
                const fn = callbacks.current.onSubmit;
                if (!fn) return false;
                fn();
                return true;
              },
            },
            {
              key: 'Mod-s',
              run: () => {
                const fn = callbacks.current.onSave;
                if (!fn) return false;
                fn();
                return true;
              },
            },
            ...closeBracketsKeymap,
            ...defaultKeymap,
            ...searchKeymap,
            ...historyKeymap,
            ...foldKeymap,
            ...completionKeymap,
          ]),
          akevTheme,
          c.readOnly.of(readOnlyExt(readOnly)),
          c.wrap.of(wrap ? EditorView.lineWrapping : []),
          c.placeholder.of(placeholder ? placeholderExt(placeholder) : []),
          c.aria.of(EditorView.contentAttributes.of({ 'aria-label': aria['aria-label'] ?? 'Code editor' })),
          c.language.of([]),
          EditorView.updateListener.of((update) => {
            if (!update.docChanged) return;
            if (update.transactions.some((tr) => tr.annotation(External))) return;
            callbacks.current.onChange?.(update.state.doc.toString());
          }),
        ],
      }),
    });
    viewRef.current = view;
    if (autoFocus) view.focus();
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // The view is created once; props are synced by the effects below.
  }, []);

  // Sync value.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const current = view.state.doc.toString();
    if (current === value) return;
    const sel = view.state.selection.main;
    view.dispatch({
      changes: { from: 0, to: current.length, insert: value },
      selection: EditorSelection.cursor(Math.min(sel.head, value.length)),
      annotations: External.of(true),
    });
  }, [value]);

  // New logical document: reset selection and scroll.
  useEffect(() => {
    const view = viewRef.current;
    if (!view || contentId === undefined) return;
    view.dispatch({ selection: EditorSelection.cursor(0), scrollIntoView: true, annotations: External.of(true) });
    view.scrollDOM.scrollTop = 0;
  }, [contentId]);

  // Language (lazy).
  useEffect(() => {
    let cancelled = false;
    void loadLanguage(lang).then((ext) => {
      if (!cancelled) viewRef.current?.dispatch({ effects: compartments.current.language.reconfigure(ext) });
    });
    return () => {
      cancelled = true;
    };
  }, [lang]);

  useEffect(() => {
    viewRef.current?.dispatch({ effects: compartments.current.readOnly.reconfigure(readOnlyExt(readOnly)) });
  }, [readOnly]);
  useEffect(() => {
    viewRef.current?.dispatch({ effects: compartments.current.wrap.reconfigure(wrap ? EditorView.lineWrapping : []) });
  }, [wrap]);
  useEffect(() => {
    viewRef.current?.dispatch({ effects: compartments.current.gutter.reconfigure(gutterExt(lineNumbers)) });
  }, [lineNumbers]);
  useEffect(() => {
    viewRef.current?.dispatch({ effects: compartments.current.folding.reconfigure(foldingExt(foldOn)) });
  }, [foldOn]);
  useEffect(() => {
    viewRef.current?.dispatch({ effects: compartments.current.placeholder.reconfigure(placeholder ? placeholderExt(placeholder) : []) });
  }, [placeholder]);

  const style = {
    '--cm-min-h': typeof minHeight === 'number' ? `${minHeight}px` : (minHeight ?? 'auto'),
    '--cm-max-h': typeof maxHeight === 'number' ? `${maxHeight}px` : (maxHeight ?? 'none'),
    ...(transparent ? { '--cm-bg': 'transparent' } : {}),
  } as CSSProperties;

  return <div ref={hostRef} className={className} data-fill={fill || undefined} data-akev-code style={style} />;
}
