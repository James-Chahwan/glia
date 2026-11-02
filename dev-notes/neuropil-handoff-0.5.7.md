# neuropil handoff - glia 0.5.6 -> 0.5.7

For: neuropil. **Pin: `=0.5.7`** on the six glia path crates (`glia-py==0.5.7` for any Python tooling). Apply
after the 0.5.4, 0.5.5 and 0.5.6 handoffs (`dev-notes/neuropil-handoff-0.5.4.md`,
`dev-notes/neuropil-handoff-0.5.5.md`, `dev-notes/neuropil-handoff-0.5.6.md`).

**Status: written at glia HEAD `01a74953` (after wave W12 of 0.5.7), before the bump.** The version bump and the
`v0.5.7` tag are James's release steps after these docs; take the pin **once James tags `v0.5.7`** (at HEAD the
crates read 0.5.6, so `version = "=0.5.7"` on a path dependency fails resolution until the tag).

**Where neuropil stands.** Read read-only at HEAD `b140d1e` (the commit every handoff since 0.5.3 read): its
root `Cargo.toml:112-117` still path-depends on the pre-0.5.0 names (`repo-graph-core` ..
`repo-graph-projection-text`), so the 0.5.0 rename (section 2 of `neuropil-handoff-0.5.4.md`) is still the
first step. Act on neuropil only.

0.5.7 is the first half of the 0.5.7 scope (plan `dev-notes/next-leap-0.5.7.md`, packets
`dev-notes/leap-057-packets.json`, waves W0-W13): HP proof and process, HF flows (declared entries seed entry
flows, MCP servers as a host, Rails nested resources), HT build-target graphs (id blocks per domain, the bazel
snapshot / parser / targets sidecar / answers), HC calibrated confidence, HQ the confidence run's false-edge
fixes. No new id in the code block (next free stays node 51 / edge 39 / cell 28), no `FORMAT_VERSION` (3) or
`MANIFEST_VERSION` (2) change, no Cargo.lock move in glia.

Sources, in order of authority: the compile check below; the code at `01a74953`; each packet's landed result
(its `breaking` text) and commit body; the `HANDOFF FROM ..` paragraphs routed to HZ.2 in
`dev-notes/leap-057-corrections.json`. The hash after a packet id is the glia commit that landed it.

---

## 1. The compile check at this commit

`CARGO_BUILD_JOBS=8 bash scripts/check-neuropil.sh`, run as-is against `../neuropil` and this glia tree (it
copies neuropil into `~/.cache/glia-consumer-check/`, applies the 0.5.0 rename to the copy, and runs
`cargo check --workspace --all-targets`; nothing under neuropil is written):

```
[rename-0.5.0] rewrote 165 token(s) in 50 file(s) under /home/ivy/.cache/glia-consumer-check/neuropil/src (0 unknown token(s) left)
[neuropil-check] note - neuropil still uses the pre-0.5.0 names; checked a renamed copy
[neuropil-check] ok - new=0 fixed=0 preexisting=0 in 29.0s against glia 0.5.6+p5e972198f1784f17 (neuropil b140d1e, lock drift +45 -26)
```

- **`new=0`**: neuropil compiles against the 0.5.7 tree once the 0.5.0 rename is in. None of the Rust API
  changes in section 3 reaches a neuropil source file.
- **Lock drift +45 / -26**, as at 0.5.6. No fastembed, ort or tokenizers entry enters neuropil's lock.

## 2. Cargo: the pin

| where | change |
|---|---|
| `Cargo.toml:112-117` | after the 0.5.0 rename (`repo-graph-<x>` -> `glia-<x>`, projection-text feature `driver` -> `research`): `version = "=0.5.7"` on `glia-core`, `glia-code-domain`, `glia-graph`, `glia-engine`, `glia-activation`, `glia-projection-text` |
| `crates/neuropil-app/Cargo.toml` | nothing beyond the 0.5.0 rename |

**Stamps.** PARSER_STAMP / BUILD_STAMP moved (core, activation, code-domain, graph, engine, parsers and
build-domain edits): the first `generate_one` after the upgrade reparses, and a `.glia/graph/` layout written
by another build rebuilds once.

## 3. Rust API changes, against neuropil

None needs a neuropil edit today (section 1). These are the changes an in-process caller could meet:

| packet | change | who must act |
|---|---|---|
| HT.1 id blocks (`b6f09101`) | new `glia_core::ids` (`BLOCK_SHIFT`, `block_of`, `position_of`, `id_in`, `is_private`, `CODE` / `CORE` / `BUILD` / `GOAL` / `PRIVATE` / `NEVER`, `BlockStatus`, `Block`, `BLOCKS`, `block`, `block_name`, `check_table`) and `DomainTables::validate_blocks`. Every code id keeps its value (block 0); build-target ids are `0x0002_xxxx` | a view that hard-codes small ids should read names from a file's header once it reads a build-target file |
| HT.2 ids on the surfaces (`f0864aa0`) | `glia_store::NamedCount` gains pub field `block` and `id_label()` (`#[non_exhaustive]`, additive); `glia inspect --json` rows gain `block` | a parser of `glia inspect --json` |
| HC.4 precision beside the tier (`ffa6454d`) | new `glia_engine::confidence` (`Precision`, `Table`, `of_edge`, `lookup`, `weakest`, `strongest`, `tier_text`, `Tally`, `Annotated`, `finish`, `report`, `AUDIT`, `MIN_SPECIFIC`); `EdgeWhy`, `ViolationEdge`, `Violation`, `QueryRow`, `Implementor`, `ReviewCounts` gain a `precision` field (all `#[non_exhaustive]`) | none |
| HT.6, HT.8 build targets in the engine (`36b96b0e`, `8edfc013`) | new `glia_engine::targets` (`load_or_build`, `affected_vs_rev`, `sidecar_path`, `targets_repo`, `TargetsView`, `TargetHit`, `Affected`, `TARGETS_GMAP`, `BUILD_TABLES`, `targets_dir`, ..); `Review` and `DiffImpact` (`#[non_exhaustive]`) gain `pub targets: Option<Affected>`, filled only with a `.glia/targets-snapshot/` | none |
| HQ.12 model identity (`583f91ca`) | `glia_graph::ModelJoin` gains pub field `rule` and `model_rested()` (`#[non_exhaustive]`) | none |
| HQ.13 C# usings (`96a17709`) | `glia_code_domain::NavFact` gains the appended variant `CsUsing`; new `CsUsingForm`, `csharp_using_bindings`; `ImportBinding` gains `form` and `global` (`#[non_exhaustive]`). NavFacts live only in the parse cache | an exhaustive `match` on `NavFact` (none in neuropil) |
| HF.3, HF.4 MCP hosts (`cd3e04a3`, `5a2a7040`) | `glia_code_domain::hosts`: `decorator_registration_for(lang, method)` -> `(lang, method, held)`; `Shape::Decorator` gains `literal_name: bool`; new `Shape::Prefixed`, `HandlerAt` (`#[non_exhaustive]`), `HostRow.instances`, `HostRow::{instance_class, is_instance_path}`, `Shape::prefixed_name` | a pattern on `Shape::Decorator { .. }` without `..` (none in neuropil) |
| HQ.5 nav mounts (`e0d19816`) | `glia_code_extractors::nav_routes::extract_route_tables(source, module_id, module_qname, repo)` gains `module_qname`; `NavRouteOut` gains `pub mounts: Vec<NavFact>`; `glia_code_domain::endpoint` gains `page_qname`, `nav_mount_link`, `parse_nav_mount_link`, `nav_mount_route_qname`, `parse_nav_mount_route_qname` | none (the extractors crate is not a neuropil dependency) |
| HQ.8 test doubles (`e0222291`) | `glia_code_extractors::data_sources::extract_data_source_nodes` gains a 5th argument `test_file: bool` | none |

## 4. What re-keys (persisted view state keyed by qname orphans)

| packet | old -> new |
|---|---|
| HF.5 Rails nested resources (`e2ac26ea`) | ROUTEs under a `resources` block nest (`ANY /api/v1/notifications` -> `ANY /api/v1/accounts/:account_id/notifications`); new `ANY /<r>/:id`, member / collection / `on:` routes |
| HQ.2 C# verb + `[Route]` (`eda7a215`) | `X <prefix>` + `ANY /t` -> one `X /t`; the shared `<VERB> /` ROUTEs are gone |
| HQ.5 Angular lazy child tables | `page:/:schemaName` -> `page:/app/:appName/schemas/:schemaName` (squidex); NodeIds, HANDLED_BY / NAVIGATES_TO move with them |
| HQ.6 computed parent paths (`77a64cc1`) | `page:/list` -> `page:/~frontendURL/accounts/:accountId/settings/custom-roles/list`: a page qname may hold a `~<name>` opaque head segment; render it as is |
| HQ.14 JPQL `JOIN FETCH` (`b609daa1`) | `data_entity:sql:fetch` is gone; the joined entity's `data_entity:` node gains the edge |

## 5. Graph content: new, removed and re-stamped (no code change)

- **New qname families** from MCP servers as a host (HF.3, HF.4): CLI_COMMAND `host_cmd:mcp:tool:<name>` /
  `host_cmd:mcp:prompt:<name>`, EVENT_HANDLER `host_event:mcp:resource:<uri>` / `host_event:mcp:<handler>`, and
  for a TypeScript inline handler a FUNCTION `<module>::command tool:<name>` / `<module>::on resource:<uri>`.
  A HUD that colours by qname prefix already knows `host_cmd:` / `host_event:` from 0.5.6.
- **Entries** (HF.1 `f70f3957`): a node carrying an ENTRYPOINT cell is now an entry-flow seed, and
  `nodes_json`'s `entry` reads true for it: an `entry` view shows more nodes on repos with host starts, scripts,
  `__main__` guards or overlay `[entrypoints]`. A repo with no ENTRYPOINT cell is unchanged.
- **Heritage** (HQ.1 `b383c243`): no INHERITS_FROM / IMPLEMENTS ends at a PACKAGE or MODULE. C# heritage binds
  through usings at fact tier (rules `nested_type`, `enclosing_namespace`, `using_namespace`, `using_static`;
  was heuristic `global_unique`) and a namespace `using` IMPORTS its PACKAGE (rule `using_namespace`); C#
  IMPORT_BINDINGS rows gain `form` / `global`, with namespace rows' `name` null (HQ.13).
- **Solidity** (HQ.3 `d50142cb`, HQ.4 `c2095fee`): no EVENT_FLOWS edge ends at a missing node; inherited /
  qualified emits are `graph:solidity` EVENT_FLOWS with rules `inherited_event` / `qualified_event` (and
  `_by_name`, heuristic); a Solidity EVENT_FLOWS caveat row exists. Every RPC_PROCEDURE HANDLED_BY carries an
  explicit EVIDENCE with basis `site` (was `from_node`).
- **Fewer edges:** DOCUMENTS (HP.4 `313e7d93`: rule / key / kind positions, key chords, private Rust fns from
  outside their project, bare cross-project mentions), Python ATTRIBUTE / HAS_ATTRIBUTE for own members and
  inherited reads (HQ.9 `9d779ba9`), same-stem TESTS that tie or lose to a nearer candidate (HQ.10 `793dca86`),
  ACCESSES_DATA from mocked store clients in tests (HQ.8), Redis pub/sub nodes from `redis` inside a longer
  identifier (HQ.7 `f1b7e38d`), http_model SAME_AS classes resting only on a model-written overlay stanza
  (HQ.12).
- **Rule names:** a pairing through an unmarked or `origin = "llm"` overlay stanza reads EVIDENCE rule
  `<rule>/overlay_llm` and tiers heuristic (HC.1 `104fd310`); an llm `[[wrapper]]`'s ENDPOINT_HIT gains a
  trailing `"overlay":"wrapper#<n>"`. A reader of rules verbatim strips the suffix for the base rule.
- **Build targets** (HT.4-HT.7) live in a separate sidecar `.glia/graph/targets.gmap` with block-2 ids, written
  only by a targets answer; a code layout never holds one.
- The full per-packet list is section 4 of `dev-notes/repo-graph-handoff-0.5.7.md`.

## 6. Optional: new in-process primitives

- `glia_engine::confidence::of_edge(&edge)` for an inspector strip that shows an audited precision beside an
  edge's tier (the shipped table, audit `ac3224456ffa`), released by HC.3's calibration
  (`CONFIDENCE: R1=yes (25/25) R2=yes (0.0218) R3=yes (25) -> PASS`, `dev-notes/confidence-gate-0.5.7.md`).
- `glia_engine::targets::load_or_build(root)` for a Bazel repo's build-target graph, once
  `glia targets sync` wrote `.glia/targets-snapshot/`.

## 7. Re-test

1. Apply the 0.5.0 rename, set `=0.5.7` (after the tag), `cargo check --workspace --all-targets`.
2. Open a Rails or Angular (`loadChildren`) repo: route and page labels carry their nested / prefixed paths, and
   a computed parent reads `~<name>`.
3. Open a repo with an MCP server: its tools appear as `host_cmd:mcp:tool:<name>` nodes joined to their handlers.

## 8. Not in 0.5.7

The precision claim fails its committed rule (`[edge-audit] rule 0.5.7b verdict=FAIL
under_floor=ACCESSES_DATA|fact fact=0.999 derived=0.991 heuristic=1.000 kappa=1.000`, one wrong edge, fixed by
HQ.14 after the draw; `dev-notes/edge-precision-0.5.7.md`). No goal layer: `glia explain` is 0.5.8, a small
release of its own, and the goal layer (GOAL_UNIT / TRACES / UNIT get their emitter) is 0.5.9. Still no Rust
body-only watch path, no `glia place`, no docs-as-a-second-graph rows.

## 9. Packet index

Declared changes listed here: HC.1 (s5), HC.4 (s3, s6), HF.1 (s5), HF.3 / HF.4 (s3, s5), HF.5 (s4), HP.4
(s5), HQ.1 (s5), HQ.2 (s4), HQ.3 / HQ.4 (s5), HQ.5 (s3, s4), HQ.6 (s4), HQ.7 / HQ.8 / HQ.9 / HQ.10 (s5; HQ.8
also s3), HQ.11 (a `ruby` HANDLED_BY caveat row in coverage and absences), HQ.12 (s3, s5), HQ.13 (s3, s5),
HQ.14 (s4), HT.1 / HT.2 (s3), HT.6-HT.8 (s3, s5). HT.7's CLI and pyo3 surface (`glia targets ..`,
`targets_sync`, `PyGraph.targets_*`) and HC.4's `precision=` keywords / `--no-precision` are in section 2 of
the repo-graph handoff. Process or bench only, nothing for neuropil: HP.1, HP.2, HP.3, HP.5, HP.6, HP.7, HP.8,
HC.2, HC.3, HF.2, HT.3, HT.4, HT.5, HT.9, HT.10.
