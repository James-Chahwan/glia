#!/usr/bin/env python3
"""Hermetic tests for score.py — `python3 test_score.py` (pytest collects it too),
exit 0 on green. A temp GLIA_AGENT_AB_DIR holds a canned run (meta.json,
sessions.jsonl, patches) over two canned task rows; every metric below is
computed by hand from the patch text.
"""
import contextlib
import io
import json
import os
import sys
import tempfile
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import run  # noqa: E402
import score  # noqa: E402

TASK_A = {"id": "alpha-aaaaaaaaaaaa", "repo": "alpha", "commit": "a" * 40, "parent": "b" * 40,
          "after_gate_rev": None, "prompt_subject": "make the thing work again",
          "truth": {"code": [
              {"path": "src/a.py", "status": "M", "old_path": None, "hunks": [[10, 2], [40, 0]]},
              {"path": "src/b.py", "status": "M", "old_path": None, "hunks": [[5, 1]]}],
              "tests": ["tests/test_a.py"], "other": []}}
TASK_R = {"id": "beta-cccccccccccc", "repo": "beta", "commit": "c" * 40, "parent": "d" * 40,
          "after_gate_rev": True, "prompt_subject": "move the old module to its new name",
          "truth": {"code": [
              {"path": "src/m.go", "status": "M", "old_path": None, "hunks": [[1, 1]]},
              {"path": "src/new.go", "status": "R", "old_path": "src/old.go",
               "hunks": [[3, 1]]}], "tests": [], "other": []}}
REPOS = {"repos": [
    {"label": "alpha", "url": "https://github.com/example/alpha.git", "end_rev": "e" * 40,
     "lang": "python", "gate_rev": None, "tuned": False, "public": True},
    {"label": "beta", "url": "https://github.com/example/beta.git", "end_rev": "f" * 40,
     "lang": "go", "gate_rev": "0" * 40, "tuned": True, "public": True}]}

SOLVED = """diff --git a/src/a.py b/src/a.py
--- a/src/a.py
+++ b/src/a.py
@@ -11 +11 @@ def f():
-x
+y
@@ -38,0 +39,2 @@ def g():
+a
+b
diff --git a/src/b.py b/src/b.py
--- a/src/b.py
+++ b/src/b.py
@@ -100 +100 @@
-p
+q
diff --git a/tests/test_a.py b/tests/test_a.py
--- a/tests/test_a.py
+++ b/tests/test_a.py
@@ -1 +1 @@
-t
+u
"""
PARTIAL = """diff --git a/src/a.py b/src/a.py
--- a/src/a.py
+++ b/src/a.py
@@ -60 +60 @@
-x
+y
diff --git a/src/c.py b/src/c.py
--- a/src/c.py
+++ b/src/c.py
@@ -1 +1 @@
-x
+y
diff --git a/src/d.py b/src/d.py
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/src/d.py
@@ -0,0 +1 @@
+x
diff --git a/src/e.py b/src/e.py
--- a/src/e.py
+++ b/src/e.py
@@ -1 +1 @@
-x
+y
diff --git a/.claude/x.py b/.claude/x.py
--- a/.claude/x.py
+++ b/.claude/x.py
@@ -1 +1 @@
-x
+y
"""
RENAME = """diff --git a/src/m.go b/src/m.go
--- a/src/m.go
+++ b/src/m.go
@@ -2,0 +3 @@
+x
diff --git a/src/old.go b/src/new.go
similarity index 90%
rename from src/old.go
rename to src/new.go
--- a/src/old.go
+++ b/src/new.go
@@ -3 +3 @@
-x
+y
"""


def usage(i, c, r, o):
    return {"input_tokens": i, "cache_creation_input_tokens": c, "cache_read_input_tokens": r,
            "output_tokens": o}


def session(task, arm, rep, terminated="end", opened=(), usd=1.0, turns=5, wall_ms=2000,
            use=(100, 10, 1000, 50), glia=()):
    return {"run_id": "r1", "task_id": task["id"], "repo": task["repo"], "arm": arm, "rep": rep,
            "terminated": terminated, "files_opened": list(opened), "total_cost_usd": usd,
            "num_turns": turns, "wall_ms": wall_ms, "usage": usage(*use),
            "glia_calls": [{"subcommand": s, "flags": []} for s in glia]}


# (session row, patch text) of the canned run.
PLAN = [
    (session(TASK_A, "none", 1, opened=["README.md", "src/x.py", "src/a.py"]), SOLVED),
    (session(TASK_A, "none", 2), ""),
    (session(TASK_A, "g052", 1, usd=0.5, glia=("find", "blast-radius")), PARTIAL),
    (session(TASK_A, "g052", 2, terminated="contaminated"), SOLVED),
    (session(TASK_A, "g053", 1, terminated="timeout", glia=("find",)), SOLVED),
    (session(TASK_A, "g053", 2, terminated="network"), SOLVED),
    (session(TASK_R, "none", 1, opened=["README.md", "src/old.go"]), RENAME),
    (session(TASK_R, "g052", 1, terminated="prep_failed"), None),
    (session(TASK_R, "g053", 1, terminated="error"), ""),
    (session(TASK_R, "g053", 2, terminated="max_turns", usd=2.0), ""),
]


@contextlib.contextmanager
def canned_run():
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        d, tdir = tmp / "dir", tmp / "tasks"
        tdir.mkdir()
        (tmp / "repos.json").write_text(json.dumps(REPOS))
        (tdir / "alpha.jsonl").write_text(json.dumps(TASK_A, sort_keys=True) + "\n")
        (tdir / "beta.jsonl").write_text(json.dumps(TASK_R, sort_keys=True) + "\n")
        _, shas = run.load_tasks(tmp / "repos.json", tdir)
        run.write_json(d / "runs" / "r1" / "meta.json", {
            "config": "c" * 64, "prompt": "p" * 64, "tasks": shas,
            "claude_version": "2.1.310 (Claude Code)", "model": "claude-opus-5-5",
            "arms": {"g052": {"sha": "1" * 40, "version": "glia 0.5.2"},
                     "g053": {"sha": "2" * 40, "version": "glia 0.5.3"}}})
        lines = []
        for row, patch in PLAN:
            row = dict(row)
            if patch is not None:
                p = d / "patches" / "r1" / f"{row['task_id']}.{row['arm']}.{row['rep']}.diff"
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text(patch)
                row["patch"] = {"path": str(p), "sha256": run.sha256(patch.encode()),
                                "bytes": len(patch)}
            lines.append(json.dumps(row, sort_keys=True))
        (d / "runs" / "r1" / "sessions.jsonl").write_text("\n".join(lines) + "\n")
        old = os.environ.get("GLIA_AGENT_AB_DIR")
        os.environ["GLIA_AGENT_AB_DIR"] = str(d)
        try:
            yield tmp, ["--repos", str(tmp / "repos.json"), "--tasks-dir", str(tdir)]
        finally:
            if old is None:
                os.environ.pop("GLIA_AGENT_AB_DIR", None)
            else:
                os.environ["GLIA_AGENT_AB_DIR"] = old


def cli(args):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = score.main(args)
    return rc, out.getvalue(), err.getvalue()


def metrics_of(rows, task, arm, rep):
    return next(r["metrics"] for r in rows
                if (r["task_id"], r["arm"], r["rep"]) == (task["id"], arm, rep))


# --- tests -------------------------------------------------------------------
def test_parse_patch():
    files = score.parse_patch(PARTIAL + RENAME)
    assert files["src/d.py"] == {"status": "A", "old": None, "hunks": [[0, 0]]}
    assert files["src/a.py"]["hunks"] == [[60, 1]]
    assert files["src/new.go"] == {"status": "R", "old": "src/old.go", "hunks": [[3, 1]]}
    assert files["src/m.go"]["hunks"] == [[2, 0]]
    gone = score.parse_patch("diff --git a/src/z.py b/src/z.py\ndeleted file mode 100644\n"
                             "--- a/src/z.py\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-a\n-b\n-c\n")
    assert gone == {"src/z.py": {"status": "D", "old": "src/z.py", "hunks": [[1, 3]]}}


def test_session_metrics_by_hand():
    with canned_run() as (_, base):
        head, rows = score.metrics_from_run("r1", Path(os.environ["GLIA_AGENT_AB_DIR"]),
                                            base[1], base[3])
    # solved: E = {a, b} = T; a's [10,12) meets 11, a's insertion [40,41) meets 38 widened
    # to [35,42); b's [5,6) misses 100 widened to [97,104): overlap 2 / 3.
    m = metrics_of(rows, TASK_A, "none", 1)
    assert m["solved"] == 1.0 and m["file_recall"] == 1.0 and m["file_precision"] == 1.0
    assert abs(m["line_overlap"] - 2 / 3) < 1e-12
    assert m["first_truth_open"] == 3 and m["opened"] == 3
    assert m["tokens_in"] == 1110 and m["tokens_out"] == 50 and m["wall_s"] == 2.0
    # partial: E = {a, c, d, e} (.claude/ never counts), T = {a, b}.
    m = metrics_of(rows, TASK_A, "g052", 1)
    assert (m["solved"], m["file_recall"], m["file_precision"]) == (0.0, 0.5, 0.25)
    assert m["line_overlap"] == 0.0 and m["first_truth_open"] is None and m["glia_calls"] == 2
    # empty patch: nothing edited.
    m = metrics_of(rows, TASK_A, "none", 2)
    assert (m["solved"], m["file_recall"], m["file_precision"], m["line_overlap"]) == \
        (0.0, 0.0, 0.0, 0.0)
    # a timeout is scored as it stands.
    assert metrics_of(rows, TASK_A, "g053", 1)["solved"] == 1.0
    # rename: T = {src/m.go, src/new.go}; the agent opened the old name (rank 2); the M
    # file's [1,2) meets the insertion after 2 widened to [-1,6); the R hunk is not counted.
    m = metrics_of(rows, TASK_R, "none", 1)
    assert m["solved"] == 1.0 and m["file_recall"] == 1.0 and m["line_overlap"] == 1.0
    assert m["first_truth_open"] == 2
    # a max_turns session is scored; prep_failed / error / contaminated / network are not.
    assert metrics_of(rows, TASK_R, "g053", 2)["usd"] == 2.0
    excluded = sorted(r["terminated"] for r in rows if r["metrics"] is None)
    assert excluded == ["contaminated", "error", "network", "prep_failed"], excluded
    assert head["tasks"] == {"alpha.jsonl": head["tasks"]["alpha.jsonl"],
                             "beta.jsonl": head["tasks"]["beta.jsonl"]}


def test_score_marker_and_determinism():
    with canned_run() as (tmp, base):
        out = [str(tmp / n) for n in ("x.md", "x.json", "s.jsonl", "x2.md", "x2.json", "y.json")]
        rc, so, _ = cli(base + ["score", "r1", "--out-md", out[0], "--out-json", out[1],
                                "--out-sessions", out[2]])
        assert rc == 0 and so.strip() == "[agent-ab] score sessions=6 excluded=4 tasks=2", so
        rc, _, _ = cli(base + ["score", "r1", "--out-md", out[3], "--out-json", out[4]])
        assert rc == 0
        assert Path(out[0]).read_bytes() == Path(out[3]).read_bytes()
        assert Path(out[1]).read_bytes() == Path(out[4]).read_bytes()
        # The metrics file alone reproduces the baseline, byte for byte.
        os.environ["GLIA_AGENT_AB_DIR"] = str(tmp / "nowhere")
        rc, so, err = cli(["score", "--from-metrics", out[2], "--out-json", out[5]])
        assert rc == 0, err
        assert Path(out[1]).read_bytes() == Path(out[5]).read_bytes()
        text = Path(out[2]).read_text()
        assert "diff --git" not in text and str(tmp) not in text and "files_opened" not in text
        rows = [json.loads(line) for line in text.splitlines()]
        keys = [(r["task_id"], r["arm"], r["rep"]) for r in rows[1:]]
        assert rows[0]["kind"] == "meta" and keys == sorted(keys)
        b = json.loads(Path(out[1]).read_text())
        assert b["schema"] == 1 and b["fingerprints"]["prompt_sha256"] == "p" * 64
        assert set(b["pairs"]) == {"g052-none", "g053-none", "g053-g052"}
        # none on alpha: mean of solved (1) and empty (0) = 0.5; g052: partial only.
        none_a = next(r for r in b["per_task"] if r["task_id"] == TASK_A["id"]
                      and r["arm"] == "none")
        assert none_a["n"] == 2 and none_a["metrics"]["solved"] == 0.5
        assert b["pairs"]["g052-none"]["solved"]["diff"] == -0.5
        md = Path(out[0]).read_text()
        for needle in ("## Reading rule", "## pooled", "## untuned", "## Cost", "## glia usage",
                       "sessions excluded: contaminated 1, error 1, network 1, prep_failed 1",
                       "g053-g052 untuned:", "- g052: 1/1 sessions (1.000) made >= 1 glia call; `blast-radius` 1, `find` 1"):
            assert needle in md, needle


def test_bootstrap_and_verdicts():
    diffs = [0.1, -0.2, 0.5, 0.0, 0.3, 0.2]
    assert score.bootstrap(diffs) == score.bootstrap(list(diffs))
    d, lo, hi = score.bootstrap(diffs)
    assert lo <= d <= hi
    assert score.verdict("solved", 0.1, 0.3, 5) == "better"
    assert score.verdict("solved", -0.3, -0.1, 5) == "worse"
    assert score.verdict("usd", 0.1, 0.3, 5) == "worse"
    assert score.verdict("tokens_in", -0.3, -0.1, 5) == "better"
    assert score.verdict("first_truth_open", -3.0, -1.0, 5) == "better"
    assert score.verdict("file_recall", -0.1, 0.2, 5) == "n.s."
    assert score.verdict("solved", 0.1, 0.3, 1) == "n.s."
    assert score.bootstrap([0.0] * 7) == (0.0, 0.0, 0.0)


def test_reading_rule():
    def pm(**v):
        return {m: {"verdict": v.get(m, "n.s.")} for m in score.DIRECTION}
    assert score.reading(pm(solved="better")) == "HELPS"
    assert score.reading(pm(file_recall="better", usd="worse")) == "HELPS"
    assert score.reading(pm(solved="better", line_overlap="worse")) == "HURTS"
    assert score.reading(pm(usd="better")) == "HELPS"
    assert score.reading(pm(line_overlap="better")) == "NO MEASURED EFFECT"
    assert score.reading(pm()) == "NO MEASURED EFFECT"


def test_compare():
    with canned_run() as (tmp, base):
        x, y = str(tmp / "x.json"), str(tmp / "y.json")
        assert cli(base + ["score", "r1", "--out-json", x])[0] == 0
        rc, so, _ = cli(["compare", x, x])
        lines = so.splitlines()
        # g052 and g053 opened no truth file, so first_truth_open is unmeasured there.
        assert rc == 0 and len(lines) == 3 * len(score.DIRECTION) - 2, so
        for line in lines:
            assert line.startswith("[agent-ab] compare arm=")
            assert line.endswith(" diff=0.000 ci=[0.000,0.000] verdict=n.s."), line
        b = json.loads(Path(x).read_text())
        b["fingerprints"]["prompt_sha256"] = "q" * 64
        Path(y).write_text(json.dumps(b))
        rc, so, err = cli(["compare", x, y])
        assert rc == 2 and "prompt_sha256" in err and so == ""
        b["fingerprints"]["prompt_sha256"] = "p" * 64
        b["fingerprints"]["tasks"] = {"alpha.jsonl": "0" * 64}
        Path(y).write_text(json.dumps(b))
        rc, _, err = cli(["compare", x, y])
        assert rc == 2 and "fingerprints.tasks" in err


def test_refuses_changed_tasks():
    with canned_run() as (tmp, base):
        (Path(base[3]) / "alpha.jsonl").write_text(json.dumps(TASK_A) + "\n")
        rc, _, err = cli(base + ["score", "r1"])
        assert rc == 2 and "alpha" in err, err


def test_after_gate_split_needs_both_sides():
    def tm(n_true, n_false):
        out = {}
        for k, side in [(i, True) for i in range(n_true)] + [(i + 100, False)
                                                             for i in range(n_false)]:
            out[(f"t{k}", "none")] = {"repo": "r", "tuned": side, "after_gate_rev": side}
        return out
    names = [n for n, _ in score.subsets(tm(30, 4))]
    assert names == ["pooled", "tuned", "untuned", "repo=r"], names
    names = [n for n, _ in score.subsets(tm(5, 5))]
    assert names == ["pooled", "tuned", "untuned", "after_gate=true", "after_gate=false",
                     "repo=r"], names


def main():
    tests = [(n, f) for n, f in sorted(globals().items()) if n.startswith("test_")]
    failed = 0
    for name, fn in tests:
        try:
            fn()
            print(f"ok   {name}")
        except Exception:  # noqa: BLE001
            failed += 1
            print(f"FAIL {name}")
            traceback.print_exc()
    print(f"{len(tests) - failed} passed, {failed} failed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
