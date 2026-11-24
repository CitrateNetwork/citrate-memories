#!/usr/bin/env python3
"""citrate-memories: compare two bge golden dumps (bge_golden_dump example).

Used by the bge-parity workflow to check that a dependency change (candle,
tokenizers, safetensors) leaves bge-base-en-v1.5 embeddings unchanged, so
stored indexes stay valid without a re-embed.

Pass criteria, the same as crates/mem-index/tests/bge_golden.rs:
  * token ids identical for every entry (any difference fails);
  * per entry, cosine(baseline, candidate) >= 0.99999 and
    max |baseline[i] - candidate[i]| <= 1e-5.

Usage: compare_bge_golden.py <baseline.json> <candidate.json> [--summary FILE]
       compare_bge_golden.py --self-test
Exit codes: 0 within tolerance, 1 a criterion failed or the inputs disagree.
"""
from __future__ import annotations

import json
import math
import struct
import sys

MIN_COSINE = 0.99999
MAX_ABS_DIFF = 1e-5


def vector(hexstr: str) -> list[float]:
    raw = bytes.fromhex(hexstr)
    return list(struct.unpack(f"<{len(raw) // 4}f", raw))


def cosine(a: list[float], b: list[float]) -> float:
    dot = sum(x * y for x, y in zip(a, b))
    na = math.sqrt(sum(x * x for x in a))
    nb = math.sqrt(sum(y * y for y in b))
    if na == 0.0 and nb == 0.0:
        return 1.0
    if na == 0.0 or nb == 0.0:
        return 0.0
    return dot / (na * nb)


def compare(base: dict, cand: dict) -> tuple[list[str], list[str]]:
    """(report lines, problems). No problems means within tolerance."""
    lines: list[str] = []
    problems: list[str] = []
    for key in ("format", "model_id", "dim"):
        if base.get(key) != cand.get(key):
            problems.append(f"{key} differs: {base.get(key)!r} vs {cand.get(key)!r}")
    bnames = [e["name"] for e in base["entries"]]
    cnames = [e["name"] for e in cand["entries"]]
    if bnames != cnames:
        problems.append(f"entry names differ: {bnames} vs {cnames}")
        return lines, problems
    worst_abs, worst_cos, bit_identical = 0.0, 1.0, 0
    lines.append(f"{'entry':<22} {'tokens':>6} {'max|diff|':>12} {'1-cosine':>12} {'bits':>5}")
    for b, c in zip(base["entries"], cand["entries"]):
        name = b["name"]
        if b["text"] != c["text"]:
            problems.append(f"{name}: input text differs")
        if b["ids"] != c["ids"]:
            first = next((i for i, (x, y) in enumerate(zip(b["ids"], c["ids"])) if x != y),
                         min(len(b["ids"]), len(c["ids"])))
            problems.append(f"{name}: token ids differ (len {len(b['ids'])} vs "
                            f"{len(c['ids'])}, first mismatch at {first})")
        vb, vc = vector(b["vector_f32le_hex"]), vector(c["vector_f32le_hex"])
        if len(vb) != len(vc):
            problems.append(f"{name}: dims differ {len(vb)} vs {len(vc)}")
            continue
        same = b["vector_f32le_hex"] == c["vector_f32le_hex"]
        bit_identical += same
        mad = max(abs(x - y) for x, y in zip(vb, vc))
        cs = cosine(vb, vc)
        worst_abs, worst_cos = max(worst_abs, mad), min(worst_cos, cs)
        lines.append(f"{name:<22} {len(b['ids']):>6} {mad:>12.3e} {1 - cs:>12.3e} "
                     f"{'same' if same else 'diff':>5}")
        if cs < MIN_COSINE:
            problems.append(f"{name}: cosine {cs:.9f} < {MIN_COSINE}")
        if mad > MAX_ABS_DIFF:
            problems.append(f"{name}: max abs diff {mad:.3e} > {MAX_ABS_DIFF}")
    lines.append(f"max abs diff {worst_abs:.3e} (limit {MAX_ABS_DIFF}); min cosine "
                 f"{worst_cos:.9f} (limit {MIN_COSINE}); bit-identical "
                 f"{bit_identical}/{len(bnames)}")
    return lines, problems


def self_test() -> int:
    def doc(vec: list[float], ids: list[int]) -> dict:
        hexv = struct.pack(f"<{len(vec)}f", *vec).hex()
        return {"format": "bge-golden/1", "model_id": "m", "dim": len(vec),
                "entries": [{"name": "a", "text": "t", "ids": ids, "vector_f32le_hex": hexv}]}
    v = [0.6, 0.8, 0.0]
    cases = [
        ("identical", doc(v, [1, 2]), doc(v, [1, 2]), False),
        ("ulp drift", doc(v, [1, 2]), doc([0.6000001, 0.8, 0.0], [1, 2]), False),
        ("ids differ", doc(v, [1, 2]), doc(v, [1, 3]), True),
        ("abs diff over limit", doc(v, [1, 2]), doc([0.6, 0.8, 2e-5], [1, 2]), True),
        ("cosine under limit", doc(v, [1, 2]), doc([0.8, 0.6, 0.0], [1, 2]), True),
    ]
    bad = [n for n, a, b, want in cases if bool(compare(a, b)[1]) != want]
    if bad:
        print(f"compare_bge_golden self-test FAILED: {bad}")
        return 1
    print(f"compare_bge_golden self-test ({len(cases)} oracles): ok")
    return 0


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    summary = None
    if "--summary" in argv:
        i = argv.index("--summary")
        summary = argv[i + 1]
        argv = argv[:i] + argv[i + 2:]
    if len(argv) != 2:
        print(__doc__)
        return 1
    with open(argv[0], encoding="utf-8") as f:
        base = json.load(f)
    with open(argv[1], encoding="utf-8") as f:
        cand = json.load(f)
    lines, problems = compare(base, cand)
    head = (f"baseline {base.get('label')!r} ({base.get('os')}/{base.get('arch')}) vs "
            f"candidate {cand.get('label')!r} ({cand.get('os')}/{cand.get('arch')})")
    out = [head, *lines, *(f"FAIL: {p}" for p in problems)]
    out.append("result: " + ("FAIL" if problems else "ok, within tolerance"))
    print("\n".join(out))
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write("```\n" + "\n".join(out) + "\n```\n")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
