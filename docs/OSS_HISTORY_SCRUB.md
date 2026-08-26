# OSS history scrub — pre-PUBLIC checklist

**Status:** runbook (owner-executed at the PRIVATE → PUBLIC flip)
**Why:** the working tree at HEAD is sanitized (deploy/ templated in #18, roadmap in
this PR), but the same infra identifiers still live in **prior commits**. Flipping the
repo public exposes full history, so history must be cleaned first. This is a
destructive rewrite + force-push, so it is owner-approved (Rule 10) and done last.

## What leaks (verified 2026-08-25)

- **89 commits total.** The sensitive values appear in ~5 commits, confined to four
  files ever: `crates/mem-gateway/src/ingest_worker.rs`, `deploy/mem-gateway.service`,
  `deploy/README.md`, `docs/V1_COMPLETION_AND_OSS_ROADMAP.md` — plus the tracked
  handoff `handoffs/MEM_GATEWAY_P3_P4_UNBLOCK_HANDOFF_2026-07-19.md`.
- The values are **infra identifiers, not credentials**: a private-tailnet IP, the
  DGX hostname, the droplet's public IP, and a personal `$HOME` path. Moderate
  sensitivity — clean them, but this is disclosure hygiene, not a key leak.

The literal values are intentionally NOT listed in this committed doc (that would
re-introduce them). They live in the private handoff above and in the agent memory
note `citrate-memories-reactivation-2026-08-25`.

## Option A — fresh-history public mirror (recommended)

Simplest and leak-proof in one move: publish a NEW public repo whose history starts
clean. Since v1 ships ahead of the chain and the private history has little external
value, a single "initial public release" commit is usually right.

```bash
# from a clean checkout of sanitized main
rm -rf .git
git init && git add -A
git commit -m "citrate-memories v1 — initial public release"
git remote add origin git@github.com:CitrateNetwork/citrate-memories-oss.git   # new PUBLIC repo
git push -u origin main
```

Verify no leaks survive (see checks below) before the first push.

## Option B — surgical scrub with git-filter-repo (preserves history)

Use when you want to keep the commit history. `git-filter-repo` is already installed
(`/opt/homebrew/bin/git-filter-repo`); otherwise `pipx install git-filter-repo`.

1. Create an **uncommitted** replacements file (this file is gitignored — see
   `.gitignore`). One `literal==>replacement` per line:

   ```
   # ~/oss-scrub-replacements.txt   (DO NOT COMMIT)
   <DGX_TAILNET_IP>==><TAILNET_IP>
   <DROPLET_PUBLIC_IP>==><EDGE_IP>
   <DGX_HOSTNAME>==><BACKEND_HOST>
   /home/<user>==>$HOME
   ```

2. Run on a **fresh mirror** (filter-repo refuses a repo with a remote by default;
   a fresh clone is safest):

   ```bash
   git clone --no-local . /tmp/mem-scrub && cd /tmp/mem-scrub
   git filter-repo \
     --replace-text ~/oss-scrub-replacements.txt \
     --invert-paths --path handoffs/MEM_GATEWAY_P3_P4_UNBLOCK_HANDOFF_2026-07-19.md
   ```

   `--replace-text` redacts the strings across every commit; `--invert-paths` deletes
   the handoff from all history.

3. Verify, then push to the public remote (a new repo, or force-push with explicit
   owner approval — Rule 10).

## Verification (run before any public push)

```bash
# every value from the replacements file must return ZERO across ALL history
for v in "<DGX_TAILNET_IP>" "<DROPLET_PUBLIC_IP>" "<DGX_HOSTNAME>" "/home/<user>"; do
  n=$(git log --all -S"$v" --oneline | wc -l)
  echo "$v -> $n commits (must be 0)"
done
# the handoff must be gone from history
git log --all --oneline -- handoffs/ | head
# run the repo's gitleaks config as a backstop
gitleaks detect --config .gitleaks.toml --no-banner
```

Only flip the GitHub visibility to PUBLIC after these pass and the Rule-13
PRIVATE→PUBLIC review sign-off is recorded.
