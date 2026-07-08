# Tier 4 — external doc-source ingestion (Confluence / Notion / wikis)

**Status:** design/scoping (2026-07-08). Prereq — Tier 1-3 substrate complete
(blind 48→0). This extends the substrate from *code* to *external docs*.

## Why (ties to the v6 thesis)

The handoff thesis names **"rules-docs growing past context"** as a graph-wins
scenario. In real orgs the architecture/rules/runbook docs live in Confluence /
Notion / wikis — not in the repo — and they *govern code across many repos*. A
Confluence space that exceeds context, linked to the code it governs, is exactly
the "size × complexity × cross-boundary-ness > context" case, and a moat.

## What exists today (and its one coupling)

glia already ingests **in-repo markdown**:
- `.md` files (well-known root files incl. `CODE_RULES.md`/`CLAUDE.md`, `docs/`,
  `.ai/`) → per-heading **`DOC_SECTION`** nodes (`node_kind 42`), capped prose,
  provenance `documentation` (`engine/src/lib.rs` G18, ~line 1190).
- **`link_doc_sections`** (WP-H) links each doc section to the code symbols it
  names — via **backtick-quoted identifiers** (`` `MyClass` ``) matched to symbol
  names → **`DOCUMENTS`** edges (doc→symbol, ≤25/doc). Docs aren't islands.
- Plus inline docstrings/JSDoc as `doc` cells (all langs).

**The one coupling:** doc collection lives *inside the filesystem walk* (the `.md`
extension check in `build_graphs_for_repo`). Everything downstream (DOC_SECTION
build + `link_doc_sections`) is already source-agnostic — it operates on
`(path, text)`. So the work is a **seam**, an **adapter**, and a **linker upgrade**.

## The three pieces

### 1. The seam — decouple doc-ingest from the file walk
Extract doc-ingest into a source-agnostic entry:
```
ingest_docs(records: Vec<DocRecord>) -> (DOC_SECTION nodes)   // then link_doc_sections
DocRecord { id, title_path: Vec<String>, text_markdown: String,
            provenance: DocProvenance }
DocProvenance { kind: File|Confluence|Notion|Wiki, url, space_or_db, version_etag }
```
The file walk becomes **one** producer (`FileDocSource`) that reproduces today's
behavior; external adapters are others. Provenance rides on a DOC_SECTION cell
(source kind + url + version) so a doc traces back to its page.

**Determinism (critical):** network fetches are non-deterministic → they must NOT
be in the byte-identical build path. Split into two steps:
- **`glia docs sync`** (explicit, network) → fetches to a **content-addressed
  snapshot** (local manifest with per-doc version/etag).
- **build** ingests the *snapshot* deterministically → the byte-identical
  determinism gate stays valid.

### 2. The doc-source adapter (`DocSource`) — a new I/O crate, NOT in engine
Per glia's purity split (parsers/engine do no network I/O; that's why the engine
is deterministic and only `py/` publishes), the fetch layer is a **separate
`doc-sources` crate** driven by the CLI/py, producing snapshots the engine ingests.
```
trait DocSource { fn sync(&self, into: &SnapshotStore) -> Result<Vec<DocRecord>>; }
```
- **ConfluenceDocSource** (highest value): REST (`/wiki/rest/api/content`,
  `/child/page`); body is **storage-format XHTML** → convert to markdown
  preserving headings, fenced code blocks (+language), and **inline `<code>` spans**.
  Auth: API token via env/secret ref. Incremental: `version.number`. Space-scoped.
- **NotionDocSource**: Notion API block tree (heading/code/paragraph/child_page)
  → markdown. Auth: integration token. Incremental: `last_edited_time`.
- **WikiDocSource**: format-varied —
  - GitHub/GitLab wikis = git repos of markdown → clone + reuse `FileDocSource` (cheapest).
  - MediaWiki: `action=query` API, wikitext → markdown.
  - Generic HTML crawl (sitemap) → markdown (lowest precision; opt-in).
- Config: a `docs.yaml` (source kind, base-url, space/db ids, include/exclude,
  auth *reference*). Secrets never in config.

### 3. The doc→code linker — the interesting problem
Current linker (backtick-identifier → symbol) is high-precision but assumes repo
markdown conventions. For external docs:

- **Format is half the battle, solved in the adapter.** Confluence/Notion don't use
  backticks — but they DO mark code (inline `<code>`, fenced blocks). If the
  adapter's markdown conversion **faithfully preserves code spans** (emits
  backticks/fences), the existing linker fires unchanged. So: *invest in faithful
  code-span preservation in each adapter.*
- **Bare prose mentions need precision gating.** "the `OrderService` handles…"
  written without code formatting → matching bare tokens risks false links
  (common words, ambiguous simple names). Tier candidates + carry edge confidence:
  - **Strong:** code-span match, or a qualified name (`billing::OrderService`), or a
    fenced code block naming the symbol, or an explicit source-file link in the doc.
  - **Weak (opt-in):** bare simple-name token that is a *unique, non-dictionary*
    identifier. Never link common/ambiguous simple names.
- **Cross-repo by construction.** `link_doc_sections` already runs on the
  `MergedGraph`; a Confluence space maps to code across N repos. Provenance lets
  you trace back per repo.
- **Richer signals (beyond identifier match):** explicit GitHub/GitLab source links
  in the doc → direct file↔doc edge; page title / labels ↔ module/service name;
  (optional, P3-adjacent) activation-assisted linking for prose describing a concept
  with no named symbol — lower precision, opt-in.
- **Payoff → a P3 primitive:** DOCUMENTS edges make **`governing_docs(node)`** /
  **`docs_for(feature)`** answerable ("what are the rules for X" → code node → its
  doc sections). That is an answer-shaped primitive, straight from the handoff's P3.

## Eval — mirror the substrate-gap approach (recall AND precision)
A **doc-link eval** (`bench/doc-link/`): fixtures of Confluence/Notion/wiki-style
content (raw source-format + expected converted markdown) + a small code graph,
with hand-enumerated ground-truth doc→code links. Measure per source-format:
- **code-span preservation** (did the adapter keep the `<code>`/fences?),
- **link recall + precision** (precision matters *more* here — false links are the
  failure mode). Reuse `bench/grade.py`'s recall/precision grader.

## Registry / model fit (respects the locked registry)
- Reuse `DOC_SECTION (42)` + `DOCUMENTS (6)`. Provenance + confidence ride on
  **cells** (extensible) — no locked-id changes.
- **Open:** a `DOC_SPACE`/`DOC_COLLECTION` grouping node-kind (like MODULE for docs)
  would be a *new* locked id — decide vs. reuse MODULE-with-provenance.

## Rollout (atomic, eval-gated slices)
1. **Seam** — refactor doc-ingest to `Vec<DocRecord>`; `FileDocSource` reproduces
   today byte-identically (determinism gate proves zero regression).
2. **Doc-link eval** harness (span-preservation + precision/recall grader).
3. **ConfluenceDocSource** (REST + storage→markdown w/ faithful code spans) → snapshot → ingest → eval.
4. **Linker precision pass** (qualified/rare gating, confidence tiers, source-link + code-block signals).
5. **Notion + wiki** adapters (reuse seam + linker).
6. **P3 primitive** `governing_docs(node)` / `docs_for(feature)`.

## Decisions — LOCKED 2026-07-08 (JRC)
- **Auth/secrets → env vars, referenced by name.** `docs.yaml` names the env var
  (`auth: { token_env: CONFLUENCE_TOKEN }`); the token value is never in config.
  The `auth` block is a tagged reference, so a `secret_ref:` backend (Vault/AWS/1P)
  can be added later without changing call sites.
- **Snapshot store → gitignored cache by DEFAULT, commit is opt-in.** Content-
  addressed cache at `.glia/docs-snapshot/` (gitignored, like `.gmap`/parse-cache);
  a config flag (`snapshot: { commit: true }`) opts a team into committing it for
  fully-offline/reproducible builds. Build reads the snapshot; `glia docs sync`
  refreshes it (network) — keeps the byte-identical determinism gate valid.
- **Doc container → new `DOC_SPACE` node-kind (registry id 44).** Next free id
  after STATE_VAR=43, same growth pattern as DOC_SECTION=42. A Confluence space /
  Notion DB / wiki is a first-class container; DOC_SECTIONs belong to it via
  CONTAINS. Update `reference_kind_category_ids.md` + `code-domain` registry.
  (Chosen over reusing MODULE, which would pollute every "list code modules" query.)
- **Linker default → conservative.** Link only code-formatted spans (`` `X` ``),
  qualified names, and explicit source-file links (all Strong). The aggressive
  bare-token tier (unique non-dictionary identifiers) is opt-in and always emitted
  as **Weak** confidence so consumers can filter. Precision-first: a wrong doc→code
  edge misleads `governing_docs`.

## Still-open (smaller, can decide at build time)
- **Freshness cadence:** on-demand `sync` vs scheduled ingest (default: on-demand).
- **Snapshot format:** exact on-disk layout of `.glia/docs-snapshot/` (content-hash
  dir + per-doc `{markdown, version/etag, provenance}` — settle when building the seam).
