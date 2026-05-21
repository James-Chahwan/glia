#!/usr/bin/env python3
"""ablation_analyze — A3 companion to lens-ablate Rust bin.

Reads the ablation.jsonl produced by lens-ablate, ranks input tokens by
their influence on the model's greedy decode (Hamming-distance-based), and
emits a markdown summary + per-step heatmap PNG.

The Rust bin emits one Header line + one record line per masked-token
position. This analyzer aggregates them into actionable views.

Usage:
  python ablation_analyze.py --in path/to/ablation.jsonl [--out-dir out/]
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import Counter
from pathlib import Path


def load_ablation(path: Path):
    header = None
    records = []
    with open(path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            if obj.get("kind") == "Header":
                header = obj
            else:
                records.append(obj)
    return header, records


def render_markdown(header: dict, records: list, top_n: int = 20) -> str:
    s = []
    s.append(f"# Token-importance ablation — {header.get('label', '?')}")
    s.append("")
    s.append(f"- prompt tokens: {header.get('n_prompt_tokens', '?')}")
    s.append(f"- ablation range: {header.get('ablation_range', '?')}")
    s.append(f"- max_new (per pass): {header.get('max_new', '?')}")
    s.append(f"- mask token id: {header.get('mask_token_id', '?')}")
    s.append(f"- baseline generated tokens: {len(header.get('baseline_tokens', []))}")
    s.append("")

    ranked = sorted(records, key=lambda r: r["hamming_count"], reverse=True)

    s.append(f"## Top-{top_n} most impactful input tokens (highest Hamming on output)")
    s.append("| rank | token_idx | token_str | hamming | first_div_step |")
    s.append("|---|---|---|---|---|")
    for i, r in enumerate(ranked[:top_n], 1):
        ts = r["token_str"].replace("|", "\\|").replace("\n", "\\n")
        s.append(f"| {i} | {r['token_idx']} | `{ts}` | {r['hamming_count']} | "
                 f"{r['first_divergence_step'] if r['first_divergence_step'] is not None else '—'} |")
    s.append("")

    bottom = [r for r in records if r["hamming_count"] == 0]
    s.append(f"## Zero-impact tokens ({len(bottom)} of {len(records)})")
    s.append("These tokens, when masked, produced byte-identical output. Either "
             "redundant context or stopwords the model ignored.")
    s.append("")
    if bottom:
        sample = bottom[:30]
        s.append("First 30 zero-impact tokens:")
        s.append("| token_idx | token_str |")
        s.append("|---|---|")
        for r in sample:
            ts = r["token_str"].replace("|", "\\|").replace("\n", "\\n")
            s.append(f"| {r['token_idx']} | `{ts}` |")
        s.append("")

    # Aggregate stats
    hamming_counter = Counter(r["hamming_count"] for r in records)
    s.append("## Hamming-count histogram")
    s.append("| hamming | count of input tokens |")
    s.append("|---|---|")
    for h in sorted(hamming_counter):
        s.append(f"| {h} | {hamming_counter[h]} |")
    s.append("")

    total = len(records)
    nonzero = sum(1 for r in records if r["hamming_count"] > 0)
    pct = 100 * nonzero / max(total, 1)
    s.append(f"## Summary")
    s.append(f"- {nonzero}/{total} input tokens ({pct:.0f}%) had ANY effect on greedy decode.")
    s.append(f"- top-10 cumulative Hamming = "
             f"{sum(r['hamming_count'] for r in ranked[:10])} "
             f"(of {sum(r['hamming_count'] for r in records)} total)")
    if nonzero > 0:
        s.append("")
        s.append("**Interpretation**: tokens with high Hamming + early first_divergence_step "
                 "are the steering tokens. Tokens with zero Hamming are dead weight in the "
                 "current context — either consider compression or accept them as noise margin.")
    return "\n".join(s) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="in_path", required=True)
    ap.add_argument("--out-dir", default=None,
                    help="defaults to dir(--in)")
    ap.add_argument("--top-n", type=int, default=20)
    args = ap.parse_args()

    in_path = Path(args.in_path)
    out_dir = Path(args.out_dir) if args.out_dir else in_path.parent
    out_dir.mkdir(parents=True, exist_ok=True)

    header, records = load_ablation(in_path)
    if header is None:
        print(f"no header line in {in_path}", file=sys.stderr)
        return 1
    print(f"loaded {len(records)} ablation records", file=sys.stderr)

    md = render_markdown(header, records, top_n=args.top_n)
    md_path = out_dir / (in_path.stem + "_analysis.md")
    md_path.write_text(md)
    print(f"wrote: {md_path}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
