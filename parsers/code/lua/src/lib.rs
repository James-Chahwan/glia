//! The Lua parser (0.5.5). Built from Brian Ealdwine's design in glia PR #3
//! (`lua-support`, its L1 packet), rebuilt on 0.5.4 without the plugin
//! families, plus the gaps its review found.
//!
//! Extracts what the AST makes available, nothing more:
//!
//! | Lua source | Extracted |
//! |---|---|
//! | `function f()` / `local function f()` / `local f = function() end` / `f = function() end` | `FUNCTION`, key `f` |
//! | `function T.f()` / `T.f = function() end` (any dotted depth) | `FUNCTION`, key `T.f`: a table field, modelled flat |
//! | `function T:m()` | `METHOD`, key `T.m`; `self` inside it is `T` |
//! | `local T = { f = function() end, sub = { g = function() end } }` | `FUNCTION`s `T.f`, `T.sub.g` |
//! | `return { f = function() end }` | `FUNCTION` `f` (the module's export table) |
//! | a local function inside another function | `FUNCTION`, key `<enclosing key>.f` |
//! | `require("a.b")` (one string-literal argument) | an `IMPORTS` [`ImportStmt`], the spec verbatim; `local x = require(..)` binds alias `x` |
//! | `f(..)` | a `CALLS` [`CallSite`], `Bare(<key>)`: the visible local function's key, else the name |
//! | `T.f(..)`, `T:m(..)`, `alias.f(..)` | a `CALLS` [`CallSite`], `Attribute { base, name }` |
//! | a class table: `function T:m()`, `T.__index = T`, or a metatable in `setmetatable(x, T)` / `{ __index = T }` | `CLASS`, key `T` |
//! | `setmetatable(Child, { __index = Base })`, `local Child = setmetatable({}, { __index = Base })` (Child a class), `Base:extend()` / `Base:subclass(..)`, middleclass `class("Child", Base)` | an `INHERITS_FROM` [`UnresolvedRef`] Child -> `Base` |
//! | `local x = T.new(..)` / `T:new(..)` / `setmetatable({}, T)` | `x` typed `T`: `x:m()` is `Attribute { T, m }` |
//! | Lapis `app:get("/users/:id", function(self) .. end)` (verbs get / post / put / delete / patch, `match` = ANY; a route name first is skipped) in a file requiring `lapis` | `ROUTE` `<METHOD> <path>` + a `HANDLED_BY` ref to the handler (an anonymous one minted as `FUNCTION` `<METHOD> <path>`) |
//! | `socket.http`'s `http.request("http://h/p")` (POST with a body) / `http.request{ url = .., method = .. }`, `resty.http`'s `httpc:request_uri("http://h/p", { method = .. })` | an `ENDPOINT` from the caller (code-domain's client helpers, `host` on the hit) |
//! | busted `it("adds", function() .. end)` in a test file ([`is_test_file`]) | `FUNCTION`, key `<describe ..> adds` (space-joined, as Elixir's ExUnit tests), its calls its own |
//! | a host registration (0.5.6, GH.2, code-domain [`hosts`]): Neovim `vim.api.nvim_create_user_command("Name", h, ..)` / `nvim_buf_create_user_command(buf, "Name", h, ..)`, Maki `maki.api.register_tool{ name = "t", execute = .., restore = .. }` / `register_command`, the callee rooted at the unshadowed global `vim` / `maki` or a local alias of it (`local api = vim.api`), the name a string literal | `CLI_COMMAND` `host_cmd:<host>:<name>` (no parent: the host's) + a `HANDLED_BY` ref to each handler (an anonymous one minted as `FUNCTION` `command <name>`, a second table handler `command <name> <key>`, each owning its calls) |
//! | a host callback (GH.4, [`hosts::callback_for`]): LOVE's `function love.update(dt)` / `love.draw = function() end`, the root `love` the unshadowed global or a local alias of it | the `FUNCTION` gets an `ENTRYPOINT` cell `{"decl":"<file>:<row>","source":"host:love"}` |
//! | a plugin unit (GH.4, [`hosts::load_for`]): Neovim `plugin/**/*.lua`, `ftplugin/**/*.lua`, `after/**/*.lua`, a root `init.lua` reading `vim`; LOVE's root `main.lua` / `conf.lua` showing `love`; WezTerm's root `wezterm.lua` / `.wezterm.lua` requiring `wezterm` | the `MODULE` gets an `ENTRYPOINT` cell, source `host:<host>` |
//! | a script (GH.4): a first line `#!.. lua` / `luajit` / `lua5.N` ([`hosts::shebang_entry`]), else a read of the global `arg` outside every function with no local, parameter or loop variable `arg` in scope | the `MODULE` gets an `ENTRYPOINT` cell, source `shebang` / `script_arg` (one entry cell per node, a host load first) |
//!
//! The qname of a symbol is `<module>::<key>`; a key two declarations of one
//! file would share gets `@<line>` (1-based) on the later one, so every node
//! id is unique. Calls inside an anonymous function attribute to the nearest
//! enclosing named function, else the file MODULE. Not read: metatables,
//! `pcall` / `load` / `dofile` indirection, string-keyed dispatch, globals
//! defined in another file. `build_lua` (graph) binds the calls and requires.
//!
//! fired_on, per parsed file:
//! `[lua] functions=<f> methods=<m> classes=<k> heritage=<h> routes=<r> endpoints=<e> tests=<t> requires=<q> dynamic_requires=<d> calls=<c> file=<p>`,
//! then, per file holding a host registration, callback, setting or load (GH.2, GH.4):
//! `[host] lang=lua commands=<c> events=<e> callbacks=<k> loads=<o> config=<g> minted=<m> aux=<a> unread=<u> hosts=<h:n,..> file=<p>`,
//! and per script entry (GH.4): `[script-entry] lang=lua source=<shebang|script_arg> module=<q> file=<p>`.

use std::collections::{BTreeSet, HashMap, HashSet};

use glia_code_domain::endpoint::{
    ClientEndpoint, HitExtras, client_url_split, push_client_endpoint_with, route_qname,
};
use glia_code_domain::hosts::{
    self, ConfigFlavour, ConfigMatch, ConfigSide, HostSink, HostSite, HostTally, Pattern, Shape,
    config_builder_for, config_for, host_for_require, push_config_key, push_registration,
    registration_for, returned_config_host,
};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, NodeKindId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_lua::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();
    let root = tree.root_node();

    let module_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, module_qname);
    let mut w = Walk {
        src,
        file_rel_path,
        module_qname,
        module_id,
        repo,
        nodes: Vec::new(),
        edges: Vec::new(),
        calls: Vec::new(),
        pending: Vec::new(),
        imports: Vec::new(),
        nav: CodeNav::default(),
        keys: HashSet::new(),
        globals: HashMap::new(),
        scopes: vec![HashMap::new()],
        types: vec![HashMap::new()],
        classes: BTreeSet::new(),
        tables: HashSet::new(),
        decls: HashMap::new(),
        aliases: HashSet::new(),
        heritage: Vec::new(),
        refs: Vec::new(),
        lapis: false,
        socket_http: HashSet::new(),
        resty_http: false,
        endpoints: HashSet::new(),
        tests: is_test_file(file_rel_path).then(Vec::new),
        counts: Counts::default(),
        host_bindings: vec![HashMap::new()],
        host: HostTally::default(),
        table_keys: HashMap::new(),
        host_roots: BTreeSet::new(),
        script_arg: None,
    };
    w.nodes.push(Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: cells(&root, src, file_rel_path),
    });
    let simple = module_qname.rsplit("::").next().unwrap_or(module_qname);
    w.nav
        .record(module_id, simple, module_qname, node_kind::MODULE, None);

    let top = Ctx {
        owner: module_id,
        key: None,
        self_table: None,
    };
    w.visit_children(root, &top);
    w.settle_pending();
    w.mint_classes();
    w.module_entry(source);

    eprintln!(
        "[lua] functions={} methods={} classes={} heritage={} routes={} endpoints={} tests={} requires={} dynamic_requires={} calls={} file={file_rel_path}",
        w.counts.functions,
        w.counts.methods,
        w.counts.classes,
        w.counts.heritage,
        w.counts.routes,
        w.endpoints.len(),
        w.counts.tests,
        w.counts.requires,
        w.counts.dynamic_requires,
        w.calls.len(),
    );
    if let Some(line) = w.host.marker("lua", file_rel_path) {
        eprintln!("{line}");
    }

    Ok(FileParse {
        nodes: w.nodes,
        edges: w.edges,
        imports: w.imports,
        calls: w.calls,
        refs: w.refs,
        nav: w.nav,
        properties: Default::default(),
    })
}

#[derive(Default)]
struct Counts {
    functions: usize,
    methods: usize,
    classes: usize,
    heritage: usize,
    routes: usize,
    tests: usize,
    requires: usize,
    dynamic_requires: usize,
}

/// Where a subtree sits: the node its calls attribute to, the key of the
/// nearest enclosing named function (a nested local's key prefix) and the
/// table `self` names inside a `function T:m()`.
#[derive(Clone)]
struct Ctx {
    owner: NodeId,
    key: Option<String>,
    self_table: Option<String>,
}

struct Walk<'a> {
    src: &'a [u8],
    file_rel_path: &'a str,
    module_qname: &'a str,
    module_id: NodeId,
    repo: RepoId,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    calls: Vec<CallSite>,
    /// Bare calls no visible local answered, by index into `calls`: settled
    /// against the file's globals once the whole file is read (a global
    /// function is visible before its definition runs).
    pending: Vec<(usize, String)>,
    imports: Vec<ImportStmt>,
    nav: CodeNav,
    /// Every key minted, for the `@<line>` suffix.
    keys: HashSet<String>,
    /// Global function name -> key (`function f()`, `f = function() end`).
    globals: HashMap<String, String>,
    /// Lexical scopes, innermost last: local function name -> key.
    scopes: Vec<HashMap<String, String>>,
    /// Typed locals, scoped with `scopes`: name -> class table.
    types: Vec<HashMap<String, String>>,
    /// Class tables seen (sorted: CLASS nodes mint in this order).
    classes: BTreeSet<String>,
    /// Names a table constructor was assigned to.
    tables: HashSet<String>,
    /// The first assignment that names a table: a CLASS's location.
    decls: HashMap<String, Vec<Cell>>,
    /// Names bound to a `require`: a class of another file, never minted here.
    aliases: HashSet<String>,
    /// Inheritance candidates: (child, base, row, firm). A firm one is an
    /// explicit subclassing call; the rest hold only when the child is a class.
    heritage: Vec<(String, String, u32, bool)>,
    refs: Vec<UnresolvedRef>,
    /// The file requires `lapis`: `x:get("/p", ..)` is a route.
    lapis: bool,
    /// Names bound to `require("socket.http")` / `require("ssl.https")`.
    socket_http: HashSet<String>,
    /// The file requires `resty.http`: `x:request_uri(url, ..)` is a client call.
    resty_http: bool,
    /// ENDPOINT ids already minted in this file.
    endpoints: HashSet<NodeId>,
    /// In a test file, the enclosing busted `describe` names; `None` elsewhere.
    tests: Option<Vec<String>>,
    counts: Counts,
    /// Locals that matter to a host global, scoped with `scopes`: name ->
    /// `Some(<host path>)` for an alias of a host global or one of its fields
    /// (`local api = vim.api`), `None` for any other local or parameter,
    /// which shadows a global of its name.
    host_bindings: Vec<HashMap<String, Option<String>>>,
    host: HostTally,
    /// Chunk-level locals bound to a table literal (host binding
    /// [`TABLE_LOCAL`]`<name>`): name -> the keys set on the table, each with
    /// its site, in source order. A chunk-level `return <name>` in a file
    /// requiring a host that reads the returned table makes them its
    /// settings (GH.5); otherwise they are dropped. Looked up, never iterated.
    table_keys: HashMap<String, Vec<(String, TsNode<'a>)>>,
    /// Host roots the file shows (GH.4): a host global read unshadowed, a
    /// host module required. A gated plugin-unit load holds only for them.
    host_roots: BTreeSet<String>,
    /// The 0-based row of the first chunk-level read of the global `arg`
    /// (GH.4): the file is run as a script.
    script_arg: Option<u32>,
}

/// The host-binding path of a chunk-level local bound to a table literal:
/// `TABLE_LOCAL` + its name. It roots no host path (no row starts with it),
/// so it shadows a host global as any local does.
const TABLE_LOCAL: &str = "\u{1}table:";

impl<'a> Walk<'a> {
    fn visit_children(&mut self, node: TsNode<'a>, ctx: &Ctx) {
        let mut cursor = node.walk();
        let children: Vec<TsNode<'a>> = node.named_children(&mut cursor).collect();
        for child in children {
            self.visit(child, ctx);
        }
    }

    fn visit(&mut self, node: TsNode<'a>, ctx: &Ctx) {
        match node.kind() {
            "function_declaration" => self.function_declaration(node, ctx),
            "variable_declaration" => {
                // `local x = ..`: the assignment inside, its names local.
                let mut cursor = node.walk();
                let children: Vec<TsNode<'a>> = node.named_children(&mut cursor).collect();
                for child in children {
                    if child.kind() == "assignment_statement" {
                        self.assignment(child, ctx, true);
                    } else {
                        if child.kind() == "variable_list" {
                            // `local x`: declared, no value.
                            let mut c = child.walk();
                            let names: Vec<TsNode<'a>> = child.named_children(&mut c).collect();
                            for name in names.iter().filter(|n| n.kind() == "identifier") {
                                self.bind_host(text(*name, self.src), None);
                            }
                        }
                        self.visit(child, ctx);
                    }
                }
            }
            "assignment_statement" => self.assignment(node, ctx, false),
            "return_statement" if ctx.key.is_none() && is_chunk_level(node) => {
                // The module's export table: `return { f = function() end }`.
                let mut cursor = node.walk();
                let values: Vec<TsNode<'a>> = node
                    .named_children(&mut cursor)
                    .filter(|c| c.kind() == "expression_list")
                    .flat_map(|l| {
                        let mut c = l.walk();
                        l.named_children(&mut c).collect::<Vec<_>>()
                    })
                    .collect();
                for value in values {
                    self.returned_settings(value, ctx);
                    if value.kind() == "table_constructor" {
                        self.table(value, None, ctx);
                    } else {
                        self.visit(value, ctx);
                    }
                }
            }
            "function_definition" => {
                // An anonymous function: its own scope, its calls the owner's.
                self.scoped_body(node, ctx);
            }
            "block" => {
                self.push_scope();
                self.visit_children(node, ctx);
                self.pop_scope();
            }
            "function_call" => self.call(node, ctx),
            "dot_index_expression" if self.setting_read(node, ctx) => {}
            "identifier" => self.name_read(node),
            _ => self.visit_children(node, ctx),
        }
    }

    /// `function f()`, `local function f()`, `function T.f()`, `function T:m()`.
    fn function_declaration(&mut self, node: TsNode<'a>, ctx: &Ctx) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        let local = first_token_is(node, "local");
        let (dotted, kind, self_table) = match name.kind() {
            "method_index_expression" => {
                let (Some(table), Some(method)) = (
                    name.child_by_field_name("table"),
                    name.child_by_field_name("method"),
                ) else {
                    return;
                };
                let Some(table) = self.dotted(table, ctx) else {
                    return;
                };
                let dotted = format!("{table}.{}", text(method, self.src));
                self.classes.insert(table.clone());
                (dotted, node_kind::METHOD, Some(table))
            }
            "identifier" | "dot_index_expression" => {
                let Some(dotted) = self.dotted(name, ctx) else {
                    return;
                };
                (dotted, node_kind::FUNCTION, None)
            }
            _ => return,
        };
        let plain = name.kind() == "identifier";
        let callback = (kind == node_kind::FUNCTION)
            .then(|| self.host_callback(name, ctx))
            .flatten();
        let key = if plain && (local || ctx.key.is_some()) {
            nested_key(ctx, &dotted)
        } else {
            dotted.clone()
        };
        let key = self.unique(key, node);
        if plain {
            if local {
                // Visible in its own body (recursion) and after.
                self.bind_local(&dotted, &key);
                self.bind_host(&dotted, None);
            } else {
                self.globals.entry(dotted).or_insert_with(|| key.clone());
            }
        }
        let id = self.mint(&key, kind, node);
        if let Some(host) = callback {
            self.callback_entry(id, host, node);
        }
        let inner = Ctx {
            owner: id,
            key: Some(key),
            self_table: self_table.or_else(|| ctx.self_table.clone()),
        };
        self.scoped_body(node, &inner);
    }

    /// `x = <value>` / `local x = <value>` (several names pair with values
    /// by position). A function value mints the name; a table constructor
    /// mints its function fields under the name; `require(..)` binds the name
    /// as the require's alias; anything else walks.
    fn assignment(&mut self, node: TsNode<'a>, ctx: &Ctx, local: bool) {
        let mut cursor = node.walk();
        let children: Vec<TsNode<'a>> = node.named_children(&mut cursor).collect();
        let names: Vec<TsNode<'a>> = children
            .iter()
            .filter(|c| c.kind() == "variable_list")
            .flat_map(|l| {
                let mut c = l.walk();
                l.named_children(&mut c).collect::<Vec<_>>()
            })
            .collect();
        let values: Vec<TsNode<'a>> = children
            .iter()
            .filter(|c| c.kind() == "expression_list")
            .flat_map(|l| {
                let mut c = l.walk();
                l.named_children(&mut c).collect::<Vec<_>>()
            })
            .collect();
        // A local's host binding: read before the values walk (`local vim =
        // vim` reads the global), bound after it (the new local is not in
        // scope inside its own initialiser).
        let host_bound: Vec<(String, Option<String>)> = if local {
            names
                .iter()
                .enumerate()
                .filter(|(_, n)| n.kind() == "identifier")
                .map(|(i, n)| {
                    let name = text(*n, self.src).to_string();
                    let path = values
                        .get(i)
                        .and_then(|v| self.local_host_path(&name, *v, ctx));
                    (name, path)
                })
                .collect()
        } else {
            Vec::new()
        };
        // Host settings written by this statement (GH.5), read before the
        // values walk so a `vim.o.a = vim.o.b` defines a before it reads b.
        for (name, value) in names.iter().zip(values.iter()) {
            self.setting_write(*name, *value, node, ctx);
        }
        for (i, value) in values.iter().enumerate() {
            if let Some(name) = names.get(i) {
                if let Some(dotted) = self.dotted(*name, ctx) {
                    let at = cells(&node, self.src, self.file_rel_path);
                    self.decls.entry(dotted).or_insert(at);
                }
                self.class_facts(*name, *value, ctx, local);
            }
            let target = names.get(i).and_then(|n| {
                matches!(n.kind(), "identifier" | "dot_index_expression")
                    .then(|| self.dotted(*n, ctx))
                    .flatten()
                    .map(|d| (d, n.kind() == "identifier"))
            });
            match (value.kind(), target) {
                ("function_definition", Some((dotted, plain))) => {
                    let callback = names.get(i).and_then(|n| self.host_callback(*n, ctx));
                    let key = if plain && (local || ctx.key.is_some()) {
                        nested_key(ctx, &dotted)
                    } else {
                        dotted.clone()
                    };
                    let key = self.unique(key, *value);
                    let id = self.mint(&key, node_kind::FUNCTION, *value);
                    if let Some(host) = callback {
                        self.callback_entry(id, host, *value);
                    }
                    let inner = Ctx {
                        owner: id,
                        key: Some(key.clone()),
                        self_table: ctx.self_table.clone(),
                    };
                    self.scoped_body(*value, &inner);
                    if plain {
                        if local {
                            self.bind_local(&dotted, &key);
                        } else {
                            self.globals.entry(dotted).or_insert(key);
                        }
                    }
                }
                ("table_constructor", Some((dotted, _))) => {
                    self.tables.insert(dotted.clone());
                    self.table(*value, Some(&dotted), ctx);
                }
                ("function_call", Some((dotted, true))) if self.is_require(*value) => {
                    self.aliases.insert(dotted.clone());
                    self.require(*value, Some(dotted));
                }
                _ => self.visit(*value, ctx),
            }
        }
        for (name, path) in host_bound {
            self.bind_host(&name, path);
        }
    }

    /// A table constructor's `key = function() end` fields, minted under
    /// `prefix` (`None`: the export table, keys bare), nested tables too.
    fn table(&mut self, node: TsNode<'a>, prefix: Option<&str>, ctx: &Ctx) {
        let mut cursor = node.walk();
        let fields: Vec<TsNode<'a>> = node.named_children(&mut cursor).collect();
        for field in fields {
            let Some(value) = field.child_by_field_name("value") else {
                self.visit(field, ctx);
                continue;
            };
            let name = field
                .child_by_field_name("name")
                .filter(|n| n.kind() == "identifier")
                .map(|n| text(n, self.src).to_string());
            let Some(name) = name else {
                self.visit(value, ctx);
                continue;
            };
            let dotted = match prefix {
                Some(p) => format!("{p}.{name}"),
                None => name,
            };
            match value.kind() {
                "function_definition" => {
                    let key = self.unique(dotted, value);
                    let id = self.mint(&key, node_kind::FUNCTION, value);
                    let inner = Ctx {
                        owner: id,
                        key: Some(key),
                        self_table: ctx.self_table.clone(),
                    };
                    self.scoped_body(value, &inner);
                }
                "table_constructor" => self.table(value, Some(&dotted), ctx),
                _ => self.visit(value, ctx),
            }
        }
    }

    /// A function's parameters and body in a fresh scope.
    fn scoped_body(&mut self, node: TsNode<'a>, ctx: &Ctx) {
        self.push_scope();
        if let Some(params) = node.child_by_field_name("parameters") {
            let mut c = params.walk();
            let names: Vec<TsNode<'a>> = params.named_children(&mut c).collect();
            for name in names.iter().filter(|n| n.kind() == "identifier") {
                self.bind_host(text(*name, self.src), None);
            }
        }
        if let Some(body) = node.child_by_field_name("body") {
            self.visit_children(body, ctx);
        }
        self.pop_scope();
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.types.push(HashMap::new());
        self.host_bindings.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        self.types.pop();
        self.host_bindings.pop();
    }

    fn call(&mut self, node: TsNode<'a>, ctx: &Ctx) {
        if self.is_require(node) {
            self.require(node, None);
            return;
        }
        if self.busted(node, ctx) {
            return;
        }
        if self.lapis_route(node, ctx) {
            return;
        }
        if self.host_registration(node, ctx) {
            return;
        }
        if self.setting_mutator(node, ctx) {
            return;
        }
        self.http_client(node, ctx);
        // `setmetatable(X, meta)` as a statement: X inherits (when X is a
        // class) or is typed by the metatable's class.
        if let Some((Some(target), base)) = self.setmetatable_args(node, ctx) {
            self.bind_type(&target, &base);
            self.heritage.push((target, base, row(node), false));
        }
        let line = row(node);
        if let Some(callee) = node.child_by_field_name("name") {
            match callee.kind() {
                "identifier" => {
                    let name = text(callee, self.src).to_string();
                    let index = self.calls.len();
                    let qualifier = match self.lookup_local(&name) {
                        Some(key) => CallQualifier::Bare(key),
                        None => {
                            self.pending.push((index, name.clone()));
                            CallQualifier::Bare(name)
                        }
                    };
                    self.calls.push(CallSite {
                        from: ctx.owner,
                        qualifier,
                        line,
                    });
                }
                "dot_index_expression" | "method_index_expression" => {
                    let (table, member) = if callee.kind() == "dot_index_expression" {
                        (
                            callee.child_by_field_name("table"),
                            callee.child_by_field_name("field"),
                        )
                    } else {
                        (
                            callee.child_by_field_name("table"),
                            callee.child_by_field_name("method"),
                        )
                    };
                    if let (Some(table), Some(member)) = (table, member)
                        && let Some(base) = self.call_base(table, ctx)
                    {
                        self.calls.push(CallSite {
                            from: ctx.owner,
                            qualifier: CallQualifier::Attribute {
                                base,
                                name: text(member, self.src).to_string(),
                            },
                            line,
                        });
                    }
                }
                _ => {}
            }
        }
        // Calls in the callee (`f(x)(y)`) and in the arguments.
        self.visit_children(node, ctx);
    }

    /// A busted block in a test file: `describe` / `context` / `insulate` /
    /// `expose` prefix the tests inside; `it` / `test` / `spec` mint one
    /// FUNCTION, key `<describe ..> <name>`, that owns its calls. The block's
    /// first argument must be a string literal and one argument an anonymous
    /// function; anything else is an ordinary call.
    fn busted(&mut self, node: TsNode<'a>, ctx: &Ctx) -> bool {
        if self.tests.is_none() {
            return false;
        }
        let Some(callee) = node
            .child_by_field_name("name")
            .filter(|c| c.kind() == "identifier")
        else {
            return false;
        };
        let block = text(callee, self.src);
        let describe = matches!(block, "describe" | "context" | "insulate" | "expose");
        if !describe && !matches!(block, "it" | "test" | "spec") {
            return false;
        }
        let Some(args) = node.child_by_field_name("arguments") else {
            return false;
        };
        let mut cursor = args.walk();
        let argv: Vec<TsNode<'a>> = args.named_children(&mut cursor).collect();
        let Some(name) = argv
            .first()
            .filter(|a| a.kind() == "string")
            .and_then(|a| a.child_by_field_name("content"))
            .map(|c| text(c, self.src).to_string())
        else {
            return false;
        };
        let Some(body) = argv
            .iter()
            .find(|a| a.kind() == "function_definition")
            .copied()
        else {
            return false;
        };
        if describe {
            if let Some(stack) = self.tests.as_mut() {
                stack.push(name);
            }
            self.scoped_body(body, ctx);
            if let Some(stack) = self.tests.as_mut() {
                stack.pop();
            }
            return true;
        }
        let mut words = self.tests.clone().unwrap_or_default();
        words.push(name);
        let key = self.unique(words.join(" "), node);
        self.counts.tests += 1;
        let id = self.mint(&key, node_kind::FUNCTION, node);
        let inner = Ctx {
            owner: id,
            key: Some(key),
            self_table: ctx.self_table.clone(),
        };
        self.scoped_body(body, &inner);
        true
    }

    /// The class facts of `name = value`: `T.__index = T`, a typed local from
    /// a constructor, a subclassing call.
    fn class_facts(&mut self, name: TsNode<'a>, value: TsNode<'a>, ctx: &Ctx, local: bool) {
        let Some(target) = self.dotted(name, ctx) else {
            return;
        };
        // `T.__index = T` / `Child.__index = Base`.
        if let Some(table) = target.strip_suffix(".__index") {
            if let Some(base) = self.dotted(value, ctx) {
                self.classes.insert(table.to_string());
                self.classes.insert(base.clone());
                if base != table {
                    self.heritage
                        .push((table.to_string(), base, row(value), false));
                }
            }
            return;
        }
        if value.kind() != "function_call" {
            return;
        }
        let plain = name.kind() == "identifier";
        if let Some((_, base)) = self.setmetatable_args(value, ctx) {
            // `local x = setmetatable({}, T)`: x is a T; when x turns out to be a
            // class itself (it gets methods), it inherits T.
            if plain {
                self.bind_type(&target, &base);
            }
            self.heritage.push((target, base, row(value), false));
            return;
        }
        let Some(callee) = value.child_by_field_name("name") else {
            return;
        };
        match callee.kind() {
            // `T.new(..)` / `T:new(..)`: an instance of T. `B:extend(..)` /
            // `B:subclass(..)`: a class inheriting B.
            "dot_index_expression" | "method_index_expression" => {
                let member = callee
                    .child_by_field_name("field")
                    .or_else(|| callee.child_by_field_name("method"))
                    .map(|m| text(m, self.src));
                let Some(table) = callee
                    .child_by_field_name("table")
                    .and_then(|t| self.dotted(t, ctx))
                else {
                    return;
                };
                match member {
                    Some("new") if plain => self.bind_type(&target, &table),
                    Some("extend" | "subclass") => {
                        self.classes.insert(target.clone());
                        self.classes.insert(table.clone());
                        self.heritage.push((target, table, row(value), true));
                    }
                    _ => {}
                }
            }
            // middleclass: `class("Dog", Animal)` / `class("Animal")`.
            "identifier" if text(callee, self.src) == "class" => {
                self.classes.insert(target.clone());
                let args = value.child_by_field_name("arguments");
                let base = args.and_then(|a| {
                    let mut c = a.walk();
                    let argv: Vec<TsNode<'a>> = a.named_children(&mut c).collect();
                    argv.get(1).and_then(|b| self.dotted(*b, ctx))
                });
                if let Some(base) = base {
                    self.classes.insert(base.clone());
                    self.heritage.push((target, base, row(value), true));
                }
            }
            _ => {}
        }
        let _ = local;
    }

    /// `setmetatable(x, meta)`: `(x's name when plain, meta's class)`, the
    /// class being `meta` itself or its `__index` field. Marks the class.
    fn setmetatable_args(
        &mut self,
        node: TsNode<'a>,
        ctx: &Ctx,
    ) -> Option<(Option<String>, String)> {
        let callee = node.child_by_field_name("name")?;
        if callee.kind() != "identifier" || text(callee, self.src) != "setmetatable" {
            return None;
        }
        let args = node.child_by_field_name("arguments")?;
        let mut cursor = args.walk();
        let argv: Vec<TsNode<'a>> = args.named_children(&mut cursor).collect();
        let [first, meta] = argv.as_slice() else {
            return None;
        };
        let base = match meta.kind() {
            "identifier" | "dot_index_expression" => self.dotted(*meta, ctx)?,
            "table_constructor" => {
                let mut c = meta.walk();
                let fields: Vec<TsNode<'a>> = meta.named_children(&mut c).collect();
                fields.into_iter().find_map(|f| {
                    let key = f.child_by_field_name("name")?;
                    (text(key, self.src) == "__index")
                        .then(|| f.child_by_field_name("value"))
                        .flatten()
                        .and_then(|v| self.dotted(v, ctx))
                })?
            }
            _ => return None,
        };
        self.classes.insert(base.clone());
        let target = (first.kind() == "identifier")
            .then(|| self.dotted(*first, ctx))
            .flatten();
        Some((target, base))
    }

    /// A call's receiver text: the method's table for `self`, a typed local's
    /// class, else the dotted text.
    fn call_base(&self, node: TsNode<'a>, ctx: &Ctx) -> Option<String> {
        if node.kind() == "identifier" {
            let name = text(node, self.src);
            // A class table is no instance: `Dog.new()` stays on Dog even
            // though `local Dog = setmetatable({}, ..)` typed the name.
            let own_self = name == "self" && ctx.self_table.is_some();
            if !own_self
                && !self.classes.contains(name)
                && let Some(ty) = self.types.iter().rev().find_map(|s| s.get(name))
            {
                return Some(ty.clone());
            }
        }
        self.dotted(node, ctx)
    }

    fn bind_type(&mut self, name: &str, class: &str) {
        if let Some(scope) = self.types.last_mut() {
            scope.insert(name.to_string(), class.to_string());
        }
    }

    /// Mint one CLASS per class table this file defines (not a require alias,
    /// and a table it assigns, or one with members or a firm subclassing),
    /// then the INHERITS_FROM refs whose child is one.
    fn mint_classes(&mut self) {
        let firm: HashSet<String> = self
            .heritage
            .iter()
            .filter(|h| h.3)
            .map(|h| h.0.clone())
            .collect();
        let defined = |w: &Self, name: &str| {
            !w.aliases.contains(name)
                && name != "self"
                && (w.tables.contains(name)
                    || firm.contains(name)
                    || w.keys.iter().any(|k| {
                        k.strip_prefix(name)
                            .is_some_and(|rest| rest.starts_with('.'))
                    }))
        };
        let mut ids: HashMap<String, NodeId> = HashMap::new();
        let classes: Vec<String> = self.classes.iter().cloned().collect();
        for class in classes {
            if !defined(self, &class) {
                continue;
            }
            let key = if self.keys.contains(&class) {
                format!("{class}@class")
            } else {
                class.clone()
            };
            self.keys.insert(key.clone());
            let qname = format!("{}::{key}", self.module_qname);
            let id = NodeId::from_parts(GRAPH_TYPE, self.repo, node_kind::CLASS, &qname);
            // Located at the assignment that first names the table, else (a
            // table only `function T:m()` declares) the file.
            let at = self.decls.get(&class).cloned().unwrap_or_else(|| {
                self.nodes
                    .first()
                    .map(|n| n.cells.clone())
                    .unwrap_or_default()
            });
            self.nodes.push(Node {
                id,
                repo: self.repo,
                confidence: Confidence::Strong,
                cells: at,
            });
            self.edges.push(Edge::new(
                self.module_id,
                id,
                edge_category::DEFINES,
                Confidence::Strong,
            ));
            self.nav
                .record(id, &key, &qname, node_kind::CLASS, Some(self.module_id));
            self.counts.classes += 1;
            ids.insert(class, id);
        }
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for (child, base, line, _) in std::mem::take(&mut self.heritage) {
            let Some(from) = ids.get(&child).copied() else {
                continue;
            };
            if child == base || !seen.insert((child, base.clone())) {
                continue;
            }
            self.counts.heritage += 1;
            self.refs.push(UnresolvedRef {
                from,
                from_module: self.module_id,
                qualifier: CallQualifier::Bare(base),
                category: edge_category::INHERITS_FROM,
                line,
            });
        }
    }

    /// A Lapis route: `app:get("/p", handler)` (or `app:get("name", "/p",
    /// handler)`), `match` = ANY. Mints the ROUTE and a HANDLED_BY ref; an
    /// anonymous handler is minted as FUNCTION `<METHOD> <path>` and owns its
    /// calls.
    fn lapis_route(&mut self, node: TsNode<'a>, ctx: &Ctx) -> bool {
        if !self.lapis {
            return false;
        }
        let Some(callee) = node
            .child_by_field_name("name")
            .filter(|c| c.kind() == "method_index_expression")
        else {
            return false;
        };
        let verb = match callee
            .child_by_field_name("method")
            .map(|m| text(m, self.src))
        {
            Some("get") => "GET",
            Some("post") => "POST",
            Some("put") => "PUT",
            Some("delete") => "DELETE",
            Some("patch") => "PATCH",
            Some("match") => "ANY",
            _ => return false,
        };
        let Some(args) = node.child_by_field_name("arguments") else {
            return false;
        };
        let mut cursor = args.walk();
        let argv: Vec<TsNode<'a>> = args.named_children(&mut cursor).collect();
        let strings: Vec<String> = argv
            .iter()
            .take_while(|a| a.kind() == "string")
            .filter_map(|a| a.child_by_field_name("content"))
            .map(|c| text(c, self.src).to_string())
            .collect();
        // The path: the last leading string that starts with `/`.
        let Some(path) = strings.iter().rev().find(|p| p.starts_with('/')).cloned() else {
            return false;
        };
        let Some(handler) = argv.get(strings.len()).copied() else {
            return false;
        };
        let line = row(node);
        let qname = route_qname(verb, &path);
        let route = NodeId::from_parts(GRAPH_TYPE, self.repo, node_kind::ROUTE, &qname);
        let mut route_cells = vec![Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text(verb.to_string()),
        }];
        route_cells.extend(
            cells(&node, self.src, self.file_rel_path)
                .into_iter()
                .skip(1),
        );
        self.nodes.push(Node {
            id: route,
            repo: self.repo,
            confidence: Confidence::Strong,
            cells: route_cells,
        });
        self.nav
            .record(route, &qname, &qname, node_kind::ROUTE, None);
        self.counts.routes += 1;
        let qualifier = match handler.kind() {
            "function_definition" => {
                let key = self.unique(format!("{verb} {path}"), node);
                let id = self.mint(&key, node_kind::FUNCTION, handler);
                let inner = Ctx {
                    owner: id,
                    key: Some(key.clone()),
                    self_table: ctx.self_table.clone(),
                };
                self.scoped_body(handler, &inner);
                Some(CallQualifier::Bare(key))
            }
            "identifier" => {
                let name = text(handler, self.src).to_string();
                Some(CallQualifier::Bare(
                    self.lookup_local(&name).unwrap_or(name),
                ))
            }
            "dot_index_expression" => self.dotted(handler, ctx).and_then(|d| {
                let (base, name) = d.rsplit_once('.')?;
                Some(CallQualifier::Attribute {
                    base: base.to_string(),
                    name: name.to_string(),
                })
            }),
            _ => None,
        };
        if let Some(qualifier) = qualifier {
            self.refs.push(UnresolvedRef {
                from: route,
                from_module: self.module_id,
                qualifier,
                category: edge_category::HANDLED_BY,
                line,
            });
        }
        true
    }

    /// A host registration (code-domain [`hosts`]): a call whose dotted
    /// callee, rooted at an unshadowed host global, matches a row's
    /// registration, with the name a string literal. Mints the CLI_COMMAND /
    /// EVENT_HANDLER and a HANDLED_BY ref to each handler; an anonymous
    /// handler is minted as FUNCTION `command <name>` (`on <event>`) and owns
    /// its calls, as a Lapis route's does. The call's other arguments walk
    /// from the enclosing scope. A prefix match that registers nothing
    /// (`aux=`) and a name that is no literal (`unread=`) fall through to
    /// the ordinary call.
    fn host_registration(&mut self, node: TsNode<'a>, ctx: &Ctx) -> bool {
        let Some(callee) = node
            .child_by_field_name("name")
            .filter(|c| c.kind() == "dot_index_expression")
        else {
            return false;
        };
        let Some(path) = self.host_path(callee, ctx) else {
            return false;
        };
        let Some(found) = registration_for("lua", &path) else {
            return false;
        };
        let host = found.host.name;
        let pattern = found.registration.pattern;
        self.host.hit(host);
        if !found.mints {
            self.host.aux += 1;
            return false;
        }
        let read = node
            .child_by_field_name("arguments")
            .and_then(|args| read_registration(args, found.registration.shape, self.src));
        let Some(read) = read else {
            self.host.unread += 1;
            return false;
        };
        let mut handlers = Vec::new();
        for (i, (field, handler)) in read.handlers.iter().enumerate() {
            // The first handler is the registration's own: `command <name>`.
            let field = if i == 0 { None } else { *field };
            if let Some(q) = self.host_handler(*handler, pattern, &read.name, field, ctx) {
                handlers.push(q);
            }
        }
        for rest in read.rest {
            self.visit(rest, ctx);
        }
        let cells: Vec<Cell> = cells(&node, self.src, self.file_rel_path)
            .into_iter()
            .skip(1)
            .collect();
        let site = HostSite::new(self.repo, self.module_id, host, row(node), cells);
        let mut sink = HostSink::new(&mut self.nodes, &mut self.nav, &mut self.refs);
        push_registration(&site, pattern, &read.name, &handlers, &mut sink);
        self.host.registered(pattern);
        true
    }

    /// The HANDLED_BY qualifier of a registration's handler: an anonymous
    /// function minted under [`Pattern::handler_key`], a local or global
    /// function by name, a table field as an attribute. A string (Neovim's
    /// `":cmd"` form) names no handler; anything else walks and names none.
    fn host_handler(
        &mut self,
        handler: TsNode<'a>,
        pattern: Pattern,
        name: &str,
        field: Option<&str>,
        ctx: &Ctx,
    ) -> Option<CallQualifier> {
        match handler.kind() {
            "function_definition" => {
                let key = self.unique(pattern.handler_key(name, field), handler);
                let id = self.mint(&key, node_kind::FUNCTION, handler);
                self.host.minted += 1;
                let inner = Ctx {
                    owner: id,
                    key: Some(key.clone()),
                    self_table: ctx.self_table.clone(),
                };
                self.scoped_body(handler, &inner);
                Some(CallQualifier::Bare(key))
            }
            "identifier" => {
                let name = text(handler, self.src).to_string();
                Some(CallQualifier::Bare(
                    self.lookup_local(&name).unwrap_or(name),
                ))
            }
            "dot_index_expression" => self.dotted(handler, ctx).and_then(|d| {
                let (base, name) = d.rsplit_once('.')?;
                Some(CallQualifier::Attribute {
                    base: base.to_string(),
                    name: name.to_string(),
                })
            }),
            "string" => None,
            _ => {
                self.visit(handler, ctx);
                None
            }
        }
    }

    /// The dotted path of an identifier / `a.b` chain rooted at a host
    /// global ([`hosts::is_host_global`]), a local alias of one expanded
    /// (`api.x` with `local api = vim.api` is `vim.api.x`); `None` when the
    /// root is any other name or a local shadows the global.
    fn host_path(&self, node: TsNode<'a>, ctx: &Ctx) -> Option<String> {
        if !matches!(node.kind(), "identifier" | "dot_index_expression") {
            return None;
        }
        let dotted = self.dotted(node, ctx)?;
        let (root, rest) = match dotted.split_once('.') {
            Some((r, t)) => (r, Some(t)),
            None => (dotted.as_str(), None),
        };
        let base = match self.host_bindings.iter().rev().find_map(|s| s.get(root)) {
            Some(Some(path)) => path.clone(),
            Some(None) => return None,
            None if hosts::is_host_global("lua", root) => root.to_string(),
            None => return None,
        };
        Some(match rest {
            Some(t) => format!("{base}.{t}"),
            None => base,
        })
    }

    fn bind_host(&mut self, name: &str, path: Option<String>) {
        if let Some(scope) = self.host_bindings.last_mut() {
            scope.insert(name.to_string(), path);
        }
    }

    /// The host binding of `local <name> = <value>`: a host path (an alias
    /// of a host global or of one of its fields), the required host module
    /// (`require("wezterm")`), a builder call's result (`wezterm.config_builder()`,
    /// path `<prefix>()`), or, at chunk level, a table literal
    /// ([`TABLE_LOCAL`]`<name>`, its keys recorded); `None` otherwise.
    fn local_host_path(&mut self, name: &str, value: TsNode<'a>, ctx: &Ctx) -> Option<String> {
        match value.kind() {
            "function_call" if self.is_require(value) => value
                .child_by_field_name("arguments")
                .and_then(|a| single_string(a, self.src))
                .filter(|spec| host_for_require("lua", spec).is_some()),
            "function_call" => {
                let callee = value.child_by_field_name("name")?;
                let path = self.host_path(callee, ctx)?;
                config_builder_for("lua", &path).map(|_| format!("{path}()"))
            }
            "table_constructor" if ctx.key.is_none() && self.host_bindings.len() == 1 => {
                let keys = self.literal_keys(value);
                self.table_keys.insert(name.to_string(), keys);
                Some(format!("{TABLE_LOCAL}{name}"))
            }
            _ => self.host_path(value, ctx),
        }
    }

    /// The `<key> = <value>` fields of a table literal whose value is no
    /// function, each with its field: a returned table's settings.
    fn literal_keys(&self, table: TsNode<'a>) -> Vec<(String, TsNode<'a>)> {
        let mut cursor = table.walk();
        let fields: Vec<TsNode<'a>> = table.named_children(&mut cursor).collect();
        fields
            .into_iter()
            .filter(|f| {
                f.child_by_field_name("value")
                    .is_some_and(|v| v.kind() != "function_definition")
            })
            .filter_map(|f| {
                let key = f
                    .child_by_field_name("name")
                    .filter(|n| n.kind() == "identifier")?;
                Some((text(key, self.src).to_string(), f))
            })
            .collect()
    }

    /// `<target> = <value>` where target is a host setting (code-domain
    /// [`hosts::config_for`]: `vim.opt.number`, a builder local's
    /// `config.font_size`): a DEFINES_CONFIG from the MODULE, at the
    /// statement. A key set on a chunk-level table local is recorded for its
    /// return. A function value is never a setting.
    fn setting_write(&mut self, target: TsNode<'a>, value: TsNode<'a>, at: TsNode<'a>, ctx: &Ctx) {
        if target.kind() != "dot_index_expression" || value.kind() == "function_definition" {
            return;
        }
        let Some(path) = self.host_path(target, ctx) else {
            return;
        };
        if let Some(rest) = path.strip_prefix(TABLE_LOCAL) {
            if let Some((local, key)) = rest.split_once('.')
                && is_identifier(key)
                && let Some(keys) = self.table_keys.get_mut(local)
            {
                keys.push((key.to_string(), at));
            }
            return;
        }
        if let Some(found) = config_for("lua", &path) {
            self.host_setting(&found, ConfigSide::Define, self.module_id, at);
        }
    }

    /// `<setting>:append(..)` / `:prepend` / `:remove` on an
    /// [`ConfigFlavour::Assign`] setting (`vim.opt.shortmess:append("c")`):
    /// a DEFINES_CONFIG from the MODULE; the arguments walk.
    fn setting_mutator(&mut self, node: TsNode<'a>, ctx: &Ctx) -> bool {
        let Some(callee) = node
            .child_by_field_name("name")
            .filter(|c| c.kind() == "method_index_expression")
        else {
            return false;
        };
        let (Some(table), Some(method)) = (
            callee.child_by_field_name("table"),
            callee.child_by_field_name("method"),
        ) else {
            return false;
        };
        if !hosts::CONFIG_MUTATORS.contains(&text(method, self.src)) {
            return false;
        }
        let Some(found) = self
            .host_path(table, ctx)
            .and_then(|p| config_for("lua", &p))
            .filter(|m| m.flavour == ConfigFlavour::Assign)
        else {
            return false;
        };
        self.host_setting(&found, ConfigSide::Define, self.module_id, node);
        if let Some(args) = node.child_by_field_name("arguments") {
            self.visit(args, ctx);
        }
        true
    }

    /// An rvalue `<setting>` of an [`ConfigFlavour::Assign`] host
    /// (`vim.o.background`): a READS_CONFIG from the enclosing scope. A
    /// builder local's fields are the file's own table, never a read.
    fn setting_read(&mut self, node: TsNode<'a>, ctx: &Ctx) -> bool {
        let Some(found) = self
            .host_path(node, ctx)
            .and_then(|p| config_for("lua", &p))
            .filter(|m| m.flavour == ConfigFlavour::Assign)
        else {
            return false;
        };
        self.host_setting(&found, ConfigSide::Read, ctx.owner, node);
        true
    }

    /// A chunk-level returned value that is a host's config, in a file
    /// requiring a host that reads the returned table (code-domain
    /// [`hosts::returned_config_host`]): a table literal's keys, or the keys
    /// recorded for a returned table local. Each a DEFINES_CONFIG from the
    /// MODULE at its site.
    fn returned_settings(&mut self, value: TsNode<'a>, ctx: &Ctx) {
        let keys = match value.kind() {
            "table_constructor" => self.literal_keys(value),
            "identifier" => match self.host_path(value, ctx) {
                Some(path) => match path.strip_prefix(TABLE_LOCAL) {
                    Some(local) => self.table_keys.remove(local).unwrap_or_default(),
                    None => return,
                },
                None => return,
            },
            _ => return,
        };
        if keys.is_empty() {
            return;
        }
        let required: Vec<&str> = self
            .imports
            .iter()
            .filter_map(|i| match &i.target {
                ImportTarget::Module { path, .. } => Some(path.as_str()),
                _ => None,
            })
            .collect();
        let Some(host) = returned_config_host("lua", required.into_iter()) else {
            return;
        };
        for (key, at) in keys {
            let found = ConfigMatch::new(host, ConfigFlavour::Builder, key);
            self.host_setting(&found, ConfigSide::Define, self.module_id, at);
        }
    }

    /// Mint a host setting's CONFIG_KEY (code-domain
    /// [`hosts::push_config_key`]) located at `at`, its edge from `from`.
    fn host_setting(
        &mut self,
        found: &ConfigMatch,
        side: ConfigSide,
        from: NodeId,
        at: TsNode<'a>,
    ) {
        let cells: Vec<Cell> = cells(&at, self.src, self.file_rel_path)
            .into_iter()
            .skip(1)
            .collect();
        let site = HostSite::new(self.repo, self.module_id, found.host.name, row(at), cells);
        let mut sink = HostSink::new(&mut self.nodes, &mut self.nav, &mut self.refs);
        push_config_key(&site, found, side, from, &mut sink, &mut self.edges);
        self.host.config += 1;
        self.host.hit(found.host.name);
    }

    /// A client call: `socket.http`'s `http.request(url[, body])` /
    /// `http.request{ url = .., method = .. }`, `resty.http`'s
    /// `x:request_uri(url[, { method = .. }])`. The URL must be a string
    /// literal; its path is the ENDPOINT, its authority the hit's `host`.
    fn http_client(&mut self, node: TsNode<'a>, ctx: &Ctx) {
        let Some(callee) = node.child_by_field_name("name") else {
            return;
        };
        let (table, member) = match callee.kind() {
            "dot_index_expression" => (
                callee.child_by_field_name("table"),
                callee.child_by_field_name("field"),
            ),
            "method_index_expression" => (
                callee.child_by_field_name("table"),
                callee.child_by_field_name("method"),
            ),
            _ => return,
        };
        let (Some(table), Some(member)) = (table, member) else {
            return;
        };
        let member = text(member, self.src);
        let socket = member == "request"
            && callee.kind() == "dot_index_expression"
            && self.socket_http.contains(text(table, self.src));
        let resty = member == "request_uri" && self.resty_http;
        if !socket && !resty {
            return;
        }
        let Some(args) = node.child_by_field_name("arguments") else {
            return;
        };
        let argv: Vec<TsNode<'a>> = if args.kind() == "arguments" {
            let mut cursor = args.walk();
            args.named_children(&mut cursor).collect()
        } else {
            vec![args]
        };
        let field = |t: TsNode<'a>, key: &str| -> Option<String> {
            let mut c = t.walk();
            let fields: Vec<TsNode<'a>> = t.named_children(&mut c).collect();
            fields.into_iter().find_map(|f| {
                let k = f.child_by_field_name("name")?;
                if text(k, self.src) != key {
                    return None;
                }
                let v = f
                    .child_by_field_name("value")
                    .filter(|v| v.kind() == "string")?;
                Some(text(v.child_by_field_name("content")?, self.src).to_string())
            })
        };
        let literal = |n: TsNode<'a>| url_text(n, self.src);
        let (url, method) = match argv.first() {
            Some(t) if t.kind() == "table_constructor" => (field(*t, "url"), field(*t, "method")),
            Some(first) => {
                let options = argv.get(1).filter(|o| o.kind() == "table_constructor");
                let method = match options {
                    Some(o) => field(*o, "method"),
                    // socket.http: a second (body) argument makes it a POST.
                    None if socket && argv.len() > 1 => Some("POST".to_string()),
                    None => None,
                };
                (literal(*first), method)
            }
            None => (None, None),
        };
        let Some(url) = url else {
            return;
        };
        let (host, path) = client_url_split(&url);
        let Some(path) = path else {
            return;
        };
        let pos = node.start_position();
        let ep = ClientEndpoint {
            method: method
                .unwrap_or_else(|| "GET".to_string())
                .to_ascii_uppercase(),
            path,
            file: self.file_rel_path.to_string(),
            line: pos.row + 1,
            col: pos.column + 1,
            confidence: Confidence::Strong,
        };
        let extras = HitExtras {
            host: host.as_deref(),
            ..HitExtras::default()
        };
        push_client_endpoint_with(
            self.repo,
            &ep,
            extras,
            ctx.owner,
            &mut self.nodes,
            &mut self.edges,
            &mut self.nav,
            &mut self.endpoints,
        );
    }

    fn is_require(&self, node: TsNode<'a>) -> bool {
        node.kind() == "function_call"
            && node
                .child_by_field_name("name")
                .is_some_and(|c| c.kind() == "identifier" && text(c, self.src) == "require")
    }

    /// `require("spec")`: an IMPORTS statement, the spec verbatim; a
    /// non-literal argument is counted, never guessed.
    fn require(&mut self, node: TsNode<'a>, alias: Option<String>) {
        self.counts.requires += 1;
        let spec = node
            .child_by_field_name("arguments")
            .and_then(|a| single_string(a, self.src))
            .filter(|s| !s.is_empty());
        let Some(spec) = spec else {
            self.counts.dynamic_requires += 1;
            return;
        };
        match spec.as_str() {
            s if s == "lapis" || s.starts_with("lapis.") => self.lapis = true,
            "socket.http" | "ssl.https" => {
                if let Some(a) = &alias {
                    self.socket_http.insert(a.clone());
                }
            }
            "resty.http" => self.resty_http = true,
            _ => {}
        }
        if let Some(host) = host_for_require("lua", &spec) {
            self.host_roots.insert(host.gate.root().to_string());
        }
        self.imports.push(ImportStmt {
            from_module: self.module_qname.to_string(),
            target: ImportTarget::Module { path: spec, alias },
            line: row(node),
        });
    }

    /// The dotted text of an identifier / `a.b.c` chain; `self` reads as the
    /// enclosing method's table. `None` for anything dynamic.
    fn dotted(&self, node: TsNode<'a>, ctx: &Ctx) -> Option<String> {
        match node.kind() {
            "identifier" => {
                let t = text(node, self.src);
                match (&ctx.self_table, t) {
                    (Some(table), "self") => Some(table.clone()),
                    _ => Some(t.to_string()),
                }
            }
            "dot_index_expression" => {
                let table = node.child_by_field_name("table")?;
                let field = node.child_by_field_name("field")?;
                Some(format!(
                    "{}.{}",
                    self.dotted(table, ctx)?,
                    text(field, self.src)
                ))
            }
            _ => None,
        }
    }

    /// The host that calls the function declared under `name` by its name
    /// (GH.4, [`hosts::callback_for`]): `love.update`, the root read as
    /// [`Walk::host_path`] reads it, so a local `love` is the file's own.
    fn host_callback(&self, name: TsNode<'a>, ctx: &Ctx) -> Option<&'static hosts::HostRow> {
        if name.kind() != "dot_index_expression" {
            return None;
        }
        let path = self.host_path(name, ctx)?;
        hosts::callback_for("lua", &path)
    }

    /// The callback `id` declared at `at` is an entry of `host`.
    fn callback_entry(&mut self, id: NodeId, host: &'static hosts::HostRow, at: TsNode<'a>) {
        if self.entry(id, &format!("host:{}", host.name), row(at)) {
            self.host.callbacks += 1;
            self.host.hit(host.name);
        }
    }

    /// One ENTRYPOINT cell ([`hosts::entry_cell`]) on the node `id`, unless it
    /// holds one already. True when written.
    fn entry(&mut self, id: NodeId, source: &str, row0: u32) -> bool {
        let Some(node) = self.nodes.iter_mut().rev().find(|n| n.id == id) else {
            return false;
        };
        if node.cells.iter().any(|c| c.kind == cell_type::ENTRYPOINT) {
            return false;
        }
        let row1 = row0.saturating_add(1);
        node.cells
            .push(hosts::entry_cell(self.file_rel_path, row1, source));
        true
    }

    /// An identifier in a value position: a host global read unshadowed
    /// marks its host present; the first chunk-level read of the global
    /// `arg` (no local, parameter or loop variable of that name in scope,
    /// no enclosing function) marks the file a script.
    fn name_read(&mut self, node: TsNode<'a>) {
        let Some(parent) = node.parent() else {
            return;
        };
        let is_field = |f: &str| {
            parent
                .child_by_field_name(f)
                .is_some_and(|c| c.id() == node.id())
        };
        let member = match parent.kind() {
            "dot_index_expression" => is_field("field"),
            "method_index_expression" => is_field("method"),
            "field" => is_field("name"),
            _ => false,
        };
        if member {
            return;
        }
        let name = text(node, self.src);
        if self.host_bindings.iter().any(|s| s.contains_key(name)) {
            return;
        }
        if hosts::is_host_global("lua", name) {
            self.host_roots.insert(name.to_string());
        }
        if name == "arg" && self.script_arg.is_none() && global_at_chunk_level(node, self.src) {
            self.script_arg = Some(row(node));
        }
    }

    /// The MODULE's entry, once the file is read (GH.4): a plugin unit its
    /// host loads by path ([`hosts::load_for`], a gated load only where the
    /// file shows the host), else a script whose `#!` names a Lua
    /// interpreter ([`hosts::shebang_entry`]), else one reading the global
    /// `arg` at chunk level.
    fn module_entry(&mut self, source: &str) {
        let loaded = hosts::load_for("lua", self.file_rel_path, |h| {
            self.host.hosts.contains_key(h.name) || self.host_roots.contains(h.gate.root())
        });
        if let Some(host) = loaded {
            if self.entry(self.module_id, &format!("host:{}", host.name), 0) {
                self.host.loads += 1;
            }
            return;
        }
        let first = source.lines().next().unwrap_or("");
        let script = if hosts::shebang_entry(first, LUA_INTERPRETERS).is_some() {
            Some(("shebang", 0))
        } else {
            self.script_arg.map(|r| ("script_arg", r))
        };
        if let Some((how, at)) = script
            && self.entry(self.module_id, how, at)
        {
            eprintln!(
                "[script-entry] lang=lua source={how} module={} file={}",
                self.module_qname, self.file_rel_path
            );
        }
    }

    fn bind_local(&mut self, name: &str, key: &str) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), key.to_string());
        }
    }

    fn lookup_local(&self, name: &str) -> Option<String> {
        self.scopes.iter().rev().find_map(|s| s.get(name).cloned())
    }

    /// Bare calls no local answered: a global function of this file, else
    /// the name as written.
    fn settle_pending(&mut self) {
        for (index, name) in std::mem::take(&mut self.pending) {
            if let Some(key) = self.globals.get(&name)
                && let Some(call) = self.calls.get_mut(index)
            {
                call.qualifier = CallQualifier::Bare(key.clone());
            }
        }
    }

    /// `key`, or `key@<line>` when this file already minted it.
    fn unique(&mut self, key: String, node: TsNode<'a>) -> String {
        let key = if self.keys.contains(&key) {
            format!("{key}@{}", node.start_position().row + 1)
        } else {
            key
        };
        self.keys.insert(key.clone());
        key
    }

    fn mint(&mut self, key: &str, kind: NodeKindId, node: TsNode<'a>) -> NodeId {
        if kind == node_kind::METHOD {
            self.counts.methods += 1;
        } else {
            self.counts.functions += 1;
        }
        let qname = format!("{}::{key}", self.module_qname);
        let id = NodeId::from_parts(GRAPH_TYPE, self.repo, kind, &qname);
        self.nodes.push(Node {
            id,
            repo: self.repo,
            confidence: Confidence::Strong,
            cells: cells(&node, self.src, self.file_rel_path),
        });
        self.edges.push(Edge::new(
            self.module_id,
            id,
            edge_category::DEFINES,
            Confidence::Strong,
        ));
        self.nav.record(id, key, &qname, kind, Some(self.module_id));
        id
    }
}

/// A registration's name, handlers and other arguments, by its shape.
/// `None` when the name is no string literal, the arguments do not have
/// the shape, or the shape is not one a Lua call can hold.
fn read_registration<'t>(args: TsNode<'t>, shape: Shape, src: &[u8]) -> Option<HostRead<'t>> {
    let argv: Vec<TsNode<'t>> = if args.kind() == "arguments" {
        let mut cursor = args.walk();
        args.named_children(&mut cursor).collect()
    } else {
        // `f{ .. }` / `f"x"`: the one argument is the node itself.
        vec![args]
    };
    match shape {
        Shape::Table {
            name_key,
            handler_keys,
        } => {
            let [table] = argv.as_slice() else {
                return None;
            };
            let (fields, mut rest) = table_fields(*table, src)?;
            let name = fields
                .iter()
                .find(|(k, _)| k == name_key)
                .and_then(|(_, v)| string_literal(*v, src))?;
            let mut handlers = Vec::new();
            for key in handler_keys {
                if let Some((_, v)) = fields.iter().find(|(k, _)| k == key) {
                    handlers.push((Some(*key), *v));
                }
            }
            rest.extend(
                fields
                    .iter()
                    .filter(|(k, _)| k != name_key && !handler_keys.contains(&k.as_str()))
                    .map(|(_, v)| *v),
            );
            Some(HostRead {
                name,
                handlers,
                rest,
            })
        }
        Shape::Positional {
            name_arg,
            handler_arg,
            handler_table,
        } => {
            let name = argv
                .get(name_arg.checked_sub(1)?)
                .and_then(|a| string_literal(*a, src))?;
            let mut handlers = Vec::new();
            let mut rest = Vec::new();
            let handler_at = match handler_table {
                Some((arg, keys)) => {
                    if let Some(table) = argv.get(arg.checked_sub(1)?) {
                        let (fields, other) = table_fields(*table, src)?;
                        rest.extend(other);
                        for (k, v) in &fields {
                            match keys.iter().find(|key| **key == k.as_str()) {
                                Some(key) => handlers.push((Some(*key), *v)),
                                None => rest.push(*v),
                            }
                        }
                    }
                    arg
                }
                None => {
                    if let Some(h) = argv.get(handler_arg.checked_sub(1)?) {
                        handlers.push((None, *h));
                    }
                    handler_arg
                }
            };
            rest.extend(
                argv.iter()
                    .enumerate()
                    .filter(|(i, _)| *i + 1 != name_arg && *i + 1 != handler_at)
                    .map(|(_, a)| *a),
            );
            Some(HostRead {
                name,
                handlers,
                rest,
            })
        }
        _ => None,
    }
}

/// A table constructor's `key = value` fields with identifier keys, and
/// the values of its other fields; `None` when `node` is no table.
fn table_fields<'t>(node: TsNode<'t>, src: &[u8]) -> Option<TableFields<'t>> {
    if node.kind() != "table_constructor" {
        return None;
    }
    let mut cursor = node.walk();
    let mut keyed = Vec::new();
    let mut other = Vec::new();
    for field in node.named_children(&mut cursor) {
        let Some(value) = field.child_by_field_name("value") else {
            other.push(field);
            continue;
        };
        match field
            .child_by_field_name("name")
            .filter(|n| n.kind() == "identifier")
        {
            Some(key) => keyed.push((text(key, src).to_string(), value)),
            None => other.push(value),
        }
    }
    Some((keyed, other))
}

/// A host registration as read: its literal name, its handlers (each with
/// the table key it sat at) and the arguments that are neither.
struct HostRead<'t> {
    name: String,
    handlers: Vec<(Option<&'static str>, TsNode<'t>)>,
    rest: Vec<TsNode<'t>>,
}

/// A table constructor's identifier-keyed fields `(key, value)`, and the
/// values of its other fields.
type TableFields<'t> = (Vec<(String, TsNode<'t>)>, Vec<TsNode<'t>>);

/// A non-empty string literal's text.
fn string_literal(node: TsNode, src: &[u8]) -> Option<String> {
    (node.kind() == "string")
        .then(|| node.child_by_field_name("content"))
        .flatten()
        .map(|c| text(c, src).to_string())
        .filter(|s| !s.is_empty())
}

/// A busted test file: `*_spec.lua` / `*_test.lua` / `test_*.lua`, or any
/// `.lua` under a `spec/`, `test/` or `tests/` directory.
pub fn is_test_file(path: &str) -> bool {
    let path = path.replace('\\', "/");
    let file = path.rsplit('/').next().unwrap_or(&path);
    let stem = file.strip_suffix(".lua").unwrap_or(file);
    stem.ends_with("_spec")
        || stem.ends_with("_test")
        || stem.starts_with("test_")
        || path
            .split('/')
            .rev()
            .skip(1)
            .any(|d| matches!(d, "spec" | "test" | "tests"))
}

/// A plain name's key inside a function: `<enclosing key>.<name>`; at file
/// level the name itself.
fn nested_key(ctx: &Ctx, name: &str) -> String {
    match &ctx.key {
        Some(outer) => format!("{outer}.{name}"),
        None => name.to_string(),
    }
}

/// True for a statement directly in the file's chunk.
fn is_chunk_level(node: TsNode) -> bool {
    node.parent().is_some_and(|p| p.kind() == "chunk")
}

/// The interpreters a Lua script's `#!` line may name (GH.4).
const LUA_INTERPRETERS: &[&str] = &["lua", "luajit"];

/// True when the identifier `node` sits in no function (a chunk-level
/// statement, at any block depth) and no enclosing `for` declares its name.
fn global_at_chunk_level(node: TsNode, src: &[u8]) -> bool {
    let name = text(node, src);
    let mut at = node.parent();
    while let Some(n) = at {
        match n.kind() {
            "function_declaration" | "function_definition" => return false,
            "for_statement" if for_declares(n, name, src) => return false,
            _ => {}
        }
        at = n.parent();
    }
    true
}

/// True when the `for` statement's clause declares `name` as a loop variable.
fn for_declares(stmt: TsNode, name: &str, src: &[u8]) -> bool {
    let Some(clause) = stmt.child_by_field_name("clause") else {
        return false;
    };
    if clause
        .child_by_field_name("name")
        .is_some_and(|n| text(n, src) == name)
    {
        return true;
    }
    let mut cursor = clause.walk();
    let lists: Vec<TsNode> = clause
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "variable_list")
        .collect();
    lists.into_iter().any(|l| {
        let mut c = l.walk();
        let names: Vec<TsNode> = l.named_children(&mut c).collect();
        names
            .into_iter()
            .any(|n| n.kind() == "identifier" && text(n, src) == name)
    })
}

fn first_token_is(node: TsNode, token: &str) -> bool {
    node.child(0).is_some_and(|c| c.kind() == token)
}

/// A URL argument's text: a string literal, or a `..` concatenation that
/// starts with one, each non-literal operand read as `${…}` (the
/// interpolation marker the HTTP path normaliser collapses, as a TypeScript
/// template's): `"http://h/users/" .. id` -> `http://h/users/${…}`.
fn url_text(node: TsNode, src: &[u8]) -> Option<String> {
    // `..` is right-associative: flatten the chain, then read it left to right.
    fn operands<'t>(n: TsNode<'t>, src: &[u8], out: &mut Vec<TsNode<'t>>) {
        let concat = n.kind() == "binary_expression"
            && n.child_by_field_name("operator").map(|o| text(o, src)) == Some("..");
        match (
            concat,
            n.child_by_field_name("left"),
            n.child_by_field_name("right"),
        ) {
            (true, Some(l), Some(r)) => {
                operands(l, src, out);
                operands(r, src, out);
            }
            _ => out.push(n),
        }
    }
    let literal = |n: TsNode| {
        (n.kind() == "string")
            .then(|| n.child_by_field_name("content"))
            .flatten()
            .map(|c| text(c, src).to_string())
    };
    let mut parts = Vec::new();
    operands(node, src, &mut parts);
    let mut out = literal(*parts.first()?)?;
    for part in &parts[1..] {
        out.push_str(&literal(*part).unwrap_or_else(|| "${…}".to_string()));
    }
    Some(out)
}

/// The one string-literal argument of a call: `f("x")`, `f 'x'`, `f[[x]]`.
fn single_string(args: TsNode, src: &[u8]) -> Option<String> {
    let string = if args.kind() == "string" {
        args
    } else {
        let mut cursor = args.walk();
        let children: Vec<TsNode> = args.named_children(&mut cursor).collect();
        match children.as_slice() {
            [only] if only.kind() == "string" => *only,
            _ => return None,
        }
    };
    let content = string.child_by_field_name("content")?;
    Some(text(content, src).to_string())
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn row(node: TsNode) -> u32 {
    u32::try_from(node.start_position().row).unwrap_or(u32::MAX)
}

fn text<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

fn cells(node: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text(*node, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(format!(
                "{{\"file\":\"{}\",\"start_line\":{},\"end_line\":{}}}",
                file_rel.replace('\\', "\\\\").replace('"', "\\\""),
                node.start_position().row,
                node.end_position().row
            )),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> FileParse {
        parse_file(source, "lua/app/init.lua", "lua::app::init", RepoId(7)).unwrap()
    }

    fn keys(fp: &FileParse, kind: NodeKindId) -> Vec<String> {
        let mut out: Vec<String> = fp
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&kind))
            .map(|(_, q)| q.trim_start_matches("lua::app::init::").to_string())
            .collect();
        out.sort();
        out
    }

    fn calls(fp: &FileParse) -> Vec<String> {
        fp.calls
            .iter()
            .map(|c| match &c.qualifier {
                CallQualifier::Bare(n) => n.clone(),
                CallQualifier::Attribute { base, name } => format!("{base}:{name}"),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[test]
    fn every_function_form_mints_a_symbol() {
        let fp = parse(
            "local M = {}\n\
             local function helper() end\n\
             function global_fn() end\n\
             function M.setup() end\n\
             function M.a.b() end\n\
             function M:run() end\n\
             M.f = function() end\n\
             local g = function() end\n\
             h = function() end\n\
             local T = { k = function() end, sub = { deep = function() end } }\n\
             return { exported = function() end }\n",
        );
        assert_eq!(
            keys(&fp, node_kind::FUNCTION),
            [
                "M.a.b",
                "M.f",
                "M.setup",
                "T.k",
                "T.sub.deep",
                "exported",
                "g",
                "global_fn",
                "h",
                "helper"
            ]
        );
        assert_eq!(keys(&fp, node_kind::METHOD), ["M.run"]);
        // `function M:run()` makes M a class table.
        assert_eq!(keys(&fp, node_kind::CLASS), ["M"]);
        let defines = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::DEFINES)
            .count();
        assert_eq!(defines, 12);
    }

    #[test]
    fn nested_locals_get_their_enclosing_key_and_never_collide() {
        let fp = parse(
            "local M = {}\n\
             function M.a() local function step() end step() end\n\
             function M.b() local function step() end step() end\n\
             local cb = function() end\n\
             do local cb = function() end end\n",
        );
        assert_eq!(
            keys(&fp, node_kind::FUNCTION),
            ["M.a", "M.a.step", "M.b", "M.b.step", "cb", "cb@5"]
        );
        assert_eq!(calls(&fp), ["M.a.step", "M.b.step"]);
    }

    #[test]
    fn bare_calls_take_the_visible_local_then_a_file_global() {
        let fp = parse(
            "local function a() end\n\
             function run() a(); later(); print(1) end\n\
             function later() end\n",
        );
        assert_eq!(calls(&fp), ["a", "later", "print"]);
    }

    #[test]
    fn requires_are_imports_with_their_alias() {
        let fp = parse(
            "local util = require(\"a.b\")\n\
             require 'side.effect'\n\
             local x = require(name)\n\
             util.go()\n",
        );
        let imports: Vec<(String, Option<String>)> = fp
            .imports
            .iter()
            .map(|i| match &i.target {
                ImportTarget::Module { path, alias } => (path.clone(), alias.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            imports,
            [
                ("a.b".to_string(), Some("util".to_string())),
                ("side.effect".to_string(), None)
            ]
        );
        assert_eq!(calls(&fp), ["util:go"]);
    }

    #[test]
    fn busted_blocks_are_test_functions_only_in_test_files() {
        let src = "local calc = require(\"calc\")\n\
                   describe(\"calc\", function()\n\
                     before_each(function() calc.reset() end)\n\
                     it(\"adds\", function() calc.add(1, 2) end)\n\
                     describe(\"sub\", function() it(\"works\", function() calc.sub(2, 1) end) end)\n\
                   end)\n";
        let fp = parse_file(src, "spec/calc_spec.lua", "spec::calc_spec", RepoId(7)).unwrap();
        let mut tests: Vec<String> = fp
            .nav
            .qname_by_id
            .values()
            .filter_map(|q| q.strip_prefix("spec::calc_spec::").map(str::to_string))
            .collect();
        tests.sort();
        assert_eq!(tests, ["calc adds", "calc sub works"]);
        let owner = |name: &str| {
            let id = fp
                .calls
                .iter()
                .find(|c| matches!(&c.qualifier, CallQualifier::Attribute { name: n, .. } if n == name))
                .map(|c| c.from)
                .unwrap();
            fp.nav.qname_by_id.get(&id).cloned().unwrap()
        };
        assert_eq!(owner("add"), "spec::calc_spec::calc adds");
        assert_eq!(owner("sub"), "spec::calc_spec::calc sub works");
        assert_eq!(owner("reset"), "spec::calc_spec");
        // The same source in an app file mints nothing.
        let app = parse_file(src, "lua/calc_runner.lua", "lua::calc_runner", RepoId(7)).unwrap();
        assert_eq!(app.nav.qname_by_id.len(), 1);
        assert!(is_test_file("tests/util.lua") && is_test_file("calc_test.lua"));
        assert!(!is_test_file("lua/spec.lua") && !is_test_file("contest.lua"));
    }

    #[test]
    fn metatable_classes_inherit_and_type_their_instances() {
        let fp = parse(
            "local Animal = require(\"animal\")\n\
             local Dog = setmetatable({}, { __index = Animal })\n\
             Dog.__index = Dog\n\
             function Dog.new(name)\n\
               local self = setmetatable({}, Dog)\n\
               self:init(name)\n\
               return self\n\
             end\n\
             function Dog:bark() self:speak() end\n\
             local Cat = Animal:extend()\n\
             local Fox = class(\"Fox\", Dog)\n\
             local inst = setmetatable({}, { __index = Animal })\n\
             local d = Dog.new(\"rex\")\n\
             d:bark()\n",
        );
        assert_eq!(keys(&fp, node_kind::CLASS), ["Cat", "Dog", "Fox"]);
        let mut heritage: Vec<String> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::INHERITS_FROM)
            .map(|r| {
                let child = fp.nav.name_by_id.get(&r.from).cloned().unwrap_or_default();
                match &r.qualifier {
                    CallQualifier::Bare(b) => format!("{child}->{b}"),
                    other => format!("{other:?}"),
                }
            })
            .collect();
        heritage.sort();
        assert_eq!(heritage, ["Cat->Animal", "Dog->Animal", "Fox->Dog"]);
        // `self` in the constructor and `d` are typed Dog; inst is no class.
        let on_dog: Vec<String> = calls(&fp)
            .into_iter()
            .filter(|c| c.starts_with("Dog:"))
            .collect();
        assert_eq!(on_dog, ["Dog:init", "Dog:speak", "Dog:new", "Dog:bark"]);
    }

    #[test]
    fn lapis_routes_and_http_clients() {
        let fp = parse(
            "local lapis = require(\"lapis\")\n\
             local http = require(\"socket.http\")\n\
             local rhttp = require(\"resty.http\")\n\
             local app = lapis.Application()\n\
             local function show(self) return { json = {} } end\n\
             app:get(\"/users/:id\", show)\n\
             app:post(\"create_user\", \"/users\", function(self) audit() end)\n\
             app:match(\"/health\", function() end)\n\
             local function sync()\n\
               http.request(\"http://api.example.com/v1/orders\")\n\
               http.request(\"http://api.example.com/v1/orders\", \"body\")\n\
               http.request(\"http://api.example.com/v1/orders/\" .. id .. \"/items\")\n\
               http.request{ url = \"https://pay.example.com/charge\", method = \"PUT\" }\n\
               local httpc = rhttp.new()\n\
               httpc:request_uri(\"http://inv.example.com/stock\", { method = \"DELETE\" })\n\
             end\n",
        );
        let mut routes: Vec<String> = fp
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, q)| q.clone())
            .collect();
        routes.sort();
        assert_eq!(routes, ["ANY /health", "GET /users/:id", "POST /users"]);
        let handled: Vec<String> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::HANDLED_BY)
            .map(|r| format!("{:?}", r.qualifier))
            .collect();
        assert_eq!(
            handled,
            [
                "Bare(\"show\")",
                "Bare(\"POST /users\")",
                "Bare(\"ANY /health\")"
            ]
        );
        let mut endpoints: Vec<String> = fp
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ENDPOINT))
            .map(|(_, q)| q.clone())
            .collect();
        endpoints.sort();
        assert_eq!(
            endpoints,
            [
                "endpoint:DELETE:/stock",
                "endpoint:GET:/v1/orders",
                "endpoint:GET:/v1/orders/${…}/items",
                "endpoint:POST:/v1/orders",
                "endpoint:PUT:/charge"
            ]
        );
    }

    #[test]
    fn self_reads_as_the_methods_table_and_closures_attribute_to_their_owner() {
        let fp = parse(
            "local M = {}\n\
             function M:run() self:stop(); vim.schedule(function() self.helper() end) end\n",
        );
        assert_eq!(calls(&fp), ["M:stop", "vim:schedule", "M:helper"]);
        let run = fp
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.ends_with("::M.run"))
            .map(|(id, _)| *id)
            .unwrap();
        assert!(fp.calls.iter().all(|c| c.from == run));
    }

    /// `(command name, handler ref)` for every HANDLED_BY ref from a
    /// CLI_COMMAND, in emission order.
    fn host_commands(fp: &FileParse) -> Vec<(String, String)> {
        fp.refs
            .iter()
            .filter(|r| r.category == edge_category::HANDLED_BY)
            .filter(|r| fp.nav.kind_by_id.get(&r.from) == Some(&node_kind::CLI_COMMAND))
            .map(|r| {
                (
                    fp.nav.qname_by_id[&r.from].clone(),
                    format!("{:?}", r.qualifier),
                )
            })
            .collect()
    }

    fn commands(fp: &FileParse) -> Vec<String> {
        let mut out: Vec<String> = fp
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::CLI_COMMAND))
            .map(|(_, q)| q.clone())
            .collect();
        out.sort();
        out
    }

    #[test]
    fn neovim_user_commands_are_host_commands() {
        let fp = parse(
            "local function greet(opts)\n  print(\"hi \" .. opts.args)\nend\n\
             vim.api.nvim_create_user_command(\"Greet\", greet, { nargs = 1 })\n\
             vim.api.nvim_create_user_command(\"GreetAll\", function()\n  greet({ args = \"all\" })\nend, {})\n",
        );
        assert_eq!(
            commands(&fp),
            ["host_cmd:neovim:Greet", "host_cmd:neovim:GreetAll"]
        );
        assert_eq!(
            host_commands(&fp),
            [
                (
                    "host_cmd:neovim:Greet".to_string(),
                    "Bare(\"greet\")".to_string()
                ),
                (
                    "host_cmd:neovim:GreetAll".to_string(),
                    "Bare(\"command GreetAll\")".to_string()
                ),
            ]
        );
        // The anonymous handler is its own FUNCTION and owns its calls.
        assert_eq!(
            keys(&fp, node_kind::FUNCTION),
            ["command GreetAll", "greet"]
        );
        let handler = fp
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.ends_with("::command GreetAll"))
            .map(|(id, _)| *id)
            .unwrap();
        let greet_calls: Vec<_> = fp
            .calls
            .iter()
            .filter(|c| matches!(&c.qualifier, CallQualifier::Bare(n) if n == "greet"))
            .collect();
        assert_eq!(greet_calls.len(), 1);
        assert_eq!(greet_calls[0].from, handler);
        // The command is the host's, located at its registration, parentless.
        let greet = fp
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.as_str() == "host_cmd:neovim:Greet")
            .map(|(id, _)| *id)
            .unwrap();
        assert!(!fp.nav.parent_of.contains_key(&greet));
        let node = fp.nodes.iter().find(|n| n.id == greet).unwrap();
        assert!(node.cells.iter().any(|c| c.kind == cell_type::POSITION
            && matches!(&c.payload, CellPayload::Json(j) if j.contains("\"start_line\":3"))));
        // An option key is no command.
        assert!(!commands(&fp).iter().any(|c| c.contains("nargs")));
    }

    #[test]
    fn maki_tools_and_commands_mint_other_registrations_are_aux() {
        let fp = parse(
            "local function summarize_text(s) return s end\n\
             maki.api.register_tool({\n  name = \"summarize\",\n  execute = function(args)\n    return summarize_text(args.text)\n  end,\n  restore = function() end,\n})\n\
             maki.api.register_command{ name = \"clear\", execute = summarize_text }\n\
             maki.api.register_hook({ name = \"after\", execute = function() summarize_text(1) end })\n",
        );
        assert_eq!(
            commands(&fp),
            ["host_cmd:maki:clear", "host_cmd:maki:summarize"]
        );
        assert_eq!(
            host_commands(&fp),
            [
                (
                    "host_cmd:maki:summarize".to_string(),
                    "Bare(\"command summarize\")".to_string()
                ),
                (
                    "host_cmd:maki:summarize".to_string(),
                    "Bare(\"command summarize restore\")".to_string()
                ),
                (
                    "host_cmd:maki:clear".to_string(),
                    "Bare(\"summarize_text\")".to_string()
                ),
            ]
        );
        assert_eq!(
            keys(&fp, node_kind::FUNCTION),
            [
                "command summarize",
                "command summarize restore",
                "summarize_text"
            ]
        );
        // The aux hook mints nothing; its closure's call stays the module's.
        assert!(!commands(&fp).iter().any(|c| c.contains("after")));
        let module = fp.nodes[0].id;
        assert!(fp.calls.iter().any(|c| c.from == module
            && matches!(&c.qualifier, CallQualifier::Bare(n) if n == "summarize_text")));
    }

    #[test]
    fn host_globals_read_through_aliases_and_never_through_shadows() {
        let fp = parse(
            "local api = vim.api\n\
             local v = vim\n\
             api.nvim_create_user_command(\"A\", \":echo a\", {})\n\
             v.api.nvim_buf_create_user_command(0, \"B\", function() end, {})\n\
             local function setup(vim)\n  vim.api.nvim_create_user_command(\"Shadowed\", function() end, {})\nend\n\
             do\n  local vim = { api = {} }\n  vim.api.nvim_create_user_command(\"Local\", function() end, {})\nend\n\
             local name = \"C\"\n\
             vim.api.nvim_create_user_command(name, function() end, {})\n\
             vim.keymap.set(\"n\", \"<leader>g\", function() end)\n",
        );
        assert_eq!(commands(&fp), ["host_cmd:neovim:A", "host_cmd:neovim:B"]);
        // A string command is no handler; the anonymous one is minted.
        assert_eq!(
            host_commands(&fp),
            [(
                "host_cmd:neovim:B".to_string(),
                "Bare(\"command B\")".to_string()
            )]
        );
    }

    #[test]
    fn a_positional_handler_table_reads_its_keys_and_leaves_the_rest() {
        // No 0.5.6 Lua row uses `handler_table` yet (Neovim autocmds will):
        // read a call against the shape directly.
        let src =
            "f(\"BufWritePost\", { pattern = \"*.lua\", callback = function() end, desc = g() })";
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_lua::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(src, None).unwrap();
        let mut call = tree.root_node();
        while call.kind() != "function_call" {
            call = call.named_child(0).unwrap();
        }
        let args = call.child_by_field_name("arguments").unwrap();
        let shape = Shape::Positional {
            name_arg: 1,
            handler_arg: 2,
            handler_table: Some((2, &["callback"])),
        };
        let read = read_registration(args, shape, src.as_bytes()).unwrap();
        assert_eq!(read.name, "BufWritePost");
        let handlers: Vec<(Option<&str>, &str)> =
            read.handlers.iter().map(|(k, v)| (*k, v.kind())).collect();
        assert_eq!(handlers, [(Some("callback"), "function_definition")]);
        let mut rest: Vec<&str> = read.rest.iter().map(|n| n.kind()).collect();
        rest.sort_unstable();
        assert_eq!(rest, ["function_call", "string"]);
        // A computed name is unread.
        let shape_name_2 = Shape::Positional {
            name_arg: 2,
            handler_arg: 1,
            handler_table: None,
        };
        assert!(read_registration(args, shape_name_2, src.as_bytes()).is_none());
        // A table shape needs exactly one table argument.
        let table = Shape::Table {
            name_key: "pattern",
            handler_keys: &["callback"],
        };
        assert!(read_registration(args, table, src.as_bytes()).is_none());
    }

    /// `(qname, edge, from)` of every DEFINES_CONFIG / READS_CONFIG edge, in
    /// emission order; `from` is the MODULE (`<module>`) or a function key.
    fn config_edges(fp: &FileParse) -> Vec<(String, &'static str, String)> {
        fp.edges
            .iter()
            .filter(|e| {
                e.category == edge_category::DEFINES_CONFIG
                    || e.category == edge_category::READS_CONFIG
            })
            .map(|e| {
                let side = if e.category == edge_category::DEFINES_CONFIG {
                    "defines"
                } else {
                    "reads"
                };
                let from = fp.nav.qname_by_id[&e.from].as_str();
                let from = if from == "lua::app::init" {
                    "<module>".to_string()
                } else {
                    from.trim_start_matches("lua::app::init::").to_string()
                };
                (fp.nav.qname_by_id[&e.to].clone(), side, from)
            })
            .collect()
    }

    fn defined(fp: &FileParse) -> Vec<String> {
        config_edges(fp)
            .into_iter()
            .filter(|(_, side, _)| *side == "defines")
            .map(|(q, _, _)| q)
            .collect()
    }

    #[test]
    fn wezterm_builder_keys_are_host_settings_and_values_are_not() {
        let fp = parse(
            "local wezterm = require(\"wezterm\")\n\
             local config = wezterm.config_builder()\n\
             config.font_size = 12\n\
             config.color_scheme = \"Gruvbox Dark\"\n\
             config.font = wezterm.font(\"JetBrains Mono\")\n\
             config.colors.background = \"#000\"\n\
             config.font_size = 13\n\
             return config\n",
        );
        assert_eq!(
            keys(&fp, node_kind::CONFIG_KEY),
            [
                "config:wezterm:color_scheme",
                "config:wezterm:font",
                "config:wezterm:font_size"
            ]
        );
        // One edge per (key, side), from the MODULE, in source order.
        assert_eq!(
            defined(&fp),
            [
                "config:wezterm:font_size",
                "config:wezterm:color_scheme",
                "config:wezterm:font"
            ]
        );
        assert!(config_edges(&fp).iter().all(|(_, _, f)| f == "<module>"));
        // The key node is named by its key, located at its first site.
        let id = fp
            .nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.as_str() == "config:wezterm:font_size")
            .map(|(id, _)| *id)
            .unwrap();
        assert_eq!(fp.nav.name_by_id[&id], "font_size");
        let node = fp.nodes.iter().find(|n| n.id == id).unwrap();
        assert!(matches!(
            &node.cells[..],
            [c] if c.kind == cell_type::POSITION
                && matches!(&c.payload, CellPayload::Json(j) if j.contains("\"start_line\":2"))
        ));
        assert_eq!(fp.calls.len(), 2, "the builder and wezterm.font still call");
    }

    #[test]
    fn a_returned_table_is_the_wezterm_config_only_when_the_file_requires_it() {
        let literal = parse(
            "local wezterm = require 'wezterm'\n\
             return {\n  font_size = 11,\n  keys = {},\n  on_load = function() end,\n}\n",
        );
        assert_eq!(
            defined(&literal),
            ["config:wezterm:font_size", "config:wezterm:keys"]
        );
        // The idiom: a table, upgraded to the builder when there is one.
        let local = parse(
            "local wezterm = require(\"wezterm\")\n\
             local config = { hide_tab_bar_if_only_one_tab = true }\n\
             if wezterm.config_builder then\n  config = wezterm.config_builder()\nend\n\
             config.font_size = 12\n\
             return config\n",
        );
        assert_eq!(
            defined(&local),
            [
                "config:wezterm:hide_tab_bar_if_only_one_tab",
                "config:wezterm:font_size"
            ]
        );
        // No wezterm require: an ordinary module table.
        let plain = parse("local M = {}\nM.size = 3\nreturn { a = 1 }\n");
        assert!(config_edges(&plain).is_empty());
        assert!(keys(&plain, node_kind::CONFIG_KEY).is_empty());
        // A returned local that is not the table, or a non-chunk return.
        let other = parse(
            "local wezterm = require(\"wezterm\")\n\
             local M = {}\nM.x = 1\n\
             local function f() local t = {}\n t.y = 2\n return t end\n\
             return f()\n",
        );
        assert!(config_edges(&other).is_empty());
        // `wezterm` as a bare global (no require) roots nothing.
        let global = parse("local c = wezterm.config_builder()\nc.font_size = 1\n");
        assert!(config_edges(&global).is_empty());
    }

    #[test]
    fn neovim_options_and_globals_are_host_settings_read_where_read() {
        let fp = parse(
            "vim.opt.number = true\n\
             vim.o.tabstop = 4\n\
             vim.g.mapleader = \" \"\n\
             vim.opt.shortmess:append(\"c\")\n\
             local opt = vim.opt_local\n\
             opt.spell = true\n\
             vim.bo[0].filetype = \"lua\"\n\
             local function toggle()\n  if vim.o.background == \"dark\" then end\n  local w = vim.opt.wrap:get()\nend\n",
        );
        assert_eq!(
            config_edges(&fp),
            [
                (
                    "config:neovim:number".to_string(),
                    "defines",
                    "<module>".to_string()
                ),
                (
                    "config:neovim:tabstop".to_string(),
                    "defines",
                    "<module>".to_string()
                ),
                (
                    "config:neovim:g.mapleader".to_string(),
                    "defines",
                    "<module>".to_string()
                ),
                (
                    "config:neovim:shortmess".to_string(),
                    "defines",
                    "<module>".to_string()
                ),
                (
                    "config:neovim:spell".to_string(),
                    "defines",
                    "<module>".to_string()
                ),
                (
                    "config:neovim:background".to_string(),
                    "reads",
                    "toggle".to_string()
                ),
                (
                    "config:neovim:wrap".to_string(),
                    "reads",
                    "toggle".to_string()
                ),
            ]
        );
        // A local named `vim` shadows the host.
        let shadow = parse("local vim = {}\nvim.opt = {}\nvim.opt.number = true\n");
        assert!(config_edges(&shadow).is_empty());
    }

    /// `(key, source, decl)` of every node carrying an ENTRYPOINT cell, the
    /// MODULE as `<module>`, in node order.
    fn entries(fp: &FileParse) -> Vec<(String, String, String)> {
        let module = fp.nodes[0].id;
        fp.nodes
            .iter()
            .flat_map(|n| {
                n.cells
                    .iter()
                    .filter(|c| c.kind == cell_type::ENTRYPOINT)
                    .map(move |c| (n.id, c))
            })
            .map(|(id, c)| {
                let CellPayload::Json(p) = &c.payload else {
                    panic!("ENTRYPOINT is JSON");
                };
                // hosts::entry_cell's fixed shape.
                let (decl, source) = p
                    .strip_prefix("{\"decl\":\"")
                    .and_then(|r| r.strip_suffix("\"}"))
                    .and_then(|r| r.split_once("\",\"source\":\""))
                    .unwrap_or_else(|| panic!("entry cell shape: {p}"));
                let key = if id == module {
                    "<module>".to_string()
                } else {
                    fp.nav.name_by_id[&id].clone()
                };
                (key, source.to_string(), decl.to_string())
            })
            .collect()
    }

    fn parse_at(source: &str, path: &str) -> FileParse {
        let module = path.trim_end_matches(".lua").replace('/', "::");
        parse_file(source, path, &module, RepoId(7)).unwrap()
    }

    fn e(key: &str, source: &str, decl: &str) -> (String, String, String) {
        (key.to_string(), source.to_string(), decl.to_string())
    }

    #[test]
    fn love_callbacks_are_entries_and_main_lua_is_the_game() {
        let src = "local function step(dt) end\n\
                   function love.load()\nend\n\
                   love.update = function(dt)\n  step(dt)\nend\n\
                   function love.draw()\n  love.graphics.print(\"x\", 1, 1)\nend\n\
                   function love.helper()\nend\n";
        let fp = parse_at(src, "main.lua");
        assert_eq!(
            entries(&fp),
            [
                e("<module>", "host:love", "main.lua:1"),
                e("love.load", "host:love", "main.lua:2"),
                e("love.update", "host:love", "main.lua:4"),
                e("love.draw", "host:love", "main.lua:7"),
            ]
        );
        // The callback's body still calls.
        let update = fp
            .nav
            .name_by_id
            .iter()
            .find(|(_, n)| n.as_str() == "love.update")
            .map(|(id, _)| *id);
        assert!(
            fp.calls.iter().any(
                |c| Some(c.from) == update && c.qualifier == CallQualifier::Bare("step".into())
            ),
            "{:?}",
            calls(&fp)
        );
        // Anywhere else in the tree the callbacks are still LOVE's; the file
        // is no plugin unit.
        let nested = parse_at(src, "src/game.lua");
        assert_eq!(entries(&nested).len(), 3);
        assert!(entries(&nested).iter().all(|(k, _, _)| k != "<module>"));
        // A local `love` is the file's own table: no callback, and a root
        // main.lua that never shows the host is no LOVE game.
        let shadow = parse_at(
            "local love = {}\nfunction love.update() end\nlove.draw = function() end\n",
            "main.lua",
        );
        assert!(entries(&shadow).is_empty(), "{:?}", entries(&shadow));
        // An alias of the global reads through to it.
        let alias = parse_at("local l = love\nfunction l.draw() end\n", "src/a.lua");
        assert_eq!(entries(&alias), [e("l.draw", "host:love", "src/a.lua:2")]);
    }

    #[test]
    fn plugin_units_are_module_entries() {
        let plain = "local M = {}\nreturn M\n";
        for path in [
            "plugin/greet.lua",
            "ftplugin/rust.lua",
            "after/plugin/x.lua",
        ] {
            assert_eq!(
                entries(&parse_at(plain, path)),
                [e("<module>", "host:neovim", &format!("{path}:1"))],
                "{path}"
            );
        }
        // A root init.lua is Neovim's only when it reads `vim`.
        assert!(entries(&parse_at(plain, "init.lua")).is_empty());
        assert_eq!(
            entries(&parse_at("vim.keymap.set('n', 'x', 'y')\n", "init.lua")),
            [e("<module>", "host:neovim", "init.lua:1")]
        );
        assert!(entries(&parse_at("local vim = {}\nvim.x()\n", "init.lua")).is_empty());
        // WezTerm's config file, when it requires wezterm.
        assert_eq!(
            entries(&parse_at(
                "local wezterm = require 'wezterm'\nreturn {}\n",
                ".wezterm.lua"
            )),
            [e("<module>", "host:wezterm", ".wezterm.lua:1")]
        );
        assert!(entries(&parse_at(plain, "wezterm.lua")).is_empty());
        // Not a load path.
        assert!(entries(&parse_at(plain, "lua/greet/init.lua")).is_empty());
    }

    #[test]
    fn scripts_are_entries_by_shebang_or_a_chunk_level_arg_read() {
        let fp = parse_at(
            "#!/usr/bin/env lua\nlocal path = arg[1]\nprint(path)\n",
            "tools/count_lines.lua",
        );
        assert_eq!(
            entries(&fp),
            [e("<module>", "shebang", "tools/count_lines.lua:1")]
        );
        let luajit = parse_at("#!/usr/bin/luajit\nprint(1)\n", "bin/x.lua");
        assert_eq!(entries(&luajit), [e("<module>", "shebang", "bin/x.lua:1")]);
        // No shebang: a chunk-level read of the global `arg`, at its row.
        let arg = parse_at(
            "local M = {}\nif #arg > 0 then\n  print(arg[1])\nend\n",
            "tools/run.lua",
        );
        assert_eq!(
            entries(&arg),
            [e("<module>", "script_arg", "tools/run.lua:2")]
        );
        // Not script entries: `arg` inside a function, a local `arg`, a
        // loop variable `arg`, a field named arg, another interpreter.
        for src in [
            "local function f(...)\n  return arg\nend\n",
            "local arg = {...}\nprint(arg[1])\n",
            "for _, arg in ipairs(t) do print(arg) end\n",
            "local t = {}\nprint(t.arg)\nlocal u = { arg = 1 }\n",
            "#!/bin/sh\nprint(1)\n",
            "local f = function() return arg end\n",
        ] {
            assert!(entries(&parse_at(src, "tools/x.lua")).is_empty(), "{src}");
        }
    }
}
