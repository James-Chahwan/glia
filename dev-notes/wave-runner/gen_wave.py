#!/usr/bin/env python3
"""Render a wave's Workflow script from the committed programme files.

Inputs (all committed): wave-packets.json, packet-corrections.json,
wave-runner/{shared_brief.md,baseline.json}, and schedule.py's waves.

    python3 gen_wave.py 6 /path/to/wave6.js

Refuses to render a wave whose packets are not all file-disjoint, and prints
which packets carry no correction so a missing one is a decision, not an
accident.
"""
import json
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
import schedule  # noqa: E402

FIELDS = ["id", "title", "batch", "loc", "rationale", "entry_points", "files_touched",
          "implementation", "fixtures", "acceptance", "fired_on_marker", "depends_on",
          "breaking", "risks"]

RESULT_SCHEMA = {
    "type": "object",
    "properties": {
        "packet": {"type": "string"},
        "status": {"type": "string", "description": "green | partial | blocked | not-needed"},
        "commit": {"type": "string", "description": "sha, or \"none\""},
        "files_changed": {"type": "array", "items": {"type": "string"}},
        "loc_actual": {"type": "number"},
        "graph_queries": {"type": "string", "description": "which repo-graph calls you ran and what they told you"},
        "baseline_recorded": {"type": "string", "description": "verbatim grade.py output for each new fixture BEFORE the fix, or why there is none"},
        "gate_output": {"type": "string", "description": "verbatim output of every gate command, each with its command line above it"},
        "marker": {"type": "string", "description": "the literal fired_on stderr line, and the command that greps it"},
        "breaking": {"type": "string", "description": "none | qname_shape | edge_removal | cell_value | api_signature — and what moved with it"},
        "surprises": {"type": "array", "items": {"type": "string"}},
        "followups": {"type": "array", "items": {"type": "string"}},
    },
    "required": ["packet", "status", "commit", "files_changed", "gate_output", "marker"],
}


def render_baseline(b):
    r, m = b["run_py"], b["matrix_py"]
    hist = " -> ".join(map(str, b["history_passed"]))
    return f"""=== BASELINE, MEASURED AT HEAD {b['measured_at_head']} AFTER WAVE {b['after_wave']} (take as given) ===
- `cargo test --workspace` = {b['cargo_test_workspace']['passed']} passing, {b['cargo_test_workspace']['failed']} failing. It has moved every wave ({hist}). NEVER gate on a literal count.
- `python3 bench/substrate-gap/run.py --no-log` = {r['fixtures']} fixtures; PARTIAL {r['partial']}, FORBID VIOLATIONS {r['forbid_violations']}, MISSING CELLS {r['missing_cells']}, GRADER ERRORS {r['grader_errors']}; BLIND SPOTS {r['blind_spots']} — {r['blind_spot_detail']}. Not regressions, not yours unless your packet is one of those.
- `python3 bench/substrate-gap/matrix.py` = {m['full']} full, {m['partial']} partial, {m['none']} none, {m['unknown']} unknown; {m['covered']}/{m['grid']} cells; INVALID CELL DECLARATIONS {m['invalid_cell_declarations']}; `--check` exits {m['check_exit']}.
- `test_matrix.py` {b['test_matrix_py']}. `test_grade.py` {b['test_grade_py']}."""


def main():
    wave, out = int(sys.argv[1]), Path(sys.argv[2])
    P = schedule.load()
    waves, claims = schedule.schedule(P)
    ids = waves[wave - 1]

    hits = Counter(f for i in ids for f in claims[i])
    clash = {f: n for f, n in hits.items() if n > 1}
    if clash:
        sys.exit(f"wave {wave} is not file-disjoint: {clash}")

    corr = json.loads((ROOT / "dev-notes" / "packet-corrections.json").read_text())["corrections"]
    base = json.loads((HERE / "baseline.json").read_text())
    brief = ((HERE / "shared_brief.md").read_text()
             .replace("{{WAVE}}", str(wave))
             .replace("{{WAVES}}", str(len(waves)))
             .replace("{{BASELINE}}", render_baseline(base)))

    packets = [{**{k: P[i][k] for k in FIELDS if P[i].get(k) is not None},
                "CORRECTION": corr.get(i)} for i in ids]
    loc = sum(p.get("loc", 0) for p in packets)
    js = f"""export const meta = {{
  name: 'glia-wave-{wave}',
  description: 'Wave {wave} of {len(waves)}: {len(ids)} file-disjoint packets ({", ".join(ids)})',
  phases: [{{ title: 'Wave {wave}', detail: '{len(ids)} implementing agents on disjoint file sets, each committing its own packet on main' }}],
}}

const SHARED = {json.dumps(brief)};
const SCHEMA = {json.dumps(RESULT_SCHEMA)};
const PACKETS = {json.dumps(packets, indent=1)};

phase('Wave {wave}')
log('{len(ids)} packets, ~{loc} LOC, disjoint file sets')

const results = await parallel(PACKETS.map(p => () => agent(
  SHARED + '\\n\\n=== YOUR PACKET ===\\n' + JSON.stringify(p, null, 1) +
  (p.CORRECTION ? '\\n\\n=== CORRECTION — THIS OVERRIDES THE SPEC ABOVE WHEREVER THEY DISAGREE ===\\n' + p.CORRECTION : ''),
  {{ label: 'w{wave}:' + p.id, phase: 'Wave {wave}', schema: SCHEMA }}
)));

const ok = results.filter(Boolean);
log(ok.length + '/' + PACKETS.length + ' returned — ' + ok.filter(r => r.status === 'green').length + ' green');
return {{ wave: {wave}, returned: ok.length, expected: PACKETS.length, results: ok }};
"""
    out.write_text(js)
    missing = [i for i in ids if not corr.get(i)]
    print(f"wave {wave}: {len(ids)} packets, ~{loc} LOC -> {out}")
    print(f"  no correction: {missing or 'none'}")


if __name__ == "__main__":
    main()
