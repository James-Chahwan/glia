#!/usr/bin/env python3
"""HP.1's tests for closeout.py's gate reuse: digests, input sets, the decision, the record, TMPDIR and the
ratchet / release-set / [gate-asks] rules. Stdlib unittest; each test builds its own throwaway git repo under
tempfile with a synthetic `cargo metadata` (no cargo, no glia). One test reads this checkout: every path literal
a tracked *.rs names must lie inside the workspace tests' input set.

    python3 -m unittest dev-notes/wave-runner/test_closeout.py
"""
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import closeout as C  # noqa: E402

GIT = ["git", "-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"]


def git(root, *args):
    r = subprocess.run(GIT + list(args), cwd=root, capture_output=True, text=True)
    if r.returncode:
        raise RuntimeError(r.stderr)
    return r.stdout.strip()


# name -> (dir, [(dep name, kind)]) ; kind None = normal, "dev", "build"
PACKAGES = {
    "glia-core": ("core", []),
    "glia-stamp": ("stamp", []),
    "glia-testkit": ("testkit", [("glia-toy-domain", "dev")]),
    "glia-engine": ("engine", [("glia-core", None), ("glia-stamp", "build"), ("glia-testkit", "dev"),
                               ("glia-parser-bazel", "dev")]),
    "glia-parser-bazel": ("parsers/build/bazel", []),
    "glia-cli": ("cli", [("glia-engine", None)]),
    "glia-py": ("py", [("glia-engine", None)]),
    "glia-toy-domain": ("toy-domain", []),
}

FILES = {
    "Cargo.toml": "[workspace]\n",
    "Cargo.lock": "# lock\n",
    "rustfmt.toml": "max_width = 120\n",
    "README.md": "# readme\n",
    "SECURITY.md": "security\n",
    "docs/overlay.md": "overlay v1\n",
    "dev-notes/x.md": "notes v1\n",
    "dev-notes/wave-runner/closeout.py": "# stand-in\n",
    "core/Cargo.toml": "", "core/src/lib.rs": "pub fn core() {}\n",
    "stamp/Cargo.toml": "", "stamp/src/lib.rs": "pub fn stamp() {}\n",
    "testkit/Cargo.toml": "", "testkit/src/lib.rs": "pub fn kit() {}\n",
    "engine/Cargo.toml": "",
    "engine/src/lib.rs": ('pub const O: &str = include_str!("../../docs/overlay.md");\n'
                          '#[cfg(test)]\nmod t {\n    const A: &str = include_str!(concat!(\n'
                          '        "../../bench/substrate-gap/fixtures/a/",\n        "x.go"\n    ));\n}\n'),
    "engine/tests/t.rs": 'const S: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../SECURITY.md"));\n',
    "parsers/build/bazel/Cargo.toml": "", "parsers/build/bazel/src/lib.rs": "\n",
    "cli/Cargo.toml": "", "cli/src/main.rs": "fn main() {}\n",
    "py/Cargo.toml": "", "py/src/lib.rs": "\n", "py/check_api_surface.py": "\n",
    "toy-domain/Cargo.toml": "", "toy-domain/src/lib.rs": "\n",
    "bench/substrate-gap/fixtures/a/x.go": "package a\n",
    "bench/substrate-gap/fixtures/b/key.json": "{}\n",
    "bench/substrate-gap/matrix_vocab.py": "X = 1\n",
    "bench/substrate-gap/grade.py": "import json\n",
    "bench/substrate-gap/results-latest.json": "{}\n",
    "bench/substrate-gap/COVERAGE.md": "cov\n",
    "bench/substrate-gap/legacy-latest.json": "{}\n",
    "bench/doc-link/cases.json": "[]\n",
    "bench/edge-audit/sample.json": "[]\n",
    "bench/lens/src/main.rs": "fn main() {}\n",
    "bench/message-contracts/check.sh": "true\n",
    "bench/cohort/cells.py": ('import sys\nfrom pathlib import Path\nHERE = Path(__file__).resolve().parent\n'
                              'sys.path.insert(0, str(HERE))\nsys.path.insert(0, str(HERE.parent / "substrate-gap"))\n'
                              'import cohort as C  # noqa: E402\nimport matrix_vocab as vocab  # noqa: E402\n'),
    "bench/cohort/cohort.py": "import json\n",
    "bench/cohort/tasks/a.jsonl": "{}\n",
    "bench/cohort/results/baseline/summary.json": '{"repos": 1}\n',
    "bench/cohort/results/0.5.4-pre/summary.json": "{}\n",
    "bench/sibyl/run.py": "import os\n",
    "bench/rank-gate/gate.json": json.dumps({"rankers": ["P1"], "items": ["x"], "accepted": []}) + "\n",
    "bench/rank-gate/snapshots.json": "[]\n",
    "bench/rank-gate/README.md": "rank gate\n",
    "bench/rank-gate/baseline/current/shell.json": '{"r10": 0.5}\n',
    "scripts/rank-gate.sh": "true\n",
    "scripts/cohort-gate.sh": "true\n",
}


def synthetic_meta(root):
    ids = {n: f"path+file://{root}/{d}#{n}@0.5.7" for n, (d, _) in PACKAGES.items()}
    packages = [{"id": ids[n], "name": n, "manifest_path": f"{root}/{d}/Cargo.toml"} for n, (d, _) in PACKAGES.items()]
    nodes = [{"id": ids[n], "deps": [{"name": dn, "pkg": ids[dn], "dep_kinds": [{"kind": k, "target": None}]}
                                     for dn, k in deps]}
             for n, (_, deps) in PACKAGES.items()]
    return {"packages": packages, "workspace_members": list(ids.values()), "resolve": {"nodes": nodes},
            "workspace_root": str(root)}


class Repo:
    """A throwaway git repo holding FILES, and an Inputs over its HEAD with fixed env probes."""

    def __init__(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name).resolve()
        self.probes = {cmd: f"{name} 1.0" for name, cmd in C.ENV_PROBES.items()}
        self.fe = self.root.parent / (self.root.name + "-fastembed")
        for p, text in FILES.items():
            self.write(p, text)
        git(self.root, "init", "-q")
        self.commit("init")

    def write(self, path, text):
        p = self.root / path
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_text(text)

    def commit(self, msg="c"):
        git(self.root, "add", "-A")
        git(self.root, "commit", "-q", "--allow-empty", "-m", msg)
        return git(self.root, "rev-parse", "HEAD")

    def change(self, path, text):
        self.write(path, text)
        return self.commit(f"change {path}")

    def inputs(self, installed=("so", "sha-so-1", "stamp-1")):
        return C.Inputs(root=self.root, meta=synthetic_meta(self.root), probe=lambda cmd: self.probes[cmd],
                        closeout_sha="closeout-1", fastembed_dir=self.fe, installed=installed)

    def close(self):
        self.tmp.cleanup()


def entry(d, tier="full", ok=True, env_fail=False, commit="a" * 40, **more):
    e = {"digest": d["digest"], "measure": d["measure"], "judge": d["judge"], "env": d["env"],
         "env_digest": d["env_digest"], "tier": tier, "ok": ok, "env_fail": env_fail, "commit": commit,
         "wave": 0, "line": "", "lines": [], "data": {}}
    e.update(more)
    return e


def record_with(*pairs):
    rec = {"schema": C.RECORD_SCHEMA, "gates": {}}
    for gate, e in pairs:
        C.record_result(rec, gate, e)
    return rec


class DigestTests(unittest.TestCase):
    def setUp(self):
        self.repo = Repo()

    def tearDown(self):
        self.repo.close()

    def test_included_path_moves_the_digest_excluded_does_not(self):
        before = self.repo.inputs().digest("tests")
        self.repo.change("dev-notes/x.md", "notes v2\n")
        self.assertEqual(self.repo.inputs().digest("tests")["digest"], before["digest"], "dev-notes/ is not a tests input")
        self.repo.change("bench/cohort/results/baseline/summary.json", '{"repos": 2}\n')
        self.assertEqual(self.repo.inputs().digest("tests")["digest"], before["digest"],
                         "the cohort baseline is not a tests input")
        self.repo.change("bench/substrate-gap/results-latest.json", '{"x": 1}\n')
        self.assertEqual(self.repo.inputs().digest("tests")["digest"], before["digest"], "a bench output is no input")
        self.repo.change("docs/overlay.md", "overlay v2\n")
        self.assertNotEqual(self.repo.inputs().digest("tests")["digest"], before["digest"])
        mid = self.repo.inputs().digest("tests")
        self.repo.change("bench/substrate-gap/fixtures/b/key.json", '{"k": 1}\n')
        self.assertNotEqual(self.repo.inputs().digest("tests")["digest"], mid["digest"], "fixtures are tests inputs")

    def test_cohort_baseline_moves_the_judge_only(self):
        before = self.repo.inputs().digest("cohort")
        self.repo.change("bench/cohort/results/baseline/summary.json", '{"repos": 2}\n')
        after = self.repo.inputs().digest("cohort")
        self.assertEqual(after["measure"], before["measure"])
        self.assertNotEqual(after["judge"], before["judge"])
        self.assertNotEqual(after["digest"], before["digest"])

    def test_gate_json_accepted_moves_rank_judge_only(self):
        before = self.repo.inputs().digest("rank")
        gj = json.loads(FILES["bench/rank-gate/gate.json"])
        gj["accepted"] = [{"row": "shell/single/PT*/mrr", "floor": 0.593}]
        self.repo.change("bench/rank-gate/gate.json", json.dumps(gj) + "\n")
        after = self.repo.inputs().digest("rank")
        self.assertEqual(after["measure"], before["measure"])
        self.assertNotEqual(after["judge"], before["judge"])
        gj["items"] = ["x", "y"]
        self.repo.change("bench/rank-gate/gate.json", json.dumps(gj) + "\n")
        self.assertNotEqual(self.repo.inputs().digest("rank")["measure"], after["measure"])

    def test_code_closure_from_synthetic_metadata(self):
        inp = self.repo.inputs()
        # normal + build deps transitively, the root's own dev-deps; never a dev-dep's dev-deps
        self.assertEqual(inp.closure("glia-engine"),
                         {"glia-engine", "glia-core", "glia-stamp", "glia-testkit", "glia-parser-bazel"})
        self.assertEqual(inp.closure("glia-cli"), {"glia-cli", "glia-engine", "glia-core", "glia-stamp"})
        code = inp.code(["glia-cli"])
        self.assertIn("cli/src/main.rs", code)
        self.assertIn("core/src/lib.rs", code)
        self.assertNotIn("testkit/src/lib.rs", code, "engine's dev-dep is not the CLI's")
        self.assertNotIn("toy-domain/src/lib.rs", code)
        self.assertTrue({"Cargo.toml", "Cargo.lock"} <= code)

    def test_include_targets_outside_a_crate_join_its_code_set(self):
        inp = self.repo.inputs()
        code = inp.code(["glia-engine"])
        self.assertIn("docs/overlay.md", code, "include_str! literal")
        self.assertIn("bench/substrate-gap/fixtures/a/x.go", code, "concat! prefix")
        self.assertIn("SECURITY.md", code, "CARGO_MANIFEST_DIR concat")
        self.assertNotIn("bench/substrate-gap/fixtures/b/key.json", code)
        self.assertNotIn("docs/overlay.md", inp.code(["glia-core"]))
        before = inp.digest("rank")
        self.repo.change("docs/overlay.md", "overlay v3\n")
        self.assertNotEqual(self.repo.inputs().digest("rank")["measure"], before["measure"])

    def test_gate_paths_sit_inside_their_gates_digest_sets(self):
        inp = self.repo.inputs()
        for gate in ("rank", "cohort"):
            measure, judge = inp.gate_sets(gate)
            for p in inp.tracked:
                if p.startswith(C.GATE_PATHS[gate]):
                    key = p + "#measure" if p == C.GATE_JSON else p
                    self.assertTrue(key in measure or p in judge, f"{gate}: {p}")
                    if not p.startswith(("bench/rank-gate/baseline/", "bench/cohort/results/baseline/")):
                        self.assertIn(key, measure, f"{gate} measure: {p}")
        self.assertIn("parsers/build/bazel/src/lib.rs", inp.gate_sets("cohort")[0], "a GATE_PATHS path off the closure")

    def test_cohort_reads_its_python_imports(self):
        measure, _ = self.repo.inputs().gate_sets("cohort")
        self.assertIn("bench/substrate-gap/matrix_vocab.py", measure)
        self.assertIn("bench/cohort/cohort.py", measure)
        self.assertNotIn("bench/substrate-gap/grade.py", measure)
        self.assertNotIn("bench/cohort/results/baseline/summary.json", measure)

    def test_bench_reads_the_installed_wheel(self):
        a = self.repo.inputs(installed=("so", "sha-so-1", "s")).digest("bench")
        b = self.repo.inputs(installed=("so", "sha-so-2", "s")).digest("bench")
        self.assertNotEqual(a["measure"], b["measure"])

    def test_env_lines_move_the_digest_and_name_what_moved(self):
        inp = self.repo.inputs()
        d = inp.digest("tests")
        rec = record_with(("tests", entry(d)))
        self.repo.probes[C.ENV_PROBES["rustc"]] = "rustc 2.0"
        d2 = self.repo.inputs().digest("tests")
        self.assertEqual(d2["measure"], d["measure"])
        self.assertEqual(C.decide("tests", "full", d2, rec)[2], "env moved: rustc")
        other = C.Inputs(root=self.repo.root, meta=synthetic_meta(self.repo.root), probe=lambda c: "rustc 1.0",
                         closeout_sha="closeout-2", fastembed_dir=self.repo.fe, installed=("", "", ""))
        self.assertIn("closeout", C.decide("fmt", "full", other.digest("fmt"),
                                           record_with(("fmt", entry(inp.digest("fmt")))))[2])

    def test_digest_is_stable(self):
        a = self.repo.inputs()
        b = self.repo.inputs()
        for g in C.RECORDED_GATES:
            self.assertEqual(a.digest(g)["digest"], b.digest(g)["digest"], g)


class DecisionTests(unittest.TestCase):
    D = {"digest": "d1", "measure": "m1", "judge": "j1", "env": {"closeout": "c"}, "env_digest": "e1"}

    def test_full_entry_serves_partial_need_not_the_reverse(self):
        need = C.partial_tier(C.RANK_FAST)
        rec = record_with(("rank", entry(self.D, tier="full")))
        self.assertEqual(C.decide("rank", need, self.D, rec)[0], "reuse")
        rec = record_with(("rank", entry(self.D, tier=need)))
        action, _, reason = C.decide("rank", "full", self.D, rec)
        self.assertEqual((action, reason), ("run", "need full, record partial"))
        self.assertEqual(C.decide("rank", C.partial_tier("shell,websocket"), self.D, rec)[0], "reuse", "subset")
        self.assertEqual(C.decide("rank", C.partial_tier("shell,twenty"), self.D, rec)[0], "run", "not a subset")

    def test_failed_or_env_fail_entry_is_never_reused(self):
        rec = record_with(("tests", entry(self.D, ok=False)))
        self.assertEqual(C.decide("tests", "full", self.D, rec)[::2], ("run", "last result FAILED"))
        rec = record_with(("cohort", entry(self.D, env_fail=True)))
        self.assertEqual(rec["gates"]["cohort"]["green"], [])
        self.assertEqual(C.decide("cohort", "full", self.D, rec)[0], "run")
        # a later failure at the same digest outranks an older green
        rec = record_with(("tests", entry(self.D)), ("tests", entry(self.D, ok=False)))
        self.assertEqual(C.decide("tests", "full", self.D, rec)[::2], ("run", "last result FAILED"))

    def test_fresh_all_forces_every_gate(self):
        fresh = C.fresh_gates("all")
        self.assertEqual(fresh, set(C.RECORDED_GATES))
        for g in C.RECORDED_GATES:
            rec = record_with((g, entry(self.D)))
            self.assertEqual(C.decide(g, "full", self.D, rec, fresh)[::2], ("run", "forced"))
        self.assertEqual(C.fresh_gates("", "rank, cohort"), {"rank", "cohort"})
        with self.assertRaises(ValueError):
            C.fresh_gates("nope")

    def test_release_need_with_a_partial_entry_runs(self):
        needs = C.gate_needs([], {"rank": False, "cohort": False}, set(), environ={})
        self.assertTrue(all(needs[g] == "full" for g in C.RECORDED_GATES))
        for g in ("rank", "cohort"):
            rec = record_with((g, entry(self.D, tier=C.partial_tier(C.COHORT_FAST if g == "cohort" else C.RANK_FAST))))
            self.assertEqual(C.decide(g, needs[g], self.D, rec)[0], "run")
        mid = C.gate_needs([["X.1"]], {"rank": False, "cohort": False}, {"engine/src/rank/mod.rs"}, environ={})
        self.assertEqual(mid["rank"], "full", "a RANK_CODE_PATHS touch needs the full rank gate")
        self.assertEqual(mid["cohort"], C.partial_tier(C.COHORT_FAST))

    def test_no_record_moved_inputs_dirty_and_extra(self):
        self.assertEqual(C.decide("tests", "full", self.D, record_with())[2], "no record")
        self.assertEqual(C.decide("tests", "full", None, record_with())[2], "no digest")
        rec = record_with(("tests", entry(self.D)))
        self.assertEqual(C.decide("tests", "full", self.D, rec, dirty=True)[2], "dirty tree")
        moved = dict(self.D, digest="d2", measure="m2")
        self.assertEqual(C.decide("tests", "full", moved, rec)[2], "inputs moved")
        rec = record_with(("wheel", entry(self.D, so_sha256="a", stamp="s")))
        extra = lambda e: None if (e.get("so_sha256"), e.get("stamp")) == ("b", "s") else "installed-wheel"
        self.assertEqual(C.decide("wheel", "full", self.D, rec, extra=extra)[2], "env moved: installed-wheel")

    def test_ratchet_is_not_due_for_a_reused_gate(self):
        self.assertFalse(C.ratchet_due({"how": "reused", "tier": "full", "ok": True}, True))
        self.assertTrue(C.ratchet_due({"how": "fresh", "tier": "full", "ok": True}, True))
        self.assertFalse(C.ratchet_due({"how": "fresh", "tier": C.partial_tier(C.RANK_FAST), "ok": True}, True))
        self.assertFalse(C.ratchet_due({"how": "fresh", "tier": "full", "ok": True}, False))
        self.assertFalse(C.ratchet_due(None, True))

    def test_gate_asks_line_carries_reused_from(self):
        line, _ = C.gate_attribution("rank", "[rank-gate] check: ok - 1176 rows", ["HP.1"], {"HP.1": {"dev-notes/x"}},
                                     reused_from="0123456789abcdef")
        self.assertEqual(line, "== [gate-asks] rank result=ok reused_from=0123456789ab asked_by=1 (HP.1) "
                               "touched_by=0 (-)")
        line, _ = C.gate_attribution("cohort", "", [], {})
        self.assertNotIn("reused_from", line)

    def test_release_set(self):
        st = {g: {"how": "fresh", "tier": "full", "ok": True, "commit": "c" * 40} for g in C.RECORDED_GATES}
        st["rank"] = {"how": "reused", "tier": "full", "ok": True, "commit": "abcdef0123456789"}
        line, per, missing = C.release_set(st, "f" * 40)
        self.assertEqual(line, f"[closeout] set tree={'f' * 12} fresh=7 reused=1 full=8/8")
        self.assertIn("   rank=reused abcdef012345", per)
        self.assertEqual(missing, [])
        st["cohort"] = {"how": "fresh", "tier": C.partial_tier(C.COHORT_FAST), "ok": True, "commit": "c"}
        del st["clippy"]
        line, _, missing = C.release_set(st, "f" * 40)
        self.assertTrue(line.endswith("full=6/8"))
        self.assertEqual(missing, ["clippy", "cohort"])

    def test_line_tier(self):
        self.assertEqual(C.line_tier("[rank-gate] check: ok - 5 rows (partial: only shell,websocket)", "full"),
                         "partial:shell,websocket")
        self.assertEqual(C.line_tier("[rank-gate] check: ok - 6 rows", "full"), "full")


class RecordTests(unittest.TestCase):
    def test_empty_missing_and_corrupt_records_are_empty(self):
        with tempfile.TemporaryDirectory() as t:
            p = Path(t) / "gate-record.json"
            self.assertEqual(C.load_record(p)["gates"], {})
            p.write_text("")
            self.assertEqual(C.load_record(p)["gates"], {})
            p.write_text("{not json")
            self.assertEqual(C.load_record(p)["gates"], {})

    def test_roundtrip_and_green_kept(self):
        d = {"digest": "d", "measure": "m", "judge": "j", "env": {}, "env_digest": "e"}
        with tempfile.TemporaryDirectory() as t:
            p = Path(t) / "sub" / "gate-record.json"
            rec = C.load_record(p)
            for i in range(C.GREEN_KEEP + 3):
                C.record_result(rec, "fmt", entry(d, commit=f"{i:040d}"))
            C.record_result(rec, "fmt", entry(d, ok=False))
            C.save_record(p, rec)
            back = C.load_record(p)
            self.assertEqual(len(back["gates"]["fmt"]["green"]), C.GREEN_KEEP)
            self.assertEqual(back["gates"]["fmt"]["green"][-1]["commit"], f"{C.GREEN_KEEP + 2:040d}")
            self.assertFalse(back["gates"]["fmt"]["last"]["ok"])
            self.assertEqual([x.name for x in p.parent.iterdir()], ["gate-record.json"], "no temp file left")

    def test_record_path_override(self):
        self.assertEqual(C.record_path({"GLIA_CLOSEOUT_RECORD": "/x/r.json"}), Path("/x/r.json"))
        self.assertEqual(C.record_path({}), Path.home() / ".cache/glia-closeout/gate-record.json")


class EnvironmentTests(unittest.TestCase):
    def test_tmpdir_resolution(self):
        home = "/home/someone"
        self.assertEqual(C.closeout_tmp({}, home), "/home/someone/.cache/glia-tmp")
        self.assertEqual(C.closeout_tmp({"TMPDIR": "/tmp/x"}, home), "/home/someone/.cache/glia-tmp")
        self.assertEqual(C.closeout_tmp({"TMPDIR": "/tmp"}, home), "/home/someone/.cache/glia-tmp")
        self.assertEqual(C.closeout_tmp({"TMPDIR": "/home/a/b"}, home), "/home/a/b")
        self.assertEqual(C.closeout_tmp({"TMPDIR": "/tmp/x", "GLIA_CLOSEOUT_TMP": "/home/c"}, home), "/home/c")

    def test_env_fail_signatures(self):
        self.assertEqual(C.env_fail_signature("ok", "write: Disk quota exceeded (os error 122)"), "Disk quota exceeded")
        self.assertEqual(C.env_fail_signature("Error: os error 28"), "os error 28")
        self.assertIsNone(C.env_fail_signature("os error 280", "fine"))
        with tempfile.TemporaryDirectory() as t:
            old = Path(t) / "a.log"
            old.write_text("No space left on device")
            os.utime(old, (1, 1))
            (Path(t) / "b.jsonl").write_text("{}")
            (Path(t) / "c.txt").write_text("No space left on device")
            self.assertEqual(C.recent_texts(t, 100), ["{}"])


class TestsSetSelfCheck(unittest.TestCase):
    """Every repo-path string literal in a non-comment line of a tracked *.rs (outside examples/, which the tests
    gate never runs) lies inside the tests gate's input set, so a test that reads a file cannot be reused stale.
    SYNTHETIC lists the literals that name no file a test reads (pure-function inputs, temp-repo paths, skip lists)."""
    LITERAL = re.compile(r'(?<!\\)"((?:\.\./)*(?:(?:bench|docs|skills|tests|\.glia|dev-notes)/[\w./*@+-]*|README\.md))"')
    SYNTHETIC = {
        ("engine/src/passes.rs", "bench/cohort/cohort.py"),       # doc_tree() over a qname, no file read
        ("engine/src/passes.rs", "bench/lens/src/main.rs"),       # the same, a doc-linker unit case
        ("engine/tests/intent_defs.rs", "dev-notes/plan.md"),     # written into the test's temp repo
        ("engine/tests/intent_ids.rs", "dev-notes/plan-0.5.3.md"),          # a glob matcher's input
        ("engine/tests/intent_ids.rs", "dev-notes/leap-053-packets.json"),  # the same
        ("engine/tests/self_scan.rs", "bench/"),                  # self_scan's SKIP list
        ("engine/tests/self_scan.rs", "dev-notes/"),              # the same
    }

    def test_rs_path_literals_lie_inside_the_tests_set(self):
        files = subprocess.run(["git", "ls-files", "*.rs"], cwd=C.ROOT, capture_output=True, text=True).stdout.split()
        self.assertTrue(files)
        outside = []
        for f in files:
            if "/examples/" in f:
                continue
            for n, line in enumerate((C.ROOT / f).read_text(errors="replace").splitlines(), 1):
                if line.strip().startswith(("//", "*", "/*")):
                    continue
                for m in self.LITERAL.finditer(line):
                    p = m.group(1)
                    while p.startswith("../"):
                        p = p[3:]
                    if not C.in_tests_set(p) and (f, p) not in self.SYNTHETIC:
                        outside.append(f"{f}:{n}: {p}")
        self.assertEqual(outside, [], "a test may read a path outside closeout.py's tests input set "
                                      "(widen in_tests_set, or add a SYNTHETIC row if no file is read)")

    def test_in_tests_set(self):
        self.assertTrue(C.in_tests_set("README.md"))
        self.assertTrue(C.in_tests_set("skills/glia-find/SKILL.md"))
        self.assertTrue(C.in_tests_set("bench/substrate-gap/fixtures/a/key.json"))
        self.assertTrue(C.in_tests_set("bench/edge-audit/x.json"))
        self.assertFalse(C.in_tests_set("bench/substrate-gap/COVERAGE.md"))
        self.assertFalse(C.in_tests_set("bench/rank-gate/gate.json"))
        self.assertFalse(C.in_tests_set("dev-notes/wave-runner/closeout.py"))


if __name__ == "__main__":
    unittest.main()
