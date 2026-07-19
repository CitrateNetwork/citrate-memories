#!/usr/bin/env python3
"""Citrate Memories — MCP connector shim (stdio <-> HTTP).

Your AI agent (Claude Code, Claude Desktop, or any MCP client) speaks MCP over
stdio to this small program. For every JSON-RPC message the agent sends, the shim
forwards it to the gateway's per-user endpoint

    POST {MEM_GATEWAY_ORIGIN}/mcp/u/{MEM_USER_SUB}
    Authorization: Bearer {MEM_CONNECT_TOKEN}
    Content-Type: application/json
    body: the JSON-RPC message (one per line)

and writes the gateway's reply back to the agent over stdout. That is the whole
job. It exists because the gateway's BYOM endpoint speaks newline-delimited
JSON-RPC (the same framing MCP uses over stdio) rather than the Streamable-HTTP
transport an MCP `http` server entry expects — so this shim bridges the two until
the gateway exposes Streamable HTTP directly.

The gateway builds a *fresh, stateless* memory server per request (it re-verifies
the connect token, looks up your org membership, and mints a short-lived grant
each time), so there is no session to keep alive here — the shim is a stateless
1:1 pipe.

Zero dependencies: Python 3 standard library only. No build, no `pip install`.

Configuration (all via environment, so your token never appears in `ps`):
    MEM_GATEWAY_ORIGIN   gateway base URL, e.g. https://mem-gateway.citrate.ai   (required)
    MEM_CONNECT_TOKEN    your connect token from Memrizz -> Connect an agent      (required)
    MEM_USER_SUB         your user id (shown in your Memrizz profile)             (required)
    MEM_CONNECT_TIMEOUT  per-request timeout in seconds (default: 30)

Exit codes: 0 on clean stdin EOF; 2 on missing/invalid configuration.

stdout carries the MCP protocol and MUST stay pure JSON-RPC — all diagnostics go
to stderr.
"""

import json
import os
import sys
import urllib.error
import urllib.parse
import urllib.request

_LOG_PREFIX = "citrate-memories connector"


def _log(msg: str) -> None:
    """Diagnostics go to stderr; stdout is the protocol channel."""
    print(f"{_LOG_PREFIX}: {msg}", file=sys.stderr, flush=True)


def _config():
    origin = os.environ.get("MEM_GATEWAY_ORIGIN", "").strip().rstrip("/")
    token = os.environ.get("MEM_CONNECT_TOKEN", "").strip()
    sub = os.environ.get("MEM_USER_SUB", "").strip()
    try:
        timeout = float(os.environ.get("MEM_CONNECT_TIMEOUT", "30"))
    except ValueError:
        timeout = 30.0

    missing = [
        name
        for name, val in (
            ("MEM_GATEWAY_ORIGIN", origin),
            ("MEM_CONNECT_TOKEN", token),
            ("MEM_USER_SUB", sub),
        )
        if not val
    ]
    if missing:
        _log(
            "missing required environment variable(s): "
            + ", ".join(missing)
            + ". Set them in your MCP server config's `env` block."
        )
        sys.exit(2)

    # The gateway matches the token's `sub` against the path segment, so a wrong
    # MEM_USER_SUB surfaces as 403; keep them consistent.
    url = f"{origin}/mcp/u/{urllib.parse.quote(sub, safe='')}"
    return url, token, timeout


def _error_response(rpc_id, message: str) -> str:
    """A well-formed JSON-RPC error so the agent sees a clean failure, not a
    truncated HTTP body leaking into the protocol stream."""
    return json.dumps(
        {
            "jsonrpc": "2.0",
            "id": rpc_id,
            "error": {"code": -32000, "message": f"{_LOG_PREFIX}: {message}"},
        }
    )


def _rpc_id(line: str):
    """Best-effort extraction of the JSON-RPC id. Returns (has_id, id).

    Notifications carry no `id` and expect no reply; requests carry one (which may
    legitimately be null). We only synthesize an error reply when there was an id.
    """
    try:
        obj = json.loads(line)
    except (ValueError, TypeError):
        return False, None
    if isinstance(obj, dict) and "id" in obj:
        return True, obj["id"]
    return False, None


def _forward(url: str, token: str, timeout: float, line: str) -> str:
    """POST one JSON-RPC line to the gateway; return the raw response body.

    Raises urllib.error.URLError / HTTPError on transport or HTTP failures.
    """
    req = urllib.request.Request(
        url,
        data=line.encode("utf-8"),
        method="POST",
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "Accept": "application/json",
        },
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return resp.read().decode("utf-8", errors="replace")


def main() -> int:
    url, token, timeout = _config()
    out = sys.stdout

    # `iter(readline, "")` — NOT `for line in sys.stdin` — so each message is
    # dispatched the instant its newline arrives. File iteration reads ahead to
    # fill a buffer, which deadlocks a request/response stdio server: the agent
    # sends `initialize` and waits for the reply before sending anything else,
    # but a read-ahead loop would block waiting for more input that never comes.
    for raw in iter(sys.stdin.readline, ""):
        line = raw.strip()
        if not line:
            continue

        has_id, rpc_id = _rpc_id(line)

        try:
            body = _forward(url, token, timeout, line)
        except urllib.error.HTTPError as e:
            detail = e.read().decode("utf-8", errors="replace").strip()
            msg = f"gateway returned HTTP {e.code}"
            if detail:
                msg += f": {detail[:300]}"
            _log(msg)
            if has_id:
                out.write(_error_response(rpc_id, msg) + "\n")
                out.flush()
            continue
        except (urllib.error.URLError, OSError) as e:
            msg = f"cannot reach gateway ({url}): {e}"
            _log(msg)
            if has_id:
                out.write(_error_response(rpc_id, msg) + "\n")
                out.flush()
            continue

        # A non-empty body is one or more newline-joined JSON-RPC responses; an
        # empty body means the message was a notification (no reply expected).
        body = body.strip("\n")
        if body:
            out.write(body + "\n")
            out.flush()

    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except BrokenPipeError:
        # The agent closed the connection; nothing left to do.
        sys.exit(0)
    except KeyboardInterrupt:
        sys.exit(0)
