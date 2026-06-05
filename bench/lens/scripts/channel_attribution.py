#!/usr/bin/env python3
"""channel_attribution — A2, cycle 0.6 spitball bucket.

For each PASS in a cycle's results, identifies which directive channel(s)
named the gold target qname. Falsifies "B4 composer is doing real work":
if all PASSes attribute to a single channel, the composer is over-engineered
for the current corpus.

Inputs:
  - cycle/cycle-<N>-results.jsonl       (PASS instances)
  - per-instance workdir's directive_channels/{traceback,test_expectation,prose_mention}.md
    OR fallback workdir/directive.txt (legacy single-channel cycles ≤ 0.3)
  - gold patch from SWE-bench Lite parquet — extract gold qname via
    nearest-enclosing-def heuristic on touched line numbers.

Outputs:
  - cycle/cycle-<N>-channel-attribution.md   markdown table + hit-rate summary
  - cycle/cycle-<N>-channel-attribution.jsonl one record per PASS instance

Usage:
  python channel_attribution.py --cycle 0.7
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from dataclasses import dataclass, asdict, field
from pathlib import Path
from typing import Optional

import pandas as pd

GLIA = Path("/home/ivy/Code/glia")
LATENT_OUT = GLIA / "bench/latent/out"
CYCLE_DIR = GLIA / "bench/lens/cycle"
SWE_WORK = Path("/home/ivy/swe-work")
PARQUET_BASE = Path("/home/ivy/Datasets/swe-bench-lite/data")

CHANNEL_NAMES = ["traceback", "test_expectation", "prose_mention"]
LEGACY_CHANNEL = "legacy_traceback"  # cycle 0.3 single-channel form

# Match the bullet shape any synth bin emits: `- `qname` (`...`)`.
BULLET_QNAME_RE = re.compile(r"^[-*]\s*`([^`]+)`", re.M)
# Match Python function/class def lines: extracts the name.
PY_DEF_RE = re.compile(r"^\s*(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)", re.M)
# Diff hunk parser (file + new line range + context).
DIFF_FILE_RE = re.compile(r"^\+\+\+ b/(.+)$", re.M)
DIFF_GIT_RE = re.compile(r"^diff --git a/\S+ b/(\S+)$", re.M)
HUNK_HDR_RE = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,(\d+))? @@(.*)$")


@dataclass
class Attribution:
    instance_id: str
    cycle: str
    gold_qname: Optional[str]
    gold_files: list
    gold_lines: list
    primary_channel: Optional[str] = None
    channels_attributed: list = field(default_factory=list)
    channel_qnames: dict = field(default_factory=dict)
    legacy_form: bool = False
    notes: str = ""


def load_gold_patch(instance_id: str) -> Optional[str]:
    for split in ("test", "dev"):
        df = pd.read_parquet(PARQUET_BASE / f"{split}-00000-of-00001.parquet")
        hit = df[df.instance_id == instance_id]
        if len(hit):
            return hit.iloc[0].patch
    return None


def extract_gold_qname(instance_id: str, gold_patch: str) -> tuple[Optional[str], list, list]:
    """Heuristic: parse the gold patch's hunk header context (the trailing
    text after `@@`) — Python diff convention puts the enclosing def name
    there. Return (qname-tail, files, line ranges)."""
    files = sorted(set(DIFF_FILE_RE.findall(gold_patch)) | set(DIFF_GIT_RE.findall(gold_patch)))
    lines = []
    contexts = []
    cur_file = None
    for line in gold_patch.splitlines():
        m = DIFF_FILE_RE.match(line)
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
            ctx = (m.group(3) or "").strip()
            lines.append(f"{cur_file}:{start}-{start + count - 1}")
            # Extract Python def name from context.
            dm = PY_DEF_RE.match(ctx)
            if dm:
                contexts.append(dm.group(1))
            else:
                # Sometimes the context starts with "class Foo:" or "def bar(self):"
                # but is prefixed by whitespace + the function signature.
                bare = re.search(r"(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)", ctx)
                if bare:
                    contexts.append(bare.group(1))

    # If contexts is empty, fall back to nearest-enclosing-def search in
    # the actual repo file (if we have a clone).
    if not contexts and files:
        repo_dir = SWE_WORK / instance_id
        for f in files:
            full = repo_dir / f
            if full.exists():
                try:
                    src = full.read_text(errors="replace").splitlines()
                except Exception:
                    continue
                # For each gold line, walk upward to find the nearest def/class.
                for r in lines:
                    if not r.startswith(f"{f}:"):
                        continue
                    start = int(r.split(":")[1].split("-")[0])
                    for i in range(min(start, len(src)) - 1, -1, -1):
                        m = re.match(r"^\s*(?:def|class)\s+([A-Za-z_][A-Za-z0-9_]*)", src[i])
                        if m:
                            contexts.append(m.group(1))
                            break
    qname_tail = contexts[0] if contexts else None
    return qname_tail, files, lines


def locate_workdir(instance_id: str, model: str, cycle: str) -> Optional[Path]:
    candidates = [
        LATENT_OUT / f"inst-{instance_id}-{model}-cycle-{cycle}",
        LATENT_OUT / f"inst-{instance_id}-{model}",
    ]
    candidates += sorted(LATENT_OUT.glob(f"inst-{instance_id}-{model}-*cycle-{cycle}*"))
    for c in candidates:
        if c.exists():
            return c
    return None


def extract_channel_qnames(directive_text: str) -> set:
    """All backtick-quoted qnames in bullet lines of a directive."""
    return set(BULLET_QNAME_RE.findall(directive_text))


def qname_matches(qname: str, target_tail: str) -> bool:
    """A channel attribution match: the directive's qname's tail (after
    last `::`) equals the gold qname's tail, OR the gold tail appears as
    any segment of the directive qname."""
    if not target_tail:
        return False
    tails = qname.split("::")
    return target_tail in tails


def attribute_instance(record: dict) -> Attribution:
    instance_id = record["instance_id"]
    model = record.get("model", "7b-q4")
    cycle = str(record.get("cycle", "?"))
    gold = load_gold_patch(instance_id) or ""
    gold_tail, gold_files, gold_lines = extract_gold_qname(instance_id, gold)

    workdir = locate_workdir(instance_id, model, cycle)
    if workdir is None:
        return Attribution(
            instance_id=instance_id,
            cycle=cycle,
            gold_qname=gold_tail,
            gold_files=gold_files,
            gold_lines=gold_lines,
            notes="workdir not found",
        )

    attribution = Attribution(
        instance_id=instance_id,
        cycle=cycle,
        gold_qname=gold_tail,
        gold_files=gold_files,
        gold_lines=gold_lines,
    )

    channels_dir = workdir / "directive_channels"
    if channels_dir.exists():
        for ch in CHANNEL_NAMES:
            ch_path = channels_dir / f"{ch}.md"
            if not ch_path.exists():
                continue
            text = ch_path.read_text(errors="replace")
            qnames = extract_channel_qnames(text)
            attribution.channel_qnames[ch] = sorted(qnames)
            if gold_tail and any(qname_matches(q, gold_tail) for q in qnames):
                attribution.channels_attributed.append(ch)
    else:
        # Legacy single-channel form: parse the monolithic directive.txt.
        directive_path = workdir / "directive.txt"
        if directive_path.exists():
            attribution.legacy_form = True
            text = directive_path.read_text(errors="replace")
            qnames = extract_channel_qnames(text)
            attribution.channel_qnames[LEGACY_CHANNEL] = sorted(qnames)
            if gold_tail and any(qname_matches(q, gold_tail) for q in qnames):
                attribution.channels_attributed.append(LEGACY_CHANNEL)
        else:
            attribution.notes = "no directive files in workdir"

    # Primary channel inference (composer puts highest-score first; legacy
    # has only one).
    if attribution.channels_attributed:
        attribution.primary_channel = attribution.channels_attributed[0]

    return attribution


def render_markdown(cycle: str, attributions: list[Attribution]) -> str:
    s = []
    s.append(f"# Channel attribution — cycle {cycle}")
    s.append("")
    s.append(f"PASS instances analyzed: {len(attributions)}")
    s.append("")

    # Hit-rate per channel.
    channel_hits: Counter = Counter()
    for a in attributions:
        for ch in a.channels_attributed:
            channel_hits[ch] += 1

    if channel_hits:
        s.append("## Channel hit rate")
        s.append("| channel | hits | of PASSes | rate |")
        s.append("|---|---|---|---|")
        for ch in CHANNEL_NAMES + [LEGACY_CHANNEL]:
            if ch not in channel_hits:
                continue
            n = channel_hits[ch]
            s.append(f"| {ch} | {n} | {len(attributions)} | "
                     f"{100 * n / max(len(attributions), 1):.0f}% |")
        s.append("")

    s.append("## Per-instance attribution")
    s.append("| instance | gold_qname | primary | all_attributed | legacy? |")
    s.append("|---|---|---|---|---|")
    for a in attributions:
        s.append(
            f"| {a.instance_id} | "
            f"`{a.gold_qname or '?'}` | "
            f"{a.primary_channel or '—'} | "
            f"{', '.join(a.channels_attributed) or '—'} | "
            f"{'yes' if a.legacy_form else 'no'} |"
        )
    s.append("")

    s.append("## Interpretation")
    s.append("")
    s.append(
        "- **All PASSes attributed to one channel** → B4 composer is "
        "over-engineered for the current corpus; one synth bin carries the wins."
    )
    s.append(
        "- **PASSes attributed to multiple channels with cross-channel agreement** "
        "→ B4 composer is the load-bearing piece; redundancy lifts confidence."
    )
    s.append(
        "- **No channel attributed (gold_qname not in any channel's bullets)** "
        "→ either gold extraction failed (check gold_qname column) or the "
        "PASS came from sub-symbol guidance (line numbers, exception class) "
        "rather than named-target channels — diagnostic gap to close."
    )
    return "\n".join(s) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--cycle", required=True)
    ap.add_argument("--results", default=None)
    ap.add_argument("--out-dir", default=str(CYCLE_DIR))
    ap.add_argument("--include-fail", action="store_true",
                    help="also analyze FAIL instances (default: PASS only)")
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
            r = json.loads(line)
            if args.include_fail or r.get("f2p") == "PASS":
                records.append(r)

    print(f"loaded {len(records)} {'all' if args.include_fail else 'PASS'} records", file=sys.stderr)

    attributions = [attribute_instance(r) for r in records]

    md_path = out_dir / f"cycle-{args.cycle}-channel-attribution.md"
    md_path.write_text(render_markdown(args.cycle, attributions))

    jsonl_path = out_dir / f"cycle-{args.cycle}-channel-attribution.jsonl"
    with open(jsonl_path, "w") as f:
        for a in attributions:
            f.write(json.dumps(asdict(a)))
            f.write("\n")

    print(f"wrote: {md_path}", file=sys.stderr)
    print(f"wrote: {jsonl_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
