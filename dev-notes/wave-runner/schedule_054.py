#!/usr/bin/env python3
"""Wave scheduler for the glia 0.5.4 packet set (dev-notes/leap-054-packets.json): scale, embeddings, the goal layer, precision.

Rule (the 0.5.0 leap's, dev-notes/wave-runner/leap_schedule.py): two packets share
a wave only if their CLAIMED files (files_touched) are disjoint, because the
implementing agents edit one working tree concurrently. Among ready packets, the one
with the longest chain of dependants goes first, then id order (0.5.4 change). Wave 0 is the F0 packets,
run one at a time in `wave0_serial` order (F0.5, the whole-tree rustfmt, last); every
other packet implicitly depends on all of them. A packet listed in the file's
`exclusive` runs ALONE in its wave (FD.7d edits test pins across the tree; FE.4 holds
James's cost approval). After that, a packet is ready when every depends_on is done, and each
wave is the maximal set of ready packets, taken in id order, with pairwise-disjoint
files_touched.

The runner schedules with at most CAP packets per wave (0.5.0's largest wave was 26):
a smaller wave keeps the end-of-wave gate readable (0.5.2 set the cap; 0.5.3 and 0.5.4 keep it).
gen_wave.py / closeout.py `--release 054` read `waves()` (the 0.5.4 plan commit adds
"054" to their RELEASES with RELEASE_PREFIX "F").

A gated (b) packet whose eval (a) reported FAIL ends `not-needed`; it still counts as
done, so nothing that depends on it stalls.

Tracks (tracks_054.py): James builds 0.5.4 a track at a time. SELECTED names the tracks
chosen so far (T0, wave 0, is always in); `tracks_054.py --select T2 T5` adds to it. waves()
is then the LANDED waves verbatim, followed by the remaining packets of the selected tracks
scheduled on top of them (landed packets count as done). Choosing another track later only
schedules after the last landed wave, so history never moves. A selected track whose packet
depends on a packet of an unselected, unlanded track is refused, naming the track to add.

Usage:
  python3 schedule_054.py                  # the selected tracks' waves (LANDED + scheduled), sizes
  python3 schedule_054.py --all            # the whole 169-packet plan as one schedule (the spec run's view)
  python3 schedule_054.py --verify         # LANDED waves: contiguous, file-disjoint, dependencies in earlier waves
  python3 schedule_054.py --wave N         # wave N's packets, one per line (W0 = the serial F0 packets)
  python3 schedule_054.py --priority       # also run 0.5.0's order (most-unblocking first)
  python3 schedule_054.py --json           # machine-readable waves
  python3 schedule_054.py --stats          # contention table (files with 3+ claimants)
  python3 schedule_054.py --cap 26         # also schedule with at most N packets per wave
  python3 schedule_054.py --file other.json
"""
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACKETS = HERE.parents[1] / "dev-notes" / "leap-054-packets.json"
if not PACKETS.exists():  # the spec run's scratchpad copy sits beside this file
    PACKETS = HERE / "leap-054-packets.json"
CAP = 26
EXCLUSIVE = set()

# Waves that have landed, by number; closeout.py --release 054 appends each one, and
# --verify then guards that re-running the scheduler reproduces history.
LANDED = {}
# Tracks chosen for building (tracks_054.py --select appends; T0 is wave 0 and always in).
SELECTED = "T0"
GROUP_ORDER = {"0": 0, "A": 1, "B": 2, "C": 3, "D": 4, "E": 5, "Z": 9}

# 0.5.3 (the ranking release, ids E*) lands before 0.5.4's W0: its packets are treated
# as done, so a depends_on naming one is satisfied from the start.
E_DONE = re.compile(r"E[0-9A-Z]\.\d+[a-z]?")


def id_key(pid):
    m = re.fullmatch(r"F([0-9A-Z])\.(\d+)([a-z]?)", pid)
    if not m:
        raise SystemExit(f"bad packet id {pid!r}")
    return (GROUP_ORDER.get(m.group(1), 8), m.group(1), int(m.group(2)), m.group(3))


def load(path):
    doc = json.loads(Path(path).read_text())
    P = {p["id"]: p for p in doc["packets"]}
    serial = list(doc.get("wave0_serial", []))
    claims = {i: set(p["files_touched"]) for i, p in P.items()}
    deps = {i: {d for d in (p.get("depends_on") or []) if not E_DONE.fullmatch(d)} for i, p in P.items()}
    global EXCLUSIVE
    EXCLUSIVE = set(doc.get("exclusive", []))
    missing = sorted({(i, d) for i, ds in deps.items() for d in ds if d not in P})
    if missing:
        raise SystemExit(f"depends_on names no packet: {missing[:10]}")
    for i in P:
        if i not in serial:
            deps[i] |= set(serial)
    return P, claims, deps, serial


def check_acyclic(deps):
    state = {}

    def visit(i, stack):
        s = state.get(i)
        if s == 1:
            cyc = stack[stack.index(i):] + [i]
            raise SystemExit("dependency cycle: " + " -> ".join(cyc))
        if s == 2:
            return
        state[i] = 1
        for d in sorted(deps[i]):
            visit(d, stack + [i])
        state[i] = 2

    for i in sorted(deps, key=id_key):
        visit(i, [])


def tail_depth(deps):
    """Longest chain of dependants hanging below each packet (itself included)."""
    rdeps = defaultdict(set)
    for i, ds in deps.items():
        for d in ds:
            rdeps[d].add(i)
    memo = {}

    def depth(i):
        if i not in memo:
            memo[i] = 1 + max((depth(j) for j in rdeps[i]), default=0)
        return memo[i]

    return {i: depth(i) for i in deps}


def schedule(P, claims, deps, serial, priority=False, cap=0):
    unblocks = Counter(d for ds in deps.values() for d in ds)
    tail = tail_depth(deps)
    waves = [list(serial)]
    done = set(serial)
    left = set(P) - done
    while left:
        ready = [i for i in left if deps[i] <= done]
        if not ready:
            raise SystemExit(f"deadlock: {sorted(left, key=id_key)[:12]}")
        if priority:
            ready.sort(key=lambda i: (-unblocks[i], -len(claims[i]), id_key(i)))
        else:
            # 0.5.4: the packet with the longest chain of dependants below it goes
            # first, then id order (0.5.3 used id order alone; with 169 packets on
            # shared files, id order let the gated FA chain hold watch_parity.rs
            # while the goal-layer critical path waited).
            ready.sort(key=lambda i: (-tail[i], id_key(i)))
        used, wave = set(), []
        excl = [i for i in ready if i in EXCLUSIVE]
        if excl:  # an exclusive packet runs alone, as soon as it is ready
            ready = excl[:1]
        for i in ready:
            if claims[i] & used:
                continue
            if cap and len(wave) >= cap:
                break
            wave.append(i)
            used |= claims[i]
        waves.append(sorted(wave, key=id_key))
        done |= set(wave)
        left -= set(wave)
    return waves


def critical_path(P, deps, serial):
    """Longest depends_on chain over the non-F0 packets (F0 is the serial wave 0)."""
    memo = {}

    def depth(i):
        if i not in memo:
            best = (0, [])
            for d in deps[i]:
                if d in serial:
                    continue
                cand = depth(d)
                if cand[0] > best[0] or (cand[0] == best[0] and cand[1] < best[1]):
                    best = cand
            memo[i] = (best[0] + 1, best[1] + [i])
        return memo[i]

    return max((depth(i) for i in P if i not in serial), key=lambda t: (t[0], [id_key(x) for x in t[1]]), default=(0, []))


def landed_waves():
    """LANDED as a list of waves, W0 first; refuses a gap in the numbering."""
    if sorted(LANDED) != list(range(len(LANDED))):
        raise SystemExit(f"LANDED is not W0..W{len(LANDED) - 1} contiguous: {sorted(LANDED)}")
    return [LANDED[n].split() for n in range(len(LANDED))]


def selected(P, deps):
    """The packets of the SELECTED tracks plus every landed packet; refuses a selection whose
    packets depend on an unselected, unlanded track."""
    import tracks_054
    of = tracks_054.track_of(P)
    tracks = set(SELECTED.split()) | {"T0"}
    unknown = tracks - set(tracks_054.TRACKS)
    if unknown:
        raise SystemExit(f"SELECTED names no track: {sorted(unknown)}")
    done = {i for w in landed_waves() for i in w}
    sel = {i for i in P if of[i] in tracks} | done
    need = defaultdict(set)
    for i in sel - done:
        for d in deps[i]:
            if d not in sel:
                need[of[d]].add(f"{i}<-{d}")
    if need:
        raise SystemExit("selected tracks need unselected ones: "
                         + "; ".join(f"{t} ({' '.join(sorted(v))})" for t, v in sorted(need.items())))
    return sel, done


def schedule_rest(P, claims, deps, done, sel, cap=0):
    """The waves after the landed ones: the selected packets not yet done, scheduled as
    schedule() does with `done` already satisfied."""
    tail = tail_depth({i: deps[i] & sel for i in sel})
    done, left, out = set(done), set(sel) - set(done), []
    while left:
        ready = [i for i in left if deps[i] <= done]
        if not ready:
            raise SystemExit(f"deadlock: {sorted(left, key=id_key)[:12]}")
        ready.sort(key=lambda i: (-tail[i], id_key(i)))
        used, wave = set(), []
        excl = [i for i in ready if i in EXCLUSIVE]
        if excl:
            ready = excl[:1]
        for i in ready:
            if claims[i] & used:
                continue
            if cap and len(wave) >= cap:
                break
            wave.append(i)
            used |= claims[i]
        out.append(sorted(wave, key=id_key))
        done |= set(wave)
        left -= set(wave)
    return out


def waves(path=PACKETS):
    """(P, claims, waves) exactly as the runner schedules them: LANDED, then the selected
    tracks' remaining packets. With nothing landed, W0 is the serial F0 wave."""
    P, claims, deps, serial = load(path)
    check_acyclic(deps)
    sel, done = selected(P, deps)
    head = landed_waves()
    if not head:
        head, done = [list(serial)], set(serial)
    return P, claims, head + schedule_rest(P, claims, deps, done, sel, cap=CAP)


def verify_landed(P, claims, deps, serial):
    """Each landed wave: its packets exist, their dependencies landed in an earlier wave (W0's
    serial order excepted), and a parallel wave is file-disjoint."""
    bad, before = 0, set()
    for n, w in enumerate(landed_waves()):
        probs = [i for i in w if i not in P]
        if n:
            probs += [f"{i}<-{d}" for i in w if i in P for d in sorted(deps[i] - before)]
            hits = Counter(f for i in w if i in P for f in claims[i])
            probs += [f"shared {f}" for f, k in hits.items() if k > 1]
        elif w != list(serial):
            probs.append(f"W0 is not wave0_serial {' '.join(serial)}")
        print(f"W{n} " + ("ok" if not probs else "MISMATCH " + "; ".join(probs)) + f" ({len(w)} packets)")
        bad += bool(probs)
        before |= set(w)
    return bad


def main(argv):
    path = PACKETS
    if "--file" in argv:
        path = Path(argv[argv.index("--file") + 1])
    P, claims, deps, serial = load(path)
    check_acyclic(deps)
    if "--verify" in argv:
        bad = verify_landed(P, claims, deps, serial)
        print("VERIFIED" if not bad else f"{bad} waves differ")
        return 1 if bad else 0
    if "--all" in argv:
        waves = schedule(P, claims, deps, serial, cap=CAP)
    else:
        _, _, waves = globals()["waves"](path)
        P = {i: P[i] for w in waves for i in w}
    if "--wave" in argv:
        print("\n".join(waves[int(argv[argv.index("--wave") + 1])]))
        return 0
    n_len, chain = critical_path(P, deps, serial)
    loc = {i: P[i]["loc"] for i in P}
    if "--json" in argv:
        print(json.dumps({"waves": waves, "critical_path": chain}, indent=1))
        return 0
    print(f"packets {len(P)}  LOC {sum(loc.values())}  wave0 (serial) {len(serial)}: {' '.join(serial)}")
    print(f"WAVES {len(waves)} (W0 serial + {len(waves) - 1} parallel, at most {CAP} packets per wave)")
    for n, w in enumerate(waves):
        tag = "W0 serial" if n == 0 else f"W{n}"
        print(f"{tag:>9} {len(w):>3} pkts {sum(loc[i] for i in w):>6} LOC  {' '.join(w)}")
    print(f"CRITICAL PATH (depends_on, after W0): {n_len} packets: {' -> '.join(chain)}")
    print(f"  lower bound on waves: 1 (W0) + {n_len}; file contention adds {len(waves) - 1 - n_len}")
    if "--priority" in argv:
        pw = schedule(P, claims, deps, serial, priority=True)
        print(f"PRIORITY ORDER (0.5.0 leap_schedule: most-unblocking first): {len(pw)} waves")
        for n, w in enumerate(pw):
            if n:
                print(f"{'W' + str(n):>9} {len(w):>3} pkts  {' '.join(w)}")
    if "--cap" in argv:
        cap = int(argv[argv.index("--cap") + 1])
        cw = schedule(P, claims, deps, serial, cap=cap)
        print(f"WITH A CAP OF {cap} PACKETS PER WAVE: {len(cw)} waves ({' '.join(str(len(w)) for w in cw[1:])})")
    if "--stats" in argv:
        c = defaultdict(list)
        for i in P:
            for f in claims[i]:
                c[f].append(i)
        print("CONTENTION (files with 3+ claimants):")
        for f, ids in sorted(c.items(), key=lambda kv: (-len(kv[1]), kv[0])):
            if len(ids) >= 3:
                print(f"  {len(ids):>2} {f}: {' '.join(sorted(ids, key=id_key))}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
