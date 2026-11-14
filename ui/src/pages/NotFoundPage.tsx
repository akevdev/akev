import { ArrowLeft, Compass, Settings } from 'lucide-react';
import { NAV_ITEMS } from '../app/navItems';
import { navigate, useRoute } from '../app/router';
import { Button, EmptyState, Toolbar } from '../components/ui';
import styles from './NotFoundPage.module.css';

/** Any URL that matches no page (e.g. an old bookmark): say so and offer the pages that exist. */
export function NotFoundPage() {
  const route = useRoute();
  const canGoBack = window.history.length > 1;
  return (
    <div className={styles.page}>
      <Toolbar titleBar left={<h1 className={styles.title}>Page not found</h1>} />
      <div className={styles.body}>
        <EmptyState
          icon={Compass}
          size="lg"
          tone="accent"
          title="There's no page here"
          description={
            <>
              Nothing lives at <code className={styles.path}>#{route.path || '/'}</code>. The link may be old or incomplete.
            </>
          }
          actions={
            <>
              <Button variant="primary" onClick={() => navigate('/traffic')}>
                Go to Traffic
              </Button>
              {canGoBack && (
                <Button icon={ArrowLeft} onClick={() => window.history.back()}>
                  Back
                </Button>
              )}
            </>
          }
        >
          <nav className={styles.pages} aria-label="Pages">
            {NAV_ITEMS.filter((item) => item.page !== 'traffic').map((item) => (
              <Button key={item.page} variant="ghost" size="sm" icon={item.icon} onClick={() => navigate(item.path)}>
                {item.label}
              </Button>
            ))}
            <Button variant="ghost" size="sm" icon={Settings} onClick={() => navigate('/settings')}>
              Settings
            </Button>
          </nav>
        </EmptyState>
      </div>
    </div>
  );
}

export default NotFoundPage;
