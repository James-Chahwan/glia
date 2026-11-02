#!/usr/bin/env bash
# build-timing.sh - big-repo build timings as a regression check (HP.3, 0.5.7). Cold `glia build` runs of the
# repos in bench/build-timing/repos.json (twenty, photoprism, prefect, tolgee at their bench/cohort/repos.json pins),
# the median of every committed `[timing]` phase, total CPU and peak RSS, compared with
# bench/build-timing/baseline.json under bench/build-timing/RULE.md (committed before the first baseline).
#
#   bash scripts/build-timing.sh check [opts]      build glia, measure, compare with the baseline
#   bash scripts/build-timing.sh run [opts]        measure into the scratch dir, compare nothing
#   bash scripts/build-timing.sh baseline [opts]   measure, then write bench/build-timing/baseline.json
#                                                  (a declared re-baseline: by hand or a perf packet, never a close-out)
#   bash scripts/build-timing.sh compare <base.json> <new.json> [--only l,..]   compare two summaries (read-only)
#   bash scripts/build-timing.sh fingerprint       this machine's fingerprint (`<cores> cores, <model name>`)
#   bash scripts/build-timing.sh selftest          the compare rules and the load guard on seeded results (no glia)
#
#   opts: --glia <bin>    the glia under test (default: `cargo build --release -p glia-cli` in the target dir, the
#                         binary the cohort gate builds, so a close-out builds it once)
#         --only <l,..>   only these repos (a partial check: its line says so)
#         --runs <n>      builds per repo (default: repos.json's `runs`)
#
# Copies: each pin is `git archive`d once from the cohort's bare clone (${GLIA_COHORT_DIR:-~/.cache/glia-cohort},
# cloned / fetched as cohort.py does when it is missing) into ${GLIA_BUILD_TIMING_DIR:-~/.cache/glia-build-timing}/
# <label>@<sha12>; a row that bench/cohort/repos.json does not mark `public: true` with a GitHub https url and a
# 40-hex sha is refused. Each run's raw results land in <dir>/run/ (<label>.json, summary.json, logs/).
# GLIA_BUILD_TIMING_SKIP=<reason>: print `check: skipped - <reason>` and exit 3 (never a silent pass).
#
# Lines (stderr-free: everything is stdout), gated on by dev-notes/wave-runner/closeout.py's `timing` gate:
#   [build-timing] run label=<l> run=<i> wall_ms=<w> cpu_ms=<c> rss_mb=<m> load=<l>
#   [build-timing] check: ok - <r> repos, <p> phases, <g> gains, 0 regressions          exit 0
#   [build-timing] check: FAILED (<n> regressions: <label>/<value> <base>-><new>, ..)    exit 1
#   [build-timing] check: FAILED (<reason>)                                             exit 2
#   [build-timing] check: skipped - <reason>                                            exit 3
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
exec python3 -I - "$HERE" "$@" <<'PY'
import json, os, re, shutil, statistics, subprocess, sys, tempfile, time
from pathlib import Path

HERE = Path(sys.argv[1])
ARGS = sys.argv[2:]
CMD = ARGS.pop(0) if ARGS else ""
BT = HERE / "bench" / "build-timing"
COHORT_REPOS = HERE / "bench" / "cohort" / "repos.json"
DIR = Path(os.environ.get("GLIA_BUILD_TIMING_DIR") or Path.home() / ".cache" / "glia-build-timing").expanduser()
CLONES = Path(os.environ.get("GLIA_COHORT_DIR") or Path.home() / ".cache" / "glia-cohort").expanduser()
TARGET = Path(os.environ.get("CARGO_TARGET_DIR") or HERE / "target")
LOADAVG = os.environ.get("GLIA_BUILD_TIMING_LOADAVG") or "/proc/loadavg"   # the selftest seeds one
LOAD_WAIT = float(os.environ.get("GLIA_BUILD_TIMING_LOAD_WAIT") or 120)
LOAD_SHARE = 0.5
POLL = 5.0
HEX40 = re.compile(r"^[0-9a-f]{40}$")
# RULE.md, verbatim: (ratio, absolute floor) per value kind; a gain is below GAIN x base.
CPU_RATIO, WALL_RATIO, WALL_ABS, PHASE_RATIO, PHASE_ABS, RSS_RATIO, GAIN = 1.20, 1.30, 1000.0, 1.50, 300.0, 1.25, 0.80
REPO_PHASES = ("walk", "parse", "const_scan", "grafts", "language_build")
BUILD_PHASES = ("resolve", "post", "finalize", "total")
PERSIST_PHASES = ("persist", "spans", "encode", "write")
PHASES = REPO_PHASES + BUILD_PHASES + PERSIST_PHASES
VERB = {"check": "check", "baseline": "baseline", "run": "run", "compare": "check"}.get(CMD, CMD or "usage")


def say(s):
    print(f"[build-timing] {s}", flush=True)


def finish(kind, msg, code):
    """The verdict line and exit: kind ok | FAILED | skipped."""
    sep = " - " if kind in ("ok", "skipped") else " "
    say(f"{VERB}: {kind}{sep}{msg}")
    sys.exit(code)


def fail(msg):
    finish("FAILED", f"({msg})", 2)


def opts(args):
    o = {"glia": "", "only": [], "runs": None, "pos": []}
    i = 0
    while i < len(args):
        a = args[i]
        if a in ("--glia", "--only", "--runs"):
            if i + 1 >= len(args):
                fail(f"{a} takes a value")
            v = args[i + 1]
            if a == "--glia":
                o["glia"] = v
            elif a == "--only":
                if not re.fullmatch(r"[A-Za-z0-9._,-]+", v):
                    fail("--only takes labels")
                o["only"] = [x for x in v.split(",") if x]
            else:
                if not re.fullmatch(r"[1-9][0-9]?", v):
                    fail("--runs takes 1..99")
                o["runs"] = int(v)
            i += 2
        else:
            o["pos"].append(a)
            i += 1
    return o


# --- the machine and the load --------------------------------------------------------------------------
def cores():
    return os.cpu_count() or 1


def fingerprint():
    model = "unknown"
    try:
        for line in open("/proc/cpuinfo"):
            if line.startswith("model name"):
                model = " ".join(line.split(":", 1)[1].split())
                break
    except OSError:
        pass
    return f"{cores()} cores, {model}"


def load1():
    try:
        return float(open(LOADAVG).read().split()[0])
    except (OSError, ValueError, IndexError):
        return 0.0


def wait_quiet():
    """Wait (at most LOAD_WAIT s) for the 1-min load to fall under LOAD_SHARE x cores; the load it reached, or a
    skip (exit 3)."""
    limit, t0 = LOAD_SHARE * cores(), time.monotonic()
    while True:
        l = load1()
        if l < limit:
            return l
        if time.monotonic() - t0 >= LOAD_WAIT:
            finish("skipped", f"load {l:.2f} on {cores()} cores", 3)
        time.sleep(min(POLL, max(0.1, LOAD_WAIT - (time.monotonic() - t0))))


# --- the repos and their copies --------------------------------------------------------------------------
def repos(only):
    try:
        mine = json.loads((BT / "repos.json").read_text())
        pins = {r["label"]: r for r in json.loads(COHORT_REPOS.read_text())["repos"]}
    except (OSError, ValueError, KeyError) as e:
        fail(f"repos.json unreadable: {e.__class__.__name__}")
    labels = [r["label"] for r in mine.get("repos", [])]
    unknown = [x for x in only if x not in labels]
    if unknown:
        fail(f"--only names no repo of bench/build-timing/repos.json: {','.join(unknown)}")
    out = []
    for lab in labels:
        if only and lab not in only:
            continue
        r = pins.get(lab)
        if r is None:
            fail(f"{lab}: not in bench/cohort/repos.json")
        if r.get("public") is not True or not str(r.get("url", "")).startswith("https://github.com/") \
                or not HEX40.match(str(r.get("sha", ""))):
            fail(f"{lab}: refused, not a public pinned row of bench/cohort/repos.json")
        out.append({"label": lab, "url": r["url"], "sha": r["sha"]})
    return out, int(mine.get("runs") or 3)


def git(*a, cwd=None, check=True):
    p = subprocess.run(["git", *a], cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True)
    if check and p.returncode:
        raise RuntimeError(f"git {' '.join(a[:3])}: {p.stderr.decode(errors='replace').strip()[-200:]}")
    return p


def copy_of(r):
    """<DIR>/<label>@<sha12>, archived once from the bare clone (a `.build-timing-sha` file marks a whole copy)."""
    dest = DIR / f"{r['label']}@{r['sha'][:12]}"
    mark = dest / ".build-timing-sha"
    if mark.is_file() and mark.read_text().strip() == r["sha"]:
        return dest
    gd = CLONES / f"{r['label']}.git"
    try:
        if not gd.exists():
            gd.parent.mkdir(parents=True, exist_ok=True)
            git("clone", "--bare", "--quiet", r["url"], str(gd))
        if git("-C", str(gd), "cat-file", "-e", f"{r['sha']}^{{commit}}", check=False).returncode:
            git("-C", str(gd), "fetch", "--quiet", r["url"], r["sha"], check=False)
        part = DIR / f".{r['label']}@{r['sha'][:12]}.part"
        shutil.rmtree(part, ignore_errors=True)
        shutil.rmtree(dest, ignore_errors=True)
        part.mkdir(parents=True)
        arch = subprocess.Popen(["git", "-C", str(gd), "archive", "--format=tar", r["sha"]], stdout=subprocess.PIPE,
                                stdin=subprocess.DEVNULL, stderr=subprocess.PIPE)
        tar = subprocess.run(["tar", "-x", "-C", str(part)], stdin=arch.stdout, capture_output=True)
        arch.stdout.close()
        if arch.wait() or tar.returncode:
            raise RuntimeError(f"git archive {r['sha'][:12]} | tar: {(arch.stderr.read() + tar.stderr).decode(errors='replace').strip()[-200:]}")
        (part / ".build-timing-sha").write_text(r["sha"] + "\n")
        os.replace(part, dest)
    except (OSError, RuntimeError) as e:
        fail(f"copy of {r['label']}: {e}")
    say(f"copy {r['label']}@{r['sha'][:12]} -> {dest}")
    return dest


def glia_binary(given):
    if given:
        b = Path(given).expanduser().resolve()
        if not (b.is_file() and os.access(b, os.X_OK)):
            fail(f"no glia at {given}")
        return b
    say(f"build glia (release) in {TARGET}")
    DIR.mkdir(parents=True, exist_ok=True)
    log = DIR / "build.log"
    with open(log, "w") as f:
        rc = subprocess.run(["cargo", "build", "--release", "--quiet", "-p", "glia-cli"], cwd=HERE, stdout=f,
                            stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL).returncode
    if rc:
        print("".join(open(log).readlines()[-20:]), end="")
        fail("build glia")
    return TARGET / "release" / "glia"


# --- one build ------------------------------------------------------------------------------------------
MS = r"([0-9]+(?:\.[0-9]+)?)ms"


def parse_timing(text):
    """The phases (ms) of one build's stderr: the `[timing] repo=` lines summed, the `[timing] build` and
    `[timing] persist` lines (the last of each), and slowest_pass. Missing lines -> a ValueError."""
    ph, slowest, seen = {p: 0.0 for p in PHASES}, None, set()
    for line in text.splitlines():
        if line.startswith("[timing] repo="):
            for p in REPO_PHASES:
                m = re.search(rf" {p}={MS}", line)
                if m:
                    ph[p] += float(m.group(1))
            seen.add("repo")
        elif line.startswith("[timing] build "):
            for p in BUILD_PHASES:
                m = re.search(rf" {p}={MS}", line)
                if m:
                    ph[p] = float(m.group(1))
            m = re.search(r" slowest_pass=(\S+)", line)
            slowest = m.group(1) if m else None
            seen.add("build")
        elif line.startswith("[timing] persist "):
            m = re.search(rf"^\[timing\] persist writer=\S+ {MS}", line)
            if m:
                ph["persist"] = float(m.group(1))
            for p in ("spans", "encode", "write"):
                m = re.search(rf" {p}_ms=([0-9]+(?:\.[0-9]+)?)", line)
                if m:
                    ph[p] = float(m.group(1))
            seen.add("persist")
    missing = {"repo", "build", "persist"} - seen
    if missing:
        raise ValueError(f"no [timing] {'/'.join(sorted(missing))} line")
    return ph, slowest


def tmpdir():
    t = os.environ.get("TMPDIR")
    if t and not os.path.realpath(t).startswith("/tmp"):
        return t
    return os.environ.get("GLIA_CLOSEOUT_TMP") or str(Path.home() / ".cache" / "glia-tmp")


def one_build(binary, copy, label, i, logs):
    shutil.rmtree(copy / ".glia", ignore_errors=True)
    out = DIR / "out" / label
    shutil.rmtree(out, ignore_errors=True)
    out.parent.mkdir(parents=True, exist_ok=True)
    load = wait_quiet()
    env = {k: v for k, v in os.environ.items() if k not in ("GLIA_THREADS", "GLIA_NO_PERSIST")}
    env.update(TMPDIR=tmpdir(), LC_ALL="C")
    Path(env["TMPDIR"]).mkdir(parents=True, exist_ok=True)
    log = logs / f"{label}-{i}.log"
    with open(log, "w") as f:
        t0 = time.monotonic()
        p = subprocess.Popen([str(binary), "build", str(copy), "--no-incremental", "--out", str(out)], stdout=f,
                             stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL, env=env, cwd=str(DIR))
        _, status, ru = os.wait4(p.pid, 0)
        wall = (time.monotonic() - t0) * 1000.0
    p.returncode = os.waitstatus_to_exitcode(status)
    shutil.rmtree(out, ignore_errors=True)
    shutil.rmtree(copy / ".glia", ignore_errors=True)
    text = log.read_text(errors="replace")
    if p.returncode:
        fail(f"glia build {label} rc={p.returncode}, see {log}")
    try:
        ph, slowest = parse_timing(text)
    except ValueError as e:
        fail(f"{label}: {e}, see {log}")
    row = {"run": i, "wall_ms": round(wall, 1), "cpu_ms": round((ru.ru_utime + ru.ru_stime) * 1000.0, 1),
           "rss_mb": round(ru.ru_maxrss / 1024.0, 1), "load": round(load, 2), "phases": ph, "slowest_pass": slowest}
    say(f"run label={label} run={i} wall_ms={row['wall_ms']:.0f} cpu_ms={row['cpu_ms']:.0f} "
        f"rss_mb={row['rss_mb']:.0f} load={row['load']:.2f}")
    return row


def median_of(rows):
    med = lambda xs: round(statistics.median(xs), 1)
    out = {k: med([r[k] for r in rows]) for k in ("wall_ms", "cpu_ms", "rss_mb")}
    out["phases"] = {p: med([r["phases"][p] for r in rows]) for p in PHASES}
    slow = [r["slowest_pass"] for r in rows if r.get("slowest_pass")]
    out["slowest_pass"] = sorted(slow, key=lambda s: (-slow.count(s), s))[0] if slow else None
    return out


def measure(o):
    """Every build of the run, interleaved by round; writes <DIR>/run/ and returns the summary."""
    if os.environ.get("GLIA_BUILD_TIMING_SKIP"):
        finish("skipped", os.environ["GLIA_BUILD_TIMING_SKIP"], 3)
    todo, runs = repos(o["only"])
    runs = o["runs"] or runs
    # the box must be quiet before anything starts (a skip measures nothing, and builds nothing)
    wait_quiet()
    binary = glia_binary(o["glia"])
    try:
        version = subprocess.run([str(binary), "--version"], capture_output=True, text=True,
                                 stdin=subprocess.DEVNULL).stdout.strip()
    except OSError as e:
        fail(f"glia --version: {e}")
    copies = {r["label"]: copy_of(r) for r in todo}
    run_dir = DIR / "run"
    shutil.rmtree(run_dir, ignore_errors=True)
    logs = run_dir / "logs"
    logs.mkdir(parents=True)
    head = git("rev-parse", "HEAD", cwd=HERE, check=False).stdout.decode().strip()
    say(f"run glia={version} head={head[:12]} repos={','.join(copies)} runs={runs} fingerprint={fingerprint()}")
    rows = {lab: [] for lab in copies}
    for i in range(1, runs + 1):
        for r in todo:
            rows[r["label"]].append(one_build(binary, copies[r["label"]], r["label"], i, logs))
    # glia_head is this checkout's HEAD at the run; glia_source says whether this tree built the glia (`built`) or
    # --glia handed one in (`given`: the version line's build stamp names it, not the head)
    summary = {"fingerprint": fingerprint(), "glia": version, "glia_head": head, "runs": runs,
               "glia_source": "given" if o["glia"] else "built",
               "only": o["only"], "measured_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "repos": {}}
    for r in todo:
        lab = r["label"]
        res = {"label": lab, "sha": r["sha"], "runs": rows[lab], "median": median_of(rows[lab])}
        (run_dir / f"{lab}.json").write_text(json.dumps(res, indent=1, sort_keys=True) + "\n")
        m = res["median"]
        summary["repos"][lab] = {"sha": r["sha"], "median": m,
                                 "wall_ms_runs": [x["wall_ms"] for x in rows[lab]],
                                 "cpu_ms_runs": [x["cpu_ms"] for x in rows[lab]],
                                 "rss_mb_runs": [x["rss_mb"] for x in rows[lab]]}
        say(f"median label={lab} wall_ms={m['wall_ms']:.0f} cpu_ms={m['cpu_ms']:.0f} rss_mb={m['rss_mb']:.0f} "
            f"total_ms={m['phases']['total']:.0f} slowest_pass={m['slowest_pass']}")
    (run_dir / "summary.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n")
    return summary


# --- the rule -------------------------------------------------------------------------------------------
def fmt(v):
    return f"{v:.0f}" if v >= 10 else f"{v:.1f}"


def compare(base, new, only):
    """RULE.md applied: (regressions, gains, phases compared, repos compared, skip reason or None)."""
    if base.get("fingerprint") != new.get("fingerprint"):
        return [], [], 0, 0, f"machine differs ({base.get('fingerprint')} vs {new.get('fingerprint')})"
    regs, gains, nph, nrep = [], [], 0, 0
    for lab in sorted(base.get("repos", {})):
        if only and lab not in only:
            continue
        b = base["repos"][lab]["median"]
        got = (new.get("repos") or {}).get(lab)
        if got is None:
            regs.append(f"{lab}/missing")
            continue
        n = got["median"]
        nrep += 1
        if n["cpu_ms"] > b["cpu_ms"] * CPU_RATIO:
            regs.append(f"{lab}/cpu_ms {fmt(b['cpu_ms'])}->{fmt(n['cpu_ms'])}")
        elif n["cpu_ms"] < b["cpu_ms"] * GAIN:
            gains.append(f"{lab}/cpu_ms {fmt(b['cpu_ms'])}->{fmt(n['cpu_ms'])}")
        if n["wall_ms"] > b["wall_ms"] * WALL_RATIO and n["wall_ms"] > b["wall_ms"] + WALL_ABS:
            regs.append(f"{lab}/wall_ms {fmt(b['wall_ms'])}->{fmt(n['wall_ms'])}")
        if n["rss_mb"] > b["rss_mb"] * RSS_RATIO:
            regs.append(f"{lab}/rss_mb {fmt(b['rss_mb'])}->{fmt(n['rss_mb'])}")
        elif n["rss_mb"] < b["rss_mb"] * GAIN:
            gains.append(f"{lab}/rss_mb {fmt(b['rss_mb'])}->{fmt(n['rss_mb'])}")
        for p in PHASES:
            bp, np_ = b["phases"].get(p), n["phases"].get(p)
            if bp is None:
                continue
            nph += 1
            if np_ is None:
                regs.append(f"{lab}/{p} {fmt(bp)}->none")
            elif np_ > bp * PHASE_RATIO and np_ > bp + PHASE_ABS:
                regs.append(f"{lab}/{p} {fmt(bp)}->{fmt(np_)}")
            elif np_ < bp * GAIN:
                gains.append(f"{lab}/{p} {fmt(bp)}->{fmt(np_)}")
    return regs, gains, nph, nrep, None


def judge(base, new, only):
    regs, gains, nph, nrep, skip = compare(base, new, only)
    if skip:
        finish("skipped", skip, 3)
    for g in gains:
        say(f"gain {g}")
    for r in regs:
        say(f"regression {r}")
    partial = f" (partial: only {','.join(only)})" if only else ""
    if regs:
        shown = ", ".join(regs[:12]) + (", .." if len(regs) > 12 else "")
        finish("FAILED", f"({len(regs)} regressions: {shown}){partial}", 1)
    finish("ok", f"{nrep} repos, {nph} phases, {len(gains)} gains, 0 regressions{partial}", 0)


def read_json(path, what):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError) as e:
        fail(f"{what} unreadable: {path} ({e.__class__.__name__})")


# --- selftest -------------------------------------------------------------------------------------------
def selftest():
    def seeded():
        ph = {"walk": 800.0, "parse": 6000.0, "const_scan": 90.0, "grafts": 1600.0, "language_build": 2500.0,
              "resolve": 700.0, "post": 850.0, "finalize": 130.0, "total": 13000.0, "persist": 2600.0,
              "spans": 300.0, "encode": 700.0, "write": 900.0}
        med = lambda w, c, r, k: {"wall_ms": w, "cpu_ms": c, "rss_mb": r, "slowest_pass": "http:300.0ms",
                                  "phases": {p: v * k for p, v in ph.items()}}
        return {"fingerprint": "16 cores, Seeded CPU", "glia": "glia x", "runs": 3, "repos": {
            "twenty": {"sha": "a" * 40, "median": med(16000.0, 60000.0, 2400.0, 1.0)},
            "tolgee": {"sha": "b" * 40, "median": med(2500.0, 21000.0, 900.0, 0.3)}}}
    cases, fails = [], 0

    def case(name, want_rc, needle, mutate=None, only=(), run=None):
        nonlocal fails
        base, new = seeded(), seeded()
        if mutate:
            mutate(new)
        with tempfile.TemporaryDirectory() as t:
            if run is None:
                bp, np_ = Path(t) / "base.json", Path(t) / "new.json"
                bp.write_text(json.dumps(base))
                np_.write_text(json.dumps(new))
                cmd = ["bash", str(HERE / "scripts/build-timing.sh"), "compare", str(bp), str(np_)]
                if only:
                    cmd += ["--only", ",".join(only)]
                p = subprocess.run(cmd, capture_output=True, text=True, stdin=subprocess.DEVNULL)
            else:
                p = run(Path(t))
        out = p.stdout + p.stderr
        ok = p.returncode == want_rc and needle in out
        cases.append(name)
        if ok:
            print(f"selftest {name}: ok")
        else:
            fails += 1
            print(f"selftest {name}: FAILED (rc={p.returncode} want {want_rc}, needle {needle!r})")
            print("\n".join(out.strip().splitlines()[-3:]))

    def scale(lab, key, k, phase=False):
        def f(s):
            m = s["repos"][lab]["median"]
            if phase:
                m["phases"][key] *= k
            else:
                m[key] *= k
        return f

    case("identical", 0, "check: ok - 2 repos, 26 phases, 0 gains, 0 regressions")
    case("language_build+60%", 1, "twenty/language_build 2500->4000", scale("twenty", "language_build", 1.6, True))
    case("parse-30%", 0, "check: ok - 2 repos, 26 phases, 1 gains, 0 regressions", scale("twenty", "parse", 0.7, True))
    case("small-phase+60%-under-300ms", 0, "0 regressions", scale("tolgee", "walk", 1.6, True))
    case("cpu+25%", 1, "twenty/cpu_ms 60000->75000", scale("twenty", "cpu_ms", 1.25))
    case("cpu+15%", 0, "0 regressions", scale("twenty", "cpu_ms", 1.15))
    case("wall+35%", 1, "twenty/wall_ms 16000->21600", scale("twenty", "wall_ms", 1.35))
    case("rss+30%", 1, "tolgee/rss_mb 900->1170", scale("tolgee", "rss_mb", 1.3))
    case("rss-25%", 0, "1 gains", scale("tolgee", "rss_mb", 0.75))
    case("missing-repo", 1, "tolgee/missing", lambda s: s["repos"].pop("tolgee"))
    case("only", 0, "check: ok - 1 repos, 13 phases, 0 gains, 0 regressions (partial: only twenty)",
         lambda s: s["repos"].pop("tolgee"), only=("twenty",))
    case("machine-differs", 3, "check: skipped - machine differs", lambda s: s.update(fingerprint="8 cores, Other"))

    # a 35% wall rise that stays under +1000 ms (2500 -> 3375) is no regression
    case("wall+35%-under-1000ms", 0, "0 regressions", scale("tolgee", "wall_ms", 1.35))

    def load_run(t):
        la = t / "loadavg"
        la.write_text(f"{cores() * 2:.2f} 1.00 1.00 1/1 1\n")
        env = dict(os.environ, GLIA_BUILD_TIMING_LOADAVG=str(la), GLIA_BUILD_TIMING_LOAD_WAIT="0",
                   GLIA_BUILD_TIMING_DIR=str(t / "dir"))
        return subprocess.run(["bash", str(HERE / "scripts/build-timing.sh"), "run", "--glia", "/bin/true"],
                              capture_output=True, text=True, env=env, stdin=subprocess.DEVNULL)
    case("load-skip", 3, f"run: skipped - load {cores() * 2:.2f} on {cores()} cores", run=load_run)

    def parse_run(t):
        text = ("[timing] repo=a walk=1.0ms parse=2.0ms const_scan=0.5ms grafts=3.0ms language_build=4.0ms\n"
                "[timing] repo=b walk=1.0ms parse=2.0ms const_scan=0.5ms grafts=3.0ms language_build=4.0ms\n"
                "[timing] build repos=2 resolve=5.0ms post=6.0ms finalize=7.0ms external_cells=0.1ms total=90.0ms "
                "slowest_pass=http:4.0ms\n"
                "[timing] persist writer=cli 534.5ms dir=/x/.glia/graph spans_ms=128.8 encode_ms=131.2 write_ms=209.1\n")
        ph, slow = parse_timing(text)
        good = (ph["walk"] == 2.0 and ph["language_build"] == 8.0 and ph["total"] == 90.0 and ph["persist"] == 534.5
                and ph["write"] == 209.1 and slow == "http:4.0ms")
        try:
            parse_timing("[timing] build repos=1 resolve=1.0ms post=1.0ms finalize=1.0ms total=3.0ms slowest_pass=none\n")
            good = False
        except ValueError:
            pass
        return subprocess.CompletedProcess([], 0 if good else 1, "parsed" if good else f"bad {ph} {slow}", "")
    case("parse-timing-lines", 0, "parsed", run=parse_run)

    if fails == 0:
        print(f"[build-timing] selftest: ok - {len(cases)} cases")
        sys.exit(0)
    print(f"[build-timing] selftest: FAILED ({fails} of {len(cases)} cases)")
    sys.exit(1)


# --- dispatch -------------------------------------------------------------------------------------------
def main():
    if CMD == "fingerprint":
        print(fingerprint())
        return
    if CMD == "selftest":
        selftest()
    if CMD not in ("check", "run", "baseline", "compare"):
        print("usage: build-timing.sh check|run|baseline [--glia <bin>] [--only <l,..>] [--runs <n>] "
              "| compare <base.json> <new.json> [--only <l,..>] | fingerprint | selftest", file=sys.stderr)
        sys.exit(2)
    o = opts(ARGS)
    if DIR.resolve() == HERE.resolve() or HERE.resolve() in DIR.resolve().parents:
        fail("GLIA_BUILD_TIMING_DIR is inside this checkout")
    if CMD == "compare":
        if len(o["pos"]) != 2:
            fail("compare takes <base.json> <new.json>")
        judge(read_json(o["pos"][0], "base"), read_json(o["pos"][1], "new"), o["only"])
    if o["pos"]:
        fail(f"unexpected argument {o['pos'][0]}")
    if CMD == "check":
        if os.environ.get("GLIA_BUILD_TIMING_SKIP"):
            finish("skipped", os.environ["GLIA_BUILD_TIMING_SKIP"], 3)
        base = read_json(BT / "baseline.json", "bench/build-timing/baseline.json")
        if base.get("fingerprint") != fingerprint():
            finish("skipped", f"machine differs ({base.get('fingerprint')} vs {fingerprint()})", 3)
        new = measure(o)
        judge(base, new, o["only"])
    summary = measure(o)
    if CMD == "baseline":
        if o["only"]:
            fail("baseline measures every repo (no --only)")
        (BT / "baseline.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n")
        finish("ok", f"wrote bench/build-timing/baseline.json ({len(summary['repos'])} repos, "
                     f"{summary['runs']} runs, {summary['fingerprint']})", 0)
    finish("ok", f"{len(summary['repos'])} repos measured into {DIR / 'run'}", 0)


main()
PY
