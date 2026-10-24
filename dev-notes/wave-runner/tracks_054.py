#!/usr/bin/env python3
"""0.5.4 build tracks: the 169 packets cut into chunks James can build one at a time.

Each track is a set of packets; its prerequisites are the other tracks its packets depend on.
T0 (wave 0: registry ids, crates, slots, rustfmt) runs before any track.

  python3 tracks_054.py            # the track table, cross-track deps, LOC
  python3 tracks_054.py --track T5 # that track's own wave schedule (assumes its prerequisite tracks landed)
  python3 tracks_054.py --json     # {track: [packet ids]} for gen_wave.py --only
  python3 tracks_054.py --select T2 T5   # add tracks to schedule_054.SELECTED, print the waves
                                         # gen_wave / closeout --release 054 then build
"""
import json, re, sys
from pathlib import Path
from collections import defaultdict
HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import schedule_054 as S

def ids(spec):
    out = []
    for part in spec.split():
        m = re.fullmatch(r"(F[A-Z0-9])\.(\d+)-(\d+)", part)
        if m:
            out += [f"{m.group(1)}.{n}" for n in range(int(m.group(2)), int(m.group(3)) + 1)]
        else:
            out.append(part)
    return out

TRACKS = {
    "T0": ("wave 0: ids, crates, slots, rustfmt", "F0.1-5"),
    "T1": ("proof: agent before / after (item 11)", "FE.1-4 FE.8"),
    "T2": ("self-graph hygiene, tier vocabulary, rules / docs hygiene, the audit tool", "FD.2 FD.4a FD.4b FD.4c FD.4d FD.5a FD.5b FD.6 FD.7a FD.7b FD.7c FD.7d FD.22 FD.23a FD.23b FD.24 FD.25 FD.27 FD.28 FD.34 FD.65 FD.69"),
    "T3": ("edge-precision audit, judging + report (item 12)", "FD.3a FD.3b FD.3c"),
    "T4": ("per-language precision and recall residuals", "FD.8 FD.9 FD.10a FD.10b FD.11-21 FD.29-32 FD.35-64 FD.68 FD.70 FD.72 FD.33 FD.71"),
    "T5": ("embeddings + the embedding gate (item 13) + Engram data", "FB.1 FB.2 FB.3 FB.4 FB.5a FB.5b FB.5c FB.6 FB.7a FB.7b FB.7c FB.8 FB.11 FB.12a FB.12b FB.13a FB.13b FB.13c FB.13d"),
    "T6": ("goal-layer connector + clarification queue (§G, §C)", "FC.1 FC.2 FC.3a FC.3b FC.4 FC.5a FC.5b FC.6-14 FC.17a FC.17b FC.18 FC.23 FC.24"),
    "T7": ("concept search, PR / issue intent, placement (items 6, 14, 15)", "FC.15 FC.16 FC.19 FC.20 FC.21 FC.22 FB.9"),
    "T8": ("incremental resolution, watch fast path, multi-repo watch, memory budget", "FA.1-14 FA.24 FB.10 FD.26 FD.66 FD.67"),
    "T9": ("build-target graphs (bazel, first non-code domain)", "FA.15-20"),
    "T10": ("org-internal package routing", "FA.21 FA.22 FA.23"),
    "T11": ("distribution: zero-install CLI, skills into glia", "FE.5 FE.6 FE.7"),
    "TZ": ("release docs and handoffs (after the tracks that ship)", "FZ.1 FZ.2"),
}

def track_of(P):
    """{packet id: track}; refuses a packet in no track, in two, or a track naming no packet."""
    of = {}
    for t, (_, spec) in TRACKS.items():
        for i in ids(spec):
            if i not in P: raise SystemExit(f"{t}: no packet {i}")
            if i in of: raise SystemExit(f"{i} in {of[i]} and {t}")
            of[i] = t
    missing = sorted(set(P) - set(of), key=S.id_key)
    if missing: raise SystemExit(f"unassigned: {missing}")
    return of


def select(add):
    """Append tracks to schedule_054.SELECTED (in file), then print the resulting waves."""
    bad = [t for t in add if t not in TRACKS]
    if bad: raise SystemExit(f"no such track: {bad}")
    cur = S.SELECTED.split()
    new = cur + [t for t in add if t not in cur]
    path = HERE / "schedule_054.py"
    src = path.read_text()
    m = re.search(r'^SELECTED = "([^"]*)"$', src, re.M)
    if not m: raise SystemExit("schedule_054.py has no SELECTED line")
    S.SELECTED = " ".join(new)
    S.waves()  # refuses a selection that needs an unselected track, before anything is written
    path.write_text(src[:m.start()] + f'SELECTED = "{S.SELECTED}"' + src[m.end():])
    print(f"SELECTED = {S.SELECTED}")
    return S.main([])


def main(argv):
    if "--select" in argv:
        return select(argv[argv.index("--select") + 1:])
    P, claims, deps, serial = S.load(S.PACKETS)
    of = track_of(P)
    if "--json" in argv:
        print(json.dumps({t: sorted([i for i in P if of[i] == t], key=S.id_key) for t in TRACKS}, indent=1)); return 0
    pre = defaultdict(lambda: defaultdict(set))
    for i, ds in deps.items():
        for d in ds:
            if of[d] != of[i] and of[d] != "T0":
                pre[of[i]][of[d]].add(f"{i}<-{d}")
    if "--track" in argv:
        t = argv[argv.index("--track") + 1]
        sub = {i for i in P if of[i] == t}
        Pt = {i: P[i] for i in sub}
        dt = {i: {d for d in deps[i] if d in sub} for i in sub}
        ser = [i for i in serial if i in sub]
        print(f"{t}: {TRACKS[t][0]}  prerequisite tracks: {', '.join(sorted(pre[t])) or 'T0 only'}")
        for n, w in enumerate(S.schedule(Pt, {i: claims[i] for i in sub}, dt, ser)):
            if w: print(f"  w{n:<2} {sum(P[i]['loc'] for i in w):>5} LOC  {' '.join(w)}")
        return 0
    print(f"{'track':5} {'pkts':>4} {'LOC':>6}  prerequisites (besides T0)  — what")
    for t, (what, _) in TRACKS.items():
        mem = [i for i in P if of[i] == t]
        print(f"{t:5} {len(mem):>4} {sum(P[i]['loc'] for i in mem):>6}  {', '.join(f'{k}({len(v)})' for k, v in sorted(pre[t].items())) or '-':28} — {what}")
    print("\ncross-track edges (packet <- dependency):")
    for t in TRACKS:
        for k, v in sorted(pre[t].items()):
            print(f"  {t} needs {k}: {' '.join(sorted(v))}")
    return 0

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
