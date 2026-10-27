#!/usr/bin/env python3
"""citrate-memories: keep `.cargo/audit.toml` honest.

`cargo audit` exits 0 once an advisory is in the ignore list, so an accepted
advisory disappears from every report. This check fails the CI audit step when:

1. the file has ignores but no `# EXPIRES: YYYY-MM-DD` line;
2. the expiry has passed (forces a fresh decision, not renewal by default);
3. an ignored advisory is no longer reported against Cargo.lock (the
   dependency moved or the advisory was withdrawn: delete the stale ignore).

`cargo audit` drops ignored advisories from its output, so the unfiltered
report comes from a copy of Cargo.lock in a directory with no audit.toml.

Usage: check_audit_ignores.py [--self-test]
Exit codes: 0 ok, 1 a rule above failed.
"""
import json
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import date
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
AUDIT_TOML = ROOT / ".cargo" / "audit.toml"


def parse(text: str) -> tuple[list[str], date | None]:
    """Ignored advisory ids (from the ignore list only) and the EXPIRES date."""
    ids: list[str] = []
    m = re.search(r"^ignore\s*=\s*\[(.*?)^\]", text, re.S | re.M)
    if m:
        body = "\n".join(line.split("#", 1)[0] for line in m[1].splitlines())
        ids = re.findall(r'"(RUSTSEC-\d{4}-\d{4})"', body)
    e = re.search(r"^#\s*EXPIRES:\s*(\d{4})-(\d{2})-(\d{2})", text, re.M)
    expiry = date(int(e[1]), int(e[2]), int(e[3])) if e else None
    return ids, expiry


def evaluate(text: str, reported: set[str], today: date) -> list[str]:
    """Every reason the ignore file is not honest. Empty means ok."""
    ignored, expiry = parse(text)
    if not ignored:
        return []
    problems = []
    if expiry is None:
        problems.append("audit.toml has ignores but no `# EXPIRES: YYYY-MM-DD` line")
    elif today > expiry:
        problems.append(f"the accepted advisories EXPIRED on {expiry}: re-decide each "
                        f"one, then move the date. Do not renew by reflex.")
    stale = [i for i in ignored if i not in reported]
    if stale:
        problems.append("ignored but no longer reported, delete from .cargo/audit.toml: "
                        + ", ".join(stale))
    return problems


def unfiltered_report() -> set[str]:
    with tempfile.TemporaryDirectory() as tmp:
        shutil.copy(ROOT / "Cargo.lock", Path(tmp) / "Cargo.lock")
        proc = subprocess.run(["cargo", "audit", "--json", "-f", "Cargo.lock"],
                              cwd=tmp, capture_output=True, text=True)
    if not proc.stdout.strip():
        print(f"  cargo audit produced no JSON: {proc.stderr.strip()[:400]}")
        sys.exit(1)
    report = json.loads(proc.stdout)
    return {v["advisory"]["id"] for v in report["vulnerabilities"]["list"]}


def self_test() -> int:
    good = ('# EXPIRES: 2026-11-09\n[advisories]\nignore = [\n'
            '    "RUSTSEC-2026-0367", # not reachable: "RUSTSEC-2026-9999" in a comment\n]\n')
    day = date(2026, 10, 23)
    cases = [
        ("current, all reported", good, {"RUSTSEC-2026-0367"}, day, False),
        ("expired", good, {"RUSTSEC-2026-0367"}, date(2026, 11, 10), True),
        ("expiry day itself still valid", good, {"RUSTSEC-2026-0367"}, date(2026, 11, 9), False),
        ("stale ignore", good, set(), day, True),
        ("no expiry line", good.replace("# EXPIRES: 2026-11-09\n", ""),
         {"RUSTSEC-2026-0367"}, day, True),
        ("expiry only in prose", good.replace("# EXPIRES:", "# was EXPIRES:"),
         {"RUSTSEC-2026-0367"}, day, True),
        ("no ignores", "[advisories]\nignore = [\n]\n", set(), day, False),
    ]
    ok = True
    for name, text, reported, today, should_fail in cases:
        failed = bool(evaluate(text, reported, today))
        if failed != should_fail:
            ok = False
            print(f"self-test: {name}: expected {'fail' if should_fail else 'pass'}, "
                  f"got {'fail' if failed else 'pass'}")
    print(f"audit-ignore check self-test ({len(cases)} oracles):", "ok" if ok else "FAILED")
    return 0 if ok else 1


def main() -> int:
    if "--self-test" in sys.argv:
        return self_test()
    if not AUDIT_TOML.exists():
        print("  no .cargo/audit.toml: nothing is ignored")
        return 0
    text = AUDIT_TOML.read_text()
    problems = evaluate(text, unfiltered_report(), date.today())
    for p in problems:
        print(f"  {p}")
    if problems:
        return 1
    ignored, expiry = parse(text)
    print(f"  {len(ignored)} accepted, expires {expiry}, all still reported")
    return 0


if __name__ == "__main__":
    sys.exit(main())
