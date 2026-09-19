#!/usr/bin/env python3
"""The 0.5.0 crate rename (LD.11a): every library package repo-graph-<x> -> glia-<x>
and every Rust path repo_graph_<x> -> glia_<x>, driven by the explicit table below.

    python3 dev-notes/rename-0.5.0.py --check            list every would-be change; exit 1 if any
    python3 dev-notes/rename-0.5.0.py --apply            rewrite the files in place
    python3 dev-notes/rename-0.5.0.py --check --root DIR  the same over another checkout
                                                         (neuropil's session replays it there)

Never a blanket regex. A token is renamed only when the WHOLE token is a table row:
`repo-graph-<x>` (hyphen form: package names, `cargo -p`, prose) or `repo_graph_<x>`
(underscore form: Rust paths), with no identifier character on either side, so
`render_repo_graph_full` and `repo_graph_roundtrips_through_gmap_file` stay, and a
hyphen-continued token that is not a row (`repo-graph-core-x`) is left whole and reported.

Never touched here:
  - `repo_graph_py` (py's [lib] name, the #[pymodule] fn, `import repo_graph_py`) and the
    PyPI dist `repo-graph-py` (pyproject, pip, the wheel CI) - the Python rename is LD.11b.
    Only py's CARGO package moves: its `name = "repo-graph-py"` line in a Cargo.toml and a
    `-p repo-graph-py` cargo package spec (a command naming the old package would fail).
  - `repo-graph` alone (the MCP product), `mcp-repo-graph`, `.ai/repo-graph` / `.glia/graph`
    (the on-disk dirs; LC.9 owns them).
  - Cargo.lock files: cargo rewrites the path entries on the next resolve, and nothing here
    may regenerate a lock from scratch (registry versions would drift).
  - dev-notes history (plans, handoffs, packet JSON, glia-memory snapshots) - records, not
    tooling. dev-notes/wave-runner IS rewritten (the next waves run it), and this script
    skips itself (it spells every old name).
  - bench/substrate-gap's committed artefacts (results-latest.json, legacy-latest.json,
    COVERAGE.md) - regenerated at end of wave, never edited.

Refuses (exit 2) when a Cargo.toml under the root declares a `repo-graph-*` package that is
not a table row: a crate added after this table was written must be added to it first.

Prose the table cannot express (onboarding's "the prefix is locked" lines, the
`repo-graph-{core,..}` brace list, CLAUDE.md's roadmap) was rewritten by hand in the same
commit; this script is the mechanical part.
"""

import argparse
import os
import re
import subprocess
import sys
from pathlib import Path

# (old package, new package). The Rust path is the package with '-' -> '_'.
PACKAGES = [
    ("repo-graph-core", "glia-core"),
    ("repo-graph-code-domain", "glia-code-domain"),
    ("repo-graph-graph", "glia-graph"),
    ("repo-graph-engine", "glia-engine"),
    ("repo-graph-store", "glia-store"),
    ("repo-graph-activation", "glia-activation"),
    ("repo-graph-projection-text", "glia-projection-text"),
    ("repo-graph-stamp", "glia-stamp"),
    ("repo-graph-doc", "glia-doc"),
    ("repo-graph-doc-sources", "glia-doc-sources"),
    ("repo-graph-code-extractors", "glia-code-extractors"),
    ("repo-graph-engram-export", "glia-engram-export"),
    ("repo-graph-latent", "glia-latent"),
    ("repo-graph-toy-domain", "glia-toy-domain"),
    ("repo-graph-parser-python", "glia-parser-python"),
    ("repo-graph-parser-go", "glia-parser-go"),
    ("repo-graph-parser-typescript", "glia-parser-typescript"),
    ("repo-graph-parser-rust", "glia-parser-rust"),
    ("repo-graph-parser-java", "glia-parser-java"),
    ("repo-graph-parser-kotlin", "glia-parser-kotlin"),
    ("repo-graph-parser-csharp", "glia-parser-csharp"),
    ("repo-graph-parser-ruby", "glia-parser-ruby"),
    ("repo-graph-parser-php", "glia-parser-php"),
    ("repo-graph-parser-swift", "glia-parser-swift"),
    ("repo-graph-parser-c-cpp", "glia-parser-c-cpp"),
    ("repo-graph-parser-scala", "glia-parser-scala"),
    ("repo-graph-parser-clojure", "glia-parser-clojure"),
    ("repo-graph-parser-dart", "glia-parser-dart"),
    ("repo-graph-parser-elixir", "glia-parser-elixir"),
    ("repo-graph-parser-solidity", "glia-parser-solidity"),
    ("repo-graph-parser-terraform", "glia-parser-terraform"),
    ("repo-graph-parser-react", "glia-parser-react"),
    ("repo-graph-parser-angular", "glia-parser-angular"),
    ("repo-graph-parser-vue", "glia-parser-vue"),
]

# Underscore-form rows that are not a package: a stale doc-comment name for `repo-graph-doc`
# (extractors' contracts.rs / grpc.rs cite `repo_graph_docs::position_json`).
EXTRA_PATHS = [("repo_graph_docs", "glia_doc")]

# py's cargo package: renamed here (LD.11b finds it glia-py); its Python names are LD.11b's.
PY_PACKAGE = ("repo-graph-py", "glia-py")

# Kept by design; everything else that still reads repo[-_]graph[-_]<ident> after a rewrite is
# reported as unknown so a reviewer sees it.
KEPT = {"repo_graph_py", "repo-graph-py", "repo_graph_roundtrips_through_gmap_file"}

# Relative to a glia root. A non-glia --root (neuropil) has none of these, so all of it is in scope.
SKIP_PREFIXES = ("dev-notes/",)
KEEP_PREFIXES = ("dev-notes/wave-runner/",)
SKIP_FILES = {
    "dev-notes/rename-0.5.0.py",
    "dev-notes/wave-runner/usage-log.jsonl",
    "bench/substrate-gap/results-latest.json",
    "bench/substrate-gap/legacy-latest.json",
    "bench/substrate-gap/COVERAGE.md",
}

ID = r"A-Za-z0-9_"


def _rows():
    """(regex, replacement, label) for every rename, one alternation per form, longest first."""
    hy = sorted(PACKAGES, key=lambda r: -len(r[0]))
    us = sorted([(o.replace("-", "_"), n.replace("-", "_")) for o, n in PACKAGES] + EXTRA_PATHS,
                key=lambda r: -len(r[0]))
    return {
        # Hyphen form: no identifier char or '-' before (so `mcp-repo-graph-...` is never read as
        # a crate), and no identifier char or '-<alnum>' after (a longer unknown token is not
        # half-renamed).
        "hyphen": (re.compile(rf"(?<![{ID}-])({'|'.join(re.escape(o) for o, _ in hy)})"
                              rf"(?![{ID}]|-[A-Za-z0-9])"), dict(hy)),
        "underscore": (re.compile(rf"(?<![{ID}])({'|'.join(re.escape(o) for o, _ in us)})"
                                  rf"(?![{ID}])"), dict(us)),
    }


ROWS = _rows()
# `cargo ... -p repo-graph-py`, also wrapped onto the next line of a `///`, `//!` or `#` comment.
PY_SPEC = re.compile(r"(?<![A-Za-z0-9_-])(-p\s+(?:(?://[/!]?|#)[ \t]*)?)repo-graph-py(?![A-Za-z0-9_-])")
PY_NAME = re.compile(r'^(name\s*=\s*")repo-graph-py(")', re.M)
WHEEL = re.compile(r"repo_graph_py-\d")
LEFTOVER = re.compile(r"(?<![A-Za-z0-9_-])repo[-_]graph[-_][A-Za-z0-9_-]*[A-Za-z0-9]")
PKG_DECL = re.compile(r'^\[package\][^\[]*?^name\s*=\s*"([^"]+)"', re.M | re.S)


def files(root):
    """Tracked files when root is a git checkout (untracked build output never matters),
    otherwise every file outside target/ and .git/."""
    try:
        out = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True,
                             check=True).stdout.decode()
        rels = [p for p in out.split("\0") if p]
    except (subprocess.CalledProcessError, FileNotFoundError):
        rels = []
        for d, dirs, fs in os.walk(root):
            dirs[:] = [x for x in dirs if x not in ("target", ".git")]
            rels += [os.path.relpath(os.path.join(d, f), root) for f in fs]
    return sorted(rels)


def in_scope(rel):
    if rel in SKIP_FILES or os.path.basename(rel) == "Cargo.lock":
        return False
    if rel.startswith(SKIP_PREFIXES) and not rel.startswith(KEEP_PREFIXES):
        return False
    return True


def read_text(path):
    try:
        raw = path.read_bytes()
    except OSError:
        return None
    if b"\0" in raw:
        return None
    try:
        return raw.decode("utf-8")
    except UnicodeDecodeError:
        return None


def rewrite(rel, text):
    """New text plus one (line, old, new) per rename."""
    hits = []

    def line_of(pos):
        return text.count("\n", 0, pos) + 1

    spans = []   # (start, end, new) over the ORIGINAL text; the forms never overlap
    for rx, table in ROWS.values():
        for m in rx.finditer(text):
            spans.append((m.start(1), m.end(1), table[m.group(1)]))
    for m in PY_SPEC.finditer(text):
        s = m.start() + len(m.group(1))
        spans.append((s, s + len(PY_PACKAGE[0]), PY_PACKAGE[1]))
    if os.path.basename(rel) == "Cargo.toml":
        for m in PY_NAME.finditer(text):
            s = m.start() + len(m.group(1))
            spans.append((s, s + len(PY_PACKAGE[0]), PY_PACKAGE[1]))
    spans.sort()
    out, last = [], 0
    for s, e, new in spans:
        if s < last:
            continue   # a py-spec match inside a span already taken (cannot happen; guard)
        out.append(text[last:s])
        out.append(new)
        hits.append((line_of(s), text[s:e], new))
        last = e
    out.append(text[last:])
    return "".join(out), hits


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    mode = ap.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--apply", action="store_true")
    ap.add_argument("--root", default=str(Path(__file__).resolve().parent.parent))
    ap.add_argument("--quiet", action="store_true", help="print only the per-file counts and the summary")
    a = ap.parse_args()
    root = Path(a.root).resolve()

    olds = {o for o, _ in PACKAGES} | {PY_PACKAGE[0]}
    unknown_pkgs = []
    for rel in files(root):
        if os.path.basename(rel) == "Cargo.toml":
            text = read_text(root / rel) or ""
            m = PKG_DECL.search(text)
            if m and m.group(1).startswith("repo-graph-") and m.group(1) not in olds:
                unknown_pkgs.append(f"{rel}: {m.group(1)}")
    if unknown_pkgs:
        for u in unknown_pkgs:
            print(f"[rename-0.5.0] REFUSE package not in the table: {u}")
        print("[rename-0.5.0] add each to PACKAGES first; nothing was changed")
        return 2

    changed_files, total, leftovers = 0, 0, []
    for rel in files(root):
        if not in_scope(rel):
            continue
        path = root / rel
        text = read_text(path)
        if text is None or ("repo_graph" not in text and "repo-graph" not in text):
            continue
        new, hits = rewrite(rel, text)
        for m in LEFTOVER.finditer(new):
            # A wheel file name (`repo_graph_py-0.4.18-cp311-...whl`) is the Python dist: kept.
            if m.group(0) not in KEPT and not WHEEL.match(m.group(0)):
                leftovers.append(f"{rel}:{new.count(chr(10), 0, m.start()) + 1}: {m.group(0)}")
        if not hits:
            continue
        changed_files += 1
        total += len(hits)
        print(f"{rel}: {len(hits)}")
        if not a.quiet:
            for ln, old, nw in hits:
                print(f"    {rel}:{ln}: {old} -> {nw}")
        if a.apply:
            path.write_text(new, encoding="utf-8")
    for l in leftovers:
        print(f"[rename-0.5.0] note - not a table row, left as is: {l}")
    verb = "rewrote" if a.apply else "would rewrite"
    print(f"[rename-0.5.0] {verb} {total} token(s) in {changed_files} file(s) under {root}"
          f" ({len(leftovers)} unknown token(s) left)")
    return 1 if (a.check and total) else 0


if __name__ == "__main__":
    sys.exit(main())
