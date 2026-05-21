#!/usr/bin/env python3
"""build_file_priors — P4.4 (Option 6, cycle 1.1).

Aggregate "which files do gold patches touch?" per repo across SWE-bench
Lite (dev + test). Cached as scratch/latent/out/file_priors/<repo>.json.
run_instance.py reads this and surfaces top-3 as a soft routing hint:
"In <repo>, fixes most commonly touch X (67%), Y (22%), Z (8%)."

Anti-leak: file priors are aggregated across all instances of a repo,
but caller (run_instance.py) excludes the CURRENT instance at read time.

Usage:
  python build_file_priors.py [--top-n 3]
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

import pandas as pd

PARQ_DIR = Path("/home/ivy/Datasets/swe-bench-lite/data")
OUT_DIR = Path("/home/ivy/Code/glia/scratch/latent/out/file_priors")

FILE_RE = re.compile(r"^\+\+\+ b/(.+)$", re.M)


def files_touched(patch: str) -> list[str]:
    return FILE_RE.findall(patch or "")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--top-n", type=int, default=3)
    ap.add_argument("--out-dir", default=str(OUT_DIR))
    args = ap.parse_args()

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    # per-repo: list of (instance_id, [files...])
    by_repo: dict[str, list[tuple[str, list[str]]]] = defaultdict(list)
    for parq in sorted(PARQ_DIR.glob("*.parquet")):
        df = pd.read_parquet(parq)
        for _, row in df.iterrows():
            files = files_touched(row.get("patch") or "")
            if files:
                by_repo[row["repo"]].append((row["instance_id"], files))

    for repo, instances in by_repo.items():
        # Aggregate file → instance set (deduped per instance: 1 vote per inst).
        file_to_instances: dict[str, set] = defaultdict(set)
        for inst_id, files in instances:
            for f in set(files):
                file_to_instances[f].add(inst_id)
        # Total instances for this repo (denominator).
        n_total = len(instances)
        # Convert to count + pct.
        rows = [
            {"file": f, "instance_count": len(insts), "pct": 100.0 * len(insts) / n_total}
            for f, insts in file_to_instances.items()
        ]
        rows.sort(key=lambda r: (-r["instance_count"], r["file"]))
        slug = repo.replace("/", "__")
        path = out_dir / f"{slug}.json"
        with open(path, "w") as f:
            json.dump({
                "repo": repo,
                "n_total_instances": n_total,
                "files": rows[:50],  # cap to top-50 per repo
                # Include the per-file → instances map so the run_instance
                # caller can exclude its own instance at read time.
                "by_file_to_instances": {f: sorted(insts)
                                          for f, insts in file_to_instances.items()},
            }, f)
        print(f"  {repo}: {n_total} instances, top files: "
              f"{', '.join(r['file'] for r in rows[:3])} → {path.name}",
              file=sys.stderr)

    print(f"wrote priors for {len(by_repo)} repos to {out_dir}", file=sys.stderr)


if __name__ == "__main__":
    main()
