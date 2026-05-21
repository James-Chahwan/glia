"""diff_healer.py — post-process unified diffs to fix common 7B Q4 failure modes.

Two recovery axes:
  1. Wrong-file: claimed file missing → search for basename match, or apply
     Python module-as-package rule (foo/bar.py ↔ foo/bar/__init__.py).
     Never redirects when claimed file exists (avoids silently patching the
     wrong file when model picked semantically-wrong-but-existing target).
  2. Line-drift: re-anchor each `@@` hunk by content-matching its pre-image
     (context + remove lines, in order) against the real source. Falls back
     to whitespace-tolerant comparison. If multiple matches, picks the one
     closest to claimed line.

Defensive: drops trailing hunks whose re-anchored start would overlap or
precede a previous hunk's end (catches generation loops where the model
repeats the same hunk shape with shifting line numbers).

Returns the original diff unchanged when nothing is healable. Public entry:
heal_diff(diff_text, repo_dir).
"""
from __future__ import annotations

import re
from pathlib import Path
from typing import List, Optional


_SKIP_DIRS = {".git", "__pycache__", ".tox", "node_modules", "build", "dist", ".venv"}


def heal_diff(diff_text: str, repo_dir: Path) -> str:
    if not diff_text:
        return diff_text
    sections = re.split(r"(?m)(?=^diff --git )", diff_text)
    healed: List[str] = []
    for s in sections:
        if not s.strip():
            continue
        if not s.startswith("diff --git"):
            healed.append(s)
            continue
        healed.append(_heal_section(s, repo_dir))
    return "".join(healed)


def _heal_section(section: str, repo_dir: Path) -> str:
    m = re.search(r"^--- a/(\S+)", section, re.M)
    if not m:
        return section
    claimed = m.group(1)

    real_path: Optional[str] = claimed if (repo_dir / claimed).is_file() else None
    if real_path is None:
        for cand in _resolve_candidates(claimed, repo_dir, section=section):
            if _candidate_valid(repo_dir / cand, section):
                real_path = cand
                break
        if real_path is None:
            return section
        section = re.sub(r"^diff --git a/\S+ b/\S+",
                         f"diff --git a/{real_path} b/{real_path}",
                         section, count=1, flags=re.M)
        section = re.sub(r"^--- a/\S+", f"--- a/{real_path}", section, count=1, flags=re.M)
        section = re.sub(r"^\+\+\+ b/\S+", f"+++ b/{real_path}", section, count=1, flags=re.M)

    real_file = repo_dir / real_path
    if not real_file.is_file():
        return section
    try:
        source_lines = real_file.read_text(errors="replace").splitlines()
    except Exception:
        return section
    return _heal_hunks(section, source_lines)


def _candidate_valid(real_file: Path, section: str) -> bool:
    """Return True if the first hunk's `-` lines (or, lacking them, ANY context
    line) appear in the candidate file. Loose-by-design: model context drifts
    routinely but `-` lines are usually verbatim from real source."""
    if not real_file.is_file():
        return False
    parts = re.split(r"(?m)(?=^@@ )", section)
    if len(parts) < 2:
        return False
    first_hunk = parts[1]
    body = first_hunk.split("\n")[1:]
    minus = [ln[1:] for ln in body if ln.startswith("-")]
    context = [ln[1:] for ln in body if ln.startswith(" ") and ln[1:].strip()]
    needles = minus or context[:3]
    if not needles:
        return False
    try:
        source_lines = real_file.read_text(errors="replace").splitlines()
    except Exception:
        return False
    src_strip = [s.strip() for s in source_lines]
    for needle in needles:
        if needle.strip() in src_strip:
            return True
    return False


def _resolve_candidates(claimed: str, repo_dir: Path, section: Optional[str] = None) -> List[str]:
    """Return ordered list of candidate paths to try (best-first)."""
    out: List[str] = []
    seen = set()

    def add(p: str) -> None:
        if p in seen:
            return
        seen.add(p)
        out.append(p)

    if claimed.endswith(".py"):
        as_pkg = claimed[:-3] + "/__init__.py"
        if (repo_dir / as_pkg).is_file():
            add(as_pkg)

    basename = claimed.split("/")[-1]
    matches: List[str] = []
    try:
        for f in repo_dir.rglob(basename):
            if not f.is_file():
                continue
            try:
                rel = f.relative_to(repo_dir)
            except ValueError:
                continue
            if any(p in _SKIP_DIRS for p in rel.parts):
                continue
            matches.append(str(rel))
    except Exception:
        matches = []

    claimed_parts = claimed.split("/")

    def align_score(m: str) -> int:
        m_parts = m.split("/")
        common = 0
        for a, b in zip(reversed(claimed_parts), reversed(m_parts)):
            if a == b:
                common += 1
            else:
                break
        return -common

    matches.sort(key=align_score)
    for m in matches:
        add(m)

    # Content-based fallback: if section provided, take the first hunk's `-`
    # lines (or first non-empty context line) and find which .py file under
    # repo_dir contains them. Useful when basename/module-as-package don't
    # match the gold (e.g. seaborn `_core.py` → `_oldcore.py`).
    if section is not None:
        needle = _first_strong_needle(section)
        if needle and len(needle.strip()) >= 20:
            try:
                for f in repo_dir.rglob("*.py"):
                    if not f.is_file():
                        continue
                    try:
                        rel = f.relative_to(repo_dir)
                    except ValueError:
                        continue
                    if any(p in _SKIP_DIRS for p in rel.parts):
                        continue
                    rel_str = str(rel)
                    if rel_str in seen:
                        continue
                    try:
                        content = f.read_text(errors="replace")
                    except Exception:
                        continue
                    if needle in content:
                        add(rel_str)
            except Exception:
                pass
    return out


def _first_strong_needle(section: str) -> Optional[str]:
    parts = re.split(r"(?m)(?=^@@ )", section)
    if len(parts) < 2:
        return None
    body = parts[1].split("\n")[1:]
    for ln in body:
        if ln.startswith("-") and len(ln[1:].strip()) >= 20:
            return ln[1:]
    for ln in body:
        if ln.startswith(" ") and len(ln[1:].strip()) >= 20:
            return ln[1:]
    return None


def _heal_hunks(section: str, source_lines: List[str]) -> str:
    parts = re.split(r"(?m)(?=^@@ )", section)
    if len(parts) < 2:
        return section
    header = parts[0]
    healed_hunks: List[str] = []
    last_end = -1
    for hunk in parts[1:]:
        new_hunk, new_start, pre_count = _heal_one_hunk(hunk, source_lines)
        if new_start is None or pre_count is None:
            healed_hunks.append(hunk)
            continue
        if new_start <= last_end:
            continue
        last_end = new_start + pre_count - 1
        healed_hunks.append(new_hunk)
    return header + "".join(healed_hunks)


def _heal_one_hunk(hunk: str, source_lines: List[str]):
    lines = hunk.split("\n")
    if not lines or not lines[0].startswith("@@"):
        return hunk, None, None
    hdr = lines[0]
    body = lines[1:]

    m = re.match(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))?", hdr)
    claimed_start = int(m.group(1)) if m else 1
    src_quota = int(m.group(2)) if (m and m.group(2)) else 1
    tgt_quota = int(m.group(4)) if (m and m.group(4)) else 1

    # Slice body to the header's declared src/tgt counts. 7B Q4 commonly dumps
    # extra body well past the claimed range; trimming keeps the pre-image
    # short enough to match real source.
    sliced: List[str] = []
    pre: List[str] = []
    src_left, tgt_left = src_quota, tgt_quota
    for ln in body:
        if src_left <= 0 and tgt_left <= 0:
            break
        if not ln:
            if src_left > 0 and tgt_left > 0:
                sliced.append(ln)
                pre.append("")
                src_left -= 1
                tgt_left -= 1
            else:
                break
            continue
        c = ln[0]
        rest = ln[1:]
        if c == " ":
            if src_left > 0 and tgt_left > 0:
                sliced.append(ln)
                pre.append(rest)
                src_left -= 1
                tgt_left -= 1
            else:
                break
        elif c == "-":
            if src_left > 0:
                sliced.append(ln)
                pre.append(rest)
                src_left -= 1
            else:
                break
        elif c == "+":
            if tgt_left > 0:
                sliced.append(ln)
                tgt_left -= 1
            else:
                break
        elif c == "\\":
            sliced.append(ln)
        else:
            break
    if not pre:
        return hunk, None, None

    minus = sum(1 for ln in sliced if ln.startswith("-"))
    plus = sum(1 for ln in sliced if ln.startswith("+"))
    ctx = sum(1 for ln in sliced if ln.startswith(" ") or ln == "")
    pre_count = minus + ctx
    post_count = plus + ctx

    start = _find_run(source_lines, pre, exact=True, near=claimed_start - 1)
    if start is None:
        start = _find_run(source_lines, pre, exact=False, near=claimed_start - 1)

    if start is not None:
        new_start = start + 1
        mt = re.match(r"^@@ [^@]+ @@(.*)$", hdr)
        trailing = mt.group(1) if mt else ""
        new_hdr = f"@@ -{new_start},{pre_count} +{new_start},{post_count} @@{trailing}"
        return new_hdr + "\n" + "\n".join(sliced) + "\n", new_start, pre_count

    # Aggressive fallback: anchor on the `-` lines in source, rebuild hunk
    # with REAL surrounding context. Handles the common 7B Q4 case where the
    # model hallucinates context lines (e.g. dropping a docstring between
    # `def foo` and the first body line) but the actual `-` lines are real.
    aggr = _aggressive_heal(sliced, source_lines)
    if aggr is not None:
        new_hunk, new_start, pre_count = aggr
        return new_hunk, new_start, pre_count
    return hunk, None, None


def _aggressive_heal(sliced_body: List[str], source_lines: List[str]):
    minus = [ln[1:] for ln in sliced_body if ln.startswith("-")]
    plus = [ln[1:] for ln in sliced_body if ln.startswith("+")]
    if not minus:
        return None

    def find_anchor(needles: List[str], exact: bool) -> Optional[int]:
        if not needles:
            return None
        eq = (lambda a, b: a == b) if exact else (lambda a, b: a.strip() == b.strip())
        cands: List[int] = []
        n = len(needles)
        for i in range(len(source_lines) - n + 1):
            ok = True
            for j in range(n):
                if not eq(source_lines[i + j], needles[j]):
                    ok = False
                    break
            if ok:
                cands.append(i)
        if len(cands) != 1:
            return None
        return cands[0]

    start = find_anchor(minus, exact=True)
    if start is None:
        start = find_anchor(minus, exact=False)
    if start is None:
        return None
    end = start + len(minus)
    ctx_before = max(0, start - 3)
    ctx_after = min(len(source_lines), end + 3)

    new_body: List[str] = []
    for i in range(ctx_before, start):
        new_body.append(" " + source_lines[i])
    for i in range(start, end):
        new_body.append("-" + source_lines[i])
    for p in plus:
        new_body.append("+" + p)
    for i in range(end, ctx_after):
        new_body.append(" " + source_lines[i])

    new_start = ctx_before + 1
    src_count = (start - ctx_before) + len(minus) + (ctx_after - end)
    tgt_count = (start - ctx_before) + len(plus) + (ctx_after - end)
    new_hdr = f"@@ -{new_start},{src_count} +{new_start},{tgt_count} @@"
    return new_hdr + "\n" + "\n".join(new_body) + "\n", new_start, src_count


def _find_run(source_lines: List[str], needles: List[str], exact: bool, near: int) -> Optional[int]:
    n = len(needles)
    if n == 0 or n > len(source_lines):
        return None
    if exact:
        def eq(a: str, b: str) -> bool:
            return a == b
    else:
        def eq(a: str, b: str) -> bool:
            return a.strip() == b.strip()

    candidates: List[int] = []
    upper = len(source_lines) - n + 1
    for i in range(upper):
        ok = True
        for j in range(n):
            if not eq(source_lines[i + j], needles[j]):
                ok = False
                break
        if ok:
            candidates.append(i)
    if not candidates:
        return None
    if len(candidates) == 1:
        return candidates[0]
    return min(candidates, key=lambda i: abs(i - near))
