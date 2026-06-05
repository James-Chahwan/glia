#!/usr/bin/env python3
"""attention_analyze — D2 companion to lens-attention Rust bin.

Reads the attention-norms JSONL produced by lens-attention and computes
the bloat-ratio metric:

  bloat_ratio = sum(norms at non-pool prompt positions) /
                sum(norms at pool prompt positions)

A high bloat means the model attended diffusely (pool present but the
prefix/suffix dominated). A low bloat (≪ 1) means pool tokens received
concentrated attention. The metric is computed per-layer + averaged
across the decision band (L25-27 by default per cycle 0.4 lens).

The pool-region span needs to be passed as --pool-token-range (the
token-index range of the pool relative to the prompt). Inferring it
from prefix.txt would require re-tokenization; for now caller supplies.

Output:
  - <out>/attention_bloat.md     summary + per-layer table
  - <out>/attention_bloat.jsonl  per-layer record

Usage:
  python attention_analyze.py --in path/to/attention.jsonl \\
      --pool-token-range 3000..4500 \\
      [--decision-band 25..28] [--out-dir out/]
"""
from __future__ import annotations

import argparse
import json
import sys
from collections import defaultdict
from pathlib import Path
from statistics import fmean


def parse_range(spec: str) -> tuple[int, int]:
    if "..=" in spec:
        lo, hi = spec.split("..=", 1)
        return int(lo), int(hi) + 1
    if ".." in spec:
        lo, hi = spec.split("..", 1)
        return int(lo), int(hi)
    raise ValueError(f"bad range spec: {spec}")


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--in", dest="in_path", required=True)
    ap.add_argument("--pool-token-range", required=True,
                    help="prompt-token-index range covering pool (e.g. 3000..4500)")
    ap.add_argument("--decision-band", default="25..28",
                    help="layer range to summarize (default 25..28 per cycle 0.4 lens)")
    ap.add_argument("--out-dir", default=None)
    args = ap.parse_args()

    in_path = Path(args.in_path)
    out_dir = Path(args.out_dir) if args.out_dir else in_path.parent
    out_dir.mkdir(parents=True, exist_ok=True)

    pool_lo, pool_hi = parse_range(args.pool_token_range)
    band_lo, band_hi = parse_range(args.decision_band)

    header = None
    by_layer: dict[int, list[tuple[int, float]]] = defaultdict(list)
    with open(in_path) as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            obj = json.loads(line)
            if obj.get("kind") == "Header":
                header = obj
                continue
            layer = obj.get("layer")
            pos = obj.get("position_idx")
            norm = obj.get("norm")
            if layer is None or pos is None or norm is None:
                continue
            by_layer[layer].append((pos, norm))

    if header is None:
        print(f"no header line in {in_path}", file=sys.stderr)
        return 1
    n_prompt = header.get("n_prompt_tokens", 0)
    tensor_names = header.get("attn_tensor_names_seen", [])
    print(f"header: n_prompt={n_prompt} tensor_names={tensor_names}", file=sys.stderr)
    print(f"loaded {sum(len(v) for v in by_layer.values())} records across "
          f"{len(by_layer)} layers", file=sys.stderr)

    # Per-layer bloat ratio.
    per_layer_records = []
    for layer in sorted(by_layer):
        rows = by_layer[layer]
        # Restrict to prompt-position rows only (positions < n_prompt). The
        # later rows are generation-step decoder outputs (single-token per
        # step) and aren't directly comparable.
        prompt_rows = [(p, n) for (p, n) in rows if p < n_prompt]
        pool_norms = [n for (p, n) in prompt_rows if pool_lo <= p < pool_hi]
        nonpool_norms = [n for (p, n) in prompt_rows if not (pool_lo <= p < pool_hi)]
        pool_sum = sum(pool_norms)
        nonpool_sum = sum(nonpool_norms)
        bloat = (nonpool_sum / pool_sum) if pool_sum > 0 else float("inf")
        per_layer_records.append({
            "layer": layer,
            "pool_n": len(pool_norms),
            "pool_sum": pool_sum,
            "pool_mean": fmean(pool_norms) if pool_norms else 0.0,
            "nonpool_n": len(nonpool_norms),
            "nonpool_sum": nonpool_sum,
            "nonpool_mean": fmean(nonpool_norms) if nonpool_norms else 0.0,
            "bloat_ratio": bloat,
        })

    # Decision-band aggregate.
    band_layers = [r for r in per_layer_records if band_lo <= r["layer"] < band_hi]
    band_bloat_mean = fmean(r["bloat_ratio"] for r in band_layers if r["bloat_ratio"] != float("inf")) if band_layers else 0.0
    band_pool_mean = fmean(r["pool_mean"] for r in band_layers) if band_layers else 0.0
    band_nonpool_mean = fmean(r["nonpool_mean"] for r in band_layers) if band_layers else 0.0

    md = []
    md.append(f"# Attention-bloat-ratio — {header.get('label', '?')}")
    md.append("")
    md.append(f"- pool token range: {pool_lo}..{pool_hi}  ({pool_hi - pool_lo} tokens)")
    md.append(f"- decision band: layers {band_lo}..{band_hi}")
    md.append(f"- generated tokens: {header.get('generated_token_count', '?')}")
    md.append(f"- attention tensor names that fired: `{tensor_names}`")
    md.append("")
    md.append(f"## Decision-band summary (L{band_lo}..{band_hi-1})")
    md.append(f"- mean bloat ratio (non-pool ÷ pool): **{band_bloat_mean:.3f}**")
    md.append(f"- mean pool-token attention norm: {band_pool_mean:.3f}")
    md.append(f"- mean non-pool-token attention norm: {band_nonpool_mean:.3f}")
    md.append("")
    md.append("Interpretation:")
    md.append("- bloat ≪ 1: pool tokens received concentrated attention (good signal)")
    md.append("- bloat ≈ 1: pool attended proportional to its share of tokens (neutral)")
    md.append("- bloat ≫ 1: pool was largely ignored relative to prefix/suffix (bloat)")
    md.append("")
    md.append("## Per-layer table")
    md.append("| layer | pool_n | pool_mean | nonpool_n | nonpool_mean | bloat |")
    md.append("|---|---|---|---|---|---|")
    for r in per_layer_records:
        md.append(f"| {r['layer']} | {r['pool_n']} | {r['pool_mean']:.3f} | "
                  f"{r['nonpool_n']} | {r['nonpool_mean']:.3f} | "
                  f"{r['bloat_ratio']:.3f} |")
    md_path = out_dir / (in_path.stem + "_bloat.md")
    md_path.write_text("\n".join(md) + "\n")

    jsonl_path = out_dir / (in_path.stem + "_bloat.jsonl")
    with open(jsonl_path, "w") as f:
        for r in per_layer_records:
            f.write(json.dumps(r))
            f.write("\n")

    print(f"wrote: {md_path}", file=sys.stderr)
    print(f"wrote: {jsonl_path}", file=sys.stderr)
    print(f"decision-band mean bloat ratio: {band_bloat_mean:.3f}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
