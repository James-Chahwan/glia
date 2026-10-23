#!/usr/bin/env python3
"""Wave scheduler for the glia 0.5.3 packet set (dev-notes/leap-053-packets.json): the ranking release.

Rule (the 0.5.0 leap's, dev-notes/wave-runner/leap_schedule.py): two packets share
a wave only if their CLAIMED files (files_touched) are disjoint, because the
implementing agents edit one working tree concurrently. Wave 0 is the E0 packets,
run one at a time in `wave0_serial` order; every other packet implicitly depends on
all of them. After that, a packet is ready when every depends_on is done, and each
wave is the maximal set of ready packets, taken in id order, with pairwise-disjoint
files_touched.

The runner schedules with at most CAP packets per wave (0.5.0's largest wave was 26):
a smaller wave keeps the end-of-wave gate readable (0.5.2's schedule_053.py set the cap; 0.5.3 keeps it).
gen_wave.py / closeout.py `--release 053` read `waves()` (the 0.5.3 plan commit adds
"053" to their RELEASES with RELEASE_PREFIX "E").

Usage:
  python3 schedule_053.py                  # waves, sizes, critical path
  python3 schedule_053.py --verify         # assert LANDED waves still match
  python3 schedule_053.py --wave N         # wave N's packets, one per line (W0 = the serial E0 packets)
  python3 schedule_053.py --priority       # also run 0.5.0's order (most-unblocking first)
  python3 schedule_053.py --json           # machine-readable waves
  python3 schedule_053.py --stats          # contention table (files with 3+ claimants)
  python3 schedule_053.py --cap 26         # also schedule with at most N packets per wave
  python3 schedule_053.py --file other.json
"""
import json
import re
import sys
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
PACKETS = HERE.parents[1] / "dev-notes" / "leap-053-packets.json"
if not PACKETS.exists():  # the spec run's scratchpad copy sits beside this file
    PACKETS = HERE / "leap-053-packets.json"
CAP = 26

# Waves that have landed, by number; closeout.py --release 053 appends each one, and
# --verify then guards that re-running the scheduler reproduces history.
LANDED = {
    0: "E0.1 E0.2",
    1: "EA.1 EB.1 EB.2 EC.1 EG.1 EG.4 EG.5",
    2: "EA.2 EB.4 EG.2",
    3: "EA.3 EG.3",
    4: "EA.4 EB.3",
    5: "EA.5 EA.6 EB.5a EB.5c EC.2",
    6: "EA.7 EA.8 EB.5b EC.3",
    7: "EA.6b EB.6 EC.4",
    8: "EA.9 EB.7 EC.5",
    9: "EB.8 EC.8 EC.10",
    10: "EB.9 EC.6 EC.9",
    11: "EC.11 EG.6",
    12: "EC.12",
    13: "EC.13",
}
GROUP_ORDER = {"0": 0, "A": 1, "B": 2, "C": 3, "G": 4, "Z": 9}


def id_key(pid):
    m = re.fullmatch(r"E([0-9A-Z])\.(\d+)([a-z]?)", pid)
    if not m:
        raise SystemExit(f"bad packet id {pid!r}")
    return (GROUP_ORDER.get(m.group(1), 8), m.group(1), int(m.group(2)), m.group(3))


def load(path):
    doc = json.loads(Path(path).read_text())
    P = {p["id"]: p for p in doc["packets"]}
    serial = list(doc.get("wave0_serial", []))
    claims = {i: set(p["files_touched"]) for i, p in P.items()}
    deps = {i: set(p.get("depends_on") or []) for i, p in P.items()}
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


def schedule(P, claims, deps, serial, priority=False, cap=0):
    unblocks = Counter(d for ds in deps.values() for d in ds)
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
            ready.sort(key=id_key)
        used, wave = set(), []
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
    """Longest depends_on chain over the non-E0 packets (E0 is the serial wave 0)."""
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

    return max((depth(i) for i in P if i not in serial), key=lambda t: (t[0], [id_key(x) for x in t[1]]))


def waves(path=PACKETS):
    """(P, claims, waves) exactly as the runner schedules them."""
    P, claims, deps, serial = load(path)
    check_acyclic(deps)
    return P, claims, schedule(P, claims, deps, serial, cap=CAP)


def main(argv):
    path = PACKETS
    if "--file" in argv:
        path = Path(argv[argv.index("--file") + 1])
    P, claims, deps, serial = load(path)
    check_acyclic(deps)
    waves = schedule(P, claims, deps, serial, cap=CAP)
    if "--verify" in argv:
        bad = 0
        for n, want in LANDED.items():
            if waves[n] != want.split():
                bad += 1
                print(f"W{n} MISMATCH\n  landed:    {want}\n  scheduler: {' '.join(waves[n])}")
            else:
                print(f"W{n} ok ({len(waves[n])} packets)")
        print("VERIFIED" if not bad else f"{bad} waves differ")
        return 1 if bad else 0
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
