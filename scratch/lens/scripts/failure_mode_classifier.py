#!/usr/bin/env python3
"""failure_mode_classifier — A1, cycle 0.6 spitball bucket.

Reads a cycle's per-instance result JSONL + matching out.txt + gold patch
from SWE-bench Lite parquet, classifies each result into one of:

  PASS                      F2P reports PASS (already known).
  NO-DIFF                   out.txt empty / only code fences / single backtick.
  APPLY-FAIL                Diff parses but apply rejected.
  WRONG-TARGET              Diff applied but model-touched files disjoint from gold files.
  RIGHT-TARGET-WRONG-EDIT   Files overlap, but model line ranges miss gold line ranges.
  RIGHT-LINE-WRONG-CONTENT  Lines intersect gold's but tests still fail (semantic miss).

Outputs:
  cycle/cycle-<N>-failure-modes.md     histogram + per-instance table
  cycle/cycle-<N>-failure-modes.jsonl  per-instance tagged JSONL

Usage:
  python failure_mode_classifier.py --cycle 0.7
  python failure_mode_classifier.py --cycle 0.3 --out-dir cycle/
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Optional

import pandas as pd

GLIA = Path("/home/ivy/Code/glia")
LATENT_OUT = GLIA / "scratch/latent/out"
CYCLE_DIR = GLIA / "scratch/lens/cycle"
PARQUET_BASE = Path("/home/ivy/Datasets/swe-bench-lite/data")

HUNK_HDR_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@", re.M)
FILE_RE = re.compile(r"^\+\+\+ b/(.+)$", re.M)
# Fallback: some model outputs skip the +++ b/ block but still emit
# `diff --git a/<path> b/<path>` — extract from the b-path.
DIFF_GIT_RE = re.compile(r"^diff --git a/\S+ b/(\S+)$", re.M)


@dataclass
class Classification:
    instance_id: str
    cycle: str
    mode: str
    apply_status: str
    f2p_field: Optional[str]
    gold_files: list
    model_files: list
    gold_lines: list
    model_lines: list
    out_bytes: int
    diff_emitted: bool


def load_gold_patch(instance_id: str) -> Optional[str]:
    for split in ("test", "dev"):
        path = PARQUET_BASE / f"{split}-00000-of-00001.parquet"
        df = pd.read_parquet(path)
        hit = df[df.instance_id == instance_id]
        if len(hit):
            return hit.iloc[0].patch
    return None


def parse_diff_files(diff: str) -> list[str]:
    if not diff:
        return []
    files = set(FILE_RE.findall(diff))
    # Fallback to `diff --git a/X b/Y` when +++ b/ is missing.
    files.update(DIFF_GIT_RE.findall(diff))
    return sorted(files)


def parse_diff_line_ranges(diff: str) -> list[tuple[str, int, int]]:
    """Return list of (file, new_start_line, new_end_line) per hunk."""
    if not diff:
        return []
    out = []
    cur_file = None
    for line in diff.splitlines():
        m = FILE_RE.match(line)
        if m:
            cur_file = m.group(1)
            continue
        m = DIFF_GIT_RE.match(line)
        if m:
            cur_file = m.group(1)
            continue
        m = HUNK_HDR_RE.match(line)
        if m and cur_file:
            start = int(m.group(1))
            count = int(m.group(2) or 1)
            out.append((cur_file, start, start + count - 1))
    return out


def overlap(a: tuple[str, int, int], b: tuple[str, int, int]) -> bool:
    if a[0] != b[0]:
        return False
    return a[1] <= b[2] and b[1] <= a[2]


def is_no_diff(out_text: str) -> bool:
    s = out_text.strip()
    if not s:
        return True
    # The cycle 0.3 matplotlib-22835 NO-DIFF artifact was a single backtick.
    s2 = s.strip("`").strip()
    if not s2:
        return True
    # No `diff --git` or `--- a/` line means no actual diff content.
    if "diff --git" not in s and not s.startswith("--- a/"):
        return True
    return False


def locate_workdir(instance_id: str, model: str, cycle: str) -> Optional[Path]:
    """Find the per-instance run dir. Tries cycle-<N> tag form first."""
    candidates = [
        LATENT_OUT / f"inst-{instance_id}-{model}-cycle-{cycle}",
        LATENT_OUT / f"inst-{instance_id}-{model}",
    ]
    # Also try fuzzy: glob inst-<id>-<model>-* and take the most recent.
    glob_pattern = f"inst-{instance_id}-{model}-*cycle-{cycle}*"
    candidates += sorted(LATENT_OUT.glob(glob_pattern))
    for c in candidates:
        if c.exists() and (c / "out.txt").exists():
            return c
    return None


def classify_instance(record: dict) -> Classification:
    instance_id = record["instance_id"]
    model = record.get("model", "7b-q4")
    cycle = str(record.get("cycle", "?"))
    apply_status = str(record.get("apply", ""))
    f2p_field = record.get("f2p")

    workdir = locate_workdir(instance_id, model, cycle)
    out_text = ""
    if workdir is not None:
        out_path = workdir / "out.txt"
        if out_path.exists():
            out_text = out_path.read_text(errors="replace")

    gold = load_gold_patch(instance_id) or ""
    gold_files = parse_diff_files(gold)
    model_files = parse_diff_files(out_text)
    gold_ranges = parse_diff_line_ranges(gold)
    model_ranges = parse_diff_line_ranges(out_text)

    diff_emitted = not is_no_diff(out_text)

    # Cycle 1.4-no-channels exposed a bug: when f2p was a NO-RUN string
    # (build_ext fail, IndentationError, missing module), the classifier
    # treated the diff's gold-file/line overlap as if the test had run +
    # failed, bucketing it as RIGHT-TARGET-WRONG-EDIT or
    # RIGHT-LINE-WRONG-CONTENT. That inflates "we're close" stats —
    # NO-RUN means the test pipeline never executed at all.
    f2p_str = str(f2p_field or "")
    is_no_run = f2p_str.startswith("NO-RUN") or "NO-RUN" in f2p_str

    if not diff_emitted:
        mode = "NO-DIFF"
    elif apply_status == "NO-DIFF":
        mode = "NO-DIFF"
    elif "fail" in apply_status.lower() or apply_status == "NO-APPLY":
        mode = "APPLY-FAIL"
    elif is_no_run:
        mode = "NO-RUN"
    elif f2p_field == "PASS":
        mode = "PASS"
    else:
        # Diff applied; tests RAN; tests failed. Diagnose against gold.
        gold_file_set = set(gold_files)
        model_file_set = set(model_files)
        if not gold_file_set or not (gold_file_set & model_file_set):
            mode = "WRONG-TARGET"
        else:
            any_line_overlap = any(
                overlap(mr, gr) for mr in model_ranges for gr in gold_ranges
            )
            if not any_line_overlap:
                mode = "RIGHT-TARGET-WRONG-EDIT"
            else:
                mode = "RIGHT-LINE-WRONG-CONTENT"

    return Classification(
        instance_id=instance_id,
        cycle=cycle,
        mode=mode,
        apply_status=apply_status,
        f2p_field=f2p_field,
        gold_files=gold_files,
        model_files=model_files,
        gold_lines=[f"{f}:{s}-{e}" for (f, s, e) in gold_ranges],
        model_lines=[f"{f}:{s}-{e}" for (f, s, e) in model_ranges],
        out_bytes=len(out_text),
        diff_emitted=diff_emitted,
    )


def render_markdown(cycle: str, results: list[Classification]) -> str:
    counts = Counter(r.mode for r in results)
    total = len(results) or 1
    # Stable mode order for the histogram.
    mode_order = [
        "PASS",
        "RIGHT-LINE-WRONG-CONTENT",
        "RIGHT-TARGET-WRONG-EDIT",
        "WRONG-TARGET",
        "APPLY-FAIL",
        "NO-DIFF",
    ]
    s = []
    s.append(f"# Failure mode classification — cycle {cycle}")
    s.append("")
    s.append(f"Instances: {total}")
    s.append("")
    s.append("| Mode | Count | % |")
    s.append("|---|---|---|")
    for m in mode_order:
        c = counts.get(m, 0)
        s.append(f"| {m} | {c} | {100 * c / total:.0f}% |")
    # Any extra modes outside the ordered list.
    extras = [m for m in counts if m not in mode_order]
    for m in sorted(extras):
        c = counts[m]
        s.append(f"| {m} | {c} | {100 * c / total:.0f}% |")
    s.append("")
    s.append("## Per-instance table")
    s.append("| instance | mode | apply | f2p | model_files | gold_files | model_lines | gold_lines |")
    s.append("|---|---|---|---|---|---|---|---|")
    for r in sorted(results, key=lambda x: (x.mode, x.instance_id)):
        s.append(
            f"| {r.instance_id} | {r.mode} | {r.apply_status} | "
            f"{r.f2p_field or '—'} | "
            f"{', '.join(r.model_files) or '—'} | "
            f"{', '.join(r.gold_files) or '—'} | "
            f"{', '.join(r.model_lines) or '—'} | "
            f"{', '.join(r.gold_lines) or '—'} |"
        )
    s.append("")
    s.append("## Reading the histogram")
    s.append("")
    s.append("- **PASS** is the win condition.")
    s.append("- **NO-DIFF** means the model emitted no parseable diff — most likely a directive contradiction or context overflow. Diagnose via `out.txt` content.")
    s.append("- **APPLY-FAIL** means the diff is malformed or fuzz tolerance exceeded — typically a hunk-header line-number drift. See `feedback_swebench_apply_check_fuzz_first`.")
    s.append("- **WRONG-TARGET** means the model picked a different file. Indicates the directive's named target was unconvincing or absent.")
    s.append("- **RIGHT-TARGET-WRONG-EDIT** means the right file, wrong line region. Often a context-window distraction.")
    s.append("- **RIGHT-LINE-WRONG-CONTENT** is the closest miss — right place, wrong fix. The compositional gap cycle 0.4 lens identified.")
    return "\n".join(s) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cycle", required=True, help="cycle tag (e.g. 0.7)")
    ap.add_argument("--results", default=None,
                    help="path to cycle-<N>-results.jsonl (default: CYCLE_DIR/cycle-<cycle>-results.jsonl)")
    ap.add_argument("--out-dir", default=str(CYCLE_DIR))
    args = ap.parse_args()

    results_path = Path(args.results) if args.results else CYCLE_DIR / f"cycle-{args.cycle}-results.jsonl"
    if not results_path.exists():
        print(f"results file not found: {results_path}", file=sys.stderr)
        return 1

    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    records = []
    with open(results_path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            records.append(json.loads(line))
    print(f"loaded {len(records)} result records from {results_path}", file=sys.stderr)

    classifications = [classify_instance(r) for r in records]

    md_path = out_dir / f"cycle-{args.cycle}-failure-modes.md"
    md_path.write_text(render_markdown(args.cycle, classifications))

    jsonl_path = out_dir / f"cycle-{args.cycle}-failure-modes.jsonl"
    with open(jsonl_path, "w") as f:
        for c in classifications:
            f.write(json.dumps(asdict(c)))
            f.write("\n")

    counts = Counter(c.mode for c in classifications)
    print("Classification summary:", dict(counts), file=sys.stderr)
    print(f"wrote: {md_path}", file=sys.stderr)
    print(f"wrote: {jsonl_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
