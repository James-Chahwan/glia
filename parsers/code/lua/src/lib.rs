//! The Lua parser (L1, lua-support).
//!
//! Extracts what the AST makes available, nothing more (the spec's
//! non-goals: metatables / `setmetatable` OOP, `pcall` indirection, `coroutine`
//! flows, string-keyed dynamic dispatch, `dofile` / `load`, Neovim's `vim.*`,
//! LuaJIT FFI):
//!
//! | Lua source | Extracted |
//! |---|---|
//! | `local function f()` / `function f()` | `FUNCTION` node, qname `<module>::f` |
//! | `function T.f()` (a dotted name, any depth) | `FUNCTION` node, qname `<module>::T.f` — a table field, modelled flat |
//! | `function T:m()` (colon) | `METHOD` node, qname `<module>::T.m`; the implicit `self` is not a node |
//! | `local T = {}` + field assigns | nothing extra |
//! | `require("x")` / `require("a.b")`, string-literal argument | `UnresolvedRef` `IMPORTS` ref, spec text verbatim (a non-literal argument is counted, never guessed) |
//! | `f(...)` | `UnresolvedRef` `CALLS` ref, `Bare` — resolved by `build_lua` in-file, else left bare |
//! | `T.f(...)`, `T:m(...)`, `alias.f(...)` | `UnresolvedRef` `CALLS` ref, `Attribute` |
//! | `maki.api.register_tool{ name = "x", execute = function() .. end, .. }` — any call whose dotted callee starts a shipped host row's call family | one `PLUGIN_TOOL` node + one `FUNCTION` per handler closure + `HANDLED_BY` edges (below); the family's shape picks the reader (table-keyed or positional); auxiliary registrations (the family's `tools` gate the mint) mint nothing, count `aux=`, and their arguments still walk. A call matching NO family but carrying a known registration shape is recorded as a detection candidate (L4) |
//!
//! Every edge carries exactly one EVIDENCE cell; the parser-emitted `DEFINES`
//! edges carry none (the engine's fill pass places them from their endpoints,
//! the terraform-parser convention), the `HANDLED_BY` edges carry their
//! registration site's own.
//!
//! Host-genericity: configured call families are matched first; structurally
//! detected calls outside those families remain ordinary call refs and are
//! recorded for the detection report.
//!
//! fired_on, per parsed file:
//! `[lua] functions=<f> methods=<m> requires=<r> registrations=<g> aux=<x> unnamed=<u> cands=<c> file=<p>`.

use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, NavFact,
    ParseError, UnresolvedRef, cell_type, edge_category, evidence, node_kind, plugin_families,
};

/// The qname owner segment of a registration: the basename of the registering
/// file's directory (`plugins/question/init.lua` -> `question`). A file with
/// no directory (a root-level `init.lua`) registers under its own stem, so
/// the qname stays deterministic; the plugin manifest's directory is what
/// L3's `same_dir` rule pairs, and a root-level registration pairs nothing.
fn plugin_dir_of(file_rel_path: &str) -> String {
    match file_rel_path.rsplit_once('/') {
        Some((dir, _)) => dir.rsplit('/').next().unwrap_or(dir).to_string(),
        None => file_rel_path
            .rsplit_once('.')
            .map(|(stem, _)| stem.to_string())
            .unwrap_or_else(|| file_rel_path.to_string()),
    }
}

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
    families: &plugin_families::PluginFamilies,
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
        families,
        nodes: Vec::new(),
        edges: Vec::new(),
        refs: Vec::new(),
        nav: CodeNav::default(),
        counts: Counts::default(),
    };

    w.nodes.push(Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: file_cells(&root, src, file_rel_path),
    });
    let module_simple = module_qname.rsplit("::").next().unwrap_or(module_qname);
    w.nav.record(
        module_id,
        module_simple,
        module_qname,
        node_kind::MODULE,
        None,
    );

    w.visit(root, module_id);

    eprintln!(
        "[lua] functions={} methods={} requires={} registrations={} aux={} unnamed={} cands={} file={file_rel_path}",
        w.counts.functions,
        w.counts.methods,
        w.counts.requires,
        w.counts.registrations,
        w.counts.aux,
        w.counts.unnamed,
        w.counts.cands,
    );

    Ok(FileParse {
        nodes: w.nodes,
        edges: w.edges,
        imports: Vec::new(),
        calls: Vec::new(),
        refs: w.refs,
        nav: w.nav,
        properties: Default::default(),
    })
}

#[derive(Default)]
struct Counts {
    functions: usize,
    methods: usize,
    requires: usize,
    registrations: usize,
    aux: usize,
    unnamed: usize,
    cands: usize,
}

struct Walk<'a> {
    src: &'a [u8],
    file_rel_path: &'a str,
    module_qname: &'a str,
    module_id: NodeId,
    repo: RepoId,
    families: &'a plugin_families::PluginFamilies,
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    refs: Vec<UnresolvedRef>,
    nav: CodeNav,
    counts: Counts,
}

impl<'a> Walk<'a> {
    /// Walk one node. `owner` is the node a call site inside this subtree is
    /// attributed to: the file MODULE at top level, the enclosing FUNCTION /
    /// METHOD inside one, a registration's handler closure inside one.
    fn visit(&mut self, node: TsNode<'a>, owner: NodeId) {
        match node.kind() {
            "function_declaration" => {
                let Some(id) = self.mint_function(node) else {
                    return;
                };
                // The body's calls attribute to the new symbol.
                self.visit_children(node, id);
            }
            "function_call" => {
                if !self.handle_call(node, owner) {
                    self.visit_children(node, owner);
                }
            }
            "assignment_statement" => {
                self.record_require_alias(node);
                self.visit_children(node, owner);
            }
            _ => self.visit_children(node, owner),
        }
    }

    /// Every named child, in source order.
    fn visit_children(&mut self, node: TsNode<'a>, owner: NodeId) {
        let mut cursor = node.walk();
        let children: Vec<TsNode> = node.named_children(&mut cursor).collect();
        for child in children {
            self.visit(child, owner);
        }
    }

    /// One named function definition: `function f()`, `local function f()`,
    /// `function T.f()`, `function A.B:m()`. The dotted name is modelled flat
    /// (a table field, no table node); the METHOD kind is the colon form.
    /// Returns the minted node's id, or `None` for a dynamic name
    /// (`function (x).y()`), which mints nothing.
    fn mint_function(&mut self, node: TsNode<'a>) -> Option<NodeId> {
        let name_node = node.child_by_field_name("name")?;
        let dotted = match name_node.kind() {
            "identifier" | "dot_index_expression" => dotted_name(name_node, self.src)?,
            "method_index_expression" => {
                let table = name_node.child_by_field_name("table")?;
                let method = name_node.child_by_field_name("method")?;
                format!(
                    "{}.{}",
                    dotted_name(table, self.src)?,
                    text_of(method, self.src)
                )
            }
            _ => return None,
        };
        let kind = if name_node.kind() == "method_index_expression" {
            self.counts.methods += 1;
            node_kind::METHOD
        } else {
            self.counts.functions += 1;
            node_kind::FUNCTION
        };
        let qname = format!("{}::{}", self.module_qname, dotted);
        let id = NodeId::from_parts(GRAPH_TYPE, self.repo, kind, &qname);
        self.nodes.push(Node {
            id,
            repo: self.repo,
            confidence: Confidence::Strong,
            cells: entity_cells(&node, self.src, self.file_rel_path),
        });
        self.edges.push(Edge::new(
            self.module_id,
            id,
            edge_category::DEFINES,
            Confidence::Strong,
        ));
        self.nav
            .record(id, &dotted, &qname, kind, Some(self.module_id));
        Some(id)
    }

    /// One call expression. `true` when the node is fully handled (a require,
    /// or a registration whose fields were walked here); `false` when the
    /// generic descent should still walk its children (an ordinary call, whose
    /// arguments may hold calls of their own).
    fn handle_call(&mut self, node: TsNode<'a>, owner: NodeId) -> bool {
        let Some(callee) = node.child_by_field_name("name") else {
            return false;
        };
        let args = node.child_by_field_name("arguments");
        let line = line_at(node);

        // require("spec") — the spec verbatim, IMPORTS ref (spec text as the
        // Bare qualifier). A dotted spec stays dotted ("a.b"); the graph's
        // require resolution maps it to a path. A non-literal argument is
        // counted and never guessed.
        if callee.kind() == "identifier" && text_of(callee, self.src) == "require" {
            self.counts.requires += 1;
            if let Some(spec) = args.and_then(|a| single_string_argument(a, self.src))
                && !spec.is_empty()
            {
                self.refs.push(UnresolvedRef {
                    from: owner,
                    from_module: self.module_id,
                    qualifier: CallQualifier::Bare(spec),
                    category: edge_category::IMPORTS,
                    line,
                });
                return true;
            }
            return false;
        }

        // A registration: the dotted callee starts a shipped host row's call
        // family. The host rows are data (the table), so the callee text is
        // only ever compared against them. The family's shape picks the
        // reader; whether the registration mints a PLUGIN_TOOL is the
        // family's `tools`. A call matching NO family still gets classified
        // structurally (L4): a call of a known registration shape is a
        // detection candidate whoever owns the API.
        if let Some(dotted) = callee_dotted(callee, self.src) {
            if let Some((host, family)) = self.families.call_for(&dotted) {
                if plugin_families::mints_tool(&dotted, host) {
                    match family.shape {
                        plugin_families::Shape::Table => {
                            self.mint_registration(node, &host.name, owner)
                        }
                        plugin_families::Shape::Positional { .. } => {
                            self.mint_registration_positional(node, &host.name, family, owner);
                        }
                    }
                } else {
                    self.counts.aux += 1;
                    self.walk_registration_args(node, owner);
                }
                return true;
            }
            // A detection candidate (L4) is recorded and still binds as the
            // ordinary call it is: the fact mints nothing, the CALLS ref is
            // the graph's structural truth.
            self.record_registration_candidate(node, &dotted);
        }

        // An ordinary call: bare or qualified, as a CALLS ref the graph binds
        // file-locally. A call-result receiver (`f().g()`) or parenthesised
        // callee (`(f)()`) is dynamic: no ref, its inner calls still walk.
        if let Some(qualifier) = callee_qualifier(callee, self.src) {
            self.refs.push(UnresolvedRef {
                from: owner,
                from_module: self.module_id,
                qualifier,
                category: edge_category::CALLS,
                line,
            });
        }
        false
    }

    /// A table-shape registration call (the family's shape is
    /// `plugin_families::Shape::Table`, e.g. `maki.api.register_tool`): the
    /// first table-constructor argument's `name` string field names the
    /// registration; every field whose value is an anonymous
    /// `function_definition` is a handler closure. Mints the PLUGIN_TOOL,
    /// one FUNCTION per named handler, `DEFINES` from the module, and
    /// `HANDLED_BY` PLUGIN_TOOL -> handler at the call's row. A call with no
    /// `name` string field mints nothing (counted `unnamed=`); its table
    /// fields still walk. Auxiliary registrations never reach here (`aux=`
    /// in `handle_call`).
    fn mint_registration(&mut self, call: TsNode<'a>, host_name: &str, owner: NodeId) {
        let Some(table) = call
            .child_by_field_name("arguments")
            .and_then(|a| table_argument(a))
        else {
            self.counts.unnamed += 1;
            return;
        };
        let tool_name = table
            .named_children(&mut table.walk())
            .find_map(|field| named_string_field(field, "name", self.src));
        let Some(tool_name) = tool_name else {
            // A tool-kind registration with no literal name (often a dynamic
            // `name = spec.name`): mints nothing, counted `unnamed=`; its
            // table fields still walk, attributed to the call site's owner.
            self.counts.unnamed += 1;
            self.walk_registration_args(call, owner);
            return;
        };
        self.counts.registrations += 1;
        let tool_id = self.mint_tool(&tool_name, host_name, call);

        let line = line_at(call);
        let mut cursor = table.walk();
        let fields: Vec<TsNode> = table.named_children(&mut cursor).collect();
        for field in fields {
            let Some(value) = field.child_by_field_name("value") else {
                continue;
            };
            if value.kind() != "function_definition" {
                // A non-handler field's expressions still walk (calls inside
                // a table value attribute to the enclosing owner).
                self.visit(value, owner);
                continue;
            }
            let Some(key) = field
                .child_by_field_name("name")
                .and_then(|n| (n.kind() == "identifier").then(|| text_of(n, self.src).to_string()))
            else {
                // A keyless anonymous handler: nothing names it, so it mints
                // no node; its calls attribute to the enclosing owner.
                self.visit_children(value, owner);
                continue;
            };
            self.mint_handler(tool_id, &format!("{tool_name}.{key}"), value, line);
        }
    }

    /// A positional-shape registration call (the family's shape is
    /// `plugin_families::Shape::Positional`, the neovim / factorio / wezterm
    /// family): the name is the string literal at the family's `name_arg`
    /// position (a dynamic value there leaves the registration unnamed,
    /// counted `unnamed=`), handlers are the anonymous functions at the
    /// family's `handler_args` positions named by the family's
    /// `handler_name`, and a `handler_table` argument's named keys carry
    /// handlers under exactly those keys. Everything else walks with the
    /// owner, like the table reader's non-handler fields.
    fn mint_registration_positional(
        &mut self,
        call: TsNode<'a>,
        host_name: &str,
        family: &plugin_families::RegistrationCall,
        owner: NodeId,
    ) {
        let plugin_families::Shape::Positional {
            name_arg,
            handler_args,
            handler_table,
        } = &family.shape
        else {
            return;
        };
        let Some(args) = call.child_by_field_name("arguments") else {
            self.counts.unnamed += 1;
            return;
        };
        let argv: Vec<TsNode> = args.named_children(&mut args.walk()).collect();
        let tool_name = (*name_arg != 0)
            .then(|| argv.get(*name_arg as usize - 1))
            .flatten()
            .filter(|n| n.kind() == "string")
            .and_then(|n| string_content(*n, self.src))
            .filter(|n| !n.is_empty());
        let Some(tool_name) = tool_name else {
            self.counts.unnamed += 1;
            self.visit(args, owner);
            return;
        };
        self.counts.registrations += 1;
        let tool_id = self.mint_tool(&tool_name, host_name, call);
        let line = line_at(call);
        let handler_member: &str = if family.handler_name.is_empty() {
            "run"
        } else {
            &family.handler_name
        };
        for index in handler_args {
            let Some(value) = argv.get(*index as usize - 1) else {
                continue;
            };
            if value.kind() == "function_definition" {
                self.mint_handler(
                    tool_id,
                    &format!("{tool_name}.{handler_member}"),
                    *value,
                    line,
                );
            } else {
                self.visit(*value, owner);
            }
        }
        if let Some((table_index, keys)) = handler_table {
            let Some(table) = argv
                .get(*table_index as usize - 1)
                .filter(|n| n.kind() == "table_constructor")
                .copied()
            else {
                return;
            };
            let mut cursor = table.walk();
            let fields: Vec<TsNode> = table.named_children(&mut cursor).collect();
            for field in fields {
                let Some(value) = field.child_by_field_name("value") else {
                    continue;
                };
                let keyed = field.child_by_field_name("name").is_some_and(|k| {
                    k.kind() == "identifier"
                        && keys.iter().any(|key| key.as_str() == text_of(k, self.src))
                });
                if keyed && value.kind() == "function_definition" {
                    let key = text_of(field.child_by_field_name("name").unwrap(), self.src);
                    self.mint_handler(tool_id, &format!("{tool_name}.{key}"), value, line);
                } else {
                    self.visit(value, owner);
                }
            }
        }
    }

    /// The PLUGIN_TOOL node itself: the node, its `DEFINES` edge from the
    /// module, and its nav record. Both shape readers share it.
    fn mint_tool(&mut self, tool_name: &str, host_name: &str, call: TsNode<'a>) -> NodeId {
        let dir = plugin_dir_of(self.file_rel_path);
        let qname = format!("plugin_tool:{} @{}/{}", tool_name, host_name, dir);
        let tool_id = NodeId::from_parts(GRAPH_TYPE, self.repo, node_kind::PLUGIN_TOOL, &qname);
        self.nodes.push(Node {
            id: tool_id,
            repo: self.repo,
            confidence: Confidence::Strong,
            cells: entity_cells(&call, self.src, self.file_rel_path),
        });
        // No nav parent: a registration is not a Lua symbol, so it must not
        // enter the module's symbol table under the tool's bare name. The
        // module's DEFINES edge is its structural anchor.
        self.nav
            .record(tool_id, tool_name, &qname, node_kind::PLUGIN_TOOL, None);
        self.edges.push(Edge::new(
            self.module_id,
            tool_id,
            edge_category::DEFINES,
            Confidence::Strong,
        ));
        tool_id
    }

    /// One handler FUNCTION: the node, its `DEFINES` edge, its nav record,
    /// its `HANDLED_BY` edge from the tool (evidence at the registration
    /// call's row), and its body walked with the handler as owner. Both
    /// shape readers share it.
    fn mint_handler(&mut self, tool_id: NodeId, dotted_member: &str, value: TsNode<'a>, line: u32) {
        let qname = format!("{}::{}", self.module_qname, dotted_member);
        let id = NodeId::from_parts(GRAPH_TYPE, self.repo, node_kind::FUNCTION, &qname);
        self.nodes.push(Node {
            id,
            repo: self.repo,
            confidence: Confidence::Strong,
            cells: entity_cells(&value, self.src, self.file_rel_path),
        });
        self.edges.push(Edge::new(
            self.module_id,
            id,
            edge_category::DEFINES,
            Confidence::Strong,
        ));
        self.nav.record(
            id,
            dotted_member,
            &qname,
            node_kind::FUNCTION,
            Some(self.module_id),
        );
        self.counts.functions += 1;
        let ev = evidence::Evidence::emitter("extractor:lua")
            .rule("register_handler")
            .at(self.file_rel_path, line);
        self.edges.push(
            Edge::new(tool_id, id, edge_category::HANDLED_BY, Confidence::Strong)
                .with_cell(ev.to_cell()),
        );
        // The handler's body walks with the handler as owner, so its
        // calls carry real edges into the plugin's own modules.
        self.visit_children(value, id);
    }

    /// A call that matched no shipped host family is classified structurally
    /// (L4): a call of a known registration shape is a detection candidate
    /// whoever owns the API. The fact lands on the file MODULE for
    /// `resolver:plugin`'s detection report; the call itself still binds as
    /// an ordinary call. Single-identifier callees stay out: a registration
    /// API is a namespaced host API (`maki.api`, `vim.api`, `core.register`)
    /// — measured on maki, every single-segment positional candidate was a
    /// local callback idiom (`case("pat", fn)` dispatch), not a registration.
    fn record_registration_candidate(&mut self, call: TsNode<'a>, dotted: &str) {
        if !dotted.contains('.') {
            return;
        }
        let Some(shape) = candidate_shape(call, self.src) else {
            return;
        };
        let named = candidate_named(call, self.src, shape);
        self.counts.cands += 1;
        self.nav.record_fact(
            self.module_id,
            NavFact::Registration {
                callee: dotted.to_string(),
                shape,
                named,
            },
        );
    }

    /// A non-minting registration's arguments still walk: an auxiliary
    /// registration's content closures and an unnamed tool's handler bodies
    /// run real calls, and with no tool node to own them they attribute to
    /// the call site's owner. Named tool handlers walk in the shape readers
    /// instead.
    fn walk_registration_args(&mut self, call: TsNode<'a>, owner: NodeId) {
        let Some(args) = call.child_by_field_name("arguments") else {
            return;
        };
        let Some(table) = table_argument(args) else {
            // No table constructor (the argument is an expression, e.g.
            // `register_options(spec.extend({..}))`): walk the arguments as
            // ordinary expressions of the owner.
            self.visit_children(args, owner);
            return;
        };
        let mut cursor = table.walk();
        let fields: Vec<TsNode> = table.named_children(&mut cursor).collect();
        for field in fields {
            if let Some(value) = field.child_by_field_name("value") {
                self.visit(value, owner);
            }
        }
    }

    /// `local <alias> = require("spec")`: records the alias fact on the file
    /// MODULE, which `build_lua`'s call binding reads (rule `require_alias`).
    /// Only the `local` form records it — the grammar's `variable_declaration`
    /// wraps the assignment; a global `T = require(..)` is a bare
    /// `assignment_statement`.
    fn record_require_alias(&mut self, assignment: TsNode<'a>) {
        // The parent here is the declaration wrapper (`variable_declaration`
        // through the transparent `declaration` supertype) for a local, or
        // the enclosing block for a global assignment.
        let parent = assignment.parent();
        let is_local = parent.is_some_and(|p| p.kind() == "variable_declaration");
        if !is_local {
            return;
        }
        let mut cursor = assignment.walk();
        let children: Vec<TsNode> = assignment.named_children(&mut cursor).collect();
        let variables = children
            .iter()
            .find(|c| c.kind() == "variable_list")
            .copied();
        let values = children
            .iter()
            .find(|c| c.kind() == "expression_list")
            .copied();
        let (Some(variables), Some(values)) = (variables, values) else {
            return;
        };
        let names: Vec<String> = variables
            .named_children(&mut variables.walk())
            .filter(|v| v.kind() == "identifier")
            .map(|v| text_of(v, self.src).to_string())
            .collect();
        let values: Vec<TsNode> = values.named_children(&mut values.walk()).collect();
        for (name, value) in names.into_iter().zip(values) {
            if value.kind() != "function_call" {
                continue;
            }
            let callee = value.child_by_field_name("name");
            let args = value.child_by_field_name("arguments");
            let is_require = callee
                .is_some_and(|c| c.kind() == "identifier" && text_of(c, self.src) == "require");
            if !is_require {
                continue;
            }
            if let Some(spec) = args.and_then(|a| single_string_argument(a, self.src))
                && !spec.is_empty()
            {
                self.nav
                    .record_fact(self.module_id, NavFact::RequireAlias { alias: name, spec });
            }
        }
    }
}

/// The dotted text of an identifier / `dot_index_expression` chain
/// (`T`, `T.f`, `A.B.f`). `None` for anything dynamic.
fn dotted_name(node: TsNode, src: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => Some(text_of(node, src).to_string()),
        "dot_index_expression" => {
            let table = node.child_by_field_name("table")?;
            let field = node.child_by_field_name("field")?;
            Some(format!(
                "{}.{}",
                dotted_name(table, src)?,
                text_of(field, src)
            ))
        }
        _ => None,
    }
}

/// The full dotted text of a call's callee, for the registration-prefix match:
/// an identifier (`require`), a dot chain (`maki.api.register_tool`), or a
/// colon method rendered dotted (`maki.api:register_tool` -> `maki.api.register_tool`).
fn callee_dotted(callee: TsNode, src: &[u8]) -> Option<String> {
    match callee.kind() {
        "identifier" | "dot_index_expression" => dotted_name(callee, src),
        "method_index_expression" => {
            let table = callee.child_by_field_name("table")?;
            let method = callee.child_by_field_name("method")?;
            Some(format!(
                "{}.{}",
                dotted_name(table, src)?,
                text_of(method, src)
            ))
        }
        _ => None,
    }
}

/// The `CallQualifier` of an ordinary call's callee: `Bare(f)` for a plain
/// identifier, `Attribute { base, name }` for `T.f` / `A.B.f` / `T:m` — the
/// qualifier text `build_lua` binds file-locally.
fn callee_qualifier(callee: TsNode, src: &[u8]) -> Option<CallQualifier> {
    match callee.kind() {
        "identifier" => Some(CallQualifier::Bare(text_of(callee, src).to_string())),
        "dot_index_expression" => {
            let dotted = dotted_name(callee, src)?;
            let (base, name) = dotted.rsplit_once('.')?;
            Some(CallQualifier::Attribute {
                base: base.to_string(),
                name: name.to_string(),
            })
        }
        "method_index_expression" => {
            let table = callee.child_by_field_name("table")?;
            let method = callee.child_by_field_name("method")?;
            Some(CallQualifier::Attribute {
                base: dotted_name(table, src)?,
                name: text_of(method, src).to_string(),
            })
        }
        _ => None,
    }
}

/// The call's first argument as a table constructor, through either argument
/// form: `f{ .. }` (the arguments node IS the table) or `f({ .. })`.
fn table_argument(args: TsNode) -> Option<TsNode> {
    if args.kind() == "table_constructor" {
        return Some(args);
    }
    if args.kind() != "arguments" {
        return None;
    }
    let mut cursor = args.walk();
    let first = args.named_children(&mut cursor).next()?;
    (first.kind() == "table_constructor").then_some(first)
}

/// The single string-literal argument of a call: `require("x")`,
/// `require 'x'`, `require[[x]]`. `None` for zero, several or non-string
/// arguments (and for an empty string).
fn single_string_argument(args: TsNode, src: &[u8]) -> Option<String> {
    if args.kind() == "string" {
        return string_content(args, src);
    }
    if args.kind() != "arguments" {
        return None;
    }
    let mut cursor = args.walk();
    let children: Vec<TsNode> = args.named_children(&mut cursor).collect();
    if children.len() != 1 || children[0].kind() != "string" {
        return None;
    }
    string_content(children[0], src)
}

/// The content of a string literal, quoted or long-bracket.
fn string_content(string: TsNode, src: &[u8]) -> Option<String> {
    let content = string.child_by_field_name("content")?;
    Some(text_of(content, src).to_string())
}

/// A `field` whose `name` is the bare identifier `key` and whose value is a
/// string literal, as its content text.
fn named_string_field(field: TsNode, key: &str, src: &[u8]) -> Option<String> {
    let name = field.child_by_field_name("name")?;
    if name.kind() != "identifier" || text_of(name, src) != key {
        return None;
    }
    let value = field.child_by_field_name("value")?;
    if value.kind() != "string" {
        return None;
    }
    string_content(value, src)
}

/// Whether a table `field`'s key is the bare identifier `key` (any value).
fn field_named(field: TsNode, key: &str, src: &[u8]) -> bool {
    field
        .child_by_field_name("name")
        .is_some_and(|n| n.kind() == "identifier" && text_of(n, src) == key)
}

/// Whether a table `field`'s value is an anonymous `function_definition` —
/// a handler closure under either shape reader.
fn anon_fn_field(field: TsNode) -> bool {
    field
        .child_by_field_name("value")
        .is_some_and(|v| v.kind() == "function_definition")
}

/// The registration shape a call carries structurally, when it does. The
/// callee is not consulted — the shape is a property of the call, which is
/// what lets the detection report group candidates by family afterwards.
/// Table: a table-constructor argument holding a `name` key (any value) and
/// at least one anonymous-function field. Positional: a string-literal
/// argument beside an anonymous-function argument.
fn candidate_shape(call: TsNode, src: &[u8]) -> Option<plugin_families::ShapeTag> {
    let args = call.child_by_field_name("arguments")?;
    if let Some(table) = table_argument(args) {
        let mut cursor = table.walk();
        let fields: Vec<TsNode> = table.named_children(&mut cursor).collect();
        let named = fields.iter().any(|f| field_named(*f, "name", src));
        let handlers = fields.iter().any(|f| anon_fn_field(*f));
        return (named && handlers).then_some(plugin_families::ShapeTag::Table);
    }
    let argv: Vec<TsNode> = args.named_children(&mut args.walk()).collect();
    let string = argv.iter().any(|a| a.kind() == "string");
    let handler = argv.iter().any(|a| a.kind() == "function_definition");
    (string && handler).then_some(plugin_families::ShapeTag::Positional)
}

/// Whether the candidate's name slot held a string literal: the table
/// shape's `name` field value, or any string argument of the positional
/// shape.
fn candidate_named(call: TsNode, src: &[u8], shape: plugin_families::ShapeTag) -> bool {
    let Some(args) = call.child_by_field_name("arguments") else {
        return false;
    };
    match shape {
        plugin_families::ShapeTag::Table => table_argument(args)
            .and_then(|table| {
                table
                    .named_children(&mut table.walk())
                    .find(|f| field_named(*f, "name", src))
            })
            .and_then(|f| f.child_by_field_name("value"))
            .is_some_and(|v| v.kind() == "string"),
        plugin_families::ShapeTag::Positional => args
            .named_children(&mut args.walk())
            .any(|a| a.kind() == "string"),
    }
}

/// The 0-based row a node starts on: the `line` of the `UnresolvedRef` it
/// asserts (LC.3b, POSITION convention).
fn line_at(n: TsNode) -> u32 {
    u32::try_from(n.start_position().row).unwrap_or(u32::MAX)
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

fn position_json(start_row: usize, end_row: usize, file_rel: &str) -> String {
    format!(
        "{{\"file\":\"{}\",\"start_line\":{},\"end_line\":{}}}",
        file_rel.replace('\\', "\\\\").replace('"', "\\\""),
        start_row,
        end_row
    )
}

fn file_cells(root: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*root, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(position_json(
                root.start_position().row,
                root.end_position().row,
                file_rel,
            )),
        },
    ]
}

fn entity_cells(node: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*node, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(position_json(
                node.start_position().row,
                node.end_position().row,
                file_rel,
            )),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_core::NodeKindId;

    fn repo() -> RepoId {
        RepoId(1)
    }

    fn families() -> plugin_families::PluginFamilies {
        plugin_families::PluginFamilies::new(plugin_families::seed_families())
    }

    fn parse(source: &str, path: &str, module: &str) -> FileParse {
        parse_file(source, path, module, repo(), &families()).unwrap()
    }

    fn qnames_of_kind(fp: &FileParse, kind: NodeKindId) -> Vec<String> {
        let mut out: Vec<String> = fp
            .nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&kind))
            .map(|(_, q)| q.clone())
            .collect();
        out.sort();
        out
    }

    fn ids_of_kind(fp: &FileParse, kind: NodeKindId) -> Vec<NodeId> {
        fp.nav
            .qname_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&kind))
            .map(|(id, _)| *id)
            .collect()
    }

    #[test]
    fn functions_and_methods_are_flat_qnames() {
        let source = r#"
local function greet(name)
  return "hi " .. name
end

function M.run(opts)
  return greet(opts.name)
end

function M.Store:push(item)
  table.insert(self.items, item)
end

function A.B.deep()
end
"#;
        let fp = parse(source, "plugins/x/init.lua", "plugins::x::init");
        let fns = qnames_of_kind(&fp, node_kind::FUNCTION);
        assert_eq!(
            fns,
            vec![
                "plugins::x::init::A.B.deep",
                "plugins::x::init::M.run",
                "plugins::x::init::greet",
            ]
        );
        let methods = qnames_of_kind(&fp, node_kind::METHOD);
        assert_eq!(methods, vec!["plugins::x::init::M.Store.push"]);
        // MODULE -DEFINES-> each symbol (4 symbols), all from the module.
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "plugins::x::init");
        let defines = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::DEFINES && e.from == module)
            .count();
        assert_eq!(defines, 4);
        // Every symbol's nav parent is the module; nav names are the dotted
        // tails, so the module symbol table can key `M.run` distinctly.
        for id in ids_of_kind(&fp, node_kind::FUNCTION) {
            assert_eq!(fp.nav.parent_of.get(&id), Some(&module));
        }
        assert!(fp.nav.name_by_id.values().any(|n| n == "M.Store.push"));
    }

    #[test]
    fn requires_emit_imports_refs_with_the_spec_verbatim() {
        let source = r#"
local a = require("helper")
local b = require('a.b')
local c = require[[long.name]]
local d = require(some_var)
local e = require("helper", "extra")
local f = require()
"#;
        let fp = parse(source, "x.lua", "x");
        let specs: Vec<String> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::IMPORTS)
            .map(|r| match &r.qualifier {
                CallQualifier::Bare(s) => s.clone(),
                _ => panic!("a require ref is Bare"),
            })
            .collect();
        assert_eq!(specs, vec!["helper", "a.b", "long.name"]);
        // The non-literal, two-argument and empty-argument forms emit no ref
        // (counted in the marker's requires=, never guessed).
        assert_eq!(fp.refs.len(), 3);
        // `from` is the file MODULE at top level.
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "x");
        assert!(
            fp.refs
                .iter()
                .all(|r| r.from == module && r.from_module == module)
        );
    }

    #[test]
    fn require_alias_is_recorded_for_local_only() {
        let local = parse("local T = require(\"maki.tool_view\")\n", "a.lua", "a");
        let facts = local.nav.nav_facts.values().flatten().collect::<Vec<_>>();
        assert!(
            facts.iter().any(
                |f| matches!(f, NavFact::RequireAlias { alias, spec } if alias == "T" && spec == "maki.tool_view")
            ),
            "the local form records RequireAlias, got {facts:?}"
        );
        let global = parse("T = require(\"maki.tool_view\")\n", "a.lua", "a");
        let facts = global.nav.nav_facts.values().flatten().collect::<Vec<_>>();
        assert!(
            !facts
                .iter()
                .any(|f| matches!(f, NavFact::RequireAlias { .. })),
            "a global binding records nothing, got {facts:?}"
        );
        let other = parse("local T = make_table()\n", "a.lua", "a");
        let facts = other.nav.nav_facts.values().flatten().collect::<Vec<_>>();
        assert!(
            !facts
                .iter()
                .any(|f| matches!(f, NavFact::RequireAlias { .. }))
        );
    }

    #[test]
    fn registration_mints_tool_handlers_and_edges() {
        let source = r#"
local M = {}

maki.api.register_tool({
  name = "greet",
  desc = "say hi",
  execute = function(args)
    return helper.greet(args[1])
  end,
  restore = function()
    M.cleanup()
  end,
})
"#;
        let fp = parse(source, "plugins/mytool/init.lua", "plugins::mytool::init");
        let tools = qnames_of_kind(&fp, node_kind::PLUGIN_TOOL);
        assert_eq!(tools, vec!["plugin_tool:greet @maki/mytool"]);
        let handlers = qnames_of_kind(&fp, node_kind::FUNCTION);
        assert_eq!(
            handlers,
            vec![
                "plugins::mytool::init::greet.execute",
                "plugins::mytool::init::greet.restore"
            ]
        );
        let tool = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::PLUGIN_TOOL,
            "plugin_tool:greet @maki/mytool",
        );
        let handled = fp
            .edges
            .iter()
            .filter(|e| e.category == edge_category::HANDLED_BY && e.from == tool)
            .count();
        assert_eq!(handled, 2, "one HANDLED_BY per handler closure");
        // The HANDLED_BY edges carry their own evidence (site, rule).
        let ev = fp
            .edges
            .iter()
            .find(|e| e.category == edge_category::HANDLED_BY)
            .and_then(|e| evidence::Evidence::read(&e.cells))
            .expect("HANDLED_BY carries evidence");
        assert_eq!(ev.emitter, "extractor:lua");
        assert_eq!(ev.rule.as_deref(), Some("register_handler"));
        assert_eq!(ev.basis, evidence::Basis::Site);
        // The handler's body call attributes to the handler: a CALLS ref
        // whose `from` is the handler FUNCTION node.
        let exec = NodeId::from_parts(
            GRAPH_TYPE,
            repo(),
            node_kind::FUNCTION,
            "plugins::mytool::init::greet.execute",
        );
        assert!(
            fp.refs
                .iter()
                .any(|r| r.category == edge_category::CALLS
                    && r.from == exec
                    && matches!(&r.qualifier, CallQualifier::Attribute { base, name } if base == "helper" && name == "greet"))
        );
        // The registration call mints no CALLS ref of its own.
        assert!(
            !fp.refs
                .iter()
                .any(|r| r.category == edge_category::CALLS
                    && matches!(&r.qualifier, CallQualifier::Attribute { base, .. } if base.contains("api")))
        );
    }

    #[test]
    fn registration_without_a_name_mints_nothing() {
        let source = "maki.api.register_tool({ execute = function() end })\n";
        let fp = parse(source, "x.lua", "x");
        assert!(qnames_of_kind(&fp, node_kind::PLUGIN_TOOL).is_empty());
        assert!(qnames_of_kind(&fp, node_kind::FUNCTION).is_empty());
        // A dotted registration callee with no table argument, ditto.
        let fp = parse("maki.api.register_tool()\n", "y.lua", "y");
        assert!(qnames_of_kind(&fp, node_kind::PLUGIN_TOOL).is_empty());
        // Neither counts as a call ref (a host-API call is not modelled).
        assert!(fp.refs.iter().all(|r| r.category != edge_category::CALLS));
    }

    #[test]
    fn auxiliary_registrations_mint_nothing_and_still_walk() {
        // An aux registration (the family's tools gate): no
        // PLUGIN_TOOL, no handler FUNCTION — and its content closure's call
        // attributes to the module instead of being lost with the table.
        let source = r#"
maki.api.register_prompt_hint({
  slot = "tool_usage",
  content = function()
    return render(page)
  end,
})
"#;
        let fp = parse(source, "plugins/x/init.lua", "plugins::x::init");
        assert!(qnames_of_kind(&fp, node_kind::PLUGIN_TOOL).is_empty());
        assert!(qnames_of_kind(&fp, node_kind::FUNCTION).is_empty());
        let module = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "plugins::x::init");
        let calls = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::CALLS && r.from == module)
            .count();
        assert_eq!(calls, 1, "the aux closure's call attributes to the module");
    }

    #[test]
    fn a_named_auxiliary_call_mints_no_tool() {
        // Ruling 3's closed class: an auxiliary registration that carries a
        // literal `name` (an options-for-X API) must not mint a PLUGIN_TOOL.
        let fp = parse(
            "maki.api.register_options({ name = \"x\", check = function() end })\n",
            "y.lua",
            "y",
        );
        assert!(qnames_of_kind(&fp, node_kind::PLUGIN_TOOL).is_empty());
        assert!(qnames_of_kind(&fp, node_kind::FUNCTION).is_empty());
    }

    #[test]
    fn a_synthetic_family_row_drives_the_registration_scan() {
        let host = plugin_families::PluginFamily {
            name: "toy".to_string(),
            manifest: "host.toml".to_string(),
            calls: vec![plugin_families::RegistrationCall {
                prefix: "toy.register_".to_string(),
                shape: plugin_families::Shape::Table,
                tools: Vec::new(),
                handler_name: String::new(),
            }],
            lib_roots: Vec::new(),
        };
        let fams = plugin_families::PluginFamilies::new(vec![host.clone()]);
        let source = "toy.register_thing({ name = \"thing\", run = function() end })\n";
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_lua::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let src = source.as_bytes();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "x::init");
        let mut w = Walk {
            src,
            file_rel_path: "x/init.lua",
            module_qname: "x::init",
            module_id,
            repo: repo(),
            families: &fams,
            nodes: Vec::new(),
            edges: Vec::new(),
            refs: Vec::new(),
            nav: CodeNav::default(),
            counts: Counts::default(),
        };
        // Find the call and mint through the synthetic row directly.
        fn find_call(node: TsNode) -> Option<TsNode> {
            if node.kind() == "function_call" {
                return Some(node);
            }
            let mut cursor = node.walk();
            node.named_children(&mut cursor).find_map(find_call)
        }
        let call = find_call(tree.root_node()).expect("the registration call");
        w.mint_registration(call, &host.name, module_id);
        let tools: Vec<&String> = w
            .nav
            .qname_by_id
            .values()
            .filter(|q| q.starts_with("plugin_tool:"))
            .collect();
        assert_eq!(
            tools,
            vec!["plugin_tool:thing @toy/x"],
            "a non-maki host mints too"
        );
    }

    #[test]
    fn a_positional_family_row_mints_tool_and_handlers() {
        // The L4 positional shape reader, driven by a synthetic row: the
        // neovim-style call mints the tool, the positional handler under the
        // family's handler_name, and the keyed handler from the options
        // table. The non-handler field walks and mints nothing.
        let host = plugin_families::PluginFamily {
            name: "toy".to_string(),
            manifest: String::new(),
            calls: vec![plugin_families::RegistrationCall {
                prefix: "toy.on_".to_string(),
                shape: plugin_families::Shape::Positional {
                    name_arg: 1,
                    handler_args: vec![2],
                    handler_table: Some((3, vec!["complete".to_string()])),
                },
                tools: Vec::new(),
                handler_name: "run".to_string(),
            }],
            lib_roots: Vec::new(),
        };
        let fams = plugin_families::PluginFamilies::new(vec![host.clone()]);
        let source = r#"
toy.on_thing("thing", function() work() end, { complete = function() list() end, plain = "x" })
"#;
        let mut parser = Parser::new();
        let lang: tree_sitter::Language = tree_sitter_lua::LANGUAGE.into();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let src = source.as_bytes();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "x::init");
        let mut w = Walk {
            src,
            file_rel_path: "x/init.lua",
            module_qname: "x::init",
            module_id,
            repo: repo(),
            families: &fams,
            nodes: Vec::new(),
            edges: Vec::new(),
            refs: Vec::new(),
            nav: CodeNav::default(),
            counts: Counts::default(),
        };
        fn find_call(node: TsNode) -> Option<TsNode> {
            if node.kind() == "function_call" {
                return Some(node);
            }
            let mut cursor = node.walk();
            node.named_children(&mut cursor).find_map(find_call)
        }
        let call = find_call(tree.root_node()).expect("the registration call");
        let family = host.calls.first().unwrap();
        w.mint_registration_positional(call, &host.name, family, module_id);
        let tools: Vec<&String> = w
            .nav
            .qname_by_id
            .values()
            .filter(|q| q.starts_with("plugin_tool:"))
            .collect();
        assert_eq!(tools, vec!["plugin_tool:thing @toy/x"]);
        let mut handlers: Vec<&String> = w
            .nav
            .qname_by_id
            .values()
            .filter(|q| q.contains("::thing."))
            .collect();
        handlers.sort();
        assert_eq!(
            handlers,
            vec!["x::init::thing.complete", "x::init::thing.run"],
            "the positional handler takes the family's handler_name; the keyed one its key"
        );
        let handled = w
            .edges
            .iter()
            .filter(|e| e.category == edge_category::HANDLED_BY)
            .count();
        assert_eq!(handled, 2, "one HANDLED_BY per handler");
        assert_eq!(w.counts.registrations, 1);
        assert_eq!(w.counts.cands, 0, "a host-family call is not a candidate");
    }

    #[test]
    fn unaffiliated_shape_calls_are_detection_candidates() {
        // A call of a known registration shape whose callee matches no host
        // family records a NavFact::Registration candidate on the MODULE and
        // mints nothing (L4: the zero-config front door's raw material).
        let source = r#"
some.api.register_thing("thing", function() work() end)
other.config({ name = "plain", apply = function() end })
local dynamic = spec.name
yet.another.register({ name = dynamic, handler = function() end })
not_a_registration(function() end)
case("pattern", function() end)
"#;
        let fp = parse(source, "x.lua", "x");
        assert!(qnames_of_kind(&fp, node_kind::PLUGIN_TOOL).is_empty());
        let cands: Vec<NavFact> = fp
            .nav
            .nav_facts
            .values()
            .flatten()
            .filter_map(|f| match f {
                NavFact::Registration {
                    callee,
                    shape,
                    named,
                } => Some(NavFact::Registration {
                    callee: callee.clone(),
                    shape: *shape,
                    named: *named,
                }),
                _ => None,
            })
            .collect();
        assert_eq!(
            cands,
            vec![
                NavFact::Registration {
                    callee: "some.api.register_thing".into(),
                    shape: plugin_families::ShapeTag::Positional,
                    named: true,
                },
                NavFact::Registration {
                    callee: "other.config".into(),
                    shape: plugin_families::ShapeTag::Table,
                    named: true,
                },
                NavFact::Registration {
                    callee: "yet.another.register".into(),
                    shape: plugin_families::ShapeTag::Table,
                    named: false,
                },
            ],
            "the dynamic-name table stays a candidate but unnamed; a handler without a string stays out"
        );
    }

    #[test]
    fn call_qualifiers_are_bare_and_attribute() {
        let source = r#"
run()
T.f()
T:m()
A.B.deep()
local x = f().g()
(q)()
"#;
        let fp = parse(source, "x.lua", "x");
        let calls: Vec<(String, String)> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::CALLS)
            .map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => (String::new(), n.clone()),
                CallQualifier::Attribute { base, name } => (base.clone(), name.clone()),
                _ => panic!("unexpected qualifier"),
            })
            .collect();
        assert!(
            calls.contains(&("".into(), "run".into())),
            "bare f, got {calls:?}"
        );
        assert!(calls.contains(&("T".into(), "f".into())));
        assert!(
            calls.contains(&("T".into(), "m".into())),
            "the colon form is Attribute"
        );
        assert!(calls.contains(&("A.B".into(), "deep".into())));
        // The chained call's inner `f()` is its own call site; the outer
        // `...g()` (a call-result receiver) mints nothing.
        assert!(calls.contains(&("".into(), "f".into())));
        assert!(!calls.iter().any(|(_, n)| n == "g"));
        // `(q)()` is dynamic: nothing.
        assert_eq!(calls.len(), 5);
    }

    #[test]
    fn calls_inside_a_function_attribute_to_it() {
        let source = "function M.run()\n  helper()\nend\n";
        let fp = parse(source, "x.lua", "x");
        let run = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, "x::M.run");
        assert!(fp.refs.iter().any(|r| r.category == edge_category::CALLS
            && r.from == run
            && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "helper")));
    }

    #[test]
    fn plugin_dir_is_the_registering_files_directory() {
        assert_eq!(plugin_dir_of("plugins/question/init.lua"), "question");
        assert_eq!(plugin_dir_of("mytool/init.lua"), "mytool");
        // A root-level file registers under its stem, deterministically.
        assert_eq!(plugin_dir_of("init.lua"), "init");
        assert_eq!(plugin_dir_of("deep/nested/tools/tool.lua"), "tools");
    }
}
