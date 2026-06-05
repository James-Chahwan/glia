# Cycle 4.0 Full SWE-bench Lite — Pre-Launch Report

Date: 2026-05-24 evening (pacific)
Pod: 7× A40 @ root@194.68.245.36:22071 (~$2.20/hr cluster)
Stack: Sprint 17-29 cumulative (FIM + shape classifier + per-instance retrieve probe + diagnostic markers)

## Where we are

### Cumulative unique strict-PASS instances ever: 13

Across 11 cycles (3.0 → 3.12) the model has produced strict PASS at least once on:

1. marshmallow-1359 — baseline (cycle 0+) / restored cycle 3.11
2. marshmallow-1343 — cycle 3.7 NEW via retrieval / variable
3. pytest-11143 — cycle 2.1 / restored cycle 3.12 via Sprint 28 L7
4. pytest-7373 — cycle 3.2
5. pytest-11148 — cycle 3.8 NEW
6. requests-863 — cycle 3.5 (via FIM directly, NOT B1 as previously misattributed)
7. xarray-5131 — cycle 3.5 (variable)
8. django-11039 — cycle 3.8 NEW
9. django-11099 — cycle 3.8 NEW
10. django-11049 — cycle 3.8 NEW
11. seaborn-3010 — cycle 3.8 NEW
12. xarray-4248 — cycle 3.11 NEW
13. django-16139 — cycle 3.11 NEW

### Cycle-by-cycle PASS counts

| Cycle | Bench size | Done | PASS | Stack |
|-------|-----------|------|------|-------|
| 3.7 | 50 | 24 (killed) | 4 | Sprint 14-16 |
| 3.8 | 50 | 49 | **7** (5 NEW unique) | Sprint 17-25 |
| 3.10 | 14 | 12 (killed) | 1 | Sprint 26 |
| 3.11 | 14 | 14 | **6** (2 NEW unique) | Sprint 27.1 |
| 3.12 | 14 | 14 | 5 | Sprint 28 |
| 3.13 | 14 | in flight | TBD | Sprint 29 |

## Sprint 29 stack — what's NEW vs cycle 3.12

### Fixes (all silent-dead-code bugs found in 3.12 forensics)

| # | Bug | Fix |
|---|-----|-----|
| 29.1 | Sprint 28 L14 `shape_decision.json` never written (workdir was NameError caught silently) | Thread `workdir` from `fim_edit` → `build_fim_prompt` |
| 29.2a | Sprint 26 B1 raw-diff fallback never fires — needs APPLY-FAIL which doesn't happen on PASSes | Always-on `B1 GATE:` log + `test_result['b1_gate_eligible']` marker |
| 29.2b | Sprint 26 B4 PARTIAL bridge never fires | Always-on `B4 GATE:` log + `test_result['b4_gate_fired']` marker |
| 29.5 | Sprint 25 C1 body_delta read wrong field (`fim_meta` doesn't exist) | Multi-source body_chars lookup |

### New uplift

| # | What | Mechanism |
|---|------|-----------|
| 29.3 | **Per-instance retrieve probe** | When shape strategy says retrieval_k=0 (FOCUSED_BUGFIX), still probe for a same-tail same-repo peer (min_score=15). If exists, upgrade to 1 exemplar. **Should restore marshmallow-1343 + xarray-4248** which had 3.11 PASS but 3.12 FAIL because L7 fully disabled their guidance. |

### Diagnostic discipline

Every speculative path now has a fired_on_inst marker so we can grep
cycle results JSONL for actual usage. No more "I assumed B1 was the
mechanism" misattributions.

## Cycle 3.13-sentinels expected outcome (validation before full bench)

| Instance | 3.11 (Sprint 27.1) | 3.12 (Sprint 28) | 3.13 prediction |
|----------|---|---|---|
| pytest-11143 | FAIL | PASS (L7) | PASS (kept — no peer) |
| pytest-11148 | PASS | PASS | PASS |
| marshmallow-1359 | PASS | PASS | PASS |
| **marshmallow-1343** | PASS | FAIL | **PASS** (probe restores marshmallow-1359 peer) |
| **xarray-4248** | PASS | FAIL | **PASS** (probe restores some peer) or FAIL (no high-quality peer) |
| xarray-5131 | PARTIAL | PARTIAL | PARTIAL → maybe PASS via B4 (if 29.2 reveals it now fires) |
| requests-863 | PASS | PASS | PASS |
| django-16139 | PASS | PASS | PASS |
| pytest-7373 | FAIL reg=81 | FAIL reg=81 | FAIL (content problem, no fix in 29) |
| flask-4045 | FAIL no-op | FAIL no-op | FAIL |
| astropy-14995 | FAIL reg=174 | FAIL reg=174 | FAIL |
| pylint-7993 | FAIL reg=7 | FAIL reg=7 | FAIL |
| xarray-4094 | FAIL reg=1 | FAIL reg=1 | FAIL |
| sphinx-7686 | FAIL no-op | FAIL no-op | FAIL |

**Target: 7-8 PASS in 3.13.** If we hit it, Sprint 29 wins and full bench is justified.

## Cycle 4.0 full SWE-bench Lite plan

### Composition

323 instances total (300 test + 23 dev). Stratified across 14 repos:
- django/django ~50
- sympy/sympy ~40
- matplotlib/matplotlib ~22
- scikit-learn/scikit-learn ~22
- pytest-dev/pytest ~20
- sphinx-doc/sphinx ~20
- pydata/xarray ~20
- astropy/astropy ~22
- psf/requests ~6
- pylint-dev/pylint ~12
- pallets/flask ~11
- mwaskom/seaborn ~4
- marshmallow-code/marshmallow ~6
- pylint-dev/astroid ~6
- (remainder smaller repos)

### Resource

- **GPU**: 7× A40, 1 daemon per GPU pinned via CUDA_VISIBLE_DEVICES
- **Sharding**: sha1(instance_id) % 7 → ~46 instances per shard
- **Per-instance wall**: ~5-7 min average (FIM + 3 iters + apply+test)
- **Imbalance**: ~12% from sha1 distribution
- **Expected wall**: ~5-6 hours
- **Expected cost**: ~$13-15 ($2.20/hr × 6h)
- **Budget remaining**: ~$35 → comfortable

### Expected PASS count

Per-shape extrapolation from cycle 3.8 + 3.11 data:
- **FOCUSED_BUGFIX shape** (marshmallow/pytest tight bodies): ~30% PASS rate
- **IDIOM_MATCH shape**: ~25% (Sprint 28 idiom pool + 29.3 probe should help)
- **FEATURE_ADD shape** (django/sympy big bodies): ~15-20% (cycle 3.8 django sweep was the 3-in-row that proves this works)
- **BROAD_REFACTOR**: ~10%

Naive estimate: **50-70 PASS** (15-22% rate on 323).

Conservative band: **40-55 PASS** if Sprint 29.3 probe over-fires.

## What we'll measure mid-run

H1 live cycle analyzer (shipped Sprint 25) emits a body_delta_3.13.md
and an analogous file for 4.0. Per-cycle dashboard:
- Per-shape PASS rate (Sprint 28 L9, now actually working post-29.1)
- B1 / B4 gate-fired counts (post-29.2 markers)
- 29.3 probe upgrade count
- Cumulative wall + ETA

Mid-run alert: if PASS rate < 10% by 50 instances done, abort early
(save GPU $, fix root cause).

## What we'll fix/improve post-bench

Already known content-fail classes that need Sprint 30+:
1. **pytest-7373 / flask-4045 wrong-direction content**: model picks a
   bad fix direction despite SHAPELESS conditions. Need pre-FIM scope
   inference call OR different exemplar tier OR 32B fallback.
2. **astropy-14995 reg=174 persistence**: content cascades through
   astropy's tightly-coupled code. Needs diff-hunk exemplars (store
   the diff that PASSed, not the full body) or smaller targets via
   multi-region FIM.
3. **xarray-4094 near-miss reg=1**: model needs the literal failing
   assertion in the hint (B3 SHOULD already do this, may not be plumbed
   correctly — investigate post-bench).
4. **iter test_callback vs outer apply_and_test discrepancy**: cycle
   3.12 marshmallow-1359 was scored 0 by iter test but PASS by outer
   test on same diff. State isolation bug. Investigate via iter outcome
   vs outer outcome log in 3.13 data.

## Launch sequence

1. Wait for cycle 3.13-sentinels (~22min wall)
2. Confirm 3.13 score ≥ 6 (i.e. Sprint 29 didn't regress)
3. Reset pod state (kill any lingering daemons, clear sockets)
4. Launch `cycle_4_0_spawn5.sh` (despite name, takes N_SHARDS=7 env)
5. Monitor with single ssh-tail monitor
6. Generate per-50-instance analysis snapshots

## Risk register

| Risk | Mitigation |
|------|-----------|
| Sprint 29.3 probe regresses PASS count | cycle 3.13 catches before full bench |
| Pod connectivity drops mid-run | crash_guard logs silent failures; per-shard JSONL preserved |
| One shard hangs on a slow instance (sympy timeout) | 1800s timeout per instance; spawn5 doesn't block on individual |
| Disk fills with per-instance workdirs | 100 GB volume buffer + workdirs only retained for forensics |
| GPU OOM from per-shard CUDA pin | already validated in cycle 3.8-3.12 (each daemon 17-18 GB; pathB peaks at +14 GB on same GPU = 32 GB on 46 GB A40) |
