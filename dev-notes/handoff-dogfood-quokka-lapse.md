# Dogfood handoff: quokka-stack and lapse drop `generate-repo-map.py` for `glia flows`

For the quokka-stack and lapse sessions. glia does not edit those repos; this note lists the edits for them to make.
**Act on it once glia 0.5.0 is installed.** On 0.4.18 there is no `glia flows --features`, no `data_entity` wrapper, and
the layout still lives in `.ai/repo-graph/`. The `glia` on PATH (`~/.cargo/bin/glia`, a 0.4.x build from 2026-05-31) has
to be the 0.5.0 build: run `cargo install --path cli` in the glia checkout.

Sources: LG.3c `db34713` (flows `--features/--out`, the measured table), LG.3d `5dae2b2` (overlay `data_entity`
wrapper), LC.9 `91e7d7d` (layout moved to `.glia/graph`), LG.3a `a54cf43` (feature records and writer), LG.3b `898e7b1`
(DATA_ENTITY precision), LE.9b `967456e` (spec-status). Every file:line below was re-checked read-only on 2026-09-19,
at quokka-stack `a77d4cb` and lapse `c0ece02` (glia `3f427fb`). Rows marked **LG.3e** were measured for this note. They
used a `git archive` copy of quokka `a77d4cb` and `target/debug/glia` (`0.4.18+p0fe3cded57f3f594`, the W28 build).

## TL;DR

- `glia flows . --features` replaces `python3 scripts/generate-repo-map.py`. It writes `.glia/graph/flows/<feature>.yaml`
  plus `index.json`. It commits nothing: the layout dir holds its own `.gitignore` of `*`.
- **quokka-stack:** delete `scripts/generate-repo-map.py`, the two `.githooks` lines that run it, and its committed
  outputs. Add `.glia/overlay.toml` with the `NewCollection` stanza. Collections only reach the flows after 12 more
  `[[edge]]` stanzas (the known gap in section 5).
- **lapse:** has no script (`ls scripts` finds nothing), but five files still name it and three more point at its
  outputs. Fix them and delete the stale outputs.
- `.ai/repo-graph/` is glia's **legacy** dir after 0.5.0. glia reports it (`[gmap] legacy layout ignored: ...; safe to
  delete`) and never deletes it. Both repos should delete it.

## 1. What replaces what

| script output (`.ai/repo-graph/`) | glia 0.5.0 |
|---|---|
| `nodes.json` / `edges.json` | `glia analyze . --format json` (`{nodes, edges}`), or the MCP `find` / `impact` / `trace` / `read` |
| `flows/<feature>.yaml` | `glia flows . --features` writes `.glia/graph/flows/<feature>.yaml` + `index.json`. Each file has a header, then `feature`, `grouping`, `services`, and `entries[]` (`entry`, `weakest`, `callers[]`, `steps[]`), then `data_sources[]`. Every step, caller and sink is one line holding a JSON record (qname, kind, service, via, confidence, cross_service, depth, file, 1-based line) |
| one view per entry | `glia flows .` (table, `--json`), or `glia flows . --features --group-by entry` |
| `frontend_page -uses-> frontend_service` | page `ROUTE -HANDLED_BY-> COMPONENT`, `-INJECTS->` service CLASS |
| `frontend_service -calls-> backend_route` | `ENDPOINT -HTTP_CALLS-> ROUTE`: the entry's `callers[]` |
| `backend_route -handled_by-> handler` | `ROUTE -HANDLED_BY->` handler FUNCTION / METHOD |
| `repository -reads_from-> db_collection` | `data_entity:nosql:<name>` + `ACCESSES_DATA`. The tier is `FACT` when the edge leaves a flow node and `HEURISTIC` when it leaves that node's module. quokka needs the overlay stanza (section 3) |
| `nats_topic` (hard-coded) + `publishes_to` | `QUEUE_*` nodes when the subject is a string literal. quokka's subjects are built at runtime, so they need overlay `[[edge]]` / `[[note]]` stanzas (section 3) |
| `state.md` git-state section | **not replaced**: it is not graph data. Use `git status` / `git log --oneline -5` |
| `state.md` feature-coverage table (`features/<f>/{backend,ui,current-state}.md` present?) | `glia spec-status .` (LE.9b) answers the stronger question: which declared ops a ROUTE implements. File presence is `ls features/*/` |
| `state.md` "Available Flows" | `index.json` `features[].feature` |

## 2. Where flows land, and where they must not

- Default: `<repo>/.glia/graph/flows/`, next to the layout and ignored by git. `glia flows . --features --json` prints
  the records and writes nothing. `--out <dir outside the repo>` writes anywhere outside the repo tree.
- A dir **inside** a repo but outside its `.glia/` is refused with exit 2: `feature flows: refusing to write to <dir>:
  it is inside the repo <repo> and not under its .glia/, so the next build would walk the files as sources`. The next
  build would route every `.yaml` it reaches through the cron / config / iac / contract sniffers, so flows would feed
  back into the graph. `<repo>/.ai/repo-graph/flows` is refused too.
- Do not use `--out .glia/<anything but graph>`. The writer accepts it, but every file under `.glia/` outside
  `.glia/graph/` counts as a build input (LF.1d fingerprint). Each flows write would mark the layout stale, and the MCP
  would rebuild.
- The old workflow committed flows with the feature work. Stop doing that. Flows are regenerated per commit (hook in
  sections 3 and 4) or on demand.

## 3. quokka-stack

**Measured (LG.3c, `db34713`, copied).** The script copy re-run gave 140 nodes, 183 edges and 11 flows, matching the
2026-09-18 baseline.

| measure | script | glia flows --out |
|---|---|---|
| flows / features | 11 | 40 features, 106 entries (63 turps + 1 android server ROUTE, 22 web + 17 android pages, 1 GRPC_SERVICE, 1 GRPC_SERVER, 1 WS_HANDLER) |
| backend routes | 59 | 63 turps routes, each an entry |
| svc -> route calls | 40 (web) | 47 cross-service endpoint -> route HTTP_CALLS callers (30 web, 17 android) on 42 routes. Web: 29 routes, 22 shared with the script, 7 the script lacks (GET/PUT /user/profile, /user/search, /user/validate, /chat/history, /healthz, /metrics), 17 missed, 1 script artefact (GET /2fa) |
| collections | 12 | 0; data sources 3, all HEURISTIC (data_source:resend) |
| NATS topics | 2 (hardcoded) | 0 queue entries |
| cross-service steps | - | 0 (forward walks stop at the injected service CLASS) |

Script flow -> glia feature key: all 11 map, none missing. activities -> `activities`, auth -> `auth`, chat -> `chat`,
complete-profile -> `complete-profile`, connect -> `connect`, discover -> `discover`, groups -> `groups`, home -> `home`,
profile -> `profile`, settings -> `settings`, user-profile-view -> `user`. The Angular folder
`features/user-profile-view` serves `/user/:publicId`, and glia keys a feature by its route's first static segment.
complete-profile, connect, discover, home and profile are page-only records (gap 3 below).

**Overlay (LG.3d, `5dae2b2`, copied).** One `NewCollection` stanza on a copy of quokka gave
`[overlay] wrappers ... stanzas=1 sites=14 minted=12 duplicate=0 skipped_nonliteral=2 ... data_entity=12`. That is 12
`data_entity:nosql:*` nodes, the script's 12 `db_collection` names: activities, auth_tokens, chat_previews, dm_rooms,
group_activity_invites, group_feedback_events, group_matching_rooms, group_recommendation_responses,
group_recommendations, groups, swipe_events, users. Each has a function-level ACCESSES_DATA from its
`New<X>Repository`. The control without the stanza gives 0. The 2 non-literal sites are docstring lines in
`scripts/generate-repo-map.py` itself.

**Flows with the overlay (LG.3e).** Measured with `glia flows <copy> --features --json`:

| copy | `[feature-flows]` line | collections in any flow |
|---|---|---|
| no overlay | `features=40 entries=106 steps=949 callers=145 sinks=3 (fact=0 heuristic=3)` | 0 |
| + `NewCollection` stanza | `features=40 entries=106 steps=949 callers=145 sinks=3 (fact=0 heuristic=3)` | 0 (the 12 nodes exist but no flow reaches them, gap 1) |
| + stanza + 12 provider `[[edge]]`s below | `features=40 entries=106 steps=1242 callers=145 sinks=53 (fact=50 heuristic=3)` | 11 of 12 (not dm_rooms), across 17 features, all FACT |

Per script feature, script collections vs glia collections (stanza + edges): activities 4 / 2, auth 3 / 3 (auth_tokens,
groups, users on both sides), chat 4 / 1, groups 10 / 5, settings 9 / 8, user-profile-view -> `user` 3 / 1,
complete-profile 3 / 0, connect 3 / 0, discover 9 / 0, home 5 / 0, profile 3 / 0. The script's sets are
handler-wide: every repository a controller touches. glia's sets are what the flow's functions reach.

**Edits** (check each line before editing):

1. **New `.glia/overlay.toml`, checked in** (`.glia/` is untracked today):
   ```toml
   version = 1

   [[wrapper]]
   call = "NewCollection"
   kind = "data_entity"
   flavor = "nosql"
   name_arg = 2
   origin = "human"
   ```
   Until gap 1 closes in glia, also add one stanza per getter in `turps/Services/repository_provider.go`. There are 12
   (lines 33-139), each calling its constructor inside `xOnce.Do(func() { ... })`. For example:
   ```toml
   [[edge]]
   from = "turps::Services::repository_provider::ChatPreviewRepository"
   to = "turps::Services::Repositories::chat_preview_repository::NewChatPreviewRepository"
   category = "CALLS"
   note = "call inside sync.Once.Do(func() {...})"
   origin = "human"
   ```
   On the copy all 12 gave `[overlay] edges ... declared=12 applied=12 redundant=0 orphaned=0 rejected=0`. Drop them
   once glia attributes closure calls; `glia gaps .` then lists each one as `redundant_rule`.
2. **NATS subjects are not recoverable statically.** `turps/Services/chat/notifications.go:75`
   `conn.Publish(notifSubject(targetUserID), payload)` builds `"notif." + userID` (`:46`).
   `turps/Services/chat/nats.go:167` `s.JS.Publish(subject, ...)` takes its subject from `roomSubject()` (`:112`),
   which reads the config field `NATS.JetStream.Publish.SubjectTemplate`. An overlay `queue_producer` wrapper needs a
   literal topic argument, so it cannot mint these either. What the overlay can declare (checked on the copy:
   `declared=13 applied=13 orphaned=0`, `note=1`):
   ```toml
   [[edge]]
   from = "turps::Services::chat::nats::NATSChatService::ProduceMessage"
   to = "turps::Services::chat::nats::NATSChatService::CreateRoomSubscription"
   category = "QUEUE_FLOWS"
   note = "NATS chat.room.<roomId>: subject from config NATS.JetStream.Publish.SubjectTemplate"
   origin = "human"

   [[note]]
   anchor = "turps::Services::chat::notifications::PublishNotification"
   text = "publishes NATS notif.<userId> (notifSubject); subscribers are clients outside this repo"
   ```
   The script's hard-coded `publishes_to` list names 3 publishers, but `chat.Notify(` has 5 call sites:
   `friends_controller.go:147,234`, `group_activity_invite_controller.go:142`, `recommendation_controller.go:234`
   (missing from the script) and `Tasks/group_matching_tasks.go:631`. glia already has a CALLS edge to
   `chat::notifications::Notify` from each of the 5 enclosing functions (LG.3e).
3. **Delete `scripts/generate-repo-map.py`** (779 lines).
4. **`.githooks/pre-commit`: delete the file.** After the shebang and `set -e`, its only lines (4-6) run
   `generate-repo-map.py`, then `git add .ai/repo-graph/`. Today that `git add` also stages the 17 untracked glia 0.4.x files there (manifest,
   cross_stack, 15 shards; 4,340,219 bytes) into the next commit.
5. **`.githooks/post-merge:4`** (runs `generate-repo-map.py`): replace it with `glia build . && glia flows . --features`.
   `core.hooksPath` is `.githooks`, so `glia install-hooks` writes there. It skips any hook it did not write, and
   post-merge / post-checkout are not glia's.
6. Run `glia install-hooks . --command "glia build . && glia flows . --features"`. It writes `.githooks/post-commit`
   (glia-managed) and prints `skipping ... not glia-managed` for post-merge and post-checkout. Commit the new hook:
   the dir is tracked. A post-commit hook cannot block a commit. LG.3c timed `glia flows --out` on quokka at 2.1 s
   (debug build).
7. **`.claude/settings.local.json:34`** allows `Bash(python3 scripts/generate-repo-map.py && cat
   .ai/REPO_MAP.generated.md)`. The script no longer writes that `.md`. Replace the entry with `Bash(glia flows:*)` and
   `Bash(glia build:*)`. The file is local, covered by the global gitignore `~/.config/git/ignore:1`.
8. **`CLAUDE.md`** "Session Startup" (lines 4-14) never mentions flows. Add a step: "`glia flows . --features`, then read
   `.glia/graph/flows/index.json` and `.glia/graph/flows/<feature>.yaml` for the feature" (LG.3c correction).
9. **`.claude/commands/start.md:50, :52-53`**: replace `.ai/repo-graph/state.md` with `git status` and
   `git log --oneline -5`; replace "available flows listed in state.md" with `index.json`; replace
   `.ai/repo-graph/flows/<feature>.yaml` with `.glia/graph/flows/<feature>.yaml`.
10. **`.ai/REPO_MAP.md:16`** says "scripts/ ← tooling (repo map generator)". Drop the generator. **`:86-91`** "For the
    current state" should point at `glia flows . --features` and `.glia/graph/flows/`.
11. **`.ai/repo-graph/`**: 81 tracked files are already deleted in the working tree, not committed (README.md,
    nodes.json, edges.json, state.md, 77 flows). 66 of those flows are names the script's `build_flows` never
    produces (63 `<verb>__<path>.yaml`, plus `get`, `nats`, `quokkachatservice.quokkachatstream`); it keys only by
    feature folder and never prunes. After 0.5.0: `git rm -r -q .ai/repo-graph && rm -rf .ai/repo-graph`. The first
    0.5.0 `glia build .` also removes the 9 flat 0.4.x shards under `.glia/` as orphans
    (`[gmap] removed <k> orphan shard(s) from .glia`).

## 4. lapse

**Measured (LG.3c, `db34713`, copied).** lapse has no `scripts/generate-repo-map.py`, and its committed
`.ai/repo-graph/{nodes,edges}.json` are empty (0 flows). glia: 34 features, 138 entries (107 server routes, 31 pages);
91 cross-service endpoint -> route callers on 90 routes; 59 data sources (56 FACT, 3 HEURISTIC) over 22 SQL tables plus
resend. `glia flows --out` took 1.5 s (debug build). lapse needs no overlay: its tables come from SQL, which the
extractor reads directly (LG.3b).

**Edits:**

1. **`CLAUDE.md:8`** (`.ai/repo-graph/state.md`): use `git status` / `git log --oneline -5`. **`:13`** (load
   `.ai/repo-graph/flows/<feature>.yaml`): run `glia flows . --features`, then load `.glia/graph/flows/<feature>.yaml`
   (keys are in `index.json`).
2. **`.ai/CLAUDE.md:14`**: same flows path change.
3. **`.ai/WORKFLOW.md:5, :11`**: same two changes as 1. **`:76-84`** "Repo-graph maintenance": `:81`
   `python3 scripts/generate-repo-map.py` becomes `glia flows . --features`. Change `:84` ("Commit the updated graph
   with the feature work") to "not committed; regenerated by the post-commit hook" (LG.3c correction).
4. **`.ai/docs/backend-patterns.md:5`** and **`.ai/docs/frontend-patterns.md:5`** say "Regenerate the repo graph ...
   `python3 scripts/generate-repo-map.py`". Point them at `glia flows . --features`. Both files also still describe
   quokka's `turps/` / `quokka_web/` paths.
5. **`.ai/REPO_MAP.md:63-66`** "For the current state": point at `.glia/graph/flows/`.
6. **`.ai/repo-graph/README.md:3, :47`** and **`state.md:3, :29`** name the missing script. They go with the dir: 4
   tracked files (README.md, state.md, `[]` nodes.json and edges.json) plus an empty `flows/`, next to 11 untracked
   glia 0.4.x files (3,555,345 bytes). After 0.5.0: `git rm -r -q .ai/repo-graph && rm -rf .ai/repo-graph`.
7. Hooks: `glia install-hooks . --command "glia build . && glia flows . --features"`. lapse sets no `core.hooksPath`
   and has no hooks, so all three land in `.git/hooks`.
8. lapse has no `features/` dir, so `state.md`'s coverage table never had rows. Nothing to replace.

## 5. Known gaps, with evidence

1. **Go calls inside a func literal are dropped, so quokka's collections never join a flow.** `collect_calls_in`
   (`parsers/code/go/src/lib.rs`) does not descend into `func_literal`. Every `repository_provider.go` getter builds its
   repository inside `xOnce.Do(func() { ... })`, so each getter has 0 outgoing edges and every `New<X>Repository` has
   no caller (LG.3e, analyze JSON on the stanza copy). **Closing item:** none in `leap-packets.json`; LA.18d covers only
   func-literal *route handlers*. It needs a new Go parser item. Until then, use the `[[edge]]` workaround above.
2. **Templated URLs through the URL builder mint `<unresolved>`: the 17 missed web calls.** In `classify_path_arg`
   (`parsers/code/typescript/src/lib.rs`), the `call_expression` arm only picks a *string literal* out of the builder
   call. A template argument gets no path. LG.3e probe: `this.http.get(buildApiUrl('protected/literal/mine'))` gives
   `endpoint:GET:/protected/literal/mine`, and a direct `` get(`protected/direct/${id}`) `` gives
   `/protected/direct/${…}`. `` buildApiUrl(`protected/plain/${id}`) `` and
   `` buildApiUrl(`protected/encoded/${encodeURIComponent(id)}`) `` both give `endpoint:GET:<unresolved>`. Sites
   include `quokka_web/src/app/core/services/activity.service.ts:150`, `friend.service.ts:63` and
   `group.service.ts:83`. **Closing item:** none yet. It needs a new TS extractor item that sends the builder's argument
   through `classify_template`. An overlay `http` wrapper cannot fix it, because the method comes from the outer
   `http.get`.
3. **Page-only records and 0 cross-service steps.** A page reaches its injected service CLASS through INJECTS, but
   class -> method is DEFINES, and flow walks never follow structural edges (LD.4b). The flow never reaches the
   methods that make the HTTP calls. LD.4b's hand-off to LG.3a / LG.3c flagged this, and neither closed it.
   **Closing item:** none yet. It needs a new item for a class -> method step in the carry walk.
4. **NATS subjects** are runtime-built (section 3, edit 2). This is not a glia defect. Declare them in the overlay.
5. **dm_rooms** is in no flow, even with the provider edges: no flow step reaches `DMRoomRepository()`.

## 6. Re-test commands (0.5.0 installed)

```sh
cd ~/Code/quokka-stack
glia --version                                                  # glia 0.5.0 (build ...)
glia build . 2>&1 | grep '^\[gmap\]'                            # wrote .glia/graph ...; legacy layout ignored (until deleted); orphan shards removed
glia flows . --features 2>&1 >/dev/null | grep -E '^\[(overlay|feature-flows)\]'
#   stanza only:     [overlay] wrappers ... minted=12 ... data_entity=12 ...   /  [feature-flows] features=40 ... sinks=3
#   + 12 edges:      [overlay] edges ... applied=12 orphaned=0             /  [feature-flows] ... sinks=53 (fact=50 heuristic=3)
python3 -c 'import json; print([f["feature"] for f in json.load(open(".glia/graph/flows/index.json"))["features"]])'
glia spec-status .     # LE.9b measured: features=12 declared=82 implemented=80 declared_missing=2 undeclared=24
glia gaps .            # what the overlay can still repair
grep -rn generate-repo-map . --exclude-dir=node_modules --exclude-dir=.git    # expect no output
git status --short .ai .glia .githooks

cd ~/Code/lapse
glia build . 2>&1 | grep '^\[gmap\]'
glia flows . --features 2>&1 >/dev/null | grep '^\[feature-flows\]'           # LG.3c: 34 features, 138 entries
grep -rn generate-repo-map . --exclude-dir=node_modules --exclude-dir=.git    # expect no output
```

These numbers are from quokka `a77d4cb` and lapse `c0ece02`. They will change as those repos change.

## 7. Done vs pending

- **Done in glia (landed):** LC.9 layout at `.glia/graph`, LG.3a feature records and writer, LG.3b DATA_ENTITY
  precision, LG.3c `glia flows --features / --out`, LG.3d `data_entity` wrapper, LE.9b `glia spec-status`, LG.2
  `install-hooks`.
- **Pending in glia:** gaps 1-3 in section 5. None has a packet yet.
- **Pending in quokka-stack / lapse (their sessions, after the 0.5.0 bump):** the edits in sections 3 and 4.
