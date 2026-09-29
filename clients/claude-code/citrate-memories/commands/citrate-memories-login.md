---
description: Sign in to Citrate Memories (opens your browser at the identity layer; no key to paste)
---

Run the Citrate Memories sign-in for this user. This performs a one-time,
browser-based login at the Citrate identity layer (`auth.citrate.ai`) and stores
a rotating credential locally — the user never copies or pastes a token.

Do this:

1. Run:

   ```bash
   python3 "${CLAUDE_PLUGIN_ROOT}/mcp-connector.py" login
   ```

2. A browser tab opens to the Citrate sign-in page. Tell the user to sign in
   with their Citrate account (the same login as the rest of the Citrate tools).
   If the browser does not open, the command prints a URL in the terminal — tell
   the user to paste it into their browser.

3. When it prints `signed in as …`, sign-in succeeded. The `citrate-memories`
   MCP server will now answer from the shared knowledge DAG. If the server was
   already running, ask the user to reconnect it (restart Claude Code, or
   `/mcp` → reconnect).

Notes:
- Access follows the user's **email/identity**, not a key. If sign-in works but
  memory calls return `no membership in org` (403), the user's email is not on
  the team allowlist yet — an operator adds it (see `docs/TEAM_ACCESS.md`).
- The login needs a local browser. On a headless/server host (e.g. an agent
  running remotely) this browser flow does not apply — see the device-code note
  in `docs/TEAM_ACCESS.md`.
