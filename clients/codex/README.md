# Citrate Memories in Codex

Codex supports MCP servers over stdio. Use the same connector as every other
client; sign in once at the identity layer (no key to paste).

## 1. Get the connector (once)

```bash
mkdir -p ~/.citrate
curl -fsSL https://mem-gateway.citrate.ai/connector.py -o ~/.citrate/mcp-connector.py
chmod +x ~/.citrate/mcp-connector.py
```

That endpoint is public and holds no secrets.

## 2. Sign in (once)

```bash
python3 ~/.citrate/mcp-connector.py login
```

A browser opens to `auth.citrate.ai`; sign in with your Citrate account. When it
prints `signed in as …`, you're done — a rotating credential is stored under
`~/.citrate/` (0600). You never paste a token.

## 3. Add the server to Codex

Edit `~/.codex/config.toml` and add:

```toml
[mcp_servers.citrate-memories]
command = "python3"
args = ["/ABSOLUTE/PATH/TO/.citrate/mcp-connector.py"]
env = { MEM_GATEWAY_ORIGIN = "https://mem-gateway.citrate.ai", MEM_ISSUER = "https://auth.citrate.ai" }
```

Use the **absolute** path (replace `/ABSOLUTE/PATH/TO` with your home dir; `~` is
not expanded here). Restart Codex.

## Use it

Ask Codex things like:
- "Using citrate memories, what's the latest on citrate-radar?"
- "Recall the storyline for core-membership."

Every answer carries a freshness/provenance note.

## Troubleshooting

| Symptom | Meaning | Fix |
| --- | --- | --- |
| `not signed in` | no stored credential | run step 2 |
| `no membership in org` (403) | your email isn't on the team allowlist yet | ask an operator to add it (`docs/TEAM_ACCESS.md`) |
| `session expired … run login` | refresh token gone/expired | run step 2 again |
