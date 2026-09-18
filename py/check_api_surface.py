#!/usr/bin/env python3
"""Cross-check the committed pyo3 surface snapshots against the INSTALLED wheel.

`py/tests/api_surface.rs` pins the surface from SOURCE (the in-wave gate).
This script runs at wave close-out, after the wheel rebuild, and catches what a
source scan cannot see: a macro-generated `#[pyfunction]`, a registration the
scan resolved wrongly, a stale `.so`. It compares, per module function and per
class member: the name set, the member kind, and the parameter names, kinds
and defaults of `__text_signature__` against `py/api_surface/*.txt`.

Stdlib only. Usage: `python3 py/check_api_surface.py [snapshot_dir]` (the
argument only exists to test this script against a mutated copy). Exit 0 and
print `[api-surface] wheel OK (N functions, M members)`, or print each mismatch
and exit 1.
"""
from __future__ import annotations

import ast
import importlib
import inspect
import pathlib
import re
import sys

SNAPSHOT_DIR = pathlib.Path(__file__).resolve().parent / "api_surface"

LINE = re.compile(
    r"^(?P<kind>pymodule|fn|class|method|staticmethod|classmethod|new|getter|setter|classattr|variant)"
    r" (?P<rest>.*)$"
)


def split_top_level(text: str) -> list[str]:
    """Split a rendered parameter list at commas outside <>, (), [], and quotes."""
    parts, depth, cur, quote = [], 0, [], None
    i = 0
    while i < len(text):
        ch = text[i]
        if quote:
            cur.append(ch)
            if ch == "\\" and i + 1 < len(text):
                cur.append(text[i + 1])
                i += 1
            elif ch == quote:
                quote = None
        elif ch == '"':
            quote = ch
            cur.append(ch)
        elif ch in "<([":
            depth += 1
            cur.append(ch)
        elif ch in ">)]" and not (ch == ">" and i > 0 and text[i - 1] == "-"):
            depth -= 1
            cur.append(ch)
        elif ch == "," and depth == 0:
            parts.append("".join(cur).strip())
            cur = []
        else:
            cur.append(ch)
        i += 1
    if "".join(cur).strip():
        parts.append("".join(cur).strip())
    return parts


def params_of(rest: str) -> tuple[str, list[str]]:
    """`name(<params>) -> ret` -> (name, [param, ...])."""
    open_at = rest.index("(")
    name = rest[:open_at]
    depth = 0
    for i in range(open_at, len(rest)):
        if rest[i] == "(":
            depth += 1
        elif rest[i] == ")":
            depth -= 1
            if depth == 0:
                return name, split_top_level(rest[open_at + 1 : i])
    raise ValueError(f"unbalanced parameter list: {rest!r}")


_MISSING = object()
_ANY = object()


def rust_default(text: str):
    """A Rust default expression as the Python value pyo3 shows for it."""
    mapped = {"true": "True", "false": "False"}.get(text, text)
    try:
        return ast.literal_eval(mapped)
    except (ValueError, SyntaxError):
        return _ANY  # a non-literal default: pyo3 renders it as `...`


def expected_params(params: list[str]) -> list[tuple[str, str, object]]:
    """Rendered params -> [(name, kind, default)], kind in {pos, kwonly, var, varkw}."""
    out, kwonly = [], False
    for p in params:
        if p == "/":
            continue
        if p == "*":
            kwonly = True
            continue
        head, _, default = p.partition(" = ")
        name = head.split(":", 1)[0].strip()
        if name.startswith("**"):
            out.append((name[2:], "varkw", _MISSING))
        elif name.startswith("*"):
            out.append((name[1:], "var", _MISSING))
            kwonly = True
        else:
            dflt = rust_default(default) if default else _MISSING
            out.append((name, "kwonly" if kwonly else "pos", dflt))
    return out


def actual_params(obj) -> list[tuple[str, str, object]] | None:
    sig_text = getattr(obj, "__text_signature__", None)
    if sig_text is None:
        return None
    sig = inspect.signature(obj)
    out = []
    for p in sig.parameters.values():
        if p.name in ("self", "$self", "cls", "$cls", "type", "$type"):
            continue
        kind = {
            inspect.Parameter.VAR_POSITIONAL: "var",
            inspect.Parameter.VAR_KEYWORD: "varkw",
            inspect.Parameter.KEYWORD_ONLY: "kwonly",
        }.get(p.kind, "pos")
        dflt = _MISSING if p.default is inspect.Parameter.empty else p.default
        out.append((p.name, kind, dflt))
    return out


def same_params(want, got) -> bool:
    if len(want) != len(got):
        return False
    for (wn, wk, wd), (gn, gk, gd) in zip(want, got):
        if wn != gn or wk != gk:
            return False
        if wd is _ANY or gd is Ellipsis:
            continue
        if (wd is _MISSING) != (gd is _MISSING) or (wd is not _MISSING and wd != gd):
            return False
    return True


def show(params) -> str:
    def one(n, k, d):
        prefix = {"var": "*", "varkw": "**"}.get(k, "")
        return f"{prefix}{n}" + ("" if d is _MISSING else f"={d!r}")
    return "(" + ", ".join(one(*p) for p in params) + ")"


def load_snapshots():
    module_name = None
    functions: dict[str, list[str]] = {}
    classes: set[str] = set()
    members: dict[str, dict[str, tuple[str, str]]] = {}
    for path in sorted(SNAPSHOT_DIR.glob("*.txt")):
        for raw in path.read_text().splitlines():
            if not raw.strip() or raw.startswith("#"):
                continue
            m = LINE.match(raw)
            if not m:
                raise SystemExit(f"[api-surface] unparseable line in {path.name}: {raw!r}")
            kind, rest = m["kind"], m["rest"]
            if kind == "pymodule":
                module_name = rest.strip()
            elif kind == "fn":
                name, params = params_of(rest)
                functions[name] = params
            elif kind == "class":
                classes.add(rest.split(" ", 1)[0])
            else:
                owner, _, tail = rest.partition(".")
                name = re.split(r"[(: ]", tail, maxsplit=1)[0]
                members.setdefault(owner, {})[name] = (kind, tail)
    if module_name is None:
        raise SystemExit("[api-surface] no `pymodule` line in py/api_surface/*.txt")
    return module_name, functions, classes, members


def main() -> int:
    global SNAPSHOT_DIR
    if len(sys.argv) > 1:  # an alternate snapshot dir (to test this script)
        SNAPSHOT_DIR = pathlib.Path(sys.argv[1])
    module_name, functions, classes, members = load_snapshots()
    mod = importlib.import_module(module_name)
    errors: list[str] = []

    wheel_classes = {n for n, v in vars(mod).items() if isinstance(v, type) and not n.startswith("_")}
    wheel_fns = {
        n
        for n, v in vars(mod).items()
        if not n.startswith("_") and callable(v) and not isinstance(v, type)
        and type(v).__name__ == "builtin_function_or_method"
    }
    for n in sorted(set(functions) - wheel_fns):
        errors.append(f"function in snapshot, not in wheel: {n}")
    for n in sorted(wheel_fns - set(functions)):
        errors.append(f"function in wheel, not in snapshot: {n}")
    for n in sorted(set(functions) & wheel_fns):
        want, got = expected_params(functions[n]), actual_params(getattr(mod, n))
        if got is not None and not same_params(want, got):
            errors.append(f"fn {n}: snapshot {show(want)} != wheel {show(got)}")

    for n in sorted(classes ^ wheel_classes):
        side = "snapshot" if n in classes else "wheel"
        errors.append(f"class only in {side}: {n}")

    member_count = 0
    for cls_name in sorted(classes & wheel_classes):
        cls = getattr(mod, cls_name)
        snap = members.get(cls_name, {})
        member_count += len(snap)
        wheel = {n for n in vars(cls) if not (n.startswith("__") and n.endswith("__"))}
        want_names = {n for n in snap if not (n.startswith("__") and n.endswith("__"))}
        for n in sorted(want_names - wheel):
            errors.append(f"{cls_name}.{n}: in snapshot, not in wheel")
        for n in sorted(wheel - want_names):
            errors.append(f"{cls_name}.{n}: in wheel, not in snapshot")
        for n, (kind, tail) in sorted(snap.items()):
            if n not in vars(cls) and n != "__new__":
                if n.startswith("__"):
                    errors.append(f"{cls_name}.{n}: in snapshot, not in wheel")
                continue
            raw = vars(cls).get(n)
            wheel_kind = type(raw).__name__
            if kind in ("getter", "setter"):
                if wheel_kind != "getset_descriptor":
                    errors.append(f"{cls_name}.{n}: snapshot {kind}, wheel {wheel_kind}")
                continue
            if kind in ("classattr", "variant"):
                continue
            if kind == "staticmethod" and wheel_kind not in ("staticmethod", "builtin_function_or_method"):
                errors.append(f"{cls_name}.{n}: snapshot staticmethod, wheel {wheel_kind}")
            if kind == "classmethod" and wheel_kind != "classmethod_descriptor":
                errors.append(f"{cls_name}.{n}: snapshot classmethod, wheel {wheel_kind}")
            if kind == "method" and wheel_kind != "method_descriptor":
                errors.append(f"{cls_name}.{n}: snapshot method, wheel {wheel_kind}")
            target = cls if kind == "new" else getattr(cls, n)
            _, params = params_of(tail)
            want, got = expected_params(params), actual_params(target)
            if got is not None and not same_params(want, got):
                errors.append(f"{cls_name}.{n}: snapshot {show(want)} != wheel {show(got)}")

    if errors:
        for e in errors:
            print(f"[api-surface] MISMATCH {e}")
        print(f"[api-surface] wheel FAILED ({len(errors)} mismatches) against {SNAPSHOT_DIR}")
        return 1
    print(f"[api-surface] wheel OK ({len(functions)} functions, {member_count} members)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
