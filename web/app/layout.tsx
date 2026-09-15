import type { Metadata, Viewport } from "next";
import { Syne, JetBrains_Mono } from "next/font/google";
import { KeyProvider } from "@/components/KeyContext";
import { SmoothScroll } from "@/components/SmoothScroll";
import { Grain } from "@/components/Grain";
import { Analytics } from "@vercel/analytics/next";
import "./globals.css";

// A grotesque and a mono, and nothing else. An editorial serif on a warm
// ground is the exact look this page is trying not to have.
//
// Neither face is Archivo or IBM Plex Mono. Those two are the reflex choice
// for every dark, technical, AI-adjacent product built since 2025 — using
// them is how a page ends up looking like a template even when nothing else
// about it is. Syne's proportions are unusual enough at display size to read
// as a deliberate choice; JetBrains Mono still reads as "terminal" without
// being the same terminal font as everything else.
const display = Syne({
  subsets: ["latin"],
  weight: ["500", "600", "700", "800"],
  variable: "--font-display-face",
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
      className={`${display.variable} ${mono.variable}`}
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
