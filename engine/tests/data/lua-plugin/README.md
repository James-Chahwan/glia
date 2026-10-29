# lua-plugin fixture (lua-support)

One repo, one directory per feature the Lua plugin bridge must bind. Each
artifact is minimal on purpose; the ecosystems the shapes come from are named
so the fixture stays honest without shipping anyone's tree.

| Directory | Feature | As employed in |
|---|---|---|
| `tool-registration/` | a table-shape registration call (`maki.api.register_tool{ name = ..., execute = function .. end }`) minting one `PLUGIN_TOOL` with handler closures; a same-dir `require("helper")` | maki (`plugins/<dir>/init.lua`) |
| `manifest-permissions/` | a routed manifest whose only content is the `[permissions]` section (runtime config, counted, never structure) | maki (`plugins/<dir>/plugin.toml`) |
| `lib/maki/` | the lib-root require: a dotted `require("maki.tool_view")` resolving through the family's `lib` root | maki (`plugins/lib/maki/`) |
| `host-loader/` | the host side: a manifest-name literal (the `manifest_literal` LOADS anchor) and an `include_str!` of a plugin module | maki (`maki-lua/src/loader.rs`) |
