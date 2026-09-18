#!/usr/bin/env python3
"""Stamp a matrix-cell fixture skeleton from the machine-readable vocabulary.

    python3 scaffold.py <language> <mechanism> [--force]

16 x 30 = 480 cells is far past hand-authoring, and six corpus packets follow
this one. Every fixture needs four decisions the vocabulary ALREADY holds:

    how many dirs   `cross_repo` -- a cross-repo resolver only ever has a second
                    RepoId to pair against when the fixture ships two dirs
                    (grade.py calls generate_many() at 2+), so cross_repo True
                    means dirs ["client", "server"] and False means dirs ["."].
    which kinds     the FIRST `kinds` group -- the intended extraction path.
    which category  `categories[0]` -- the routing proof. An ANCHOR mechanism
                    (`anchor: True`, e.g. subproject) has no routing vocabulary:
                    it gets no expect_edges skeleton and a duplicate-anchor
                    forbid guard instead.
    which literal   `literal` -- the identifying string that must survive into
                    the node name/qname. Extraction without it is PARTIAL.

So the scaffolder reads matrix_vocab.py and turns each cell from a design task
into a fill-in-the-source task. It writes ONLY the frozen key.json vocabulary
(W0.4: framework, language, dirs, expect_nodes, expect_edges, expect_cells,
forbid, mechanism, cells, note) -- grade.py RAISES on any other top-level field,
so a scaffold that invented one would make every generated fixture vanish from
the matrix. In particular there is no `expect_literals`: a literal assertion IS
an `expect_nodes` entry whose `name` is the literal, because the identity
matcher is a case-folded substring over name OR qname.

See AUTHORING.md for the six-step per-cell recipe this skeleton feeds.
"""
import argparse
import json
import sys
from pathlib import Path

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from matrix_vocab import mechanism, normalize_language  # noqa: E402

MATRIX = HERE / "matrix"

# The template lives here as a constant, NOT as an on-disk matrix/_template/
# dir: run.py globs fixtures/* and a later matrix.py will glob matrix/*/*, and
# neither must ever discover a skeleton full of TODOs as if it were a fixture.
TEMPLATE = """{
  "framework":    "<lang>-<mech>",
  "language":     "<lang>",
  "dirs":         ["client", "server"],   // or ["."] when cross_repo is False
  "mechanism":    "<mech>",
  "cells":        ["<lang>/<mech>"],
  "expect_nodes": [{"kind": "<KIND>", "name": "TODO", "note": "<vocab literal>"}],
  "expect_edges": [{"from": "TODO", "to": "TODO", "category": "<CATEGORY>",
                    "note": "routing proof"}],
  "expect_cells": [],                     // OPTIONAL -- add from `--dump` reality
  "forbid":       [],
  "note":         "..."
}"""

# ext, line-comment prefix, mandatory file preamble
LANGS = {
    "python":     (".py",     "#",  ""),
    "go":         (".go",     "//", "package main\n\n"),
    "typescript": (".ts",     "//", ""),
    "java":       (".java",   "//", ""),
    "csharp":     (".cs",     "//", ""),
    "ruby":       (".rb",     "#",  ""),
    "php":        (".php",    "//", "<?php\n\n"),
    "swift":      (".swift",  "//", ""),
    "c_cpp":      (".cpp",    "//", ""),
    "scala":      (".scala",  "//", ""),
    "clojure":    (".clj",    ";;", ""),
    "dart":       (".dart",   "//", ""),
    "elixir":     (".ex",     "#",  ""),
    "rust":       (".rs",     "//", ""),
    "solidity":   (".sol",    "//", "// SPDX-License-Identifier: MIT\npragma solidity ^0.8.0;\n\n"),
    "terraform":  (".tf",     "#",  ""),
}

# family -> ((basename, role), (basename, role)). Index 0 lands in dirs[0].
STUBS = {
    "messaging": (("consumer", "consume side - subscribe/receive the topic"),
                  ("producer", "produce side - publish/send to the topic")),
    "http":      (("client", "caller side - the outbound request"),
                  ("server", "handler side - the route declaration")),
    "rpc":       (("client", "stub/caller side"),
                  ("server", "service implementation side")),
    "streaming": (("client", "connect side"), ("server", "handler side")),
    "cli":       (("invoker", "invocation side - shelling out to the command"),
                  ("definer", "definition side - where the command is declared")),
    "config":    (("reader", "read side - os.environ / config.get"),
                  ("definer", "define side - .env / compose / manifest")),
    "schedule":  (("worker", "the scheduled work"),
                  ("scheduler", "the schedule declaration")),
    "data":      (("query", "the access site - query / ORM call"),
                  ("model", "the entity / table declaration")),
    "intra":     (("main", "the referring site"),
                  ("lib", "the referred-to definition")),
    "topology":  (("client", "service A"), ("server", "service B")),
}
MECH_STUBS = {"tests": (("lib", "the unit under test"),
                        ("test_lib", "the test that exercises it"))}

# Secondary literals that do NOT live in the node identity, so they need a cell
# gate rather than an expect_nodes name. MEASURED 2026-09-16 (grade.py --dump on
# the committed fixtures): the ROUTE identity shape is NOT portable across
# languages -- Go/chi emits name "/users" qname "route:/users" with no method at
# all, Spring emits "GET /users/{id}", ASP.NET emits "ANY api/users" with the
# method present but NO leading slash. So a portable key asserts the PATH against
# name|qname via expect_nodes, and the METHOD separately via this cell, which was
# verified satisfiable against fixtures/java-spring-http.
EXTRA_CELLS = {
    "http_server": {"kind": "ROUTE", "cell": "ROUTE_METHOD",
                    "note": "the HTTP method, which the ROUTE name shape does not "
                            "carry portably across languages (go/chi omits it "
                            "entirely) - assert the PATH via expect_nodes"},
}


def build_key(lang, mech, dirs, stubs):
    """The frozen-vocabulary key.json for one cell, prefilled from the vocab."""
    kinds = mech["kinds"][0]          # the INTENDED extraction path
    # the routing proof; None for an ANCHOR mechanism, which has no routing
    # vocabulary by definition (matrix_vocab `anchor: True`)
    cat = mech["categories"][0] if mech["categories"] else None
    files = ", ".join(f for _, f, _ in stubs)
    if cat is None:
        expect_edges = []
        # The anchor's precision guard: one node per anchor, never duplicated.
        # Forbid matches EXACTLY, so 'TODO' must become the anchor's full qname.
        forbid = [
            {"kind": k, "name": "TODO", "max_nodes": 1,
             "note": "TODO: replace 'TODO' with the anchor's full qname (forbid "
                     "matches exactly) - one anchor per root, never duplicated"}
            for k in kinds
        ]
    else:
        expect_edges = [
            {"from": "TODO", "to": "TODO", "category": cat,
             "note": "routing proof"
                     + (f" - {mech['note']}" if mech["note"] else "")}
        ]
        forbid = []
    key = {
        "framework": f"{lang}-{mech['id']}",
        "language": lang,
        "dirs": dirs,
        "mechanism": mech["id"],
        "cells": [f"{lang}/{mech['id']}"],
        "expect_nodes": [
            {"kind": k, "name": "TODO",
             "note": f"TODO: replace 'TODO' with the identifying LITERAL - {mech['literal']}"}
            for k in kinds
        ],
        "expect_edges": expect_edges,
        # NO blanket POSITION gate. MEASURED 2026-09-16 against the installed
        # 0.4.18 wheel: the cross-cutting extractors mint their nodes WITHOUT a
        # span -- QUEUE_CONSUMER, QUEUE_PRODUCER, ROUTE and ENDPOINT all dump
        # `path=None`, while AST entities (MODULE/FUNCTION/CLASS) carry one.
        # Stamping POSITION per kind would hand every messaging/http/rpc cell a
        # gate it can NEVER satisfy, capping it at partial forever and teaching
        # six parallel authors that a permanently-red cell is normal.
        # expect_cells is optional: AUTHORING.md step 5 adds one only where
        # `grade.py --dump` shows the cell actually exists.
        "expect_cells": [],
        "forbid": forbid,
        "note": f"SCAFFOLDED, NOT AUTHORED (stubs: {files}). Follow AUTHORING.md: write the source, "
                "`grade.py <dir> --dump`, write the key against the INTENDED graph, "
                "record the failing baseline, add `expect_cells` for any cell "
                "`--dump` shows, then add `forbid` entries for every phantom the "
                "fixture provokes.",
    }
    extra = EXTRA_CELLS.get(mech["id"])
    if extra:
        key["expect_cells"].append({**extra, "node": "TODO"})
    return key


def build_stub(lang, mech, rel, role, comment, preamble):
    """One source stub: a banner, the role, the literal, and the two hard rules."""
    c = comment
    lines = [
        f"{c} FIXTURE: {lang}/{mech['id']}. Smallest real-library usage that a real",
        f"{c} repo would contain. Replace TODOs, then: python3 grade.py {rel} --dump",
        f"{c}",
        f"{c} ROLE: {role}.",
        f"{c} LITERAL that must survive into the node name/qname:",
        f"{c}   {mech['literal']}",
        f"{c}",
        f"{c} HARD RULES, both measured - worked examples in AUTHORING.md step 2:",
        f"{c}   (a) keep the library's REAL import line. Broad needles are gated",
        f"{c}       on a framework signal and here the import is the only place",
        f"{c}       that word appears, so dropping it can take the cell from",
        f"{c}       extracted to blind - a FALSE blind spot.",
        f"{c}   (b) keep the library's CANONICAL casing. The gate is case-",
        f"{c}       insensitive but the needles are not, so re-casing an API name",
        f"{c}       measures a language that does not exist.",
        f"{c}",
        f"{c} This banner is deliberately prose-only: the extractors scan TEXT,",
        f"{c} not an AST, so a call form written in a comment is extracted as if",
        f"{c} it were code and would contaminate this cell's baseline.",
        f"{c}",
        f"{c} TODO: write the smallest real usage here (<= 20 lines).",
    ]
    return preamble + "\n".join(lines) + "\n"


def scaffold(language, mech_id, force=False):
    lang = normalize_language(language)
    mech = mechanism(mech_id)
    if lang not in LANGS:  # pragma: no cover - LANGS covers all 16 rows
        raise ValueError(f"no file template for language {lang!r}")
    ext, comment, preamble = LANGS[lang]

    dirs = ["client", "server"] if mech["cross_repo"] else ["."]
    names = MECH_STUBS.get(mech["id"], STUBS[mech["family"]])
    # cross_repo: one stub per dir. single-dir: both stubs in the fixture root.
    stubs = [(dirs[i] if len(dirs) > 1 else ".", base + ext, role)
             for i, (base, role) in enumerate(names)]

    target = MATRIX / lang / mech["id"]
    # The logical cell path, not a filesystem-relative one: it is the marker and
    # the copy-pasteable grade.py argument, and must read the same either way.
    rel = f"matrix/{lang}/{mech['id']}"
    if target.exists() and not force:
        print(f"[matrix] scaffold REFUSED: {rel} already exists "
              f"(pass --force to overwrite its key.json and stubs)", file=sys.stderr)
        return None

    written = []
    for d, fname, role in stubs:
        path = target / d / fname if d != "." else target / fname
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(build_stub(lang, mech, rel, role, comment, preamble))
        written.append(path)
    key_path = target / "key.json"
    key_path.write_text(json.dumps(build_key(lang, mech, dirs, stubs), indent=2) + "\n")
    written.append(key_path)

    print(f"[matrix] scaffold: {rel} ({len(dirs)} dirs, {len(written)} files)",
          file=sys.stderr)
    for p in written:
        print(f"  {rel}/{p.relative_to(target).as_posix()}")
    print(f"next: edit the stubs, then `python3 grade.py {rel} --dump`")
    return target


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("language", nargs="?", help="one of the 16 matrix rows")
    ap.add_argument("mechanism", nargs="?", help="one of the 30 matrix columns")
    ap.add_argument("--force", action="store_true",
                    help="overwrite an existing cell dir (DESTROYS an authored key)")
    ap.add_argument("--template", action="store_true",
                    help="print the key.json shape this stamps and exit")
    args = ap.parse_args(argv)
    if args.template:
        print(TEMPLATE)
        return 0
    if not args.language or not args.mechanism:
        ap.error("language and mechanism are required (or pass --template)")
    try:
        return 0 if scaffold(args.language, args.mechanism, args.force) else 1
    except ValueError as exc:
        print(f"[matrix] scaffold FAILED: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
