import type { Metadata, Viewport } from "next";
import { Anton, Space_Grotesk, JetBrains_Mono } from "next/font/google";
import { KeyProvider } from "@/components/KeyContext";
import { SmoothScroll } from "@/components/SmoothScroll";
import { Grain } from "@/components/Grain";
import { Analytics } from "@vercel/analytics/next";
import "./globals.css";

// Three faces doing three different jobs, not one grotesque stretched to
// cover headline, body, and data. A single family everywhere is exactly what
// makes a page read as templated: there's no contrast in the type itself for
// the eye to register as "someone chose this." Anton is a poster face —
// ultra-heavy, condensed, built to be shouted at headline size, not a
// slightly-bolder version of a body font. Space Grotesk carries the actual
// sentences, where a poster face would be unreadable. JetBrains Mono stays
// for data and labels.
const display = Anton({
  subsets: ["latin"],
  weight: ["400"],
  variable: "--font-display-face",
  display: "swap",
});

const body = Space_Grotesk({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  variable: "--font-body-face",
  display: "swap",
});

const mono = JetBrains_Mono({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  variable: "--font-mono-face",
  display: "swap",
});

export const metadata: Metadata = {
  metadataBase: new URL("https://erebusagents.live"),
  title: "Erebus",
  description:
    "Private coordination and shielded settlement for AI agents on Starknet. Two agents negotiate over an encrypted channel and settle atomically through the STRK20 pool.",
  openGraph: {
    title: "Erebus",
    description:
      "Private coordination and shielded settlement for AI agents on Starknet.",
    type: "website",
    url: "/",
    siteName: "Erebus",
    images: [
      {
        url: "/og.png",
        width: 1200,
        height: 630,
        alt: "Erebus",
      },
    ],
  },
  twitter: {
    card: "summary_large_image",
    title: "Erebus",
    description:
      "Private coordination and shielded settlement for AI agents on Starknet.",
    images: ["/og.png"],
  },
};

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  themeColor: "#0a0908",
};

/**
 * The document ships unkeyed. Every value is still plaintext in the markup —
 * the redaction is an ink bar drawn over it — so a reader with no JavaScript,
 * a crawler, or a link preview sees the complete page. The reveal is theatre
 * layered on top of readable content, never a substitute for it.
 */
const BOOT = `
try {
  var r = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
  document.documentElement.setAttribute('data-key', r ? 'held' : 'dropped');
} catch (e) {
  document.documentElement.setAttribute('data-key', 'held');
}
`.trim();

export default function RootLayout({ children }: { children: React.ReactNode }) {
  return (
    <html
      lang="en"
      data-key="dropped"
      className={`${display.variable} ${body.variable} ${mono.variable}`}
    >
      <head>
        <script dangerouslySetInnerHTML={{ __html: BOOT }} />
        <noscript>
          <style>{`[data-key="dropped"] .redact::after{clip-path:inset(0 0 0 100%)}`}</style>
        </noscript>
      </head>
      <body>
        <KeyProvider>
          <SmoothScroll />
          {children}
          <Grain />
        </KeyProvider>
        <Analytics />
      </body>
    </html>
  );
}
