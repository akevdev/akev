/**
 * Shared exchange-inspection composites used by the Traffic detail pane, Compose (response
 * view), breakpoints and the Compare dialog.
 *
 * CONTRACT: the props interfaces below are frozen — pages are built against them in parallel.
 * Implementations may be improved freely (more view modes, polish), never narrowed.
 */
export { BodyViewer, type BodyViewerProps } from './BodyViewer';
export { HeadersTable, type HeadersTableProps } from './HeadersTable';
export { CookiesTable, type CookiesTableProps } from './CookiesTable';
export { TimingView, type TimingViewProps } from './TimingView';
export { ConnectionView, type ConnectionViewProps } from './ConnectionView';
export { CacheView, type CacheViewProps } from './CacheView';
export { CodeView, type CodeViewProps } from './CodeView';
export { TextDiff, type TextDiffProps } from './TextDiff';
export { ExchangeCompare, type ExchangeCompareProps } from './ExchangeCompare';
