# `matrix/` — the per-cell coverage corpus

One directory per cell of the 17-language × 30-mechanism coverage matrix, in the
canonical layout `matrix/<language>/<mechanism>/key.json`, spelled exactly as
[`../matrix_vocab.py`](../matrix_vocab.py) spells them.

**To author one: [`../AUTHORING.md`](../AUTHORING.md).** Start with
`python3 ../scaffold.py <language> <mechanism>` — it stamps a correct skeleton
from the vocabulary, so a cell is a fill-in-the-source task rather than a design
task.

## Why this is not `fixtures/`

The two trees are graded by different tools, on purpose.

- `run.py` discovers **`fixtures/*` only**. That is the gated corpus: its
  BLIND SPOTS / PARTIAL / FORBID VIOLATIONS / MISSING CELLS / GRADER ERRORS
  counters are a release signal, and driving them to zero is the point of the
  P1 work. A tree full of deliberately-failing work-in-progress cells must never
  be able to move those numbers.
- `matrix.py` discovers **both** — `fixtures/*/key.json` as `legacy_only` and
  `matrix/*/*/key.json` as canonical — and reports per-cell levels
  (`full` ● / `partial` ◐ / `none` · / `unknown` ? / `n/a` - / `error` !).
  `n/a` is a cell in `../matrix_vocab.py`'s `NOT_APPLICABLE` list that no
  fixture claims: the language cannot express the mechanism (Solidity x
  brokers, Terraform x calls, ...). A directory here for such a cell makes it an
  `error` until the same commit deletes the entry, and `scaffold.py` refuses to
  stamp one.

A new cell here is *supposed* to start red. That is the whole method: record the
`0.00` baseline first, then close it. Keeping those baselines out of `run.py`'s
discovery is what lets a cell be honestly red without breaking the gate.

## What a cell directory holds

```
matrix/python/kafka/
  key.json          the frozen-vocabulary key (see ../README.md)
  client/           dirs[0] — present only when the mechanism is cross_repo
  server/           dirs[1]
```

A `cross_repo` mechanism gets two dirs because a cross-service flow is a
client/server shape, not because one dir cannot show the edge: stack resolvers
(HTTP, gRPC, queue, …) pair inside one repo, and only the `SHARES_*` resolvers
need two repos — see [`../AUTHORING.md`](../AUTHORING.md) step 1 for the measured
table. Single-repo mechanisms use `dirs: ["."]` and keep their stubs in the cell
root.

The scaffolder's template lives as a constant inside `scaffold.py`
(`python3 ../scaffold.py --template` prints it), **not** as an on-disk
`_template/` directory — so neither `run.py` nor `matrix.py` can ever discover a
skeleton full of `TODO`s and grade it as if it were a fixture.
