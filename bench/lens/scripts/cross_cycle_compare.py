#!/usr/bin/env python3
"""
cross_cycle_compare.py — side-by-side cycle comparison + FIM-specific buckets.

For Phase D #153. Reads cycle-N.M-{tag}-results.jsonl files and produces
a markdown table showing failure-mode shifts per instance across cycles.

Failure buckets:
  PASS         — strict f2p_pass == total, reg_fail == 0
  PARTIAL      — applied + some f2p_pass > 0 (and not strict PASS)
  FAIL_LOGIC   — applied + 0 f2p_pass, reg_fail == 0 (model semantics wrong)
  FAIL_REGR    — applied + reg_fail > 0 (broke other tests)
  NO_RUN       — applied but tests couldn't run (ImportError etc)
  APPLY_FAIL   — diff didn't apply (geometry)
  TESTPATCH_FAIL — SWE-bench test_patch couldn't apply (data issue)
  NO_DIFF      — model produced no diff

FIM-specific (from fim_result.json in workdir):
  FIM_USED     — FIM produced a body that made it through ast.parse
  FIM_NOLOCATE — extractor returned qname, locate failed
  FIM_INFER    — FIM inference failed (timeout, model error)
  FIM_NOCHANGE — model echoed original body
  FIM_NOEXTRACT — extractor returned no target

Usage:
  python cross_cycle_compare.py --cycles 3.0-holdout 3.1-trim 3.2-fim 3.3-iter
  python cross_cycle_compare.py --cycles 3.2-fim 3.3-iter --out /tmp/compare.md
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Optional

CYCLE_DIR = Path("/home/ivy/Code/glia/bench/lens/cycle")
WORKDIR_TPL = Path("/home/ivy/Code/glia/bench/latent/out/inst-{iid}-14b-q4-{tag}")


def classify(row: dict) -> str:
    """Map row → failure bucket."""
    apply = (row.get("apply") or "").lower()
    f2p = row.get("f2p") or ""
    f2p_pass = int(row.get("f2p_pass") or 0)
    f2p_fail = int(row.get("f2p_fail") or 0)
    reg_fail = int(row.get("reg_fail") or 0)

    if apply == "no-diff":
        return "NO_DIFF"
    if apply == "apply-fail":
        return "APPLY_FAIL"
    if apply == "testpatch-fail":
        return "TESTPATCH_FAIL"
    if "NO-RUN" in f2p:
        return "NO_RUN"
    if f2p == "PASS":
        return "PASS" if reg_fail == 0 else "PASS_REGR"
    if "regressions" in f2p:
        return "PASS_REGR"
    # Anything else with FAIL or applied
    if apply.startswith("applied"):
        if f2p_pass > 0:
            return "PARTIAL" if f2p_pass < (f2p_pass + f2p_fail) else "PASS"
        if reg_fail > 0:
            return "FAIL_REGR"
        return "FAIL_LOGIC"
    return "UNKNOWN"


def fim_status(iid: str, tag: str) -> dict:
    """Read fim_result.json + fim_iterations.json from workdir if present."""
    wd = Path(str(WORKDIR_TPL).replace("{iid}", iid).replace("{tag}", tag))
    fimr = wd / "fim_result.json"
    iters = wd / "fim_iterations.json"
    extract_debug = wd / "fim_extract_debug.json"

    out = {"fim_status": None, "fim_body_chars": 0, "fim_error": None,
           "iters": None, "iters_passed_on": None,
           "extracted_qname": None}

    if not fimr.exists():
        # No FIM attempted (path disabled OR pre-fix instance)
        return out

    try:
        d = json.loads(fimr.read_text())
        if d.get("success"):
            out["fim_status"] = "USED"
            out["fim_body_chars"] = d.get("body_chars", 0)
        else:
            err = d.get("error") or ""
            out["fim_error"] = err
            if "locate" in err.lower():
                out["fim_status"] = "NOLOCATE"
            elif "extract" in err.lower() or "qname not found" in err.lower():
                out["fim_status"] = "NOLOCATE"
            elif "inference" in err.lower() or "timeout" in err.lower():
                out["fim_status"] = "INFER"
            elif "no-change" in err.lower() or "echoed" in err.lower():
                out["fim_status"] = "NOCHANGE"
            elif "ast" in err.lower():
                out["fim_status"] = "AST"
            else:
                out["fim_status"] = "FAIL"
    except Exception:
        pass

    if iters.exists():
        try:
            d = json.loads(iters.read_text())
            out["iters"] = len(d.get("iterations", []))
            out["iters_passed_on"] = d.get("succeeded_on_iter")
        except Exception:
            pass

    if extract_debug.exists():
        try:
            d = json.loads(extract_debug.read_text())
            out["extracted_qname"] = d.get("target_qname")
        except Exception:
            pass

    return out


def load_cycle(tag: str) -> dict:
    """Load cycle results JSONL into {instance_id: enriched_row}."""
    path = CYCLE_DIR / f"cycle-{tag}-results.jsonl"
    rows = {}
    if not path.exists():
        return rows
    for line in path.read_text().splitlines():
        if not line.strip():
            continue
        try:
            o = json.loads(line)
        except Exception:
            continue
        iid = o.get("instance_id")
        if not iid:
            continue
        o["_bucket"] = classify(o)
        o["_fim"] = fim_status(iid, tag)
        rows[iid] = o
    return rows


def render_md(cycles: list[tuple[str, dict]]) -> str:
    """Render a markdown report comparing all loaded cycles."""
    all_instances = set()
    for _tag, rows in cycles:
        all_instances.update(rows.keys())
    instances = sorted(all_instances)

    lines = []
    lines.append("# Cross-cycle comparison\n")
    lines.append(f"Cycles: {', '.join(tag for tag, _ in cycles)}\n")
    lines.append("")

    # Per-cycle aggregate
    lines.append("## Aggregate per cycle\n")
    lines.append("| Cycle | PASS | PARTIAL | FAIL_LOGIC | FAIL_REGR | NO_RUN | APPLY_FAIL | TESTPATCH_FAIL | NO_DIFF | FIM_USED | FIM_NOLOC |")
    lines.append("|---|---|---|---|---|---|---|---|---|---|---|")
    for tag, rows in cycles:
        buckets = {b: 0 for b in ["PASS","PASS_REGR","PARTIAL","FAIL_LOGIC","FAIL_REGR","NO_RUN","APPLY_FAIL","TESTPATCH_FAIL","NO_DIFF","UNKNOWN"]}
        fim_used = fim_noloc = 0
        for r in rows.values():
            buckets[r["_bucket"]] = buckets.get(r["_bucket"], 0) + 1
            fs = r["_fim"]["fim_status"]
            if fs == "USED": fim_used += 1
            elif fs in ("NOLOCATE", "FAIL"): fim_noloc += 1
        pass_cell = str(buckets["PASS"])
        if buckets["PASS_REGR"]: pass_cell += f"(+{buckets['PASS_REGR']}regr)"
        lines.append(f"| {tag} | {pass_cell} | {buckets['PARTIAL']} | {buckets['FAIL_LOGIC']} | {buckets['FAIL_REGR']} | {buckets['NO_RUN']} | {buckets['APPLY_FAIL']} | {buckets['TESTPATCH_FAIL']} | {buckets['NO_DIFF']} | {fim_used} | {fim_noloc} |")

    # Per-instance table — show outcomes across cycles
    lines.append("\n## Per-instance shift\n")
    header = "| Instance | " + " | ".join(tag for tag, _ in cycles) + " |"
    lines.append(header)
    lines.append("|" + "---|" * (len(cycles) + 1))
    for iid in instances:
        cells = [iid]
        for tag, rows in cycles:
            r = rows.get(iid)
            if not r:
                cells.append("—")
                continue
            bucket = r["_bucket"]
            fim = r["_fim"]
            wall = int(r.get("wall_s_outer") or r.get("wall_s", 0))
            cell = f"{bucket} ({wall}s)"
            if fim["fim_status"] == "USED":
                cell += f" FIM✓"
                if fim["iters"]:
                    cell += f" ×{fim['iters']}"
                    if fim["iters_passed_on"]:
                        cell += f" PASS@{fim['iters_passed_on']}"
            elif fim["fim_status"]:
                cell += f" FIM✗ ({fim['fim_status']})"
            cells.append(cell)
        lines.append("| " + " | ".join(cells) + " |")

    # Headline: PASS counts + FIM-USED counts
    lines.append("\n## Headline\n")
    for tag, rows in cycles:
        n_pass = sum(1 for r in rows.values() if r["_bucket"] == "PASS")
        n_fim = sum(1 for r in rows.values() if r["_fim"]["fim_status"] == "USED")
        n_total = len(rows)
        lines.append(f"- **{tag}**: {n_pass}/{n_total} PASS, {n_fim}/{n_total} FIM-USED")

    return "\n".join(lines)


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--cycles", nargs="+", required=True,
                   help="cycle tags (e.g. 3.0-holdout 3.1-trim 3.2-fim 3.3-iter)")
    p.add_argument("--out", default=None, help="output md path; default: stdout")
    args = p.parse_args()

    cycles = [(tag, load_cycle(tag)) for tag in args.cycles]
    md = render_md(cycles)

    if args.out:
        Path(args.out).write_text(md)
        print(f"wrote {args.out}")
    else:
        print(md)


if __name__ == "__main__":
    main()
