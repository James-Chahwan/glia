# glia 0.5.3 — scale, plus the Engram sidecars (scope seed)

Seeded 2026-10-15 at `6da2965`, before any 0.5.2 wave ran. This is a scope list, not a spec: the packet spec run
happens after the 0.5.2 release (2026-10-18, §0), so every packet is verified against what 0.5.2
actually built. Items join this list as 0.5.2's waves surface them (close-out followups with no 0.5.2 owner).

## 0. Decisions (verbatim)

- 2026-09-19 / 09-30 (0.5.1 plan §1, §5): 0.5.3 = **scale**: Stack Graphs-style incremental resolution and
  build-target graphs.
- 2026-10-02 (0.5.2 plan §0.1): moved here: multi-repo `glia watch --with` (Q13), the watch body-only fast path A2
  (Q15), org-internal package routing (Q19); 0.5.2 plan §4.3 lists the spec run's findings with no owner.
- 2026-10-15, after comparing glia with AWS Context: *"i think we have some of this covered in the builds to come and
  can we possibly integrate engram ?"*, then *"yeah thats a 0.5.3 spec if ivee ever seen it so yeah lets spec up 0.5.3
  with what we also need but do the 0.5.2 uplift then hand over to repo-graph session"*. Order: 0.5.2 waves → the
  0.5.2 handoffs → this spec run.
- 2026-10-16, on the first pass of this seed: *"a review queue for human input clarification with an agent should
  work better why do we need a db export of the graph lmao can't we do somehting better then that"*; *"yeah lets get
  the leftover list in 0.5.3"*; *"fix 2."* (the self-graph hygiene cluster); *"3. yep add it"* (watch memory budget);
  *"4. is a good idea"* (one tier vocabulary); *"5. sounds good"* (runner fixes); and on find / flows: *"i noticed this
  my self with my work at work when it did a cache search with flow it didn't find it because it thought the flow was
  named cache thats probably something we should look at for find and flow"*. Recorded in §2 C and §2 F.
- 2026-10-16, on the open questions (with the session's options and recommendations): *"1. yeah outta 0.5.3"*;
  *"2. a makes sense so does b"* (the semantic tier feeds `find` and the flow search, and `pack` as a fallback only;
  never blast-radius); *"3. A first and get the hand off to engram ready we may need to decide what happens to engram
  soon"* (fixtures, contract and validator in glia first; the Engram ask drafted in `dev-notes/engram-ask-0.5.3.md`,
  not sent); *"4. D yeap totally a great call"* (proximity, then the clarification queue).
- 2026-10-16, later: *"well shouldn't we bring parts of engram in if we need the embeddings ?"* and *"okay i don't
  understand the issue of using the embedding models thats tens of mb when it solves isn't it locally run in engram i
  swear"*: glia embeds locally itself (fastembed / bge-small, as Engram does), §B.
- 2026-10-16: *"also its great to take this from engram and lets us focus on the substrate vs the specialisation"*.
  The split this release follows: glia is the substrate (the graph, its embeddings, query and rules, the answers every
  consumer reads); consumers are the specialisation (Engram: learning from use over session history and consolidated
  memory; repo-graph: MCP transport and the human conversation surface, including the clarification queue's).
- 2026-10-16, on Engram's evaluation and corrections (answering "sure" to the session's recommendations): the
  determinism check runs now at 0.5.2 wave 5 (`3921a636`); the Kina + glia replays (re-keys with X1 off and on,
  `flat-ppr` vs `flat-fs`) run once at the 0.5.2 release candidate after W14; the 8 public streams only if those pass.
  Corrections: James then, *"no still in 0.5.3"* - the Engram corrections writer stays in 0.5.3 (§B).
- 2026-10-16 (reversed 2026-10-18, below): *"Hold repo graph for post 0.5.3 better to do all changes at once"*. The
  repo-graph handover moves after 0.5.3: repo-graph takes 0.5.2 and 0.5.3 together, from one combined handoff written
  by 0.5.3's release docs packet.
  New order: 0.5.2 waves -> DZ.1 / DZ.2 (the 0.5.2 handoffs are written and committed; the repo-graph one is not
  sent) -> Engram's Kina + glia replays at the 0.5.2 release candidate -> this spec run -> 0.5.3 waves -> the combined
  repo-graph handover.
- 2026-10-16: *"For 0.5.3 all follow must be completed within it also just so I can keep testing tomorrow etc do some
  tweaks if need be before we go for the release"*. Every followup a 0.5.3 wave raises is finished inside 0.5.3: none
  goes to a 0.5.4 seed (§4). 0.5.2 stays unreleased while James tests it; his tweaks land on main before the bump.

- 2026-10-16, alignment (*"yeah sure lets get everyone aligned sorry this got a bit confusing"*): items MOVED out of this
  seed into the 0.5.2 tweaks (`dev-notes/tweaks-0.5.2.md`, T1-T12), so they are not 0.5.3 work any more: the client <->
  server contract model (T1), NATS dynamic / config subjects (T2), WebSocket inside an HTTP route (T3), the trailing query
  interpolation (T4), dead_symbol framework noise (T5), test-file routes (T6), in-project forbid anchoring and visible
  orphaned constraints (T7, the forbid part of §D's orphaned-constraint item), check caveats once and located GQL rows
  (T8), pyo3 category names (T9), smart contracts across the stack (T10), GraphQL root-field pairing (T11), and one hint
  format with `glia cell set --hint` (T12, the §B "caller hint" and "one hint format" items). The 0.5.3 spec run reads
  this seed minus those, and re-measures after the tweaks land.
- 2026-10-17 (reversed 2026-10-18, below): *"can't we just go 0.5.1-> 0.5.3 release"*, then *"yeah sounds fine"*.
  **0.5.2 is never released.** The commit where the 0.5.2 tweaks end (DT.13) gets a local marker tag
  `0.5.2-complete` (no bump, no publish); Engram's Kina + glia replays and its `--hint` writer switch run at that tag,
  built from source (binaries there print `0.5.1+p<stamp>`). The version goes 0.5.1 -> 0.5.3 in one bump (Cargo workspace, `py/pyproject.toml`,
  `engram-export/Cargo.toml`) at the end of 0.5.3. 0.5.3's release docs packet merges all three 0.5.2 handoffs
  (`dev-notes/{repo-graph,neuropil,engram}-handoff-0.5.2.md`) into one combined 0.5.1 -> 0.5.3 handoff per consumer,
  and folds README / py/README's 0.5.2 sections into 0.5.3's. Also *"okay after this we should test kina and quokka-stack
  again and hopefully we can rul the 0.5.3 answer any questions then begin build overnight"*: after DT.13, the Kina +
  quokka-stack demos re-run, then this spec run, then 0.5.3's waves.
- 2026-10-18: *"maybe we just do the 0.5.2 and slowly work on 0.5.3 over the next week"*, then "I prep, you push" and
  "repo-graph upgrades to 0.5.2 now". **0.5.2 is released** (2026-10-18, tag `v0.5.2`, glia-py 0.5.2 on PyPI; the
  session prepares the release commit, James pushes, tags and publishes), reversing the two entries above: the version
  goes 0.5.1 -> 0.5.2 now (Cargo workspace, `py/pyproject.toml`, `engram-export/Cargo.toml`), the three 0.5.2 handoffs
  are each consumer's whole 0.5.2 handoff with exact `=0.5.2` / `glia-py==0.5.2` pins, and repo-graph upgrades to 0.5.2
  now (no longer held). Engram's registered replays at `0.5.2-complete` (`196728e3`, below and §E) stand: no source file
  changes between that tag and `v0.5.2`. **0.5.3 is a normal 0.5.2 -> 0.5.3 release**, paced over the week: this spec
  run, its waves and follow-ups, then its own release docs packet with its own 0.5.2 -> 0.5.3 handoff per consumer (no
  combined 0.5.1 -> 0.5.3 handoff, no fold of README / py/README's 0.5.2 sections).

- 2026-10-18: *Don't notify neuropil it's so far behind visual 3d stuff takes much more free time not contracting for work but on the other not committed it can go to 0.5.3*. neuropil skips 0.5.2 and is not notified; 0.5.3's release docs write neuropil a
  0.5.1 -> 0.5.3 handoff that carries `dev-notes/neuropil-handoff-0.5.2.md` forward (repo-graph and Engram get 0.5.2 -> 0.5.3).
- 2026-10-18, relayed by engram-34 after its registered run at `0.5.2-complete` (PPR candidates: 9 wins / 20 losses):
  *"if it has no benefit and we know of better versions we should tell glia what we learned for this specific choice vs ppr
  and get it removed and changed before 0.5.3 it's a dumb feature to keep then"*. So 0.5.3 REMOVES DE.5 (§F item 7), and
  glia builds nothing for Engram's ranking on spec (glia's vectors and flow search as Engram recall candidates included):
  Engram registers a test with a reason from its own record first, then asks. Engram's lessons, for the spec run: ranking
  by static code structure did not help Engram's recall three times (spreading inside the read, a negative learned weight
  -0.13, PPR pool candidates 9/20); what carries value is learning from use, the session's work neighbourhood / per-file
  evidence, rename / move identity (DE.3 / DE.4: 13 of Kina's 18 and 19 of glia's 40 renamed questions answered only with
  old addresses) and accurate edges / fewer false dead symbols (glia stream strict +15 / -2 at the tag). This covers
  Engram's recall only, not glia's own PPR in find / activation (judged by glia's gates). Anchoring memories on flows
  (Kina corrections with no code node) is a separate question, open with James.

## 1. The train

| release | contents |
|---|---|
| **0.5.2** bets | Datalog rules + GQL, `glia watch`, cross-repo identity, Engram anchors, deferrals, the matrix rest (`next-leap-0.5.2.md`) |
| **0.5.3** scale | this doc |
| **0.5.4** proof | precision rate, agent before / after, big-repo timings, calibrated confidence, `decide/` a–e, `glia judge` |

## 2. Scope seeds

### A. Scale (on the train since 0.5.1)

- **Stack Graphs-style incremental resolution** (0.5.1 plan §5: github/stack-graphs archived 2025-09-09; one
  partial-path model across 20 languages is the unknown), with watch's A2 body-only fast path on top (R2-watch).
- **Build-target graphs**: `bazel query` through a `snapshots/` crate, since macros and `select()` defeat a static
  parse. The likely first real non-code domain on the 0.5.0 seam.
- **Multi-repo watch** (`glia watch --with`).
- **Org-internal package routing** (sibling-repo imports resolved to the sibling's symbols, on 0.5.2's SAME_AS).

### B. Embeddings in glia; Engram, data only (proposed 2026-10-15, revised 2026-10-16)

James, 2026-10-16: *"well shouldn't we bring parts of engram in if we need the embeddings ?"*, then *"okay i don't
understand the issue of using the embedding models thats tens of mb when it solves isn't it locally run in engram i
swear"*. So glia embeds itself, locally, with the library Engram uses (fastembed-rs on the ONNX runtime, bge-small;
`engram-live`'s `local-embed` feature): inference runs in-process, the model downloads once. glia depends on fastembed
directly and copies no Engram code, so the boundary holds: `engram-core` is the only shared crate, Engram depends on
no glia crate, and anything between them crosses as side files (R2-engram §2, Engram's 2026-10-02 answer). 0.5.2's
Engram anchors (cross-clone determinism, re-key pairing) still matter: Engram's learning lives on experience → code
links, so they must survive a rename.

- **`glia embed`, a snapshot step (never inside `glia build`,** like `history sync` / `scip import`): embeds each
  node's text - name, qname, leading_doc and code, since Engram measured full text beating name + qname 78 vs 54 of
  100 - and writes VECTOR rows to `.glia/vectors.jsonl` (the LF.1 sidecar, `external_inputs::VECTORS_FILE`; a build
  already loads it as cells). Each vector is cached by a hash of its text, so a re-run embeds only changed nodes; rows
  carry `model` and `dims`. Its own crate behind a Cargo feature (the CLI on; whether the PyPI wheel ships it is
  measured in the spec run). Determinism needs no special handling: the text-hash cache gives one machine the same
  vectors every run, and nothing gates on cross-machine layout bytes (James: the byte-identical concern "could be
  overstated").
- **When embeds run** (James: *"we could have a way to determine when to run the embeds"*). The cache makes a run
  cost only the changed nodes, so the question is the trigger. Candidates for the spec run:
  `glia watch` embeds changed nodes in the background after an apply, debounced; `install-hooks` adds a post-commit /
  post-merge `glia embed`; a semantic query counts nodes whose text hash has no vector and embeds them inline under a
  small threshold, else answers from what it has with an absence caveat ("<n> nodes not embedded; run glia embed");
  an explicit `glia embed` always works. A `[embed]` marker reports embedded / cached / stale counts.
- **Semantic tier** in `find` and the flow search (§F.6), and `pack` as a fallback (§3): the query text is embedded
  in-process with the sidecar's model; HEURISTIC, reported as its own match tier with its similarity, never mixed into
  the lexical order. Blast-radius never seeds from vectors.
- **The sidecar stays writer-agnostic:** the written VectorRow contract and its validator (rows whose qname matches
  no node, mixed models, wrong dims) hold for any writer, and Engram can read glia's vectors (same model, same space)
  instead of embedding again.
- **Engram's embedding learnings** (engram-34's answer, 2026-10-16, measured unless dated; binding for the spec run):
  - Model: fastembed-rs `EmbeddingModel::BGESmallENV15` = `Xenova/bge-small-en-v1.5`, `onnx/model.onnx`, fp32, 384
    dims, CLS pooling (ort 2.0.0-rc.9 / ORT 1.20). Not `BGESmallENV15Q`, a different artifact. Rows name the exact
    artifact in `model`; a reader skips rows whose model differs from its query model.
  - Text: Engram embeds name, qname, leading doc (500-char cap) and up to 10 imports not already in name + qname; never
    a code body. Its 78-vs-54 was that text vs name + qname + imports (the gap was paraphrased questions, 28 vs 9 of
    44). **Code bodies were never measured**: the spec run A/Bs them. fastembed silently truncates at 512 tokens, so
    code goes LAST and truncation never eats the doc.
  - Query: the raw question, same function, no BGE instruction prefix (untested; a reader-side A/B). fastembed
    L2-normalises; rank by raw cosine (a hashed sparse projection lost 11-13 of 100 lookups).
  - **No cosine floor means "no match".** A dense tier is never silent, and Engram's best silence rule failed held-out.
    So the semantic tier never decides presence: `find` / the flow search / `pack`'s fallback keep the lexical absence
    ("nothing matches by name") beside any semantic rows.
  - Determinism: on CPU a text's vector is bit-identical alone, padded or batched (256-text chunks), so the text-hash
    cache holds. GPU bit-identity is untested: the determinism gate stays CPU-only.
  - Gotchas: the model cache defaults to `.fastembed_cache` under the CWD (set `FASTEMBED_CACHE_DIR` to a fixed user
    cache, never a repo dir); two ORT sessions at once thrash (13 s vs 0.8 s), and each takes every core, so cap
    intra-op threads and never embed concurrently with a build's rayon pool. GPU recipe (if wanted): Microsoft's
    onnxruntime-linux-x64-gpu-1.20.1, ort features `cuda, load-dynamic, ndarray, half`, `ORT_DYLIB_PATH`,
    `CUDAExecutionProvider::default().build().error_on_failure()` (else silent CPU fallback); never combine
    `ort-download-binaries` with `load-dynamic`.
- **Corrections as cells: IN 0.5.3 (James, 2026-10-16: "no still in 0.5.3"); Engram has no source yet.** Engram forms no consolidated corrections today: its Cell C (lessons from
  user corrections) failed its ceiling check on 2026-10-11 (2 of 13 recurrences in a later session) and was not built,
  so the Engram writer of `.glia/cells.jsonl` is new Engram work (forming consolidated corrections, then exporting
  them). It is judged on glia's side by whether the cells improve answers, not by Engram's recall ceiling, which
  measured a different use (corrections as recall cues). glia's side already exists: the LF.1 sidecar and the `orphaned_cell` gaps row. Agreed lifetime rule
  for any writer: the writer owns its rows, glia never deletes another source's rows, an orphan is kept, reported and
  attached to nothing, and the writer re-anchors or deletes it on its next write; an entry `id` is a hash of the
  correction's content.
- **Engram's corrections writer, scoped** (engram-34, 2026-10-16, with James's rulings: code-naming corrections only;
  Engram writes `.glia/cells.jsonl` and never commits it, James commits; the live writer reads the session transcript at
  sleep, the hook log stays text-free). DECISION = a choice about what specific code does or keeps; process rules are
  never exported. Few rows (Engram's read: about 7 of 46 recorded corrections name code, all Kina; 0 for glia). The
  node is picked from the Symbol facts the corrected edits touched (or "no node", no row); `qname` + glia's
  `identity_hint`, no `kind`. One row per (node, class), rewritten in place; `id = "engram:" + hash(class key + node
  hint)`, `source = "api"`; written at each Engram sleep, only Engram's own rows touched. glia's answers to its
  questions (from the code): free-form ids of 1..=128 chars; extra entry keys are stored but unread, so the why stays
  in `text`; qname + hint binds without `kind` unless two same-qname nodes defeat the hint (`ambiguous`, nothing
  applied); other writers exist (`glia cell` CLI, pyo3 `set_cell` / `write_cell`), so a writer takes
  `.glia/cells.lock` (exclusive create, 30 s stale) or shells out to `glia cell set`, which adds validation and
  redaction.
- **A caller hint on the cell write surfaces** (engram-34, 2026-10-16). `glia cell set` (`SetArgs`: repo, qname, cell,
  json / text / file, kind, model, dims) and pyo3 `write_cell` / `PyGraph.set_cell` take no hint; the store's
  `CellWrite::with_hint` already does, and the store keeps the caller's hint over the bound node's. Today the hint is
  captured only when `.glia/graph` is fresh, so a writer after edits (Engram at sleep) writes a hint-less `pending`
  row; it binds by qname at the next build, and a later rename orphans it (`--rekey` needs a hint). Add `--hint` /
  `hint=`, validated as an identity hint; when the layout is fresh and the caller's hint names a different node than
  the qname binds, report it (`target`), never pick silently. No 0.5.2 packet owns `cli/src/cmd/store/cell.rs` or
  `py/src/cells.rs`. Interim for any writer on 0.5.2: build first (`glia build`), then `cell set`, so the store
  captures the hint.
- **Engram ask: the repo's identifier vocabulary** (engram-34, 2026-10-16; James picked 0.5.3).
  `glia-export-engram --vocab` writes `<out>.vocab.jsonl`: every identifier-like token (`[A-Za-z_][A-Za-z0-9_]*`,
  3+ chars) in the repo's tracked text at the exported commit, including everything the export filters (tests,
  generated, vendored, noise, non-code); rows `{"t", "files": [file ids], "n"}` with `files` capped at 16 (`"common":
  true` past it), sorted by token, bound to the gmap by digest like `.ppr.jsonl`. Why: Engram's "(not in memory: X)"
  note caught 60/60 never-existed names on Kina and 43/60 on glia with 0 false flags, but only 0.74 / 0.61 of what it
  flagged is truly absent from the repo; the rest are real names the export never emits as symbols (locals, fields,
  keys, filtered paths), and only glia sees the whole tracked text. Engram uses it under its own rule (a live A/B for
  the note; name -> files as a needle-tested pool source). Measured on glia's tree (2026-10-16, Engram's rule, a
  Python count over `git ls-files`): 3,640 text files (33.2 MB), 52,764 tokens, 4,779 in more than 16 files; 2.7 MB of
  JSONL per export, 0.6 MB deflated. Settled with Engram (2026-10-16): tracked = the git index at the commit (like-for-like with its `git grep`,
  and untracked files would break cross-clone byte identity); a full file per `--since` step, no diff format
  (Engram's replay caches 852 glia steps, ~2.3 GB more at most); one file-id space: the export's file-table ids,
  then path rows `{"f": <id>, "path": "<repo-relative>"}` for files outside the table, numbered after the
  table's largest id, sorted by path and written before the token rows `{"t", "files", "n"}` /
  `{"t", "common": true, "n"}`. Substrate value beyond Engram:
  `find`'s absence says "no node named X, but X occurs in <files>" (a FACT-tier "named but not modelled" signal),
  and `gaps` gains a named-but-not-modelled category over the same index. **Endorsed, 2026-10-16** (James: *"sick good
  gaps coverage and enhancements for glia"*): the vocabulary index is a glia substrate feature first, with Engram's
  `--vocab` export as one reader of it.
- **One hint format between glia and Engram** (engram-34, 2026-10-16, writing the first real row into Kina, James
  approved). For one node two "identity hints" exist: the gmap's `identity_hint` from glia-export-engram,
  `frontend/src/app/app.routes.ts:3:0` (anchor file, kind code, ordinal; DE.3), and the cell store's `Identity::hint`
  (`graph/src/identity.rs`, LB.6), `v1|3|app.routes.ts|9bdbdf56d843b8d0|entryGuard`. The cell store keeps and rekeys by
  the second, Engram holds the first, so Engram cannot match a row's stored hint against its facts and follows rows by
  qname until this lands. **Decision (the session's, James can overrule):** the `v1|..` form is the one shared key - it
  names the node and hashes its content, where the ordinal form can pass to a sibling after a deletion. glia-export-engram
  writes it per fact in a digest-bound side file `<out>.hints.jsonl` (as `.ppr.jsonl` is; no GMAP contract change), and
  `glia cell set --hint` / pyo3 `hint=` take that form.
- **Value flow from config into channel names** (James, 2026-10-16: *"is it nessacary at some point and we should not it
  for later"*). Production services name their NATS subjects, Kafka topics, queues and base URLs in config: an env var
  with a default (`getenv("ORDER_SUBJECT_TEMPLATE", "shop.orders.%s.placed")`), a YAML / TOML key, a struct field set at
  start-up (`Publisher.template <- cfg.SubjectTemplate`). The 0.5.2 tweaks fold literals, same-package helper functions and
  `fmt.Sprintf` templates; a value that travels through config and struct fields stays an unresolved tag with a caveat.
  Needed: one value-flow reader (config key -> default / file value -> the field or variable it is assigned to -> the call
  that names the channel), shared by queues, HTTP / WebSocket base URLs and gRPC targets.
- **Spec item: scope-level conventions.** In glia, CONV is a free-text note on ONE node (overlay `[[note]]`; the
  format spec's "conversation history"), not a convention. A convention that covers a scope ("modals are centred,
  never bottom-anchored": every modal component) has no home: until it has one, Engram writes it as a CONV note on the
  corrected node (`convention: ...`). The spec run decides whether a convention attaches to a module / component /
  project scope (the reflexion model's components are a candidate anchor) and which answers show it.
- **Engram's half** (corrections, its own eval rule: a new candidate source enters its pool as a weight-0 feature and
  stays only if it beats its ablation) is a handoff ask, not a packet: `dev-notes/engram-ask-0.5.3.md`, unsent.
  Learning from use (the release gate over agent-session history) stays Engram's: glia has no session data.
- Rule for every learned or embedded row: it ranks, it never decides what is true. A FACT / DERIVED row is never
  promoted or hidden by a learned or similarity score.

### C. Gaps from the AWS Context comparison (candidates for James, from public sources)

AWS Context (announced at AWS Summit New York, 2026-06-17) is a knowledge graph over an organisation's data catalog
and warehouse, queried by agents over MCP. It reads no source code. What it has that glia does not, and how 0.5.3
could answer each (each needs James's ruling before it becomes a packet):

- **Semantic search** → B's VECTOR tier.
- **Learning from use** → Engram's learned gate, through B; never a glia-side learned ranking.
- **Corrections that reach every agent through a review step** — **IN (James, 2026-10-16), as a clarification queue,
  not an accept list.** glia's overlay loop shares an accepted stanza through git only. What glia cannot settle
  (an ambiguous bind, a `suspected_edge`, a rotted rule, a doc mention with two candidates, an unpaired client) becomes
  a queued question carrying its evidence and candidate answers; an agent works it through with a human, and the answer
  lands as the overlay stanza / DECISION cell it implies, with who answered and why. The queue and the answer format
  are glia's (engine + CLI + pyo3); the conversation surface is repo-graph's MCP.
- **Queries scoped to the caller's identity**: matters only if the shared cache or a hosted repo-graph serves more than
  one team. Candidate, likely out.
- **Metadata readable by SQL engines** — **DROPPED (James, 2026-10-16: "why do we need a db export of the graph").**
  A copy of the graph in another engine goes stale and loses the tiers and witnesses. The better answer ships in 0.5.2:
  `glia query` (Datalog, plus GQL, the ISO graph query language) over the live graph, through the CLI, pyo3 and so the
  MCP.
- **Business meaning (glossary, metrics, lineage)**: a business-data domain is where glia would meet AWS Context head
  on. Not proposed; build-target graphs stay the first non-code domain.

### D. 0.5.2 spec-run findings with no owner (0.5.2 plan §4.3)

- Go generic instantiation calls `F[T](...)` mint no CALLS.
- Angular lifecycle hooks and constructors of reached components are dead_symbol rows; TS interface members are not
  METHOD nodes, so interface-typed receivers bind nothing.
- TS: call-initialised locals have no type; module-level instance consts read in a function; statics inherited
  through a subclass name; superclass STATE_VARs through a typed receiver; default-imported class types.
- A `[[constraint]]` with no anchor and no PROJECT scope is orphaned silently; `[walk] tests` is invisible to a layout
  merge.
- RepoId-ordered tie-breaks left in pick_primary, cochange, overlay bind, declared, cells; a witness path among equal
  alternatives can differ between checkouts.
- Per-group remainders: maven / gradle / pubspec / mix / csproj PACKAGE_DEPs; tRPC external marking, Swift protocol
  receivers, Dart export barrels; Rust trait members, Juniper / Sangria; Elixir one-line defs, Absinthe
  import_fields, Ruby mixins.
- Generated HTTP clients (0.5.2 §4.2 decision 5 kept generated-code provenance to gRPC sites).

### F. Committed by James (2026-10-16)

Not candidates: each becomes packets in the spec run.

1. **The §E leftover list is a 0.5.3 group** (per-language precision residuals, with §D's), packeted like 0.5.2's DF /
   DG / DH matrix groups and gated by the substrate-gap fixtures and the matrix.
2. **Self-graph hygiene, fixed first** (§E W2 and W3): every needle scanner reads through the CJ.1 code guard (cron
   YAML, CLI `INVOCATION_PATTERNS`, `data_entities.rs` SQL literals, the migration DSLs), the guard covers the other
   languages' comments and strings (Ruby first), and glia's self-scan gains a zero-phantom gate. A declared
   graph-content change.
3. **A memory budget for long-lived watch.** DC.4's memos add one entry per walked path and a watch session keeps them;
   multi-repo watch multiplies that. Measure RSS over repeated applies on the 0.5.2 WatchSession (DC.7a), then cap or
   evict, with a `[watch]` RSS marker and a gate.
   First data (DC.7a, W6): post-trim RSS 404 -> 432 -> 441 MB on glia and 180 -> 228 -> 241 MB on grpc-go over 3
   updates, with paired incremental builds in the same process. **It levels off** (DC.8, W9): 100 watch-only updates on
   a glia copy (31,137 nodes / 104,825 edges, debug build, rss_mb after malloc_trim): first 562 MB, last 664, max 683;
   means 597 (updates 1-10), 637 (11-25), 642 (26-50), 656 (51-75), 654 (76-100) - about +75 MB over the first ~12
   updates, then flat at 615-685 MB. A held pyo3 WatchSession creeps about 80 MB over 12 updates on glia (DC.9). The
   budget question is the plateau's size, not a leak.
   **The rules evaluator joins this budget** (James, 2026-10-16, on 0.5.2 DA.17: *"Yeah do a then"*). DA.17's RSS gate
   was restated from R1's proposed 150 MB to the measured 287,408 kB evaluator peak plus headroom (320 MB = 327,680 kB)
   for CHECK20 over glia x10 (300,130 nodes, 1,019,544 edges; whole process 449,728 kB vs R1's spike at 1,654,172 kB).
   Where the memory goes (DA.17, at x10): about 115 B per edge row before any rule runs (EDB rows plus Relation indexes
   and the value table), the carry edge/3 relation 92,084 kB, node/2 46,104 kB, scc over carry 46,988 kB, reaches from
   every ROUTE 87,788 kB, one direct forbid 99,388 kB. The representation work is in `rules/src/{relation,value,eval,
   closure}.rs`; 0.5.3 sets one memory budget for watch and rules together; when it lands, `X10_RSS_GATE_KB` in `rules/tests/cost.rs`
   comes back down toward 153,600 kB (x10 peak at 0.5.2 W7: 287,236 kB).
   Data point (repo-graph-8d, 2026-10-18, its 0.5.2 port; VmRSS steady over six edits): a held WatchSession vs a rebuild
   loop is quokka-stack 164 vs 138 MB (0.12 vs 0.17 s / update) and glia 940 vs 500 MB (1.93 vs 2.33 s).
4. **One tier vocabulary.** `rules::value::Tier`, the string consts in `why.rs` / `check.rs` and `implementors.rs`'s
   enum become one code-domain type (§E W1, DA.3), before Engram's HEURISTIC rows add a fifth reading.
5. **Runner fixes** (the 0.5.2 waves found both; the session landed them for the remaining 0.5.2 waves too):
   `closeout.py` lists every followup it could not fold (no packet id, or ids of landed packets only) instead of
   dropping it, and `gen_wave.py --only <ids> --resume <notes.json>` renders a continuation run of unreturned packets
   (a Workflow resume re-runs every agent after the first changed one).
6. **Concept search over flows (find and trace / flows).** `trace <word>` resolves the word to a node by qname, name or
   find's top hit, and its entry-flow fallback matches flow keys made from entry names alone (`engine/src/trace.rs`
   module doc), so "cache" finds a node or a flow *named* cache, never the flows that *go through* a cache (James hit
   this at work). A flow should match a word through what it touches: hop names and qname words, data sources and
   DATA_ENTITY kinds, mechanisms, external hosts, docs governing a hop. Rank by match tier, then the matching hop's
   depth; show which hop matched. `find` gains the same view (the flows through its hits). §B's semantic tier feeds
   this too (open question 2, in part).

7. **Remove DE.5's Engram side file (James 2026-10-18, §0; then *"oh it's diff usecases of ppr btw in engram its recal in
   glia its walking"*).** engram-export loses `--ppr-candidates`, `<out>.ppr.jsonl`, `src/ppr_side.rs`, exit code 8, its own
   `glia-activation` dependency and tests/ppr_side_v6.rs; 0.5.3's Engram handoff declares the removal as a break (it
   undoes the 0.5.2 Engram handoff's §3.5, its breaks-table row and the `.ppr.jsonl` .gitignore line); CLAUDE.md /
   README lose the export text. glia's PPR for walking
   stays: `glia_activation::activate` / ActivationPlan (power iteration) in blast-radius / diff-impact / review, resolve /
   find, pack, hotspots and `PyGraph.activate`. `activation::algo::ppr` (push PPR, Andersen-Chung-Lang, local: touches only
   nodes near the seeds) stays too, though ppr_side was its only caller: it is a candidate in item 8's ranking test, kept or
   removed on that result. Engram removes its reader (flat-ppr, `--ppr`, the release feature).

8. **Ranking: intent + structure (James 2026-10-18: *"we probably should test also ppr or a form of hybrid ppr is probably
   what we want like ppr pooled clusters"*, *"i knew ppr for walking + hybridsation from engram would slap"*, *"yeah that
   goes into 0.5.3 here"*).** Evidence: `dev-notes/ranking-eval-2026-10-18.md` (harness `engine/examples/rank_eval.rs`):
   on held-out commits, H9 = minmax(BM25 over past commit messages -> the files they touched) blended with PPR beats PPR
   alone (glia recall@10 0.294 -> 0.411, Kina +0.08-0.11, quokka +0.06 n.s.); PPR beats BFS only on glia; co-change helps
   only as a tie-break (H10, +0.10 MRR); use-weighted edges raise MRR, not recall (84-93% of co-changing file pairs share
   no direct edge). First re-run the eval with real PR / issue text as the intent (the commit's own message is optimistic),
   then: H9 for `review` / `find` when text exists, H10 for `impact` / blast-radius, sum pooling to files, the PPR index
   kept hot between calls (~3x), edge weights retuned from the inefficiency map (HTTP_CALLS / HANDLED_BY overpaid, TESTS /
   IMPLEMENTS / INHERITS_FROM underpaid), and task / intent as a first-class domain (commits, PRs, issues as nodes linking
   intent to the files they touched: the "walker" the ranking eval measured).
   Second run, the vertical layers (`dev-notes/ranking-eval-vertical-2026-10-18.md`, James: *"imagine graphs stacked upon
   themselves and vertically"*): requirement ids cited in code are real vertical edges (co-change pairs share one 16-19%
   of the time vs ~1%; V1 beats its random-id control) but add only +0.01-0.04 over PPR and nothing over intent text on
   Kina; every hop up (spec, law, James's rulings) loses signal, law and rulings sit on their controls; routing a commit
   message's requirement id to the files that cite it is Kina's largest real gain (+0.039, +0.082 where the message cites
   one); a plan that names its files is by far the strongest edge (glia packets' `files_touched`: recall@10 0.890). So
   item 8 also makes requirement / task / plan nodes first-class with two vertical edges at the most specific node (code
   cites an id; a task or doc row names a file, pending or done), routes PR / commit text's ids through that layer in
   review, and keeps law / spec / rulings for "why" answers and provenance, not ranking. Ingestion needs scoping rules
   (spec-kit FR-002 / T014 are per spec; a global D6 is not spec 002's D6; a law key needs at least two doc units).
   Third run, impact from above (`rank_eval --law-impact`; the detailed report is private, kept outside this repo): a
   law's section-level diff traced through a regulated app's goal layer (docs citing sections -> the files they name) found
   the code its implementing commits changed far better than text similarity or structure (F1 0.63-0.66 vs 0.43 / 0.43,
   random 0.16), but a permuted section map did as well: the docs had been written for that change, so the layer predicts
   "the compliance surface", not which amendment. The goal layer is a stack (law section -> research finding -> decision
   row -> spec row -> task -> file); impact from above works through its "names a file" edges; law needs section-level
   nodes and an instrument resolver (which act / jurisdiction, about 80-85% accurate by context here).
   Fourth run, resolved cross-layer keys (`rank_eval --keys on`; James: *"Then we haven't made the connections between
   layers or links like we resolve services"*, *"there could be N to n amount of domains and links required"*): per-domain
   key extractors + one resolver per key type (path with renames, scoped id, id lineage, citation / instrument, table row,
   BM25 text fallback) + one tiered joiner, run over two domains with only the extractors differing. Verdict: the law
   nulls were real absences (resolution doubles the links, no co-change signal, law-impact chains unchanged); the spec
   layer was a resolution / granularity gap (resolved at heading units it beats its control by +0.134 recall@10, raw
   +0.014, mostly through doc -> file paths); glia's rulings, reached through resolved packet links, now beat their
   permutation control (+0.105) and add +0.027 over intent + structure. Typed keys resolve precisely (paths 0.94-0.97,
   scoped ids 0.98 route-weighted, table rows 47/48); reused bare ids (D1-D12 in four series) are the weak spot; the BM25
   text linker hurts everywhere (precision 0.33). The joiner buys coverage, not signal: co-change pairs with no code
   edge that have an upper link rise to 48-51% (16-27% raw). Results: `dev-notes/keys-eval-glia-2026-10-18.md` (glia).
9. **A zero-install `glia` CLI on PyPI** (a maturin `bindings = "bin"` package, so `uvx --from <pkg> glia find ..` works;
   James via repo-graph-8d, 2026-10-18), with its CI workflow and install tests.
10. **repo-graph's six skills move into glia** (orient / find / debug / impact / trace / init) as one family with the MCP
    call, the `glia` CLI and `uvx mcp-repo-graph` as alternate transports, once item 9 ships (James via repo-graph-8d);
    until then repo-graph keeps its six. Licence (PolyForm NC + overlay) is James's to confirm.

### E. From the 0.5.2 waves

Close-out followups that name no 0.5.2 packet are appended here as each wave lands.

- W0 (D0.6): `engine/tests/git_fixture/mod.rs` `root` and `git_mv` are unused in the `contract_breaks` test binary
  (present since 0.5.1). A shared test module compiled per binary; unfinished wiring, not a `dead_code` allow.
- W0 (D0.1): `cargo fmt --check` is not clean in code-domain (`code_span.rs`, `profile.rs`) at 0.5.1; no gate runs
  rustfmt. Candidate: one formatting commit plus a fmt gate in the close-out.
- W1 (DA.3): `rules::value::Tier` is a fourth tier vocabulary, beside the string consts in engine `why.rs` and
  `check.rs` and `implementors.rs`'s uppercase enum. Its words match `why.rs`, so DB maps them 1:1; one shared
  vocabulary is in no 0.5.2 item.
- W1 (DE.1): an ambiguous bare doc mention still binds one candidate at Weak, now stable rather than RepoId-dependent.
  Measured weak binds: glia 240, quokka-stack 193, Kina 4. Alternatives: proximity ranking, or mint no edge (question 4).
- W1 (DD.1): `RUSTDOCFLAGS=-D warnings cargo doc -p glia-activation` fails on links broken before 0.5.2 (`EffectSink`
  in profile.rs; `plan` → the private `ppr_vector`); the `algo` module-doc links went to DD.3 as a correction.
- W1 (DF.1): PHP `use` mid-line (`<?php use \A\{B};`, `namespace X; use \A\{B};`) is still swallowed by
  tree-sitter-php 0.24; the mask reads only line-start statements.
- W1 (DF.2): a PHP namespaced `use` under a root the repo does not own still takes the `tail_unique` guess
  (`use Symfony\Bundle\SecurityBundle\Security` → PACKAGE `App::Security`); a file with several statement-form
  `namespace A; … namespace B;` blocks lists its types under every namespace (needs a parser-side type owner).
- W1 (DF.5): C++ heritage: a `base_class_clause` should become an unresolved INHERITS_FROM ref so `CppScope.bases`
  fills (binds Flutter runners' `this->Show()`, 30 lambda sites, and implicit-this calls to base members); and
  `make_box().get()` arrives as `Attribute { base: "make_box()" }`, bindable through `return_types`.
- W1 (DF.14): a Pub/Sub consumer whose subscription S is bound to topic T ≠ S still pairs exactly with a producer
  publishing to a topic named S; the declared binding contradicts that pair.
- W1 (DF.17): TS locals from `inject()` (`const auth = inject(AuthService); auth.user()`) have no type, and Angular
  template reads (`{{ auth.user()?.displayName }}`) bind nothing (Kina guards and templates).
- W1 (DF.18): an OpenAPI YAML `servers:` sequence at indent 0 (`servers:\n- url: /v1`, what yaml.v2 emitters write)
  reads as no servers; fixing it moves those documents' op qnames, so the packet declares a qname break. Flow-style
  `servers: [{url: ..}]` / `variables: {..}` stay unread.
- W1 (DF.20): `deployment_hosts` could apply a `.replace` span's transform to each alternative and write `hosts`; the
  config-expression grammar could take a SCREAMING_CASE bare identifier (`ADMIN_BASE + '/users'`).
- W2 (DG.7a, DG.8, DH.1): the CJ.1 code guard misses three needle scanners, so glia's self-graph mints phantoms: the
  k8s CronJob YAML scanner in `cron.rs` (`cron:0 6 * * 1:report`), the CLI invocation needles
  (`INVOCATION_PATTERNS`, 10 phantom `cli_invoke` nodes from `cli.rs` comments and test literals), and Ruby source
  (a `# topic "x"` comment in a Karafka file reads as a site; LazyGuard covers `.rs` / `.py` only). Each is a declared
  graph-content change for glia's self-scan.
- W2 (DD.4): `ProjectInfo`, `glia projects --json` and py `project_roots` do not surface the PROJECT ORIGIN's
  `label_source`; showing it moves `py/api_surface` and the `test_arch.py` key pin.
- W2 (DF.16): a Go handler base that is a `self.<field>` alias or a chain rooted in another local
  (`h := svc.Handlers()`) keeps base `h`; binding it needs the registering callable on the ROUTE ref.
- W2 (DH.20): Vapor: SelfMethod HANDLED_BY from a ROUTE (`boot(routes:)` with `use: self.index`, the common controller
  shape) needs the registering method's owner; grouped builders (`app.grouped("users")`) do not join the prefix; a
  client call with a trailing closure mints a phantom `POST /https://...` ROUTE; `app.get("/users")` reads
  `GET //users`; `app.get(use: index)` mints no `GET /`.
- W2 (DH.19): reitit route emission should read the route-data map with `map_entries` (emits `{:post {:handler b}}`
  too; a declared new-ROUTE break); a Clojure FUNCTION behind an unbound `:as` alias has no HANDLED_BY fallback
  (`unique_global_method` looks only at METHODs).
- W2 (DH.15): Dart injectable `@factoryMethod` / `@module` / prefixed annotations (`@di.injectable`); a shelf
  handler that is a method of the enclosing class (`router.get('/x', _list)`) and `@Route.get('/p')` need
  class-scoped handler refs.
- W2 (DH.13): Ruby: dry-auto_inject readers could bind `user_service.find(id)` through a recorded field type; a scoped
  superclass reduces to its last segment (`ActiveRecord::Base` → an app `Base`); registrations in another file than
  the include bind only by the camelised-name convention.
- W2 (DH.14): PHP enum methods are not nodes (`visit_enum` does not walk `enum_declaration_list`), so
  `enum E implements I` gets no method-level IMPLEMENTS.
- W2 (DH.1): a Bunny fanout / topic consumer (`q.bind("logs")`) does not name the exchange a key-less fanout
  `.publish(` goes to; the producer keeps `queue_producer:unresolved:rabbitmq`.
- W2 (DG.3, DG.4, DG.5, DG.7a, DG.8, DG.9, DG.10): Rust / Scala matrix residuals. Event bus: Akka Java / Kotlin
  `getEventStream()`, a bare-imported `Sender<Event>`, async_broadcast, an enum only inferred from
  `broadcast::channel(16)`. GraphQL: Juniper / Sangria (also in D), `#[derive(SimpleObject)]` root struct fields. WS:
  an axum fn mounted on two routes reads its first path; `get(Self::chat)` pairs only through route inheritance;
  Akka's Java DSL and Scala 3 braceless directives. Cron: tokio-cron `JobBuilder` chains, multi-statement run closures,
  a lifetime in call args unclosing the quote scan. CLI: scala.sys.process pipeline operators, an argv built elsewhere,
  `.run` without parentheses, `Seq.apply(..)`. DB: Slick tables through a type alias, a generic base or a qualified
  `extends profile.api.Table[`. HTTP: Akka `pathEnd` / `pathSuffix` / `rawPathPrefix` / `|` prefixes, Scala 3
  brace-less blocks, handler names for inline directive blocks.
- W3 (DH.11): the W2 code-guard cluster grows: the `data_entities.rs` SQL-literal scan and the older migration DSL
  needles do not read through `LazyGuard`, so glia's own `data_entities.rs` / `migrations.rs` test literals mint 67
  self-noise ACCESSES_DATA edges at HEAD (a declared graph-content change for `.rs` / `.py`).
- W3 (DF.10): `glia serves` lists external HTTP ENDPOINTs (match `external`) but not the external WS / gRPC / GraphQL
  clients DF.10 now marks; `serves::external_servers` is ENDPOINT-only.
- W3 (DF.15): CI.5's parser-side `import_dirs` and the `external_pkgs` / `import_local_names` skip set still use path
  spellings, so a Go package imported without an alias under a clause name unlike its directory keeps the
  `api.Server` field-owner miss; needs a graph-side owner mapping.
- W3 (DF.22b): TS inherited calls: a STATE_VAR declared on a superclass is not looked up (`nearest_definer` reads
  `class_methods`, METHODs only); CL.5a's `[recv] constructed receivers` tally still counts a receiver this pass binds
  as unbound.
- W3 (DF.13): Python signals: re-exports through `__init__.py` and star imports bind nothing (the parser emits no
  ImportStmt for `from x import *`). Decorator handlers (`@ee.on`, `@sig.connect`, `@receiver`) anchor to the module,
  not HANDLED_BY the decorated def, because the decorator line sits outside the def's POSITION span (`anchor::attach`,
  every Python decorator site). `marker_swap::swap` could take the evidence rule and the IMPORTS behaviour as
  parameters, shrinking `grafts.rs` `swap_signal_nodes` to a `replace_event_nodes` call.
- W3 (DG.11): ScalaTest: a suite extending an in-house base (`MySpec extends BaseSpec`, BaseSpec extends AnyFlatSpec)
  is not recognised (needs a resolver-level base walk); Scala 3 brace-less `test("x"):`, cases registered in loops or
  helper defs, and fixture-suite bases (`FixtureAnyFlatSpec`) mint no case.
- W3 (DH.2): Laravel queues: `implements ShouldQueue` on the next line, `self::dispatch()` / `static::dispatch()`,
  `dispatchSync` / `dispatchIf` / `Queue::push(new X)`, subclasses of a ShouldQueue base and the global `Redis` alias
  have no rows; a job consumer is anchored at its class line, not HANDLED_BY `handle()` (the CL.4 binding; the
  coverage caveat went to DF.21).
- W3 (DH.5, DH.10): Dart `Platform.environment.containsKey('X')` / `String.fromEnvironment('X')` and Elixir
  `Application.get_env` are unread; PHP `exec(` (collides with `regex.exec`), a fully qualified
  `new \Symfony\...\Process(`, package:args `aliases`, pubspec `executables:` and gemspec `executables.concat(..)` /
  `+=` are unread.
- W3 (DH.8): GraphQL: Absinthe `import_fields` is not expanded across modules; graphql-ruby root classes are found by
  name, not through `query Types::X`; a Dart document passed by value (`document: listOrders`) or QueryOptions built
  outside the call is unread; anonymous `query { .. }` is unnamed. `graphql.rs`'s private `elixir_code_mask` could
  become DH.10's `CodeMap::script(.., Script::Elixir)` once that lexer blanks sigils.
- W3 (DH.9): Ratchet: a Slim / Laravel `->route(` in a Ratchet file reads as a socket route, and `"/ws/$x"` is read
  verbatim; Dart `Uri(scheme: 'ws', ..)` and adjacent-literal concatenation read as the generic `ws_client:ws`; the ws
  call-argument reader is a private copy of `queue_topic.rs`'s.
- W3 (DH.11): sqflite renames, Ecto `rename table(:a), to: table(:b)` (only `a`), `create(table(:x))` and Dart raw
  strings `r'users'` name no table.
- W3 (DH.21): libcurl: handle state is per function (options set in a helper read as GET), the handle key is textual
  (`curl` ≠ `self->curl`), and non-literal URLs (an `snprintf` buffer, `BASE "/x"`) name nothing; a const-table fold
  of `#define BASE "http://.."` would cover the macro case.
- W4 (DG.13a): Rust: no method-level IMPLEMENTS (the parser mints no METHOD under a trait, `visit_type`), and no
  IMPLEMENTS at all for an impl whose self type is declared in another file (glia's own `impl GraphSource for
  MergedGraph` / `for RepoGraph` in `graph/src/traversal.rs`), for tuples, references, blanket impls and impls inside
  fn bodies; the self type could bind by the same path rules (`type_in_scope`).
- W4 (DC.5): glia's self-graph (the installed MCP) has no nodes for the functions in `engine/src/build/mod.rs` and
  `assemble.rs` (`finish_build`, `build_graphs_for_repo`); check on HEAD with the self-graph hygiene group (§F.2).
- W4 (DG.7b): HOCON `include` and `${X}` substitutions are not followed; a Quartz schedule whose expression comes only
  from an included file or an env substitution is dropped (counted in `unresolved=`).
- W4 (DG.2): Alpakka AMQP: a consumer-side `TemporaryQueueSourceSettings(..).withRoutingKey("k")` or
  `BindingDeclaration(..).withRoutingKey("k")` reads as a producer of `k`; gate `.withRoutingKey(` on a receiver chain
  rooted at AmqpWriteSettings / WriteMessage.
- W4 (DE.4): `engram-export/tests/keep_v6.rs`'s list of tests pinning `--since` accuracy could name DE.4's four
  `diff.rs` rename tests (no 0.5.2 packet owns the file).
- W5 (DG.13b): Rust DI depth: a handler extracting a wrapper struct (`State<AppState>` holding the services) injects
  AppState, not its fields; following AppState's field types would reach the services.
- W5 (DH.12): elixir-grpc: a file only handed a channel (`GRPC.Channel`, no `GRPC.Stub`) gets nothing for a service
  without a Service / Svc suffix; the server side (`use GRPC.Server, service: ..`) mints no GRPC_SERVER.
- W5 (DD.7): the SCIP snapshot is read and its fresh documents re-hashed twice per build (the Resolve identity pass,
  then the Post SCIP stage); a cache on CodeBuildCtx could share the read.
- W5 (DH.7): Laravel string events (`Event::dispatch('order.placed')`, `Event::listen('order.*', ..)`), the
  EventServiceProvider `$listen` array, `broadcast(new X)`, paren-less Elixir remote calls and Rails
  `ActiveSupport::Notifications.monotonic_subscribe` / `subscribed` are unread.
- W5 (DH.3): Dart SNS (`aws_sns_api` `sns.publish(topicArn: ..)`) has no row; redis_task_queue's
  `worker.handle(type, handlerFn)` and dart_amqp's `consumer.listen(cb)` bind no HANDLED_BY; dart_amqp
  `exchange.publish(msg, 'routing.key')` never reads the routing key.
- W6 (DF.11): a layout merge (`CodeBuildCtx::for_merge`) has no RepoInputs, so declared TESTS edges built by
  `glia build` disappear in `glia merge` (it drops `pass:tests` edges and recomputes them); the same gap hits CJ.4's
  ORIGIN and overlay `[[edge]]` stanzas. The merge could re-read each member's `.glia/overlay.toml` from its recorded
  root (for_merge already carries them as scip_roots). No 0.5.2 packet owns `engine/src/merge.rs`.
- W6 (DF.11, for 0.5.4's precision rate): every module under a declared `[walk] tests` path pairs by stem, barrels and
  helpers included (`qa/index.ts` -> up to three `index` modules, Python `__init__`); exclude generic stems (index,
  mod, __init__, main) for declared modules if real repos show noise.
- W6 (DF.4b): Dart `export` barrels are not read (an `import 'package:app/app.dart'` barrel binds nothing it
  re-exports); `hide` is treated as visible; calls inside closures (`builder: (..) => const MyCart()`) are not entered.
- W6 (DH.4): Elixir brokers not read: BroadwaySQS.Producer (`queue_url:`), BroadwayRabbitMQ.Producer (`queue:`),
  ExAws.SNS.publish (`topic_arn:`), KafkaEx / Elsa producers and consumers.
- W6 (DH.4, DH.7; DD.10): small duplicates for one owner of both files: `eventbus.rs`'s PHP helpers vs `queues.rs`'s,
  and `cli/src/cmd/blast_radius.rs`'s place / also_in formatting vs `query/find.rs`'s location_cell (move to
  `cli/src/common.rs`).
- W7 (DH.16): Dart calls inside closures are not extracted (`collect_calls_in` in `parsers/code/dart/src/lib.rs` skips
  `function_expression`), so `test('sums', () { total(..) })` mints no CALLS and matrix/dart/tests stays partial. Credit
  a closure argument's calls to the enclosing FUNCTION / METHOD as DF.5 did for C++ lambdas and CA.1 for Go closures,
  then re-measure the dart calls / tests cells (every package:test `test(..)` / `group(..)` body binds).
- W7 (DF.21): residuals now written into the coverage rows: C++ heritage (no INHERITS_FROM, so base-member calls never
  bind); C++ `auto` from a qualified call, member call, trailing return or init-capture; Swift return-value / chain
  receivers and `any P` / `some P`; Dart `export` barrels, closure-body calls and parameter / local receivers; Laravel
  job consumers HANDLED_BY `handle()` (the CL.4 way); Pub/Sub subscriptions created in code and topic refs across repos;
  CMake `$<IF:..>` both branches and headers generated under CMAKE_CURRENT_BINARY_DIR.
- W7 (DH.17b): Elixir, graph side: an INTERFACE's FUNCTION children into module_symbols, with enclosing_package /
  bare_call_target treating an Elixir INTERFACE like a PACKAGE (then the parser's `BodyShape::is_contract` can drop
  `!self.children`); IMPLEMENTS / INJECTS refs through ElixirModules' full-name index, not the repo-wide bare-name
  fallback (`@behaviour Ecto.Type` -> an in-repo `MyApp.Type`); `import` / `use` keep their IMPORTS edge but bind no
  alias, and `alias X, as: Y` binds Y; LocalModuleIndex declares an INTERFACE's dotted prefixes like PACKAGE prefixes;
  StreamData `property` blocks as tests and ENDPOINTs for HTTP calls in test bodies.
- W7 (DC.7b): a watch update deep-clones every group (50-221 ms on glia) by design; cutting it needs passes that do not
  mutate group graphs (§A, A2).
- W8 (DA.18): rules witnesses are shortest at the row's own level only: a row keeps one height, so a lower-level prefix
  is read at its own level's height (graph 4: 99 of 3,812 tc rows longer than the lattice BFS depth). Per-level heights
  (relation.rs Batch heights per level, the exec.rs binding height from the view's level, explain.rs searching under
  the level's height) make every witness BFS-shortest at its tier; widen oracle.rs test 9 to every row of every graph.
- W8 (DB.2b, DA.19, DA.14b): the rules row budget counts only fully emitted bindings, so a join that multiplies
  intermediate bindings runs unbudgeted (budget per step, or count Scan / Lookup emissions); and it counts bindings plus
  inserted rows (about 2N for N rows), while the Budget error says "derived rows". Both belong with §F.3's evaluator work.
- W8 (DA.19): closure.rs's test oracle uses HashSet; switching it to BTreeSet lets `rules/tests/determinism.rs`'s source
  guard drop its `in_test` exemption.
- W8 (DH.18): ACCESS_MODE for parser ORM edges (Ecto's Repo function names read vs write; only the extractor re-home pass
  writes ACCESS_MODE today, for no parser's ORM edges - Ruby, PHP, Go, Elixir alike); Repo calls inside ExUnit `test`
  blocks and module-attribute queries (`@query from(u in User)`) are unread; same-name schemas in different namespaces
  share one `data_entity:sql:User` (the A13.1 last-segment rule).
- W9 (DD.16, DF.19): contract_route_classes (`graph/src/resolvers/identity.rs`) should try link_contract_routes'
  ORIGIN `bases` / `annotated_base` tiers, through one shared lookup in `graph/src/resolvers/http.rs` that both it and
  `engine/src/passes.rs` call; it adds SAME_AS contract_route edges and moves identity_resolver.rs pins, so its own
  packet with a fixture.
- W9 (DC.10a): the watch group key's `overlay_fp` is redundant with the post-graft parse fingerprints for every overlay
  section today; dropping it would let an overlay change rebuild only the groups it moves.
- W9 (DC.8): watch debounce has no max-latency cap, so a path whose (len, mtime) moves every tick is never applied.
- W10 (DC.10b): contracts for any later watch work (no 0.5.2 owner remains): a new language group or High input class
  needs a `group_of` arm in `engine/tests/watch_parity.rs`'s random oracle (plus a not-a-table-entry exclusion when the
  input is not a walked file), and a new edit op needs a weight that keeps it in the default 15-card sequences.
- W10 (DC.10b): `glia delta` and watch rows report no row for an edge whose EVIDENCE changes while its confidence does
  not (a SCIP CONFIRM of an already-Strong edge); reporting evidence re-stamps is a new row kind and a declared break.
- W10 (DD.13): `serves` folds identity classes of servers only; if an identity rule ever pairs routes or consumers,
  decide whether the kept server absorbs the folded rows' handlers.
- W11 (DB.4, DB.3b): a `[[constraint]] rule` with no `violation` output predicate is a `glia check` error (exit 2)
  but no `rotted_rule` gaps row, because the row reads `rules::validate`, which has no output-predicate check; add the
  check to validate (`engine/src/rules/query.rs`) or to the row.
- W11 (DB.3b): each transitive forbid rule re-tiers every kept edge of its chain (`edge_tier_of` -> `Evidence::of`);
  several transitive rules over one big merge could share one per-edge level vector on Ctx, keyed by edge ordinal.
- W12 (DB.3c, DB.3d): review renders a rule / forbid_path identity as `_subject: ... - tier <t> at <file:line>_`, the
  witness table, then up to 5 `⚠` caveat lines with a `(<shown> of <total> caveats)` footer; check's rendering landed in
  the same wave, so confirm the two surfaces match and share one renderer.
- W12 (DB.9b): `overlay try` skips leave-one-out for `[[constraint]]` stanzas because no declared rule mints a node or
  edge; if one ever does, the skip goes (`overlay_loop_rules.rs` pins the delta and fails first).
- W12 (DD.14): under TopLevelDir / ProjectRoots keying, an unlocated SAME_AS member (PACKAGE_DEP) stays unplaced in
  `arch`; placing it needs the manifest's PROJECT, which has no edge to its dependencies today.
- W13 (DD.15, with DD.11 / DD.12 / DD.16's open handoffs): one packet owning `engine/src/identity.rs`, `answers.rs`,
  `trace.rs` and `tests_for.rs` adds `IdentityView::walk_members(rep)` / `in_walk_class(id)` and deletes the three
  SAME_AS-endpoint stopgaps (`tests_for::WalkClasses`, `answers::walk_class_reps`, `trace::WalkNodes`).
- W13 (DD.15): move the repo-label fallback (`collapse.repo_labels.is_empty() -> rev.after.repo_labels`) into
  `diff_impact_from_delta`, so every caller of `diff_impact_vs_rev` / `diff_impact_from_delta` gets labelled
  `also_in` repos and the CLI / py surfaces can call `diff_impact_vs_rev` again.
- W13 (DB.8b): `reflexion::derive` runs unbudgeted (`max_rows = usize::MAX`); a finite budget must exceed |placed nodes| x
  |declared paths| bindings for `owns`.
- W13 (DB.9c; joins W0's fmt item): `py/src/overlay_loop.rs` is rustfmt-dirty outside DB.9c's lines; the one formatting
  commit plus a fmt gate covers it.
- W14 (DZ.2): the header comment of `engine/src/rules/reflexion.dl` still calls the native functions the parity oracle
  "until DB.8b deletes them"; the goldens (`engine/tests/data/reflexion/`) are the oracle now (comment-only; engine is a
  PARSER_STAMP root, so the first 0.5.3 packet in `engine/src/rules` or the release docs packet makes it).
- W14 (DZ.2), superseded 2026-10-18 (§0): `dev-notes/repo-graph-handoff-0.5.2.md` is the active 0.5.2 handoff
  (`glia-py==0.5.2`), and **0.5.3's release docs packet writes its own 0.5.2 -> 0.5.3 repo-graph handoff** with the
  `glia-py==0.5.3` pin; the 0.5.2 rows name the wrapper sites at repo-graph 0f8ecfa, so re-read the wrapper's line
  numbers at its HEAD then.
- W14 (DZ.2): `scripts/check-neuropil.sh` still copies neuropil verbatim (open since the 0.5.0 handoff); the neuropil
  handoff's NEUROPIL_DIR copy recipe stands in until neuropil renames.
- W14 (DZ.1, DD.16): `scripts/eval_substrate.py:229` keeps a PackageResolver row for category 31, and
  `bench/substrate-gap/matrix_vocab.py` REPO_PAIRWISE_CATEGORIES / AUTHORING.md's "SHARES_* (all seven)" still list
  SHARES_DEPENDENCY, which no resolver emits since DD.16 (drop it while test_matrix.py stays green).
- Kina demo of the four bets (2026-10-16, `git archive` of Kina 8ac238c, glia c4987609) - candidates for 0.5.2 tweaks
  before the release, else 0.5.3 (James decides):
  - **A `forbid_edge` between directories inside one project never anchors.** `from = "backend/internal/handlers"` (a
    directory of the `backend` project) prints `[declared] orphaned constraint ... (no PROJECT)` on stderr and the rule
    silently leaves `glia check` (`rules=1`, not 2): the commonest layering rule (handlers -> repository inside one
    service) cannot be written as a forbid; the same check as a rule invariant works (8 handler methods call repository
    methods directly, FACT, located). Extends §D's "a `[[constraint]]` with no anchor is orphaned silently".
  - Routes registered in `_test.go` files count as product routes: `unhandled_route` lists `GET /panic` and
    `POST /api/auth/password/reset/:token` from `backend/internal/middleware/accesslog_test.go`.
  - `glia check` repeats every coverage caveat paragraph under every violation (three long HANDLED_BY paragraphs per
    row); print each caveat once per rule.
  - GQL answers carry no location (`at` is `-`), while the same Datalog query locates every row.
  - **Smart contracts are not modelled across the stack.** Kina's `EscrowManager` is defined in `contracts/` (Solidity,
    Hardhat), called from the frontend (ethers, 7+ files) and the backend (`ethclient` in `signer.go` / `indexer.go`);
    no edge joins a TS / Go contract call to the Solidity function, and identity has no ABI rule. A contract-call
    extractor plus an identity rule (Solidity contract <-> its ABI clients) is the cross-stack link a dApp needs.
  - **The client <-> server contract model is not identified - IN 0.5.3** (James, on Kina's 0 identity classes: *"the
    frontend talks to backend as a service definition somewhere so thats incorrect"*, then *"straight into 0.5.3 for that
    one"*). Kina has no OpenAPI / proto: its contract
    is code, the Angular services + `core/models` (`export interface Offer`) and the Go handlers + `models.go`
    (`type Offer struct` with `json` tags). glia pairs the calls (108 HTTP_CALLS pairs) but not the models, and on
    quokka-stack the 39 contract_op classes join swagger, the Go annotation and the feature spec, never the web /
    Android client models. Candidate: an identity rule pairing a client model and a server model through the paired
    endpoint <-> route (the client's typed call `http.get<Offer[]>`, the handler's serialised struct) and their JSON
    field names, plus model drift (a field one side has and the other does not serialise) in the contract-fields answer.
  - quokka-stack's NATS JetStream chat mints **no queue node at all**: `conn.Publish(notifSubject(targetUserID), payload)`
    on a `*nats.Conn` parameter and JetStream subjects from `config.Appconf.NATS.JetStream.Publish.SubjectTemplate`
    give 0 QUEUE_PRODUCER / QUEUE_CONSUMER nodes, while the QUEUE_FLOWS caveat says such topics are "extracted as an
    unresolved framework tag". Either mint the unresolved producer / consumer (so the flow is visible and caveated) or
    correct the caveat; then fold config-templated subjects through the config keys.
  - pyo3 `PyGraph.neighbours(.., categories=[..])` takes category ids only: `["CALLS"]` raises `TypeError: argument
    'categories': 'str' object cannot be interpreted as an integer`; accept NAMEs as every CLI flag and `glia query` do.

- W15 (DT.1b): TS re-export barrels are not followed: `export * from` / `export { X } from` emit no ImportStmt, so a
  name imported through a barrel gets the IMPORTS edge to the directory's index module (DT.1b) but no binding.
  Measured: quokka-stack web imports from `@app/core` in 32 files through two levels of barrel (`core/index.ts` ->
  `./models` -> `models/index.ts` -> `./user.model`); Kina `import { WalletActionSheet } from '../../wallet'`. Kina's
  `core/models/index.ts` declares its interfaces itself, so DT.1b already binds those. Candidate for a 0.5.2 tweak if
  James wants it before the release.
- W15 (DT.1b): a `'.'`, `'..'` or trailing-slash specifier names only a directory in tsc, but resolve_ts_source maps it to
  the directory's qname, so a same-stem sibling FILE (`src/app.ts` beside `src/app/`) still wins (0 instances in Kina,
  quokka-stack, examples-glia).
- W15 (DT.2a, James's call): drop the first-hit site kept for all-topic-less queue rows (tag_sites, the `if !tag.mixed`
  line). It removes the spurious USES / HANDLED_BY a tag gets at a call another row named (pika `channel.basic_publish`
  beside `basic_publish(`) and moves such a tag's POSITION to its first unexplained site; a declared edge_removal.
- W15 (DT.4): the trailing query-interpolation idiom in Dart (`'$base/smr$q'`), Python f-strings and Go `fmt.Sprintf`
  clients is not dropped from the endpoint path; DT.4 covers TS / JS only.
- W15 (DT.5f): gqlgen object resolvers (`orderResolver.Customer` for `type Order { customer: Customer }`) stay unpaired:
  extract_graphql_sdl_file_nodes mints GRAPHQL_RESOLVERs for root types only (the marker counts them as object_structs).
  Minting non-root field resolvers (`graphql_resolver:Order.customer`) lets GqlgenResolver's `<Type>.` branch pair them.
- W15 (DT.12a): engram-export's body hash costs about 100 ns/byte in debug builds; a byte loop with an ASCII fast path
  must stay exactly equal to `split_whitespace` + `chars().count()` (the hashes are stored in cell rows). Invariant for
  any later edit: a `nodes.push` in build_gmap_with_nodes also pushes `fact_nodes` (hints_side_v6 (a) fails on drift).
- DT.1o spec (verifier defaults): an overlay-level opt-in for blocking model breaks (`[contracts] models_min_tier`) changes
  the overlay schema, so 0.5.2 has only the CLI / pyo3 flag. Not judged by DT.1o: DT.1i's client_contract pairing (a
  typed client against an OpenAPI op; a server-side op change is already a contract-breaks schema row), DT.1k's TS
  result model against its own selection, and GraphQL argument / input evolution.
- W16 (DT.10a): the Solidity noise gate never fires: `is_literal_primitive` should unwrap tree-sitter's `expression`
  wrapper (unwrap_expression). Fixing it drops STATE_VAR nodes for literal-primitive public constants (a declared node
  removal); gated public constants then take the module-CONTAINS procedure path.
- W16 (DT.5e, James's call): a Riverpod provider passed by name (`ref.watch(xProvider)`, 77 on quokka-stack) as USES ->
  STATE_VAR? The graph half: a USES-by-value ref kind binding only FUNCTION / METHOD targets, so imported top-level
  functions passed by name become USES too (graph/src/calls.rs resolve_refs binds a Bare ref to any symbol kind today).
- W16 (DT.5e): a widget whose in-repo base extends a Flutter widget (`class X extends BaseScreen`) gets no ROLE
  COMPONENT; only the base does. A graph pass following in-repo INHERITS_FROM to a COMPONENT base closes it.
- W16 (DT.6a): ENDPOINT provenance still uses the first POSITION plus the qname rule (`endpoint:GET:/api/ab_test` stays
  test_fixture), and HttpRouteMatcher (the contract linker, `glia serves`) still indexes test-only routes.
- W16 (DT.3b, DF.8): to never mark a const-sourced WS host `external` (CG.4a's reading for HTTP), endpoint_fold.rs needs
  to tell folded WS hits apart: a payload field, or the fold reporting its folded site ids.
- W16 (DT.7b, DT.7a): a sub-scoped `[[decision]]` orphans with the exact-PROJECT rule `(no PROJECT)`; decisions and notes
  are records, so declared_misses does not list them (gaps' orphaned_rule reports stale-anchor ones).
- W16 (DT.1j, self-graph hygiene): embedded SDL (resolvers and now types) is unguarded in `.rs` files by design (CJ.1b);
  only the line-start rule keeps it out of glia's own one-line test strings.
- DT.1q spec (verifier defaults): Dart `export` directives (quokka has 2, both in generated protobuf code) and CommonJS
  re-exports (`module.exports = require(..)`) are not followed; the same NavFacts could carry them. A renamed re-export
  (`export { A as B } from`) binds through a named import only, never `ns.B` on a namespace import of the barrel (kept out
  of module_symbols so the name-only `global_unique` fallbacks cannot bind it).
- W17 (DT.6b): `unhandled_route` still lists client-router redirects (Kina's five `page:/...` `redirectTo` routes); a
  nav-route exclusion is a separate prelude decision.
- W17 (DT.1d): request-side residuals: a receiver field typed AxiosInstance / AxiosStatic writes no request (counted
  request_unknown_client) although axios fixes its body position; a `.then(r => r.json())` fetch chain and ky's
  `.json<T>()` post-call typing are not read; `await this.repo.find<Foo>(1)` yields no CallSite (extract_call_qualifier
  sees an await_expression callee; awaited_generic_callee could unwrap it).
- W17 (DT.1e): Go wire residuals: an encoder held in a local (`enc := json.NewEncoder(w); enc.Encode(v)`), a context
  passed into a helper under another name or type, gin `ShouldBindWith` / `ShouldBindBodyWith(binding.JSON)`.
- W17 (DT.2c): an overlay-pinned queue topic is not minted Weak: the queue fold's resolver would have to report
  ConstTable::is_pinned (e.g. extract_queue_nodes_with_consts taking a resolver returning (value, pinned)); spans
  engine/src/build/grafts.rs and parsers/code/extractors/src/queues.rs.
- W17 (DT.11b): GraphQL transport residuals: apollo-angular `httpLink.create({ uri })`, aliased imports
  (`import { ApolloClient as AC }`), namespace calls (`new Apollo.ApolloClient(..)`) and CommonJS-destructured urql mint no
  transport ENDPOINT.
- W18 (DT.11c): an unowned GRAPHQL_RESOLVER (an SDL file outside every project) looks routes up under owner None only,
  so a route inside a nested project of that repo is not seen and the transport is exempted; reading an unowned resolver
  as covering every project of its repo is the conservative alternative.
- W18 (DT.1f): Kina `[go-wire] unresolved=51` is mostly calls whose first result is a predeclared scalar (return_types
  records nothing for scalars, so a scalar looks unknown), plus `err.Error()` and function-local types; a Go parser fact
  for scalar first results would leave only real misses in the count.
- W18 (DT.5c, question for James): an external template's receiver CALLS carry the component's `templateUrl` row in its
  TS file, not the `.html` row, because CallSite has no file field; adding one touches every parser's CallSite literal.
- W19 (DT.5g): the TS parser's collect_calls_in skips a `function f() {}` / `class K {}` declared inside any walked body (a
  describe callback's `async function setup()`, a helper inside a method): only the name is recorded as a shadow, and the
  declaration's calls, endpoints and value references are never read. Walking it (credited to the enclosing scope, as
  nested arrows are) or minting a nested FUNCTION closes quokka's activities spec providers; an undeclared CALLS change,
  so its own packet.
- W19 (DT.5g): a Bare USES ref bound through a default import to a MODULE could look through to the module's
  default-exported declaration (as heritage_through_module does for heritage); DT.5g drops those refs in the parser.
- W19 (DT.5g): JSX attribute handlers (`onClick={handle}`) and `export default <expr>` objects are not value positions;
  a React `export const X = memo(() => ..)` credits the arrow's calls to the STATE_VAR X, which the role fold cannot merge
  with a COMPONENT overlay (untested on real React repos).
- W20 (DT.10i, by design): an ethers class-field binding is read only inside its own class body (a subclass in another
  file calling `this.escrow.fn()` is not read); `R[name]` with a const-held name counts dynamic and is not folded through
  the const table; `this.x = new ..` outside any class (a JS function constructor) binds nothing.
- W21 (DT.10k): go-ethereum `abi.ABI.UnpackIntoMap(m, "n", data)` (name argument 1, a function or an event) and
  cross-package `escrowbind.EscrowMetaData.GetAbi()` / `.ParseABI()` bindings in user code are not read (only a same-file
  metadata declaration binds).
- W21 (DT.10d): hardhat-deploy `ethers.getContract('C')`, plain ethers `new ContractFactory(abi, bytecode, signer)`, waffle
  `deployContract(wallet, Artifact)`, a factory rebinding `const f2 = F.connect(s)`, an inline
  `(await ethers.getContractFactory('C')).deploy()` and a factory passed inline to deployProxy mint no contract client.
- W21 (DT.10d, question for James): hardhat test sites hang off the test MODULE (CONTAINS, not a carry edge), so
  `tests-for` on a Solidity function does not reach the mocha test; that needs FUNCTION nodes for `it(..)` / `beforeEach`
  callbacks.
- W22 (DT.10f, DT.10k): an EVM name argument held in a function-local `<id> := "<lit>"` or a same-file
  `const <X><E>EventName = "<E>"` is not resolved (CA.4's no-value-tracking rule; abigen v2's UnpackXEvent
  `event := "Released"` counts dynamic=2 per event).
- W22 (DT.10j): not read: a spread of a same-file config object (`{ ...escrowContract, functionName: 'x' }`, a common
  wagmi pattern) counts dynamic; viem `encodeFunctionData` / `decodeFunctionResult` / `decodeEventLog` (the TS twin of
  Go's Pack); the standalone `web3-eth-contract` `new Contract(abi, addr)`; truffle's global `web3` without an import.
- W22 (DT.1l, James's call): quokka's GroupUser / GroupLastMessage read every key through a same-file
  `_pick(j, ['userID', 'userId', 'UserID'])` helper, so they write no SCHEMA_FIELDS (21 of quokka's 23 fromJson models
  do); a same-file key-alias helper rule (a Map plus a List<String> of literals, the body subscripting the map by an
  element) is interprocedural.
- W23 (DT.1k): a GraphQL document built by string concatenation, or an inline gql`` at the call site, stays unresolved;
  graphql-codegen TypedDocumentNode results typed by a type alias (`type GetUsersQuery = {..}`) mint no node, so they form
  no graphql_model class (checkable once TS type aliases carry shapes).
- W23 (DT.10g): the cross-file ABI join (engine/src/build/evm_abi.rs) does not follow TS re-export barrels
  (`export { X } from './abi'`): DT.1q's ReExportName / ReExportStar / ExportLocal facts sit in the parses Index::build
  reads, so a chain is a lookup; also unread: a local export list `const X = [..]; export { X }` (ExportLocal covers it)
  and tsconfig path aliases (TsAliasSet could be passed in). A Go ABI held in a package `var` of another file is not a
  GoStringConst (DF.7's FileConsts records consts only). ClientHit could fold into graph/src/resolvers/evm.rs ClientFacts
  (change the test helper `cell()` to `..Default::default()` first).
- W25 (DT.1p; joins W0's D0.6 `git_mv` item): a spec file moved with `git mv` and otherwise untouched reads in
  contract-breaks as its op `removed` (breaking) + `added`; the op should pair through the move, as LB.6 moves pair nodes.
  The move test is the missing user of engine/tests/git_fixture's `git_mv` (the contract_breaks crate's dead_code warning).
- W25 (DT.1p, DT.1m): WirePair.client_model is None for a union client root, so a union-judged model break is keyed on
  its ENDPOINT, not the model; naming the picked member there would fold it into the model pair's row.
- W26 (DT.13, DT.1p): cli/src/cmd/contracts.rs's module doc and `--fields` help name only http_model / client_contract;
  add graphql_selection (regenerate cli/surface/contracts.txt if the help text is in it).
- W26 (DT.13; DT.11c, DT.6b, DT.3c): docs/overlay.md has no text for the implied GraphQL transport exemption and the
  `graphql transport; ` detail, the withheld test-file routes or WS-served routes (the skills and CLAUDE.md carry them).
- W26 (DT.13, DT.1k): engine/src/contract_fields.rs element_kind reads an envelope-or-array client union
  (`{ activities: C[] } | C[]`) as no kind, so a pair the models pass confirms stays client_type_unresolved (quokka-stack
  52 rows); it should read graph::resolvers::models::union_member (made pub(crate) / pub on the graph crate).
- W26 (DT.13, DT.11z): a WebSocket base bound once per deployment (environment.ts + environment.prod.ts) stays unfolded;
  port endpoint_fold::deployment_hosts to WS_CLIENT.
- W26 (DT.13, DT.5g): a getter read through a typed receiver or injected field (`this.authService.isAuthenticated`)
  should be a CALLS of the other class's accessor in graph/src/calls.rs's receiver pass (quokka-stack has 2).
- W26 (DT.13), for the repo-graph handoff (0.5.2's §9 lists them; 0.5.3's carries any still open): optional wrapper fixes - `_dial_text` returns None for an HTTP
  call-site payload (has `method`; DT.11b's GraphQL transport hits end `"via":"graphql"`) and renders `via=evm` hits by
  library / ABI; `_cell_text` could render SCHEMA_FIELDS as `name: type` pairs.
- W27 (DT.5l, its own risks): gate a Go `init` entry on its package being imported by live code; an ext-gated named-entry
  row with prefixes would need hubs.rs and duplicate_flows.rs to call accepts_ext; the external-base invoked-member rows miss
  aliased imports (`import { TranslocoLoader as TL }`) and in-repo intermediate bases.
- W28 (DT.5i, DT.5e): a Riverpod provider read is still not a USES of its STATE_VAR (the CH.4 / DT.5e decision): a
  provider initialiser's callees (ActivityService::mine via myActivitiesProvider) get an incoming CALLS but reach the live set
  only when the provider itself is live. Not read: riverpod_generator `@riverpod` providers (a `.g.dart` part file),
  `ref.read(p.future)` and FutureProvider / StreamProvider values (AsyncValue, on purpose), AsyncNotifier and bloc Cubit /
  Bloc state.
- W29 (DT.5j): a Go value ref or composite type named through a dot-import, or a block-scoped local type in another block of
  the same fn, is read with the flat per-fn shadow table (conservative: it drops, never binds wrong).
- W29 (DT.5j; bench): grade.py forbid rows cannot filter by EVIDENCE rule, so DT.5j removed models-go-wire's
  `Fleet::Get -USES-> models::vehicle::Problem` forbid (written for DT.1f's go_wire edge; Get builds `models.Problem{..}`,
  so the new composite_literal USES is correct). Add an evidence-rule filter to forbid rows and restore the guard as
  "no USES rule json_response to Problem".
- W30 (DT.5k, its own risks): an accessor inherited from a superclass is not bound (heritage edges are drawn after
  resolve_calls); member reads in Angular templates (DT.5b) are not covered; `TestBed.inject(T)` locals stay untyped (20 sites
  across Kina and quokka-stack); the static-member fallback keeps LA.30c's Upper.Upper filter, so a lowercase static like
  `Api.baseUrl` stays unbound cross-file; `Promise<T>` / `Observable<T>` return annotations name only the wrapper.
- W31 (DT.14), for the consumer handoffs (0.5.2's list them; 0.5.3's carry any still open): repo-graph's `read` labels a parser-read main_guard ENTRYPOINT cell "declared
  entry point" (server.py:663 / :703; label it "entry point" when source == main_guard, repo-graph handoff 10.1 row 1);
  neuropil's HUD cell_label (hud.rs:1552-1569) falls back to "CELL" for ROLE and ENTRYPOINT (optional arms, neuropil handoff
  11.4).
- Engram's registered run at `0.5.2-complete` (196728e3, 2026-10-18; Engram 11b3a3c, docs/eval-plan-2026-09.md): nothing
  broke. PPR as a release pool source FAILS (flat-ppr vs flat-fs pooled: hit 9 wins / 20 losses, strict 12 / 21; guards
  equal), so Engram does not use `--ppr-candidates`: DE.5's side file stays available but unused (decide keep / drop in
  0.5.3). DE.5 format clean on 771 steps. Re-keys with X1 on / off: renamed Kina 17/18 vs 4/18, glia 28/40 vs 9/40. `since`
  pairing summed: Kina words 17, hint 0, hint_refused 14; glia words 39, hint 30, hint_refused 32, name 284 (the
  hint_refused counts are worth a look: why the export's v1 hint is refused for a re-keyed fact). Headline, Engram on the
  tag's exports vs 0.5.1's: Kina hit +27 / -23 (flat); glia strict +15 / -2 (p = .0023). Live Kina synced with the tag's
  exporter; the corrections writer passes `--hint`.
- 0.5.2 release CI (wheels-py.yml run 39046742898, 2026-10-18, green): GitHub warns `actions/checkout@v4` and
  `actions/upload-artifact@v4` target the deprecated Node.js 20 (forced onto Node 24), and `ubuntu-latest` moves to Ubuntu 26
  from 2026-10-19; bump the actions and pin the runner image before 0.5.3's wheels build.

## 3. Open questions for James

All four answered 2026-10-16 (§0); recorded as rulings:

1. Identity-scoped queries: **out of 0.5.3.** Revisit with a hosted repo-graph; provenance on every node and cell
   first, filtering by the host.
2. The semantic tier feeds `find` and the flow search (§F.6), and **`pack` as a fallback only** (seeds only when
   nothing matches by name, each labelled HEURISTIC with its similarity). Blast-radius never seeds from vectors.
3. **glia's half first, against fixtures** (fixture `.glia/vectors.jsonl` files, a deterministic test-only embedding,
   the VectorRow contract and its validator), **then glia embeds itself** (revised 2026-10-16, §B): `glia embed` with
   fastembed / bge-small, the query embedded in-process. The Engram ask (`dev-notes/engram-ask-0.5.3.md`, draft)
   shrinks to corrections as cells and reading glia's vectors; James decides Engram's direction before it is sent.
4. An ambiguous bare doc mention: **proximity, then the clarification queue.** Bind the one candidate a recorded rule
   picks (same directory / project / service as the doc, or its owner also named in the section) at Weak with the rule
   as evidence; when no rule separates them, mint no edge, queue the question (§2 C) and report a gaps row. Measure at
   spec time how many of glia's 240 / quokka-stack's 193 / Kina's 4 proximity settles.

## 4. Spec run

After the 0.5.2 release (2026-10-18, §0): the same method as 0.5.1 / 0.5.2 (per group a spec agent and an
adversarial verifier, one integrator for wave 0 and the release docs), packet ids `E<group>.<n>[letter]` with `E0.*`
as wave 0, packets in `dev-notes/leap-053-packets.json`, the runner's `--release 053` mode.

**Followups finish inside 0.5.3** (James, 2026-10-16, §0). The 0.5.2 runner routed a followup to a later packet or to the
next leap's seed; 0.5.3 does not defer. After the last scheduled wave (and after each wave whose close-out lists
unrouted followups with no remaining owner), the session clusters them by file, specs each cluster as an `E<group>.<n>`
packet with a fixture or test, schedules a follow-up wave, and repeats until a close-out lists none. The release docs
packet runs last, after the final follow-up wave.
