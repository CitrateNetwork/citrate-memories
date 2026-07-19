---
title: Citrate Memories — Team Install Guide (non-engineer friendly)
created: 2026-07-19
audience: anyone on the team who uses a frontier-model agent (Claude, etc.) or a browser
status: draft — Option B endpoints go live with the gateway rollout (P4)
---

# Using Citrate Memories

Citrate Memories is the team's shared memory of every Citrate repo. It answers
questions like "what's the latest on citrate-radar?" or "what did we decide about
the 40204 chain id?" in seconds, without anyone reading the code. It never serves
you stale or contradicted information: every answer carries how fresh it is and
whether it has been superseded.

There are two ways to use it. Pick the one that fits how you work.

---

## Option A — In your browser (nothing to install)

This is the easiest path and needs no setup.

1. Go to **https://memrizz.citrate.ai**.
2. Sign in with your Citrate account (the same login as the rest of our tools).
3. You will see the constellation view of the federation. Use the search box, or
   click a repo to read its storyline.

That is it. If you can log in, you are done. Use this if you mostly want to look
things up yourself.

> If sign-in fails, it is almost always an account/permissions issue, not you.
> Message the team and include a screenshot of the error.

---

## Option B — Inside your AI agent (talk to it, it answers from memory)

This lets your agent (Claude Code, or any agent that supports MCP) pull from the
shared memory while it works for you. You do not need to be an engineer. You copy
two values in once, and after that you just talk to your agent normally.

### What you need (one-time)

1. **Your personal connect token.** In the Memrizz webapp (Option A), open
   **Settings → Connect an agent**. Click **Generate token** and copy it. It is a
   long secret string. Treat it like a password. Do not paste it into chats or
   share it.
2. **The gateway address:** `https://mem-gateway.citrate.ai`.

### Install it by asking your agent

You can literally ask your agent to set this up. Paste this to it (fill in your
token where shown):

> "Please add the Citrate Memories MCP server to my configuration. It runs at
> `https://mem-gateway.citrate.ai`, my connect token is `PASTE_YOUR_TOKEN_HERE`,
> and my user id is the one shown in my Memrizz profile. Use the citrate-memories
> connector helper. After adding it, confirm you can call `memory.search`."

Your agent will add an entry to its MCP settings and confirm the connection. From
then on, you can ask things like:

- "Using citrate memories, what's the latest on citrate-radar?"
- "Recall the storyline for core-membership."
- "Has anything we decided about the treasury signer been superseded?"

The agent will show you the answer along with a freshness note (for example
"HEAD abc123, ingested 2 minutes ago") so you always know how current it is.

### If you prefer to paste config yourself

Add this to your agent's MCP config (in Claude Code, that is `.mcp.json` in your
project, or the global MCP settings). Replace the two placeholder values:

```json
{
  "mcpServers": {
    "citrate-memories": {
      "type": "http",
      "url": "https://mem-gateway.citrate.ai/mcp/u/YOUR_USER_ID",
      "headers": { "Authorization": "Bearer YOUR_CONNECT_TOKEN" }
    }
  }
}
```

> Status note (2026-07-19): the direct `http` transport above depends on the
> gateway exposing the standard MCP Streamable HTTP transport, which is part of
> the P2 spec-alignment work. Until that ships, teammates who want the in-agent
> path use a tiny connector helper we provide (a one-line install that speaks to
> your agent locally and forwards to the gateway with your token). Ask the team
> for the current connector command. Option A (browser) works today.

---

## Talking to it well

- Name the repo when you can ("in citrate-comms, ...") — answers get sharper.
- Ask for the storyline, not just a fact, when you are getting oriented:
  "recall the storyline for citrate-atlas."
- If an answer looks old, ask "is this still current?" — the memory can tell you
  whether it has been superseded or contradicted.

## Troubleshooting

| Symptom | What it means | Fix |
| --- | --- | --- |
| "connect token rejected" or 401 | your token expired or was mistyped | regenerate it in Memrizz → Settings → Connect an agent, paste the new one |
| "no membership in org" or 403 | your account is not yet added to the org | ask the team to add you (owner console) |
| answers seem stale | the live feed may be catching up | check the freshness note; if it is far behind, tell the team |

## Keep your token safe

Your connect token acts as you. Never paste it into a shared chat, a public repo,
or a screenshot. If it leaks, regenerate it (that instantly invalidates the old
one).
