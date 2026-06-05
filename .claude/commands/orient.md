---
description: Run the glia session-start Orient checklist (WORKFLOW.md §1) and print a one-screen state-of-the-world
argument-hint: "[subsystem/area you're about to work on, e.g. 'lens pool-cap' or 'go parser docs']"
---

You are starting a glia working session. Execute the **Orient** checklist from `WORKFLOW.md §1` for the focus area: **$ARGUMENTS** (if empty, orient broadly on the repo's current state).

Do these now, in parallel where independent, then synthesize — do **not** start editing code yet:

1. **repo-graph MCP `status`** — load the structural map. If a focus area is given, also `activate`/`find` from the most relevant seed qnames to locate it in the graph.
2. **mempalace `status`**, then **`search`** (and `kg_query` if a named decision/person/cycle is involved) scoped to the focus area. Query before asserting — surface what the palace already knows. Search **both** "glia" and "repo-graph" terms (rename seam, WORKFLOW.md §2).
3. **`dev-notes/issues_surfacing_now.md`** + any matching `dev-notes/*_plan.md` — read the live open-work surface for this area.
4. **Native auto-memory** is already in context — pull the relevant live cycle/sprint/project state for this area.
5. **Constraints** — skim the `CLAUDE.md` (architecture) and `CODE_RULES.md` (operational) sections that govern this subsystem. Note what is **locked** (registry IDs, `.gmap` format, extract-vs-resolve split, append-only files).
6. **If the task is a ≥100-instance bench cycle** — load the Pre-flight checklist (`WORKFLOW.md §3`) and flag that it must be run + pasted before launch.

Then print a **one-screen orientation**, no longer than ~25 lines:

- **Where we are** — current branch/state + the live cycle/sprint headline.
- **Focus** — what `$ARGUMENTS` maps to in the graph + the relevant open issues/plans.
- **Locked / careful** — invariants you must not break in this area.
- **What could bite** — the gotchas from memory or pre-flight relevant to this work.
- **Proposed first step** — the smallest verifiable next action, scoped in LOC/tokens (never time).

Follow the house-style rules in `WORKFLOW.md §4` for the rest of the session: ship a `fired_on` marker with every feature, honest-failure framing, append-only result files, push back on bad sequence, and walk through changes before any push or publish.
