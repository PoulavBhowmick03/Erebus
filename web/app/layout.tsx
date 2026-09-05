import type { Metadata, Viewport } from "next";
import { Instrument_Serif, Cinzel, IBM_Plex_Mono } from "next/font/google";
import { KeyProvider } from "@/components/KeyContext";
import "./globals.css";

const instrument = Instrument_Serif({
  subsets: ["latin"],
  weight: "400",
  style: ["normal", "italic"],
  variable: "--font-instrument",
  display: "swap",
});

const cinzel = Cinzel({
  subsets: ["latin"],
  weight: ["400", "500"],
  variable: "--font-cinzel",
  display: "swap",
});

const plex = IBM_Plex_Mono({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  variable: "--font-plex",
  display: "swap",
});

export const metadata: Metadata = {
  metadataBase: new URL("https://erebus-private-agents.vercel.app"),
  title: "Erebus — negotiate in darkness, settle in silence",
  description:
    "Private coordination and shielded settlement infrastructure for AI agents on Starknet. Two agents negotiate over an encrypted channel and settle atomically through the STRK20 privacy pool.",
  openGraph: {
    title: "Erebus",
    description:
      "Private coordination and shielded settlement for AI agents on Starknet. Erebus hides the terms, not the relationship.",
    type: "website",
  },
};

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  themeColor: "#f2efe9",
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
      className={`${instrument.variable} ${cinzel.variable} ${plex.variable}`}
    >
      <head>
        <script dangerouslySetInnerHTML={{ __html: BOOT }} />
        <noscript>
          <style>{`[data-key="dropped"] .redact::after{clip-path:inset(0 0 0 100%)}`}</style>
        </noscript>
      </head>
      <body>
        <KeyProvider>{children}</KeyProvider>
      </body>
    </html>
  );
}
