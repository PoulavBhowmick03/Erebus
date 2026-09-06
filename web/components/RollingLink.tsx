/**
 * A link whose label rolls over on hover: the visible text slides up and out
 * while an identical copy slides in from below. Pure CSS, no measurement.
 *
 * The duplicate is `aria-hidden` and `select-none` so it stays out of the
 * accessibility tree and out of copied text.
 */
export function RollingLink({
  href,
  children,
  className = "",
  external = false,
}: {
  href: string;
  children: string;
  className?: string;
  external?: boolean;
}) {
  const label = external ? `${children} ↗` : children;
  return (
    <a href={href} className={`roll ${className}`}>
      <span className="roll-label">
        {label}
        <span aria-hidden className="roll-label-copy select-none">
          {label}
        </span>
      </span>
    </a>
  );
}
