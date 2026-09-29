#!/usr/bin/env python3
"""Citrate Memories — MCP connector + one-command login (single file, stdlib only).

Your AI agent (Claude Code, Claude Desktop, Cursor, any MCP client) speaks MCP
over stdio to this program; it forwards each JSON-RPC message to the Citrate
Memories gateway and writes the reply back. It has two modes:

  login    Interactive. Opens auth.citrate.ai in your browser — the IDENTITY
           layer only, no app to click through — completes an RFC 8252 loopback
           PKCE login, and stores a rotating refresh token under ~/.citrate/.
           Run this ONCE:  python3 mcp-connector.py login

  (default) MCP stdio bridge. For every JSON-RPC line your agent sends, this
           mints a fresh id_token from your stored refresh token and POSTs the
           line to the gateway's MCP endpoint:
               POST {MEM_GATEWAY_ORIGIN}/mcp/u/{sub}
               Authorization: Bearer {id_token}
           You never paste a token: after `login`, entitlement follows your
           identity (an operator adds your email to the team allowlist and you
           are provisioned on first call).

Zero dependencies: Python 3 standard library only. No `pip install`.

Configuration (env; production-ready defaults):
  MEM_ISSUER          OIDC issuer            (default https://auth.citrate.ai)
  MEM_GATEWAY_ORIGIN  gateway base URL       (default https://mem-gateway.citrate.ai)
  MEM_OIDC_CLIENT_ID  OAuth client id        (default memrizz)
  MEM_OIDC_SCOPE      requested scope        (default "openid profile wallet offline_access")
  MEM_LOOPBACK_PORT   loopback port for the login redirect (default 3000; must
                      match the client's registered http://127.0.0.1:PORT/auth/callback)
  MEM_AUTH_FILE       token store            (default ~/.citrate/memories-auth.json)
  MEM_CONNECT_TIMEOUT per-request timeout seconds (default 30)

Legacy fallback: if MEM_CONNECT_TOKEN and MEM_USER_SUB are set (the older
webapp-minted connect-token flow), the bridge uses those instead of the login
store — so existing setups keep working untouched.

stdout carries the MCP protocol and MUST stay pure JSON-RPC; all diagnostics go
to stderr.
"""

import base64
import hashlib
import json
import os
import secrets
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import webbrowser
from http.server import BaseHTTPRequestHandler, HTTPServer

_LOG = "citrate-memories connector"


def _log(msg):
    print(f"{_LOG}: {msg}", file=sys.stderr, flush=True)


def _env(name, default):
    v = os.environ.get(name, "").strip()
    return v if v else default


ISSUER = _env("MEM_ISSUER", "https://auth.citrate.ai").rstrip("/")
GATEWAY = _env("MEM_GATEWAY_ORIGIN", "https://mem-gateway.citrate.ai").rstrip("/")
CLIENT_ID = _env("MEM_OIDC_CLIENT_ID", "memrizz")
SCOPE = _env("MEM_OIDC_SCOPE", "openid profile wallet offline_access")
LOOPBACK_PORT = int(_env("MEM_LOOPBACK_PORT", "3000"))
CALLBACK_PATH = "/auth/callback"
AUTH_FILE = os.path.expanduser(_env("MEM_AUTH_FILE", "~/.citrate/memories-auth.json"))
TIMEOUT = float(_env("MEM_CONNECT_TIMEOUT", "30"))


# --------------------------------------------------------------------------
# Small helpers
# --------------------------------------------------------------------------
def _b64url(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode("ascii")


def _jwt_payload(token):
    """Decode a JWT's payload WITHOUT verifying (local read of `sub`/`exp` only —
    the gateway does the real cryptographic verification)."""
    try:
        p = token.split(".")[1]
        p += "=" * (-len(p) % 4)
        return json.loads(base64.urlsafe_b64decode(p.encode("ascii")))
    except Exception:
        return {}


def _discover():
    """Fetch the OIDC discovery document for the authorize/token endpoints."""
    url = f"{ISSUER}/.well-known/openid-configuration"
    with urllib.request.urlopen(url, timeout=TIMEOUT) as r:
        d = json.loads(r.read().decode("utf-8"))
    return d["authorization_endpoint"], d["token_endpoint"]


def _post_form(url, fields):
    data = urllib.parse.urlencode(fields).encode("utf-8")
    req = urllib.request.Request(
        url, data=data, method="POST",
        headers={"Content-Type": "application/x-www-form-urlencoded",
                 "Accept": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
        return json.loads(r.read().decode("utf-8"))


def _save_auth(obj):
    os.makedirs(os.path.dirname(AUTH_FILE), exist_ok=True)
    tmp = AUTH_FILE + ".tmp"
    with open(tmp, "w") as f:
        json.dump(obj, f)
    os.chmod(tmp, 0o600)
    os.replace(tmp, AUTH_FILE)


def _load_auth():
    try:
        with open(AUTH_FILE) as f:
            return json.load(f)
    except Exception:
        return None


# --------------------------------------------------------------------------
# login — RFC 8252 loopback PKCE against the identity layer
# --------------------------------------------------------------------------
class _CallbackHandler(BaseHTTPRequestHandler):
    result = {}

    def do_GET(self):  # noqa: N802
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path != CALLBACK_PATH:
            self.send_response(404)
            self.end_headers()
            return
        q = urllib.parse.parse_qs(parsed.query)
        _CallbackHandler.result = {k: v[0] for k, v in q.items()}
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers()
        ok = "code" in _CallbackHandler.result
        msg = ("Signed in to Citrate Memories. You can close this tab and return "
               "to your terminal.") if ok else "Sign-in failed — check the terminal."
        self.wfile.write(
            f"<!doctype html><meta charset=utf-8><title>Citrate Memories</title>"
            f"<body style='font:16px system-ui;padding:3rem;max-width:32rem'>"
            f"<h2>{'✓ ' if ok else '✕ '}Citrate Memories</h2><p>{msg}</p></body>".encode()
        )

    def log_message(self, *_):  # silence the default stderr access log
        pass


def do_login():
    verifier = _b64url(secrets.token_bytes(48))
    challenge = _b64url(hashlib.sha256(verifier.encode("ascii")).digest())
    state = _b64url(secrets.token_bytes(16))
    redirect_uri = f"http://127.0.0.1:{LOOPBACK_PORT}{CALLBACK_PATH}"

    try:
        authorize_ep, token_ep = _discover()
    except Exception as e:
        _log(f"cannot reach identity discovery at {ISSUER} ({e})")
        return 2

    params = {
        "response_type": "code",
        "client_id": CLIENT_ID,
        "redirect_uri": redirect_uri,
        "scope": SCOPE,
        "state": state,
        "code_challenge": challenge,
        "code_challenge_method": "S256",
    }
    auth_url = authorize_ep + "?" + urllib.parse.urlencode(params)

    try:
        server = HTTPServer(("127.0.0.1", LOOPBACK_PORT), _CallbackHandler)
    except OSError as e:
        _log(f"cannot bind 127.0.0.1:{LOOPBACK_PORT} ({e}). Close whatever is using "
             f"that port, or set MEM_LOOPBACK_PORT to another registered loopback port.")
        return 2

    _log("opening your browser to sign in at the identity layer:")
    _log(f"  {ISSUER}")
    print(f"\nIf the browser does not open, paste this URL into it:\n\n{auth_url}\n",
          file=sys.stderr, flush=True)
    try:
        webbrowser.open(auth_url)
    except Exception:
        pass

    server.timeout = 300
    server.handle_request()  # blocks for the single callback (or times out)
    server.server_close()

    res = _CallbackHandler.result
    if res.get("state") != state:
        _log("state mismatch or timeout — sign-in aborted, nothing was saved.")
        return 2
    if "code" not in res:
        _log(f"no authorization code returned ({res.get('error', 'unknown error')}).")
        return 2

    try:
        tok = _post_form(token_ep, {
            "grant_type": "authorization_code",
            "code": res["code"],
            "redirect_uri": redirect_uri,
            "client_id": CLIENT_ID,
            "code_verifier": verifier,
        })
    except urllib.error.HTTPError as e:
        _log(f"token exchange failed: HTTP {e.code} {e.read().decode('utf-8','replace')[:300]}")
        return 2
    except Exception as e:
        _log(f"token exchange failed: {e}")
        return 2

    id_token = tok.get("id_token")
    if not id_token:
        _log("no id_token in the token response — cannot continue.")
        return 2
    claims = _jwt_payload(id_token)
    sub = claims.get("sub")
    if not sub:
        _log("id_token has no sub — cannot continue.")
        return 2

    _save_auth({
        "sub": sub,
        "id_token": id_token,
        "refresh_token": tok.get("refresh_token", ""),
        "id_token_exp": claims.get("exp", 0),
        "issuer": ISSUER,
        "client_id": CLIENT_ID,
        "gateway": GATEWAY,
    })
    who = claims.get("email") or claims.get("wallet_address") or sub
    _log(f"signed in as {who}. Credentials saved to {AUTH_FILE} (0600).")
    if not tok.get("refresh_token"):
        _log("note: no refresh token was issued; you may need to re-run `login` when "
             "the session expires.")
    _log("You can now use Citrate Memories from your agent — no token to paste.")
    return 0


# --------------------------------------------------------------------------
# bridge — MCP stdio  ->  gateway MCP-over-HTTP
# --------------------------------------------------------------------------
def _fresh_id_token(auth):
    """Return a currently-valid id_token, refreshing via the refresh token when it
    is within 60s of expiry. Persists rotated tokens. Returns (id_token, sub)."""
    now = int(time.time())
    if auth.get("id_token") and int(auth.get("id_token_exp", 0)) - 60 > now:
        return auth["id_token"], auth["sub"]
    rt = auth.get("refresh_token")
    if not rt:
        raise RuntimeError("session expired and no refresh token — run `login` again")
    _, token_ep = _discover()
    tok = _post_form(token_ep, {
        "grant_type": "refresh_token",
        "refresh_token": rt,
        "client_id": auth.get("client_id", CLIENT_ID),
    })
    id_token = tok.get("id_token")
    if not id_token:
        raise RuntimeError("refresh returned no id_token — run `login` again")
    claims = _jwt_payload(id_token)
    auth["id_token"] = id_token
    auth["id_token_exp"] = claims.get("exp", 0)
    if tok.get("refresh_token"):  # rotating refresh tokens
        auth["refresh_token"] = tok["refresh_token"]
    auth["sub"] = claims.get("sub", auth["sub"])
    try:
        _save_auth(auth)
    except Exception as e:
        _log(f"warning: could not persist refreshed token ({e})")
    return id_token, auth["sub"]


def _resolve_auth():
    """Decide how the bridge authenticates. Returns a callable () -> (bearer, sub).

    Precedence: (1) legacy MEM_CONNECT_TOKEN + MEM_USER_SUB env (unchanged old
    flow); (2) the login store (OIDC id_token, auto-refreshed)."""
    legacy_tok = os.environ.get("MEM_CONNECT_TOKEN", "").strip()
    legacy_sub = os.environ.get("MEM_USER_SUB", "").strip()
    if legacy_tok and legacy_sub:
        return lambda: (legacy_tok, legacy_sub)
    auth = _load_auth()
    if auth and (auth.get("refresh_token") or auth.get("id_token")):
        return lambda: _fresh_id_token(auth)
    _log("not signed in. Run:  python3 " + os.path.basename(__file__) + " login")
    _log("(or set MEM_CONNECT_TOKEN + MEM_USER_SUB for the legacy connect-token flow.)")
    sys.exit(2)


def _rpc_id(line):
    try:
        obj = json.loads(line)
    except (ValueError, TypeError):
        return False, None
    if isinstance(obj, dict) and "id" in obj:
        return True, obj["id"]
    return False, None


def _error_response(rpc_id, message):
    return json.dumps({"jsonrpc": "2.0", "id": rpc_id,
                       "error": {"code": -32000, "message": f"{_LOG}: {message}"}})


def _forward(url, bearer, line):
    req = urllib.request.Request(
        url, data=line.encode("utf-8"), method="POST",
        headers={"Authorization": f"Bearer {bearer}",
                 "Content-Type": "application/json",
                 "Accept": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        return resp.read().decode("utf-8", errors="replace")


def do_bridge():
    get_creds = _resolve_auth()
    out = sys.stdout
    # `iter(readline, "")` so each message dispatches the instant its newline
    # arrives (file iteration would read ahead and deadlock a request/response
    # stdio server).
    for raw in iter(sys.stdin.readline, ""):
        line = raw.strip()
        if not line:
            continue
        has_id, rpc_id = _rpc_id(line)
        try:
            bearer, sub = get_creds()
            url = f"{GATEWAY}/mcp/u/{urllib.parse.quote(sub, safe='')}"
            body = _forward(url, bearer, line)
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
        except (urllib.error.URLError, OSError, RuntimeError) as e:
            msg = f"cannot reach gateway ({GATEWAY}): {e}"
            _log(msg)
            if has_id:
                out.write(_error_response(rpc_id, msg) + "\n")
                out.flush()
            continue
        body = body.strip("\n")
        if body:
            out.write(body + "\n")
            out.flush()
    return 0


def main():
    args = sys.argv[1:]
    if args and args[0] in ("login", "--login", "auth"):
        return do_login()
    if args and args[0] in ("-h", "--help", "help"):
        print(__doc__)
        return 0
    return do_bridge()


if __name__ == "__main__":
    try:
        sys.exit(main())
    except BrokenPipeError:
        sys.exit(0)
    except KeyboardInterrupt:
        sys.exit(0)
