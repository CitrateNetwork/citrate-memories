import type { Metadata, Viewport } from "next";
import { Geist, Geist_Mono, Space_Grotesk } from "next/font/google";
import "./globals.css";
import "./memrizz.css";

// Self-hosted at build time by next/font — no runtime request to Google, so the
// nonce CSP needs no fonts.googleapis.com origin (planset §3.B.2). The CSS var
// names match those referenced in globals.css.
//
// Space Grotesk is the Citrate brand DISPLAY/title font (matches the federation
// tokens). It's a variable font (wght 300–700), so we omit `weight` and the
// display type classes (font-weight 360–700) resolve against the variable axis.
const spaceGrotesk = Space_Grotesk({
  subsets: ["latin"],
  variable: "--font-space-grotesk",
  display: "swap",
});
const geistSans = Geist({
  subsets: ["latin"],
  weight: ["300", "400", "500", "600", "700"],
  variable: "--font-geist-sans",
  display: "swap",
});
const geistMono = Geist_Mono({
  subsets: ["latin"],
  weight: ["400", "500", "600"],
  variable: "--font-geist-mono",
  display: "swap",
});

export const metadata: Metadata = {
  title: "Memrizz — Citrate Memory Constellation",
  description:
    "The human face of the citrate-memories knowledge DAG: see, ask, and steward your org's memory.",
};

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
  themeColor: "#0a1810",
};

export default function RootLayout({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <html lang="en">
      <body
        className={`${spaceGrotesk.variable} ${geistSans.variable} ${geistMono.variable}`}
      >
        {children}
      </body>
    </html>
  );
}
