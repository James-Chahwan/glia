# engram-export `hint_refused` by reason (FD.34, 0.5.4)

Engram's registered run (glia `196728e3`) asked why the export's identity hint is refused
for a re-keyed fact: its glia chain summed `hint_refused 32` over 495 exports (words 39, hint 30,
name 284). FD.34 splits the count by the guard test that fails, and this note reads the split on
glia's own (public) history.

## What a refusal is

`--since` pairs nodes in five passes (`engram-export/src/diff.rs`, module doc). Pass 5 pairs an
unmatched base node with an unmatched target node whose `identity_hint` (file token, kind,
ordinal) is unique on both sides, unless the guard refuses it. The guard (`hint_refusal`) names
the first test that fails:

| reason | the pair |
|---|---|
| `verb_mismatch` | two ROUTEs or two ENDPOINTs whose verbs differ |
| `path_unalike` | two ROUTEs or two ENDPOINTs of one verb whose paths are not alike |
| `shape_mismatch` | a non-route pair of different content shapes (Symbol vs Proposition, a Vector) |
| `no_words` | a non-route pair one of whose names has no words (`_`, `$`), names differing |
| `words_unfit` | a non-route pair whose names have words but do not fit (no shared word, or Jaccard < 1/3 with neither word set holding the other) |

A refused pair is one removed and one added fact: Engram mints a fresh FactId instead of
handing the new fact the old one's. The counts sum to `hint_refused`.

The marker is its own line after the unchanged pairing line (Engram's replay sums the
`since pairing:` line's `k=v` fields, `crates/engram-live/examples/replay.rs`, so that line keeps
its shape):

```
[engram-export] v6 since pairing: route=0 endpoint=0 name=0 words=2 hint=5 hint_refused=1
[engram-export] v6 since refusals: hint_refused=1 (verb_mismatch=0 path_unalike=0 shape_mismatch=0 no_words=0 words_unfit=1)
```

## Measured on glia's history

The exporter built by `scripts/check-engram-export.sh` with FD.34, run on `git archive` copies
under a user cache dir (nothing written into a checkout):

| run | exports | words | hint | hint_refused | by reason |
|---|---|---|---|---|---|
| `v0.5.2` -> `v0.5.3` (one step) | 1 | 1 | 0 | 0 | - |
| `v0.5.3` -> `a19ac3fe` (one step) | 1 | 0 | 0 | 0 | - |
| first-parent, one step per commit, `v0.5.3~12..v0.5.3` | 12 | 2 | 0 | 0 | - |
| first-parent, one step per commit, `v0.5.2..v0.5.3` | 108 | 7 | 5 | 3 | `words_unfit` 3 |

Every refusal on glia's history is `words_unfit`. The three pairs (read back by pairing the
removed and added nodes of each step's diff by their unique `identity_hint`):

1. `563385dd` (an example file's domain constants replaced by generic ones): a STATE_VAR removed
   and an unrelated one added at the same ordinal of `engine/examples/rank_eval.rs`; the names
   share one of four words.
2. `182de4f1` (EC.11): `engine::src::review::IntentSignals::fusions` removed,
   `IntentSignals::order` added at its ordinal; no shared word.
3. `b252607e` (EG.7): `engine::src::review::ids_by_file` removed, `engine::src::review::intent_rank`
   added at its ordinal; no shared word.

Each is two different facts that took one ordinal after an edit in the same file, the case DE.4's
guard exists for: pairing them would hand the new fact the old one's FactId. None is a rename the
guard lost, so **no reason is a glia bug** and FD.34 changes no pairing. The one known lost
rename class stays DE.4's documented trade (a true rename sharing no word is removed + added).

The route reasons (`verb_mismatch`, `path_unalike`) never fire on glia, which holds few ROUTEs;
`shape_mismatch` and `no_words` never fire either. Engram's 32 over 495 steps of its glia chain
are about the rate measured here (3 over 108).
