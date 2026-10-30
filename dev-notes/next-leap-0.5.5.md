# glia 0.5.5 — Lua, and the matrix's use-pattern axis

**Born 2026-10-31**, the day 0.5.4 shipped. The first community patch: Brian Ealdwine (GitHub `eode`) opened glia PR #3,
*"feat(lua): Lua support -- Lua parser, PLUGIN graph, plugin-families"* (+10,412 / -1,347, 49 files, on 0.5.3-era
`main`). James: *"we might wanna maybe think about this we could a small lua pr 0.5.5 then bump our 0.5.5 to 0.5.6 like
they probably did a bunch of testing and had a usecase for lua"*, then *"yeah read the full diff just for the lua support
we could do that now and cite it"*. The old 0.5.5 seed is `dev-notes/next-leap-0.5.6.md`.

The PR was read as text only, never checked out or run (James: *"don't even run it"*). Most of its size was rustfmt
churn (a default-config format of a pre-0.5.4 tree); its new ids (node 50 PLUGIN_TOOL, 51 PLUGIN, edge 38 LOADS)
collided with 0.5.4's GOAL_UNIT / TRACES. Its Lua core (the L1 packet of its `dev-notes/lua-support.md`) is the design
0.5.5 rebuilt on 0.5.4, credited `Co-authored-by` on the Lua commits.

## 1. Rulings

- **Lua support, rebuilt and credited** (above). The PR's gaps are fixed, not ported: functions assigned as values
  (`M.f = function`, `local f = function`, table-literal fields, the returned export table) were not minted; `require`
  never tried `?/init.lua` (the PR description claims it, the code lacks it) nor a `lua/` root without a plugin family;
  same-named nested locals collided on one qname; the plain-Lua test the description names (`lua_pure.rs`) was not in
  the diff; the require search roots came only from plugin families.
- **Plugin families move to 0.5.6** as one general host-to-plugin mechanism (VS Code commands, Obsidian plugins, Flask
  blueprints, webpack plugins, Neovim, Maki), with the PR's family table as its first case. James on whether it is
  useful: *"Well that seems important maybe mark it for 0.5.6"*.
- **The matrix row is not deferred.** James: *"Well now we shouldn't defer the matrix if we add it"*.
- **A second matrix axis, use patterns, now.** James: *"Isn't lua used a bit differently this is the problem with the
  matrix also it doesn't deal with language use patterns breakdown like lua is in game scripting for glue work or
  processing scripts or Linux stuff"*, then *"lua changes the matrix we should handle here and now with 0.5.6 in mind"*,
  then the option *"Lua row + use-pattern axis now"*: the axis and Lua's pattern rows are graded in 0.5.5 (blindness
  stated), shaped so 0.5.6's host-to-plugin graph fills them.
- **More of Lua in this release** (James: *"how hard is to complete a bit more of lua"*): `os.execute` / `io.popen`,
  busted test functions, metatable classes, Lapis routes and `socket.http` / `resty.http` clients.

## 2. What 0.5.5 ships

**The parser** (`parsers/code/lua`, tree-sitter-lua 0.5): `FUNCTION` for `function f`, `local function f`,
`function T.f`, `f = function`, `T.f = function`, table-literal fields (`T.sub.g`) and the returned export table; `METHOD`
for `function T:m` (`self` reads as `T`); a nested local keyed under its enclosing function (`M.setup.helper`), a
collision suffixed `@<line>`; `require` as an `IMPORTS` statement with its alias; bare calls resolved lexically by the
parser (visible local, then the file's globals); `T.f(..)` / `x:m(..)` as `Attribute` calls; busted `it` blocks in test
files as test FUNCTIONs `<describe ..> <name>` (as Elixir's ExUnit tests); metatable classes (a table with methods,
`T.__index = T`, or a metatable role is a `CLASS`), inheritance (`setmetatable(Child, { __index = Base })`,
`Base:extend()`, `:subclass`, middleclass `class("Child", Base)`) as `INHERITS_FROM` refs, typed instances
(`T.new(..)`, `setmetatable({}, T)`); Lapis routes (`app:get("/p", h)`, `match` = ANY) as canonical `ROUTE`s with a
`HANDLED_BY` ref; `socket.http` / `resty.http` client calls as `ENDPOINT`s (a `..` concatenation read as `${…}`).
`[lua] functions= methods= classes= heritage= routes= endpoints= tests= requires= dynamic_requires= calls= file=`.

**The graph** (`graph/src/build/lua.rs`, `glia_graph::build_lua`): require resolution (beside the file, then the ancestor
walk under `""` / `lua` / `lib` / `src`, the file before `<spec>/init.lua`; rules `require_sibling` / `require_path` /
`require_root`), heritage (in-file `metatable`, cross-file `require_alias`), file-local call binding (`local_call`,
`module_table`, `require_alias`, `inherited_method` up 8 levels). `[imports] lua requires ..`, `[lua-classes] ..`,
`[lua-calls] ..`. The CLI invocation needles gain `os.execute(` / `io.popen(`; `engine::health` stops counting `.lua` as
unparsed.

**The matrix** (`bench/substrate-gap`): an 18th row, `lua` (`matrix/lua/`: calls, imports, config, tests, cli_inv, impl,
http_server, http_client, all full), and the use-pattern axis: `matrix_vocab.PATTERNS` (host_command, host_event,
host_callback, config_table, embedded_script, script_entry), graded by `matrix.derive` like a mechanism (a pattern's
`proof_cells` route it where no edge can: an `ENTRYPOINT` on a host-called function), filed under
`patterns/<language>/<pattern>/`, rendered as its own table in COVERAGE.md and never counted in the 30-column totals.
`[matrix] patterns: 18 languages x 6 use patterns, ..`. Lua's six: host_command, host_event and config_table `none`,
host_callback, embedded_script and script_entry `partial` (the functions / modules are read, the host's link or entry is
not): the 0.5.6 host-to-plugin graph's targets. Fixtures `fixtures/lua-pure` and `fixtures/lua-nvim` (graded by run.py)
and `engine/tests/lua.rs`.

No registry id, no FORMAT_VERSION / MANIFEST_VERSION change; PARSER_STAMP moves (a new parser crate); Cargo.lock gains
tree-sitter-lua.

## 3. Out (0.5.6)

The host-to-plugin graph (PR #3's plugin families, manifests and LOADS, generalised) and the use patterns it fills; Lua
globals across files; `pcall` / `load` / `dofile` indirection; metatable `__call` constructors; OpenResty's nginx-side
routing (the `embedded_script` pattern); a Lua row for the use patterns of other languages.
