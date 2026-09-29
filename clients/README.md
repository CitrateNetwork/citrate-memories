# Connect to Citrate Memories — pick your path

Citrate Memories is the team's shared, always-current memory of every Citrate
repo. Access follows your **identity** — an operator adds your email to the team
allowlist, and you're in. There's no key to copy or paste.

## Just want to look things up? (no install)

Go to **https://memrizz.citrate.ai** and sign in with your Citrate account. Done.
This is the right path for most non-technical use. See
[`browser/README.md`](browser/README.md).

## Want your AI agent to pull from memory?

You sign in once (a browser login at the identity layer), then your agent can
recall the federation's knowledge while it works. Pick your client:

| Client | Guide |
| --- | --- |
| **Claude Code** | Install the plugin (below), then `/citrate-memories-login`. |
| **Codex** | [`codex/README.md`](codex/README.md) |
| **Cursor / any MCP client** | [`cursor/README.md`](cursor/README.md) |

### Claude Code plugin (the no-config path)

```
/plugin marketplace add CitrateNetwork/citrate-memories
/plugin install citrate-memories@citrate
/citrate-memories-login
```

The plugin bundles the connector and adds the `citrate-memories` MCP server; the
login command opens your browser to sign in. After it says `signed in as …`,
reconnect the server (restart Claude Code or `/mcp` → reconnect) and ask:
"Using citrate memories, what's the latest on <repo>?"

## Non-technical, but your agent should do the setup

You don't have to run any commands yourself. Tell your agent (Claude Code,
Codex, …): **"Set up Citrate Memories and sign me in."** It runs the one-time
login for you; you just complete the sign-in in the browser tab that opens.

## Notes

- **Entitlement:** if sign-in works but memory calls return `no membership in
  org` (403), your email isn't on the allowlist yet — an operator adds it. See
  [`../docs/TEAM_ACCESS.md`](../docs/TEAM_ACCESS.md).
- **Server-side / headless agents** (e.g. Hermes running remotely) can't use the
  browser login; a device-code flow is the planned path — see
  [`../docs/TEAM_ACCESS.md`](../docs/TEAM_ACCESS.md).
