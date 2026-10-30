# Contributing to glia

Thank you for looking. glia is not yet set up to merge pull requests directly, and it is better to say so up front.

## Why, for now

Some things in glia are the way they are because it has not reached a PR-friendly stage yet, not because
contributions are unwelcome:

- **Releases land as big leaps.** A release can bring hundreds of commits at once (a repo-wide format, new modules,
  moved code), so a branch opened against one release is usually far behind by the next.
- **Graph ids are allocated by hand.** Node kinds, edge categories and cell types are numbered in
  `code-domain/src/lib.rs`, and a number is permanent once a `.gmap` is written with it. Two branches that each take
  "the next free id" collide, and one of them has to be renumbered. A domain-aware allocator is planned (below).
- **The gates are heavy.** A release runs the full test suite, a rank gate on six pinned public snapshots and a cohort
  gate on 21 public repos, about an hour in all.
- **The graph work is moving.** The next releases add more graph layers, stacking and new domains, so the shape a
  contribution would plug into is still changing.

## What works today

- **Issues**: a bug, a missing language or framework, a repo glia reads wrong, a use case. A short example repo helps
  most.
- **Example fixtures**: a directory under `bench/substrate-gap/fixtures/` (source files plus a `key.json` naming the
  nodes and edges glia should find; `bench/substrate-gap/README.md`) changes no engine code and can be merged almost any
  time. A fixture glia fails is a useful contribution in itself.
- **Code PRs are read as designs.** A pull request is read (never run unreviewed), and the idea is rebuilt into the next
  release on top of the current tree, with you credited as `Co-authored-by` on the commits that use it. glia 0.5.5's Lua
  support came this way, from glia PR #3.
- **Never allocate an id** in a PR. Say in the issue what you need, and one is assigned at release.
- **Start a big feature as an issue** (a new language, a new kind of edge), so it can be lined up with a release.

## Licence

glia is under the [Glia Software License v0.1](./LICENSE) (PolyForm Noncommercial 1.0.0 plus a worker-protection
overlay). A contribution is accepted under the same licence.
