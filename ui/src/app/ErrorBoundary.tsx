import { ChevronRight, ClipboardCopy, RefreshCw, RotateCcw, TriangleAlert } from 'lucide-react';
import { Component, type ErrorInfo, type ReactNode } from 'react';
import { copyText } from '../lib/platform';
import { Logo } from './Logo';
import styles from './ErrorBoundary.module.css';

interface Props {
  children: ReactNode;
  /** `app` = full-screen crash screen; `page` = in-place panel with "Try again". */
  scope?: 'app' | 'page';
  /** Changing this resets the boundary (e.g. the route). */
  resetKey?: string;
}

interface State {
  error: Error | null;
  componentStack: string;
  showDetails: boolean;
  copied: boolean;
}

function details(error: Error, componentStack: string): string {
  return [
    `Akev UI error: ${error.name}: ${error.message}`,
    `URL: ${window.location.hash || '/'}`,
    `User agent: ${navigator.userAgent}`,
    '',
    error.stack ?? '(no stack)',
    '',
    'Component stack:',
    componentStack.trim(),
  ].join('\n');
}

/** Catches render errors and shows a friendly recovery screen instead of a blank window. */
export class ErrorBoundary extends Component<Props, State> {
  override state: State = { error: null, componentStack: '', showDetails: false, copied: false };

  static getDerivedStateFromError(error: unknown): Partial<State> {
    return { error: error instanceof Error ? error : new Error(String(error)) };
  }

  override componentDidCatch(error: Error, info: ErrorInfo): void {
    this.setState({ componentStack: info.componentStack ?? '' });
    console.error('[akev] UI crashed', error, info.componentStack);
  }

  override componentDidUpdate(prev: Props): void {
    if (prev.resetKey !== this.props.resetKey && this.state.error) this.reset();
  }

  private reset = () => this.setState({ error: null, componentStack: '', showDetails: false, copied: false });

  private copy = async () => {
    const { error, componentStack } = this.state;
    if (!error) return;
    if (await copyText(details(error, componentStack))) {
      this.setState({ copied: true });
      setTimeout(() => this.setState({ copied: false }), 1500);
    }
  };

  override render(): ReactNode {
    const { error, showDetails, copied, componentStack } = this.state;
    if (!error) return this.props.children;
    const page = this.props.scope === 'page';
    return (
      <div className={page ? styles.page : styles.screen} role="alert">
        <div className={styles.card}>
          {page ? (
            <div className={styles.icon}>
              <TriangleAlert size={22} strokeWidth={1.6} aria-hidden />
            </div>
          ) : (
            <Logo size={40} />
          )}
          <h1 className={styles.title}>{page ? 'This page hit a snag' : 'Something went wrong'}</h1>
          <p className={styles.text}>
            {page
              ? 'An unexpected error stopped this page from rendering. The rest of Akev keeps working, and captured traffic is safe in the Akev background service.'
              : 'The interface hit an unexpected error. Your captured traffic and rules are safe in the Akev background service — reloading usually fixes this.'}
          </p>
          <code className={styles.message}>{error.message || error.name}</code>
          <div className={styles.actions}>
            {page ? (
              <button type="button" className={styles.primary} onClick={this.reset}>
                <RotateCcw size={14} strokeWidth={1.75} aria-hidden /> Try again
              </button>
            ) : null}
            <button type="button" className={page ? styles.secondary : styles.primary} onClick={() => window.location.reload()}>
              <RefreshCw size={14} strokeWidth={1.75} aria-hidden /> Reload Akev
            </button>
            <button type="button" className={styles.secondary} onClick={this.copy}>
              <ClipboardCopy size={14} strokeWidth={1.75} aria-hidden /> {copied ? 'Copied' : 'Copy details'}
            </button>
          </div>
          <button type="button" className={styles.toggle} onClick={() => this.setState({ showDetails: !showDetails })} aria-expanded={showDetails}>
            <ChevronRight size={12} strokeWidth={2} style={{ transform: showDetails ? 'rotate(90deg)' : undefined }} aria-hidden />
            Technical details
          </button>
          {showDetails && <pre className={styles.stack}>{details(error, componentStack)}</pre>}
        </div>
      </div>
    );
  }
}
