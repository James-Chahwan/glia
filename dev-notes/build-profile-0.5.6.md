# Build profile, measured per language (GP.1, 0.5.6)

GP.1 (NEW.f4o.1) measured where a cold `glia build` spends its single-core time on the three cohort repos
whose shapes differ: twenty (TypeScript), photoprism (Go + Vue) and tolgee (Kotlin / Java + TS). It is
measurement only. The timers were added in a throwaway tree and nothing from that tree is committed.
GP.2 (the TS group front), GP.3 (the Go and JVM group fronts) and GP.4 (persist) read this table first and
drop any part whose phase measured under 200 ms.

## How it was measured

- **Code.** A `git archive` of HEAD `17e5cfb8` (0.5.5 + 0.5.6 W0's landed packets) was unpacked under
  `~/.cache/glia-GP.1/`. Temporary `[prof] <phase> ms=<ms> thread=<id> cpu_ms=<ms>` guards were added, each
  printing wall time and the thread's on-CPU time (`/proc/thread-self/schedstat`), around:
  - the group builders `build_ts_like`, `build_go_passes_with` (every pass) and `build_dotted` (the JVM
    family's builder: `lang_build::build_solo` sends `java` / `kotlin` there);
  - inside them: `merge_parses` (its loop and `fold_role_overlays`), `build_symbol_table` and each import
    resolver, with `resolve_imports_ts`'s barrel read, materialize and statement loop timed separately;
  - `resolve_calls`: its `GlobalNames`, pooled decide, serial apply (with the `extra_hook` total summed
    inside apply) and the marker tail;
  - `resolve_refs`: `SiteFiles`, decide, apply, and the marker / nav tail;
  - the engine's `build_one`: unattributed edges, builder, stamp, heritage / line tally, and
    `fill_group_evidence`;
  - `plan_groups` / `build_groups` / `fold_groups`;
  - `persist_graph`, and per shard in the store: `code_core`, `CodeSection::from_repo_graph`, the code
    section, the raw-form checks, `intern_evidence`, `span_code_cells`, the strings section, the rkyv
    `encode_file`, the hash / on-disk compare, `write_atomic` and the manifest.

  Per-call accumulators summed `unique_global_module` / `_function` / `_type`, the TS resolver callback
  (`resolve_source`), `ts_target_tagged`, `GoPackages::import_target` and the three arms of the Go call hook.
  The grafts needed no new timer, because GA.20's committed `[timing] grafts repo=.. <step>=<ms>` line
  already times each step.
- **Binary.** `cargo build --release -p glia-cli --no-default-features` (`CARGO_INCREMENTAL=0`). The
  embedding features are left out because a build never touches them.
- **Runs.** Cold `glia build --no-incremental <copy>` with `.glia/` deleted first. Each copy is a `git archive`
  from the cohort bare clones: twenty `8fd53a83`, photoprism `70e20b5e`, tolgee `43e66544`. Each run went
  through the NEW.f4o.1 sampler (`prof.py`: every stderr line timestamped, process cores from
  `/proc/<pid>/stat` every 0.5 s). 16 cores.
- **Load.** The machine was never quiet: other wave agents were running rank_eval, clippy and builds.
  Runs 1-4 (1-min load 22-38) inflated walls by 1.5-3.8x and are discarded. The table uses **runs 5 and 6**,
  taken back to back at 1-min load **10.3-13.5**, below the 16 cores. Their walls reproduce the 0.5.6
  plan's probe (twenty 15.35 s, photoprism 6.87 s, tolgee 5.10 s):

  | repo | wall run 5 | wall run 6 |
  |---|---:|---:|
  | twenty | 16.71 s | 15.32 s |
  | photoprism | 6.24 s | 5.94 s |
  | tolgee | 4.34 s | 4.27 s |

  The `thread-CPU` column is the timed thread's on-CPU time. When it equals wall time, the phase ran on
  one core with no contention. When it is lower, either the phase waited on a pool or the scheduler took
  the core away. Every phase below is single-threaded, except `resolve_calls.decide` and
  `resolve_refs.decide` (FH.21's pooled decide).
- **Shape** names what the work is, as the packets ask:
  - **per-file**: an independent map over files or parses, folded in input order;
  - **read-only**: decides over a frozen graph and applies after (FH.21's decide / apply);
  - **stateful**: each step reads what the earlier steps wrote.

  Shapes come from reading each function's body. They are not measured.

## Whole-build context (from the committed `[timing]` lines, ms, run 5 / run 6)

| repo | walk | parse | const_scan | grafts | language_build | resolve | post | finalize | persist |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| twenty | 793 / 792 | 6125 / 5605 | 151 / 90 | 1808 / 1539 | 2809 / 2483 | 727 / 670 | 896 / 813 | 135 / 123 | 2625 / 2609 |
| photoprism | 76 / 80 | 1360 / 1199 | 14 / 13 | 252 / 241 | 3587 / 3500 | 33 / 30 | 204 / 183 | 34 / 33 | 516 / 496 |
| tolgee | 90 / 97 | 1042 / 1066 | 10 / 10 | 196 / 197 | 2082 / 1996 | 63 / 59 | 187 / 181 | 23 / 22 | 518 / 521 |

**Core samples (run 6).** Twenty runs on 11-13 cores from 1.5 s to 6.5 s (walk and parse), then on
0.86-1.36 cores in every sample from 7.0 s to 15.0 s. Photoprism and tolgee run on 10-13 cores up to
1.0 s, take one transition sample (6.4 and 5.0 cores), then run on 0.96-1.16 cores to the end. So every phase below is serial unless the row says otherwise.

## The sum check (acceptance: within 5% of `language_build` + `grafts` + `persist`)

The table has two numbers:
- **Leaf sum**: every timed leaf phase (the critical group's leaves, `plan_groups`, `fold_groups`, the GA.20
  graft steps, persist's own pre / post and the store leaves).
- **Stage sum**: plan + the critical group's `build_one` + fold, plus the graft steps and persist.

The language groups build on the pool at the same time, so `build_groups` is the critical (largest) group:
TS on twenty, Go on photoprism, `java` (Java + Kotlin) on tolgee. The other groups finish inside its span.

| repo / run | language_build + grafts + persist | stage sum | leaf sum | leaf coverage |
|---|---:|---:|---:|---:|
| twenty 5 | 7242 | 7240 | 7009 | 96.8% |
| twenty 6 | 6631 | 6630 | 6414 | 96.7% |
| photoprism 5 | 4355 | 4354 | 4292 | 98.6% |
| photoprism 6 | 4236 | 4235 | 4179 | 98.6% |
| tolgee 5 | 2796 | 2794 | 2753 | 98.5% |
| tolgee 6 | 2714 | 2712 | 2670 | 98.4% |

The un-timed rest is two residuals:
- **Builder residual**: the builder minus its passes, 87 / 71 ms on twenty and 9-22 ms elsewhere. That
  is dropping the merged call / ref / import vectors after the last pass.
- **Persist residual**: persist minus its timed leaves, 143 / 144 ms on twenty and 31-39 ms elsewhere.
  On twenty's big shard about 120 ms of it falls between the rkyv write and the encode's return: dropping
  the cloned core and code section.

## twenty (TS, 39,639 files): the three largest serial phases

**Graph front: the TS group, 2464-2783 ms, all on one core** (the group's build runs from 8.09 s to
10.55 s in run 6). Every row is single-core:

| phase | ms run 5 | ms run 6 | thread-CPU run 6 | shape |
|---|---:|---:|---:|---|
| merge_parses: loop (nodes, nav, edges into one graph) | 268 | 208 | 207 | per-file prep, then an in-order fold |
| merge_parses: fold_role_overlays | 162 | 118 | 117 | stateful (rewrites calls / refs) |
| build_symbol_table | 92 | 66 | 66 | read-only over nav, writes maps |
| same_stem_table | 25 | 19 | 19 | read-only |
| resolve_imports_ts: Barrels::read | 136 | 108 | 108 | read-only |
| resolve_imports_ts: materialize | 53 | 35 | 35 | stateful (writes module_symbols) |
| **resolve_imports_ts: statement loop** | **837** | **797** | 792 | read-only decide + in-order apply |
| ... of which the TS resolver callback, 207,786 calls | 602 | 581 | - | pure fn of (from_module, specifier) |
| ... of which ts_target_tagged, 158,105 calls | 40 | 37 | - | read-only |
| resolve_calls: decide (pooled, 470,919 sites) | 28 | 27 | 13 | read-only, pooled (FH.21) |
| resolve_calls: apply | 153 | 155 | 153 | stateful, in order |
| resolve_calls: marker tail | 58 | 55 | 54 | read-only counting |
| resolve_refs: SiteFiles | 65 | 61 | 60 | read-only |
| resolve_refs: decide (pooled, 58,882 refs) | 16 | 15 | 3 | read-only, pooled |
| resolve_refs: apply | 38 | 39 | 39 | stateful, in order |
| resolve_refs: marker + nav tail | 59 | 58 | 58 | mixed |
| resolve_inherited_calls | 49 | 48 | 48 | stateful |
| emit_override / method-level IMPLEMENTS | 4 | 4 | 4 | stateful |
| group: heritage + line tally | 121 | 118 | 117 | read-only counting |
| **group: fill_group_evidence** | **504** | **437** | 433 | read-only (per-node `at`, per-edge decide), emitters interned in order |
| **group total (build_one)** | **2783** | **2464** | 2421 | |

The "group front" the 0.5.6 plan names (from `[roles]` to `[imports] reexport`) is merge + symbol table +
same-stem + resolve_imports_ts: 1.36 s in run 6, against the plan's 1.40 s.

**Grafts: 1808 / 1539 ms, all on one core** (GA.20's steps; the shapes come from reading the code):

| step | ms run 5 | ms run 6 | shape |
|---|---:|---:|---|
| **imports_filter** (A16.4) | **696** | **593** | per-file, after one repo index (GA.20 memoizes it for watch only) |
| infer_wrappers | 380 | 309 | per-file candidate scan, then a repo-wide admission fixpoint (stateful) |
| wrapper_scan | 296 | 264 | per-file text scan |
| owner | 60 | 49 | - |
| modules | 67 | 57 | per-file |
| endpoint_fold | 53 | 46 | - |
| the 16 other steps | 254 | 220 | each at most 40 ms |

**Persist: 2625 / 2609 ms, all on one core.** twenty writes 13 shards. One of them, `-09`, the TS graph
(101,488 nodes, 420,727 edges, 123,278,736 bytes), takes 2419 ms of run 6's 2608 ms `write_layout`:

| part (run 6; sum over 13 shards, the big shard is ≥ 98% of each) | ms run 5 | ms run 6 | thread-CPU run 6 | shape |
|---|---:|---:|---:|---|
| **span_code_cells** (CD.7c CODE-span matching) | **1246** | **1255** | 1243 | per-node decide, interned in order |
| **intern_evidence** (CD.7b) | **369** | **380** | 375 | per-edge decode, in-order string table |
| code_core (clone into the container) | 294 | 276 | 272 | read-only over the graph |
| CodeSection::from_repo_graph | 181 | 167 | 167 | read-only (sorted flatten) |
| encode_file (rkyv core) | 152 | 151 | 149 | serial serialise |
| write_atomic (I/O) | 130 | 130 | 38 | I/O, the big shard 84 ms |
| encode_section(code) | 67 | 62 | 62 | serial serialise |
| cross_stack, manifest, hash, strings, raw checks, pre | 48 | 49 | | |
| un-timed residual (the drops, above) | 143 | 144 | | |

**The 3 largest serial phases on twenty:**
1. persist `span_code_cells`: 1246-1255 ms.
2. `resolve_imports_ts`'s statement loop: 797-837 ms (inside the 949-1036 ms resolver). Most of it is the
   TS resolver callback, 581-602 ms over 207,786 calls.
3. graft `imports_filter`: 593-696 ms.

Next come `fill_group_evidence` (437-504), `intern_evidence` (369-380), `merge_parses` (326-429 as a
whole), `infer_wrappers` (309-380), `code_core` (276-294) and `wrapper_scan` (264-296).

## photoprism (Go + Vue, 6,892 files): the three largest serial phases

The Go group is the critical path: 3498-3585 ms on one core. The `js,vue` group finishes in 35 ms beside it.

| phase | ms run 5 | ms run 6 | thread-CPU run 6 | shape |
|---|---:|---:|---:|---|
| merge_parses | 82 | 77 | 75 | per-file prep + in-order fold |
| bind_split_go_receivers, symbol table, embed partition, package names | 21 | 19 | 19 | |
| bind_go_embeds | 10 | 10 | 9 | |
| GoPackages::build | 12 | 12 | 13 | reads edges once |
| **resolve_imports_go** | **1414** | **1404** | 1394 | read-only decide + in-order apply |
| ... of which unique_global_module, 11,486 calls | 1398 | 1387 | - | a linear scan of `module_by_qname` per call |
| ... of which GoPackages::import_target, 17,292 calls | 2 | 2 | - | |
| resolve_go_calls: decide (pooled, 270,410 sites) | 13 | 15 | 12 | read-only, pooled |
| **resolve_go_calls: apply** | **1334** | **1310** | 1302 | stateful order; the hook is in apply (FH.21) |
| ... of which the GoPackages hook (`extra_hook`), 254,171 calls | 1254 | 1232 | - | see below |
| ... unique_global_type inside the hook, 97 calls | 17 | 13 | - | |
| go_mounts::bind | 32 | 28 | 28 | stateful |
| bind_type_handlers | 23 | 20 | 20 | stateful |
| **bind_value_refs** | **339** | **313** | 311 | stateful (binds / drops refs) |
| resolve_refs | 21 | 20 | 20 | |
| go_wire, alias sigs, implicit / method IMPLEMENTS | 31 | 27 | 27 | |
| group: heritage + line tally | 37 | 34 | 34 | |
| group: fill_group_evidence | 111 | 104 | 103 | read-only, as on twenty |
| **group total** | **3585** | **3498** | 3470 | |

**The hook's three arms** come from run 7 (1-min load 40, so wall time was inflated: `resolve_go_calls`
took 3139 ms wall against 2218 ms thread-CPU, and the hook 2655 ms wall). Read the split as a share:
- `typed_receiver` (CA.2b): 65%, 201,957 calls;
- `package_call`: 35%, 254,171 calls;
- `promoted_self`: 0.5%, 180,152 calls.

At run 6's 1232 ms that is about 800 ms in `typed_receiver` and 430 ms in `package_call`. In the
`impl GoPackages` block only `GoPackages::build` iterates `g.edges` (a grep, not a proof for every
callee). But the arms count into the `GoPackages.tally` / `promoted` counters, which a pooled decide
would have to merge per chunk. Whether every arm reads only frozen state is GP.3's split to prove.

Grafts (241-252 ms): `infer_wrappers` 136-143, `imports_filter` 37-38, the rest at most 17 ms each.
Persist (496-516 ms): span 114-117, intern 85-87, code_core 69-73, write 66-73, rkyv 47, the rest at most 45.

**The 3 largest serial phases on photoprism:**
1. `resolve_imports_go`: 1404-1414 ms, 98.9% of it `unique_global_module`'s tail fallback.
2. `resolve_go_calls`' apply: 1310-1334 ms, 94% of it the GoPackages hook.
3. `bind_value_refs`: 313-339 ms.

The probe's "2.2 s unattributed" is these: the import tail fallback and `bind_value_refs`.

## tolgee (Kotlin / Java + TS, 5,611 files): the three largest serial phases

The `java` group (Java + Kotlin through `build_dotted`) is the critical path: 1992-2078 ms. The TS group
finishes in 72-75 ms beside it.

| phase | ms run 5 | ms run 6 | thread-CPU run 6 | shape |
|---|---:|---:|---:|---|
| merge_parses | 58 | 58 | 57 | per-file prep + in-order fold |
| build_symbol_table, same_stem_table | 12 | 11 | 11 | |
| **resolve_imports_python** (the dotted import resolver) | **1850** | **1774** | 1766 | read-only decide + in-order apply |
| ... of which unique_global_function, 26,809 calls | 1815 | 1742 | - | a linear scan of every module's `module_symbols` per call |
| resolve_calls (decide pooled 5, apply 21) | 31 | 30 | 28 | |
| resolve_refs | 24 | 23 | 23 | |
| group: heritage + line tally | 16 | 15 | 16 | |
| group: fill_group_evidence | 77 | 70 | 69 | read-only |
| **group total** | **2078** | **1992** | 1979 | |

Grafts (196-197 ms): `imports_filter` 132-135, the rest under 10 ms each.
Persist (518-521 ms): span 203-205, intern 56-57, code_core 49-51, write 70-76, rkyv 38-40.

**The 3 largest serial phases on tolgee:**
1. `resolve_imports_python`: 1774-1850 ms, 98% of it `unique_global_function`'s tail fallback.
2. persist `span_code_cells`: 203-205 ms.
3. graft `imports_filter`: 132-135 ms.

Resolution (`resolve_calls` + `resolve_refs`) is 53 ms. The probe's "unmarked JVM pass, about 2.4 s" is
the import tail fallback.

## What GP.2 / GP.3 / GP.4 read from this (the 200 ms rule)

**GP.2 (TS front).**
- (a) Keep `resolve_imports_ts`' decide: the statement loop is 797-837 ms. Its biggest single cost is the
  engine's resolver callback (`lang_build::resolve_ts_source_aliased`), 581-602 ms for 207,786 calls. That
  callback is a pure function of (from_module, specifier), so pooling it or memoizing it per distinct
  pair both qualify.
- (b) Keep `merge_parses`: 326-429 ms (loop 208-268, `fold_role_overlays` 118-162).
- (c) **Drop** `build_symbol_table`: 66-92 ms, under 200.
- Not in GP.2's list but over 200 and serial: `fill_group_evidence` 437-504 ms (read-only, the engine's
  `passes.rs`) and graft `imports_filter` 593-696 ms.

**GP.3 (Go / JVM fronts).**
- Go: the hook is 1232-1254 ms, so keep it.
- The JVM front's "2.4 s" is `resolve_imports_python`'s tail fallback (`unique_global_function`, 26,809
  calls). It is not per-file work that wants a pool.
- Go's `resolve_imports_go` has the same shape (`unique_global_module`, 11,486 calls, 1.39 s).
- Both scans read only `module_by_qname` / `module_symbols`. By reading the code, neither import loop writes
  them: the loops write edges and `module_import_bindings`. So one tail index built before the loop answers
  each call alike. That is cheaper than a pool and removes about 3.1 s of single-core time across the two
  repos (1.39 s + 1.74 s).
- `bind_value_refs`, 313-339 ms on photoprism, is the next Go item.

**GP.4 (persist).**
- Keep both halves on twenty: span matching 1246-1255 ms, interning 369-380 ms.
- On photoprism and tolgee both halves are under 200 except tolgee's span matching (203-205 ms), so twenty
  is GP.4's measuring repo, as its spec says.
- Also serial and over 200 on twenty: `code_core` (276-294 ms, the container clone).
- Under 200: rkyv `encode_file` 151 ms and `CodeSection::from_repo_graph` 167-181 ms.

Not in any GP packet's scope, from the same runs: twenty's `post` (813-896 ms) and `resolve` (670-727 ms)
stages, which `[timing] build` already attributes per pass (`slowest_pass=tag_synthetic_provenance`).
