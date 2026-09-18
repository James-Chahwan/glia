#!/usr/bin/env python3
"""Render a wave's Workflow script from the committed programme files.

Inputs (all committed): wave-packets.json, packet-corrections.json,
wave-runner/{shared_brief.md,baseline.json}, and schedule.py's waves.

    python3 gen_wave.py 6 /path/to/wave6.js
    python3 gen_wave.py --leap 0 /path/to/leap-w0.js     # the 0.5.0 leap

--leap reads leap-packets.json through leap_schedule.py and briefs agents with
shared_brief_leap.md. A Batch C packet's CORRECTION is its standing
packet-corrections.json entry, then its leap re-verification correction, then
its leap-corrections.json entry. Wave 0 (the serial splits) and any wave the
scheduler gives a single packet render as a sequential script.

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

FIELDS = ["id", "title", "batch", "area", "loc", "rationale", "entry_points", "files_touched",
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


def leap_inputs(wave):
    import leap_schedule
    P, claims, D, X, serial = leap_schedule.load()
    waves = leap_schedule.schedule(P, claims, D, X, serial)
    ids = waves[wave]
    notes = ROOT / "dev-notes"
    standing = json.loads((notes / "packet-corrections.json").read_text())["corrections"]
    leap_corr = json.loads((notes / "leap-corrections.json").read_text())["corrections"]
    rows = {r["packet"]: r for r in json.loads((notes / "leap-packets.json").read_text())["batch_c"]}
    corr = {}
    for i in ids:
        parts = [standing.get(i)] if i in rows else []
        if i in rows:
            parts.append("LEAP RE-VERIFICATION (" + rows[i]["verdict"] + "): " + rows[i]["correction"]
                         + "\nFILES YOU MAY TOUCH (re-verified, overrides files_touched): " + ", ".join(rows[i]["files_touched_now"]))
        parts.append(leap_corr.get(i))
        corr[i] = "\n\n".join(p for p in parts if p) or None
    brief = ((HERE / "shared_brief_leap.md").read_text()
             .replace("{{WAVE}}", str(wave))
             .replace("{{LAST}}", str(len(waves) - 1)))
    serial_run = wave == 0 or len(ids) == 1
    return P, claims, waves, ids, corr, brief, serial_run


def main():
    args = sys.argv[1:]
    leap = "--leap" in args
    if leap:
        args.remove("--leap")
    wave, out = int(args[0]), Path(args[1])
    base = json.loads((HERE / "baseline.json").read_text())
    if leap:
        P, claims, waves, ids, corr, brief, serial_run = leap_inputs(wave)
        brief = brief.replace("{{BASELINE}}", render_baseline(base))
        name, total = f"glia-leap-w{wave}", f"W0..W{len(waves) - 1}"
    else:
        P = schedule.load()
        waves, claims = schedule.schedule(P)
        ids = waves[wave - 1]
        corr = json.loads((ROOT / "dev-notes" / "packet-corrections.json").read_text())["corrections"]
        brief = ((HERE / "shared_brief.md").read_text()
                 .replace("{{WAVE}}", str(wave))
                 .replace("{{WAVES}}", str(len(waves)))
                 .replace("{{BASELINE}}", render_baseline(base)))
        name, total, serial_run = f"glia-wave-{wave}", str(len(waves)), False

    if not serial_run:
        hits = Counter(f for i in ids for f in claims[i])
        clash = {f: n for f, n in hits.items() if n > 1}
        if clash:
            sys.exit(f"wave {wave} is not file-disjoint: {clash}")

    packets = [{**{k: P[i][k] for k in FIELDS if P[i].get(k) is not None},
                "CORRECTION": corr.get(i)} for i in ids]
    loc = sum(p.get("loc", 0) for p in packets)
    shape = "one at a time, in order" if serial_run else "file-disjoint, concurrent"
    js = f"""export const meta = {{
  name: '{name}',
  description: 'Wave {wave} of {total}: {len(ids)} packets, {shape} ({", ".join(ids)})',
  phases: [{{ title: 'Wave {wave}', detail: '{len(ids)} implementing agents ({shape}), each committing its own packet on main' }}],
}}

const SHARED = {json.dumps(brief)};
const SCHEMA = {json.dumps(RESULT_SCHEMA)};
const PACKETS = {json.dumps(packets, indent=1)};

phase('Wave {wave}')
log('{len(ids)} packets, ~{loc} LOC, disjoint file sets')

const run = p => agent(
  SHARED + '\\n\\n=== YOUR PACKET ===\\n' + JSON.stringify(p, null, 1) +
  (p.CORRECTION ? '\\n\\n=== CORRECTION — THIS OVERRIDES THE SPEC ABOVE WHEREVER THEY DISAGREE ===\\n' + p.CORRECTION : ''),
  {{ label: 'w{wave}:' + p.id, phase: 'Wave {wave}', schema: SCHEMA }}
)
const SERIAL = {'true' if serial_run else 'false'}
const results = []
if (SERIAL) {{
  for (const p of PACKETS) {{
    const r = await run(p)
    results.push(r)
    if (!r || r.status !== 'green') {{ log(p.id + ' not green (' + (r ? r.status : 'no result') + ') — stopping the serial wave here'); break }}
  }}
}} else {{
  results.push(...await parallel(PACKETS.map(p => () => run(p))))
}}

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
