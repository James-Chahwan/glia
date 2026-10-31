# Explainers in the CLI: `glia explain` (built by its own session, beside the 0.5.6 waves)

**Born 2026-10-31.** James (relayed by the site session): *"all the animation explainers are great but we should get them
into the cli how should we do it it's for humans mostly but also for agents or llms to explain and show also it also
means we can use them on the site instead of making custom bespoke animations every damn time"*. On who builds it:
*"the seperate session should build it cause i gotta fucking look at a ton of animations"*. So a dedicated session builds
it with James reviewing the animations, while this repo's main session runs the 0.5.6 waves.

## What exists

The site worktree (`../glia-site`, branch `site`) holds six scenes and 28 steps: pipeline, sizes and drift on the home
page; blast, tiers and world on the concepts page. Each scene is a hand-drawn SVG with ids and an ordered step list.

- **Step captions** carry a verb DSL: `pop` / `draw` / `pulse` / `flow` / `show` / `hide` / `glow` / `dim` / `cam`, each
  written `verb:#id`, plus `tier:#id`.
- **The player** is `site/scene.js` (246 lines: WAAPI + SMIL, step controls, replay-to-step, reduced-motion safe).
- **Out of scope:** the 11 DPG Explainer boards (`site/boards/src/*.dc.html`).

## The shape (the site session's proposal; the defaults below are this session's)

1. **One scene format.** Files: `explainers/<name>.scene.json`. Each holds `title`, `summary`,
   `nodes` [{id, label, x, y, w, h, kind}], `edges` [{id, from, to, label, tier}],
   `steps` [{caption, actions: [{verb, target}]}] and `camera` boxes.
   - The SVG is rendered from the file by a small deterministic Rust renderer (positions are in the data). The same
     source gives svg, html, text and json.
   - The six scenes are ported into the format by hand. The verb DSL stays as it is.
   - The files ride in the binary via `include_str!`. They are not a HASHED_ROOTS entry: they shape no graph.
2. **`glia explain [<name>] [--step N] [--format text|json|svg|html] [--open] [--list]`.** The default format is `text` on
   a TTY and `json` when piped.
   - `text`: each step's caption, then the scene state as boxed ASCII nodes and arrows.
   - `json`: the scene plus the resolved state per step (visible / dimmed / current ids).
   - `svg`: one frame.
   - `html`: a self-contained page with scene.js embedded and the step controls; `--open` launches it.
   - No raster output.
3. **The site reads the scenes from the release ref**, like its other generated docs. `build.py` gains
   `{{scene:name}}`, and `scene.js` moves to `explainers/scene.js`. The site session does this half once a ref holds the
   format.
4. **Answers as scenes:** `glia trace --explain`, `blast-radius --explain` and `why --explain`. Rows become nodes, hops
   become edges with their tier, and there is one step per hop (file:line, tier, rule). Same renderer. It starts after
   the six canned scenes hold the format.
5. **Skills:** glia-orient's "Reading an answer" points at `glia explain tiers` and `glia explain blast`.

Defaults (James can overrule):
- JSON, not TOML: the site build and scene.js read it with no parser.
- `explain` is a top-level command.
- Part 4 follows 1-3 in the same session.

## Living beside the 0.5.6 waves (the rules for the explainer session)

The 0.5.6 waves (`dev-notes/leap-056-packets.json`, `schedule_056.py`) run up to 26 agents at once, committing onto
`main` in `/home/ivy/Code/glia`. Each wave's close-out refuses to finish while any file in that tree is uncommitted.

1. **Work only in a worktree on a branch:** `git -C /home/ivy/Code/glia worktree add ../glia-explainer -b explainer`.
   Never edit, build or commit in `/home/ivy/Code/glia`.
2. **Use your own target dir:** `CARGO_TARGET_DIR=/home/ivy/Code/glia-explainer/target`. Keep /home at 60 GB or more
   free. Delete the target dir when the work has merged.
3. **Files the waves also edit** (expect a small textual merge, nothing more):
   - `README.md` (GA.15, GB.1, GB.2, GB.5, GP.16, GZ.1)
   - `CLAUDE.md` (GP.11, GZ.1)
   - `skills/glia/SKILL.md` and `cli/tests/skill_surface.rs` (GZ.1)
   - `cli/src/cmd/trace.rs`, `cli/surface/trace.txt` and `engine/src/trace.rs` (GB.1, GB.3)
   - `cli/src/cmd/query/mod.rs` (GB.5)
   - `cli/surface/blast-radius.txt` (GP.10)

   No 0.5.6 packet touches `Cargo.toml`, `Cargo.lock`, `cli/Cargo.toml`, `cli/src/main.rs` or `cli/src/cmd/mod.rs`. A
   new crate or dependency is still a Cargo.lock move: keep it to none if you can, and say so when merging.
4. **Merging into main:** message the main glia session (`glia-83`). It pauses between waves. You rebase `explainer` on
   main, run your own crates' tests plus `cargo test -p glia-cli` (the surface tests: `cli/surface/explain.txt`, the
   README `## CLI` line), and fast-forward main. Merge early and often; part 1-3 first.
5. **Never** push, tag, or run `scripts/rank-gate.sh` / `scripts/cohort-gate.sh`. The 0.5.6 close-out runs the gates on
   whatever has merged.
6. **It ships with 0.5.6** if it has merged before GZ.1 (the release docs packet), which documents it. Otherwise it waits
   for the next release.
7. The repo is public: no private project's names or material in scenes.

## Open (James has not ruled; the site session asked him 2026-10-31)

1. **Terminal fidelity.** Captions and ASCII only (the default above: no raster output), or also real frames in
   image-capable terminals? Real frames would mean kitty / sixel through resvg, which is a new dependency and so a
   Cargo.lock move.
2. **Scene source.** A data model the renderer draws (the default above), or keep the hand-drawn SVGs, or data with an
   optional per-scene SVG override for art-directed scenes?
3. **On a TTY.** Interactive keys by default (next / back / quit), or print the whole walkthrough?

Ask James before building past the defaults on any of these. Whichever session hears his answer first records it here.
