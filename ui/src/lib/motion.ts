/**
 * Motion preferences for JavaScript-driven animation. CSS motion is handled globally
 * (`global.css` → `prefers-reduced-motion`), but `scrollTo` / `scrollIntoView` calls pass
 * their behaviour explicitly and must ask here.
 */

/** The user asked the OS for reduced motion. */
export function prefersReducedMotion(): boolean {
  return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** `'smooth'`, or `'auto'` (an instant jump) when reduced motion is preferred. */
export function smoothScroll(): ScrollBehavior {
  return prefersReducedMotion() ? 'auto' : 'smooth';
}
