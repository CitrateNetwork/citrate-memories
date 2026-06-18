/**
 * Auth seam entry. Import the concrete halves directly to keep client and server
 * bundles separate:
 *   - client components → `@/lib/auth/client`  (AuthProvider, useAuth) [WP login]
 *   - server / API routes → `@/lib/auth/session` (verifySession, requireOwner)
 * Only the provider-agnostic TYPES are safe to barrel-export here.
 */
export type { AuthSession, AuthContextValue, AuthMode } from "./types";
