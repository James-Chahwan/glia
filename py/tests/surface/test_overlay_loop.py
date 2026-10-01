#!/usr/bin/env python3
"""pyo3 surface, py/src/overlay_loop.rs (CK.1): the overlay loop as module
functions over `glia_engine::overlay_loop` (CE.3b-CE.3d), the steps of
`glia overlay propose|try|accept` (CE.3e) with the same answers.

- `overlay_propose(repo_paths, categories=None, top_k=20, snippet_lines=3)`
  -> {rows, counts, snippets, ambiguous_root, guide}; a row is the `gaps` row
  (id first) plus `snippet`. Writes nothing.
- `overlay_try(repo_paths, candidate, leave_one_out=True)` -> {stanzas, base,
  with, delta, verdict, closed, builds}. `candidate` is TOML text, not a path.
  Writes only the primary repo's parse cache.
- `overlay_accept(repo_path, candidate=None, only=None, remove=None,
  dry_run=False)` -> {added, removed, duplicates, file, dry_run, written,
  diff}. The only writer of `.glia/overlay.toml`.

A refusal raises ValueError with the engine's message; the markers end
`surface=py` and the candidate marker says `file=-`. The repo is the Go
sources of the substrate-gap fixture `go-overlay-data-wrapper` with
`NewCollection` taking a computed name (as cli/tests/overlay_loop_cli.rs and
the module's Rust test lay it out) and no overlay file, in a
TemporaryDirectory: never a real repo. Shared helpers: test_build.py."""
from __future__ import annotations

import pathlib
import sys
import tempfile

from test_build import Checks, rg, stderr_of

FIXTURE = (pathlib.Path(__file__).resolve().parents[3]
           / "bench" / "substrate-gap" / "fixtures" / "go-overlay-data-wrapper")
IMPORT_OLD = 'import (\n\t"go.mongodb.org/mongo-driver/mongo"\n)'
IMPORT_NEW = 'import (\n\t"strings"\n\n\t"go.mongodb.org/mongo-driver/mongo"\n)'
USELESS_EDGE = ('[[edge]]\nfrom = "nowhere::Caller"\nto = "nowhere::Callee"\n'
                'category = "CALLS"\n')
FIXTURE_OVERLAY = (FIXTURE / ".glia" / "overlay.toml").read_text()
# The fixture's `[[wrapper]]` table (`wrapper#1`), then an edge between two
# qnames that do not exist (`edge#1`): one stanza that adds, one that binds
# nothing.
CAND = FIXTURE_OVERLAY[FIXTURE_OVERLAY.index("[[wrapper]]"):] + "\n" + USELESS_EDGE
# Refused by the loader: the wrapper's `flavor` misspelt.
BAD = CAND.replace('flavor = "nosql"', 'flavour = "nosql"')
DEAD = [("dead_symbol", "chat_preview_repository::NewChatPreviewRepository"),
        ("dead_symbol", "collection::NewCollection"),
        ("dead_symbol", "collection::NewNamedCollection")]
CANDIDATE_MARKER = ("[overlay] candidate file=- stanzas=2 (route_prefix=0 wrapper=1 edge=1 "
                    "constants=0 entrypoints=0) gap_links=0 errors=0")


def go_repo(tmp: str) -> pathlib.Path:
    """`<tmp>/repo`: the fixture's two Go files, `NewCollection` taking
    `strings.ToLower(name)`; no `.glia/overlay.toml`."""
    collection = ((FIXTURE / "collection.go").read_text()
                  .replace(IMPORT_OLD, IMPORT_NEW)
                  .replace(".Collection(name)", ".Collection(strings.ToLower(name))"))
    if '"strings"' not in collection or "strings.ToLower(name)" not in collection:
        raise SystemExit("the fixture's NewCollection moved: rewrite go_repo")
    repo = pathlib.Path(tmp) / "repo"
    repo.mkdir()
    (repo / "collection.go").write_text(collection)
    (repo / "chat_preview_repository.go").write_text(
        (FIXTURE / "chat_preview_repository.go").read_text())
    return repo


def main() -> int:
    c = Checks("overlay_loop")
    c.check("overlay_propose signature",
            rg.overlay_propose.__text_signature__
            == "(repo_paths, categories=None, top_k=20, snippet_lines=3)",
            rg.overlay_propose.__text_signature__)
    c.check("overlay_try signature",
            rg.overlay_try.__text_signature__ == "(repo_paths, candidate, leave_one_out=True)",
            rg.overlay_try.__text_signature__)
    c.check("overlay_accept signature",
            rg.overlay_accept.__text_signature__
            == "(repo_path, candidate=None, only=None, remove=None, dry_run=False)",
            rg.overlay_accept.__text_signature__)

    with tempfile.TemporaryDirectory(prefix="glia-surface-overlay-loop-") as tmp:
        repo_path = go_repo(tmp)
        repo = str(repo_path)
        overlay = repo_path / ".glia" / "overlay.toml"

        # Propose: the work list, writes nothing.
        p, err = stderr_of(lambda: rg.overlay_propose([repo]))
        c.check("propose -> dict, keys in field order",
                type(p) is dict
                and list(p) == ["rows", "counts", "snippets", "ambiguous_root", "guide"],
                type(p) is dict and list(p))
        rows = p.get("rows", []) if type(p) is dict else []
        c.check("three dead_symbol rows",
                [(r.get("category"), r.get("qname")) for r in rows] == DEAD,
                [(r.get("category"), r.get("qname")) for r in rows])
        c.check("row keys in field order, snippet last",
                bool(rows) and list(rows[0]) == ["id", "category", "qname", "kind", "file",
                                                 "line", "detail", "suggest", "tier", "snippet"],
                rows[:1])
        coll = [r for r in rows if r.get("qname") == "collection::NewCollection"]
        snip = coll[0].get("snippet") if coll else None
        c.check("NewCollection snippet: its file, an int start_line",
                type(snip) is dict and snip.get("file") == "collection.go"
                and type(snip.get("start_line")) is int, snip)
        c.check("propose marker",
                f"[overlay] propose repo={repo} rows=3 snippets=3 ambiguous_root=0 surface=py"
                in err, err[-600:])

        # Try: base, with and one build per stanza; writes only the parse cache.
        t, err = stderr_of(lambda: rg.overlay_try([repo], CAND))
        c.check("try -> dict, keys in field order",
                type(t) is dict
                and list(t) == ["stanzas", "base", "with", "delta", "verdict", "closed", "builds"],
                type(t) is dict and list(t))
        stanzas = t.get("stanzas", []) if type(t) is dict else []
        c.check("verdicts wrapper#1 keep, edge#1 drop; 4 builds",
                [(s.get("stanza"), s.get("verdict")) for s in stanzas]
                == [("wrapper#1", "keep"), ("edge#1", "drop")]
                and t.get("builds") == 4, t)
        c.check("delta: one DATA_ENTITY, one ACCESSES_DATA, no gap moved",
                type(t) is dict and t.get("delta")
                == {"nodes": {"DATA_ENTITY": 1}, "edges": {"ACCESSES_DATA": 1}, "gaps": {}},
                type(t) is dict and t.get("delta"))
        c.check("candidate marker", CANDIDATE_MARKER in err, err[-600:])
        c.check("try marker",
                f"[overlay] try repo={repo} stanzas=2 builds=4 verdicts keep=1 review=0 drop=1 "
                "gaps 3→3 closed=0 surface=py" in err, err[-600:])
        two = rg.overlay_try([repo], CAND, leave_one_out=False)
        c.check("leave_one_out=False: two builds, no stanza rows",
                two.get("builds") == 2 and two.get("stanzas") == [], two)
        c.check("no overlay file after the tries", not overlay.exists())

        # Accept: the only writer of .glia/overlay.toml.
        dry = rg.overlay_accept(repo, CAND, only=["wrapper#1"], dry_run=True)
        c.check("dry run: the diff, nothing written",
                dry.get("dry_run") is True and dry.get("written") is False
                and "+[[wrapper]]" in dry.get("diff", "") and not overlay.exists(), dry)
        a, err = stderr_of(lambda: rg.overlay_accept(repo, candidate=CAND))
        c.check("accept both stanzas",
                a.get("added") == {"edge": 1, "wrapper": 1} and a.get("written") is True, a)
        c.check("accept marker",
                f"[overlay] accept repo={repo} added=2 (route_prefix=0 wrapper=1 edge=1 "
                "constants=0 entrypoints=0) removed=0 duplicates=0 file=.glia/overlay.toml "
                "dry_run=false surface=py" in err, err[-600:])

        # Prune rot: the edge binds nothing, so it is an orphaned rule.
        o = rg.overlay_propose([repo], categories=["orphaned_rule"])
        orows = o.get("rows", [])
        row = orows[0] if len(orows) == 1 else {}
        c.check("one orphaned_rule row: the edge",
                len(orows) == 1 and row.get("qname") == "nowhere::Caller"
                and row.get("suggest") == "remove" and row.get("file") == ".glia/overlay.toml",
                orows)
        r = rg.overlay_accept(repo, remove=[row.get("id", "gap:0000000000000000")])
        c.check("remove by gap id",
                r.get("removed") == 1 and r.get("added") == {} and r.get("written") is True, r)
        text = overlay.read_text() if overlay.exists() else ""
        c.check("the file keeps the wrapper, not the edge",
                text.startswith("version = 1\n")
                and '[[wrapper]]\ncall = "NewCollection"' in text and "[[edge]]" not in text,
                text)
        c.check("overlay_delta keeps the accepted file",
                rg.overlay_delta([repo]).get("verdict") == "keep")

        # Refusals: ValueError with the engine's message, nothing written.
        c.raises("accept with nothing", ValueError, lambda: rg.overlay_accept(repo),
                 "nothing to accept")
        c.raises("only without a candidate", ValueError,
                 lambda: rg.overlay_accept(repo, only=["wrapper#1"],
                                           remove=["gap:0000000000000000"]),
                 "no candidate was given")
        c.raises("only names an unknown stanza", ValueError,
                 lambda: rg.overlay_accept(repo, CAND, only=["wrapper#9"]), "wrapper#9")
        c.raises("try a refused candidate", ValueError, lambda: rg.overlay_try([repo], BAD),
                 "unknown field `flavour`")
        c.raises("accept a refused candidate", ValueError, lambda: rg.overlay_accept(repo, BAD),
                 "nothing written")
        c.raises("propose with no repo", ValueError, lambda: rg.overlay_propose([]),
                 "no repo paths")
        c.raises("propose an unknown category", ValueError,
                 lambda: rg.overlay_propose([repo], categories=["nope"]),
                 "unknown gaps category")
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
