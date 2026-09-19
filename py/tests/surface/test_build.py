#!/usr/bin/env python3
"""pyo3 surface, py/src/build.rs: the one build contract (LD.2).

Behaviour over the INSTALLED wheel, so it runs at wave close-out after the
wheel rebuild (dev-notes/wave-runner/closeout.py runs every
py/tests/surface/test_*.py). It complements py/tests/api_surface.rs, which pins
signatures from source.

This file also holds what the other surface tests share (`fixture_repo`,
`Checks`, `stderr_of`, `node_ids`): the build module owns the fixture repo, and
the others import it (`from test_build import ...`), which works because
python puts a script's own directory first on sys.path.

Plain python3, no pytest. The last line is `[surface] <module>: N checks, M
failed`; the exit code is non-zero on any failure.
"""
from __future__ import annotations

import inspect
import json
import os
import pathlib
import sys
import tempfile

# A build must write nothing whether or not persisting is allowed, so run
# with the opt-out UNSET: a write that only GLIA_NO_PERSIST used to hide shows.
os.environ.pop("GLIA_NO_PERSIST", None)

import repo_graph_py as rg  # noqa: E402

# `helper` / `main` for the answers, plus enough functions that some NodeId
# sits above 2**63 (the ids are deterministic: the repo dir is always
# `ld2app`, outside any git checkout, so its identity is `dir:ld2app`).
APP = (
    "import os\n\n\n"
    "def helper(x):\n    return x + 1\n\n\n"
    "def main():\n    return helper(2)\n"
    + "".join(f"\n\ndef step_{i}(v):\n    return helper(v) + {i}\n" for i in range(24))
)


def fixture_repo(tmp: str, name: str = "ld2app") -> str:
    """A one-file python repo under `tmp`; returns its path."""
    root = pathlib.Path(tmp) / name
    root.mkdir()
    (root / "app.py").write_text(APP)
    return str(root)


def tree(root: str) -> list[str]:
    """Every path under `root`, relative and sorted, directories included."""
    base = pathlib.Path(root)
    return sorted(str(p.relative_to(base)) for p in base.rglob("*"))


def node_ids(g) -> set[int]:
    return {n["id"] for n in json.loads(g.nodes_json())}


def stderr_of(fn):
    """(fn(), what the Rust side wrote to fd 2 meanwhile): eprintln! goes to
    the process's stderr fd, not to sys.stderr, so redirect the fd itself."""
    sys.stderr.flush()
    saved = os.dup(2)
    with tempfile.TemporaryFile(mode="w+b") as cap:
        os.dup2(cap.fileno(), 2)
        try:
            out = fn()
        finally:
            os.dup2(saved, 2)
            os.close(saved)
        cap.seek(0)
        return out, cap.read().decode(errors="replace")


def params(fn) -> list[tuple[str, object]]:
    """(name, default) per Python-visible parameter, `self` dropped."""
    return [
        (p.name, None if p.default is inspect.Parameter.empty else p.default)
        for p in inspect.signature(fn).parameters.values()
        if p.name != "self"
    ]


class Checks:
    def __init__(self, module: str):
        self.module, self.n, self.failed = module, 0, []

    def check(self, name: str, ok: bool, detail: object = "") -> None:
        self.n += 1
        if not ok:
            self.failed.append(name)
            print(f"FAIL {self.module}: {name}  {detail}")

    def raises(self, name: str, exc: type, fn, contains: str = "") -> None:
        try:
            fn()
        except exc as e:
            self.check(name, contains in str(e), f"message {e!r} lacks {contains!r}")
            return
        except Exception as e:  # noqa: BLE001 - reported, not swallowed
            self.check(name, False, f"raised {type(e).__name__}: {e}")
            return
        self.check(name, False, f"did not raise {exc.__name__}")

    def done(self) -> int:
        print(f"[surface] {self.module}: {self.n} checks, {len(self.failed)} failed")
        return 1 if self.failed else 0


def main() -> int:
    c = Checks("build")
    c.check("generate signature",
            rg.generate.__text_signature__ == "(repo_path, incremental=False, overlay=True)",
            rg.generate.__text_signature__)
    c.check("generate_many signature",
            rg.generate_many.__text_signature__ == "(repo_paths, incremental=False, overlay=True)",
            rg.generate_many.__text_signature__)
    c.check("purge_parse_cache exists", hasattr(rg, "purge_parse_cache"))

    with tempfile.TemporaryDirectory(prefix="glia-surface-build-") as tmp:
        repo = fixture_repo(tmp)
        before = tree(repo)
        g, err = stderr_of(lambda: rg.generate(repo))
        c.check("marker", "[build] surface=pyo3 repos=1 incremental=false" in err, err[-400:])
        c.check("generate() writes nothing into the repo", tree(repo) == before, tree(repo))
        c.check("no .ai / layout dir after generate()",
                not os.path.exists(os.path.join(repo, ".ai"))
                and not os.path.exists(rg.default_gmap_dir(repo)))
        many = rg.generate_many([repo])
        c.check("generate_many() writes nothing into the repo", tree(repo) == before, tree(repo))
        c.check("same graph either entry", many.node_count() == g.node_count() > 0,
                (many.node_count(), g.node_count()))

        cache = pathlib.Path(repo) / ".glia" / "graph" / "parse_cache.bin"
        _, err = stderr_of(lambda: rg.generate(repo, incremental=True))
        c.check("incremental marker", "[build] surface=pyo3 repos=1 incremental=true" in err, err[-400:])
        c.check("incremental=True writes the parse cache", cache.exists())
        c.check("incremental=True writes no layout",
                not (cache.parent / "manifest.json").exists(), tree(repo))
        rg.generate(repo, False)
        c.check("incremental=False no longer purges the cache", cache.exists())
        rg.purge_parse_cache(repo)
        c.check("purge_parse_cache deletes it", not cache.exists())
        rg.purge_parse_cache(repo)  # a missing cache is not an error
        c.check("purge twice is fine", not cache.exists())

        # LF.2b: overlay=False is the extraction-only build. It is marked,
        # save_to_default refuses it and save_to(dir) takes it; the default
        # build is overlay-applied and may go to the default dir.
        c.check("default build is overlay-applied", g.overlay_applied is True, g.overlay_applied)
        bare, err = stderr_of(lambda: rg.generate(repo, overlay=False))
        c.check("overlay=False marker",
                "[overlay] disabled (overlay=False): not persisting to the default gmap dir" in err,
                err[-400:])
        c.check("overlay=False graph is marked", bare.overlay_applied is False, bare.overlay_applied)
        c.check("overlay=False builds the same graph without an overlay file",
                bare.node_count() == g.node_count(), (bare.node_count(), g.node_count()))
        # Compare the tree around the refusal: the incremental steps above already
        # created .glia/graph/ (purge_parse_cache removes the file, not the dir).
        before_refusal = tree(repo)
        c.raises("save_to_default refuses an overlay=False graph", ValueError,
                 lambda: bare.save_to_default(repo), "overlay=False")
        c.check("the refusal wrote nothing", tree(repo) == before_refusal, tree(repo))
        out = os.path.join(tmp, "bare-layout")
        bare.save_to(out)
        c.check("save_to(dir) takes an overlay=False graph",
                os.path.exists(os.path.join(out, "manifest.json")))
        c.check("generate_many(overlay=False) is marked",
                rg.generate_many([repo], overlay=False).overlay_applied is False)

        c.raises("generate on a missing dir", ValueError,
                 lambda: rg.generate(os.path.join(tmp, "missing")))
        c.raises("generate_many on a missing dir", ValueError,
                 lambda: rg.generate_many([os.path.join(tmp, "missing")]))
    return c.done()


if __name__ == "__main__":
    sys.exit(main())
