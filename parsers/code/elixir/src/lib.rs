use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};
use tree_sitter::{Node as TsNode, Parser};

pub use glia_code_domain::{
    CallQualifier, CallSite, CodeNav, FileParse, GRAPH_TYPE, ImportStmt, ImportTarget, ParseError,
    UnresolvedRef, cell_type, edge_category, node_kind,
};
use glia_code_domain::data_entity;
use glia_code_domain::endpoint::{ClientEndpoint, join_scope, push_client_endpoint, url_to_path};
use glia_code_domain::evidence::{self, Evidence};
use glia_code_domain::line_of;

pub fn parse_file(
    source: &str,
    file_rel_path: &str,
    module_qname: &str,
    repo: RepoId,
) -> Result<FileParse, ParseError> {
    let mut parser = Parser::new();
    let lang: tree_sitter::Language = tree_sitter_elixir::LANGUAGE.into();
    parser
        .set_language(&lang)
        .map_err(|e| ParseError::LanguageInit(e.to_string()))?;
    let tree = parser.parse(source, None).ok_or(ParseError::NoTree)?;
    let src = source.as_bytes();
    let root = tree.root_node();

    let module_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, module_qname);
    // The file-stem MODULE qname (e.g. `controller`). Elixir `import`/`alias`/`use`
    // forms live *inside* a `defmodule`, so the walk's `parent_qname` at that point
    // is the enclosing PACKAGE qname (`controller::MyAppWeb.UserController`), which
    // `resolve_imports_python` never finds in `module_by_qname` (MODULE nodes only)
    // → every import early-continues. Stash the file MODULE qname so `collect_import`
    // emits `from_module` = the file stem, matching what the graph registers.
    let mut acc = Acc {
        module_qname: module_qname.to_string(),
        module_id: Some(module_id),
        ..Acc::default()
    };
    acc.nodes.push(Node {
        id: module_id,
        repo,
        confidence: Confidence::Strong,
        cells: file_cells(&root, src, file_rel_path),
    });
    let module_simple = module_qname.rsplit("::").next().unwrap_or(module_qname);
    acc.nav
        .record(module_id, module_simple, module_qname, node_kind::MODULE, None);

    visit_top(root, src, file_rel_path, module_qname, module_id, repo, &mut acc);

    scan_phoenix_routes(source, repo, module_id, &mut acc);
    if acc.doc_attached + acc.doc_moduledoc + acc.doc_hidden > 0 {
        eprintln!(
            "[doc] elixir attrs attached={} moduledoc={} hidden={} path={}",
            acc.doc_attached, acc.doc_moduledoc, acc.doc_hidden, file_rel_path
        );
    }
    if acc.endpoint_hits > 0 {
        eprintln!(
            "[elixir-http-client] {} endpoints in {}",
            acc.endpoint_hits, file_rel_path
        );
    }
    if let Some(line) = acc.types.marker(file_rel_path) {
        eprintln!("{line}");
    }
    if let Some(line) = acc.ecto.marker(file_rel_path) {
        eprintln!("{line}");
    }

    Ok(FileParse {
        nodes: acc.nodes,
        edges: acc.edges,
        imports: acc.imports,
        calls: acc.calls,
        refs: acc.refs,
        nav: acc.nav,
        properties: Default::default(),
    })
}

#[derive(Default)]
struct Acc {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    imports: Vec<ImportStmt>,
    calls: Vec<CallSite>,
    refs: Vec<UnresolvedRef>,
    nav: CodeNav,
    /// File-stem MODULE qname; the `from_module` every import resolves against.
    module_qname: String,
    /// The file MODULE's id: the `from_module` of every IMPLEMENTS / INJECTS
    /// ref (DH.17b), whose Bare name resolves against that file's bindings.
    /// Set by `parse_file`; a direct `visit_top` in a test leaves it unset,
    /// and then no ref is pushed.
    module_id: Option<NodeId>,
    /// DH.17b's per-file tallies (the `[elixir-types]` marker).
    types: TypeCounts,
    /// DH.18's per-file tallies (the `[orm-ecto]` marker).
    ecto: EctoCounts,
    /// DATA_ENTITY ids already pushed from this file: one node per entity,
    /// whichever of its declaration and its query sites comes first.
    data_entities: std::collections::HashSet<NodeId>,
    /// `(function, entity)` ACCESSES_DATA pairs already pushed: one edge per
    /// pair, however many sites (or clauses) repeat it.
    access_edges: std::collections::HashSet<(NodeId, NodeId)>,
    /// Per-file dedup for the shared ENDPOINT nodes client calls mint.
    endpoint_seen: std::collections::HashSet<NodeId>,
    /// Client HTTP call sites emitted from this file (drives the fired-on marker).
    endpoint_hits: usize,
    /// `@doc` texts attached to FUNCTION nodes in this file (LA.7a marker).
    doc_attached: usize,
    /// `@moduledoc` texts attached to PACKAGE / INTERFACE nodes (LA.7a marker).
    doc_moduledoc: usize,
    /// Nodes whose doc an explicit `@doc false` / `@moduledoc false` hid.
    doc_hidden: usize,
}

fn visit_top(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        visit_node(child, src, file_rel, parent_qname, parent_id, repo, acc);
    }
}

fn visit_node(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    if node.kind() != "call" {
        return;
    }
    let target = node.child_by_field_name("target").map(|n| text_of(n, src));
    let Some(target_name) = target else { return };

    match target_name {
        "defmodule" => visit_defmodule(node, src, file_rel, parent_qname, parent_id, repo, acc),
        // Outside a `defmodule` body there is no pending `@doc` to carry, so a
        // top-level `def` only ever gets the leading-comment fallback.
        "def" | "defp" => visit_def(
            node,
            src,
            file_rel,
            parent_qname,
            parent_id,
            repo,
            DocChoice::Leading,
            acc,
        ),
        "defprotocol" => visit_defprotocol(node, src, file_rel, parent_qname, parent_id, repo, acc),
        "defstruct" => visit_defstruct(node, src, file_rel, parent_qname, parent_id, repo, acc),
        "alias" => collect_import(node, src, true, acc),
        "import" | "use" => collect_import(node, src, false, acc),
        _ => {}
    }
}

fn find_args(node: TsNode) -> Option<TsNode> {
    let mut c = node.walk();
    node.named_children(&mut c).find(|ch| ch.kind() == "arguments")
}

fn visit_defmodule(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(args) = find_args(node) else {
        return;
    };
    let name = first_arg_text(args, src);
    if name.is_empty() {
        return;
    }
    // Elixir modules are dotted (`MyApp.Repo`). Keep the full dotted path in the
    // qname (for uniqueness), but record the node's *name* as the last segment
    // (`Repo`) — that is how Elixir refers to the module after `alias MyApp.Repo`,
    // and it is the short name the graph's import tail fallback matches against.
    let short = name.rsplit('.').next().unwrap_or(&name).to_string();
    let qname = format!("{parent_qname}::{name}");

    // Elixir documentation is a module attribute in the body, not a comment
    // above the node: `@moduledoc` sits INSIDE the `do_block`, after the node
    // the PACKAGE is built from, so it is pre-scanned before the push. So is
    // the body's shape (DH.17b): a behaviour is an INTERFACE, and only an
    // ExUnit case module's `test` blocks are test functions.
    let body = do_block_of(node);
    let shape = body.map_or_else(BodyShape::default, |b| body_shape(b, src));
    let kind = if shape.is_contract() {
        acc.types.behaviours += 1;
        node_kind::INTERFACE
    } else {
        node_kind::PACKAGE
    };
    let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
    let doc = body.map_or(DocChoice::Leading, |b| moduledoc_choice(b, src));
    acc.count_doc(&doc, true);
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells_with_doc(&node, src, file_rel, doc),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::CONTAINS,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav.record(id, &short, &qname, kind, Some(parent_id));

    if let Some(body) = body {
        let module = ModuleScope {
            qname: &qname,
            id,
            exunit: shape.exunit,
            ecto_schema: shape.ecto_schema,
        };
        visit_module_body(body, src, file_rel, &module, repo, acc);
    }
}

/// The module a body walk is in: its qname and node id (the parent of every
/// def, the `from` of every IMPLEMENTS / INJECTS ref), whether it `use`s
/// an ExUnit case template, so its `test` blocks are test functions, and
/// whether it `use`s Ecto.Schema, so its `schema "x" do` declares the
/// model's DATA_ENTITY (DH.18).
struct ModuleScope<'a> {
    qname: &'a str,
    id: NodeId,
    exunit: bool,
    ecto_schema: bool,
}

/// What a `defmodule` body declares at its top level, read before the node
/// is pushed: its kind depends on it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct BodyShape {
    /// A `@callback` / `@macrocallback` attribute: the module is a behaviour.
    callbacks: bool,
    /// A statement that mints a child node: `def` / `defp` / `defmodule` /
    /// `defprotocol` / `defstruct`, or (in an ExUnit case) a `test` /
    /// `describe` block.
    children: bool,
    /// A `use ExUnit.Case` / `use <..>Case` line (Phoenix's `ConnCase` /
    /// `DataCase` / `ChannelCase` `use ExUnit.Case` in turn).
    exunit: bool,
    /// A `use Ecto.Schema` / `use <App>.Schema` line ([`is_ecto_schema_use`]).
    ecto_schema: bool,
}

impl BodyShape {
    /// A behaviour (hexdocs.pm/elixir/typespecs.html#behaviours) that mints
    /// no child is a pure contract: an INTERFACE, as `defprotocol` is.
    ///
    /// A behaviour that also defines functions (default helpers) stays a
    /// PACKAGE: the graph indexes a module's FUNCTION children for call
    /// resolution only under a MODULE / PACKAGE (`build_symbol_table`'s
    /// `module_symbols`, which the Bare `package_symbol` branch and the
    /// DH.17a full-name hook read), so an INTERFACE would cut every call
    /// into its helpers. It still takes its `@behaviour` IMPLEMENTS edges;
    /// only its kind stays. Removal path: once the graph indexes an Elixir
    /// INTERFACE's FUNCTION children there, drop `!self.children`.
    fn is_contract(self) -> bool {
        self.callbacks && !self.children
    }
}

/// [`BodyShape`] of a `defmodule` body: its direct children only, so a
/// nested module's callbacks, defs and `use` lines never leak outward and a
/// `quote` block inside a `defmacro __using__` (`@behaviour X` + default
/// `def`s injected into the user) is not read.
fn body_shape(body: TsNode, src: &[u8]) -> BodyShape {
    let mut shape = BodyShape::default();
    let mut cursor = body.walk();
    let children: Vec<TsNode> = body.named_children(&mut cursor).collect();
    for child in &children {
        if let Some(call) = attribute_call(*child) {
            let attr = call.child_by_field_name("target").map(|t| text_of(t, src));
            if matches!(attr, Some("callback" | "macrocallback")) {
                shape.callbacks = true;
            }
            continue;
        }
        if child.kind() != "call" {
            continue;
        }
        match child.child_by_field_name("target").map(|t| text_of(t, src)) {
            Some("def" | "defp" | "defmodule" | "defprotocol" | "defstruct") => {
                shape.children = true;
            }
            Some("use") => {
                if let Some(args) = find_args(*child) {
                    let used = first_arg_text(args, src);
                    shape.exunit |= is_exunit_case(&used);
                    shape.ecto_schema |= is_ecto_schema_use(&used);
                }
            }
            _ => {}
        }
    }
    let tests = |c: &TsNode| {
        matches!(exunit_block(*c, src), Some(ExUnitBlock::Test | ExUnitBlock::Describe))
    };
    if shape.exunit && children.iter().any(tests) {
        shape.children = true;
    }
    shape
}

/// `use <alias>` names an ExUnit case template: `ExUnit.Case` itself, or an
/// app's own `..Case` (`MyAppWeb.ConnCase`, `MyApp.DataCase`), which `use
/// ExUnit.Case` in their `using` block. `ExUnit.CaseTemplate` (the module a
/// template is built with) is not one.
fn is_exunit_case(alias: &str) -> bool {
    alias == "ExUnit.Case"
        || alias
            .rsplit('.')
            .next()
            .is_some_and(|last| last.ends_with("Case") && is_module_alias(alias))
}

/// A dotted Elixir module alias as written (`Shop.PgUserRepo`): capitalised
/// segments of letters, digits and `_`. An atom (`:gen_server`), a variable
/// or `__MODULE__` is not one.
fn is_module_alias(text: &str) -> bool {
    !text.is_empty()
        && text.split('.').all(|seg| {
            let mut chars = seg.chars();
            chars.next().is_some_and(|c| c.is_ascii_uppercase())
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// The ExUnit block forms of a case module body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExUnitBlock {
    /// `test "name" [, ctx] [do .. end | , do: ..]`
    Test,
    /// `describe "name" do .. end`
    Describe,
    /// `setup` / `setup_all`: fixtures, not tests.
    Setup,
}

fn exunit_block(node: TsNode, src: &[u8]) -> Option<ExUnitBlock> {
    if node.kind() != "call" {
        return None;
    }
    let block = match text_of(node.child_by_field_name("target")?, src) {
        "test" => ExUnitBlock::Test,
        "describe" => ExUnitBlock::Describe,
        "setup" | "setup_all" => ExUnitBlock::Setup,
        _ => return None,
    };
    // `test` / `describe` take the name as a string literal first; a call
    // named `test` with any other first argument is not ExUnit's macro.
    if block != ExUnitBlock::Setup {
        let args = find_args(node)?;
        let mut cursor = args.walk();
        let first = args.named_children(&mut cursor).next()?;
        if first.kind() != "string" {
            return None;
        }
    }
    Some(block)
}

/// The text of a `test` / `describe` call's leading string argument, as
/// ExUnit spells it, except that a line break or tab inside a multi-line
/// literal becomes a space: a qname stays one line.
fn block_name(node: TsNode, src: &[u8]) -> Option<String> {
    let args = find_args(node)?;
    let mut cursor = args.walk();
    let first = args.named_children(&mut cursor).next()?;
    (first.kind() == "string").then(|| {
        quoted_text(first, src)
            .chars()
            .map(|c| if matches!(c, '\n' | '\r' | '\t') { ' ' } else { c })
            .collect()
    })
}

/// The value of a call's `do:` keyword argument (`def f(x), do: g(x)`,
/// `test "x", do: assert f()`): a one-liner's body.
fn do_keyword_body<'a>(node: TsNode<'a>, src: &[u8]) -> Option<TsNode<'a>> {
    keyword_value(find_args(node)?, "do", src)
}

/// DH.17b per-file tallies, the `[elixir-types]` marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct TypeCounts {
    /// `defmodule`s minted as INTERFACE (a behaviour: `@callback`, no child).
    behaviours: usize,
    /// `@behaviour X` IMPLEMENTS refs.
    implements: usize,
    /// `Application.compile_env` / `get_env` default-module INJECTS refs.
    injects: usize,
    /// ExUnit `test` blocks minted as FUNCTION.
    exunit_tests: usize,
}

impl TypeCounts {
    /// `[elixir-types] behaviours=<n> implements=<n> injects=<n>
    /// exunit_tests=<n> path=<file>`, when any count is non-zero.
    fn marker(&self, path: &str) -> Option<String> {
        (self.behaviours + self.implements + self.injects + self.exunit_tests > 0).then(|| {
            format!(
                "[elixir-types] behaviours={} implements={} injects={} exunit_tests={} path={path}",
                self.behaviours, self.implements, self.injects, self.exunit_tests
            )
        })
    }
}

/// A module attribute naming another module the graph should link to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ModuleRef {
    /// `@behaviour Shop.Notifier`: the module implements that contract.
    Behaviour(String),
    /// `@repo Application.compile_env(:shop, :user_repo, Shop.PgUserRepo)`
    /// (or `Application.get_env/3`): the default implementation the module
    /// is configured with, the explicit-contract DI idiom (hexdocs.pm/mox).
    Injected(String),
}

/// The [`ModuleRef`] a module-body statement makes, if any. Only a module
/// alias names a target: `@behaviour :gen_server` (an Erlang module) and a
/// `compile_env` / `get_env` without a default, or whose default is not a
/// module alias (`Application.get_env(:shop, :repo)`, `.., nil)`), name
/// nothing in the repo.
fn module_ref_attr(node: TsNode, src: &[u8]) -> Option<ModuleRef> {
    let call = attribute_call(node)?;
    let attr = text_of(call.child_by_field_name("target")?, src);
    let args = find_args(call)?;
    let mut cursor = args.walk();
    let value = args.named_children(&mut cursor).next()?;
    if attr == "behaviour" {
        let target = text_of(value, src).trim();
        return (value.kind() == "alias" && is_module_alias(target))
            .then(|| ModuleRef::Behaviour(target.to_string()));
    }
    if value.kind() != "call" {
        return None;
    }
    let dot = value.child_by_field_name("target")?;
    if dot.kind() != "dot" {
        return None;
    }
    let module = text_of(dot.child_by_field_name("left")?, src);
    let func = text_of(dot.child_by_field_name("right")?, src);
    if module != "Application" || !matches!(func, "compile_env" | "get_env") {
        return None;
    }
    let env_args = find_args(value)?;
    let mut cursor = env_args.walk();
    let parts: Vec<TsNode> = env_args.named_children(&mut cursor).collect();
    let [_, _, default] = parts.as_slice() else {
        return None;
    };
    let target = text_of(*default, src).trim();
    (default.kind() == "alias" && is_module_alias(target))
        .then(|| ModuleRef::Injected(target.to_string()))
}

/// The `do_block` child of a `defmodule` / `defprotocol` call.
fn do_block_of(node: TsNode) -> Option<TsNode> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|c| c.kind() == "do_block")
}

/// Walk a module body in source order, carrying a pending `@doc` to the `def`
/// it documents. A `@doc` sets the pending doc; `def` / `defp` consumes it;
/// comments and other module attributes (`@spec`, `@impl`, `@decorate`,
/// `@moduledoc`, `@typedoc`, ...) keep it; any other statement clears it, so a
/// stray `@doc` never lands on an unrelated function. A nested `defmodule` is
/// such a statement, and its own body starts with nothing pending.
///
/// Multi-clause functions (`def get(1)` / `def get(n)`) share one qname and
/// `merge_parses` appends cells by id, so once an attribute has decided a
/// function's doc, its later clauses emit no DOC at all.
///
/// DH.17b: `@behaviour X` and an `Application.compile_env` / `get_env`
/// default-module attribute are module attributes too (they keep a pending
/// `@doc`) and push their IMPLEMENTS / INJECTS ref ([`module_ref_attr`]).
/// In an ExUnit case module (`module.exunit`) a `test` block is a test
/// FUNCTION and a `describe` block's tests are walked with its name as
/// prefix ([`visit_exunit_test`]); `setup` / `setup_all` emit nothing.
///
/// DH.18: in an Ecto schema module (`module.ecto_schema`) a `schema "<table>"
/// do` statement declares the model's DATA_ENTITY ([`Acc::push_ecto_schema`]).
fn visit_module_body(
    body: TsNode,
    src: &[u8],
    file_rel: &str,
    module: &ModuleScope,
    repo: RepoId,
    acc: &mut Acc,
) {
    let (parent_qname, parent_id) = (module.qname, module.id);
    let mut pending: Option<DocAttr> = None;
    let mut decided: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        if child.kind() == "comment" {
            continue;
        }
        if let Some(attr) = doc_attr_text(child, src) {
            if matches!(attr, DocAttr::Doc(_) | DocAttr::Hidden) {
                pending = Some(attr);
            }
            continue;
        }
        if let Some(r) = module_ref_attr(child, src) {
            acc.push_module_ref(parent_id, &r, line_at(child));
            continue;
        }
        if module_attribute(child) {
            continue;
        }
        if module.exunit
            && let Some(block) = exunit_block(child, src)
        {
            pending = None;
            match block {
                ExUnitBlock::Test => {
                    visit_exunit_test(child, src, file_rel, module, None, repo, acc);
                }
                ExUnitBlock::Describe => {
                    visit_exunit_describe(child, src, file_rel, module, repo, acc);
                }
                ExUnitBlock::Setup => {}
            }
            continue;
        }
        if module.ecto_schema
            && let Some(decl) = ecto_schema_decl(child, src)
        {
            pending = None;
            acc.push_ecto_schema(module, &decl, repo);
            continue;
        }
        if let Some(name) = def_call_name(child, src) {
            let doc = match pending.take() {
                Some(DocAttr::Doc(text)) => {
                    decided.insert(name);
                    DocChoice::Text(text)
                }
                Some(DocAttr::Hidden) => {
                    decided.insert(name);
                    DocChoice::Hidden
                }
                Some(DocAttr::ModuleDoc(_) | DocAttr::ModuleHidden) | None => {
                    if decided.contains(&name) {
                        DocChoice::Omit
                    } else {
                        DocChoice::Leading
                    }
                }
            };
            visit_def(child, src, file_rel, parent_qname, parent_id, repo, doc, acc);
            continue;
        }
        pending = None;
        visit_node(child, src, file_rel, parent_qname, parent_id, repo, acc);
    }
}

/// `doc` is decided by the caller: a module body passes the pending `@doc`
/// (authoritative text, or `Hidden` for `@doc false`); everywhere else passes
/// `Leading`, the comment-above fallback.
#[allow(clippy::too_many_arguments)]
fn visit_def(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    doc: DocChoice,
    acc: &mut Acc,
) {
    let Some(args) = find_args(node) else {
        return;
    };
    let name = extract_def_name(args, src);
    if name.is_empty() {
        return;
    }
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);

    acc.count_doc(&doc, false);
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells_with_doc(&node, src, file_rel, doc),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, &name, &qname, node_kind::FUNCTION, Some(parent_id));

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "do_block" {
            collect_calls_in(child, src, id, acc);
        }
    }
    // DH.17b: a one-liner `def f(x), do: Mod.g(x)` carries its body in the
    // `do:` keyword argument; its calls are the function's too.
    if let Some(body) = do_keyword_body(node, src) {
        collect_calls_in(body, src, id, acc);
    }
    // Scan the WHOLE `def` call, not just its `do_block`: a one-liner
    // `def f(x), do: HTTPoison.get(url)` carries its body in the `do:` keyword
    // argument instead, and the head itself can never contain a client call.
    let hits = collect_client_endpoints_in(node, src, id, repo, file_rel, acc);
    acc.endpoint_hits += hits;
    // DH.18: the Ecto query sites, over the whole call for the same reason.
    collect_ecto_sites(node, src, parent_qname, id, repo, acc);
}

/// One ExUnit `test "<name>" [, ctx] do .. end` (or `, do: ..`, or the
/// body-less "not implemented" `test "<name>"`) in a case module -> the
/// FUNCTION ExUnit defines for it, named as ExUnit names it: `test <name>`,
/// inside `describe "<d>"` `test <d> <name>`
/// (hexdocs.pm/ex_unit/ExUnit.Case.html#test/3). CODE / POSITION and the
/// leading-comment DOC, DEFINES from the module, and the calls of its body as
/// its CallSites, so CL.5b's function-level TESTS pairing sees the test as
/// a root of its test module.
fn visit_exunit_test(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    module: &ModuleScope,
    describe: Option<&str>,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(name) = block_name(node, src) else {
        return;
    };
    let name = match describe {
        Some(d) => format!("test {d} {name}"),
        None => format!("test {name}"),
    };
    let qname = format!("{}::{name}", module.qname);
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::FUNCTION, &qname);
    acc.types.exunit_tests += 1;
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: module.id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav.record(id, &name, &qname, node_kind::FUNCTION, Some(module.id));
    if let Some(body) = do_block_of(node) {
        collect_calls_in(body, src, id, acc);
    }
    if let Some(body) = do_keyword_body(node, src) {
        collect_calls_in(body, src, id, acc);
    }
}

/// `describe "<d>" do .. end`: each `test` inside is a test FUNCTION named
/// with the describe prefix; `setup` blocks emit nothing; any other
/// statement walks as in the module body (ExUnit allows no nested
/// `describe`).
fn visit_exunit_describe(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    module: &ModuleScope,
    repo: RepoId,
    acc: &mut Acc,
) {
    let (Some(describe), Some(body)) = (block_name(node, src), do_block_of(node)) else {
        return;
    };
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match exunit_block(child, src) {
            Some(ExUnitBlock::Test) => {
                visit_exunit_test(child, src, file_rel, module, Some(&describe), repo, acc);
            }
            Some(ExUnitBlock::Describe | ExUnitBlock::Setup) => {}
            None => visit_node(child, src, file_rel, module.qname, module.id, repo, acc),
        }
    }
}

impl Acc {
    /// Push the IMPLEMENTS / INJECTS ref a module attribute makes, `from` the
    /// module that declares it. The qualifier is the target's last dotted
    /// segment: Elixir module nodes are named by it (`visit_defmodule`'s
    /// `short`), so `resolve_refs` binds it through the file's `alias`
    /// binding, the file's own modules, then the one repo-wide module so
    /// named; two same-named modules in different namespaces stay
    /// unresolved.
    fn push_module_ref(&mut self, from: NodeId, r: &ModuleRef, line: u32) {
        let Some(from_module) = self.module_id else {
            return;
        };
        let (target, category) = match r {
            ModuleRef::Behaviour(t) => {
                self.types.implements += 1;
                (t, edge_category::IMPLEMENTS)
            }
            ModuleRef::Injected(t) => {
                self.types.injects += 1;
                (t, edge_category::INJECTS)
            }
        };
        let short = target.rsplit('.').next().unwrap_or(target);
        self.refs.push(UnresolvedRef {
            from,
            from_module,
            qualifier: CallQualifier::Bare(short.to_string()),
            category,
            line,
        });
    }
}

fn visit_defprotocol(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let Some(args) = find_args(node) else {
        return;
    };
    let name = first_arg_text(args, src);
    if name.is_empty() {
        return;
    }
    let qname = format!("{parent_qname}::{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::INTERFACE, &qname);

    // Same `@moduledoc` pre-scan as `visit_defmodule`; the protocol body's
    // other children (function heads) are not walked.
    let doc = do_block_of(node).map_or(DocChoice::Leading, |b| moduledoc_choice(b, src));
    acc.count_doc(&doc, true);
    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells_with_doc(&node, src, file_rel, doc),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, &name, &qname, node_kind::INTERFACE, Some(parent_id));
}

fn visit_defstruct(
    node: TsNode,
    src: &[u8],
    file_rel: &str,
    parent_qname: &str,
    parent_id: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let qname = format!("{parent_qname}::__struct__");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::STRUCT, &qname);

    acc.nodes.push(Node {
        id,
        repo,
        confidence: Confidence::Strong,
        cells: entity_cells(&node, src, file_rel),
    });
    acc.edges.push(Edge {
        from: parent_id,
        to: id,
        category: edge_category::DEFINES,
        confidence: Confidence::Strong,
        cells: Vec::new(),
    });
    acc.nav
        .record(id, "__struct__", &qname, node_kind::STRUCT, Some(parent_id));
}

/// `is_alias`: the line is an `alias`, whose `as: Name` option names the
/// binding (`alias Shop.Cart, as: C` binds `C`, never `Cart`; DH.17b, the
/// DH.17a handoff). `import` / `use` take no `as:`.
fn collect_import(node: TsNode, src: &[u8], is_alias: bool, acc: &mut Acc) {
    let Some(args) = find_args(node) else {
        return;
    };
    let alias = if is_alias { alias_as(args, src) } else { None };
    // `import Plug.Conn` / `alias MyApp.Repo` / `use Phoenix.Controller`. The first
    // argument is the dotted module path; the graph's tail fallback splits it on
    // `.` and binds the final segment (`Conn`/`Repo`) to a unique in-repo module by
    // short name. External modules (e.g. `Plug.Conn`) have no in-repo target, so
    // they resolve to no edge — only in-repo aliases (e.g. an app's own `MyApp.Repo`)
    // produce an IMPORTS edge.
    let path = first_arg_text(args, src);
    if !path.is_empty() {
        acc.imports.push(ImportStmt {
            from_module: acc.module_qname.clone(),
            target: ImportTarget::Module { path, alias },
            line: line_at(node),
        });
    }
}

/// The module alias an `alias X, as: Y` line's `as:` keyword names (`Y`).
fn alias_as(args: TsNode, src: &[u8]) -> Option<String> {
    let value = keyword_value(args, "as", src)?;
    let name = text_of(value, src).trim();
    (value.kind() == "alias" && is_module_alias(name)).then(|| name.to_string())
}

/// The value of the `<key>:` pair among an `arguments` node's keyword
/// arguments (`do:` of a one-liner, `as:` of an `alias`).
fn keyword_value<'a>(args: TsNode<'a>, key: &str, src: &[u8]) -> Option<TsNode<'a>> {
    let mut cursor = args.walk();
    let keywords: Vec<TsNode<'a>> = args
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "keywords")
        .collect();
    keywords.into_iter().find_map(|kw| {
        let mut pairs = kw.walk();
        let pairs: Vec<TsNode<'a>> = kw.named_children(&mut pairs).collect();
        pairs.into_iter().find_map(|pair| {
            let k = pair.child_by_field_name("key")?;
            // The `keyword` node's text is `"do: "`: trim the blank first.
            (text_of(k, src).trim().trim_end_matches(':') == key)
                .then(|| pair.child_by_field_name("value"))
                .flatten()
        })
    })
}

fn first_arg_text(args: TsNode, src: &[u8]) -> String {
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        match child.kind() {
            "alias" | "identifier" | "atom" => {
                return text_of(child, src).trim().to_string();
            }
            _ => {
                let text = text_of(child, src).trim().to_string();
                if !text.is_empty() {
                    return text;
                }
            }
        }
    }
    String::new()
}

fn extract_def_name(args: TsNode, src: &[u8]) -> String {
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => return text_of(child, src).to_string(),
            "call" => {
                if let Some(target) = child.child_by_field_name("target") {
                    return text_of(target, src).to_string();
                }
            }
            _ => {}
        }
    }
    String::new()
}

fn collect_calls_in(node: TsNode, src: &[u8], from: NodeId, acc: &mut Acc) {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "call"
            && let Some(target) = n.child_by_field_name("target")
        {
            match target.kind() {
                "identifier" => {
                    let name = text_of(target, src);
                    if !matches!(name, "def" | "defp" | "defmodule" | "defprotocol" | "defstruct" | "import" | "alias" | "use" | "if" | "case" | "cond" | "do" | "end") {
                        acc.calls.push(CallSite {
                            from,
                            qualifier: CallQualifier::Bare(name.to_string()),
                            line: line_at(n),
                        });
                    }
                }
                "dot" => {
                    let text = text_of(target, src);
                    if let Some(pos) = text.rfind('.') {
                        acc.calls.push(CallSite {
                            from,
                            qualifier: CallQualifier::Attribute {
                                base: text[..pos].to_string(),
                                name: text[pos + 1..].to_string(),
                            },
                            line: line_at(n),
                        });
                    }
                }
                _ => {}
            }
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            if !matches!(child.kind(), "anonymous_function") {
                stack.push(child);
            }
        }
    }
}

/// The 0-based row a node starts on: the `line` of the `CallSite` /
/// `UnresolvedRef` / `ImportStmt` it asserts (LC.3b, POSITION convention).
fn line_at(n: TsNode) -> u32 {
    u32::try_from(n.start_position().row).unwrap_or(u32::MAX)
}

fn text_of<'a>(node: TsNode<'a>, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

// ---------------------------------------------------------------------------
// Client HTTP calls (HTTPoison / Tesla / Req / Finch) -> shared ENDPOINT nodes
// ---------------------------------------------------------------------------

/// Elixir HTTP client modules. Matched on the FIRST dotted segment, so
/// `HTTPoison.Base.get/1` counts as `HTTPoison`.
const ELIXIR_HTTP_CLIENTS: &[&str] = &["HTTPoison", "Tesla", "Req", "Finch"];

const ELIXIR_HTTP_VERBS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];

/// Upper-case `s` if it names an HTTP verb. Tolerates the leading `:` of an
/// atom (`:get`) and the trailing `!`/`?` of a bang/query function (`get!`).
fn http_verb(s: &str) -> Option<String> {
    let up = s
        .trim()
        .trim_start_matches(':')
        .trim_end_matches(['!', '?'])
        .to_ascii_uppercase();
    ELIXIR_HTTP_VERBS.contains(&up.as_str()).then_some(up)
}

/// Reconstruct a `string` literal's text. An `#{…}` interpolation becomes
/// `${…}` so `normalise_http_path` collapses that segment exactly as it does
/// for a TypeScript template path. Returns `(text, had_interpolation)`.
fn string_text(node: TsNode, src: &[u8]) -> (String, bool) {
    let mut out = String::new();
    let mut interpolated = false;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "interpolation" {
            out.push_str("${…}");
            interpolated = true;
        } else {
            out.push_str(text_of(child, src));
        }
    }
    (out, interpolated)
}

/// `(text, interpolated)` of the first DIRECT string-literal argument. Direct
/// only, so a string buried in a nested call argument is never mistaken for
/// the URL — and Tesla's client-first form `Tesla.get(client, "/x")` lands on
/// `"/x"` for free.
fn first_string_arg(args: TsNode, src: &[u8]) -> Option<(String, bool)> {
    let mut cursor = args.walk();
    args.named_children(&mut cursor)
        .find(|ch| ch.kind() == "string")
        .map(|ch| string_text(ch, src))
}

/// `(text, interpolated)` of `<key>: "…"` among the call's keyword arguments —
/// how `Req.get!(url: "…")` carries its URL.
fn keyword_string_arg(args: TsNode, key: &str, src: &[u8]) -> Option<(String, bool)> {
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        if child.kind() != "keywords" {
            continue;
        }
        let mut pairs = child.walk();
        for pair in child.named_children(&mut pairs) {
            let (Some(k), Some(v)) = (
                pair.child_by_field_name("key"),
                pair.child_by_field_name("value"),
            ) else {
                continue;
            };
            // The `keyword` node's text is `"url: "` — colon AND trailing
            // space — so trim the whitespace BEFORE the colon.
            if v.kind() == "string" && text_of(k, src).trim().trim_end_matches(':') == key {
                return Some(string_text(v, src));
            }
        }
    }
    None
}

/// `(verb, raw_url, interpolated)` for a `call` node that is an Elixir HTTP
/// client call, else None. Four shapes:
///   HTTPoison `HTTPoison.get("…")`            — verb is the function name
///   Tesla     `Tesla.post(client(), "…", b)`  — client-first, URL is arg 2
///   Req       `Req.get!(url: "…")`            — URL is the `url:` keyword
///   Finch     `Finch.build(:get, "…")`        — verb is the leading atom
fn client_call_candidate(n: TsNode, src: &[u8]) -> Option<(String, String, bool)> {
    let target = n.child_by_field_name("target")?;
    if target.kind() != "dot" {
        return None;
    }
    let module = text_of(target.child_by_field_name("left")?, src);
    let func = text_of(target.child_by_field_name("right")?, src);
    if !ELIXIR_HTTP_CLIENTS.contains(&module.split('.').next().unwrap_or(module)) {
        return None;
    }
    let args = find_args(n)?;
    if func == "build" {
        // `Finch.build(:get, url, …)` — the verb is the leading atom argument.
        let mut cursor = args.walk();
        let atom = args
            .named_children(&mut cursor)
            .find(|ch| ch.kind() == "atom")?;
        let verb = http_verb(text_of(atom, src))?;
        let (raw, interpolated) = first_string_arg(args, src)?;
        return Some((verb, raw, interpolated));
    }
    let verb = http_verb(func)?;
    let (raw, interpolated) =
        first_string_arg(args, src).or_else(|| keyword_string_arg(args, "url", src))?;
    Some((verb, raw, interpolated))
}

/// Outbound HTTP call sites inside a `def`/`defp` become shared ENDPOINT nodes
/// (+ a CALLS edge from the enclosing function) so `HttpStackResolver` can pair
/// them with a server ROUTE. Returns the number of call sites emitted.
fn collect_client_endpoints_in(
    body: TsNode,
    src: &[u8],
    from: NodeId,
    repo: RepoId,
    file_rel: &str,
    acc: &mut Acc,
) -> usize {
    let mut hits = 0;
    let mut stack = vec![body];
    while let Some(n) = stack.pop() {
        if n.kind() == "call"
            && let Some((method, raw, interpolated)) = client_call_candidate(n, src)
            && let Some(path) = url_to_path(&raw)
        {
            let pos = n.start_position();
            // An interpolated path is Medium (the concrete segment is unknown);
            // a plain literal is Strong — the same rule scala/swift use.
            let ep = ClientEndpoint {
                method,
                path,
                file: file_rel.to_string(),
                line: pos.row + 1,
                col: pos.column + 1,
                confidence: if interpolated {
                    Confidence::Medium
                } else {
                    Confidence::Strong
                },
            };
            push_client_endpoint(
                repo,
                &ep,
                from,
                &mut acc.nodes,
                &mut acc.edges,
                &mut acc.nav,
                &mut acc.endpoint_seen,
            );
            hits += 1;
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    hits
}

// ---------------------------------------------------------------------------
// Ecto schemas and Repo query sites -> DATA_ENTITY / ACCESSES_DATA (DH.18)
// ---------------------------------------------------------------------------
//
// An Ecto schema module (`use Ecto.Schema` + `schema "users" do .. end`,
// hexdocs.pm/ecto/Ecto.Schema.html) declares the model the rest of the app
// queries through its Repo (`Repo.all(User)`, `Repo.insert(%User{..})`,
// hexdocs.pm/ecto/Ecto.Repo.html) or a query (`from u in User`,
// hexdocs.pm/ecto/Ecto.Query.html#from/2). Identity follows A13.1's ORM rule
// (`code_domain::data_entity`): both sites key the entity on the MODEL,
// `data_entity:sql:<last alias segment>`, the one token a query site in any
// file can name, so a site lands on the declaration's node with no
// cross-file pre-pass. The declared table rides a table cell (orm `ecto`) on
// the declaration only; `DbResolver` reads it to join the model to the table
// other services name.

/// `Ecto.Repo` functions whose first argument (or piped value) is the
/// queryable, schema module or struct they read or write.
const ECTO_REPO_FUNCS: &[&str] = &[
    "all",
    "get",
    "get!",
    "get_by",
    "get_by!",
    "one",
    "one!",
    "insert",
    "insert!",
    "insert_or_update",
    "insert_or_update!",
    "update",
    "update!",
    "delete",
    "delete!",
    "insert_all",
    "update_all",
    "delete_all",
    "aggregate",
    "exists?",
    "stream",
    "preload",
];

/// DH.18 per-file tallies, the `[orm-ecto]` marker.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct EctoCounts {
    /// `schema` declarations of a schema module (each mints its DATA_ENTITY).
    schemas: usize,
    /// Of those, the ones whose source is a string literal: a table cell.
    table_cells: usize,
    /// Query sites that named an entity: a Repo call's queryable, and each
    /// `from` / `*join:` source.
    repo_sites: usize,
}

impl EctoCounts {
    /// `[orm-ecto] schemas=<n> table_cells=<n> repo_sites=<n> path=<file>`,
    /// when any count is non-zero.
    fn marker(&self, path: &str) -> Option<String> {
        (self.schemas + self.repo_sites > 0).then(|| {
            format!(
                "[orm-ecto] schemas={} table_cells={} repo_sites={} path={path}",
                self.schemas, self.table_cells, self.repo_sites
            )
        })
    }
}

/// `use <alias>` makes a module an Ecto schema: `Ecto.Schema` itself, or an
/// app's own `<App>.Schema` wrapper, whose `__using__` does `use Ecto.Schema`
/// plus shared `@primary_key` / timestamp options (the `..Case` convention of
/// [`is_exunit_case`]). A `..Schema` module that is not Ecto's (`use
/// Absinthe.Schema`) mints nothing: only a `schema "<source>" do` statement
/// declares an entity, and Absinthe's notation has none.
fn is_ecto_schema_use(alias: &str) -> bool {
    alias == "Ecto.Schema"
        || (is_module_alias(alias)
            && alias.contains('.')
            && alias.rsplit('.').next() == Some("Schema"))
}

/// A table-backed `schema "<source>" do .. end` declaration (or its `, do:`
/// one-liner form).
struct SchemaDecl {
    /// The source when it is a string literal (`"users"`); `None` for any
    /// other expression (`schema @source do`), which names no table.
    table: Option<String>,
}

/// The [`SchemaDecl`] a schema-module statement makes, if any. An
/// `embedded_schema do` (no table) and a `schema` call without a body are
/// none.
fn ecto_schema_decl(node: TsNode, src: &[u8]) -> Option<SchemaDecl> {
    if node.kind() != "call" {
        return None;
    }
    let target = node.child_by_field_name("target")?;
    if target.kind() != "identifier" || text_of(target, src) != "schema" {
        return None;
    }
    let args = find_args(node)?;
    if do_block_of(node).is_none() && keyword_value(args, "do", src).is_none() {
        return None;
    }
    let mut cursor = args.walk();
    let first = args.named_children(&mut cursor).next()?;
    if first.kind() == "keywords" {
        return None;
    }
    Some(SchemaDecl { table: literal_source(first, src) })
}

/// A string literal naming a table (`"users"`, `"public.users"`): no
/// interpolation, identifier characters and `.` only. Anything else (an
/// interpolated or blank string, a non-string) is `None`.
fn literal_source(node: TsNode, src: &[u8]) -> Option<String> {
    if node.kind() != "string" {
        return None;
    }
    let (text, interpolated) = string_text(node, src);
    let ok = !interpolated
        && !text.is_empty()
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    ok.then_some(text)
}

/// The model token of a module qname: its last dotted alias segment
/// (`lib::user::Shop.User` -> `User`), the short name `visit_defmodule`
/// gives the node. A file MODULE qname (`lib::user`, a top-level `def`'s
/// parent) is no module alias and names none.
fn model_of(module_qname: &str) -> Option<&str> {
    let name = module_qname.rsplit("::").next()?;
    let last = name.rsplit('.').next()?;
    is_module_alias(last).then_some(last)
}

/// The model-keyed DATA_ENTITY qname and id for an Ecto entity name: the one
/// construction site, shared by the declaration and every query site.
fn ecto_entity(name: &str, repo: RepoId) -> (String, NodeId) {
    let qname = format!("data_entity:sql:{name}");
    let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::DATA_ENTITY, &qname);
    (qname, id)
}

/// The entity a queryable expression names: a schema module alias
/// (`Shop.User`, `User`) or struct literal (`%User{..}`) -> its last
/// segment; `__MODULE__` / `%__MODULE__{}` -> the enclosing module's
/// (`module`); a schemaless string source (`"users"`) -> the table itself,
/// since the literal IS the table (the SQL precedent). A variable, a call or
/// an atom names nothing.
fn queryable_entity(expr: TsNode, src: &[u8], module: Option<&str>) -> Option<String> {
    match expr.kind() {
        "alias" => {
            let text = text_of(expr, src).trim();
            is_module_alias(text).then(|| text.rsplit('.').next().unwrap_or(text).to_string())
        }
        "identifier" if text_of(expr, src) == "__MODULE__" => module.map(str::to_string),
        "map" => {
            let mut cursor = expr.walk();
            let st = expr.named_children(&mut cursor).find(|c| c.kind() == "struct")?;
            let mut inner = st.walk();
            let name = st.named_children(&mut inner).next()?;
            matches!(name.kind(), "alias" | "identifier")
                .then(|| queryable_entity(name, src, module))
                .flatten()
        }
        "string" => literal_source(expr, src),
        _ => None,
    }
}

/// A `|>` pipeline operator node.
fn is_pipe(node: TsNode, src: &[u8]) -> bool {
    node.kind() == "binary_operator"
        && node
            .child_by_field_name("operator")
            .is_some_and(|op| text_of(op, src) == "|>")
}

/// When `call` is a pipeline stage (the right operand of a `|>`), the head
/// of its pipeline: the leftmost operand of the left-nested `|>` chain, the
/// value piped in as the stage's first argument.
fn pipeline_head<'a>(call: TsNode<'a>, src: &[u8]) -> Option<TsNode<'a>> {
    let parent = call.parent()?;
    if !is_pipe(parent, src) || parent.child_by_field_name("right")?.id() != call.id() {
        return None;
    }
    let mut head = parent.child_by_field_name("left")?;
    while is_pipe(head, src) {
        head = head.child_by_field_name("left")?;
    }
    Some(head)
}

/// The queryable expression of an `Ecto.Repo` call: `<..>Repo.<f>(q, ..)`
/// with `f` in [`ECTO_REPO_FUNCS`] -> its first argument or, when the call
/// is a pipeline stage (`%User{} |> User.changeset(a) |> Repo.insert()`),
/// the pipeline's head. The base gate is the Repo convention (`MyApp.Repo`,
/// generated by `mix ecto.gen.repo` with `use Ecto.Repo`): a module alias
/// whose last segment is `Repo`. No import is required, since a context
/// module calls `Repo.*` after `alias MyApp.Repo` without naming Ecto; a
/// non-Ecto module so named whose `all/1` takes a module would read as Ecto.
fn repo_call_queryable<'a>(call: TsNode<'a>, src: &[u8]) -> Option<TsNode<'a>> {
    let target = call.child_by_field_name("target")?;
    if target.kind() != "dot" {
        return None;
    }
    let base = text_of(target.child_by_field_name("left")?, src).trim();
    let func = text_of(target.child_by_field_name("right")?, src);
    if !is_module_alias(base)
        || base.rsplit('.').next() != Some("Repo")
        || !ECTO_REPO_FUNCS.contains(&func)
    {
        return None;
    }
    if let Some(head) = pipeline_head(call, src) {
        return Some(head);
    }
    let args = find_args(call)?;
    let mut cursor = args.walk();
    args.named_children(&mut cursor)
        .next()
        .filter(|first| first.kind() != "keywords")
}

/// The right operand of an `x in <source>` expression.
fn in_source<'a>(node: TsNode<'a>, src: &[u8]) -> Option<TsNode<'a>> {
    (node.kind() == "binary_operator"
        && node
            .child_by_field_name("operator")
            .is_some_and(|op| text_of(op, src) == "in"))
    .then(|| node.child_by_field_name("right"))
    .flatten()
}

/// The source operands of an `Ecto.Query.from` call (`from u in User, join:
/// p in Post, ..`, imported or written `Ecto.Query.from`): the right side of
/// its leading `in` expression, and of every `*join:` keyword's (`join`,
/// `left_join`, `inner_lateral_join`, ..). `join: p in assoc(u, :posts)`
/// names no source (a call), and other keywords (`where: u.id in ^ids`) are
/// never sources.
fn from_sources<'a>(call: TsNode<'a>, src: &[u8]) -> Vec<TsNode<'a>> {
    let Some(target) = call.child_by_field_name("target") else {
        return Vec::new();
    };
    let is_from = match target.kind() {
        "identifier" => text_of(target, src) == "from",
        "dot" => {
            target.child_by_field_name("left").is_some_and(|l| text_of(l, src) == "Ecto.Query")
                && target.child_by_field_name("right").is_some_and(|r| text_of(r, src) == "from")
        }
        _ => false,
    };
    let Some(args) = find_args(call).filter(|_| is_from) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = args.walk();
    let parts: Vec<TsNode<'a>> = args.named_children(&mut cursor).collect();
    if let Some(first) = parts.first().and_then(|p| in_source(*p, src)) {
        out.push(first);
    }
    for kw in parts.iter().filter(|p| p.kind() == "keywords") {
        let mut pairs = kw.walk();
        for pair in kw.named_children(&mut pairs) {
            let is_join = pair
                .child_by_field_name("key")
                .is_some_and(|k| text_of(k, src).trim().trim_end_matches(':').ends_with("join"));
            if let Some(source) = pair
                .child_by_field_name("value")
                .filter(|_| is_join)
                .and_then(|v| in_source(v, src))
            {
                out.push(source);
            }
        }
    }
    out
}

/// Every Ecto query site inside one `def` (the WHOLE call, as
/// [`collect_client_endpoints_in`] scans it: a one-liner carries its body in
/// the `do:` keyword, and the head holds no Repo call or `from`) ->
/// ACCESSES_DATA from the function `from` to the entity each names. An
/// anonymous function's body is the function's too. Sites are emitted in
/// source order, so a repeated pair keeps its first site's line.
fn collect_ecto_sites(
    def: TsNode,
    src: &[u8],
    module_qname: &str,
    from: NodeId,
    repo: RepoId,
    acc: &mut Acc,
) {
    let module = model_of(module_qname);
    let mut sites: Vec<(u32, usize, &'static str, String)> = Vec::new();
    let mut stack = vec![def];
    while let Some(n) = stack.pop() {
        if n.kind() == "call" {
            let (row, col) = (line_at(n), n.start_position().column);
            if let Some(name) =
                repo_call_queryable(n, src).and_then(|q| queryable_entity(q, src, module))
            {
                sites.push((row, col, "ecto_repo", name));
            }
            for source in from_sources(n, src) {
                if let Some(name) = queryable_entity(source, src, module) {
                    sites.push((row, col, "ecto_from", name));
                }
            }
        }
        let mut cursor = n.walk();
        for child in n.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    sites.sort();
    for (line, _, rule, name) in sites {
        acc.push_ecto_access(from, &name, rule, line, repo);
    }
}

impl Acc {
    /// A schema module's `schema` declaration: the model-keyed DATA_ENTITY
    /// (Strong), its table cell when the source is a literal, and a DEFINES
    /// edge from the module. A query site earlier in the file may have
    /// minted the node already: the declaration is the authoritative site,
    /// so it lifts that node's confidence and carries the table cell onto it.
    ///
    /// The nav records the entity with no parent: under the module PACKAGE
    /// it would join that package's `module_symbols` under the model's short
    /// name, beside the PACKAGE itself (also named `User` under its file
    /// MODULE), and make the repo-wide unique-name lookup ambiguous for every
    /// ref naming the module. The DEFINES edge carries the structure.
    fn push_ecto_schema(&mut self, module: &ModuleScope, decl: &SchemaDecl, repo: RepoId) {
        let Some(model) = model_of(module.qname) else {
            return;
        };
        let (qname, id) = ecto_entity(model, repo);
        let cell = decl
            .table
            .as_deref()
            .map(|table| data_entity::table_cell(table, data_entity::orm::ECTO));
        self.ecto.schemas += 1;
        self.ecto.table_cells += usize::from(cell.is_some());
        if self.data_entities.insert(id) {
            self.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Strong,
                cells: cell.into_iter().collect(),
            });
            self.nav.record(id, model, &qname, node_kind::DATA_ENTITY, None);
        } else if let Some(existing) = self.nodes.iter_mut().find(|n| n.id == id) {
            existing.confidence = Confidence::Strong;
            existing.cells.extend(cell);
        }
        self.edges.push(Edge {
            from: module.id,
            to: id,
            category: edge_category::DEFINES,
            confidence: Confidence::Strong,
            cells: Vec::new(),
        });
    }

    /// One query site: the entity `name` (Medium unless this file declared
    /// it, then the declaration's Strong node) and an ACCESSES_DATA edge
    /// from the enclosing function `from` (LE.4a: a statement site hangs on
    /// its function), its evidence at the site's 0-based `line`. The nav
    /// records no parent, as for the declaration.
    fn push_ecto_access(&mut self, from: NodeId, name: &str, rule: &str, line: u32, repo: RepoId) {
        let (qname, id) = ecto_entity(name, repo);
        self.ecto.repo_sites += 1;
        if self.data_entities.insert(id) {
            self.nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            });
            self.nav.record(id, name, &qname, node_kind::DATA_ENTITY, None);
        }
        if self.access_edges.insert((from, id)) {
            let mut edge = Edge {
                from,
                to: id,
                category: edge_category::ACCESSES_DATA,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            };
            evidence::attach(&mut edge, Evidence::emitter("parser:elixir").rule(rule).line(line));
            self.edges.push(edge);
        }
    }
}

fn scan_phoenix_routes(source: &str, repo: RepoId, module_id: NodeId, acc: &mut Acc) {
    let bytes = source.as_bytes();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut scope_stack: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        // Match word at i, then dispatch.
        if !is_word_start(bytes, i) {
            i += 1;
            continue;
        }
        let word_end = word_end(bytes, i);
        let word = &source[i..word_end];
        match word {
            "scope" => {
                let rest = &source[word_end..];
                let prefix = first_quoted(rest).unwrap_or_default();
                scope_stack.push(prefix);
                // Advance past the end of the line or next do (rough).
                if let Some(off) = rest.find(" do") {
                    i = word_end + off + 3;
                    continue;
                }
                i = word_end;
            }
            "end" => {
                // End *might* close a scope — we can't tell precisely without a full parse.
                // Pop lazily: only if non-empty.
                if !scope_stack.is_empty() {
                    scope_stack.pop();
                }
                i = word_end;
            }
            "get" | "post" | "put" | "patch" | "delete" | "head" | "options" => {
                let method = word.to_ascii_uppercase();
                let rest = &source[word_end..];
                if let Some(path) = first_quoted(rest) {
                    let full = join_scope(&scope_stack, &path);
                    // Handler action is the last atom on the route line, e.g.
                    // `get "/users", UserController, :index` -> `index`.
                    let action = route_action(rest);
                    let route_name = format!("{method} {full}");
                    if seen.insert(route_name.clone()) {
                        emit_phoenix_route(
                            &method,
                            &full,
                            action.as_deref(),
                            repo,
                            module_id,
                            line_of(source, i),
                            acc,
                        );
                    }
                }
                i = word_end;
            }
            "resources" => {
                let rest = &source[word_end..];
                if let Some(path) = first_quoted(rest) {
                    let full = join_scope(&scope_stack, &path);
                    for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
                        let route_name = format!("{m} {full}");
                        if seen.insert(route_name.clone()) {
                            // `resources` derives standard RESTful actions from the
                            // controller implicitly; no explicit action to link.
                            emit_phoenix_route(m, &full, None, repo, module_id, line_of(source, i), acc);
                        }
                    }
                }
                i = word_end;
            }
            _ => {
                i = word_end;
            }
        }
    }
}

fn is_word_start(bytes: &[u8], i: usize) -> bool {
    let c = bytes[i];
    if !(c.is_ascii_alphabetic() || c == b'_') {
        return false;
    }
    if i == 0 {
        return true;
    }
    let p = bytes[i - 1];
    !(p.is_ascii_alphanumeric() || p == b'_' || p == b'.' || p == b':')
}

fn word_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    i
}

fn first_quoted(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\n' {
            return None;
        }
        if c == b'"' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'"' {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() {
                return Some(s[start..j].to_string());
            }
            return None;
        }
        i += 1;
    }
    None
}

/// `line` is the route line's 0-based row: the site of its HANDLED_BY ref
/// (LC.3b).
fn emit_phoenix_route(
    method: &str,
    path: &str,
    action: Option<&str>,
    repo: RepoId,
    module_id: NodeId,
    line: u32,
    acc: &mut Acc,
) {
    let route_name = format!("{method} {path}");
    let route_id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::ROUTE, &route_name);
    acc.nodes.push(Node {
        id: route_id,
        repo,
        confidence: Confidence::Medium,
        cells: vec![Cell {
            kind: cell_type::ROUTE_METHOD,
            payload: CellPayload::Text(method.to_string()),
        }],
    });
    acc.nav
        .record(route_id, &route_name, &route_name, node_kind::ROUTE, None);

    // HANDLED_BY: link the ROUTE to its controller action function. The action
    // is a bare `def` in the controller module; resolve_refs binds it via the
    // global-by-name fallback (Elixir modules are MODULE/PACKAGE nodes, not
    // CLASS, so the Attribute/class-method path can't resolve them).
    if let Some(action) = action
        && !action.is_empty()
    {
        acc.refs.push(UnresolvedRef {
            from: route_id,
            from_module: module_id,
            qualifier: CallQualifier::Bare(action.to_string()),
            category: edge_category::HANDLED_BY,
            line,
        });
    }
}

/// Extract the controller action from a Phoenix route line. Given the text
/// following the verb (e.g. `"/users", UserController, :index`), returns the
/// trailing atom's name (`index`). Returns `None` when no `:atom` action is
/// present on the same line (only scans up to the first newline).
fn route_action(rest: &str) -> Option<String> {
    let line = rest.split('\n').next().unwrap_or(rest);
    let bytes = line.as_bytes();
    // Skip past the quoted path so a `:param` inside it (e.g. "/users/:id")
    // isn't mistaken for the action atom.
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b'"' {
        i += 1;
    }
    if i < bytes.len() {
        i += 1; // opening quote
        while i < bytes.len() && bytes[i] != b'"' {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
            } else {
                i += 1;
            }
        }
        if i < bytes.len() {
            i += 1; // closing quote
        }
    }
    let mut last: Option<String> = None;
    while i < bytes.len() {
        if bytes[i] == b':' && i + 1 < bytes.len() {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j > start {
                last = Some(line[start..j].to_string());
            }
            i = j;
        } else {
            i += 1;
        }
    }
    last
}

fn file_cells(root: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*root, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(glia_doc::position_json(root, file_rel)),
        },
    ]
}

fn entity_cells(node: &TsNode, src: &[u8], file_rel: &str) -> Vec<Cell> {
    entity_cells_with_doc(node, src, file_rel, DocChoice::Leading)
}

/// CODE + POSITION, plus the DOC cell `doc` selects.
fn entity_cells_with_doc(node: &TsNode, src: &[u8], file_rel: &str, doc: DocChoice) -> Vec<Cell> {
    let mut cells = vec![
        Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text(text_of(*node, src).to_string()),
        },
        Cell {
            kind: cell_type::POSITION,
            payload: CellPayload::Json(glia_doc::position_json(node, file_rel)),
        },
    ];
    let text = match doc {
        DocChoice::Leading => glia_doc::leading_doc(node, src),
        DocChoice::Text(t) if !t.is_empty() => Some(t),
        DocChoice::Text(_) | DocChoice::Hidden | DocChoice::Omit => None,
    };
    if let Some(doc) = text {
        cells.push(Cell {
            kind: cell_type::DOC,
            payload: CellPayload::Text(doc),
        });
    }
    cells
}

// ---------------------------------------------------------------------------
// Documentation attributes (`@doc` / `@moduledoc`) -> DOC cells (LA.7a)
// ---------------------------------------------------------------------------
//
// Elixir documentation is not a comment: it is a module attribute in the body,
// which `glia_doc::leading_doc` (a preceding-comment walk) never sees.
// tree-sitter-elixir 0.3 parses `@doc "x"` as
// `unary_operator(operator: "@", operand: call(target: identifier "doc",
// arguments(string | sigil | boolean)))`.

/// One documentation attribute, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DocAttr {
    /// `@doc "..."` / heredoc / `~S"""..."""`, cleaned.
    Doc(String),
    /// `@moduledoc "..."`, cleaned.
    ModuleDoc(String),
    /// `@doc false`: the author hid the function.
    Hidden,
    /// `@moduledoc false`: the author hid the module.
    ModuleHidden,
}

/// Where a node's DOC cell comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DocChoice {
    /// No attribute decided it: the leading-comment walk (comment-only defs).
    Leading,
    /// An attribute's text, authoritative over any comment. Empty = no DOC.
    Text(String),
    /// `@doc false` / `@moduledoc false`: no DOC, not even from a comment.
    Hidden,
    /// A later clause of a function whose doc an attribute already decided on
    /// the first clause: no DOC, so the merged node carries exactly one.
    Omit,
}

impl Acc {
    /// Tally an attached / hidden doc for the fired-on marker.
    fn count_doc(&mut self, doc: &DocChoice, module: bool) {
        match doc {
            DocChoice::Text(t) if !t.is_empty() => {
                if module {
                    self.doc_moduledoc += 1;
                } else {
                    self.doc_attached += 1;
                }
            }
            DocChoice::Hidden => self.doc_hidden += 1,
            DocChoice::Leading | DocChoice::Text(_) | DocChoice::Omit => {}
        }
    }
}

/// The `call` operand of a module attribute (`@name ...`), if `node` is one.
fn attribute_call(node: TsNode) -> Option<TsNode> {
    if node.kind() != "unary_operator" {
        return None;
    }
    let op = node.child_by_field_name("operator")?;
    if op.kind() != "@" {
        return None;
    }
    let operand = node.child_by_field_name("operand")?;
    (operand.kind() == "call").then_some(operand)
}

/// Any `@attr ...` statement (`@spec`, `@impl`, `@decorate`, `@typedoc`, ...).
fn module_attribute(node: TsNode) -> bool {
    attribute_call(node).is_some()
}

/// A `@doc` / `@moduledoc` attribute's value. `None` for other attributes and
/// for doc forms that carry no text (`@doc since: "1.0"` metadata).
fn doc_attr_text(node: TsNode, src: &[u8]) -> Option<DocAttr> {
    let call = attribute_call(node)?;
    let target = call.child_by_field_name("target")?;
    if target.kind() != "identifier" {
        return None;
    }
    let module = match text_of(target, src) {
        "doc" => false,
        "moduledoc" => true,
        _ => return None,
    };
    let args = find_args(call)?;
    let mut cursor = args.walk();
    let value = args.named_children(&mut cursor).next()?;
    match value.kind() {
        "string" | "sigil" => {
            let text = clean_doc(&quoted_text(value, src));
            Some(if module {
                DocAttr::ModuleDoc(text)
            } else {
                DocAttr::Doc(text)
            })
        }
        "boolean" if text_of(value, src) == "false" => Some(if module {
            DocAttr::ModuleHidden
        } else {
            DocAttr::Hidden
        }),
        _ => None,
    }
}

/// The literal text of a `string` / `sigil` (single-line or `"""` heredoc):
/// its `quoted_content` runs, escapes decoded, interpolations kept verbatim.
fn quoted_text(node: TsNode, src: &[u8]) -> String {
    let mut out = String::new();
    let mut cursor = node.walk();
    for part in node.named_children(&mut cursor) {
        match part.kind() {
            "quoted_content" | "interpolation" => out.push_str(text_of(part, src)),
            "escape_sequence" => {
                let esc = text_of(part, src);
                match esc {
                    "\\n" | "\\t" | "\\r" => out.push(' '),
                    _ => match esc.strip_prefix('\\') {
                        Some(rest) if rest.chars().count() == 1 => out.push_str(rest),
                        _ => out.push_str(esc),
                    },
                }
            }
            _ => {}
        }
    }
    out
}

/// Dedent, trim and collapse a doc body to one line (the shape
/// `glia_doc::leading_doc` produces: lines joined by single spaces), then
/// cap at [`glia_doc::DOC_MAX`] on a char boundary. Collapsing every
/// whitespace run subsumes the heredoc dedent: each line's common indent goes
/// with the rest of its leading whitespace.
fn clean_doc(raw: &str) -> String {
    let joined = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.len() <= glia_doc::DOC_MAX {
        return joined;
    }
    let mut end = glia_doc::DOC_MAX;
    while !joined.is_char_boundary(end) {
        end -= 1;
    }
    joined[..end].trim_end().to_string()
}

/// The function name of a `def` / `defp` call, if `node` is one.
fn def_call_name(node: TsNode, src: &[u8]) -> Option<String> {
    if node.kind() != "call" {
        return None;
    }
    let target = node.child_by_field_name("target")?;
    if !matches!(text_of(target, src), "def" | "defp") {
        return None;
    }
    let name = extract_def_name(find_args(node)?, src);
    (!name.is_empty()).then_some(name)
}

/// The DOC choice for a `defmodule` / `defprotocol` node, from the
/// `@moduledoc` among its body's direct children (the last one wins, as in
/// Elixir). No `@moduledoc` keeps the leading-comment fallback. Nested module
/// bodies are not entered, so their `@moduledoc` never leaks outward.
fn moduledoc_choice(body: TsNode, src: &[u8]) -> DocChoice {
    let mut choice = DocChoice::Leading;
    let mut cursor = body.walk();
    for child in body.named_children(&mut cursor) {
        match doc_attr_text(child, src) {
            Some(DocAttr::ModuleDoc(text)) => choice = DocChoice::Text(text),
            Some(DocAttr::ModuleHidden) => choice = DocChoice::Hidden,
            Some(DocAttr::Doc(_) | DocAttr::Hidden) | None => {}
        }
    }
    choice
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoId {
        RepoId(1)
    }

    /// Every DOC text on the node(s) named `name` of `kind`, across all the
    /// `Node` entries sharing its id (multi-clause functions push one per clause).
    fn docs_of(fp: &FileParse, kind: glia_core::NodeKindId, name: &str) -> Vec<String> {
        let ids: Vec<NodeId> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, n)| n.as_str() == name && fp.nav.kind_by_id.get(*id) == Some(&kind))
            .map(|(id, _)| *id)
            .collect();
        assert!(!ids.is_empty(), "no {kind:?} node named {name}");
        fp.nodes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .flat_map(|n| n.cells.iter())
            .filter(|c| c.kind == cell_type::DOC)
            .filter_map(|c| match &c.payload {
                CellPayload::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    /// The LA.7a fixture file (bench/substrate-gap/fixtures/elixir-docs).
    const ACCOUNTS: &str = r#"defmodule MyApp.Accounts do
  @moduledoc """
  Account management context.
  """

  @doc """
  Fetches a user by id.
  """
  @spec get_user(integer) :: map
  def get_user(id), do: %{id: id}

  @doc "Creates a user."
  def create_user(attrs), do: attrs

  # Hidden helper comment.
  @doc false
  def internal(x), do: x

  # Plain comment above.
  def comment_only(x), do: x
end
"#;

    #[test]
    fn doc_heredoc_attaches_through_spec() {
        let fp = parse_file(ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "get_user"), vec!["Fetches a user by id."]);
    }

    #[test]
    fn doc_single_line_string() {
        let fp = parse_file(ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "create_user"), vec!["Creates a user."]);
    }

    #[test]
    fn moduledoc_on_package() {
        let fp = parse_file(ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(
            docs_of(&fp, node_kind::PACKAGE, "Accounts"),
            vec!["Account management context."]
        );
    }

    #[test]
    fn doc_false_hides_even_the_comment_above() {
        let fp = parse_file(ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::FUNCTION, "internal").is_empty());
        // Control: the leading-comment path still documents a comment-only def.
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "comment_only"), vec!["Plain comment above."]);

        // A comment BETWEEN `@doc false` and the def is directly above the def,
        // so `leading_doc` would pick it up; `@doc false` must still win.
        let source = r#"
defmodule M do
  @doc false
  # Would be picked up by the comment walk.
  def hidden(x), do: x
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::FUNCTION, "hidden").is_empty());
    }

    #[test]
    fn moduledoc_false_hides_package_comment() {
        let source = r#"
# Comment above the module.
defmodule M do
  @moduledoc false
  def f(x), do: x
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::PACKAGE, "M").is_empty());
    }

    #[test]
    fn doc_survives_spec_impl_and_comments() {
        let source = r#"
defmodule M do
  @doc "Handles a call."
  # a comment between
  @impl true
  @spec handle(term) :: :ok
  def handle(_), do: :ok
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "handle"), vec!["Handles a call."]);
    }

    #[test]
    fn stray_doc_before_a_statement_attaches_to_nothing() {
        let source = r#"
defmodule M do
  @doc "Stray."
  alias Foo.Bar
  def after_alias(x), do: x

  @doc "Also stray."
  defmodule Inner do
    def inner(x), do: x
  end

  def after_inner(x), do: x
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::FUNCTION, "after_alias").is_empty());
        assert!(docs_of(&fp, node_kind::FUNCTION, "inner").is_empty());
        assert!(docs_of(&fp, node_kind::FUNCTION, "after_inner").is_empty());
        assert!(docs_of(&fp, node_kind::PACKAGE, "Inner").is_empty());
    }

    #[test]
    fn multi_clause_function_gets_one_doc() {
        let source = r#"
defmodule M do
  @doc "Gets a thing."
  def get(1), do: :one
  # Second clause comment.
  def get(n), do: n
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "get"), vec!["Gets a thing."]);
    }

    #[test]
    fn nested_module_moduledoc_stays_on_its_own_package() {
        let source = r#"
defmodule Outer do
  defmodule Inner do
    @moduledoc "Inner docs."
  end
end
"#;
        let fp = parse_file(source, "lib/o.ex", "lib::o", repo()).unwrap();
        assert!(docs_of(&fp, node_kind::PACKAGE, "Outer").is_empty());
        assert_eq!(docs_of(&fp, node_kind::PACKAGE, "Inner"), vec!["Inner docs."]);
    }

    #[test]
    fn protocol_moduledoc_on_interface() {
        let source = r#"
defprotocol Size do
  @moduledoc """
  Computes the size of a data structure.
  """
  def size(data)
end
"#;
        let fp = parse_file(source, "lib/size.ex", "lib::size", repo()).unwrap();
        assert_eq!(
            docs_of(&fp, node_kind::INTERFACE, "Size"),
            vec!["Computes the size of a data structure."]
        );
    }

    #[test]
    fn sigil_heredoc_escapes_and_dedent() {
        let source = r#"
defmodule M do
  @doc ~S"""
      Indented   first line.
        Deeper second line, café.
  """
  def a(x), do: x

  @doc "Say \"hi\" to #{name}."
  def b(name), do: name
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(
            docs_of(&fp, node_kind::FUNCTION, "a"),
            vec!["Indented first line. Deeper second line, café."]
        );
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "b"), vec!["Say \"hi\" to #{name}."]);
    }

    #[test]
    fn long_utf8_doc_capped_on_char_boundary() {
        let body = "é".repeat(glia_doc::DOC_MAX);
        let source = format!("defmodule M do\n  @doc \"{body}\"\n  def a(x), do: x\nend\n");
        let fp = parse_file(&source, "lib/m.ex", "lib::m", repo()).unwrap();
        let docs = docs_of(&fp, node_kind::FUNCTION, "a");
        assert_eq!(docs.len(), 1);
        assert!(docs[0].len() <= glia_doc::DOC_MAX);
        assert!(docs[0].chars().all(|c| c == 'é'));
        assert_eq!(docs[0].chars().count(), glia_doc::DOC_MAX / 2);
    }

    #[test]
    fn doc_metadata_keyword_form_is_not_text() {
        // `@doc since: "1.0"` carries metadata, not text: it neither documents
        // the def nor clears the real `@doc` above it.
        let source = r#"
defmodule M do
  @doc "Real doc."
  @doc since: "1.0"
  def a(x), do: x
end
"#;
        let fp = parse_file(source, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "a"), vec!["Real doc."]);
    }

    #[test]
    fn doc_attr_counters_match_the_fixture_marker() {
        let lang: tree_sitter::Language = tree_sitter_elixir::LANGUAGE.into();
        let mut parser = Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(ACCOUNTS, None).unwrap();
        let mut acc = Acc::default();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::accounts");
        visit_top(
            tree.root_node(),
            ACCOUNTS.as_bytes(),
            "lib/accounts.ex",
            "lib::accounts",
            module_id,
            repo(),
            &mut acc,
        );
        assert_eq!(
            (acc.doc_attached, acc.doc_moduledoc, acc.doc_hidden),
            (2, 1, 1),
            "marker: [doc] elixir attrs attached=2 moduledoc=1 hidden=1"
        );
    }

    #[test]
    fn module_and_functions() {
        let source = r#"
defmodule MyApp.Users do
  def get_user(id) do
    Repo.get(User, id)
  end

  defp validate(user) do
    :ok
  end
end
"#;
        let fp = parse_file(source, "lib/users.ex", "lib::users", repo()).unwrap();
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::PACKAGE).count(), 1);
        assert_eq!(fp.nav.kind_by_id.values().filter(|k| **k == node_kind::FUNCTION).count(), 2);
    }

    #[test]
    fn imports() {
        let source = r#"
defmodule MyApp.Web do
  import Plug.Conn
  alias MyApp.Repo
  use Phoenix.Controller
end
"#;
        let fp = parse_file(source, "lib/web.ex", "lib::web", repo()).unwrap();
        assert_eq!(fp.imports.len(), 3);

        // Regression: `import`/`alias`/`use` live *inside* the `defmodule`, but the
        // emitted `from_module` must be the file-stem MODULE qname (`lib::web`) — the
        // key the graph registers in `module_by_qname` — NOT the enclosing PACKAGE
        // qname (`lib::web::MyApp.Web`). Otherwise `resolve_imports_python`
        // early-continues and no elixir import ever resolves.
        assert!(
            fp.imports.iter().all(|i| i.from_module == "lib::web"),
            "from_module must be the file MODULE qname, got: {:?}",
            fp.imports.iter().map(|i| &i.from_module).collect::<Vec<_>>()
        );

        // The dotted alias path is preserved so the graph's tail fallback can split
        // it to the short module name it binds against.
        let paths: Vec<&str> = fp
            .imports
            .iter()
            .filter_map(|i| match &i.target {
                ImportTarget::Module { path, .. } => Some(path.as_str()),
                _ => None,
            })
            .collect();
        assert!(paths.contains(&"Plug.Conn"), "paths: {paths:?}");
        assert!(paths.contains(&"MyApp.Repo"), "paths: {paths:?}");
    }

    #[test]
    fn defmodule_recorded_by_short_name() {
        // `defmodule MyApp.Repo` records short name `Repo` (how Elixir refers to it
        // after `alias MyApp.Repo`) so the graph's import tail fallback binds an
        // in-repo alias by its unique short name — while the full dotted path stays
        // in the qname for uniqueness.
        let source = r#"
defmodule MyApp.Repo do
  def all(q), do: q
end
"#;
        let fp = parse_file(source, "lib/repo.ex", "lib::repo", repo()).unwrap();
        let pkg = fp
            .nav
            .name_by_id
            .iter()
            .find(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::PACKAGE))
            .expect("defmodule package node");
        assert_eq!(pkg.1, "Repo", "short name should be the last dotted segment");
        assert_eq!(
            fp.nav.qname_by_id.get(pkg.0).map(String::as_str),
            Some("lib::repo::MyApp.Repo"),
            "qname keeps the full dotted path"
        );
    }

    #[test]
    fn phoenix_routes_basic() {
        let source = r#"
defmodule MyAppWeb.Router do
  use MyAppWeb, :router

  scope "/api", MyAppWeb do
    get "/users", UserController, :index
    post "/users", UserController, :create
    put "/users/:id", UserController, :update
    delete "/users/:id", UserController, :delete
    resources "/posts", PostController
  end
end
"#;
        let fp = parse_file(source, "lib/router.ex", "lib::router", repo()).unwrap();
        let route_names: Vec<&str> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n.as_str())
            .collect();
        assert!(route_names.contains(&"GET /api/users"));
        assert!(route_names.contains(&"POST /api/users"));
        assert!(route_names.contains(&"PUT /api/users/:id"));
        assert!(route_names.contains(&"DELETE /api/users/:id"));
        assert!(route_names.contains(&"GET /api/posts"));
        assert!(route_names.contains(&"POST /api/posts"));
    }

    #[test]
    fn calls_detected() {
        let source = r#"
defmodule MyApp.Service do
  def run(data) do
    validate(data)
    Repo.insert(data)
  end
end
"#;
        let fp = parse_file(source, "lib/service.ex", "lib::service", repo()).unwrap();
        assert!(fp.calls.iter().any(|c| matches!(&c.qualifier, CallQualifier::Bare(n) if n == "validate")));
        assert!(fp.calls.iter().any(|c| matches!(&c.qualifier, CallQualifier::Attribute { base, name } if base == "Repo" && name == "insert")));
    }

    #[test]
    fn handled_by_refs_link_route_to_action() {
        let source = r#"
defmodule MyAppWeb.Router do
  use MyAppWeb, :router

  scope "/api", MyAppWeb do
    get "/users", UserController, :index
    post "/users", UserController, :create
    put "/users/:id", UserController, :update
  end
end
"#;
        let fp = parse_file(source, "lib/router.ex", "lib::router", repo()).unwrap();

        // One HANDLED_BY ref per explicit-verb route, qualifier = Bare(action).
        let actions: Vec<&str> = fp
            .refs
            .iter()
            .filter(|r| r.category == edge_category::HANDLED_BY)
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        assert!(actions.contains(&"index"), "actions: {actions:?}");
        assert!(actions.contains(&"create"), "actions: {actions:?}");
        // `:id` inside the path must not be captured as the action.
        assert!(actions.contains(&"update"), "actions: {actions:?}");
        assert!(!actions.contains(&"id"), "path param leaked: {actions:?}");

        // The ref's `from` must be the ROUTE node id (GET /api/users).
        let route_id = fp
            .nav
            .name_by_id
            .iter()
            .find(|(id, n)| {
                n.as_str() == "GET /api/users"
                    && fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE)
            })
            .map(|(id, _)| *id)
            .expect("route node exists");
        assert!(
            fp.refs.iter().any(|r| r.from == route_id
                && r.category == edge_category::HANDLED_BY
                && matches!(&r.qualifier, CallQualifier::Bare(n) if n == "index")),
            "expected HANDLED_BY from GET /api/users -> index"
        );
    }

    // --- client HTTP calls (HTTPoison / Tesla / Req / Finch) -----------------

    /// Every ENDPOINT display name the parse produced, sorted.
    fn endpoint_names(fp: &FileParse) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ENDPOINT))
            .map(|(_, n)| n.clone())
            .collect();
        v.sort();
        v
    }

    /// True if a CALLS edge runs from the FUNCTION named `from` to the ENDPOINT
    /// named `to`.
    fn has_calls_edge(fp: &FileParse, from: &str, to: &str) -> bool {
        let id_of = |name: &str, kind| {
            fp.nav
                .name_by_id
                .iter()
                .find(|(id, n)| n.as_str() == name && fp.nav.kind_by_id.get(*id) == Some(&kind))
                .map(|(id, _)| *id)
        };
        let (Some(f), Some(t)) = (
            id_of(from, node_kind::FUNCTION),
            id_of(to, node_kind::ENDPOINT),
        ) else {
            return false;
        };
        fp.edges
            .iter()
            .any(|e| e.from == f && e.to == t && e.category == edge_category::CALLS)
    }

    #[test]
    fn httpoison_get_emits_endpoint() {
        let source = r#"
defmodule ApiClient do
  def fetch_user(id) do
    HTTPoison.get("http://users-svc/api/users/#{id}")
  end
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        // Host stripped by `url_to_path`; `#{id}` reconstructed as `${…}` so the
        // graph's `normalise_http_path` collapses it to `{}` and it pairs with a
        // server route `/api/users/{id}`.
        assert_eq!(endpoint_names(&fp), vec!["GET /api/users/${…}".to_string()]);
        assert!(
            has_calls_edge(&fp, "fetch_user", "GET /api/users/${…}"),
            "expected CALLS from the enclosing def to the ENDPOINT"
        );
        // An interpolated path is Medium — the concrete segment is unknown.
        let ep = fp
            .nodes
            .iter()
            .find(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
            .expect("endpoint node");
        assert_eq!(ep.confidence, Confidence::Medium);
        assert!(
            ep.cells.iter().any(|c| c.kind == cell_type::ENDPOINT_HIT),
            "ENDPOINT must carry an ENDPOINT_HIT cell"
        );
    }

    #[test]
    fn tesla_client_first_arg_form_emits_endpoint() {
        let source = r#"
defmodule ApiClient do
  def create_user(body) do
    Tesla.post(client(), "/api/users", body)
  end

  defp client, do: Tesla.client([])
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        // The URL is the SECOND argument: `first_string_arg` scans DIRECT
        // arguments for the first string literal, so `client()` is skipped.
        assert_eq!(endpoint_names(&fp), vec!["POST /api/users".to_string()]);
        assert!(has_calls_edge(&fp, "create_user", "POST /api/users"));
        // `Tesla.client([])` is not a verb -> no endpoint from the private helper.
        let ep = fp
            .nodes
            .iter()
            .find(|n| fp.nav.kind_by_id.get(&n.id) == Some(&node_kind::ENDPOINT))
            .expect("endpoint node");
        assert_eq!(ep.confidence, Confidence::Strong);
    }

    #[test]
    fn req_keyword_url_and_finch_atom_verb_emit_endpoints() {
        let source = r#"
defmodule ApiClient do
  def list_orders do
    Req.get!(url: "http://orders-svc/api/orders")
  end

  def remove_user(id) do
    Finch.build(:delete, "http://users-svc/api/users/#{id}")
  end
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        assert_eq!(
            endpoint_names(&fp),
            vec![
                "DELETE /api/users/${…}".to_string(),
                "GET /api/orders".to_string(),
            ]
        );
        // Req's URL rides a `url:` keyword, whose `keyword` node text is
        // `"url: "` — colon and trailing space.
        assert!(has_calls_edge(&fp, "list_orders", "GET /api/orders"));
        // Finch's verb is the leading atom, not the function name (`build`).
        assert!(has_calls_edge(&fp, "remove_user", "DELETE /api/users/${…}"));
    }

    #[test]
    fn elixir_client_calls_emit_no_route() {
        // `scan_phoenix_routes` is a raw-source WORD scanner: a bare `get "/x"`
        // mints a ROUTE. Every client call here is qualified (`HTTPoison.get`),
        // and `is_word_start` rejects a word preceded by `.` or `:`, so none of
        // them may be mistaken for a Phoenix route declaration.
        let source = r#"
defmodule ApiClient do
  def fetch_user(id) do
    HTTPoison.get("http://users-svc/api/users/#{id}")
  end

  def create_user(body) do
    Tesla.post(client(), "/api/users", body)
  end

  def list_orders do
    Req.get!(url: "http://orders-svc/api/orders")
  end

  def remove_user(id) do
    Finch.build(:delete, "http://users-svc/api/users/#{id}")
  end

  defp client, do: Tesla.client([])
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        let routes: Vec<&String> = fp
            .nav
            .name_by_id
            .iter()
            .filter(|(id, _)| fp.nav.kind_by_id.get(*id) == Some(&node_kind::ROUTE))
            .map(|(_, n)| n)
            .collect();
        assert!(routes.is_empty(), "phantom ROUTEs minted: {routes:?}");
        assert_eq!(endpoint_names(&fp).len(), 4, "{:?}", endpoint_names(&fp));
    }

    #[test]
    fn elixir_non_url_string_is_dropped() {
        let source = r#"
defmodule ApiClient do
  # Non-HTTP receiver: `Cache` is not a known client module.
  def cached(id), do: Cache.get("user." <> id)

  # Known client module, but the argument is not a URL path.
  def bad_url, do: HTTPoison.get("users-svc")

  # Known client module, unknown verb.
  def started, do: HTTPoison.start()
end
"#;
        let fp = parse_file(source, "lib/api_client.ex", "lib::api_client", repo()).unwrap();
        assert!(
            endpoint_names(&fp).is_empty(),
            "no endpoint may be minted: {:?}",
            endpoint_names(&fp)
        );
    }

    // --- DH.17b: behaviours, @behaviour, compile_env DI, ExUnit tests -------

    /// The matrix/elixir/impl probe's two files.
    const NOTIFIER: &str = "defmodule Shop.Notifier do\n  @callback notify(String.t()) :: :ok\nend\n";
    const EMAIL_NOTIFIER: &str = "defmodule Shop.EmailNotifier do\n  @behaviour Shop.Notifier\n\n  @impl true\n  def notify(_msg), do: :ok\nend\n";
    /// The matrix/elixir/injects probe's two files.
    const ACCOUNTS_DI: &str = "defmodule Shop.Accounts do\n  @repo Application.compile_env(:shop, :user_repo, Shop.PgUserRepo)\n\n  def list_users, do: @repo.all()\nend\n";
    const USER_REPO: &str = "defmodule Shop.UserRepo do\n  @callback all() :: [map()]\nend\n\ndefmodule Shop.PgUserRepo do\n  @behaviour Shop.UserRepo\n\n  def all, do: []\nend\n";
    /// The matrix/elixir/tests probe's test file.
    const CART_TEST: &str = "defmodule CartTest do\n  use ExUnit.Case\n\n  test \"sums\" do\n    assert Cart.total([1, 2]) == 3\n  end\nend\n";

    /// Re-parse `source` with the Acc kept, for the marker counters.
    fn counts_of(source: &str, file: &str, module_qname: &str) -> TypeCounts {
        let lang: tree_sitter::Language = tree_sitter_elixir::LANGUAGE.into();
        let mut parser = Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, module_qname);
        let mut acc = Acc {
            module_qname: module_qname.to_string(),
            module_id: Some(module_id),
            ..Acc::default()
        };
        visit_top(tree.root_node(), source.as_bytes(), file, module_qname, module_id, repo(), &mut acc);
        acc.types
    }

    /// The kind the nav recorded for `qname`, if any.
    fn kind_of(fp: &FileParse, qname: &str) -> Option<glia_core::NodeKindId> {
        fp.nav
            .qname_by_id
            .iter()
            .find(|(_, q)| q.as_str() == qname)
            .and_then(|(id, _)| fp.nav.kind_by_id.get(id).copied())
    }

    /// `(category, Bare name)` of every ref whose `from` is the node `qname`.
    fn refs_from(fp: &FileParse, qname: &str) -> Vec<(glia_core::EdgeCategoryId, String)> {
        let ids: Vec<NodeId> = fp
            .nav
            .qname_by_id
            .iter()
            .filter(|(_, q)| q.as_str() == qname)
            .map(|(id, _)| *id)
            .collect();
        fp.refs
            .iter()
            .filter(|r| ids.contains(&r.from))
            .filter_map(|r| match &r.qualifier {
                CallQualifier::Bare(n) => Some((r.category, n.clone())),
                _ => None,
            })
            .collect()
    }

    /// The qnames of every FUNCTION in the parse, sorted.
    fn functions(fp: &FileParse) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::FUNCTION)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).cloned())
            .collect();
        v.sort();
        v
    }

    /// The CallSites whose `from` is the FUNCTION `qname`.
    fn calls_from(fp: &FileParse, qname: &str) -> Vec<CallQualifier> {
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, qname);
        fp.calls.iter().filter(|c| c.from == id).map(|c| c.qualifier.clone()).collect()
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute { base: base.to_string(), name: name.to_string() }
    }

    #[test]
    fn behaviour_module_is_interface() {
        let fp = parse_file(NOTIFIER, "lib/notifier.ex", "lib::notifier", repo()).unwrap();
        assert_eq!(kind_of(&fp, "lib::notifier::Shop.Notifier"), Some(node_kind::INTERFACE));
        // Same qname, same short name: only the kind (and so the NodeId) moved.
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::INTERFACE, "lib::notifier::Shop.Notifier");
        assert_eq!(fp.nav.name_by_id.get(&id).map(String::as_str), Some("Notifier"));
        assert!(fp.nodes.iter().any(|n| n.id == id));
        assert!(
            fp.edges.iter().any(|e| e.to == id && e.category == edge_category::CONTAINS),
            "the file MODULE still CONTAINS the behaviour"
        );
        // `@macrocallback` declares a contract too.
        let src = "defmodule Shop.Dsl do\n  @macrocallback build(term) :: Macro.t()\nend\n";
        let fp = parse_file(src, "lib/dsl.ex", "lib::dsl", repo()).unwrap();
        assert_eq!(kind_of(&fp, "lib::dsl::Shop.Dsl"), Some(node_kind::INTERFACE));
    }

    #[test]
    fn plain_module_stays_package() {
        // Every module of the existing docs tests (no `@callback`) is a PACKAGE.
        let fp = parse_file(ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(kind_of(&fp, "lib::accounts::MyApp.Accounts"), Some(node_kind::PACKAGE));
        // A module that only implements a behaviour stays a PACKAGE.
        let fp = parse_file(EMAIL_NOTIFIER, "lib/email_notifier.ex", "lib::email_notifier", repo()).unwrap();
        assert_eq!(kind_of(&fp, "lib::email_notifier::Shop.EmailNotifier"), Some(node_kind::PACKAGE));
        // A nested module's `@callback` makes the NESTED module the contract,
        // never its parent (`body_shape` reads direct children only).
        let src = "defmodule Outer do\n  defmodule Port do\n    @callback x() :: :ok\n  end\nend\n";
        let fp = parse_file(src, "lib/outer.ex", "lib::outer", repo()).unwrap();
        assert_eq!(kind_of(&fp, "lib::outer::Outer"), Some(node_kind::PACKAGE));
        assert_eq!(kind_of(&fp, "lib::outer::Outer::Port"), Some(node_kind::INTERFACE));
    }

    #[test]
    fn behaviour_with_helpers_stays_package() {
        // Callbacks plus a default helper: the graph indexes FUNCTION children
        // for call resolution only under a PACKAGE, so this stays one; the
        // `@behaviour` IMPLEMENTS of its implementations still land on it.
        let src = r#"
defmodule Shop.Worker do
  @callback perform(map) :: :ok
  def enqueue(args), do: perform_later(args)
  defp perform_later(args), do: args
end
"#;
        let fp = parse_file(src, "lib/worker.ex", "lib::worker", repo()).unwrap();
        assert_eq!(kind_of(&fp, "lib::worker::Shop.Worker"), Some(node_kind::PACKAGE));
        assert_eq!(counts_of(src, "lib/worker.ex", "lib::worker").behaviours, 0);
        // A `defmacro __using__` that injects `@behaviour` + default defs is
        // still a pure contract: the `quote` body is not the module's.
        let src = r#"
defmodule Shop.Handler do
  @callback handle(term) :: :ok
  defmacro __using__(_opts) do
    quote do
      @behaviour Shop.Handler
      def handle(_), do: :ok
      defoverridable handle: 1
    end
  end
end
"#;
        let fp = parse_file(src, "lib/handler.ex", "lib::handler", repo()).unwrap();
        assert_eq!(kind_of(&fp, "lib::handler::Shop.Handler"), Some(node_kind::INTERFACE));
        assert!(fp.refs.is_empty(), "a quoted @behaviour is the user's, not this module's");
    }

    #[test]
    fn behaviour_attribute_emits_implements_ref() {
        let fp = parse_file(EMAIL_NOTIFIER, "lib/email_notifier.ex", "lib::email_notifier", repo()).unwrap();
        assert_eq!(
            refs_from(&fp, "lib::email_notifier::Shop.EmailNotifier"),
            vec![(edge_category::IMPLEMENTS, "Notifier".to_string())]
        );
        let r = &fp.refs[0];
        assert_eq!(r.from_module, NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, "lib::email_notifier"));
        assert_eq!(r.line, 1, "the @behaviour line, 0-based");
        // An Erlang behaviour (`:gen_server`) names no in-repo module.
        let src = "defmodule Shop.Srv do\n  @behaviour :gen_server\n  def init(s), do: {:ok, s}\nend\n";
        let fp = parse_file(src, "lib/srv.ex", "lib::srv", repo()).unwrap();
        assert!(fp.refs.is_empty(), "{:?}", fp.refs);
        // `@behaviour` keeps a pending `@doc`, like any module attribute.
        let src = "defmodule M do\n  @doc \"Runs.\"\n  @behaviour Shop.Job\n  def run, do: :ok\nend\n";
        let fp = parse_file(src, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(docs_of(&fp, node_kind::FUNCTION, "run"), vec!["Runs."]);
        assert_eq!(refs_from(&fp, "lib::m::M"), vec![(edge_category::IMPLEMENTS, "Job".to_string())]);
    }

    #[test]
    fn compile_env_default_emits_injects_ref() {
        let fp = parse_file(ACCOUNTS_DI, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(
            refs_from(&fp, "lib::accounts::Shop.Accounts"),
            vec![(edge_category::INJECTS, "PgUserRepo".to_string())]
        );
        assert_eq!(fp.refs[0].line, 1);
        // Application.get_env/3 with a module default is the runtime twin.
        let src = "defmodule Shop.Mailer do\n  @adapter Application.get_env(:shop, :mail_adapter, Shop.SmtpAdapter)\nend\n";
        let fp = parse_file(src, "lib/mailer.ex", "lib::mailer", repo()).unwrap();
        assert_eq!(
            refs_from(&fp, "lib::mailer::Shop.Mailer"),
            vec![(edge_category::INJECTS, "SmtpAdapter".to_string())]
        );
    }

    #[test]
    fn get_env_without_default_emits_nothing() {
        let src = r#"
defmodule Shop.Accounts do
  @repo Application.get_env(:shop, :user_repo)
  @strict Application.compile_env!(:shop, :strict_repo)
  @nothing Application.compile_env(:shop, :maybe, nil)
  @atom Application.get_env(:shop, :mod, :shop_repo)
  @other Config.compile_env(:shop, :x, Shop.Repo)
  def list_users, do: @repo.all()
end
"#;
        let fp = parse_file(src, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert!(fp.refs.is_empty(), "{:?}", fp.refs);
        assert_eq!(counts_of(src, "lib/accounts.ex", "lib::accounts"), TypeCounts::default());
    }

    #[test]
    fn exunit_test_block_is_a_function_with_calls() {
        let fp = parse_file(CART_TEST, "test/cart_test.exs", "test::cart_test", repo()).unwrap();
        let qname = "test::cart_test::CartTest::test sums";
        assert_eq!(functions(&fp), vec![qname.to_string()]);
        let id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, qname);
        assert_eq!(fp.nav.name_by_id.get(&id).map(String::as_str), Some("test sums"));
        let pkg = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::PACKAGE, "test::cart_test::CartTest");
        assert_eq!(fp.nav.parent_of.get(&id), Some(&pkg));
        assert!(fp.edges.iter().any(|e| e.from == pkg && e.to == id && e.category == edge_category::DEFINES));
        let node = fp.nodes.iter().find(|n| n.id == id).expect("test FUNCTION node");
        let pos = node.cells.iter().find(|c| c.kind == cell_type::POSITION).expect("POSITION");
        assert!(matches!(&pos.payload, CellPayload::Json(j) if j.contains("test/cart_test.exs")));
        let calls = calls_from(&fp, qname);
        assert!(calls.contains(&attr("Cart", "total")), "{calls:?}");
        let site = fp.calls.iter().find(|c| c.qualifier == attr("Cart", "total")).unwrap();
        assert_eq!(site.line, 4, "the call's own row");
        // The test module itself is no contract.
        assert_eq!(kind_of(&fp, "test::cart_test::CartTest"), Some(node_kind::PACKAGE));
    }

    #[test]
    fn describe_prefixes_the_test_name() {
        let src = r#"
defmodule MyAppWeb.CartControllerTest do
  use MyAppWeb.ConnCase, async: true

  setup do
    Cart.reset()
    :ok
  end

  describe "total" do
    setup %{conn: conn} do
      {:ok, conn: conn}
    end

    test "sums", %{conn: conn} do
      Cart.total([1, 2])
    end

    test "empty is zero", do: Cart.total([])
  end

  test "pending"
end
"#;
        let fp = parse_file(src, "test/cart_controller_test.exs", "test::cart_controller_test", repo()).unwrap();
        let m = "test::cart_controller_test::MyAppWeb.CartControllerTest";
        assert_eq!(
            functions(&fp),
            vec![
                format!("{m}::test pending"),
                format!("{m}::test total empty is zero"),
                format!("{m}::test total sums"),
            ]
        );
        assert_eq!(calls_from(&fp, &format!("{m}::test total sums")), vec![attr("Cart", "total")]);
        // A one-liner `test .., do: ..` carries its body in the `do:` keyword.
        assert_eq!(calls_from(&fp, &format!("{m}::test total empty is zero")), vec![attr("Cart", "total")]);
        // `setup` blocks are no tests: `Cart.reset()` belongs to no function.
        assert!(fp.calls.iter().all(|c| c.qualifier != attr("Cart", "reset")), "{:?}", fp.calls);
        assert_eq!(counts_of(src, "test/cart_controller_test.exs", "test::cart_controller_test").exunit_tests, 3);
        // A line break inside the literal is a space in the qname.
        let src = "defmodule T do\n  use ExUnit.Case\n  test \"two\n  lines\" do\n    :ok\n  end\nend\n";
        let fp = parse_file(src, "test/t_test.exs", "test::t_test", repo()).unwrap();
        assert_eq!(functions(&fp), vec!["test::t_test::T::test two   lines".to_string()]);
    }

    #[test]
    fn non_case_module_test_call_is_no_function() {
        // `test` is an ordinary name outside an ExUnit case module.
        let src = r#"
defmodule Shop.Lab do
  use Shop.Experiment
  test "x" do
    Cart.total([])
  end
end
"#;
        let fp = parse_file(src, "lib/lab.ex", "lib::lab", repo()).unwrap();
        assert!(functions(&fp).is_empty(), "{:?}", functions(&fp));
        assert!(fp.calls.is_empty(), "{:?}", fp.calls);
        // `ExUnit.CaseTemplate` builds a template; it is not a case module.
        let src = "defmodule MyApp.DataCase do\n  use ExUnit.CaseTemplate\n  test \"x\" do\n    :ok\n  end\nend\n";
        let fp = parse_file(src, "test/support/data_case.ex", "test::support::data_case", repo()).unwrap();
        assert!(functions(&fp).is_empty(), "{:?}", functions(&fp));
        // Inside a case module, a `test` call whose first argument is no
        // string literal is not ExUnit's macro.
        let src = "defmodule T do\n  use ExUnit.Case\n  test(name, do: :ok)\nend\n";
        let fp = parse_file(src, "test/t_test.exs", "test::t_test", repo()).unwrap();
        assert!(functions(&fp).is_empty(), "{:?}", functions(&fp));
    }

    #[test]
    fn one_liner_def_body_calls_are_collected() {
        let src = "defmodule Shop.Api do\n  def total(xs), do: Shop.Cart.total(xs)\n  defp helper(x), do: normalise(x)\nend\n";
        let fp = parse_file(src, "lib/api.ex", "lib::api", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::api::Shop.Api::total"), vec![attr("Shop.Cart", "total")]);
        assert_eq!(
            calls_from(&fp, "lib::api::Shop.Api::helper"),
            vec![CallQualifier::Bare("normalise".to_string())]
        );
        // A `do_block` body is read once, as before.
        let src = "defmodule M do\n  def f(x) do\n    g(x)\n  end\nend\n";
        let fp = parse_file(src, "lib/m.ex", "lib::m", repo()).unwrap();
        assert_eq!(calls_from(&fp, "lib::m::M::f"), vec![CallQualifier::Bare("g".to_string())]);
    }

    #[test]
    fn alias_as_names_the_binding() {
        let src = r#"
defmodule Shop.Web do
  alias Shop.Cart, as: C
  alias Shop.Order
  import Shop.Helpers
  use Shop.Schema, as: Ignored
end
"#;
        let fp = parse_file(src, "lib/web.ex", "lib::web", repo()).unwrap();
        let targets: Vec<(String, Option<String>)> = fp
            .imports
            .iter()
            .filter_map(|i| match &i.target {
                ImportTarget::Module { path, alias } => Some((path.clone(), alias.clone())),
                ImportTarget::Symbol { .. } => None,
            })
            .collect();
        assert_eq!(
            targets,
            vec![
                ("Shop.Cart".to_string(), Some("C".to_string())),
                ("Shop.Order".to_string(), None),
                ("Shop.Helpers".to_string(), None),
                // `as:` is an `alias` option only; a `use` passes it to `__using__`.
                ("Shop.Schema".to_string(), None),
            ]
        );
    }

    #[test]
    fn type_counters_match_the_fixture_markers() {
        let cases = [
            (NOTIFIER, "lib/notifier.ex", "lib::notifier", (1, 0, 0, 0)),
            (EMAIL_NOTIFIER, "lib/email_notifier.ex", "lib::email_notifier", (0, 1, 0, 0)),
            (USER_REPO, "lib/user_repo.ex", "lib::user_repo", (1, 1, 0, 0)),
            (ACCOUNTS_DI, "lib/accounts.ex", "lib::accounts", (0, 0, 1, 0)),
            (CART_TEST, "test/cart_test.exs", "test::cart_test", (0, 0, 0, 1)),
        ];
        for (src, file, module, (b, i, j, t)) in cases {
            let c = counts_of(src, file, module);
            assert_eq!(
                (c.behaviours, c.implements, c.injects, c.exunit_tests),
                (b, i, j, t),
                "{file}"
            );
        }
        assert_eq!(
            counts_of(NOTIFIER, "lib/notifier.ex", "lib::notifier").marker("lib/notifier.ex").as_deref(),
            Some("[elixir-types] behaviours=1 implements=0 injects=0 exunit_tests=0 path=lib/notifier.ex")
        );
        assert_eq!(
            counts_of(USER_REPO, "lib/user_repo.ex", "lib::user_repo").marker("lib/user_repo.ex").as_deref(),
            Some("[elixir-types] behaviours=1 implements=1 injects=0 exunit_tests=0 path=lib/user_repo.ex")
        );
        // A file with none of the four prints nothing.
        assert_eq!(counts_of(ACCOUNTS, "lib/accounts.ex", "lib::accounts").marker("lib/accounts.ex"), None);
    }

    #[test]
    fn user_repo_probe_contract_and_implementation() {
        let fp = parse_file(USER_REPO, "lib/user_repo.ex", "lib::user_repo", repo()).unwrap();
        assert_eq!(kind_of(&fp, "lib::user_repo::Shop.UserRepo"), Some(node_kind::INTERFACE));
        assert_eq!(kind_of(&fp, "lib::user_repo::Shop.PgUserRepo"), Some(node_kind::PACKAGE));
        assert_eq!(
            refs_from(&fp, "lib::user_repo::Shop.PgUserRepo"),
            vec![(edge_category::IMPLEMENTS, "UserRepo".to_string())]
        );
    }

    // -----------------------------------------------------------------------
    // DH.18: Ecto schemas and Repo query sites
    // -----------------------------------------------------------------------

    /// bench/substrate-gap/matrix/elixir/db/lib/user.ex
    const ECTO_USER: &str = "defmodule Shop.User do\n  use Ecto.Schema\n\n  schema \"users\" do\n    field :email, :string\n  end\nend\n";
    /// bench/substrate-gap/matrix/elixir/db/lib/accounts.ex
    const ECTO_ACCOUNTS: &str = "defmodule Shop.Accounts do\n  import Ecto.Query\n\n  def list_users, do: Shop.Repo.all(Shop.User)\nend\n";

    fn entity_id(name: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::DATA_ENTITY, &format!("data_entity:sql:{name}"))
    }

    fn fn_id(qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::FUNCTION, qname)
    }

    /// The DATA_ENTITY qnames the parse minted, sorted.
    fn entities(fp: &FileParse) -> Vec<String> {
        let mut v: Vec<String> = fp
            .nav
            .kind_by_id
            .iter()
            .filter(|(_, k)| **k == node_kind::DATA_ENTITY)
            .filter_map(|(id, _)| fp.nav.qname_by_id.get(id).cloned())
            .collect();
        v.sort();
        v
    }

    /// The entity names (`data_entity:sql:<name>`) the FUNCTION `qname`
    /// has an ACCESSES_DATA edge to, sorted.
    fn accesses_from(fp: &FileParse, qname: &str) -> Vec<String> {
        let from = fn_id(qname);
        let mut v: Vec<String> = fp
            .edges
            .iter()
            .filter(|e| e.from == from && e.category == edge_category::ACCESSES_DATA)
            .filter_map(|e| fp.nav.qname_by_id.get(&e.to).cloned())
            .collect();
        v.sort();
        v
    }

    fn ecto_counts_of(source: &str, file: &str, module_qname: &str) -> EctoCounts {
        let lang: tree_sitter::Language = tree_sitter_elixir::LANGUAGE.into();
        let mut parser = Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(source, None).unwrap();
        let module_id = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::MODULE, module_qname);
        let mut acc = Acc {
            module_qname: module_qname.to_string(),
            module_id: Some(module_id),
            ..Acc::default()
        };
        visit_top(tree.root_node(), source.as_bytes(), file, module_qname, module_id, repo(), &mut acc);
        acc.ecto
    }

    #[test]
    fn ecto_schema_declares_model_entity_with_table_cell() {
        let fp = parse_file(ECTO_USER, "lib/user.ex", "lib::user", repo()).unwrap();
        assert_eq!(entities(&fp), vec!["data_entity:sql:User"]);
        let id = entity_id("User");
        let node = fp.nodes.iter().find(|n| n.id == id).unwrap();
        assert_eq!(node.confidence, Confidence::Strong);
        let payloads: Vec<&str> = node
            .cells
            .iter()
            .filter(|c| c.kind == cell_type::CODE)
            .filter_map(|c| match &c.payload {
                CellPayload::Json(j) => Some(j.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(payloads, vec![r#"{"orm":"ecto","table":"users"}"#]);
        assert_eq!(data_entity::table_of(&node.cells), Some("users".to_string()));
        let package = NodeId::from_parts(GRAPH_TYPE, repo(), node_kind::PACKAGE, "lib::user::Shop.User");
        assert!(
            fp.edges
                .iter()
                .any(|e| e.from == package && e.to == id && e.category == edge_category::DEFINES),
            "the schema module DEFINES its entity"
        );
        assert_eq!(fp.nav.name_by_id.get(&id).map(String::as_str), Some("User"));
    }

    #[test]
    fn embedded_schema_mints_nothing() {
        let src = "defmodule Shop.Address do\n  use Ecto.Schema\n\n  embedded_schema do\n    field :street, :string\n  end\nend\n";
        let fp = parse_file(src, "lib/address.ex", "lib::address", repo()).unwrap();
        assert!(entities(&fp).is_empty());
        assert_eq!(ecto_counts_of(src, "lib/address.ex", "lib::address").marker("lib/address.ex"), None);
    }

    #[test]
    fn schema_call_outside_a_schema_module_mints_nothing() {
        // No `use Ecto.Schema`: a `schema` call is some other macro.
        let src = "defmodule Shop.Doc do\n  schema \"users\" do\n    field :email\n  end\nend\n";
        let fp = parse_file(src, "lib/doc.ex", "lib::doc", repo()).unwrap();
        assert!(entities(&fp).is_empty());
    }

    #[test]
    fn app_schema_wrapper_is_a_schema_module() {
        let src = "defmodule Shop.Order do\n  use Shop.Schema\n\n  schema \"orders\" do\n    field :total, :integer\n  end\nend\n";
        let fp = parse_file(src, "lib/order.ex", "lib::order", repo()).unwrap();
        assert_eq!(entities(&fp), vec!["data_entity:sql:Order"]);
        let node = fp.nodes.iter().find(|n| n.id == entity_id("Order")).unwrap();
        assert_eq!(data_entity::table_of(&node.cells), Some("orders".to_string()));
        assert!(is_ecto_schema_use("Ecto.Schema"));
        assert!(!is_ecto_schema_use("Schema"), "a bare `Schema` names no app wrapper");
        assert!(!is_ecto_schema_use("Ecto.Migration"));
    }

    #[test]
    fn repo_all_alias_is_an_access() {
        let src = "defmodule Shop.Accounts do\n  alias Shop.Repo\n  alias Shop.User\n\n  def list_users do\n    Repo.all(User)\n  end\n\n  def get_user!(id), do: Repo.get!(User, id)\nend\n";
        let fp = parse_file(src, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(accesses_from(&fp, "lib::accounts::Shop.Accounts::list_users"), vec!["data_entity:sql:User"]);
        assert_eq!(accesses_from(&fp, "lib::accounts::Shop.Accounts::get_user!"), vec!["data_entity:sql:User"]);
        // One node per entity per file; not declared here, so Medium.
        let nodes: Vec<&Node> = fp.nodes.iter().filter(|n| n.id == entity_id("User")).collect();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].confidence, Confidence::Medium);
        assert!(nodes[0].cells.is_empty(), "the table rides the declaration only");
    }

    #[test]
    fn repo_site_in_keyword_do_body() {
        let fp = parse_file(ECTO_ACCOUNTS, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(accesses_from(&fp, "lib::accounts::Shop.Accounts::list_users"), vec!["data_entity:sql:User"]);
        let edge = fp
            .edges
            .iter()
            .find(|e| e.category == edge_category::ACCESSES_DATA)
            .unwrap();
        let ev = Evidence::of(edge).unwrap();
        assert_eq!(ev.emitter, "parser:elixir");
        assert_eq!(ev.rule.as_deref(), Some("ecto_repo"));
        assert_eq!(ev.line, Some(3), "the Repo call's 0-based row");
    }

    #[test]
    fn repo_insert_struct_literal() {
        let src = r#"defmodule Shop.Accounts do
  alias Shop.Repo

  def create(email), do: Repo.insert(%Shop.User{email: email})

  def register(attrs) do
    %User{}
    |> User.changeset(attrs)
    |> Repo.insert()
  end

  def save(user, attrs) do
    user |> User.changeset(attrs) |> Repo.update()
  end
end
"#;
        let fp = parse_file(src, "lib/accounts.ex", "lib::accounts", repo()).unwrap();
        assert_eq!(accesses_from(&fp, "lib::accounts::Shop.Accounts::create"), vec!["data_entity:sql:User"]);
        assert_eq!(
            accesses_from(&fp, "lib::accounts::Shop.Accounts::register"),
            vec!["data_entity:sql:User"],
            "a pipeline's head is the stage's first argument"
        );
        assert!(
            accesses_from(&fp, "lib::accounts::Shop.Accounts::save").is_empty(),
            "a variable head names nothing"
        );
        // The pipeline stage `User.changeset(attrs)` is still a CALLS site.
        assert!(calls_from(&fp, "lib::accounts::Shop.Accounts::register").contains(&attr("User", "changeset")));
    }

    #[test]
    fn from_in_query_names_the_model() {
        let src = r#"defmodule Shop.Reports do
  import Ecto.Query
  alias Shop.Repo

  def active(ids) do
    query = from u in User, join: p in Post, on: p.user_id == u.id, left_join: c in assoc(p, :comments), where: u.id in ^ids
    Repo.all(query)
  end

  def orders, do: Ecto.Query.from(o in Shop.Order) |> Repo.all()
end
"#;
        let fp = parse_file(src, "lib/reports.ex", "lib::reports", repo()).unwrap();
        assert_eq!(
            accesses_from(&fp, "lib::reports::Shop.Reports::active"),
            vec!["data_entity:sql:Post", "data_entity:sql:User"]
        );
        assert_eq!(accesses_from(&fp, "lib::reports::Shop.Reports::orders"), vec!["data_entity:sql:Order"]);
        let ev = fp
            .edges
            .iter()
            .find(|e| e.from == fn_id("lib::reports::Shop.Reports::orders"))
            .and_then(Evidence::of)
            .unwrap();
        assert_eq!(ev.rule.as_deref(), Some("ecto_from"));
    }

    #[test]
    fn schemaless_from_string_names_the_table() {
        let src = "defmodule Shop.Stats do\n  import Ecto.Query\n\n  def emails, do: Shop.Repo.all(from u in \"users\", select: u.email)\n  def purge, do: Repo.delete_all(\"sessions\")\n  def dynamic(t), do: Repo.all(from u in \"#{t}_users\")\nend\n";
        let fp = parse_file(src, "lib/stats.ex", "lib::stats", repo()).unwrap();
        assert_eq!(accesses_from(&fp, "lib::stats::Shop.Stats::emails"), vec!["data_entity:sql:users"]);
        assert_eq!(accesses_from(&fp, "lib::stats::Shop.Stats::purge"), vec!["data_entity:sql:sessions"]);
        assert!(accesses_from(&fp, "lib::stats::Shop.Stats::dynamic").is_empty(), "an interpolated source names no table");
    }

    #[test]
    fn non_repo_call_is_no_site() {
        let src = "defmodule Shop.Web do\n  def a, do: Cache.all(User)\n  def b(repo), do: repo.all(User)\n  def c(q), do: Repo.all(q)\n  def d, do: Repo.transaction(User)\n  def e, do: Shop.UserRepo.all(User)\nend\n";
        let fp = parse_file(src, "lib/web.ex", "lib::web", repo()).unwrap();
        assert!(entities(&fp).is_empty(), "{:?}", entities(&fp));
        assert!(!fp.edges.iter().any(|e| e.category == edge_category::ACCESSES_DATA));
        assert_eq!(ecto_counts_of(src, "lib/web.ex", "lib::web").marker("lib/web.ex"), None);
    }

    #[test]
    fn schema_module_self_query_lands_on_its_declaration() {
        let src = "defmodule Shop.User do\n  use Ecto.Schema\n\n  def by_email(e), do: Shop.Repo.get_by(__MODULE__, email: e)\n\n  schema \"users\" do\n    field :email, :string\n  end\n\n  def blank, do: Shop.Repo.insert(%__MODULE__{})\nend\n";
        let fp = parse_file(src, "lib/user.ex", "lib::user", repo()).unwrap();
        assert_eq!(entities(&fp), vec!["data_entity:sql:User"]);
        let nodes: Vec<&Node> = fp.nodes.iter().filter(|n| n.id == entity_id("User")).collect();
        assert_eq!(nodes.len(), 1, "the site before the declaration and the declaration share one node");
        assert_eq!(nodes[0].confidence, Confidence::Strong, "the declaration lifts the site's node");
        assert_eq!(data_entity::table_of(&nodes[0].cells), Some("users".to_string()));
        assert_eq!(accesses_from(&fp, "lib::user::Shop.User::by_email"), vec!["data_entity:sql:User"]);
        assert_eq!(accesses_from(&fp, "lib::user::Shop.User::blank"), vec!["data_entity:sql:User"]);
    }

    #[test]
    fn ecto_counters_match_the_fixture_markers() {
        assert_eq!(
            ecto_counts_of(ECTO_USER, "lib/user.ex", "lib::user").marker("lib/user.ex").as_deref(),
            Some("[orm-ecto] schemas=1 table_cells=1 repo_sites=0 path=lib/user.ex")
        );
        assert_eq!(
            ecto_counts_of(ECTO_ACCOUNTS, "lib/accounts.ex", "lib::accounts")
                .marker("lib/accounts.ex")
                .as_deref(),
            Some("[orm-ecto] schemas=0 table_cells=0 repo_sites=1 path=lib/accounts.ex")
        );
        // A non-literal source declares the entity but no table.
        let src = "defmodule Shop.Tag do\n  use Ecto.Schema\n  schema @source do\n    field :name\n  end\nend\n";
        assert_eq!(
            ecto_counts_of(src, "lib/tag.ex", "lib::tag"),
            EctoCounts { schemas: 1, table_cells: 0, repo_sites: 0 }
        );
        // A file with neither prints nothing.
        assert_eq!(ecto_counts_of(ACCOUNTS, "lib/accounts.ex", "lib::accounts").marker("lib/accounts.ex"), None);
    }
}
