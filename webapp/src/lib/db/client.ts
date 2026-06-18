import "server-only";
import { drizzle } from "drizzle-orm/neon-http";
import { neon } from "@neondatabase/serverless";
import * as schema from "./schema";

/**
 * The Neon Postgres client (Drizzle). Lazy + memoized: it connects on first use,
 * and **throws when `DATABASE_URL` is unset** — persistence fails closed rather
 * than silently dropping data. `DATABASE_URL` is server-only (never NEXT_PUBLIC).
 */
let cached: ReturnType<typeof drizzle<typeof schema>> | null = null;

export function db() {
  if (cached) return cached;
  const url = process.env.DATABASE_URL;
  if (!url) {
    throw new Error("DATABASE_URL is not set — conversation persistence is unavailable.");
  }
  cached = drizzle(neon(url), { schema });
  return cached;
}
