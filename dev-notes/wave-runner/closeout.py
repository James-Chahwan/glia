#!/usr/bin/env python3
"""End-of-wave close-out, as one command. Commits only if every gate is clean.

    python3 closeout.py <wave> <workflow-run-id>      e.g.  closeout.py 8 wf_63d9b695-7a3
    python3 closeout.py --leap <wave> <run-id>        the 0.5.0 leap (waves W0..)
    python3 closeout.py --leap <wave> <run1>,<run2>   a packet re-run merged over its wave (later wins)
    python3 closeout.py --leap <wave> --plan-only     print the wave's packets and exit
    python3 closeout.py --release 051 <wave> <run-id> the 0.5.1 catch-up leap: schedule_051.py,
                                                      leap-051-corrections.json (ids C*.*), the leap venv

--leap schedules through leap_schedule.py, records the wave's 5-hour usage
cost (usage_gate.py --end) and prints whether the next wave can start, folds followups into
leap-corrections.json (ids A*.* and L*.*), and records LANDED in leap_schedule.py.

Steps (README order): commits + per-packet status from the workflow journal,
workspace tests, byte_identical, the out-of-workspace engram-export check
(scripts/check-engram-export.sh), wheel rebuild + staleness check, run.py,
matrix --emit/--check, python suites, fold the wave's followups into every
remaining packet's correction, update baseline.json (MEASURED) and LANDED,
schedule --verify, render the next wave, commit.

Prints a DELTA against the previous baseline so an unexpected blind spot,
partial, forbid violation or missing cell is visible instead of buried.
"""
import glob, json, os, re, shutil, subprocess, sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SG = ROOT / "bench" / "substrate-gap"
JOURNALS = Path.home() / ".claude/projects/-home-ivy-Code-glia"
# Since LD.11b the wheel is glia_py-<ver>-cp311-abi3-*.whl (dist glia-py, import glia_py);
# the version changes with the release, so take the newest by mtime. The 0.4.x wheels
# in the same dir carry the old dist name and never match.
WHEELS = ROOT / "target/wheels"
WHEEL_GLOB = "glia_py-*.whl"
# --leap installs and grades the leap wheel in its own venv. The user-site wheel
# (/usr/bin/python3) stays the pre-leap build: James's repo-graph MCP server imports
# it, and the leap's pyo3 breaks would take that server down in every repo. After the
# Python rename (LD.11b) the venv holds BOTH wheels: the old module from W0..W32 and
# glia_py. Everything below imports glia_py; importing the old name would grade the
# frozen W32 build.
LEAP_PY = os.path.expanduser(os.environ.get("GLIA_LEAP_PY", "~/.venvs/glia-leap/bin/python"))
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


def packet_id(field):
    """The packet id an agent reports. Agents sometimes append the title after an em dash, an
    en dash or a plain ` - ` (0.5.1 W7's CB.15 did), so take the leading id token, not a split."""
    m = re.match(r"\s*([A-Z][A-Z0-9]?\d*\.\d+[a-z]?)", field)
    return m.group(1) if m else field.split("—")[0].strip()


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


def package_name(crate_dir):
    """cargo package name of a workspace crate — repo-graph-* before LD.11a, glia-* after."""
    m = re.search(r'^name\s*=\s*"([^"]+)"', (ROOT / crate_dir / "Cargo.toml").read_text(), re.M)
    return m.group(1)


def main():
    args = sys.argv[1:]
    leap = "--leap" in args
    if leap:
        args.remove("--leap")
    release = None
    if "--release" in args:
        k = args.index("--release")
        release = args[k + 1]
        del args[k:k + 2]
        if release != "051":
            sys.exit(f"unknown release {release!r} (known: 051)")
    sys.path.insert(0, str(HERE))
    wave = int(args[0])
    if release:
        import schedule_051
        _, _, waves = schedule_051.waves()
        ids, remaining = waves[wave], waves[wave + 1:]
    elif leap:
        import leap_schedule
        waves = leap_schedule.schedule(*leap_schedule.load())
        ids, remaining = waves[wave], waves[wave + 1:]
    else:
        import schedule
        waves, _ = schedule.schedule(schedule.load())
        ids, remaining = waves[wave - 1], waves[wave:]
    if "--plan-only" in args:
        print(f"wave {wave}: {' '.join(ids)}")
        print(f"remaining waves: {len(remaining)}")
        return
    # One run id, or several comma-separated: a packet re-run on its own (e.g. after a
    # permission fix) is merged over the wave's run, the later run winning per packet.
    runs = args[1].split(",")
    base = json.loads((HERE / "baseline.json").read_text())
    gates, lines = [], []
    say = lambda s: (print(s), lines.append(s))
    engine_pkg, py_pkg = package_name("engine"), package_name("py")
    py = LEAP_PY if (leap or release) else "python3"   # every command that imports the wheel (glia_py)

    got = {}
    for run in runs:
        for r in journal_results(run):
            got[packet_id(r["packet"])] = r
    res = list(got.values())
    say(f"== wave {wave}: {len(res)}/{len(ids)} returned")
    for i in ids:
        r = got.get(i)
        st = r["status"] if r else "MISSING"
        say(f"   {i:<8} {st:<11} {(r or {}).get('commit','')[:8]:<9} {((r or {}).get('breaking') or '')[:56]}")
        if st not in ("green", "not-needed", "partial"):
            gates.append(f"{i} status {st}")

    dirty, _ = sh("git status --short")
    dirty = [l for l in dirty.splitlines() if l.strip() and "packet-corrections.json" not in l
             and "leap-051-corrections.json" not in l
             and "usage-log.jsonl" not in l]
    if dirty:
        gates.append(f"uncommitted paths: {dirty[:6]}")

    # /tmp is a RAM disk: agents' leftover isolated builds filled it once and the workspace
    # tests failed to link (W18). Say so up front instead of reporting a phantom compile error.
    free_gb = shutil.disk_usage("/tmp").free / 2**30
    if free_gb < 8:
        big, _ = sh("du -xsh /tmp/claude-1000/*/*/scratchpad/* 2>/dev/null | sort -h | tail -5")
        say(f"!! /tmp has only {free_gb:.1f}G free — largest scratch dirs:\n{big.rstrip()}")
        gates.append(f"/tmp free {free_gb:.1f}G < 8G")
    # The target dirs live on the repo's own disk: 0.5.1's W1 close-out hit "No space left on device"
    # there (target/ had grown to 219G, engram-export/target to 59G) and reported 26 phantom compile
    # errors. Fail up front instead, naming the largest build dirs.
    free_repo = shutil.disk_usage(str(ROOT)).free / 2**30
    if free_repo < 40:
        big, _ = sh("du -xsh target/* target/debug/* engram-export/target/debug/* ~/.cache/glia-* 2>/dev/null | sort -h | tail -6")
        say(f"!! the repo disk has only {free_repo:.1f}G free — largest build dirs:\n{big.rstrip()}\n"
            "   (target/debug/incremental and old isolated target dirs are safe to delete)")
        gates.append(f"repo disk free {free_repo:.1f}G < 40G")
    # --no-fail-fast: without it cargo stops at the first failing test binary, so one red binary hides
    # every other and the passed count is a fraction of the suite (0.5.1 W2: 262 of ~3,400 ran).
    out, _ = sh("cargo test --workspace --no-fail-fast --quiet 2>&1", timeout=2400)
    # Keep the full output: a one-line count cannot say which test or crate failed.
    (Path("/tmp/claude-1000/-home-ivy-Code-glia/wf") / f"ws-test-{'r051-' if release else 'leap-' if leap else ''}w{wave}.log").write_text(out)
    passed = sum(int(m) for m in re.findall(r"test result: ok\. (\d+) passed", out))
    failed = sum(int(m) for m in re.findall(r"(\d+) failed", out))
    comp = len(re.findall(r"^error(\[E\d+\])?:", out, re.M))
    say(f"== cargo test --workspace: {passed} passed, {failed} failed, {comp} compile errors")
    if failed or comp:
        gates.append("workspace tests")
    out, _ = sh(f"cargo test -p {engine_pkg} --test byte_identical 2>&1")
    bi = "2 passed; 0 failed" in out
    say(f"== byte_identical: {'green' if bi else 'RED'}")
    if not bi:
        gates.append("byte_identical")
    # engram-export is outside the workspace, so the workspace tests above never compile it (LG.13).
    # Gate on its own last-line prefix (scripts/check-neuropil.sh prints `[neuropil-check] ...`).
    # SKIPPED fails too: here Engram is the sibling checkout, so a skip means nothing was checked.
    out, _ = sh("bash scripts/check-engram-export.sh 2>&1", timeout=1800)
    last = (out.strip().splitlines() or [""])[-1]
    say(f"== {last}")
    if not last.startswith("[engram-export] check: ok"):
        gates.append("engram-export (out-of-workspace)")

    sh(f"cargo clean -p {engine_pkg} -p {py_pkg}")
    out, rc = sh("maturin build -m py/Cargo.toml --release 2>&1", timeout=2400)
    wheels = sorted(WHEELS.glob(WHEEL_GLOB), key=lambda p: p.stat().st_mtime)
    if wheels:
        sh(f"{py} -m pip install --force-reinstall --no-deps -q {wheels[-1]}")
    so, _ = sh(f"{py} -c \"import glia_py,glob,os;print(glob.glob(os.path.join(os.path.dirname(glia_py.__file__),'*.so'))[0])\"")
    stale, _ = sh(f"find {' '.join(CRATES)} -name '*.rs' -newer {so.strip()}")
    stamp, _ = sh(f"{py} -c 'import glia_py as r; print(r.build_stamp())'")
    say(f"== wheel: {wheels[-1].name if wheels else f'NO {WHEEL_GLOB}'} build rc={rc}, "
        f"stale={len(stale.split())}, stamp {stamp.strip()}")
    if rc or not wheels or stale.strip():
        gates.append("wheel rebuild/stale")
    # LG.6a: the installed wheel's surface (module name included: py/api_surface/lib.txt's
    # `pymodule` line) against the committed snapshots.
    out, rc = sh(f"{py} py/check_api_surface.py 2>&1")
    say(f"== api surface: {(out.strip().splitlines() or ['(no output)'])[-1]}")
    if rc:
        gates.append("py/check_api_surface.py")
    # A12: the CLI and the installed wheel report the same message contracts.
    out, rc = sh(f"PYTHON={py} bash bench/message-contracts/check.sh 2>&1")
    say(f"== message-contracts: {(out.strip().splitlines() or ['(no output)'])[-1]}")
    if rc:
        gates.append("bench/message-contracts/check.sh")
    # LD.2: the pyo3 surface behaves as its convention says, over the wheel just installed.
    # One file per py/src module; each ends on `[surface] <module>: N checks, M failed`.
    for t in sorted((ROOT / "py/tests/surface").glob("test_*.py")):
        out, rc = sh(f"{py} {t.relative_to(ROOT)} 2>&1")
        say(f"== pyo3 surface {t.stem}: {(out.strip().splitlines() or ['(no output)'])[-1]}")
        if rc:
            gates.append(f"pyo3 surface ({t.stem})")

    rp, _ = sh(f"{py} run.py --no-log 2>/dev/null", cwd=SG)
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

    sh(f"{py} run.py --no-log --emit >/dev/null 2>&1", cwd=SG)
    _, lchk = sh(f"{py} run.py --no-log --check >/dev/null 2>&1", cwd=SG)
    if lchk:
        gates.append("run.py --check (legacy-latest.json)")
    mo, _ = sh(f"{py} matrix.py --emit 2>&1", cwd=SG)
    # CF.13a: the summary APPENDS `, <n> n/a` (matrix_vocab.NOT_APPLICABLE cells with no fixture);
    # optional, so a pre-CF.13a matrix.py still parses as 0 n/a.
    m = re.search(r"(\d+) full, (\d+) partial, (\d+) none, (\d+) unknown(?:, (\d+) n/a)?", mo)
    _, chk = sh(f"{py} matrix.py --check >/dev/null 2>&1", cwd=SG)
    tm, _ = sh(f"{py} test_matrix.py 2>&1 | tail -1", cwd=SG)
    tg, _ = sh(f"{py} test_grade.py 2>&1 | tail -1", cwd=SG)
    full, part, none_, unk = map(int, m.groups()[:4]) if m else (0, 0, 0, 0)
    na = int(m.group(5) or 0) if m else 0
    # grid = the APPLICABLE cells: the n/a cells are neither covered nor work left, so they stay
    # out of both numbers and are reported alongside.
    grid = full + part + none_ + unk
    covered = full + part + none_
    say(f"== matrix: {full} full, {part} partial, {none_} none, {unk} unknown, {na} n/a = {covered}/{grid} | check={chk} | legacy check={lchk} | test_matrix: {tm.strip()} | test_grade: {tg.strip()}")
    if chk:
        gates.append("matrix --check")
    if " 0 failed" not in tm:
        gates.append("test_matrix")
    if "all passed" not in tg:
        gates.append("test_grade")

    # fold followups into remaining packets
    rem = {p for w in remaining for p in w}
    if release:
        pat = re.compile(r"\b(C[0A-Z]\.\d+[a-z]?)\b")
        cp = ROOT / "dev-notes/leap-051-corrections.json"
    else:
        pat = re.compile(r"\b(A\d+\.\d+[a-z]?|L[A-G0]\.\d+[a-z]?)\b" if leap else r"\b(A\d+\.\d+[a-c]?)\b")
        cp = ROOT / ("dev-notes/leap-corrections.json" if leap else "dev-notes/packet-corrections.json")
    cf = json.loads(cp.read_text())
    folded = {}
    for r in res:
        src = packet_id(r["packet"])
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
                         "grid": grid, "n/a": na, "invalid_cell_declarations": 0, "check_exit": chk}
    base["test_matrix_py"] = tm.strip()
    base["test_grade_py"] = tg.strip()
    (HERE / "baseline.json").write_text(json.dumps(base, indent=2))

    sp = HERE / ("schedule_051.py" if release else "leap_schedule.py" if leap else "schedule.py")
    s = sp.read_text()
    if leap or release:
        m = re.search(r"^LANDED = \{(.*?)\}$", s, re.M | re.S)
        if f"    {wave}: " not in m.group(1):
            body = m.group(1).rstrip("\n") + f'\n    {wave}: "{" ".join(ids)}",\n'
            s = s[:m.start()] + "LANDED = {" + body + "}" + s[m.end():]
            sp.write_text(s)
    elif f"    {wave}: " not in s:
        s = s.replace("\n}\n\n\ndef _graph_module", f'\n    {wave}: "{" ".join(ids)}",\n}}\n\n\ndef _graph_module', 1)
        sp.write_text(s)
    ver, vrc = sh(f"python3 dev-notes/wave-runner/{sp.name} --verify | tail -1")
    say(f"== {sp.name} --verify: {ver.strip()}")
    if vrc:
        gates.append("schedule --verify")

    if remaining:
        flag = "--release 051 " if release else "--leap " if leap else ""
        tag = "r051-" if release else "leap-" if leap else ""
        out, _ = sh(f"python3 dev-notes/wave-runner/gen_wave.py {flag}{wave+1} /tmp/claude-1000/-home-ivy-Code-glia/wf/{tag}wave{wave+1}.js")
        say("== " + out.strip().replace("\n", " | "))

    if leap or release:
        # Record this wave's 5-hour usage cost and say whether the next wave can start (usage_gate.py).
        sh(f"python3 dev-notes/wave-runner/usage_gate.py --end {'R051-' if release else ''}W{wave}")
        out, _ = sh("python3 dev-notes/wave-runner/usage_gate.py")
        say("== usage for the next wave: " + out.strip().replace("\n", " | "))

    if gates:
        say(f"!! NOT COMMITTING — gates failed: {gates}")
        sys.exit(1)
    what = f"0.5.1 wave W{wave}" if release else f"leap wave W{wave}" if leap else f"wave {wave}"
    msg = (f"bench+plan: {what} landed — {passed} tests, {fixtures} fixtures, matrix {covered}/{grid}"
           + (f", {na} n/a" if na else "") + "\n\n"
           + "\n".join(lines) + "\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>\n"
           + "Claude-Session: https://claude.ai/code/session_01U92Fg6cfqUfbvx14qoM6iw\n")
    Path("/tmp/claude-1000/-home-ivy-Code-glia/closeout.msg").write_text(msg)
    out, rc = sh("git add bench/substrate-gap/results-latest.json bench/substrate-gap/COVERAGE.md bench/substrate-gap/legacy-latest.json "
                 "dev-notes/packet-corrections.json dev-notes/leap-corrections.json dev-notes/leap-051-corrections.json dev-notes/wave-runner && "
                 "git -c user.name='james chahwan' commit -q -F /tmp/claude-1000/-home-ivy-Code-glia/closeout.msg && git log --oneline -1")
    say(f"== committed: {out.strip()}" if rc == 0 else f"!! commit failed: {out}")


if __name__ == "__main__":
    main()
