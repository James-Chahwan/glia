# DPG: the Domain-Parameterised Graph

glia's graph is a **DPG, a Domain-Parameterised Graph**: a labelled property graph whose meaning comes from a
domain profile, not from the engine. The engine knows nodes, edges, cells, walks and rankings; a domain says
which kinds exist, where a walk starts, which edges it follows, how much each edge weighs in a ranking and when
two nodes are one thing. Code is the first domain. The rest of this page says what that means today, then where
it is meant to go. Everything under [Directions](#directions) is a direction, not shipped.

## What a DPG is

A labelled property graph (LPG) has typed nodes, typed edges and properties on both. A DPG is an LPG plus four
things the domain supplies.

1. **Registries.** Each domain allocates its own node kinds, edge categories and cell types as locked `u32` ids
   (the code domain's live in `code-domain/src/lib.rs`). A `.gmap` file names the registries it was written
   with in its header, so a reader never guesses what an id means.
2. **A profile.** `DomainTables` (`activation/src/profile.rs`) holds the domain's semantics as tables:
   - the entry rule: the kinds, roles and names a liveness or flow walk starts from;
   - the carry edges: what blast radius, trace and liveness follow (structural containment stays out);
   - the effect sinks: the external effects a node can have, each a (target kind, reaching category) class;
   - the activation weights and presets: the Personalised PageRank dials per edge category;
   - the community weights: what grouping counts, kept apart from the ranking dials;
   - the identity rules: the ways two nodes are one thing, each with its key, tier and whether a class walks.

   The code domain's tables are `CODE_TABLES` (`code-domain/src/profile.rs`); the engine wraps them with the
   domain's build passes as `CODE_PROFILE`. A new entry kind, carry edge or weight is one table row there, never
   a list in a consumer. `toy-domain/` is a test-only second domain proving the seam, and the Datalog evaluator
   (`rules/`) runs over any domain that implements `FactSource`.
3. **Evidence tiers.** Every edge carries an EVIDENCE cell naming its emitter, rule, file and line. The emitter
   and rule decide its tier: `fact` (read at a site), `derived` (paired by a resolver or pass) or `heuristic` (a
   name guess, an overlay, history, a clustering or a model). An answer row carries the tier of the edges that
   produced it, never an unexplained score; a Datalog row takes the minimum over its derivations of the maximum
   over its inputs.
4. **The identity quotient.** When two nodes are one thing (a message in two repos, a client model and its
   server model, a package and the project its manifest names), the identity pass writes a SAME_AS star around
   the class's least member. Answers collapse a `fact` / `derived` class to one row, and a walk over a walking
   class treats it as one node (`algo::classes::Quotient`). SAME_AS weighs nothing in PPR: identity reaches a
   walk only through the quotient.

Nodes are multi-celled: one node holds typed cells (CODE, DOC, POSITION, INTENT, TEST, ATTN, FAIL, CONSTRAINT,
DECISION, CONV, VECTOR and more), each written by a parser, a pass or a snapshot step, so facts about a node from
different sources sit on the node instead of beside it.

### Prior art

- **Typed graphs and type graphs** (Ehrig, Ehrig, Prange and Taentzer, *Fundamentals of Algebraic Graph
  Transformation*, 2006): a graph typed over a type graph. A domain's registries are its type graph.
- **Heterogeneous information networks and meta-paths** (Sun and Han, *Mining Heterogeneous Information
  Networks*, 2012; PathSim, 2011): typed walks over a multi-typed graph. Carry edges are a domain's admissible
  meta-paths, reduced to a category set.
- **Pushouts** (category-theoretic gluing in graph transformation): joining two domains along a shared part,
  the shape the identity classes take when they join graphs.
- **Provenance semirings** (Green, Karvounarakis and Tannen, *Provenance Semirings*, PODS 2007): annotations
  that combine through joins and unions. glia's tiers are a small instance: min over derivations, max over
  inputs.
- **Personalised PageRank** (Jeh and Widom, 2002) and **HippoRAG** (Jiménez Gutiérrez et al., 2024): seed nodes,
  then PPR, then the top reachable; HippoRAG's hippocampal-indexing model of memory is where the activation pass
  came from. See [overview.md](./overview.md#acknowledgments).

What is new in a DPG is the split: the graph machinery is domain-free, and the semantics a graph database would
leave to each query (where a walk starts, what it follows, what is one thing, how sure a row is) are tables the
domain ships once.

## Directions

Code is one domain among many that together say what is to be made, why, how, because and what went wrong. The
directions below are the design's horizon. None is shipped unless the line says so; each becomes a packet only
with a registered test, and the release plans ([0.5.6](../dev-notes/next-leap-0.5.6.md),
[0.5.7](../dev-notes/next-leap-0.5.7.md)) say which are scheduled.

### How a domain relates to code

A domain relates to code in one of four ways.

1. **The same entities at another zoom.** Function, module, build target, service, deployment. Build targets
   are 0.5.7's first non-code domain (track T9); they reuse the identity classes and the quotient to walk at a
   coarser level before they need a registry of their own.
2. **Facts about code, with no entities of their own.** History, tests, docs, ADRs, decisions, issues. Most of
   these already land as cells or snapshots (co-change, test reports, DECISION / CONV notes); docs as a second
   graph is 0.5.6 work.
3. **Linked domains with their own entities**, joined by identity or a contract. Infrastructure, database
   schemas, API contracts, the runtime. 0.5.6's environment-host graph (Dockerfile, compose, Kubernetes,
   Procfile, systemd) is one.
4. **Separate domains.** Video, molecules, policy, legal citations, biological pathways: the same engine, each
   with its own profile.

Linking N domains should cost N + K, not N x N: each domain emits key mentions, one resolver per key type (path,
scoped id, id lineage, citation, table row) normalises and links them, and one tiered joiner, built on the
identity machinery, writes the links. A text-similarity linker is never a resolver.

### Stacked graphs

Purpose (law, policy, intent) sits over milestones (plans, requirements), which sit over specs and tasks, which
sit over code. Measured on glia's own history ([report](../dev-notes/ranking-eval-vertical-2026-10-18.md)):

- **The layer just above code predicts change.** A requirement id two files share beats its random-id control
  (glia: +0.166 recall@10), and a plan that names its files reaches recall@10 0.890 on glia, where structure alone
  reaches 0.294.
- **The higher layers explain change; they do not predict it.** Specs, law and rulings lose signal at every hop
  up, because they were compiled down into the layer below. They earn their place when they change: a law is
  amended, the requirements under it are wrong, the code that implements them is in the blast radius. Impact
  from above.

Co-changing files are siblings under a shared upper node; the vertical edges live in the other layers. The goal
layer (specs and tasks as first-class units, TRACES edges to code, a clarification queue) is 0.5.7's track T6.

### World domains as the why

The reason a piece of code exists often lives outside it. Each world domain would be its own graph with its own
profile, on the same engine, with glue edges pointing down into the code that implements it.

| Domain | Example chain | Carry | Identity key |
| --- | --- | --- | --- |
| Finance | ASX settlement regime, rule "settle T+2 business days", the trading calendar | HAS_RULE | calendar id |
| Law | an Act, a section, the obligation it imposes | CONTAINS, AMENDS | citation |
| Biology | the glycolysis pathway, the enzyme hexokinase, the reaction glucose to G6P | HAS_STEP | KEGG id |
| Chemistry | the Haber-Bosch process, its 400 to 500 C / 150 to 300 bar constraint, the hydrogen hazard | REQUIRES | CAS number |
| Manufacturing | a surface-mount line, its reflow-oven station, the profile tolerance | HAS_STATION | asset id |

"Why does this function exist?" walks up: the function REQUIRED_BY a section, which an Act CONTAINS, at the tier
the citation resolver set (`derived`). "The world changed" walks down: the Act is amended (an AMENDS edge with a
new validity span), the obligation is now wrong, and the function and its tests are in the blast radius. Every
world edge carries the same EVIDENCE cell as a code edge, and its tier comes from its emitter and rule. The
relation names in this table are illustrations: 0.5.6 allocates no registry id, and a world domain would
allocate its own through the domain id allocator planned for 0.5.7.

### The model's uplift: the graph as substrate

- **Shipped: dense text and packed context.** The sigil projection was designed for 40 to 70 tokens per entity,
  against 150 to 200 in the earlier formats; answers are ranked, located and tiered; `pack` fits a query's
  context to a token budget.
- **Shipped in 0.5.4: per-node vectors.** `glia embed` writes `.glia/vectors.jsonl`. The semantic tier alone
  failed its gate (FB.6, `VERDICT: FAIL`); BM25 blended with similarity passed on a held-out panel (FB.6b,
  `VERDICT: PASS`, R1 8/9, R2 8/9), so the blend orders `find --files` and picks `pack`'s fallback seeds. A
  similarity ranks; it never decides.
- **Direction: graph structure fed to the model directly.** A latent projection of the graph, or graph-aware
  attention, instead of text. The earlier latent experiments found that the lever was the task framing, not the
  injected structure, so this stays a research direction.
- **Direction (0.5.7, proof): calibrated decisions.** `decide/` and `glia judge`: a small local model making typed
  calls, always `heuristic`, never deciding truth, with a clarification queue instead of a silent guess.

The rule across all of them: rankings and vectors reorder rows; they never add or hide a `fact` / `derived` row.

### Biology as the design source

Glia let neurons talk; glia lets agents talk to a structured graph and to each other.

- **Multicellular nodes (shipped).** Typed cells on every node, above.
- **Spreading activation (shipped).** PPR over the graph, after HippoRAG's model of memory.
- **Use-dependent plasticity (direction).** Edges strengthened by the work that crosses them: a weight times
  (1 + beta x co-change support), learning from use at repo level. 0.5.3 measured static edge weights as no
  lever and shipped co-change first (H10) beside the walk instead; plasticity inside the walk is untested.
- **Pruning shows inefficiency (direction).** Where independent gains converge (dense-text compression, learning
  from use, accurate edges) is the real shape; what each one prunes (hub floods, edges no work crosses) marks the
  waste.
- **Consolidation (direction).** Fold, do not forget: corrections written back as cells. 0.5.4 ships corrections
  and conventions as cells read by `pack` and `review`; a periodic consolidation pass is the direction.
- **The missing walker (direction).** The graph models the territory; nothing yet models the person walking it:
  task, intent, what was touched, outcome. Co-change is its fossil record, and 0.5.3's co-change-first ranking
  and id routing read that record. A walker domain of its own is open.

### Also open

A type graph checked against the triples a build writes; cross-domain glue kinds; visual domains through the
same vector contract (an image space needs no format change); e-graphs for identity; bi-temporal edges (the
timeline sidecar holds validity spans, not transaction time); evidence calibration (noisy-OR, Dempster-Shafer);
succinct structures at monorepo scale; data lineage as a domain; dominators, RuntimeZone and an LSP.

### Where the releases stand

0.5.4 (embeddings and precision) and 0.5.5 (Lua and the use-pattern axis) are released. 0.5.6 finishes what is
started: precision and recall residuals, watch, concept search over flows and placement, the host-to-plugin and
environment-host graph, build speed and docs as a second graph. 0.5.7 adds the goal layer and clarification
queue, build-target graphs (the first non-code domain), proof (precision on real repos, the agent probes,
calibrated confidence, `decide/`, `glia judge`) and the domain id allocator. Around 0.5.7 is the bar the
direction is judged against: a new era of developer tool.
