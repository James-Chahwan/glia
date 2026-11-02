#!/usr/bin/env python3
"""End-of-wave close-out, as one command. Commits only if every gate is clean.

    python3 closeout.py <wave> <workflow-run-id>      e.g.  closeout.py 8 wf_63d9b695-7a3
    python3 closeout.py --leap <wave> <run-id>        the 0.5.0 leap (waves W0..)
    python3 closeout.py --leap <wave> <run1>,<run2>   a packet re-run merged over its wave (later wins)
    python3 closeout.py --leap <wave> --plan-only     print the wave's packets and exit
    python3 closeout.py --release 051|052 <wave> <run-id>  0.5.1 / 0.5.2: schedule_<r>.py,
                                                      leap-<r>-corrections.json (ids C*.* / D*.*), the leap venv

--leap schedules through leap_schedule.py, records the wave's 5-hour usage
cost (usage_gate.py --end) and prints whether the next wave can start, folds followups into
leap-corrections.json (ids A*.* and L*.*), and records LANDED in leap_schedule.py.

Steps (README order): commits + per-packet status from the workflow journal,
workspace tests + byte_identical, fmt (F0.5), clippy (HP.1), rustdoc (FD.23b), the out-of-workspace engram-export check
(scripts/check-engram-export.sh), the rank gate (EG.3), the cohort gate (FH.12:
scripts/cohort-gate.sh check, then its ratchet when every gate is green), the build timings (HP.3:
scripts/build-timing.sh check, advisory per wave, gating at the release), each gate's `[gate-asks]` line (GP.12: the
packets whose acceptance named it and those whose commits touched its paths; `--plan-only` prints the asks), wheel
rebuild + staleness check, run.py,
matrix --emit/--check, python suites, fold the wave's followups into every
remaining packet's correction, then (only when every gate is green, HP.1) update baseline.json (MEASURED)
and LANDED, schedule --verify, render the next wave, ratchet, commit.

Prints a DELTA against the previous baseline so an unexpected blind spot,
partial, forbid violation or missing cell is visible instead of buried.

Every line it prints ends `[t+<n>s, step <m>s]` (elapsed, and since the line before), on stdout, in the
recorded log (WF/closeout-<tag>w<wave>.log) and in the commit body, and the close-out ends on
`[closeout] timing fmt=<s> clippy=<s> tests=<s> rustdoc=<s> rank_gate=<s> cohort_gate=<s> build_timing=<s>
wheel=<s> bench=<s> engram=<s> total=<s>` (GP.7; HP.3's build_timing; a step that did not run reads `-`).

Gate reuse (HP.1, 0.5.7 §4b, James 2026-11-01): every recorded gate (tests, fmt, clippy, rustdoc, rank,
cohort, wheel, bench, and HP.3's timing) keys its result by a digest of exactly what it reads at HEAD (`git ls-tree` blobs of
its input set, derived from `cargo metadata` and an include_str! scan, plus its env lines); the record lives
outside the tree (~/.cache/glia-closeout/gate-record.json, GLIA_CLOSEOUT_RECORD overrides) and a gate whose
digest matches a green entry is reused (`[closeout] reuse <gate> from <commit12> ..`), else it runs
(`[closeout] run <gate> (<reason>)`). GLIA_CLOSEOUT_FRESH=all|<gate,..> or `--fresh <gate,..>` forces a run.

Re-judge (HP.2, James 2026-11-02): when only what the rank or cohort gate judges against moved (its baselines, gate.json
`accepted`) and its last result measured exactly HEAD's measure inputs, at the same env, with its run files intact on
disk, the close-out judges that measurement again (`rank-gate.sh recheck` / `cohort-gate.sh recheck`) instead of
re-measuring: `[closeout] rejudge <gate> from <commit12> (judge inputs moved: <first 3 paths>)`. A re-judged green
counts for reuse and for the release set, never for a ratchet.

    python3 closeout.py --digests                      print each gate's digest at HEAD, run nothing
    python3 closeout.py --release 057 <wave> --plan-only   the wave's packets plus each gate's decision
"""
import glob, hashlib, json, os, posixpath, re, shutil, subprocess, sys, time
from contextlib import contextmanager
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
RELEASES = ("051", "052", "053", "054", "056", "057", "058")
RELEASE_PREFIX = {"051": "C", "052": "D", "053": "E", "054": "F", "056": "G", "057": "H", "058": "J"}   # packet-id family per release
WF = Path("/tmp/claude-1000/-home-ivy-Code-glia/wf")  # wave logs / next-wave scripts; /tmp is cleared on reboot
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
# EG.3: a wave whose packet commits touch one of these runs the rank gate (scripts/rank-gate.sh check).
RANKED_PATHS = ("core/", "code-domain/", "graph/", "parsers/", "engine/", "activation/",
                "bench/rank-gate/", "scripts/rank-gate.sh")
# James 2026-10-30 ("experiment quickly to bring the gate times down"): a wave that changed no ranking
# or answer code runs the rank gate's fast tier, the five small snapshots 3 at a time (measured 136 s
# against ~13 min for all six; the glia snapshot alone ran 478 s). A wave touching RANK_CODE_PATHS, and
# the release's last close-out, run every snapshot. A partial check never ratchets.
RANK_CODE_PATHS = ("engine/src/rank/", "engine/src/intent/", "engine/src/calibrate/", "engine/src/semantic/",
                   "engine/src/answers.rs", "engine/src/find.rs", "engine/src/diff_impact.rs",
                   "engine/src/review.rs", "engine/src/pack", "activation/", "embed/",
                   "code-domain/src/profile.rs", "engine/examples/rank_eval.rs", "bench/rank-gate/",
                   "scripts/rank-gate.sh")
RANK_FAST = "shell,websocket,samples,commons-lang,commons-io"
# GP.7: both tiers run the rank gate's snapshots 3 at a time (an explicit GLIA_RANK_GATE_JOBS wins). In the full tier
# rank-gate.sh starts the glia snapshot first (GLIA_RANK_GATE_FIRST, W20-W27: 475-516 s of a ~690 s serial run), so it
# runs as its own job while the other five run 2 at a time beside it. To go back to the serial full tier, set
# GLIA_RANK_GATE_JOBS=1 (or drop the `if fast` guard below).
RANK_GATE_JOBS = "3"
# GP.7: the `[closeout] timing` steps, in the order the line prints them. clippy runs since HP.1 (James
# 2026-11-02: `cargo clippy --workspace --all-targets -- -D warnings`, a recorded, reusable gate).
# HP.3: build_timing, after cohort_gate (scripts/build-timing.sh check runs on the cohort gate's binary).
TIMING_STEPS = ("fmt", "clippy", "tests", "rustdoc", "rank_gate", "cohort_gate", "build_timing", "wheel", "bench",
                "engram")
# James 2026-10-30 ("it's not small repos all the time ... we've built enough waves we might wanna do the full"):
# the fast tiers are per wave, but every FULL_GATES_EVERY waves since a gate's last full check it runs in full,
# so a big-repo regression surfaces within a few waves of its cause. baseline.json `gates_last_full` holds
# the wave each gate last ran in full (the release's last close-out is always full).
FULL_GATES_EVERY = 4
RANK_GATE_TIMEOUT = 7200   # seconds: EG.3 measured a whole check at 313 s (6 snapshots); 2x that, at least 7200
# HP.3: scripts/build-timing.sh check builds four big repos 3 times each (about 75 s on a quiet box), waiting up to 120 s
# for a quiet load before each build; with the archive copies made once and a cold cargo build, at least 3600.
BUILD_TIMING_TIMEOUT = 3600
BUILD_TIMING_REPOS = "bench/build-timing/repos.json"
COHORT_REPOS = "bench/cohort/repos.json"
# FH.12: a wave whose packet commits touch one of these runs the cohort gate (scripts/cohort-gate.sh check)
# after the rank gate: engine / graph / parser / code-domain changes, and the gate's own inputs.
COHORT_PATHS = ("engine/", "graph/", "parsers/", "code-domain/", "bench/cohort/", "bench/sibyl/",
                "scripts/cohort-gate.sh")
COHORT_GATE_TIMEOUT = 14400   # seconds: FH.12's baseline (cohort + health + Sibyl, --jobs 4, sibling agents loading the box) took 6136 s
# James 2026-10-26 ("2 hours is crazy ... fine for ... release"): a wave's close-out checks only the fast
# tier, the cohort repos whose tasks + health summed under ~2 min of task time in FH.12's baseline
# (twenty alone summed ~10,200 s, medusa ~3,500 s), plus the whole Sibyl cover. The release's last
# close-out (`remaining` empty) runs the full cohort. A partial check is never ratcheted.
COHORT_FAST = "websocket,commons-text,click,scaffold-eth-2,shell,microservices-demo,gin-vue-admin,zora-protocol"
# GP.12 (James 2026-10-30, 0.5.6 plan §4): wave agents never run the rank or cohort gate; a packet's acceptance
# that names one is what the close-out checks for that packet. So each gate's result is attributed: the packets
# whose acceptance named it (asked_by) and the packets whose commits touched its paths (touched_by), on one
# `== [gate-asks] <gate> result=<r> asked_by=<n> (<ids>) touched_by=<m> (<ids>)` line, and a FAILED gate's
# entry in the gates list names both. Named = a gate script run (`rank-gate.sh check|run|baseline|ratchet`,
# `rank-gate check`, `cohort-gate.sh`) or the gate by name, unless the text says it is unaffected; the brief's
# own quoted rule and a `grep` over a file that names the command are not asks.
GATE_PATHS = {"rank": RANKED_PATHS, "cohort": COHORT_PATHS}
GATE_ASK_RE = {
    "rank": re.compile(r"rank-gate\.sh\s+(?:check|run|baseline-add|baseline|ratchet)\b|\brank-gate check\b"
                       r"|\brank gate\b(?! unaffected| is the close-out's)", re.I),
    "cohort": re.compile(r"cohort-gate\.sh\b|\bcohort gate\b(?! unaffected| is the close-out's)", re.I),
}
GATE_ASK_IGNORE = re.compile(r"`grep [^`]*`|never run the (?:full )?rank or cohort gate", re.I)


def gate_asks(packets, ids):
    """GP.12: {gate: sorted ids of the packets among `ids` whose acceptance names that gate}."""
    asks = {g: [] for g in GATE_ASK_RE}
    for i in ids:
        text = GATE_ASK_IGNORE.sub(" ", str((packets.get(i) or {}).get("acceptance") or ""))
        for g, rx in GATE_ASK_RE.items():
            if rx.search(text):
                asks[g].append(i)
    return {g: sorted(v) for g, v in asks.items()}


def gate_result(line):
    """A gate's last line as one word: ok, ok-partial, skipped, FAILED, or not-run (no line)."""
    if not line:
        return "not-run"
    if "check: ok" in line:
        return "ok-partial" if "(partial: only" in line else "ok"
    return "skipped" if "check: skipped" in line else "FAILED"


def gate_attribution(gate, line, asked, touched_by, reused_from=None, rejudged_from=None):
    """GP.12: (the `[gate-asks]` line, the suffix a failed gate's entry carries). HP.1: a reused gate's line
    reads `result=<r> reused_from=<commit12>`: an ask never forces a run, the gate's inputs decide. HP.2: a
    re-judged gate's reads `result=<r> rejudged_from=<commit12>`, the commit whose run it judged again."""
    moved = sorted(i for i, ps in touched_by.items() if any(p.startswith(GATE_PATHS[gate]) for p in ps))
    ids = lambda xs: ",".join(xs) or "-"
    reused = (f" reused_from={reused_from[:12]}" if reused_from else "") + \
        (f" rejudged_from={rejudged_from[:12]}" if rejudged_from else "")
    return (f"== [gate-asks] {gate} result={gate_result(line)}{reused} asked_by={len(asked)} ({ids(asked)}) "
            f"touched_by={len(moved)} ({ids(moved)})",
            f" (asked by {ids(asked)}; touched by {ids(moved)})")


# FH.18 (James 2026-10-26, "should we have an options for test and build to make it quicker"):
# every cargo the close-out runs, and the gate scripts it calls, builds with 8 jobs (half the
# cores), so the links of a wave's builds leave the desktop usable. An explicit
# CARGO_BUILD_JOBS in the caller's environment wins.
CARGO_BUILD_JOBS = "8"
# HP.1 (8): the TMPDIR every subprocess gets (closeout_tmp(); main sets it). None leaves the caller's.
GATE_TMPDIR = None


def sh(cmd, cwd=ROOT, timeout=1800):
    env = dict(os.environ)
    env.setdefault("CARGO_BUILD_JOBS", CARGO_BUILD_JOBS)
    if GATE_TMPDIR:
        env["TMPDIR"] = GATE_TMPDIR
    r = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True, timeout=timeout,
                       env=env)
    return r.stdout + r.stderr, r.returncode


def fmt_check():
    """F0.5's fmt gate: the workspace and the excluded engram-export, each by its own manifest.
    Returns (ok, marker); the marker is the stable `[fmt] check:` line the close-out prints."""
    paths = set()
    for cmd in ("cargo fmt --all --check 2>&1",
                "cargo fmt --manifest-path engram-export/Cargo.toml --check 2>&1"):
        out, code = sh(cmd)
        paths |= {l.split()[2].rsplit(':', 2)[0] for l in out.splitlines() if l.startswith('Diff in ')}
        if code != 0 and not paths:
            # a non-zero exit with no hunk (rustfmt itself failed) is drift we cannot count
            paths.add(f"<{cmd.split(' 2>')[0]} exit {code}>")
    if paths:
        return False, f"[fmt] check: FAILED ({len(paths)} files)"
    return True, "[fmt] check: ok"


def touched_crates(touched):
    """The workspace packages whose directories hold a touched path (longest manifest dir wins), or
    None when a touched path sits in no package dir but could still move docs (the workspace root
    Cargo.toml / Cargo.lock)."""
    out, code = sh("cargo metadata --no-deps --format-version 1 2>/dev/null")
    if code != 0:
        return None
    dirs = sorted(((str(Path(p["manifest_path"]).parent.relative_to(ROOT)), p["name"])
                   for p in json.loads(out)["packages"]), key=lambda d: -len(d[0]))
    crates = set()
    for t in touched:
        if t in ("Cargo.toml", "Cargo.lock"):
            return None
        hit = next((name for d, name in dirs if d != "." and t.startswith(d + "/")), None)
        if hit:
            crates.add(hit)
    return sorted(crates)


def rustdoc_check(crates=None):
    """FD.23b's rustdoc gate: every workspace crate documents with warnings denied, in the
    workspace's own target dir. `--keep-going` documents every crate a failure does not block,
    so one bad link does not hide the rest. Returns (ok, marker); the marker is the stable
    `[rustdoc] check:` line the close-out prints. `<n>` counts the located `error` lines (the
    per-crate `could not document` / `could not compile` summaries are not errors of their
    own), `<c>` the crates that failed."""
    # James 2026-10-30 (close-out speed): a wave documents only the crates it touched (155 s for the
    # whole workspace after the tests, 42 s warm); every crate on the FULL_GATES_EVERY waves and at
    # the release. `crates=[]` (no Rust touched) documents nothing.
    if crates is not None and not crates:
        return True, "[rustdoc] check: skipped - wave touched no workspace crate"
    sel = "--workspace" if crates is None else " ".join(f"-p {c}" for c in crates)
    out, code = sh(f"RUSTDOCFLAGS='-D warnings' cargo doc {sel} --no-deps --keep-going 2>&1",
                   timeout=2400)
    scope = "" if crates is None else f" (partial: only {','.join(crates)})"
    if code == 0:
        return True, f"[rustdoc] check: ok{scope}"
    errors = [l for l in out.splitlines()
              if re.match(r"^error(\[E\d+\])?:", l)
              and not re.match(r"^error: (could not (document|compile)|aborting|build failed)", l)]
    failed = set(re.findall(r"^error: could not (?:document|compile) `([^`]+)`", out, re.M))
    return False, f"[rustdoc] check: FAILED ({len(errors)} errors in {len(failed)} crates){scope}"


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


def cohort_gate(touched, remaining, wave, release, full_due=False, cmd="bash scripts/cohort-gate.sh check 2>&1",
                skip_untouched=True):
    """FH.12: the cohort gate (scripts/cohort-gate.sh, bench/cohort/results/baseline/), run after the rank
    gate: the cohort's health and task checks, `glia health` per cohort repo and the Sibyl cells, against
    the committed baseline. A wave touching none of COHORT_PATHS skips it; the release's last close-out
    (`remaining` false) always checks. GLIA_COHORT_GATE_SKIP=<reason> (the close-out cannot afford the
    run) makes the script print `check: skipped - <reason>` into the record: never a silent pass, and a
    failed gate at the release's last close-out. A wave that is not the release's last checks only
    COHORT_FAST (minutes, not hours); GLIA_COHORT_GATE_FULL=1 forces the whole cohort.
    HP.1: the touched-path skip applies only while the gate record is empty (`skip_untouched`); after
    that the gate's input digest decides whether it runs.
    Returns (ok, the `== ..` line, the failed gate or None, the gate's output)."""
    if not (ROOT / "scripts/cohort-gate.sh").exists():
        return False, "== [cohort-gate] check: skipped - no scripts/cohort-gate.sh", None, ""
    if skip_untouched and remaining and not any(p.startswith(COHORT_PATHS) for p in touched):
        return (False, "== [cohort-gate] check: skipped - wave touched no engine/, graph/, parsers/ or code-domain/ path",
                None, "")
    if remaining and not full_due and os.environ.get("GLIA_COHORT_GATE_FULL") != "1":
        cmd = f"bash scripts/cohort-gate.sh check --only {COHORT_FAST} 2>&1"
    try:
        out, _ = sh(cmd, timeout=COHORT_GATE_TIMEOUT)
    except subprocess.TimeoutExpired:
        return (False, f"== [cohort-gate] check: FAILED (timeout after {COHORT_GATE_TIMEOUT}s)", "cohort gate (timeout)",
                "")
    WF.mkdir(parents=True, exist_ok=True)
    (WF / f"cohort-gate-{f'r{release}-' if release else ''}w{wave}.log").write_text(out)
    last = (out.strip().splitlines() or [""])[-1]
    if "--only" in cmd and "(partial: only" not in last:
        # a fast-tier FAILED line (corpus drift exits before the suffix) is still partial: never a full run
        last += f" (partial: only {COHORT_FAST})"
    ok = last.startswith("[cohort-gate] check: ok")
    skipped = last.startswith("[cohort-gate] check: skipped")
    if ok or (skipped and remaining):
        return ok, f"== {last}", None, out
    return False, f"== {last}", "cohort gate" + (" (skipped at the release's last close-out)" if skipped else ""), out


# ---------------------------------------------------------------------------------------------------------------
# HP.1: gate reuse. Each recorded gate's result is keyed by a digest of exactly what it reads at HEAD, kept
# outside the tree, so a re-run whose inputs did not move reuses it and a moved input (whoever moved it: a packet
# commit or an out-of-packet fix between waves) runs it. Input sets are derived (cargo metadata, an include!
# scan, a python import scan), never hand lists; closeout.py's own sha256 is an env line of every gate.
# ---------------------------------------------------------------------------------------------------------------
# HP.3: `timing` is scripts/build-timing.sh check (bench/build-timing/RULE.md): recorded like the others, so a close-out
# whose glia, script, baseline, pins, rustc and machine did not move reuses its last green check.
RECORDED_GATES = ("tests", "fmt", "clippy", "rustdoc", "rank", "cohort", "wheel", "bench", "timing")
RECORD_SCHEMA = 1
GREEN_KEEP = 8   # green entries kept per gate
# bench/substrate-gap's outputs: the bench step writes them, so no gate reads them as an input.
SG_OUTPUTS = ("bench/substrate-gap/results-latest.json", "bench/substrate-gap/legacy-latest.json",
              "bench/substrate-gap/COVERAGE.md", "bench/substrate-gap/results.jsonl")
# The only bench/ dirs the workspace tests read (fixtures, doc-link and edge-audit data); every other bench/ dir is
# a gate's own data (rank-gate, cohort, sibyl, message-contracts) or a retired harness.
TESTS_BENCH_DIRS = ("bench/substrate-gap/", "bench/doc-link/", "bench/edge-audit/")
GATE_JSON = "bench/rank-gate/gate.json"
# A gate whose log carries one of these failed on its environment (the 0.5.6 release's full cohort hit /tmp's
# 25.6 GB user quota): never a green entry, never re-judged.
ENV_FAIL_RE = re.compile(r"Disk quota exceeded|os error 122\b|No space left on device|os error 28\b")
ENV_PROBES = {"rustc": "rustc -Vv", "cargo": "cargo -V", "rustfmt": "rustfmt --version",
              "clippy": "cargo clippy -V", "maturin": "maturin -V", "python": f"{LEAP_PY} -V",
              # HP.3: the machine a timing baseline belongs to (`<cores> cores, <cpuinfo model name>`)
              "machine": "bash scripts/build-timing.sh fingerprint"}
# Each gate's env lines (beside `closeout`, every gate's): the tools and local config its result depends on.
GATE_ENV = {
    "tests": ("rustc", "cargo", "cargo-config"),
    "fmt": ("cargo", "rustfmt"),
    "clippy": ("rustc", "cargo", "clippy", "cargo-config"),
    "rustdoc": ("rustc", "cargo", "cargo-config"),
    "rank": ("rustc", "cargo", "fastembed"),
    "cohort": ("rustc", "cargo"),
    "wheel": ("rustc", "cargo", "cargo-config", "maturin", "python"),
    "bench": ("python",),
    "timing": ("rustc", "machine"),
}
INSTALLED_PROBE = (f"{LEAP_PY} -c \"import glia_py,glob,os;"
                   f"print(glob.glob(os.path.join(os.path.dirname(glia_py.__file__),'*.so'))[0]);"
                   f"print(glia_py.build_stamp())\"")
# include!, include_str!, include_bytes!: a literal path, `concat!("<dir>/", $x)` (a prefix) or
# `concat!(env!("CARGO_MANIFEST_DIR"), "/..")` (relative to the package). OUT_DIR forms read no tracked file.
INCLUDE_RE = re.compile(r'\binclude(?:_str|_bytes)?!\s*\(\s*(concat!\s*\(\s*)?'
                        r'(env!\s*\(\s*"CARGO_MANIFEST_DIR"\s*\)\s*,\s*)?"((?:[^"\\]|\\.)*)"')
PY_IMPORT_RE = re.compile(r"^\s*(?:from\s+([A-Za-z_]\w*)[\w.]*\s+import\b|import\s+([A-Za-z_]\w*))", re.M)
PY_PATH_RE = re.compile(r'sys\.path\.(?:insert|append)\([^)]*?\bHERE((?:\.parent)*)\s*(?:/\s*"([^"]+)")?')


def sha256_bytes(b):
    return hashlib.sha256(b).hexdigest()


def sha256_file(path):
    try:
        return sha256_bytes(Path(path).read_bytes())
    except OSError:
        return "absent"


def lines_digest(lines):
    return sha256_bytes(("\n".join(lines) + "\n").encode())


def canonical_json(v):
    return json.dumps(v, sort_keys=True, separators=(",", ":"))


def under_tmp(path):
    rp = os.path.realpath(path)
    return rp == "/tmp" or rp.startswith("/tmp/")


def closeout_tmp(environ=None, home=None):
    """HP.1 (8): the TMPDIR every gate runs with. The caller's TMPDIR when it is set and not under /tmp (a RAM
    disk with a per-user quota: the 0.5.6 release's full cohort hit `Disk quota exceeded`); else
    $GLIA_CLOSEOUT_TMP, else ~/.cache/glia-tmp."""
    environ = os.environ if environ is None else environ
    t = environ.get("TMPDIR")
    if t and not under_tmp(t):
        return t
    return environ.get("GLIA_CLOSEOUT_TMP") or str(Path(home or Path.home()) / ".cache/glia-tmp")


def record_path(environ=None):
    environ = os.environ if environ is None else environ
    return Path(environ.get("GLIA_CLOSEOUT_RECORD") or Path.home() / ".cache/glia-closeout/gate-record.json").expanduser()


def load_record(path):
    """The gate record; a missing, empty, unreadable or other-schema file is an empty record."""
    try:
        text = Path(path).read_text()
    except OSError:
        text = ""
    empty = {"schema": RECORD_SCHEMA, "gates": {}}
    if not text.strip():
        return empty
    try:
        rec = json.loads(text)
    except ValueError:
        return empty
    if not isinstance(rec, dict) or rec.get("schema") != RECORD_SCHEMA:
        return empty
    rec.setdefault("gates", {})
    return rec


def save_record(path, rec):
    """Atomic: written to a sibling temp file, then renamed over the record."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(f"{path.name}.tmp{os.getpid()}")
    tmp.write_text(json.dumps(rec, indent=1, sort_keys=True))
    os.replace(tmp, path)


def usable(entry):
    """An entry a re-run may reuse: green and not an environment failure."""
    return bool(entry) and bool(entry.get("ok")) and not entry.get("env_fail")


# HP.2: the gates that judge a measurement against committed baselines, and where each keeps its last run
# (<dir>/run/run-meta.json vouches for the run's files; the scripts' `recheck` judges only what it vouches for).
GATE_RUN_DIRS = {"rank": ("GLIA_RANK_GATE_DIR", "~/.cache/glia-rank-gate"),
                 "cohort": ("GLIA_COHORT_GATE_DIR", "~/.cache/glia-cohort-gate")}
REJUDGE_TIMEOUT = 1800   # seconds: a re-judge reads files (the rank one may rebuild rank_eval first)


def gate_run_dir(gate, environ=None):
    environ = os.environ if environ is None else environ
    var, default = GATE_RUN_DIRS[gate]
    return Path(environ.get(var) or default).expanduser() / "run"


def run_meta_sha(gate, head, environ=None):
    """HP.2: the sha256 of the gate's run/run-meta.json when the run it describes was measured at `head`, else
    None (no run-meta: the run failed before its end, or the script predates HP.2)."""
    path = gate_run_dir(gate, environ) / "run-meta.json"
    try:
        raw = path.read_bytes()
        meta = json.loads(raw)
    except (OSError, ValueError):
        return None
    return sha256_bytes(raw) if isinstance(meta, dict) and meta.get("glia_head") == head else None


def measured_commit(entry):
    """The commit whose run an entry judged: its own for a fresh run, the measured one for a re-judge."""
    return (entry or {}).get("measured_at") or (entry or {}).get("commit") or ""


def run_intact(gate, entry, environ=None):
    """HP.2: None when the run an entry recorded is still on disk as it was (run-meta.json at its recorded sha256,
    every file it lists at its own); else why not."""
    want = (entry or {}).get("run_meta_sha")
    if not want:
        return "run files not recorded"
    run = gate_run_dir(gate, environ)
    try:
        raw = (run / "run-meta.json").read_bytes()
    except OSError:
        return "no run-meta.json"
    if sha256_bytes(raw) != want:
        return "another run since"
    try:
        files = json.loads(raw).get("files") or {}
    except (ValueError, AttributeError):
        return "run-meta.json unreadable"
    for rel, sha in sorted(files.items()):
        if sha256_file(run / rel) != sha:
            return f"run/{rel} changed"
    return None


def tier_only(tier):
    """The labels a partial tier names (`partial:a,b` -> `a,b`), or "" for full."""
    return tier.split(":", 1)[1] if tier and tier.startswith("partial:") else ""


def record_result(rec, gate, entry):
    """Every result becomes the gate's `last`; a usable one also joins `green` (the newest GREEN_KEEP)."""
    g = rec.setdefault("gates", {}).setdefault(gate, {"green": [], "last": None})
    g["last"] = entry
    if usable(entry):
        g["green"] = (g.get("green") or [])[-(GREEN_KEEP - 1):] + [entry]


def partial_tier(labels):
    return "partial:" + ",".join(sorted(x for x in labels.split(",") if x))


def covers(tier, need):
    """A full entry serves any need; a partial entry serves only a partial need over the same labels or a
    subset; a full need takes only a full entry."""
    if tier == "full":
        return True
    if need == "full" or not tier.startswith("partial:") or not need.startswith("partial:"):
        return False
    return set(need.split(":", 1)[1].split(",")) <= set(tier.split(":", 1)[1].split(","))


def fresh_gates(*specs):
    """GLIA_CLOSEOUT_FRESH / --fresh: `all` or a comma list of RECORDED_GATES names."""
    out = set()
    for s in specs:
        for g in (s or "").split(","):
            g = g.strip()
            if g == "all":
                out |= set(RECORDED_GATES)
            elif g:
                out.add(g)
    unknown = sorted(out - set(RECORDED_GATES))
    if unknown:
        raise ValueError(f"unknown gate(s) {unknown} (known: all, {', '.join(RECORDED_GATES)})")
    return out


def decide(gate, need, d, rec, fresh=(), dirty=False, extra=None, intact=None):
    """One gate's decision: ('reuse', entry, None), ('rejudge', the last entry, reason) or ('run', reference entry
    or None, reason). `d` is HEAD's digest (None when it could not be read); `extra(entry)` names what else moved
    (the installed wheel, the bench outputs) or returns None. HP.2: `intact(entry)` (rank and cohort only) says why
    the run behind an entry is no longer on disk, or None; with it, a gate whose last result (ok or failed, never
    an environment failure) measured HEAD's measure inputs at HEAD's env, at a tier covering the need, and whose
    judge inputs alone moved, is re-judged."""
    if gate in fresh:
        return "run", None, "forced"
    if d is None:
        return "run", None, "no digest"
    g = (rec.get("gates") or {}).get(gate) or {}
    greens = [e for e in g.get("green") or [] if usable(e)]
    last = g.get("last")
    if not greens and not last:
        return "run", None, "no record"
    if dirty:
        return "run", None, "dirty tree"
    if last and last.get("digest") == d["digest"] and not usable(last):
        return "run", last, "last result FAILED"
    same = [e for e in reversed(greens) if e.get("digest") == d["digest"]]
    extra_reason = None
    for e in same:
        if not covers(e.get("tier", "full"), need):
            continue
        why = extra(e) if extra else None
        if why is None:
            return "reuse", e, None
        extra_reason = extra_reason or why
    if extra_reason:
        return "run", same[0], f"env moved: {extra_reason}"
    no_rejudge = None
    if (intact and last and not last.get("env_fail") and last.get("measure") == d["measure"]
            and last.get("judge") != d["judge"] and last.get("env_digest") == d["env_digest"]
            and covers(last.get("tier", "full"), need)):
        no_rejudge = intact(last)
        if no_rejudge is None:
            return "rejudge", last, "judge inputs moved"
    if same:
        tier = same[0].get("tier", "full")
        return "run", same[0], ("need full, record partial" if need == "full" else f"need {need}, record {tier}")
    if no_rejudge:
        return "run", last, f"judge inputs moved; no rejudge: {no_rejudge}"
    ref = greens[-1] if greens else last
    if ref.get("measure") != d["measure"] or ref.get("judge") != d["judge"]:
        return "run", ref, "inputs moved"
    old = ref.get("env") or {}
    moved = sorted(k for k in set(old) | set(d["env"]) if old.get(k) != d["env"].get(k))
    return "run", ref, f"env moved: {','.join(moved) or 'unknown'}"


def ratchet_due(status, all_green):
    """HP.1 (6): a ratchet runs only after this close-out ran the gate FRESH and FULL, and everything is green."""
    return bool(all_green and status and status.get("how") == "fresh" and status.get("tier") == "full"
                and status.get("ok"))


def release_set(statuses, tree):
    """HP.1 (5): the release's last close-out needs every recorded gate fresh-full or reused-full at HEAD's
    digest (HP.2: or re-judged full). Returns (the `[closeout] set` line, one line per gate, the gates missing
    from the set)."""
    fresh, reused, rejudged, missing, per = [], [], [], [], []
    for g in RECORDED_GATES:
        s = statuses.get(g) or {}
        if s.get("ok") and s.get("tier") == "full" and s.get("how") in ("fresh", "reused", "rejudged"):
            {"fresh": fresh, "reused": reused, "rejudged": rejudged}[s["how"]].append(g)
            per.append(f"   {g}=" + ("fresh" if s["how"] == "fresh" else f"{s['how']} {s['commit'][:12]}"))
        else:
            missing.append(g)
            why = ("not run" if not s else "failed" if not s.get("ok") else f"{s.get('how')} {s.get('tier')}")
            per.append(f"   {g}=missing ({why})")
    line = (f"[closeout] set tree={tree[:12]} fresh={len(fresh)} reused={len(reused)} rejudged={len(rejudged)} "
            f"full={len(fresh) + len(reused) + len(rejudged)}/{len(RECORDED_GATES)}")
    return line, per, missing


def env_fail_signature(*texts):
    for t in texts:
        m = ENV_FAIL_RE.search(t or "")
        if m:
            return m.group(0)
    return None


def recent_texts(root, since, suffixes=(".jsonl", ".log")):
    """The text of every file under `root` with one of `suffixes`, modified at or after `since`."""
    out = []
    for dirpath, _, files in os.walk(root):
        for f in files:
            p = os.path.join(dirpath, f)
            try:
                if f.endswith(suffixes) and os.path.getmtime(p) >= since:
                    out.append(Path(p).read_text(errors="replace"))
            except OSError:
                continue
    return out


def outputs_now(root=ROOT):
    """{path: sha256} of the bench outputs the close-out commits (SG_OUTPUTS but the untracked results.jsonl), as
    they are on disk: a reused bench needs HEAD's to be the ones its recorded run wrote."""
    return {o: sha256_file(Path(root) / o) for o in SG_OUTPUTS if not o.endswith(".jsonl")}


def line_tier(line, need):
    """A gate's tier from its last line: `(partial: only a,b)` is partial over those labels, else the need."""
    m = re.search(r"\(partial: only ([^)]+)\)", line or "")
    return partial_tier(m.group(1).replace(" ", "")) if m else need


def tracked_blobs(root=ROOT, rev="HEAD"):
    """{path: blob} of every tracked path at `rev`: one `git ls-tree -r -z --full-tree`."""
    r = subprocess.run(["git", "ls-tree", "-r", "-z", "--full-tree", rev], cwd=root, capture_output=True)
    if r.returncode:
        raise RuntimeError(f"git ls-tree {rev}: {r.stderr.decode(errors='replace').strip()}")
    out = {}
    for rec in r.stdout.split(b"\0"):
        if rec:
            meta, path = rec.split(b"\t", 1)
            out[path.decode("utf-8", "surrogateescape")] = meta.split()[2].decode()
    return out


def in_tests_set(path):
    """The workspace tests' input set: the whole tracked tree except dev-notes/ (closeout.py is an env line) and
    every bench/ dir but TESTS_BENCH_DIRS, minus bench/substrate-gap's outputs. Broad on purpose: README.md,
    skills/, docs/ and every tracked file self_scan copies are read by some test."""
    if path.startswith("dev-notes/"):
        return False
    if path.startswith("bench/"):
        return path.startswith(TESTS_BENCH_DIRS) and path not in SG_OUTPUTS
    return True


def _under(path, d):
    return d == "" or path.startswith(d + "/")


def default_probe(cmd):
    try:
        out, rc = sh(f"{cmd} 2>&1", timeout=120)
    except subprocess.TimeoutExpired:
        return "error: timeout"
    return out.strip() if rc == 0 else f"error rc={rc}: {(out.strip().splitlines() or [''])[0]}"


class Inputs:
    """What every recorded gate reads at HEAD, read once per close-out: the tracked blobs, the cargo resolve
    graph and the env probes. `meta` is `cargo metadata --format-version 1 --locked --offline --all-features`."""

    def __init__(self, root=ROOT, meta=None, tracked=None, probe=None, closeout_sha=None, fastembed_dir=None,
                 installed=None):
        self.root = Path(root)
        self.tracked = tracked_blobs(self.root) if tracked is None else tracked
        if meta is None:
            out, rc = sh("cargo metadata --format-version 1 --locked --offline --all-features 2>/dev/null",
                         cwd=self.root)
            if rc:
                raise RuntimeError("cargo metadata --locked --offline failed")
            meta = json.loads(out)
        self.meta = meta
        self.probe = probe or default_probe
        self.closeout_sha = closeout_sha or sha256_file(HERE / "closeout.py")
        self.fastembed_dir = Path(fastembed_dir or os.environ.get("FASTEMBED_CACHE_DIR")
                                  or Path.home() / ".cache/glia/fastembed")
        self._installed = installed
        self._env, self._refs, self._digests = {}, {}, {}
        pk = {p["id"]: p for p in meta["packages"]}
        self._pk = pk
        self.dirs = {}   # workspace member name -> its directory, relative to root ("" = the root)
        for i in meta["workspace_members"]:
            d = Path(pk[i]["manifest_path"]).parent
            try:
                rel = d.resolve().relative_to(self.root.resolve()).as_posix()
            except ValueError:
                continue
            self.dirs[pk[i]["name"]] = "" if rel == "." else rel

    # --- the code sets -------------------------------------------------------------------------------------
    def closure(self, name):
        """The workspace members `name` builds against: normal and build dependencies, transitively, plus the
        root's own dev-dependencies (its tests and examples link them). `*` is every member."""
        if name == "*":
            return set(self.dirs)
        ws = set(self.meta["workspace_members"])
        roots = [i for i in ws if self._pk[i]["name"] == name]
        if not roots:
            raise KeyError(f"no workspace package {name}")
        nodes = {n["id"]: n for n in self.meta["resolve"]["nodes"]}
        root = roots[0]
        seen, stack = {root}, [root]
        while stack:
            x = stack.pop()
            for dep in nodes.get(x, {}).get("deps", []):
                kinds = {k.get("kind") for k in (dep.get("dep_kinds") or [{"kind": None}])}
                if (kinds & {None, "build"} or (x == root and "dev" in kinds)) and dep["pkg"] not in seen:
                    seen.add(dep["pkg"])
                    stack.append(dep["pkg"])
        return {self._pk[i]["name"] for i in seen if i in ws and self._pk[i]["name"] in self.dirs}

    def text(self, path):
        try:
            return (self.root / path).read_text(errors="replace")
        except OSError:
            return ""

    def pkg_dir(self, path):
        best = None
        for d in self.dirs.values():
            if _under(path, d) and (best is None or len(d) > len(best)):
                best = d
        return best if best is not None else posixpath.dirname(path)

    def include_refs(self, path):
        """(repo path, is_prefix) for every include!/include_str!/include_bytes! literal in a .rs file."""
        if path not in self._refs:
            refs = []
            for m in INCLUDE_RE.finditer(self.text(path)):
                lit = m.group(3)
                if m.group(2):
                    base, lit = self.pkg_dir(path), lit.lstrip("/")
                else:
                    base = posixpath.dirname(path)
                joined = posixpath.normpath(posixpath.join(base, lit) if base else lit)
                if joined == ".." or joined.startswith("../") or joined.startswith("/"):
                    continue
                prefix = bool(m.group(1))
                if prefix and lit.endswith("/"):
                    joined += "/"
                refs.append((joined, prefix))
            self._refs[path] = refs
        return self._refs[path]

    def include_targets(self, dirs):
        out = set()
        for p in self.tracked:
            if p.endswith(".rs") and any(_under(p, d) for d in dirs):
                for joined, prefix in self.include_refs(p):
                    if prefix:
                        out |= {t for t in self.tracked if t.startswith(joined)}
                    elif joined in self.tracked:
                        out.add(joined)
        return out

    def code(self, names):
        """CODE(roots): the dirs of the members in the roots' closure + Cargo.toml + Cargo.lock + every tracked
        path an include!/include_str!/include_bytes! in those dirs names."""
        dirs = sorted({self.dirs[n] for name in names for n in self.closure(name)})
        paths = {p for p in self.tracked if any(_under(p, d) for d in dirs)}
        paths |= {p for p in ("Cargo.toml", "Cargo.lock") if p in self.tracked}
        return paths | self.include_targets(dirs)

    def py_imports(self, seeds):
        """`seeds` plus the tracked python modules they import, transitively, through their own dir and the
        `sys.path.insert(.., HERE[.parent..] / "<dir>")` dirs they add (bench/cohort/cells.py reads
        bench/substrate-gap/matrix_vocab.py)."""
        out = set(seeds)
        queue = sorted(p for p in seeds if p.endswith(".py"))
        while queue:
            p = queue.pop()
            text, here = self.text(p), posixpath.dirname(p)
            dirs = {here}
            for m in PY_PATH_RE.finditer(text):
                d = here
                for _ in range(m.group(1).count(".parent")):
                    d = posixpath.dirname(d)
                if m.group(2):
                    d = posixpath.normpath(posixpath.join(d, m.group(2)) if d else m.group(2))
                dirs.add(d)
            for m in PY_IMPORT_RE.finditer(text):
                mod = m.group(1) or m.group(2)
                for d in dirs:
                    for cand in (posixpath.join(d, mod + ".py"), posixpath.join(d, mod, "__init__.py")):
                        if cand in self.tracked and cand not in out:
                            out.add(cand)
                            queue.append(cand)
        return out

    def json_at(self, path):
        blob = self.tracked.get(path)
        if not blob:
            return None
        r = subprocess.run(["git", "cat-file", "blob", blob], cwd=self.root, capture_output=True)
        try:
            return json.loads(r.stdout) if r.returncode == 0 else None
        except ValueError:
            return None

    def installed(self, refresh=False):
        """(installed glia_py .so path, its sha256, build_stamp()) in the leap venv, or ('', 'none', 'none')."""
        if self._installed is None or refresh:
            out = self.probe(INSTALLED_PROBE)
            ls = out.splitlines()
            if out.startswith("error") or len(ls) < 2:
                self._installed = ("", "none", "none")
            else:
                self._installed = (ls[0].strip(), sha256_file(ls[0].strip()), ls[1].strip())
            self._digests.pop("bench", None)
        return self._installed

    def gate_sets(self, gate):
        """({path: blob or value} the gate measures, {path: ..} it judges against). Only rank and cohort judge."""
        T = self.tracked
        pick = lambda paths: {p: T[p] for p in paths if p in T}
        judge = {}
        if gate == "tests":
            measure = pick({p for p in T if in_tests_set(p)} | self.code(["*"]))
        elif gate == "fmt":
            measure = pick({p for p in T if p.endswith(".rs")}
                           | {"rustfmt.toml", ".rustfmt.toml", "Cargo.toml", "engram-export/Cargo.toml"})
        elif gate in ("clippy", "rustdoc"):
            measure = pick(self.code(["*"]) | ({"clippy.toml", ".clippy.toml"} if gate == "clippy" else set()))
        elif gate == "rank":
            measure = pick(self.code(["glia-engine"]) | {"scripts/rank-gate.sh", "bench/rank-gate/snapshots.json"})
            judge = pick({p for p in T if p.startswith("bench/rank-gate/baseline/")})
            gj = self.json_at(GATE_JSON)
            if isinstance(gj, dict):
                accepted = gj.pop("accepted", None)
                measure[GATE_JSON + "#measure"] = sha256_bytes(canonical_json(gj).encode())
                judge[GATE_JSON + "#accepted"] = sha256_bytes(canonical_json(accepted).encode())
        elif gate == "cohort":
            own = {p for p in T if (p.startswith("bench/cohort/") and not p.startswith("bench/cohort/results/"))
                   or p.startswith("bench/sibyl/")}
            measure = pick(self.code(["glia-cli"]) | {"scripts/cohort-gate.sh"} | self.py_imports(own))
            judge = pick({p for p in T if p.startswith("bench/cohort/results/baseline/")})
        elif gate == "wheel":
            measure = pick(self.code(["glia-py", "glia-cli"]) | {p for p in T if p.startswith("bench/message-contracts/")})
        elif gate == "bench":
            measure = pick({p for p in T if p.startswith("bench/substrate-gap/") and p not in SG_OUTPUTS})
            measure["<installed glia_py>"] = self.installed()[1]
        elif gate == "timing":
            # HP.3: the glia it builds, the script, its own bench dir (rule, repos, baseline) and, of
            # bench/cohort/repos.json, only the rows it measures (label, url, sha): another cohort row moves nothing.
            measure = pick(self.code(["glia-cli"]) | {"scripts/build-timing.sh"}
                           | {p for p in T if p.startswith("bench/build-timing/")})
            measure[COHORT_REPOS + "#pins"] = sha256_bytes(canonical_json(self.timing_pins()).encode())
        else:
            raise KeyError(gate)
        if gate in GATE_PATHS:
            # Every tracked path a packet commit can touch for this gate (GP.12's GATE_PATHS) is in its digest,
            # so a gate a packet touched can never be reused from before that packet.
            for p in T:
                if p.startswith(GATE_PATHS[gate]) and p not in measure and p not in judge and p != GATE_JSON:
                    measure[p] = T[p]
        return measure, judge

    def timing_pins(self):
        """HP.3: [label, url, sha] of every repo bench/build-timing/repos.json names, read from bench/cohort/repos.json
        at HEAD (None for a label that file lacks)."""
        mine, cohort = self.json_at(BUILD_TIMING_REPOS), self.json_at(COHORT_REPOS)
        labels = [r.get("label") for r in (mine.get("repos") or [])] if isinstance(mine, dict) else []
        rows = {r.get("label"): r for r in (cohort.get("repos") or [])} if isinstance(cohort, dict) else {}
        return [[lab, (rows.get(lab) or {}).get("url"), (rows.get(lab) or {}).get("sha")] for lab in labels]

    def env_value(self, name):
        if name not in self._env:
            if name == "closeout":
                v = self.closeout_sha
            elif name == "cargo-config":
                v = sha256_file(self.root / ".cargo/config.toml")
            elif name == "fastembed":
                listing = []
                for dirpath, _, files in os.walk(self.fastembed_dir):
                    for f in files:
                        p = Path(dirpath) / f
                        try:
                            listing.append(f"{p.relative_to(self.fastembed_dir).as_posix()} {p.stat().st_size}")
                        except OSError:
                            continue
                v = lines_digest(sorted(listing)) if listing else "absent"
            else:
                v = sha256_bytes(self.probe(ENV_PROBES[name]).encode())
            self._env[name] = v[:16]
        return self._env[name]

    def env(self, gate):
        return {k: self.env_value(k) for k in ("closeout",) + GATE_ENV[gate]}

    def digest(self, gate):
        if gate not in self._digests:
            m, j = self.gate_sets(gate)
            md = lines_digest(sorted(f"{p} {v}" for p, v in m.items()))
            jd = lines_digest(sorted(f"{p} {v}" for p, v in j.items()))
            env = self.env(gate)
            ed = lines_digest(sorted(f"{k} {v}" for k, v in env.items()))
            self._digests[gate] = {
                "measure": md, "judge": jd, "env": env, "env_digest": ed,
                "digest": lines_digest([f"measure {md}", f"judge {jd}", f"env {ed}"]),
                "files": len(m) + len(j), "paths": {p.split("#")[0] for p in (*m, *j) if not p.startswith("<")},
                "judge_paths": {p.split("#")[0] for p in j}}
        return self._digests[gate]


def inputs_moved_reason(ref, d, tracked):
    """`inputs moved: <n> paths (<first 3>)`: what `git diff --name-only <recorded commit> HEAD` names inside the
    gate's set (a deleted path counts when a sibling in its directory is in the set)."""
    out, rc = sh(f"git diff --name-only {ref.get('commit', '')} HEAD")
    if rc:
        return "inputs moved: recorded commit unreadable"
    dirs = {posixpath.dirname(p) for p in d["paths"]}
    ps = [p for p in out.splitlines() if p in d["paths"] or (p not in tracked and posixpath.dirname(p) in dirs)]
    return f"inputs moved: {len(ps)} paths ({', '.join(ps[:3])})" if ps else "inputs moved: 0 paths (the set changed)"


def judge_moved_paths(ref, d, tracked, root=ROOT):
    """HP.2: the paths of the gate's judge set that `git diff --name-only <ref's commit> HEAD` names (a deleted
    path counts when a sibling in its directory is judged), or None when the commit is unreadable."""
    out, rc = sh(f"git diff --name-only {ref.get('commit', '')} HEAD", cwd=root)
    if rc:
        return None
    jp = d.get("judge_paths") or set()
    dirs = {posixpath.dirname(p) for p in jp}
    return [p for p in out.splitlines() if p in jp or (p not in tracked and posixpath.dirname(p) in dirs)]


def rejudge_line(gate, ref, moved):
    """HP.2's marker: `[closeout] rejudge <gate> from <commit12> (judge inputs moved: <first 3 paths>)`."""
    what = "unreadable" if moved is None else ", ".join(moved[:3]) or "the set changed"
    return f"[closeout] rejudge {gate} from {measured_commit(ref)[:12]} (judge inputs moved: {what})"


def gate_needs(remaining, full_due, touched, environ=None, rustdoc_crates=None):
    """Each recorded gate's need this close-out: `full`, `partial:<labels>`, or None (nothing to check).
    `rustdoc_crates` is touched_crates()'s answer when only touched crates are due (None = every crate)."""
    environ = os.environ if environ is None else environ
    rank_full = (not remaining or full_due.get("rank") or environ.get("GLIA_RANK_GATE_FULL") == "1"
                 or any(p.startswith(RANK_CODE_PATHS) for p in touched))
    cohort_full = not remaining or full_due.get("cohort") or environ.get("GLIA_COHORT_GATE_FULL") == "1"
    rustdoc = ("full" if rustdoc_crates is None else None if not rustdoc_crates
               else partial_tier(",".join(rustdoc_crates)))
    return {"tests": "full", "fmt": "full", "clippy": "full", "rustdoc": rustdoc,
            "rank": "full" if rank_full else partial_tier(RANK_FAST),
            "cohort": "full" if cohort_full else partial_tier(COHORT_FAST),
            "wheel": "full", "bench": "full", "timing": "full"}


def timing_verdict(line, remaining):
    """HP.3: what one `scripts/build-timing.sh check` last line means here. Returns (ok, the failed gate or None, the
    line to print). Per wave (`remaining` true) the check is advisory: a FAILED or skipped line is printed with
    ` (advisory)` and fails nothing (James 2026-11-02, the accepted default). At the release's last close-out it
    gates: a FAILED line fails it, and so does any skip (load, machine, GLIA_BUILD_TIMING_SKIP): the box must be
    quiet."""
    last = (line or "").strip() or "[build-timing] check: FAILED (no output)"
    if last.startswith("[build-timing] check: ok"):
        return True, None, f"== {last}"
    if remaining:
        return False, None, f"== {last} (advisory)"
    skipped = last.startswith("[build-timing] check: skipped")
    return False, "build timing" + (" (skipped at the release's last close-out)" if skipped else ""), f"== {last}"


def dirty_paths():
    """`git status --short` lines that block a commit (corrections and the usage log are expected)."""
    out, _ = sh("git status --short")
    return [l for l in out.splitlines() if l.strip() and "packet-corrections.json" not in l
            and not re.search(r"leap-05\d-corrections\.json", l) and "usage-log.jsonl" not in l]


def print_digests():
    """`--digests`: one line per recorded gate at HEAD; runs nothing."""
    inputs = Inputs()
    if dirty_paths():
        print("[closeout] note: the tree is dirty; digests read HEAD's blobs", file=sys.stderr)
    for g in RECORDED_GATES:
        d = inputs.digest(g)
        print(f"[closeout] digest {g} measure={d['measure'][:12]} judge={d['judge'][:12]} files={d['files']} "
              f"env={d['env_digest'][:12]}")


def clippy_check():
    """HP.1 (James 2026-11-02): `cargo clippy --workspace --all-targets -- -D warnings`. A warning is unfinished
    wiring, never `#[allow]`. Returns (ok, the `[clippy] check:` marker, the output)."""
    out, code = sh("cargo clippy --workspace --all-targets --quiet -- -D warnings 2>&1", timeout=2400)
    if code == 0:
        return True, "[clippy] check: ok", out
    diags = [l for l in out.splitlines() if re.match(r"^(warning|error)(\[[^\]]+\])?:", l)
             and not re.match(r"^(error: (could not compile|aborting)|warning: build failed"
                              r"|warning: .* generated \d+ warnings?)", l)]
    return False, f"[clippy] check: FAILED ({len(diags)} diagnostics)", out


def main():
    global GATE_TMPDIR
    args = sys.argv[1:]
    if "--digests" in args:
        print_digests()
        return
    fresh_cli = ""
    if "--fresh" in args:
        k = args.index("--fresh")
        fresh_cli = args[k + 1] if k + 1 < len(args) else ""
        del args[k:k + 2]
    try:
        fresh = fresh_gates(os.environ.get("GLIA_CLOSEOUT_FRESH", ""), fresh_cli)
    except ValueError as e:
        sys.exit(str(e))
    leap = "--leap" in args
    if leap:
        args.remove("--leap")
    release = None
    if "--release" in args:
        k = args.index("--release")
        release = args[k + 1]
        del args[k:k + 2]
        if release not in RELEASES:
            sys.exit(f"unknown release {release!r} (known: {', '.join(RELEASES)})")
    sys.path.insert(0, str(HERE))
    wave = int(args[0])
    if release:
        import importlib
        packets, _, waves = importlib.import_module(f"schedule_{release}").waves()
        ids, remaining = waves[wave], waves[wave + 1:]
    elif leap:
        import leap_schedule
        loaded = leap_schedule.load()
        packets, waves = loaded[0], leap_schedule.schedule(*loaded)
        ids, remaining = waves[wave], waves[wave + 1:]
    else:
        import schedule
        packets = schedule.load()
        waves, _ = schedule.schedule(packets)
        ids, remaining = waves[wave - 1], waves[wave:]
    asks = gate_asks(packets, ids)
    base = json.loads((HERE / "baseline.json").read_text())
    last_full = base.setdefault("gates_last_full", {"rank": 20, "cohort": 20})
    full_due = {g: wave - int(last_full.get(g, wave)) >= FULL_GATES_EVERY for g in ("rank", "cohort")}
    rec_path = record_path()
    record = load_record(rec_path)
    record_any = bool(record.get("gates"))
    if "--plan-only" in args:
        print(f"wave {wave}: {' '.join(ids)}")
        print(f"remaining waves: {len(remaining)}")
        for g, asked in asks.items():
            print(f"[gate-asks] {g} asked_by={len(asked)} ({','.join(asked) or '-'})")
        # HP.1 (10): each recorded gate's decision at HEAD (no packet commits read: a plan has no run id).
        try:
            inputs = Inputs()
        except (RuntimeError, ValueError, KeyError) as e:
            inputs = None
            print(f"[closeout] plan: no digests ({e})")
        dirty = bool(dirty_paths())
        full_doc = not remaining or any(full_due.values()) or os.environ.get("GLIA_RUSTDOC_FULL") == "1"
        needs = gate_needs(remaining, full_due, set(), rustdoc_crates=None if full_doc else [])
        for g in RECORDED_GATES:
            if needs[g] is None:
                print(f"[closeout] plan skip {g} (nothing due)")
                continue
            d = inputs.digest(g) if inputs else None
            extra = None
            if inputs and g == "wheel":
                cur = inputs.installed()
                extra = lambda e, cur=cur: None if (e.get("so_sha256"), e.get("stamp")) == cur[1:] else "installed-wheel"
            elif g == "bench":
                extra = lambda e: None if outputs_now() == (e.get("outputs") or {}) else "outputs"
            intact = (lambda e, g=g: run_intact(g, e)) if g in GATE_RUN_DIRS else None
            action, ref, reason = decide(g, needs[g], d, record, fresh, dirty, extra, intact)
            if action == "reuse":
                print(f"[closeout] plan reuse {g} from {ref['commit'][:12]} (wave {ref.get('wave')}, {ref.get('tier')}, "
                      f"digest {ref['digest'][:12]})")
            elif action == "rejudge":
                print(rejudge_line(g, ref, judge_moved_paths(ref, d, inputs.tracked)).replace(
                    "[closeout] rejudge", "[closeout] plan rejudge", 1))
            else:
                if reason == "inputs moved" and ref:
                    reason = inputs_moved_reason(ref, d, inputs.tracked)
                print(f"[closeout] plan run {g} ({reason})")
        return
    # One run id, or several comma-separated: a packet re-run on its own (e.g. after a
    # permission fix) is merged over the wave's run, the later run winning per packet.
    runs = args[1].split(",")
    gates, lines = [], []
    t0 = time.time()
    # Each line carries the close-out's elapsed wall time and the time since the line before it, on stdout AND in
    # the recorded lines (the WF log and the commit body; James 2026-10-30: see where a close-out's time goes, GP.7).
    said_at = [t0]

    def say(s):
        now = time.time()
        s = f"{s}  [t+{int(now - t0)}s, step {int(now - said_at[0])}s]"
        said_at[0] = now
        print(s)
        lines.append(s)

    # GP.7: seconds per named close-out step (TIMING_STEPS), summed over every block of that step.
    timing = {}

    @contextmanager
    def step(name):
        s0 = time.time()
        try:
            yield
        finally:
            timing[name] = timing.get(name, 0.0) + time.time() - s0

    tag = f"r{release}-" if release else "leap-" if leap else ""
    wf_record = WF / f"closeout-{tag}w{wave}.log"

    def write_record():
        WF.mkdir(parents=True, exist_ok=True)
        wf_record.write_text("\n".join(lines) + "\n")
    engine_pkg, py_pkg = package_name("engine"), package_name("py")
    py = LEAP_PY if (leap or release) else "python3"   # every command that imports the wheel (glia_py)

    # HP.1 (8): every gate runs with TMPDIR on /home (not /tmp's RAM-disk quota).
    GATE_TMPDIR = closeout_tmp()
    Path(GATE_TMPDIR).mkdir(parents=True, exist_ok=True)
    say(f"[closeout] tmpdir={GATE_TMPDIR} free_gb={shutil.disk_usage(GATE_TMPDIR).free / 2**30:.1f}")

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

    dirty = dirty_paths()
    if dirty:
        gates.append(f"uncommitted paths: {dirty[:6]}")
    dirty_at_start = {l[3:].strip() for l in dirty}

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

    touched = set()   # every path the wave's packet commits touch (rustdoc, the rank and cohort gates read it)
    touched_by = {}   # GP.12: the same paths per packet, so a gate result names the packets that moved its paths
    commits_by = {}   # HP.1 (7): the packet commits, so a reuse can be checked against them
    for r in res:
        pid = packet_id(r["packet"])
        mine = touched_by.setdefault(pid, set())
        for c in re.findall(r"\b[0-9a-f]{7,40}\b", str(r.get("commit") or "")):
            commits_by.setdefault(pid, []).append(c)
            o, _ = sh(f"git show --name-only --format= {c}")
            mine.update(l.strip() for l in o.splitlines() if l.strip())
        touched |= mine

    # HP.1: one read of what every gate reads at HEAD.
    head = sh("git rev-parse HEAD")[0].strip()
    tree = sh("git rev-parse HEAD^{tree}")[0].strip()
    try:
        inputs = Inputs()
    except (RuntimeError, ValueError, KeyError) as e:
        inputs = None
        say(f"!! [closeout] no gate digests ({e}): every gate runs and nothing is recorded")
    logs_dir = rec_path.parent / "logs"
    statuses = {}   # gate -> {how: fresh|reused, tier, ok, commit}

    def digest_of(gate):
        return inputs.digest(gate) if inputs else None

    def gate(name, need, runner, extra=None, recordable=True, env_scan=None, rejudger=None):
        """Decide one recorded gate, then reuse it (re-print its recorded lines), re-judge it (HP.2: `rejudger(gsay,
        last entry)` judges the last run again, rank and cohort only) or run it, and record the result.
        `runner(gsay)` / `rejudger` return {ok, line, data, log, tier, more}; either may append to `gates`. Returns
        (data, reused entry or None)."""
        d = digest_of(name)
        intact = (lambda e: run_intact(name, e)) if rejudger else None
        action, ref, reason = decide(name, need, d, record, fresh, bool(dirty), extra, intact)
        if action == "reuse":
            c12 = ref["commit"][:12]
            say(f"[closeout] reuse {name} from {c12} (wave {ref.get('wave')}, {ref.get('tier')}, digest {ref['digest'][:12]})")
            for l in ref.get("lines") or []:
                say(f"{l} (reused {c12})")
            statuses[name] = {"how": "reused", "tier": ref.get("tier"), "ok": True, "commit": ref["commit"],
                              "line": ref.get("line", "")}
            return ref.get("data") or {}, ref
        rejudge = action == "rejudge"
        if rejudge:
            say(rejudge_line(name, ref, judge_moved_paths(ref, d, inputs.tracked) if inputs else None))
        else:
            if reason == "inputs moved" and ref and inputs:
                reason = inputs_moved_reason(ref, d, inputs.tracked)
            say(f"[closeout] run {name} ({reason})")
        glines, n0, started = [], len(gates), time.time()

        def gsay(s):
            say(s)
            glines.append(s)
        r = rejudger(gsay, ref) if rejudge else runner(gsay)
        ok = bool(r.get("ok")) and len(gates) == n0
        sig = env_fail_signature(r.get("log", ""), *(env_scan(started) if env_scan and not rejudge else ()))
        if sig:
            gl = r.get("line") or f"[{name}]"
            say(f"{gl if gl.startswith('==') else '== ' + gl} (environment: {sig})")
            gates.append(f"{name} (environment: {sig})")
        tier = r.get("tier", ref.get("tier", need) if rejudge else need)
        # HP.2: a fresh rank / cohort run keeps the sha256 of the run-meta.json it wrote, so a later re-judge knows
        # the run on disk is this one; a re-judge carries its measured run's forward.
        more = {}
        if rejudge:
            more = {"rejudged": True, "rejudged_from": measured_commit(ref), "measured_at": measured_commit(ref),
                    "run_meta_sha": ref.get("run_meta_sha")}
        elif name in GATE_RUN_DIRS:
            more = {"measured_at": head, "run_meta_sha": run_meta_sha(name, head)}
        statuses[name] = {"how": "rejudged" if rejudge else "fresh", "tier": tier, "ok": ok and not sig,
                          "commit": measured_commit(ref) if rejudge else head, "line": r.get("line", "")}
        if d and recordable and not r.get("skip_record") and (tier == "full" or name in ("rank", "cohort")):
            log = ""
            if r.get("log"):
                logs_dir.mkdir(parents=True, exist_ok=True)
                log = str(logs_dir / f"{name}-{d['digest'][:12]}.log")
                Path(log).write_text(r["log"])
            entry = {"digest": d["digest"], "measure": d["measure"], "judge": d["judge"], "env": d["env"],
                     "env_digest": d["env_digest"], "tier": tier, "ok": ok and not sig, "line": r.get("line", ""),
                     "lines": glines, "commit": head, "tree": tree, "wave": wave, "release": release,
                     "at": time.strftime("%Y-%m-%dT%H:%M:%S%z"), "log": log, "outputs": {}, "so_sha256": None,
                     "stamp": None, "env_fail": bool(sig), "data": r.get("data") or {}}
            entry.update(more)
            entry.update(r.get("more") or {})
            record_result(record, name, entry)
            save_record(rec_path, record)
        return r.get("data") or {}, None

    needs = None   # filled once rustdoc's crates are known

    # --- tests: the workspace and byte_identical --------------------------------------------------------------
    def run_tests(gsay):
        # --no-fail-fast: without it cargo stops at the first failing test binary, so one red binary hides
        # every other and the passed count is a fraction of the suite (0.5.1 W2: 262 of ~3,400 ran).
        out, _ = sh("cargo test --workspace --no-fail-fast --quiet 2>&1", timeout=2400)
        # Keep the full output: a one-line count cannot say which test or crate failed.
        WF.mkdir(parents=True, exist_ok=True)
        (WF / f"ws-test-{f'r{release}-' if release else 'leap-' if leap else ''}w{wave}.log").write_text(out)
        passed = sum(int(m) for m in re.findall(r"test result: ok\. (\d+) passed", out))
        failed = sum(int(m) for m in re.findall(r"(\d+) failed", out))
        comp = len(re.findall(r"^error(\[E\d+\])?:", out, re.M))
        gsay(f"== cargo test --workspace: {passed} passed, {failed} failed, {comp} compile errors")
        if failed or comp:
            gates.append("workspace tests")
        # byte_identical stays its own test target (FH.18's keep-list, engine/build.rs KEEP).
        out2, _ = sh(f"cargo test -p {engine_pkg} --test byte_identical 2>&1")
        # Any all-pass result: GA.2 (0.5.6 W1) added a third test, so a literal count goes stale.
        bi = re.search(r"test result: ok\. [1-9][0-9]* passed; 0 failed", out2) is not None
        line = f"== byte_identical: {'green' if bi else 'RED'}"
        gsay(line)
        if not bi:
            gates.append("byte_identical")
        return {"ok": True, "line": line, "data": {"passed": passed, "failed": failed, "comp": comp},
                "log": out + "\n" + out2}
    with step("tests"):
        tdata, _ = gate("tests", "full", run_tests)
    passed, failed = tdata.get("passed", 0), tdata.get("failed", 0)

    def run_fmt(gsay):
        ok, marker = fmt_check()
        gsay(marker)
        if not ok:
            gates.append("fmt")
        return {"ok": ok, "line": marker}
    with step("fmt"):
        gate("fmt", "full", run_fmt)

    def run_clippy(gsay):
        ok, marker, out = clippy_check()
        gsay(marker)
        if not ok:
            gates.append("clippy")
        return {"ok": ok, "line": marker, "log": out}
    with step("clippy"):
        gate("clippy", "full", run_clippy)

    full_doc = not remaining or any(full_due.values()) or os.environ.get("GLIA_RUSTDOC_FULL") == "1"
    needs = gate_needs(remaining, full_due, touched, rustdoc_crates=None if full_doc else touched_crates(touched))
    with step("rustdoc"):
        if needs["rustdoc"] is None:
            say("[rustdoc] check: skipped - wave touched no workspace crate")
        else:
            crates = None if needs["rustdoc"] == "full" else needs["rustdoc"].split(":", 1)[1].split(",")

            def run_rustdoc(gsay):
                ok, marker = rustdoc_check(crates)
                gsay(marker)
                if not ok:
                    gates.append("rustdoc")
                # Only a FULL (`--workspace`) run is recorded; a touched-crates run is never recorded.
                return {"ok": ok, "line": marker, "tier": "full" if crates is None else needs["rustdoc"]}
            gate("rustdoc", needs["rustdoc"], run_rustdoc, recordable=crates is None)

    # engram-export is outside the workspace, so the workspace tests above never compile it (LG.13).
    # Gate on its own last-line prefix (scripts/check-neuropil.sh prints `[neuropil-check] ...`).
    # SKIPPED fails too: here Engram is the sibling checkout, so a skip means nothing was checked.
    # Not recorded (HP.1): 1 s with its own build cache, and it reads the sibling ../Engram.
    with step("engram"):
        out, _ = sh("bash scripts/check-engram-export.sh 2>&1", timeout=1800)
    last = (out.strip().splitlines() or [""])[-1]
    say(f"== {last}")
    if not last.startswith("[engram-export] check: ok"):
        gates.append("engram-export (out-of-workspace)")

    def check_reuse_touch(gname, entry):
        """HP.1 (7): a reused rank / cohort gate whose packets touched its paths must have been recorded at a
        commit that already holds those packet commits; anything else is a digest-set defect. HP.2: a touched
        judge path must be in the entry's own commit, a touched measure path in the commit it measured at (the
        same commit but for a re-judge, whose judge inputs moved after its run)."""
        judge_paths = (digest_of(gname) or {}).get("judge_paths") or set()
        bad = []
        for pid, ps in touched_by.items():
            mine = [p for p in ps if p.startswith(GATE_PATHS[gname])]
            anchors = {entry["commit"] if p in judge_paths else measured_commit(entry) for p in mine}
            if any(sh(f"git merge-base --is-ancestor {c} {a}")[1] for c in commits_by.get(pid, ()) for a in anchors):
                bad.append(pid)
        if bad:
            gates.append(f"{gname} gate: reuse with touched paths ({','.join(sorted(set(bad)))})")

    # EG.3: the rank gate (scripts/rank-gate.sh, bench/rank-gate/), in every wave of a release that
    # has bench/rank-gate/gate.json. HP.1: its input digest decides (a wave that moved none of its inputs
    # reuses the last green check); with no gate record at all, a wave touching no ranked path skips it once.
    # The release's last close-out always checks in full, then requires full coverage.
    # closeout's sh lets TimeoutExpired escape: catch it so a slow check is a failed gate, not a crash.
    rank_reused = None
    with step("rank_gate"):
        rank_line = ""
        if (ROOT / "bench/rank-gate/gate.json").exists():
            ranked = sorted(p for p in touched if p.startswith(RANKED_PATHS))
            if not record_any and remaining and not ranked:
                rank_line = "[rank-gate] check: skipped - wave touched no ranked path"
                say(f"== {rank_line} (no record yet)")
            else:
                def run_rank(gsay):
                    fast = needs["rank"] != "full"
                    if not fast:
                        last_full["rank"] = wave
                    jobs = os.environ.get("GLIA_RANK_GATE_JOBS", RANK_GATE_JOBS)
                    env = (f"GLIA_RANK_GATE_ONLY={RANK_FAST} " if fast else "") + f"GLIA_RANK_GATE_JOBS={jobs} "
                    try:
                        out, _ = sh(f"{env}bash scripts/rank-gate.sh check 2>&1", timeout=RANK_GATE_TIMEOUT)
                    except subprocess.TimeoutExpired:
                        line = f"[rank-gate] check: FAILED (timeout after {RANK_GATE_TIMEOUT}s)"
                        gsay(f"== {line}")
                        gates.append("rank gate (timeout)" + gate_attribution("rank", line, asks["rank"], touched_by)[1])
                        return {"ok": False, "line": line, "tier": needs["rank"]}
                    WF.mkdir(parents=True, exist_ok=True)
                    (WF / f"rank-gate-{f'r{release}-' if release else ''}w{wave}.log").write_text(out)
                    line = (out.strip().splitlines() or [""])[-1]
                    gsay(f"== {line}")
                    ok = line.startswith("[rank-gate] check: ok")
                    if not ok:
                        gates.append("rank gate" + gate_attribution("rank", line, asks["rank"], touched_by)[1])
                    return {"ok": ok, "line": line, "tier": line_tier(line, needs["rank"]), "log": out}

                def rejudge_rank(gsay, ref):
                    # HP.2: the last run judged again at the tier it measured (a full run is re-judged in full)
                    only = tier_only(ref.get("tier", "full"))
                    env = f"GLIA_RANK_GATE_ONLY={only} " if only else ""
                    try:
                        out, _ = sh(f"{env}bash scripts/rank-gate.sh recheck 2>&1", timeout=REJUDGE_TIMEOUT)
                    except subprocess.TimeoutExpired:
                        line = f"[rank-gate] check: FAILED (re-judge timeout after {REJUDGE_TIMEOUT}s)"
                        gsay(f"== {line}")
                        gates.append("rank gate (re-judge timeout)"
                                     + gate_attribution("rank", line, asks["rank"], touched_by)[1])
                        return {"ok": False, "line": line, "tier": ref.get("tier", "full")}
                    WF.mkdir(parents=True, exist_ok=True)
                    (WF / f"rank-gate-{f'r{release}-' if release else ''}w{wave}.log").write_text(out)
                    line = (out.strip().splitlines() or [""])[-1]
                    gsay(f"== {line}")
                    ok = line.startswith("[rank-gate] check: ok")
                    if not ok:
                        gates.append("rank gate (re-judged)"
                                     + gate_attribution("rank", line, asks["rank"], touched_by)[1])
                    return {"ok": ok, "line": line, "tier": line_tier(line, "full"), "log": out}
                _, rank_reused = gate("rank", needs["rank"], run_rank, rejudger=rejudge_rank)
                rank_line = (rank_reused or {}).get("line") or statuses.get("rank", {}).get("line", "")
                rank_rejudged = statuses.get("rank", {}).get("how") == "rejudged"
                if rank_reused or rank_rejudged:
                    st = rank_reused or record["gates"]["rank"]["last"]
                    check_reuse_touch("rank", st)
                    if needs["rank"] == "full" and st.get("tier") == "full":
                        last_full["rank"] = wave
            if not remaining:
                out, _ = sh("bash scripts/rank-gate.sh coverage 2>&1")
                last = (out.strip().splitlines() or [""])[-1]
                say(f"== {last}")
                if not last.startswith("[rank-gate] coverage: ok"):
                    gates.append("rank gate coverage")
    rank_st = statuses.get("rank", {})
    say(gate_attribution("rank", rank_line, asks["rank"], touched_by,
                         reused_from=rank_reused["commit"] if rank_reused else None,
                         rejudged_from=rank_st["commit"] if rank_st.get("how") == "rejudged" else None)[0])

    cohort_dir = Path(os.environ.get("GLIA_COHORT_GATE_DIR") or Path.home() / ".cache/glia-cohort-gate")
    cohort_reused = None
    with step("cohort_gate"):
        if not record_any and remaining and not any(p.startswith(COHORT_PATHS) for p in touched):
            _, cohort_line, _, _ = cohort_gate(touched, True, wave, release, full_due["cohort"])
            say(f"{cohort_line} (no record yet)")
        else:
            def run_cohort(gsay):
                ok, line, fail, out = cohort_gate(touched, bool(remaining), wave, release, needs["cohort"] == "full",
                                                  skip_untouched=False)
                gsay(line)
                if fail:
                    gates.append(fail + gate_attribution("cohort", line, asks["cohort"], touched_by)[1])
                skipped = "check: skipped" in line
                # A skip (GLIA_COHORT_GATE_SKIP, no script) checked nothing: never recorded.
                return {"ok": ok and not skipped, "line": line[3:] if line.startswith("== ") else line,
                        "tier": line_tier(line, needs["cohort"]), "log": out, "skip_record": skipped}
            def rejudge_cohort(gsay, ref):
                # HP.2: the last run judged again at the tier it measured (a full run is re-judged in full)
                only = tier_only(ref.get("tier", "full"))
                try:
                    out, _ = sh(f"bash scripts/cohort-gate.sh recheck{f' --only {only}' if only else ''} 2>&1",
                                timeout=REJUDGE_TIMEOUT)
                except subprocess.TimeoutExpired:
                    out = ""
                    last = f"[cohort-gate] check: FAILED (re-judge timeout after {REJUDGE_TIMEOUT}s)"
                else:
                    WF.mkdir(parents=True, exist_ok=True)
                    (WF / f"cohort-gate-{f'r{release}-' if release else ''}w{wave}.log").write_text(out)
                    last = (out.strip().splitlines() or [""])[-1]
                if only and "(partial: only" not in last:
                    last += f" (partial: only {only})"
                gsay(f"== {last}")
                ok = last.startswith("[cohort-gate] check: ok")
                if not ok:
                    gates.append("cohort gate (re-judged)"
                                 + gate_attribution("cohort", last, asks["cohort"], touched_by)[1])
                return {"ok": ok, "line": last, "tier": line_tier(last, "full"), "log": out}
            _, cohort_reused = gate("cohort", needs["cohort"], run_cohort,
                                    env_scan=lambda since: recent_texts(cohort_dir / "run", since),
                                    rejudger=rejudge_cohort)
            cohort_line = "== " + ((cohort_reused or {}).get("line") or statuses.get("cohort", {}).get("line", ""))
            cohort_rejudged = statuses.get("cohort", {}).get("how") == "rejudged"
            if cohort_reused or cohort_rejudged:
                st = cohort_reused or record["gates"]["cohort"]["last"]
                check_reuse_touch("cohort", st)
                if needs["cohort"] == "full" and st.get("tier") == "full":
                    last_full["cohort"] = wave
            elif "(partial: only" not in cohort_line and "skipped" not in cohort_line:
                last_full["cohort"] = wave
    cohort_st = statuses.get("cohort", {})
    say(gate_attribution("cohort", cohort_line, asks["cohort"], touched_by,
                         reused_from=cohort_reused["commit"] if cohort_reused else None,
                         rejudged_from=cohort_st["commit"] if cohort_st.get("how") == "rejudged" else None)[0])

    # --- HP.3: the build timings (scripts/build-timing.sh, bench/build-timing/RULE.md) ---------------------------------
    # After the cohort gate, on its binary: the script's own `cargo build --release -p glia-cli` is the cohort gate's
    # (a no-op when that just ran). Advisory per wave, gating at the release (timing_verdict); a skip is never
    # recorded, a per-wave FAILED is recorded as failed (so the next close-out runs it again).
    def run_timing(gsay):
        if not (ROOT / "scripts/build-timing.sh").exists():
            line, out = "[build-timing] check: skipped - no scripts/build-timing.sh", ""
        else:
            try:
                out, _ = sh("bash scripts/build-timing.sh check 2>&1", timeout=BUILD_TIMING_TIMEOUT)
            except subprocess.TimeoutExpired:
                out, line = "", f"[build-timing] check: FAILED (timeout after {BUILD_TIMING_TIMEOUT}s)"
            else:
                WF.mkdir(parents=True, exist_ok=True)
                (WF / f"build-timing-{f'r{release}-' if release else ''}w{wave}.log").write_text(out)
                line = (out.strip().splitlines() or [""])[-1]
        ok, fail, shown = timing_verdict(line, bool(remaining))
        gsay(shown)
        if fail:
            gates.append(fail)
        return {"ok": ok, "line": line, "log": out, "skip_record": "] check: skipped" in line}
    with step("build_timing"):
        gate("timing", needs["timing"], run_timing)

    # --- the wheel: rebuild + install + its surface checks ------------------------------------------------------
    def run_wheel(gsay):
        sh(f"cargo clean -p {engine_pkg} -p {py_pkg}")
        out, rc = sh("maturin build -m py/Cargo.toml --release 2>&1", timeout=2400)
        log = [out]
        wheels = sorted(WHEELS.glob(WHEEL_GLOB), key=lambda p: p.stat().st_mtime)
        if wheels:
            sh(f"{py} -m pip install --force-reinstall --no-deps -q {wheels[-1]}")
        so, _ = sh(f"{py} -c \"import glia_py,glob,os;print(glob.glob(os.path.join(os.path.dirname(glia_py.__file__),'*.so'))[0])\"")
        stale, _ = sh(f"find {' '.join(CRATES)} -name '*.rs' -newer {so.strip()}")
        stamp, _ = sh(f"{py} -c 'import glia_py as r; print(r.build_stamp())'")
        line = (f"== wheel: {wheels[-1].name if wheels else f'NO {WHEEL_GLOB}'} build rc={rc}, "
                f"stale={len(stale.split())}, stamp {stamp.strip()}")
        gsay(line)
        if rc or not wheels or stale.strip():
            gates.append("wheel rebuild/stale")
        # LG.6a: the installed wheel's surface (module name included: py/api_surface/lib.txt's
        # `pymodule` line) against the committed snapshots.
        out, rc = sh(f"{py} py/check_api_surface.py 2>&1")
        log.append(out)
        gsay(f"== api surface: {(out.strip().splitlines() or ['(no output)'])[-1]}")
        if rc:
            gates.append("py/check_api_surface.py")
        # A12: the CLI and the installed wheel report the same message contracts.
        out, rc = sh(f"PYTHON={py} bash bench/message-contracts/check.sh 2>&1")
        log.append(out)
        gsay(f"== message-contracts: {(out.strip().splitlines() or ['(no output)'])[-1]}")
        if rc:
            gates.append("bench/message-contracts/check.sh")
        # LD.2: the pyo3 surface behaves as its convention says, over the wheel just installed.
        # One file per py/src module; each ends on `[surface] <module>: N checks, M failed`.
        for t in sorted((ROOT / "py/tests/surface").glob("test_*.py")):
            out, rc = sh(f"{py} {t.relative_to(ROOT)} 2>&1")
            log.append(out)
            gsay(f"== pyo3 surface {t.stem}: {(out.strip().splitlines() or ['(no output)'])[-1]}")
            if rc:
                gates.append(f"pyo3 surface ({t.stem})")
        cur = inputs.installed(refresh=True) if inputs else ("", "none", "none")
        return {"ok": True, "line": line, "log": "\n".join(log), "more": {"so_sha256": cur[1], "stamp": cur[2]}}

    def wheel_extra(e):
        # Reuse needs the INSTALLED wheel to be the recorded one, else it is rebuilt.
        cur = inputs.installed() if inputs else ("", "none", "none")
        return None if (e.get("so_sha256"), e.get("stamp")) == cur[1:] else "installed-wheel"
    with step("wheel"):
        gate("wheel", "full", run_wheel, extra=wheel_extra)

    # --- the bench: run.py, matrix, test_matrix, test_grade -----------------------------------------------------
    bench_wrote = [False]

    def bench_delta(data, saying):
        """A NEW blind / forbid / missing-node / missing-cell fixture row against baseline.json is a gate (W8
        committed evm-ts-imported-instance blind: the packet deferred its grade to here). A new partial is
        allowed: a partial packet declares it."""
        old = base["run_py"]
        for label, now, prev in (("blind", data["bsl"], old.get("blind_list", [])),
                                 ("partial", data["pal"], old.get("partial_list", [])),
                                 ("forbid", data["fvl"], old.get("forbid_list", [])),
                                 ("missing cells", data["mcl"], old.get("missing_cells_list", [])),
                                 ("missing nodes", data["mnl"], old.get("missing_nodes_list", []))):
            new, gone = sorted(set(now) - set(prev)), sorted(set(prev) - set(now))
            if new:
                saying(f"   NEW {label}: {new}")
                if label != "partial":
                    gates.append(f"run.py NEW {label}: {new}")
            if gone:
                saying(f"   cleared {label}: {gone}")

    def run_bench(gsay):
        rp, _ = sh(f"{py} run.py --no-log 2>/dev/null", cwd=SG)
        fixtures = len([p for p in (SG / "fixtures").iterdir() if (p / "key.json").exists()])
        bs, bsl = section(rp, "BLIND SPOTS (recall 0.00)")
        mn, mnl = section(rp, "MISSING NODES")
        pa, pal = section(rp, "PARTIAL (0 < recall < 1)")
        fv, fvl = section(rp, "FORBID VIOLATIONS")
        mc, mcl = section(rp, "MISSING CELLS")
        ge, gel = section(rp, "GRADER ERRORS (fixture NOT in the matrix)")
        gsay(f"== run.py: {fixtures} fixtures | blind {bs} | missing nodes {mn} | partial {pa} | forbid {fv} | missing cells {mc} | grader errors {ge}")
        data = {"fixtures": fixtures, "bs": bs, "bsl": bsl, "mn": mn, "mnl": mnl, "pa": pa, "pal": pal,
                "fv": fv, "fvl": fvl, "mc": mc, "mcl": mcl, "ge": ge, "gel": gel}
        bench_delta(data, gsay)
        if ge:
            gates.append(f"grader errors: {gel}")
        bench_wrote[0] = True
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
        line = (f"== matrix: {full} full, {part} partial, {none_} none, {unk} unknown, {na} n/a = {covered}/{grid} | "
                f"check={chk} | legacy check={lchk} | test_matrix: {tm.strip()} | test_grade: {tg.strip()}")
        gsay(line)
        if chk:
            gates.append("matrix --check")
        if " 0 failed" not in tm:
            gates.append("test_matrix")
        if "all passed" not in tg:
            gates.append("test_grade")
        data.update({"lchk": lchk, "full": full, "part": part, "none": none_, "unk": unk, "na": na, "grid": grid,
                     "covered": covered, "chk": chk, "tm": tm.strip(), "tg": tg.strip()})
        return {"ok": True, "line": line, "data": data, "log": rp + "\n" + mo, "more": {"outputs": outputs_now()}}

    with step("bench"):
        bdata, bench_reused = gate("bench", "full", run_bench,
                                   extra=lambda e: None if outputs_now() == (e.get("outputs") or {}) else "outputs")
        if bench_reused:
            bench_delta(bdata, say)
    fixtures, bs, bsl, pa, pal = (bdata.get(k) for k in ("fixtures", "bs", "bsl", "pa", "pal"))
    fv, fvl, mc, mcl, mnl, ge = (bdata.get(k) for k in ("fv", "fvl", "mc", "mcl", "mnl", "ge"))
    full, part, none_, unk, na = (bdata.get(k, 0) for k in ("full", "part", "none", "unk", "na"))
    grid, covered, chk = bdata.get("grid", 0), bdata.get("covered", 0), bdata.get("chk", 1)
    tm, tg = bdata.get("tm", ""), bdata.get("tg", "")

    # fold followups into remaining packets
    rem = {p for w in remaining for p in w}
    if release:
        pat = re.compile(rf"\b({RELEASE_PREFIX[release]}[0A-Z]\.\d+[a-z]?)\b")
        cp = ROOT / f"dev-notes/leap-{release}-corrections.json"
    else:
        pat = re.compile(r"\b(A\d+\.\d+[a-z]?|L[A-G0]\.\d+[a-z]?)\b" if leap else r"\b(A\d+\.\d+[a-c]?)\b")
        cp = ROOT / ("dev-notes/leap-corrections.json" if leap else "dev-notes/packet-corrections.json")
    cf = json.loads(cp.read_text())
    folded, unrouted = {}, []
    for r in res:
        src = packet_id(r["packet"])
        for t in r.get("followups") or []:
            named = set(pat.findall(t))
            if not named & rem:
                # no packet id, or only ids of landed packets: nothing to fold into, so the
                # session routes it by hand (an owner by files_touched, or the next leap's seed)
                unrouted.append((src, sorted(named), t.strip()))
            for tgt in named & rem:
                note = f"HANDOFF FROM {src} (w{wave}): {t.strip()}"
                if note not in cf["corrections"].get(tgt, ""):
                    cf["corrections"][tgt] = (cf["corrections"].get(tgt, "") + "\n\n" + note).strip()
                    folded[tgt] = folded.get(tgt, 0) + 1
    cp.write_text(json.dumps(cf, indent=1))
    say(f"== handoffs folded: {folded or 'none'}")
    say(f"== unrouted followups (route by hand): {len(unrouted)}")
    for src, named, t in unrouted:
        print(f"   [{src}{' names ' + ','.join(named) if named else ''}] {t}")

    # HP.1 (5): the release's last close-out needs one full green set over HEAD's inputs.
    if not remaining:
        set_line, per_gate, missing = release_set(statuses, tree)
        say(set_line)
        for l in per_gate:
            say(l)
        if missing:
            gates.append(f"release set incomplete: {','.join(missing)}")

    # HP.1 (9): baseline.json, LANDED, `schedule --verify` and the next wave's script are written only by a green
    # close-out; a failed one leaves the tree as it found it (the gate record holds what passed).
    sp = HERE / (f"schedule_{release}.py" if release else "leap_schedule.py" if leap else "schedule.py")
    originals = {}
    if not gates:
        originals = {HERE / "baseline.json": (HERE / "baseline.json").read_text(), sp: sp.read_text()}
        base.update({"measured_at_head": sh("git rev-parse --short HEAD")[0].strip(), "after_wave": wave})
        base["cargo_test_workspace"] = {"passed": passed, "failed": failed}
        base["history_passed"].append(passed)
        base["run_py"].update({"fixtures": fixtures, "blind_spots": bs, "partial": pa, "forbid_violations": fv,
                               "missing_cells": mc, "grader_errors": ge, "blind_list": bsl, "partial_list": pal,
                               "forbid_list": fvl, "missing_cells_list": mcl, "missing_nodes_list": mnl,
                               "blind_spot_detail": f"blind: {bsl}; missing nodes: {mnl}; missing cells: {mcl}. "
                                                    "Those present at wave start are deliberate baselines for later packets unless your packet owns them."})
        base["matrix_py"] = {"full": full, "partial": part, "none": none_, "unknown": unk, "covered": covered,
                             "grid": grid, "n/a": na, "invalid_cell_declarations": 0, "check_exit": chk}
        base["test_matrix_py"] = tm
        base["test_grade_py"] = tg
        (HERE / "baseline.json").write_text(json.dumps(base, indent=2))

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
        if remaining and not gates:
            flag = f"--release {release} " if release else "--leap " if leap else ""
            out, _ = sh(f"python3 dev-notes/wave-runner/gen_wave.py {flag}{wave+1} /tmp/claude-1000/-home-ivy-Code-glia/wf/{tag}wave{wave+1}.js")
            say("== " + out.strip().replace("\n", " | "))
    else:
        say("== baseline.json / LANDED / next wave: not written (gates failed)")

    # James 2026-10-26 ("we gotta stop letting this happen"): /home filled during a rebuild with
    # target/ at 251 GB. Every close-out drops cargo's incremental caches (regenerable, the largest
    # disposable part) and records target/ size and free space; usage_gate.py HOLDs below 60 GB.
    shutil.rmtree(ROOT / "target/debug/incremental", ignore_errors=True)
    tgt, _ = sh("du -sh target 2>/dev/null | cut -f1")
    say(f"== disk: target/ {tgt.strip()} after dropping target/debug/incremental; "
        f"{shutil.disk_usage(ROOT).free / 2**30:.0f} GB free on /home")

    if leap or release:
        # Record this wave's 5-hour usage cost and say whether the next wave can start (usage_gate.py).
        sh(f"python3 dev-notes/wave-runner/usage_gate.py --end {f'R{release}-' if release else ''}W{wave}")
        out, _ = sh("python3 dev-notes/wave-runner/usage_gate.py")
        say("== usage for the next wave: " + out.strip().replace("\n", " | "))

    # HP.1 (6): a ratchet runs only after a FRESH and FULL green check in this close-out, everything green.
    # (The scripts' own HEAD guards already refuse a ratchet from a check made at another HEAD.)
    for gname, script, label in (("rank", "scripts/rank-gate.sh", "rank-gate"),
                                 ("cohort", "scripts/cohort-gate.sh", "cohort-gate")):
        st = statuses.get(gname)
        if ratchet_due(st, not gates):
            # The ratchet: the green check's results become baseline/current (only the moved ones; the cohort's
            # is held while a reach decline sits inside the tolerance, so the baseline never creeps down).
            with step(f"{gname}_gate"):
                out, rc = sh(f"bash {script} ratchet 2>&1")
            say(f"== {(out.strip().splitlines() or ['(no output)'])[-1]}")
            if rc:
                gates.append(f"{gname} gate ratchet")
        elif st and st.get("how") == "reused" and st.get("tier") == "full" and not gates:
            say(f"== [{label}] ratchet: skipped - reused from {st['commit'][:12]}; GLIA_CLOSEOUT_FRESH={gname} to ratchet")
        elif st and st.get("how") == "rejudged" and not gates:
            # HP.2: a re-judge measured nothing at HEAD, so it never moves a baseline (the scripts refuse it too)
            say(f"== [{label}] ratchet: skipped - re-judged run of {st['commit'][:12]}; "
                f"GLIA_CLOSEOUT_FRESH={gname} to ratchet")
    # GP.7: where the close-out's time went, one line on stdout, in the WF log and in the commit body.
    say("[closeout] timing " + " ".join(f"{k}={int(timing[k]) if k in timing else '-'}" for k in TIMING_STEPS)
        + f" total={int(time.time() - t0)}")
    write_record()
    if gates:
        # Leave the tree as the close-out found it: the files this run wrote go back to HEAD's content.
        for p, text in originals.items():
            p.write_text(text)
        if bench_wrote[0]:
            back = [o for o in SG_OUTPUTS if o not in dirty_at_start and inputs and o in inputs.tracked]
            if back:
                sh("git checkout HEAD -- " + " ".join(back))
        say(f"!! NOT COMMITTING — gates failed: {gates}")
        write_record()
        sys.exit(1)
    what = f"0.{release[1]}.{release[2]} wave W{wave}" if release else f"leap wave W{wave}" if leap else f"wave {wave}"
    msg = (f"bench+plan: {what} landed — {passed} tests, {fixtures} fixtures, matrix {covered}/{grid}"
           + (f", {na} n/a" if na else "") + "\n\n"
           + "\n".join(lines) + "\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>\n"
           + "Claude-Session: https://claude.ai/code/session_011hMXDeYW7TWXZdUnNxWakK\n")
    Path("/tmp/claude-1000/-home-ivy-Code-glia/closeout.msg").write_text(msg)
    out, rc = sh("git add bench/substrate-gap/results-latest.json bench/substrate-gap/COVERAGE.md bench/substrate-gap/legacy-latest.json "
                 "dev-notes/packet-corrections.json dev-notes/leap-corrections.json dev-notes/leap-051-corrections.json "
                 + (f"dev-notes/leap-{release}-corrections.json " if release else "")
                 + ("bench/rank-gate/baseline/current " if (ROOT / "bench/rank-gate/baseline/current").exists() else "")
                 + ("bench/cohort/results/baseline " if (ROOT / "bench/cohort/results/baseline").exists() else "")
                 + "dev-notes/wave-runner && "
                 "git -c user.name='james chahwan' commit -q -F /tmp/claude-1000/-home-ivy-Code-glia/closeout.msg && git log --oneline -1")
    say(f"== committed: {out.strip()}" if rc == 0 else f"!! commit failed: {out}")
    write_record()
    # Build caches grow by tens of GB a wave (0.5.1: target/ reached 219G and engram-export/target 59G,
    # and W1's gate died on "No space left on device"). rustc's incremental cache is the bulk and is
    # safe to drop: the next build is slower, never different. Every third landed wave, drop it.
    if rc == 0 and wave % 3 == 0:
        sh("rm -rf target/debug/incremental engram-export/target/debug/incremental")
        say(f"== dropped the incremental build caches (every 3rd wave); repo disk free "
            f"{shutil.disk_usage(str(ROOT)).free / 2**30:.0f}G")


if __name__ == "__main__":
    main()
