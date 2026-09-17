#!/usr/bin/env python3
"""Hermetic tests for run.py's committed legacy snapshot — `python3 test_run.py`
(pytest collects it too), exit 0 on green.

legacy-latest.json is only a proof if a stale copy FAILS loudly. These tests
pin the failing direction without grading a single fixture: canned grade rows
in, drift lines out.
"""
import json
import sys
import tempfile
import traceback
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import run  # noqa: E402


def _row(fw, lang, **recalls):
    return {"framework": fw, "language": lang,
            "per_category": {c: {"expected": 2, "found": int(r * 2), "recall": r}
                             for c, r in recalls.items()}}


GRADED = [("php-laravel-calls", _row("php-laravel", "php", CALLS=0.5)),
          ("php-laravel-routes", _row("php-laravel", "php", HANDLED_BY=1.0))]
LINES = {"blind_spots": ["x: USES"], "missing_cells": ["a: POSITION on WS_CLIENT 'w'"]}


def _base():
    return run.legacy_payload(3, GRADED, LINES, "0.4.18")


def _edited(mutate):
    d = json.loads(run.render_legacy(_base()))
    mutate(d)
    return run.legacy_drift(d, _base())


def test_render_is_deterministic_and_newline_terminated():
    a = run.render_legacy(_base())
    b = run.render_legacy(run.legacy_payload(3, list(GRADED), dict(LINES), "0.4.18"))
    assert a == b and a.endswith("}\n")


def test_committed_view_has_no_timestamp_or_build_stamp():
    assert not {"ts", "build_stamp"} & set(_base())


def test_schema_is_distinct_from_results_latest():
    assert isinstance(_base()["schema"], str) and _base()["schema"] != 1


def test_fixtures_sharing_a_framework_stay_distinct():
    assert sorted(_base()["recall"]) == ["php-laravel-calls", "php-laravel-routes"]


def test_sub_one_recall_is_carried():
    assert _base()["recall"]["php-laravel-calls"]["per_category"]["CALLS"] == 0.5


def test_every_summary_section_present_even_when_empty():
    s = _base()["summary"]
    assert set(s) == set(run.SUMMARY_KEYS) and s["grader_errors"] == []


def test_identical_payloads_have_no_drift():
    assert run.legacy_drift(_base(), _base()) == []


def test_hand_edited_recall_is_named_with_both_values():
    def m(d):
        d["recall"]["php-laravel-calls"]["per_category"]["CALLS"] = 1.0
    got = _edited(m)
    assert got == ["recall.php-laravel-calls.per_category.CALLS: 1.0 -> 0.5"], got


def test_blind_spot_missing_from_committed_reads_plus():
    got = _edited(lambda d: d["summary"].__setitem__("blind_spots", []))
    assert got == ['summary.blind_spots: + "x: USES"'], got


def test_blind_spot_gone_from_fresh_run_reads_minus():
    got = _edited(lambda d: d["summary"]["blind_spots"].append("y: CALLS"))
    assert got == ['summary.blind_spots: - "y: CALLS"'], got


def test_fixture_absent_from_committed_is_named():
    got = _edited(lambda d: d["recall"].pop("php-laravel-routes"))
    assert any(line.startswith("recall.php-laravel-routes.") and "(absent) ->" in line
               for line in got), got


def test_check_legacy_file_states():
    base = _base()
    with tempfile.TemporaryDirectory() as tmp:
        p = Path(tmp) / "legacy-latest.json"
        got = run.check_legacy(base, p)
        assert len(got) == 1 and "(absent)" in got[0], got
        p.write_text(run.render_legacy(base), encoding="utf-8")
        assert run.check_legacy(base, p) == []
        p.write_text(json.dumps(base, sort_keys=True), encoding="utf-8")
        got = run.check_legacy(base, p)
        assert len(got) == 1 and "bytes differ" in got[0], got
        p.write_text("{", encoding="utf-8")
        got = run.check_legacy(base, p)
        assert len(got) == 1 and "not valid JSON" in got[0], got


def test_mistyped_gate_flag_is_rejected():
    # argparse exits 2 before any fixture is graded; a silently-ignored
    # `--chek` would be a gate that always reads green.
    try:
        run.main(["--no-log", "--chek"])
    except SystemExit as exc:
        assert exc.code == 2, exc.code
    else:
        raise AssertionError("--chek was accepted")


def main():
    tests = [(n, f) for n, f in sorted(globals().items())
             if n.startswith("test_") and callable(f)]
    failed = 0
    for name, fn in tests:
        try:
            fn()
        except Exception:  # noqa: BLE001
            failed += 1
            print(f"FAIL {name}")
            traceback.print_exc()
        else:
            print(f"PASS {name}")
    print(f"\n{len(tests) - failed} passed, {failed} failed ({len(tests)} tests)")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
