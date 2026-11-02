# engram handoff - glia 0.5.6 -> 0.5.7

For: Engram (engram-export, `--writer engram`). **Pin: `=0.5.7`** (the engram-export exporter built at the
`v0.5.7` tag) / `glia-py==0.5.7`. Apply after the 0.5.4, 0.5.5 and 0.5.6 handoffs
(`dev-notes/engram-handoff-0.5.4.md`, `dev-notes/engram-handoff-0.5.5.md`, `dev-notes/engram-handoff-0.5.6.md`).

**Status: written at glia HEAD `01a74953` (after wave W12 of 0.5.7), before the bump.** The version bump and the
`v0.5.7` tag are James's release steps after these docs; at HEAD the workspace and `engram-export/Cargo.toml`
read `0.5.6`. Engram was read read-only at HEAD `e6ae2bf` (5 dirty paths), the commit the 0.5.4 and 0.5.6
handoffs read. Act on Engram only.

**What reaches Engram: graph content only.** `engram-export/src/` is unchanged since `v0.5.6`; only two of its
tests moved (`dd0c302b`: `tests/hints_side_v6.rs` and `tests/identity_v6.rs` rebuild their METHOD / ATTRIBUTE
twin as a class-level attribute, because HQ.9 removed the `self.X`-read twin they rode on).
`GMAP_FORMAT_VERSION` stays 6, glia's `FORMAT_VERSION` 3 / `MANIFEST_VERSION` 2 stand, no id is allocated in
the code block (next free node 51 / edge 39 / cell 28; HT.4's 12 ids are in block 2 and never reach a code
layout), and no 0.5.7 packet edits an Engram file. The exporter check passes at this commit:

```
$ CARGO_BUILD_JOBS=8 bash scripts/check-engram-export.sh
[engram-export] check: ok - build + 101 tests passed against engram-core GMAP_FORMAT_VERSION=6
```

0.5.7 is the first half of the 0.5.7 scope (plan `dev-notes/next-leap-0.5.7.md`, packets
`dev-notes/leap-057-packets.json`, waves W0-W13): HP proof and process, HF flows (declared entries seed entry
flows, MCP servers as a host, Rails nested resources), HT build-target graphs, HC calibrated confidence, HQ the
confidence run's false-edge fixes. Sources, in order of authority: the code at `01a74953`; each packet's landed
result (its `breaking` text) and commit body; the `HANDOFF FROM ..` paragraphs routed to HZ.2 in
`dev-notes/leap-057-corrections.json`. The hash after a packet id is the glia commit that landed it.

---

## 1. TL;DR

1. **Pin `=0.5.7`**: build the exporter at the `v0.5.7` tag once James tags it; `glia-export-engram -V` then
   prints `glia-export-engram 0.5.7 (build 0.5.7+p<stamp>)`. PARSER_STAMP / BUILD_STAMP moved, so
   `<out>.meta.json`'s stamps move and the first export reparses.
2. **No Engram code change is required.** Every change is graph content: facts keyed by the identity hint of a
   re-keyed or removed node orphan, and new facts appear (sections 2-3). `--since` pairs re-keyed endpoints and
   renamed symbols as before (DE.3 / DE.4), so a `--since` chain across the upgrade reports them as renames
   where it can and as add + remove where it cannot.
3. **Two rule-name moves a verbatim reader meets** (section 4): the `/overlay_llm` suffix on pairings that rest
   on a model-written overlay stanza (HC.1 `104fd310`), and C# heritage / IMPORTS rules from heuristic
   `global_unique` / `tail_unique` to fact `nested_type` / `enclosing_namespace` / `using_namespace` /
   `using_static` (HQ.13 `96a17709`).
4. **Python ATTRIBUTE facts drop** (HQ.9 `9d779ba9`): the METHOD / ATTRIBUTE qname twins are gone, so a hint
   (LB.6 v1, `<out>.hints.jsonl`, the DT.12b `cell set --hint` key) that named a removed ATTRIBUTE twin no
   longer binds; the export's key winner for such a qname was and stays the METHOD.

## 2. Re-keyed and removed facts (they orphan)

| packet | what moves |
|---|---|
| HF.5 Rails nested resources (`e2ac26ea`) | ROUTEs under a `resources` block re-key (`ANY /api/v1/notifications` -> `ANY /api/v1/accounts/:account_id/notifications`); new `ANY /<r>/:id`, member / collection / `on:` / symbol-verb routes; HTTP_CALLS pairs move |
| HQ.2 C# verb + `[Route]` (`eda7a215`) | an action with `[HttpX]` and `[Route(t)]` is `X /t` (was `X <prefix>` plus `ANY /t`); the shared `<VERB> /` ROUTEs are removed (squidex: 108 `GET /` HANDLED_BY -> 1); ApiExplorer / Swashbuckle ops on such actions become keyed (squidex 4 -> 204), so `contract_op` identity keys appear |
| HQ.5 Angular lazy child tables (`e0d19816`) | `page:` ROUTEs of a `loadChildren` table gain their parent prefix (squidex `page:/:schemaName` -> `page:/app/:appName/schemas/:schemaName`); `page:` ROUTEs anchor identity hints, so their facts re-key; a lazy table's `''` page no longer merges into `page:/` |
| HQ.6 computed parent paths (`77a64cc1`) | `page:/list` -> `page:/~frontendURL/accounts/:accountId/settings/custom-roles/list`: a page qname may hold a `~<name>` opaque head segment; the computed parent itself becomes a page (chatwoot: 69 -> 178 page ROUTEs); such pages stop merging across files |
| HQ.9 Python attributes | ATTRIBUTE facts for a class's own METHOD / property read or assigned through `self`, and read-only `self.X` in a class with a base other than object / ABC / Protocol, are removed (mealie: HAS_ATTRIBUTE 3513 -> 2695, own-method twins 388 -> 0) |
| HQ.14 JPQL `JOIN FETCH` (`b609daa1`) | `data_entity:sql:fetch` removed; the entity a `join fetch <Entity>` names gains the ACCESSES_DATA |
| HQ.8 test doubles (`e0222291`) | a test module that mocks a store client, or names one only in a camel-middle identifier or quoted text, loses its ACCESSES_DATA -> `data_source:*`; a DATA_SOURCE no other module reaches is removed |
| HQ.7 Redis word gate (`f1b7e38d`) | Redis pub/sub QUEUE_CONSUMER / QUEUE_PRODUCER facts from a file naming `redis` only inside a longer identifier are removed (tolgee: `queue_consumer:unresolved:redispubsub @webapp`) |

## 3. New facts and edges

- **MCP servers as a host** (HF.3 `cd3e04a3`, HF.4 `5a2a7040`): CLI_COMMAND `host_cmd:mcp:tool:<name>` /
  `host_cmd:mcp:prompt:<name>` and EVENT_HANDLER `host_event:mcp:resource:<uri>` / `host_event:mcp:<handler>`,
  HANDLED_BY their handler (EVIDENCE `parser:python` rule `host_decorator`, `parser:typescript` rule
  `host_handler`); a TypeScript inline handler is its own FUNCTION `<module>::command tool:<name>` /
  `<module>::on resource:<uri>` with a DEFINES from the MODULE, and owns the handler's calls.
- **Solidity events** (HQ.3 `d50142cb`): no EVENT_FLOWS edge ends at a missing node any more (zora-protocol:
  216 dangling -> 0, EVENT_FLOWS 291 -> 215). An own-event emit stays the parser's fact edge; an inherited emit
  is a `graph:solidity` EVENT_FLOWS to the base's EVENT_EMITTER (rule `inherited_event`, or
  `inherited_event_by_name`, heuristic), a qualified emit to the named type's event (`qualified_event` /
  `qualified_event_by_name`), else nothing (an UnresolvedRef the layout keeps). Anything that counted
  `(unknown:..)` EVENT_FLOWS targets or assumed every EVENT_FLOWS is `parser:solidity` fact must update.
- **Heritage** (HQ.1 `b383c243`): no INHERITS_FROM / IMPLEMENTS ends at a PACKAGE or MODULE (squidex: -48,
  +32 heuristic to the type); PHP gains the in-repo bases DH.14 used to skip. C# (HQ.13): 1460 edges restamp to
  fact rules on squidex, and a base the file's scope does not hold, in a file importing an undeclared
  namespace, binds nothing.
- **C# IMPORTS** (HQ.13): a namespace `using` IMPORTS its PACKAGE, rule `using_namespace`, fact (squidex 7033;
  was 1458 `tail_unique` heuristic); alias usings IMPORT their type or PACKAGE.
- **TESTS** (HQ.10 `793dca86`): a test with several same-stem candidates pairs the nearest (import, then shared
  directory names, trailing run, common prefix); more than 3 ties pair nothing. Fewer, better TESTS facts.
- **DOCUMENTS** (HP.4 `313e7d93`): fewer; rule / key / kind positions, key chords, private Rust fns from
  outside their project and bare cross-project mentions bind nothing; some picks move to a public twin.
- **Identity** (HQ.12 `583f91ca`): an http_model SAME_AS class that rested only on a model-written overlay
  pairing is no longer written.
- **Solidity HANDLED_BY** (HQ.4 `c2095fee`): every RPC_PROCEDURE HANDLED_BY carries an explicit EVIDENCE at its
  handler declaration with basis `site` (was `from_node` at the procedure's POSITION); twin contracts each get
  their own copy's file and line.

## 4. Cells and rules in the gmap

- **EVIDENCE rules.** HC.1: a pairing whose client end or route mount rests on an unmarked or `origin = "llm"`
  `[[wrapper]]` / `[constants]` / `[[route_prefix]]` stanza reads `<rule>/overlay_llm` (`exact/overlay_llm`,
  `base_fold/overlay_llm`, `pattern/overlay_llm`, `binding/overlay_llm` ..) and tiers heuristic; strip the
  suffix for the base rule. HQ.13 / HQ.3 / HQ.4 add the rules in section 3.
- **ENDPOINT_HIT** (HC.1): the hits of an ENDPOINT an llm `[[wrapper]]` minted gain a trailing
  `"overlay":"wrapper#<n>"`. engram-export's site reader (`engram-export/src/lib.rs`, the ENDPOINT_HIT
  `file` / `line` / `col` reader) ignores extra keys; nothing to change.
- **IMPORT_BINDINGS on C#** (HQ.13): every `using` is a row, in-repo or external; rows gain `form`
  (`namespace` / `static` / `alias`) and `global`, and a namespace row's spec is the whole namespace with
  `name` null (squidex: modules 964 -> 2464, rows 1739 -> 10342). The exporter does not read this cell; a
  reader that does should expect the keys.
- **ENTRYPOINT** (HF.1 `f70f3957`): no cell change; nodes carrying one now also seed entry flows.
- **Build targets** (HT.3-HT.7): `.glia/targets-snapshot/` (a snapshot step, `glia targets sync`) and a
  `.glia/graph/targets.gmap` sidecar written only by a targets answer call, never by a build or an export; its
  ids are block 2 and named in its own header. A code layout and the exporter's input never hold one.

## 5. Needs no change (checked)

- The exporter, its `--since` diff, the hints side file (`<out>.hints.jsonl`) and `--vocab`: unchanged code,
  check green above.
- Precision beside the tier (HC.4 `ffa6454d`): new keys on why / check / query / implementors / review
  answers only; the exporter reads none of them.
- NavFact gains one appended variant, `CsUsing` (HQ.13): build-time only, never in a gmap.

## 6. Not in 0.5.7

The precision claim fails its committed rule. HP.5 (`528c7e4c`): `[edge-audit] rule 0.5.7 verdict=FAIL
under_floor=ACCESSES_DATA|fact,CONTAINS|fact,EVENT_FLOWS|fact,HANDLED_BY|fact,HAS_ATTRIBUTE|fact,INHERITS_FROM|fact,NAVIGATES_TO|derived,NAVIGATES_TO|fact,TESTS|derived fact=0.993 derived=0.878 heuristic=0.990 kappa=1.000`,
every under-floor stratum from held-out verdicts on edges the candidate mostly no longer writes (the fit panel
at the candidate: fact 1.000, derived 0.936, heuristic 1.000). The re-measure on a held-out panel re-drawn at
the candidate (HP.7 / HP.8 `3fc11b04`): `[edge-audit] rule 0.5.7b verdict=FAIL under_floor=ACCESSES_DATA|fact
fact=0.999 derived=0.991 heuristic=1.000 kappa=1.000`, `[edge-audit] rule 0.5.7b rejudge independent=64
self=3`; its one under-floor stratum is a single wrong edge (tolgee's JPQL `join fetch` ->
`data_entity:sql:fetch`), whose cause HQ.14 fixed after the draw (`dev-notes/edge-precision-0.5.7.md`). No goal
layer: `glia explain` is 0.5.8, a small release of its own, and the goal layer (GOAL_UNIT / TRACES / UNIT get
their emitter) is 0.5.9.

## 7. Re-test

1. Build the exporter at `v0.5.7`; `glia-export-engram -V`.
2. Export one Rails or Angular (`loadChildren`) repo across the upgrade with `--since`: the re-keyed routes and
   pages appear as renames (or add + remove).
3. Export a Python repo with class-level properties: the ATTRIBUTE twin facts are gone, the METHOD facts stay.
4. `crates/engram-live/examples/replay.rs`' `v6 since pairing:` sums: unchanged line format.

## 8. Packet index

Declared changes listed here: HC.1 (s1, s4), HF.1 (s4), HF.3 / HF.4 (s3), HF.5 (s2), HP.4 (s3), HQ.1 (s3), HQ.2
(s2), HQ.3 (s3), HQ.4 (s3), HQ.5 / HQ.6 (s2), HQ.7 / HQ.8 / HQ.9 (s2), HQ.10 (s3), HQ.12 (s3), HQ.13 (s1, s3,
s4), HQ.14 (s2), HT.3-HT.7 (s4). Answer-surface only (no gmap content), listed in the repo-graph handoff: HC.4
(s5 here), HQ.11 (a `ruby` HANDLED_BY caveat row), HT.2 (`glia inspect --json` rows gain `block`), HT.8
(review / diff-impact `targets`). Process or bench only: HP.1, HP.2, HP.3, HP.5-HP.8 (s6), HC.2, HC.3, HF.2,
HT.1, HT.9, HT.10.
