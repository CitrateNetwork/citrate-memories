import { AppShell } from "@/components/AppShell";
import { AuthProvider } from "@/lib/auth/client";

/**
 * The Memrizz command deck.
 *
 * `force-dynamic` is REQUIRED for the nonce CSP: the per-request nonce minted in
 * `src/proxy.ts` can only be stamped onto Next's framework <script> tags when the
 * route is rendered per request. A statically prerendered page ships scripts with
 * NO nonce, which `script-src 'strict-dynamic'` then blocks — so nothing hydrates
 * and the UI is frozen. Dynamic rendering lets Next apply the request nonce.
 */
export const dynamic = "force-dynamic";

export default function Home() {
  return (
    <AuthProvider>
      <AppShell />
    </AuthProvider>
  );
}
