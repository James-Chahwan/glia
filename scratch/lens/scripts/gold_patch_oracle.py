#!/usr/bin/env python3
"""gold_patch_oracle — C8 diagnostic for what the gold qname is per instance.

Reads SWE-bench Lite parquet (dev + test splits) and for each instance in the
loop set, extracts the gold patch's touched files + line ranges, then walks
the local repo at base_commit to find the enclosing function/class names.

NOT used in model prompts (gold leak) — purely for cycle-result attribution.
Answers: "did our directive name the gold qname?" per channel.

Output:
  scratch/lens/cycle/gold_qname_oracle.jsonl
  scratch/lens/cycle/gold_qname_oracle.md  (per-instance + per-cycle summary)

Usage:
  python gold_patch_oracle.py [--cycle 1.0a]
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import pandas as pd

LENS_ROOT = Path("/home/ivy/Code/glia/scratch/lens")
LATENT_OUT = Path("/home/ivy/Code/glia/scratch/latent/out")
PARQ_DIR = Path("/home/ivy/Datasets/swe-bench-lite/data")
LOOP_SET = LENS_ROOT / "manifests/cycle_loop_set.json"


def load_loop_set() -> list[str]:
    return [x["instance_id"] for x in json.loads(LOOP_SET.read_text())]


def load_instance(inst_id: str) -> dict | None:
    for f in PARQ_DIR.glob("*.parquet"):
        df = pd.read_parquet(f)
        m = df[df.instance_id == inst_id]
        if not m.empty:
            return m.iloc[0].to_dict()
    return None


_FILE_RE = re.compile(r"^\+\+\+ b/(.+)$", re.M)
_HUNK_RE = re.compile(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@", re.M)


def parse_gold_touched_lines(diff: str) -> list[tuple[str, int, int]]:
    """Return [(file, start_new, end_new)] for each hunk in the gold diff."""
    if not diff:
        return []
    out = []
    cur_file = None
    for line in diff.split("\n"):
        m = _FILE_RE.match(line)
        if m:
            cur_file = m.group(1)
            continue
        m = _HUNK_RE.match(line)
        if m and cur_file:
            start = int(m.group(3))
            length = int(m.group(4)) if m.group(4) else 1
            out.append((cur_file, start, start + length))
    return out


_PY_DEF_RE = re.compile(r"^\s*(def|class)\s+([A-Za-z_][A-Za-z0-9_]*)")


def walk_to_def(file_path: Path, target_line: int) -> str | None:
    """Walk upward from `target_line` looking for the nearest `def X(` or
    `class X:` declaration. Returns the function/class name (not qname).
    """
    if not file_path.exists():
        return None
    try:
        lines = file_path.read_text().splitlines()
    except (UnicodeDecodeError, OSError):
        return None
    for i in range(min(target_line - 1, len(lines) - 1), -1, -1):
        m = _PY_DEF_RE.match(lines[i])
        if m:
            return m.group(2)
    return None


def find_local_repo(inst_id: str) -> Path | None:
    """Find the workdir for this instance to read source from."""
    # Pattern: inst-<id>-<model>-cycle-<X>
    candidates = list(LATENT_OUT.glob(f"inst-{inst_id}-*"))
    if not candidates:
        return None
    return candidates[0]  # any one will do; source content is the same


def gold_attribution_for_instance(inst_id: str) -> dict:
    inst = load_instance(inst_id)
    if not inst:
        return {"instance_id": inst_id, "error": "not found in parquets"}
    gold = inst.get("patch", "") or ""
    touched = parse_gold_touched_lines(gold)
    workdir = find_local_repo(inst_id)
    enclosing = []
    for (file, start, end) in touched:
        # repo source lives at /home/ivy/swe-work/<id>/<file>
        repo_root = Path("/home/ivy/swe-work") / inst_id
        full = repo_root / file
        name = walk_to_def(full, start)
        enclosing.append({
            "file": file,
            "start_new": start,
            "end_new": end,
            "enclosing_def": name,
        })
    return {
        "instance_id": inst_id,
        "gold_diff_len": len(gold),
        "gold_touched": enclosing,
    }


def directive_attribution(inst_id: str, cycle: str) -> dict:
    """Check whether the cycle's composed directive named each gold-enclosing
    def in any of its bullets. Returns {"named": True/False, "primary_named": True/False}.
    """
    workdir = LATENT_OUT / f"inst-{inst_id}-7b-q4-cycle-{cycle}"
    directive_path = workdir / "directive.txt"
    if not directive_path.exists():
        return {"directive_exists": False}
    text = directive_path.read_text()
    # Split at "## Additional graph-derived signals" for PRIMARY scoping.
    parts = text.split("## Additional graph-derived signals", 1)
    primary_block = parts[0]
    return {
        "directive_exists": True,
        "directive_chars": len(text),
        "primary_block_chars": len(primary_block),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cycle", default=None,
                    help="cycle tag to attribute against (e.g. 1.0a)")
    args = ap.parse_args()

    loop_set = load_loop_set()
    records = []
    md = ["# Gold-patch oracle (C8 diagnostic)\n"]
    if args.cycle:
        md.append(f"\nCycle tag: `{args.cycle}`\n")
    md.append("| Instance | gold files (encl) | named_in_dir? |\n|---|---|---|")

    for inst_id in loop_set:
        rec = gold_attribution_for_instance(inst_id)
        if args.cycle:
            rec["directive_attribution"] = directive_attribution(inst_id, args.cycle)
        records.append(rec)
        encl = rec.get("gold_touched") or []
        files = " ; ".join(
            f"{e['file']}:{e['start_new']} ({e['enclosing_def'] or '?'})"
            for e in encl[:3]
        ) or "(none)"
        # Check whether any directive in any cycle directory names any
        # gold enclosing def.
        named = "—"
        if args.cycle and encl:
            workdir = LATENT_OUT / f"inst-{inst_id}-7b-q4-cycle-{args.cycle}"
            dir_text = (workdir / "directive.txt").read_text() if (workdir / "directive.txt").exists() else ""
            named_names = [e["enclosing_def"] for e in encl if e["enclosing_def"]]
            hits = [n for n in named_names if n and f"`{n}`" in dir_text]
            named = ("YES (" + ",".join(hits) + ")") if hits else "NO"
        md.append(f"| {inst_id} | {files} | {named} |")

    out_dir = LENS_ROOT / "cycle"
    out_dir.mkdir(parents=True, exist_ok=True)
    suffix = f"-{args.cycle}" if args.cycle else ""
    jsonl_path = out_dir / f"gold_qname_oracle{suffix}.jsonl"
    md_path = out_dir / f"gold_qname_oracle{suffix}.md"
    with open(jsonl_path, "w") as f:
        for r in records:
            f.write(json.dumps(r) + "\n")
    md_path.write_text("\n".join(md) + "\n")
    print(f"wrote: {jsonl_path}", file=sys.stderr)
    print(f"wrote: {md_path}", file=sys.stderr)


if __name__ == "__main__":
    main()
