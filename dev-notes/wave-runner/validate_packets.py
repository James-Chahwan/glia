#!/usr/bin/env python3
"""Validate a spec run's packet file before it is merged into a packets JSON.

The 0.5.0 spec runs used a validator that lived in a session scratchpad and was
lost with it; this one is committed. It checks shape, not truth: the adversarial
verifier checks truth.

Usage:
  python3 validate_packets.py packets <file.json> <PREFIX>   # e.g. CA
  python3 validate_packets.py packets <file.json> <PREFIX> --release 0.5.1

Prints `OK <n> packets, <loc> LOC` and exits 0, or one line per problem and exits 1.
Warnings (a files_touched path that does not exist and is not declared new) print
as `WARN` and do not fail.

From 0.5.4 (`--release 0.5.4` or later) every packet carries a `probe` (James 2026-10-30,
"i believed we maybe didn't explore"): the spec writer's own run at HEAD before the spec, so
the spec rests on what the tree does, not on a reading of it. Either
  {"head": "<sha>", "commands": ["<cmd>", ..], "output": "<verbatim>", "finding": "<what it
   showed that shaped this spec>"}
or, for a packet with nothing to run (docs, a handoff), {"none": "<why nothing can be run>"},
which prints a WARN. `head` must be a commit this repo holds.
"""
import subprocess
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

REQUIRED = {
    "id": str, "title": str, "batch": str, "area": str, "loc": int,
    "rationale": str, "entry_points": list, "files_touched": list,
    "implementation": str, "acceptance": str, "fired_on_marker": str,
    "breaking": dict, "depends_on": list, "risks": str,
}
OPTIONAL = {"fixtures": list, "spec_group": str, "research": str, "probe": dict}
PROBE_FROM = (0, 5, 4)
BREAK_KINDS = {
    "none", "qname_shape", "node_id", "edge_removal", "cell_value",
    "api_signature", "format", "cli_output", "out_of_repo",
}
BREAK_FIELDS = {"is_breaking", "kind", "what_changes", "in_repo_consumers", "out_of_repo_consumers"}
# bench/substrate-gap/grade.py TOP_FIELDS (frozen key.json vocabulary)
KEY_FIELDS = {
    "framework", "language", "dirs", "expect_nodes", "expect_edges", "expect_cells",
    "forbid", "materialize", "mechanism", "cells", "note",
}
FIXTURE_FIELDS = {"name", "purpose", "source_sketch", "key_json"}
# Facades: only the wave-0 slot packet (<PREFIX>0.*) may touch them.
FACADES = {"engine/src/lib.rs", "graph/src/lib.rs"}
LOC_MIN, LOC_MAX = 10, 600


def check_probe(p, where, errs, warns):
    """The `probe` rule (module docs): a run at a real head, verbatim output, and what it showed."""
    pr = p.get("probe")
    if not isinstance(pr, dict):
        errs.append(f"{where} missing `probe` (run it at HEAD before writing the spec)")
        return
    if set(pr) == {"none"}:
        if not isinstance(pr["none"], str) or len(pr["none"].strip()) < 40:
            errs.append(f"{where} probe.none needs a reason (>= 40 chars)")
        else:
            warns.append(f"{where} no probe: {pr['none'].strip()[:80]}")
        return
    extra = set(pr) - {"head", "commands", "output", "finding"}
    if extra:
        errs.append(f"{where} probe has unknown fields {sorted(extra)}")
    head = pr.get("head")
    if not (isinstance(head, str) and re.fullmatch(r"[0-9a-f]{7,40}", head)):
        errs.append(f"{where} probe.head must be a commit sha")
    elif subprocess.run(["git", "-C", str(ROOT), "cat-file", "-e", f"{head}^{{commit}}"],
                        capture_output=True).returncode:
        errs.append(f"{where} probe.head {head} is no commit in this repo")
    cmds = pr.get("commands")
    if not (isinstance(cmds, list) and cmds and all(isinstance(c, str) and c.strip() for c in cmds)):
        errs.append(f"{where} probe.commands must list the commands run")
    out = pr.get("output")
    if not isinstance(out, str) or len(out.strip()) < 80:
        errs.append(f"{where} probe.output must be the verbatim output (>= 80 chars)")
    fnd = pr.get("finding")
    if not isinstance(fnd, str) or len(fnd.strip()) < 40:
        errs.append(f"{where} probe.finding must say what the run showed (>= 40 chars)")


def check_packet(p, prefix, errs, warns, probe=False):
    pid = p.get("id", "<no id>")
    where = f"{pid}:"
    if probe:
        check_probe(p, where, errs, warns)
    for k, t in REQUIRED.items():
        if k not in p:
            errs.append(f"{where} missing field `{k}`")
        elif not isinstance(p[k], t) or (t is int and isinstance(p[k], bool)):
            errs.append(f"{where} `{k}` must be {t.__name__}")
    for k in p:
        if k not in REQUIRED and k not in OPTIONAL:
            errs.append(f"{where} unknown field `{k}`")
    if not isinstance(p.get("id"), str):
        return
    wave0 = re.fullmatch(rf"{re.escape(prefix[0])}0\.\d+[a-z]?", pid) is not None
    if not wave0 and not re.fullmatch(rf"{re.escape(prefix)}\.\d+[a-z]?", pid):
        errs.append(f"{where} id must look like {prefix}.<n>[letter]")
    if not wave0 and p.get("batch") != prefix:
        errs.append(f"{where} batch must be `{prefix}`")
    loc = p.get("loc")
    if isinstance(loc, int) and not (LOC_MIN <= loc <= LOC_MAX):
        errs.append(f"{where} loc {loc} outside {LOC_MIN}..{LOC_MAX} (split it)")
    for s in ("rationale", "implementation", "acceptance", "risks", "title"):
        if isinstance(p.get(s), str) and len(p[s].strip()) < 20:
            errs.append(f"{where} `{s}` is too thin")
    for s in ("rationale", "implementation", "acceptance"):
        v = p.get(s, "")
        if isinstance(v, str) and re.search(r"\bTODO\b|\bTBD\b|\bstub\b", v):
            errs.append(f"{where} `{s}` contains a TODO / TBD / stub")
    m = p.get("fired_on_marker", "")
    if isinstance(m, str) and not re.search(r"\[[a-z0-9_-]+\]", m):
        errs.append(f"{where} fired_on_marker needs a stable `[prefix]`")
    if isinstance(p.get("entry_points"), list) and not p["entry_points"]:
        errs.append(f"{where} entry_points is empty")
    ft = p.get("files_touched")
    if isinstance(ft, list):
        if not ft:
            errs.append(f"{where} files_touched is empty")
        new_hint = json.dumps(p.get("entry_points", [])) + p.get("implementation", "")
        for f in ft:
            if not isinstance(f, str):
                errs.append(f"{where} files_touched entry is not a string")
                continue
            if f.startswith("/") and not f.startswith("/home/ivy/Code/"):
                errs.append(f"{where} files_touched `{f}` is outside the repos")
            if f in FACADES and not wave0:
                errs.append(f"{where} touches facade `{f}` (only the wave-0 slot packet may)")
            rel = f[len(str(ROOT)) + 1:] if f.startswith(str(ROOT)) else f
            if not f.startswith("/") and not (ROOT / rel).exists():
                if f not in new_hint and "(new)" not in new_hint:
                    warns.append(f"{where} files_touched `{f}` does not exist and is not declared new")
    b = p.get("breaking")
    if isinstance(b, dict):
        missing = BREAK_FIELDS - set(b)
        if missing:
            errs.append(f"{where} breaking lacks {sorted(missing)}")
        kinds = b.get("kind")
        kinds = kinds if isinstance(kinds, list) else [kinds]
        for k in kinds:
            if k not in BREAK_KINDS:
                errs.append(f"{where} breaking.kind `{k}` not in {sorted(BREAK_KINDS)}")
        if b.get("is_breaking") is True and kinds == ["none"]:
            errs.append(f"{where} is_breaking=true but kind=none")
        if b.get("is_breaking") is False and any(k != "none" for k in kinds):
            errs.append(f"{where} is_breaking=false but kind={kinds}")
    for fx in p.get("fixtures", []) or []:
        if not isinstance(fx, dict):
            errs.append(f"{where} fixture is not an object")
            continue
        extra = set(fx) - FIXTURE_FIELDS
        if extra:
            errs.append(f"{where} fixture `{fx.get('name')}` unknown fields {sorted(extra)}")
        kj = fx.get("key_json")
        if isinstance(kj, str):
            try:
                kj = json.loads(kj)
            except ValueError:
                errs.append(f"{where} fixture `{fx.get('name')}` key_json is not JSON")
                continue
        if isinstance(kj, dict):
            bad = set(kj) - KEY_FIELDS
            if bad:
                errs.append(f"{where} fixture `{fx.get('name')}` key_json has non-vocabulary fields {sorted(bad)}")


def main(argv):
    if len(argv) < 4 or argv[1] != "packets":
        print(__doc__)
        return 2
    path, prefix = Path(argv[2]), argv[3]
    probe = False
    if "--release" in argv:
        rel = argv[argv.index("--release") + 1] if argv.index("--release") + 1 < len(argv) else ""
        try:
            probe = tuple(int(x) for x in rel.split(".")) >= PROBE_FROM
        except ValueError:
            print(f"--release takes a version like 0.5.4, not {rel!r}")
            return 2
    try:
        doc = json.loads(path.read_text())
    except (OSError, ValueError) as e:
        print(f"cannot read {path}: {e}")
        return 1
    packets = doc.get("packets") if isinstance(doc, dict) else None
    if not isinstance(packets, list) or not packets:
        print('file must be {"packets": [...]} with at least one packet')
        return 1
    errs, warns = [], []
    seen = set()
    for p in packets:
        if not isinstance(p, dict):
            errs.append("a packet is not an object")
            continue
        if p.get("id") in seen:
            errs.append(f"{p.get('id')}: duplicate id")
        seen.add(p.get("id"))
        check_packet(p, prefix, errs, warns, probe)
    ids = {p.get("id") for p in packets if isinstance(p, dict)}
    for p in packets:
        if isinstance(p, dict) and p.get("id") in (p.get("depends_on") or []):
            errs.append(f"{p['id']}: depends on itself")
    for w in warns:
        print("WARN", w)
    for e in errs:
        print(e)
    if errs:
        return 1
    loc = sum(p.get("loc", 0) for p in packets if isinstance(p, dict))
    print(f"OK {len(ids)} packets, {loc} LOC")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
