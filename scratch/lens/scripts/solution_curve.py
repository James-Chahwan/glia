#!/usr/bin/env python3
"""solution_curve — Matrix Option C analyzer for cycle / smoke results.

Reads per-instance beam_matrix.jsonl files written by run_instance.py
(matrix Option C beam path) plus the cycle-level results.jsonl, computes
per-instance pass@k curves and sage-pass marginal-gain table, fits a
saturating geometric model, and reports asymptote + budget-to-90% / 99%.

Inputs:
  --cycle <tag>      cycle / smoke tag (e.g. "1.1" or "smoke-phase1")
  --out-dir <path>   default: scratch/lens/cycle/

Reads:
  scratch/lens/cycle/<cycle>-results.jsonl        (per-instance final outcome)
  scratch/latent/out/inst-<id>-7b-q4-cycle-<tag>/beam_matrix.jsonl
                                                  (per-candidate apply+test rows)

Output:
  <out_dir>/solution_curve-<cycle>.jsonl   per-instance curve fit + summary
  <out_dir>/solution_curve-<cycle>.md      human-readable table

The geometric model fits p_one (single-sample PASS probability) such that
pass@k = 1 - (1 - p_one)^k. Asymptote is 1.0 if p_one > 0, else 0.
For sage-pass dimension: pass@N is cumulative across pass-1, pass-2, pass-3.
"""
from __future__ import annotations

import argparse
import json
import math
import re
import sys
from collections import defaultdict
from pathlib import Path

LENS_ROOT = Path("/home/ivy/Code/glia/scratch/lens")
LATENT_OUT = Path("/home/ivy/Code/glia/scratch/latent/out")
SMOKE_PREFIX = "smoke-"


def is_pass(rec: dict) -> bool:
    """Strict PASS: F2P all pass AND P2P fail == 0."""
    f2p = rec.get("f2p") or ""
    p2p_fail = rec.get("p2p_fail") or rec.get("reg_fail") or 0
    return f2p == "PASS" and p2p_fail == 0


def is_lossy_pass(rec: dict) -> bool:
    """Lossy PASS: F2P all pass (P2P may regress)."""
    f2p = rec.get("f2p") or ""
    return f2p == "PASS" or "regressions" in f2p


def load_cycle_results(cycle: str) -> list[dict]:
    """Try both cycle-<tag>-results.jsonl and smoke-<tag>-results.jsonl."""
    candidates = [
        LENS_ROOT / "cycle" / f"cycle-{cycle}-results.jsonl",
        LENS_ROOT / "cycle" / f"smoke-{cycle}-results.jsonl",
    ]
    if cycle.startswith(SMOKE_PREFIX):
        candidates.insert(0, LENS_ROOT / "cycle" / f"{cycle}-results.jsonl")
    for p in candidates:
        if p.exists():
            return [json.loads(l) for l in p.read_text().splitlines() if l.strip()]
    raise FileNotFoundError(f"no cycle/smoke results for {cycle!r} — looked at {candidates}")


def load_beam_matrix(instance_id: str, cycle: str) -> list[dict]:
    """Find beam_matrix.jsonl for this instance + cycle tag."""
    tag = cycle if cycle.startswith("cycle") or cycle.startswith(SMOKE_PREFIX) else f"cycle-{cycle}"
    workdir = LATENT_OUT / f"inst-{instance_id}-7b-q4-{tag}"
    p = workdir / "beam_matrix.jsonl"
    if not p.exists():
        # Try the run_instance tag form (sometimes lacks the 7b-q4 segment).
        alt = list(LATENT_OUT.glob(f"inst-{instance_id}-*{tag}/beam_matrix.jsonl"))
        if alt:
            p = alt[0]
        else:
            return []
    return [json.loads(l) for l in p.read_text().splitlines() if l.strip()]


def fit_pass_at_k(beam_records: list[dict]) -> dict:
    """Fit geometric pass@k from a list of beam-tested candidates.

    Each record has 'apply' / 'f2p' / 'p2p_fail'. Single-sample p is the
    fraction that strict-passed; pass@k = 1 - (1-p)^k.

    Returns:
      {
        "n_candidates": N,
        "n_pass_strict": ...,
        "n_pass_lossy": ...,
        "p_one_strict": ...,
        "p_one_lossy": ...,
        "k_to_90_strict": k_or_inf,
        "k_to_99_strict": k_or_inf,
        "asymptote": 1.0 if p>0 else 0,
      }
    """
    n = len(beam_records)
    n_pass = sum(1 for r in beam_records if is_pass(r))
    n_pass_lossy = sum(1 for r in beam_records if is_lossy_pass(r))

    p_strict = n_pass / n if n else 0.0
    p_lossy = n_pass_lossy / n if n else 0.0

    def k_to_pct(p: float, pct: float) -> float:
        # 1 - (1-p)^k >= pct  →  k >= log(1-pct) / log(1-p)
        if p <= 0:
            return float("inf")
        if p >= 1:
            return 1.0
        return math.log(1 - pct) / math.log(1 - p)

    return {
        "n_candidates": n,
        "n_pass_strict": n_pass,
        "n_pass_lossy": n_pass_lossy,
        "p_one_strict": p_strict,
        "p_one_lossy": p_lossy,
        "k_to_90_strict": k_to_pct(p_strict, 0.90),
        "k_to_99_strict": k_to_pct(p_strict, 0.99),
        "k_to_90_lossy": k_to_pct(p_lossy, 0.90),
        "asymptote_strict": 1.0 if p_strict > 0 else 0.0,
    }


def summarize_sage_dimension(workdir: Path) -> dict:
    """Walk workdir for sage-loop artifacts (out.txt, out_pass2.txt,
    out_pass2_runtime.txt) and record which pass produced the final diff
    that got promoted. Combined with the cycle result this tells us the
    marginal gain per added sage pass.
    """
    out = {}
    if (workdir / "out.txt").exists():
        out["pass1_size"] = (workdir / "out.txt").stat().st_size
    if (workdir / "out_pass2.txt").exists():
        out["pass2_size"] = (workdir / "out_pass2.txt").stat().st_size
        out["validator_pass2_fired"] = True
    if (workdir / "out_pass2_runtime.txt").exists():
        out["pass3_size"] = (workdir / "out_pass2_runtime.txt").stat().st_size
        out["runtime_pass3_fired"] = True
    return out


def analyze_cycle(cycle: str, out_dir: Path) -> int:
    results = load_cycle_results(cycle)
    if not results:
        print(f"no records in cycle {cycle}", file=sys.stderr)
        return 1

    out_dir.mkdir(parents=True, exist_ok=True)
    out_jsonl = out_dir / f"solution_curve-{cycle}.jsonl"
    out_md = out_dir / f"solution_curve-{cycle}.md"

    md = [f"# Solution-curve analysis — cycle {cycle}\n"]
    md.append(f"Instances: {len(results)}")
    md.append("")
    md.append("## Per-instance curve fit")
    md.append("")
    md.append("| Instance | beams | pass_strict | pass_lossy | p_one | k→90% | k→99% | sage_passes |")
    md.append("|---|---|---|---|---|---|---|---|")

    per_inst_recs = []
    for r in results:
        inst = r.get("instance_id") or "?"
        beam_recs = load_beam_matrix(inst, cycle)
        curve = fit_pass_at_k(beam_recs)
        # Sage dimension comes from the workdir.
        tag = cycle if cycle.startswith("smoke-") or cycle.startswith("cycle-") else f"cycle-{cycle}"
        wd_candidates = list(LATENT_OUT.glob(f"inst-{inst}-*{tag}"))
        sage = summarize_sage_dimension(wd_candidates[0]) if wd_candidates else {}
        rec = {
            "instance_id": inst,
            "final_apply": r.get("apply"),
            "final_f2p": r.get("f2p"),
            "final_p2p_fail": r.get("p2p_fail") or r.get("reg_fail"),
            "curve": curve,
            "sage": sage,
        }
        per_inst_recs.append(rec)

        def fk(x):
            return "∞" if x == float("inf") else f"{x:.1f}"
        sage_str = "/".join(
            ["1"] +
            (["2"] if sage.get("validator_pass2_fired") else []) +
            (["3"] if sage.get("runtime_pass3_fired") else [])
        ) or "1"
        md.append(
            f"| {inst} | {curve['n_candidates']} | "
            f"{curve['n_pass_strict']}/{curve['n_candidates']} | "
            f"{curve['n_pass_lossy']}/{curve['n_candidates']} | "
            f"{curve['p_one_strict']:.2f} | "
            f"{fk(curve['k_to_90_strict'])} | {fk(curve['k_to_99_strict'])} | "
            f"{sage_str} |"
        )

    # Aggregate.
    total_beams = sum(r["curve"]["n_candidates"] for r in per_inst_recs)
    total_pass = sum(r["curve"]["n_pass_strict"] for r in per_inst_recs)
    total_lossy = sum(r["curve"]["n_pass_lossy"] for r in per_inst_recs)
    md.append("")
    md.append("## Aggregate")
    md.append("")
    md.append(f"- total beam candidates tested: {total_beams}")
    md.append(f"- total strict PASS: {total_pass}")
    md.append(f"- total lossy PASS (F2P pass, P2P may regress): {total_lossy}")
    if total_beams > 0:
        md.append(f"- aggregate p_one_strict: {total_pass / total_beams:.3f}")
        md.append(f"- aggregate p_one_lossy: {total_lossy / total_beams:.3f}")
    md.append("")
    md.append("## Reading the curves")
    md.append("")
    md.append("- `p_one` near 0 → directive-limited (no sample passes). More beams won't help. Fix the directive content.")
    md.append("- `p_one` low but > 0, k→90% finite → capability-limited; more beams genuinely help; cost scales log.")
    md.append("- `p_one` ≥ 0.5 → solved-with-budget; small beam counts suffice; the lever exists.")
    md.append("- `sage_passes = 1/2/3` shows which sage layers fired; matrix matters when multiple fired.")

    # Persist.
    with open(out_jsonl, "w") as f:
        for r in per_inst_recs:
            f.write(json.dumps(r) + "\n")
    out_md.write_text("\n".join(md) + "\n")
    print(f"wrote: {out_jsonl}", file=sys.stderr)
    print(f"wrote: {out_md}", file=sys.stderr)
    return 0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cycle", required=True,
                    help="cycle tag (e.g. '1.1') or smoke tag (e.g. 'smoke-phase1' or 'phase1')")
    ap.add_argument("--out-dir", default=None)
    args = ap.parse_args()
    out_dir = Path(args.out_dir) if args.out_dir else (LENS_ROOT / "cycle")
    sys.exit(analyze_cycle(args.cycle, out_dir))


if __name__ == "__main__":
    main()
