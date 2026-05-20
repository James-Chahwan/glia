# Noticed but not fixed

Append-only. Bugs / issues spotted outside the injection/derivation
pipeline during a cycle. Morning James decides what to do.

Format: one item per H2, dated, with file:line + a one-line description.

## run_instance.py:904 should also use `-l` (ignore whitespace) — 2026-05-21

Currently the patch fallback is `patch -p1 --forward --quiet --fuzz=5`. The iter1 winning diff had INDENT mismatch (model output 4-space, file 8-space) AND off-by-1 line numbers. Default fuzz alone wasn't enough; needed `-l` (ignore whitespace) on top. Add `-l` to the patch invocation in `run_instance.py:904`. Without it, the iter1 prescriptive-suffix wins won't be captured by run_instance.py's automated apply step.

## diff_healer.py — header reconstruction sometimes incomplete — 2026-05-21

Iter1 model output had `diff --git a/... b/...` header but missing `--- a/...` + `+++ b/...` lines. `diff_healer.heal_diff` recovered enough for `patch -p1` to handle it (the `diff --git` line is enough), but git apply --recount would have failed without the `---/+++` lines. Worth checking whether diff_healer should always emit those.

## Trailing newline on /tmp/iter1.patch — 2026-05-21

GNU patch errored with "patch unexpectedly ends in middle of line" when the diff didn't end with `\n`. diff_healer doesn't enforce this. Add a final `\n` if missing in heal_diff. Tiny but causes silent failures.
