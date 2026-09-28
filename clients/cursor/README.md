# Citrate Memories in Cursor (and other generic MCP clients)

Cursor supports stdio MCP servers. Same connector, same one-time identity login.

## 1. Get the connector (once)

```bash
mkdir -p ~/.citrate
curl -fsSL https://mem-gateway.citrate.ai/connector.py -o ~/.citrate/mcp-connector.py
chmod +x ~/.citrate/mcp-connector.py
```

## 2. Sign in (once)

```bash
python3 ~/.citrate/mcp-connector.py login
```

Sign in with your Citrate account in the browser tab that opens. No token to paste.

## 3. Add the server

Edit `~/.cursor/mcp.json` (create it if missing):

```json
{
  "mcpServers": {
    "citrate-memories": {
      "command": "python3",
      "args": ["/ABSOLUTE/PATH/TO/.citrate/mcp-connector.py"],
      "env": {
        "MEM_GATEWAY_ORIGIN": "https://mem-gateway.citrate.ai",
        "MEM_ISSUER": "https://auth.citrate.ai"
      }
    }
  }
}
```

Use the **absolute** path (`~` is not expanded in `args`). Reload Cursor.

Any MCP client works the same way — point it at `python3 <path>/mcp-connector.py`
as a stdio server, with the two `env` values above.
