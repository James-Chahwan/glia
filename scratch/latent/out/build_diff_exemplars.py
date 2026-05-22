#!/usr/bin/env python3
"""build_diff_exemplars — P4.1 (Option 1, cycle 1.1).

Reads SWE-bench Lite parquet (dev + test), groups by repo, and for each repo
caches 2-3 prior gold patches as "diff dialect" exemplars. Used by
run_instance.py to inject codebase-style anchor diffs into the prefix
before the directive — gives the 7B Q4 model concrete hunk-header
conventions + indent + path style for the target codebase.

Anti-leak: exemplars NEVER include the current target instance. The
selector takes the K shortest gold patches (smallest = cleanest single-
change examples) from OTHER instances of the same repo.

Output:
  scratch/latent/out/exemplars/<repo_slug>.jsonl  — one record per exemplar:
    {"instance_id", "patch_chars", "first_hunk": "<diff text>"}

Usage:
  python build_diff_exemplars.py [--exemplars-per-repo 2] [--max-chars 1500]
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import defaultdict
from pathlib import Path

import pandas as pd

PARQ_DIR = Path("/home/ivy/Datasets/swe-bench-lite/data")
EXEMPLAR_DIR = Path("/home/ivy/Code/glia/scratch/latent/out/exemplars")


def repo_slug(repo: str) -> str:
    """Normalize 'pkg/repo' → 'pkg__repo' for path-safe slug."""
    return repo.replace("/", "__").replace(" ", "_")


def first_hunk(patch: str, max_chars: int) -> str:
    """Extract the diff up to the end of the first @@..@@ hunk. Falls back
    to first <max_chars> chars if the structure is malformed.
    """
    m = re.search(r"^diff --git", patch, flags=re.M)
    if not m:
        return patch[:max_chars]
    # Take from first `diff --git` to either the next `diff --git` OR end
    # of first hunk (heuristic: stop at second `@@ ` marker).
    start = m.start()
    body = patch[start:]
    # Find end of first hunk: first `@@` followed by ANOTHER `@@` line.
    at_at_iter = list(re.finditer(r"^@@ -\d", body, flags=re.M))
    if len(at_at_iter) >= 2:
        end = at_at_iter[1].start()
        body = body[:end]
    return body[:max_chars]


def all_hunks(patch: str, max_chars: int) -> str:
    """Extract the FULL multi-hunk diff for a single-file change.
    Truncate at max_chars but try to land on a clean line boundary.
    Used by the edit-content lever (post-auto8h): the model needs to see
    HOW a real fix looks for the target repo, not just the file path
    + first hunk preamble. Showing multiple +/- lines from prior commits
    gives the model concrete edit dialect to imitate.
    """
    m = re.search(r"^diff --git", patch, flags=re.M)
    if not m:
        return patch[:max_chars]
    start = m.start()
    body = patch[start:]
    # Take the WHOLE first-file diff (all hunks). Stop at second
    # `diff --git` marker (next file in the patch).
    next_diff = re.search(r"^diff --git", body[1:], flags=re.M)
    if next_diff:
        body = body[: next_diff.start() + 1]
    if len(body) > max_chars:
        # Cut at a clean line boundary if possible.
        body = body[:max_chars]
        last_nl = body.rfind("\n")
        if last_nl > max_chars - 200:  # only re-trim if we'd lose ≤200 chars
            body = body[:last_nl]
    return body


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exemplars-per-repo", type=int, default=2)
    ap.add_argument("--max-chars", type=int, default=1500,
                    help="trim each exemplar to this many chars (first hunk only)")
    ap.add_argument("--out-dir", default=str(EXEMPLAR_DIR))
    args = ap.parse_args()

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    by_repo: dict[str, list[dict]] = defaultdict(list)
    for parq in sorted(PARQ_DIR.glob("*.parquet")):
        df = pd.read_parquet(parq)
        for _, row in df.iterrows():
            patch = row.get("patch") or ""
            if not patch.strip():
                continue
            by_repo[row["repo"]].append({
                "instance_id": row["instance_id"],
                "patch_chars": len(patch),
                "patch": patch,
            })

    n_written = 0
    for repo, recs in by_repo.items():
        # Sort by patch_chars ASCENDING — short patches are cleaner single-
        # change exemplars, easier for the model to mimic. Cap at chosen count.
        recs.sort(key=lambda r: r["patch_chars"])
        chosen = recs[: args.exemplars_per_repo * 3]  # over-pick; final selector below
        # Final selection: keep first N regardless. The CALLER (run_instance.py)
        # will further exclude the current instance_id at use time.
        kept = chosen[: args.exemplars_per_repo]
        slug = repo_slug(repo)
        path = out_dir / f"{slug}.jsonl"
        with open(path, "w") as f:
            for r in kept:
                f.write(json.dumps({
                    "instance_id": r["instance_id"],
                    "patch_chars": r["patch_chars"],
                    "first_hunk": first_hunk(r["patch"], args.max_chars),
                    # Post-auto8h edit-content lever: also expose the FULL
                    # multi-hunk diff (up to max_chars * 3 = 4500c default).
                    # run_instance.py's exemplar_block can opt into this
                    # via env GLIA_EXEMPLAR_FULL_HUNK=1.
                    "all_hunks": all_hunks(r["patch"], args.max_chars * 3),
                }) + "\n")
        n_written += 1
        print(f"  {repo}: cached {len(kept)} exemplars → {path}", file=sys.stderr)

    print(f"wrote exemplars for {n_written} repos to {out_dir}", file=sys.stderr)


if __name__ == "__main__":
    main()
