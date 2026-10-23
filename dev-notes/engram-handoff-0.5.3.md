# glia 0.5.3 - engram read-side handoff (0.5.2 -> 0.5.3)

**From:** the glia session, 2026-10-24, written by the handoff packet EZ.2 after glia's last code wave
of 0.5.3 (W15, HEAD `5639697b`).
**To:** an engram session. Act on Engram only.
**Status:** one break, the one Engram asked for. glia 0.5.3 removes the DE.5 PPR candidate side file
(EG.4, `19747e61`): `glia-export-engram --ppr-candidates` is gone. Nothing else in the export moves.
`GMAP_FORMAT_VERSION` stays 6, `engram-core` is untouched, and no 0.5.3 packet edits an Engram file.

0.5.3 is glia's ranking release: 43 packets in `dev-notes/leap-053-packets.json` (plan:
`dev-notes/next-leap-0.5.3.md`), waves W0-W16 on glia's local `main` since `v0.5.2` (`9837b1ff`). It
ports the ranking mechanisms the 2026-10-18..19 experiments measured in `engine/examples/rank_eval.rs`
into glia's ranked answers (blast radius, diff-impact, review, pack, and a new `find --files`). Every
ranking is computed at query time from the graph plus the history snapshot; none is stored in a
`.gmap`. Engram reads no ranking, so the only part that reaches Engram is the exporter change below.

Sources, in order of authority: EG.4's landed commit `19747e61` (its body and its diff of
`engram-export/`), the exporter source at glia HEAD `5639697b`, and the routed handoff paragraph in
`dev-notes/leap-053-corrections.json` ("record this break in dev-notes/engram-handoff-0.5.3.md. Undo the
0.5.2 handoff's section 5, its breaks-table row and its `.ppr.jsonl` .gitignore advice"). Engram was
read read-only at HEAD `e6ae2bf`. Packet ids are glossed in words; the hash after an id is the glia
commit that landed it.

---

## 1. TL;DR

1. **Pin `=0.5.3`**: build the exporter at the `v0.5.3` tag once James tags it (section 2).
2. **`--ppr-candidates` is gone** (EG.4, `19747e61`). clap rejects the flag (exit 2), `<out>.ppr.jsonl`
   is never written, and a stale one beside `<out>` is deleted. Exit 8 is retired, not reused.
   Engram's reader was already removed, and a grep of Engram's tracked files at `e6ae2bf` finds no
   caller of the flag outside `docs/eval-plan-2026-09.md`, so Engram needs no code change (section 3).
3. **Everything else in the export is unchanged**: the gmap, `<out>.files.json`, `<out>.meta.json`
   (apart from its version and stamp fields), `<out>.hints.jsonl` and `<out>.glia/` (section 4).
4. **This undoes three pieces of the 0.5.2 handoff** (`dev-notes/engram-handoff-0.5.2.md`): TL;DR item
   5 and section 3.5 (the side file), the DE.5 row of section 5's exporter table, and the
   `*.engram-gmap.ppr.jsonl` half of TL;DR item 9's `.gitignore` advice (section 3).

## 2. The pin

Engram links no glia crate; the exporter is glia's excluded `engram-export` crate, built and tested by
glia's `scripts/check-engram-export.sh`. The pin is the exporter built at the `v0.5.3` tag,
`glia-engram-export =0.5.3`:

```
git -C /home/ivy/Code/glia describe --tags --exact-match      # expect v0.5.3
/home/ivy/Code/glia/engram-export/target/debug/glia-export-engram -V
#   glia-export-engram 0.5.3 (build 0.5.3+p<stamp>)
```

**Not tagged yet.** At HEAD `5639697b` the workspace and `engram-export/Cargo.toml:3` still read
`0.5.2`: the version bump is the release step after these docs, never inside a packet. Until James
bumps and tags, a binary built from `main` prints `0.5.2 (build 0.5.2+p<stamp>)` with a stamp that
differs from `v0.5.2`'s. Every 0.5.3 packet that edits an engine, activation, code-domain or snapshots
source moves PARSER_STAMP, so the first export after the upgrade reparses from scratch; that alone
changes no graph content.

`<out>.meta.json` names the build as before (`glia_version`, `exporter_version`, `build_stamp`,
`parser_stamp`, `gmap_digest`, `meta_version` 1).

## 3. The break: the DE.5 side file is removed (EG.4, `19747e61`)

Engram asked for it gone before 0.5.3 ships: Engram's `0.5.2-complete` replays found PPR fails as a
release pool source (flat-ppr vs flat-fs pooled: hit 9 wins / 20 losses; 0.5.2 handoff section 6), and
James ruled the removal on 2026-10-18.

| what | 0.5.2 | 0.5.3 |
|---|---|---|
| `glia-export-engram ... --ppr-candidates <K>` | writes `<out>.ppr.jsonl` after the diff, prints `[engram-export] ppr side: <facts> facts, k=<K>, ...` | clap error, **exit 2**, nothing written |
| `<out>.ppr.jsonl` | written only with the flag | never written; a stale one left by a 0.5.2 run is deleted before the gmap write (exit 5 when it cannot be deleted, as for the other stale side files) |
| exit 8 (side file unwritten) | in use | **retired, not reused**; the other codes keep their numbers (hints is still 9) |
| library items | `glia_engram_export::ppr_side` (`write_ppr_side`, `ppr_candidates`, `PprSideStats`), `ppr_side_path` | removed |
| exporter dependencies | `glia-activation` (DE.5 added it) | dropped: the crate no longer depends on glia-activation; `engram-export/Cargo.lock` loses the direct edge |
| `glia_activation::algo::ppr` (push PPR) | used by the exporter | stays in glia (the rank_eval harness reads it); not reachable from the exporter |

What Engram does:
- **No code change.** Engram's reader of the side file was removed on its side before EG.4; a grep of
  Engram's tracked files at `e6ae2bf` for `ppr-candidates`, `ppr.jsonl` and `ppr_side` finds only
  `.gitignore:20` and `docs/eval-plan-2026-09.md` (lines 4580-4646, the `0.5.2-complete` record).
- **`.gitignore:20` `*.engram-gmap.ppr.jsonl` is dead.** Remove it or keep it; no 0.5.3 run writes the
  file. Keep `*.engram-gmap.hints.jsonl` (line 21, DT.12a) and `*.engram-gmap.meta.json` (line 19).
- **The eval plan's flat-ppr arm cannot be re-run on a 0.5.3 exporter.** Its recorded results stand
  (they were measured on the `0.5.2-complete` exporter); a replay script that still passes
  `--ppr-candidates` exits 2 on 0.5.3.

The 0.5.2 handoff's text this supersedes: TL;DR item 5 ("An opt-in PPR candidate side file"), section
3.5 ("The PPR candidate side file"), section 5's DE.5 row ("optional reader; `.gitignore` line"), the
`.ppr.jsonl` half of TL;DR item 9, section 6's "Engram to confirm, DE.5's format" bullet (closed: no
format left to confirm), and section 7.1's "before the PPR side file" ordering (the hints file is now
the last side file written before `<out>.glia/`).

## 4. Needs no change (checked)

- **The gmap, `<out>.files.json`, `<out>.hints.jsonl`:** EG.4 measured byte equality with the
  pre-removal HEAD on a scratch fixture; `<out>.meta.json` equal apart from the stamps
  (`build_stamp`, `parser_stamp`, `version_line`). The `--since` diff and `<out>.glia/` are not touched
  by the change.
- **Graph content:** no 0.5.3 packet declares a graph-content change. The rankings are query-time
  (Engram reads none), the history snapshot's version 2 (EB.1, `3fbe3e8f`: a `messages.jsonl`
  sidecar) is read by the rankings only, and the build's history inputs stay byte-identical with or
  without messages (`data_hash` covers `commits.jsonl` and `blame.jsonl` only). glia's substrate
  bench stayed at 419 fixtures, matrix 346/482 and blind 0 at every wave close-out W0-W15, and the
  engine's `byte_identical` test stayed green.
- **Exporter markers:** every `[engram-export]` line but the removed `ppr side:` keeps its shape.
- **SAME_AS, identity hints, the hints side file, `glia cell set --hint`:** unchanged since 0.5.2.

## 5. Re-test

At glia (EG.4's gate, on a `git archive` of HEAD plus the change):

```
GLIA_ROOT=<tree> bash scripts/check-engram-export.sh
[engram-export] check: ok - build + 95 tests passed against engram-core GMAP_FORMAT_VERSION=6
```

95 = HEAD's 102 minus `ppr_side_v6.rs`'s 4 and `ppr_side.rs`'s 3 unit tests; no warnings, no lock
drift after `--update-lock`. `hints_side_v6`'s rerun test now asserts that `--ppr-candidates` exits 2
and that a stale `.ppr.jsonl` is removed. Every later wave close-out reran the check; EG.7's commit
(`b252607e`, W14) records the same line, `build + 95 tests passed`.

In Engram: rebuild the exporter at the tag, export one repo you already replay, and check that
`<out>.ppr.jsonl` is absent and that the gmap digest matches a 0.5.2 export of the same commit (the
graph content is unchanged; only `meta.json`'s stamps move).

## 6. Done vs pending

- **Done, glia:** EG.4 (`19747e61`, W1). The exporter side of DE.5 is gone; `algo::ppr` stays in glia.
- **Pending, James:** the 0.5.2 -> 0.5.3 bump, the `v0.5.3` tag and the `glia-py` 0.5.3 publish.
- **Pending, Engram (optional):** drop `.gitignore:20`; note in the eval plan that the flat-ppr arm is
  0.5.2-only.
- **No other packet named Engram** as a consumer: the ranking packets' out-of-repo consumers are the
  repo-graph wrapper and neuropil (`dev-notes/repo-graph-handoff-0.5.3.md`,
  `dev-notes/neuropil-handoff-0.5.3.md`). The 0.5.3 release record (the rank gate, the CI pinning)
  is in section 9 of the repo-graph handoff.
