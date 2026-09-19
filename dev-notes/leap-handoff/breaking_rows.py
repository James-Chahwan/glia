#!/usr/bin/env python3
"""Every breaking block of the 0.5.0 leap that reaches one out-of-repo consumer, as Markdown.

    python3 dev-notes/leap-handoff/breaking_rows.py dev-notes/leap-packets.json \\
        dev-notes/wave-packets.json \\
        --corrections dev-notes/leap-corrections.json \\
        --consumer /home/ivy/Code/repo-graph --alias 'repo-graph wrapper' \\
        --results '~/.claude/projects/-home-ivy-Code-glia/*/subagents/workflows/*/journal.jsonl' \\
        --handoff-to LG.5a

Written for the repo-graph handoff (LG.5a); LG.5b runs it with --consumer
/home/ivy/Code/neuropil and LG.14 with --consumer /home/ivy/Code/Engram. Stdlib only.

Inputs
  SPEC.json...   a leap spec file (`packets` + `batch_c`) and the programme spec its Batch C
                 rows point into (`packets`). A file with `batch_c` selects which packets of a
                 file without it are loaded; with no `batch_c` anywhere every packet loads.
  --corrections  {"corrections": {id: text}}. Their `HANDOFF FROM <id> (wN): ...`
                 paragraphs that name --handoff-to, plus every paragraph of that packet's own
                 correction, are listed. Corrections are prose: they never change a row.
  --landed       leap_schedule.py; its LANDED dict gives each landed packet's wave.
  --results      workflow journal globs. A packet's result gives its status, its commit and
                 its LANDED `breaking` text, which outranks the spec: its first sentence sets
                 the kinds and its sentences naming the consumer become entries.

A packet is in scope when a spec out_of_repo_consumers entry contains the --consumer path,
or (with --results) a sentence of its landed `breaking` text contains the path or an
--alias. It is LANDED when it is in LANDED and, with --results, its result has a commit. In
scope and landed, it becomes a row: rows group packets whose spec entries are identical.
Kinds sort a row into one of two tables (API: api_signature, format; content: the rest); a
row with no kind is additive. A packet whose every entry says `none` / `no code change` is
counted in the footer instead, so the reader sees the check ran. Order is deterministic:
table, primary kind, first packet id.
"""

import argparse
import glob
import json
import os
import re
import sys
from pathlib import Path

KINDS = ["api_signature", "format", "qname_shape", "node_id", "edge_removal", "cell_value",
         "cli_output", "out_of_repo"]
API_KINDS = {"api_signature", "format"}
IDENTITY_KINDS = {"qname_shape", "node_id"}
NONE = r"(none|unchanged|no (code |wrapper )?change( required)?|needs? no (code )?change|nothing to change)"
NONE_HEAD = re.compile(rf"^\W*{NONE}\b[^;]*$", re.I)
NONE_TAIL = re.compile(rf"(\s[-\u2014]\s|:\s|\s){NONE}\W*$", re.I)
ACTION = re.compile(r"\b(must|replace[sd]?|delete|rewrite|switch|derive|drop|should|change to|"
                    r"pass [a-z_]+=|call [a-z_.]+\(|read \[|re-key)\b|->|\u2192", re.I)
HANDOFF = re.compile(r"^HANDOFF FROM (\S+) \((w\d+)\): ", re.S)


def pid_key(pid):
    m = re.match(r"([A-Z]+)(\d*)\.?(\d*)([a-z]*)", pid)
    b, n1, n2, s = m.groups() if m else (pid, "", "", "")
    return (b, int(n1 or 0), int(n2 or 0), s, pid)


def load_specs(paths):
    files = [json.loads(Path(p).read_text()) for p in paths]
    wanted = {r["packet"] for f in files for r in f.get("batch_c", [])}
    specs = {}
    for f in files:
        for p in f.get("packets", []):
            if "batch_c" in f or not wanted or p["id"] in wanted:
                specs[p["id"]] = p
    missing = sorted(wanted - set(specs), key=pid_key)
    if missing:
        print(f"[breaking-rows] warning: batch_c rows with no spec loaded: {' '.join(missing)}",
              file=sys.stderr)
    return specs


def load_landed(path):
    if not path:
        return None
    text = Path(path).read_text()
    block = re.search(r"^LANDED = \{(.*?)^\}", text, re.S | re.M)
    out = {}
    for wave, ids in re.findall(r"^\s*(\d+):\s*\"([^\"]*)\"", block.group(1) if block else "", re.M):
        for pid in ids.split():
            out[pid] = int(wave)
    return out


def load_results(globs):
    paths = sorted({p for g in globs for p in glob.glob(os.path.expanduser(g))},
                   key=lambda p: (os.path.getmtime(p), p))
    out = {}
    for path in paths:
        for line in open(path, encoding="utf-8"):
            try:
                d = json.loads(line)
            except ValueError:
                continue
            v = d.get("result") if d.get("type") == "result" else None
            if isinstance(v, str):
                try:
                    v = json.loads(v)
                except ValueError:
                    continue
            if isinstance(v, dict) and v.get("packet"):
                out[str(v["packet"]).split()[0]] = v
    return out


def spec_kinds(b):
    k = (b or {}).get("kind") or []
    k = [k] if isinstance(k, str) else k
    return [x for x in k if x in KINDS]


def landed_kinds(text):
    first = re.split(r"(?<=[.:])\s|\n", text.strip(), maxsplit=1)[0].split("(")[0]
    if re.match(r"\W*(none|no)\b", first, re.I):
        return []
    found = [k for k in KINDS if re.search(rf"\b{k}\b", first)]
    return found or None


def sentences(text):
    parts = (s.strip(" -") for s in re.split(r"(?<=[.;])\s+(?=[A-Z(/-])|\n+", text))
    return [s for s in parts if s and not s.endswith(":")]


BOILERPLATE = re.compile(r"undeclared|as declared|all declared|exactly as|declares|declared by|"
                         r"both declared|^\W*$", re.I)


def what_changed(text):
    """What a packet changes, in its own words: the first sentence after its kind list that
    is not boilerplate (`as declared`, `nothing undeclared shipped`)."""
    sents = sentences(text)
    head = re.match(r"^([^:]{0,80}?):\s+(.{20,})$", sents[0]) if sents else None
    if head and any(k in head.group(1) for k in KINDS):
        sents[0] = head.group(2)
    else:
        sents = sents[1:]
    for s in sents:
        s = re.sub(r"^(\(\d+\)|\d+[.)]|[-*])\s*", "", s)
        if not BOILERPLATE.search(s):
            return s
    return ""


def strip(entry, prefix):
    return re.sub(re.escape(prefix.rstrip("/")) + r"/?", "", entry).strip()


def is_none(entry, lenient):
    """The entry only says the consumer needs nothing. Strict (a packet that changes graph
    content or output, which the consumer sees even with no code change): it starts with
    `none`, or the text after its locator (`<file> - ` / `<file>: `) is only a no-change
    phrase. Lenient (a Rust-API / format-only or additive packet): a no-change phrase may
    also end the entry, or it says the consumer never links / parses the changed thing."""
    t = re.sub(r"\([^)]*\)", "", entry).strip()
    if ACTION.search(t):
        return False
    rest = re.split(r"\s[-\u2014]\s|:\s", t, maxsplit=1)[-1]
    if re.match(r"^\W*none\b", t, re.I) or NONE_HEAD.match(rest):
        return True
    return lenient and bool(NONE_TAIL.search(t) or re.search(r"\bnever (link|parse|read)", t, re.I))


def handoffs(corrections, target, needles):
    seen, rows = set(), []
    for pid in sorted(corrections, key=pid_key):
        for para in re.split(r"\n\s*\n", corrections[pid]):
            para = para.strip()
            m = HANDOFF.match(para)
            own = pid == target
            if not (own or (m and any(n in para for n in needles))):
                continue
            src, wave = (m.group(1), m.group(2)) if m else ("orchestrator", "-")
            body = para[m.end():] if m else para
            if (src, body) not in seen:
                seen.add((src, body))
                rows.append((src, wave, body))
    return rows


def cell(s, full):
    s = " ".join(s.split()).replace("|", "\\|")
    return s if full or len(s) <= 260 else s[:257] + "..."


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("specs", nargs="+")
    ap.add_argument("--corrections", action="append", default=[])
    ap.add_argument("--consumer", required=True, help="path prefix, e.g. /home/ivy/Code/repo-graph")
    ap.add_argument("--alias", action="append", default=[], help="text naming the consumer in results")
    ap.add_argument("--landed", default=str(Path(__file__).resolve().parents[1] / "wave-runner" / "leap_schedule.py"))
    ap.add_argument("--results", action="append", default=[])
    ap.add_argument("--handoff-to", default=None)
    ap.add_argument("--full", action="store_true", help="do not truncate table cells")
    a = ap.parse_args()

    specs = load_specs(a.specs)
    landed = load_landed(a.landed)
    results = load_results(a.results)
    needles = [a.consumer] + a.alias
    corrections = {}
    for c in a.corrections:
        corrections.update(json.loads(Path(c).read_text()).get("corrections", {}))

    packets, unnamed = [], []
    for pid in sorted(specs, key=pid_key):
        if pid == a.handoff_to:
            continue
        b = specs[pid].get("breaking") or {}
        spec_entries = [strip(e, a.consumer) for e in b.get("out_of_repo_consumers") or []
                        if a.consumer in e]
        res = results.get(pid)
        text = str((res or {}).get("breaking") or "")
        land_entries = [s for s in sentences(text) if any(n.lower() in s.lower() for n in needles)]
        lk = landed_kinds(text) if text else None
        kinds = lk if lk is not None else spec_kinds(b)
        wave = landed.get(pid) if landed is not None else None
        commit = str((res or {}).get("commit") or "")
        shipped = (landed is None or wave is not None) and (
            not a.results or (res is not None and commit not in ("", "none")))
        p = {"id": pid, "kinds": kinds, "spec_kinds": spec_kinds(b), "wave": wave,
             "commit": commit[:7], "status": (res or {}).get("status", ""),
             "spec": spec_entries, "landed": land_entries, "shipped": shipped,
             "breaking": bool(kinds),
             "what": what_changed(text) or " ".join(str(b.get("what_changes") or "").split()[:60])}
        if spec_entries or land_entries:
            packets.append(p)
        elif shipped and IDENTITY_KINDS & set(kinds):
            unnamed.append(p)

    tables = {"api": [], "content": [], "additive": []}
    not_landed, checked = [], []
    for p in packets:
        if not p["shipped"]:
            not_landed.append(p)
        elif all(is_none(e, lenient=not (set(p["kinds"]) - API_KINDS))
                 for e in p["spec"] + p["landed"]):
            checked.append(p)
        else:
            t = "additive" if not p["breaking"] else ("api" if API_KINDS & set(p["kinds"]) else "content")
            key = (tuple(p["spec"]) or (p["id"],))
            row = next((r for r in tables[t] if r["key"] == key), None)
            if row is None:
                tables[t].append({"key": key, "packets": [p]})
            else:
                row["packets"].append(p)

    def primary(row):
        ks = {k for p in row["packets"] for k in p["kinds"]}
        return next((i for i, k in enumerate(KINDS) if k in ks), len(KINDS))

    out = [f"# Breaking rows for `{a.consumer}`", "",
           f"specs: {', '.join(a.specs)}; corrections: {', '.join(a.corrections) or '-'}; "
           f"landed: {a.landed if landed is not None else '-'}; results: "
           f"{len(results)} packet result(s) from {len(a.results)} glob(s)", ""]
    titles = {"api": "API and on-disk changes (api_signature, format)",
              "content": "Graph content and output changes (qname_shape, node_id, edge_removal, "
                         "cell_value, cli_output, out_of_repo)",
              "additive": "Additive: names the consumer, declares no break"}
    for t in ("api", "content", "additive"):
        rows = sorted(tables[t], key=lambda r: (primary(r), pid_key(r["packets"][0]["id"])))
        out += [f"## {titles[t]} - {len(rows)} row(s)", "",
                "| # | kinds | packets (wave commit) | consumer entries |", "|---|---|---|---|"]
        for i, r in enumerate(rows, 1):
            kinds = sorted({k for p in r["packets"] for k in p["kinds"]}, key=KINDS.index)
            pk = ", ".join(f"{p['id']} (W{p['wave']} {p['commit']})" for p in r["packets"])
            ents = list(dict.fromkeys(r["packets"][0]["spec"]))
            ents += [f"[landed {p['id']}] {s}" for p in r["packets"] for s in p["landed"]]
            out.append(f"| {i} | {' '.join(kinds) or '-'} | {pk} | "
                       f"{'<br>'.join(cell(e, a.full) for e in ents)} |")
        out.append("")
    out += [f"## Identity changes that do not name the consumer - {len(unnamed)} packet(s)", "",
            "Every consumer renders qnames and holds node ids, so these reach it too.", "",
            "| packet (wave commit) | kinds | what changes |", "|---|---|---|"]
    out += [f"| {p['id']} (W{p['wave']} {p['commit']}) | {' '.join(p['kinds'])} | {cell(p['what'], a.full)} |"
            for p in unnamed]
    out.append("")
    out += [f"## Specced but not landed - {len(not_landed)} packet(s)", "",
            "| packet | status | kinds | consumer entries |", "|---|---|---|---|"]
    for p in not_landed:
        why = p["status"] or ("not in LANDED" if p["wave"] is None else "no commit")
        ents = p["spec"] + [f"[landed] {s}" for s in p["landed"]]
        out.append(f"| {p['id']} | {why} | {' '.join(p['kinds'] or p['spec_kinds']) or '-'} | "
                   f"{'<br>'.join(cell(e, a.full) for e in ents)} |")
    out.append("")
    if a.handoff_to:
        rows = handoffs(corrections, a.handoff_to, [a.handoff_to] + needles)
        out += [f"## Correction paragraphs addressed to {a.handoff_to} - {len(rows)}", "",
                "| from | wave | text |", "|---|---|---|"]
        out += [f"| {s} | {w} | {cell(t, a.full)} |" for s, w, t in rows]
        out.append("")
    out += [f"## Checked, no change - {len(checked)} packet(s)", "",
            ", ".join(p["id"] for p in checked) or "-", ""]
    n = {t: len(v) for t, v in tables.items()}
    out.append(f"[breaking-rows] consumer={a.consumer} api={n['api']} content={n['content']} "
               f"additive={n['additive']} unnamed_identity={len(unnamed)} not_landed={len(not_landed)} "
               f"checked_no_change={len(checked)} "
               f"in_scope={len(packets)}")
    print("\n".join(out))
    print(out[-1], file=sys.stderr)


if __name__ == "__main__":
    main()
