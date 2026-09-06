/**
 * Film grain. Four percent of a fractal-noise tile, multiplied into the page.
 *
 * A large field of pure black reads as a dead region on an OLED panel. Grain
 * gives it a surface, so the ground looks like a material rather than an
 * absence, and it costs one data URI and no runtime. Screen, not multiply:
 * multiplying into black is a no-op.
 */
const GRAIN =
  "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='220' height='220'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.82' numOctaves='3' stitchTiles='stitch'/%3E%3CfeColorMatrix type='saturate' values='0'/%3E%3C/filter%3E%3Crect width='220' height='220' filter='url(%23n)'/%3E%3C/svg%3E\")";

export function Grain({
  className = "",
  opacity = 0.055,
  blend = "screen",
}: {
  className?: string;
  opacity?: number;
  blend?: "multiply" | "overlay" | "screen";
}) {
  return (
    <div
      aria-hidden
      className={`pointer-events-none fixed inset-0 z-[1] ${className}`}
      style={{ backgroundImage: GRAIN, opacity, mixBlendMode: blend }}
    />
  );
}
