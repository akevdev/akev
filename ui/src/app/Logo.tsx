/**
 * The Akev mark: a chevron "A" whose crossbar is a single dot, the request being held between
 * the two sides of a connection. The legs take the surrounding text colour and the dot takes
 * the accent, so it sits on any themed surface. Geometry comes from `brand/build.ts`.
 */
export function Logo({ size = 28, className }: { size?: number; className?: string }) {
  return (
    <svg width={size} height={size} viewBox="240 240 544 544" className={className} aria-hidden>
      <path
        d="M512 280 748 744H656L512 460.9 368 744H276Z"
        fill="var(--text)"
        stroke="var(--text)"
        strokeWidth="24"
        strokeLinejoin="round"
      />
      <circle cx="512" cy="684" r="62" fill="var(--accent)" />
    </svg>
  );
}
