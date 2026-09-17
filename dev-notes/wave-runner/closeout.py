#!/usr/bin/env python3
"""End-of-wave close-out, as one command. Commits only if every gate is clean.

    python3 closeout.py <wave> <workflow-run-id>      e.g.  closeout.py 8 wf_63d9b695-7a3

Steps (README order): commits + per-packet status from the workflow journal,
workspace tests, byte_identical, wheel rebuild + staleness check, run.py,
matrix --emit/--check, python suites, fold the wave's followups into every
remaining packet's correction, update baseline.json (MEASURED) and LANDED,
schedule --verify, render the next wave, commit.

Prints a DELTA against the previous baseline so an unexpected blind spot,
partial, forbid violation or missing cell is visible instead of buried.
"""
import glob, json, os, re, subprocess, sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SG = ROOT / "bench" / "substrate-gap"
JOURNALS = Path.home() / ".claude/projects/-home-ivy-Code-glia"
WHEEL = ROOT / "target/wheels/repo_graph_py-0.4.18-cp311-abi3-manylinux_2_34_x86_64.whl"
CRATES = "engine py graph parsers code-domain store stamp doc-sources".split()


def sh(cmd, cwd=ROOT, timeout=1800):
    r = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True, timeout=timeout)
    return r.stdout + r.stderr, r.returncode


def journal_results(run):
    path = next(iter(glob.glob(str(JOURNALS / f"*/subagents/workflows/{run}/journal.jsonl"))), None)
    if not path:
        sys.exit(f"no journal for {run}")
    out = []
    for line in open(path):
        d = json.loads(line)
        if d.get("type") != "result":
            continue
        v = d.get("result")
        if isinstance(v, str):
            try:
                v = json.loads(v)
            except ValueError:
                continue
        if isinstance(v, dict) and v.get("packet"):
            out.append(v)
    return out


def section(text, head):
    m = re.search(rf"^{re.escape(head)}[^\n]*?(\d+)\)?:?\s*(\d+)?\s*$", text, re.M)
    if not m:
        return 0, []
    n = int(m.group(2) or m.group(1))
    items, rest = [], text[m.end():].split("\n")
    for l in rest[1:]:
        if l.startswith("  - "):
            items.append(l[4:].strip())
        elif l.strip():
            break
    return n, items


def main():
    wave, run = int(sys.argv[1]), sys.argv[2]
    sys.path.insert(0, str(HERE))
    import schedule
    base = json.loads((HERE / "baseline.json").read_text())
    gates, lines = [], []
    say = lambda s: (print(s), lines.append(s))

    res = journal_results(run)
    waves, _ = schedule.schedule(schedule.load())
    ids = waves[wave - 1]
    got = {r["packet"].split("—")[0].strip(): r for r in res}
    say(f"== wave {wave}: {len(res)}/{len(ids)} returned")
    for i in ids:
        r = got.get(i)
        st = r["status"] if r else "MISSING"
        say(f"   {i:<8} {st:<11} {(r or {}).get('commit','')[:8]:<9} {((r or {}).get('breaking') or '')[:56]}")
        if st not in ("green", "not-needed", "partial"):
            gates.append(f"{i} status {st}")

    dirty, _ = sh("git status --short")
    dirty = [l for l in dirty.splitlines() if l.strip() and "packet-corrections.json" not in l]
    if dirty:
        gates.append(f"uncommitted paths: {dirty[:6]}")

    out, _ = sh("cargo test --workspace --quiet 2>&1", timeout=2400)
    passed = sum(int(m) for m in re.findall(r"test result: ok\. (\d+) passed", out))
    failed = sum(int(m) for m in re.findall(r"(\d+) failed", out))
    comp = len(re.findall(r"^error(\[E\d+\])?:", out, re.M))
    say(f"== cargo test --workspace: {passed} passed, {failed} failed, {comp} compile errors")
    if failed or comp:
        gates.append("workspace tests")
    out, _ = sh("cargo test -p repo-graph-engine --test byte_identical 2>&1")
    bi = "2 passed; 0 failed" in out
    say(f"== byte_identical: {'green' if bi else 'RED'}")
    if not bi:
        gates.append("byte_identical")

    sh("cargo clean -p repo-graph-engine -p repo-graph-py")
    out, rc = sh("maturin build -m py/Cargo.toml --release 2>&1", timeout=2400)
    sh(f"pip install --force-reinstall --no-deps -q {WHEEL}")
    so, _ = sh("python3 -c \"import repo_graph_py,glob,os;print(glob.glob(os.path.join(os.path.dirname(repo_graph_py.__file__),'*.so'))[0])\"")
    stale, _ = sh(f"find {' '.join(CRATES)} -name '*.rs' -newer {so.strip()}")
    stamp, _ = sh("python3 -c 'import repo_graph_py as r; print(r.build_stamp())'")
    say(f"== wheel: build rc={rc}, stale={len(stale.split())}, stamp {stamp.strip()}")
    if rc or stale.strip():
        gates.append("wheel rebuild/stale")

    rp, _ = sh("python3 run.py --no-log 2>/dev/null", cwd=SG)
    fixtures = len([p for p in (SG / "fixtures").iterdir() if (p / "key.json").exists()])
    bs, bsl = section(rp, "BLIND SPOTS (recall 0.00)")
    mn, mnl = section(rp, "MISSING NODES")
    pa, pal = section(rp, "PARTIAL (0 < recall < 1)")
    fv, fvl = section(rp, "FORBID VIOLATIONS")
    mc, mcl = section(rp, "MISSING CELLS")
    ge, gel = section(rp, "GRADER ERRORS (fixture NOT in the matrix)")
    say(f"== run.py: {fixtures} fixtures | blind {bs} | missing nodes {mn} | partial {pa} | forbid {fv} | missing cells {mc} | grader errors {ge}")
    old = base["run_py"]
    for label, now, prev in (("blind", bsl, old.get("blind_list", [])), ("partial", pal, old.get("partial_list", [])),
                             ("forbid", fvl, old.get("forbid_list", [])), ("missing cells", mcl, old.get("missing_cells_list", [])),
                             ("missing nodes", mnl, old.get("missing_nodes_list", []))):
        new, gone = sorted(set(now) - set(prev)), sorted(set(prev) - set(now))
        if new:
            say(f"   NEW {label}: {new}")
        if gone:
            say(f"   cleared {label}: {gone}")
    if ge:
        gates.append(f"grader errors: {gel}")

    mo, _ = sh("python3 matrix.py --emit 2>&1", cwd=SG)
    m = re.search(r"(\d+) full, (\d+) partial, (\d+) none, (\d+) unknown", mo)
    _, chk = sh("python3 matrix.py --check >/dev/null 2>&1", cwd=SG)
    tm, _ = sh("python3 test_matrix.py 2>&1 | tail -1", cwd=SG)
    tg, _ = sh("python3 test_grade.py 2>&1 | tail -1", cwd=SG)
    full, part, none_, unk = map(int, m.groups()) if m else (0, 0, 0, 0)
    covered = 480 - unk
    say(f"== matrix: {full} full, {part} partial, {none_} none, {unk} unknown = {covered}/480 | check={chk} | test_matrix: {tm.strip()} | test_grade: {tg.strip()}")
    if chk:
        gates.append("matrix --check")
    if " 0 failed" not in tm:
        gates.append("test_matrix")
    if "all passed" not in tg:
        gates.append("test_grade")

    # fold followups into remaining packets
    rem = {p for w in waves[wave:] for p in w}
    pat = re.compile(r"\b(A\d+\.\d+[a-c]?)\b")
    cp = ROOT / "dev-notes/packet-corrections.json"
    cf = json.loads(cp.read_text())
    folded = {}
    for r in res:
        src = r["packet"].split("—")[0].strip()
        for t in r.get("followups") or []:
            for tgt in set(pat.findall(t)) & rem:
                note = f"HANDOFF FROM {src} (w{wave}): {t.strip()}"
                if note not in cf["corrections"].get(tgt, ""):
                    cf["corrections"][tgt] = (cf["corrections"].get(tgt, "") + "\n\n" + note).strip()
                    folded[tgt] = folded.get(tgt, 0) + 1
    cp.write_text(json.dumps(cf, indent=1))
    say(f"== handoffs folded: {folded or 'none'}")

    head, _ = sh("git rev-parse --short HEAD")
    base.update({"measured_at_head": head.strip(), "after_wave": wave})
    base["cargo_test_workspace"] = {"passed": passed, "failed": failed}
    base["history_passed"].append(passed)
    base["run_py"].update({"fixtures": fixtures, "blind_spots": bs, "partial": pa, "forbid_violations": fv,
                           "missing_cells": mc, "grader_errors": ge, "blind_list": bsl, "partial_list": pal,
                           "forbid_list": fvl, "missing_cells_list": mcl, "missing_nodes_list": mnl,
                           "blind_spot_detail": f"blind: {bsl}; missing nodes: {mnl}; missing cells: {mcl}. "
                                                "Those present at wave start are deliberate baselines for later packets unless your packet owns them."})
    base["matrix_py"] = {"full": full, "partial": part, "none": none_, "unknown": unk, "covered": covered,
                         "grid": 480, "invalid_cell_declarations": 0, "check_exit": chk}
    base["test_matrix_py"] = tm.strip()
    base["test_grade_py"] = tg.strip()
    (HERE / "baseline.json").write_text(json.dumps(base, indent=2))

    sp = HERE / "schedule.py"
    s = sp.read_text()
    if f"    {wave}: " not in s:
        s = s.replace("\n}\n\n\ndef _graph_module", f'\n    {wave}: "{" ".join(ids)}",\n}}\n\n\ndef _graph_module', 1)
        sp.write_text(s)
    ver, vrc = sh("python3 dev-notes/wave-runner/schedule.py --verify | tail -1")
    say(f"== schedule --verify: {ver.strip()}")
    if vrc:
        gates.append("schedule --verify")

    if wave < len(waves):
        out, _ = sh(f"python3 dev-notes/wave-runner/gen_wave.py {wave+1} /tmp/claude-1000/-home-ivy-Code-glia/wf/wave{wave+1}.js")
        say("== " + out.strip().replace("\n", " | "))

    if gates:
        say(f"!! NOT COMMITTING — gates failed: {gates}")
        sys.exit(1)
    msg = (f"bench+plan: wave {wave} landed — {passed} tests, {fixtures} fixtures, matrix {covered}/480\n\n"
           + "\n".join(lines) + "\n\nCo-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>\n")
    Path("/tmp/claude-1000/-home-ivy-Code-glia/closeout.msg").write_text(msg)
    out, rc = sh("git add bench/substrate-gap/results-latest.json bench/substrate-gap/COVERAGE.md "
                 "dev-notes/packet-corrections.json dev-notes/wave-runner && "
                 "git -c user.name='james chahwan' commit -q -F /tmp/claude-1000/-home-ivy-Code-glia/closeout.msg && git log --oneline -1")
    say(f"== committed: {out.strip()}" if rc == 0 else f"!! commit failed: {out}")


if __name__ == "__main__":
    main()
