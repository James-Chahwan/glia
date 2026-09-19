//! Kotlin call sites, supertypes and field types (A14.3).
//!
//! Parsers extract, the graph crate resolves: every call inside a declared
//! function's body leaves as a [`CallSite`] for the generic `resolve_calls`,
//! every supertype as an INHERITS_FROM / IMPLEMENTS [`UnresolvedRef`]
//! `Bare(SimpleName)` for `resolve_refs` (never a name-derived NodeId), and
//! every typed instance property as a `CodeNav::record_field_type` entry, so
//! A6.2a's receiver-type pass binds `userService.findById(id)` to `findById`
//! on the property's declared type.
//!
//! # Receiver-less calls: `SelfMethod` or `Bare`, decided here
//!
//! Kotlin resolves a call without an explicit receiver LOCAL > implicit
//! receiver MEMBER > top-level (Kotlin spec, "call without an explicit
//! receiver"). Unlike Java it has real top-level functions, so a bare `f()`
//! is not always `this.f()`; `logIt(saved)` inside a class and
//! `topLevelHelper(2)` are syntactically identical. The parser can see the
//! enclosing class body, so it classifies:
//!
//! - a name declared locally in the function (a parameter, a local `val` /
//!   `var`, a local `fun`, a lambda parameter, a method of an anonymous
//!   `object :` in the body) emits NOTHING: the callee is the local;
//! - a name declared as a method of the enclosing class / object body
//!   (companion members included, same file) is `SelfMethod(name)`, which
//!   `resolve_calls` binds through `class_methods` of the enclosing type;
//! - anything else is `Bare(name)`: import binding, then the module's own
//!   top-level declarations.
//!
//! An inherited method called bare (`ping()` in `class S : Base()`) is not in
//! the class body, so it is `Bare` and stays unresolved unless a top-level
//! declaration shares its name. A bare call inside an INTERFACE's default
//! method is `SelfMethod` but does not resolve: `enclosing_class_or_struct`
//! matches CLASS / STRUCT / ENUM only. A lambda with a receiver
//! (`with(repo) { save(x) }`, `apply { }`) changes the implicit receiver in a
//! way no parser can see without types; its receiver-less calls are
//! classified as if the lambda were not there.
//!
//! # Receivers
//!
//! `a.m()` / `a?.m()` is `Attribute { a, m }` (the safe call parses to the
//! same `navigation_expression [identifier, identifier]`); `this.m()` and
//! `this@Own.m()` are `SelfMethod(m)` (in an extension function an unlabeled
//! `this` is the extension receiver: `ComplexReceiver`); `super.m()` is
//! `SuperMethod(m)`; any other receiver (`this.svc.m()`, `A.B.m()`,
//! `"s".m()`) is `ComplexReceiver { receiver, m }` — `this.svc` still binds
//! through `svc`'s field type in the graph. A parameter or local named like a
//! property of a different type binds through the property's type: the same
//! residual every A6.2a language has. Parameters are not fields, so
//! `fun Application.itemRoutes(store: ItemStore) { store.all() }` stays
//! unresolved.
//!
//! # What the walk covers
//!
//! The `function_body` of every declared FUNCTION / METHOD, descending into
//! lambdas (`routing { get("/x") { store.all() } }` nests calls three lambdas
//! deep), anonymous objects and local functions — all of it runs as part of
//! the declared function, the same owner rule the Ktor route handler uses. It
//! does not descend into a local `class` / `object` declaration (another
//! receiver). Code outside a function body — `init { }` blocks, secondary
//! constructor bodies, property initializers and accessors, default argument
//! values — records no calls.
//!
//! A Ktor DSL call ([`routes::ktor_call`]: `get("/p") { … }`, `route("/p")
//! { … }`) is a ROUTE, not a call: it mints no CallSite (neither the verb nor
//! its inner head call), but its lambda and arguments are walked, so
//! `store.all()` inside a handler is still a call of the enclosing function.
//! A call whose head is itself a call (`run(x) { … }` parses as
//! `call_expression [call_expression run(x), annotated_lambda]`) is that one
//! inner call: only the inner node mints a CallSite.
//!
//! # Supertypes (tree-sitter-kotlin-ng 1.1.0, measured)
//!
//! `delegation_specifiers > delegation_specifier >`
//! - `constructor_invocation` (`: BaseService()`) — the superclass:
//!   INHERITS_FROM;
//! - `user_type` (`: Auditable`) — an interface: IMPLEMENTS from a class /
//!   object / enum, INHERITS_FROM from an interface (an interface extends its
//!   super-interfaces, the LD.7a direction and category);
//! - `explicit_delegation` (`: Repo by impl`) — interface delegation:
//!   IMPLEMENTS.
//!
//! One exception, read off the declaration: a class with NO primary
//! constructor whose secondary constructor delegates to `super(args)` names
//! its superclass without a call (`class V : View { constructor(c: C) :
//! super(c) }`). When exactly one `user_type` specifier is present it is that
//! superclass (INHERITS_FROM); with several the category stays IMPLEMENTS.
//!
//! # Field types
//!
//! A primary-constructor `val` / `var` parameter and a class-body property
//! with a declared type record `name -> SimpleType` on the owning type (a
//! companion's properties on the class they attach to). `T?` records `T`; a
//! generic type records nothing (the Java A6.2c rule: `Provider<Repo>` has
//! Provider's methods), and neither does a value type (`Int`, `String`,
//! `List`, …) or a function type. A constructor parameter without `val` /
//! `var` is no property.

use std::collections::HashSet;

use repo_graph_code_domain::{CallQualifier, CallSite, UnresolvedRef, endpoint, jvm};
use repo_graph_core::{EdgeCategoryId, NodeId};
use tree_sitter::Node as TsNode;

use crate::{Acc, File, android, edge_category, named_child_of_kind, routes, text_of};

/// The declaration a function belongs to, as its body's receiver-less calls
/// see it.
#[derive(Clone, Copy)]
pub(crate) struct CallScope<'a> {
    /// Methods declared in the enclosing class / object body (companion
    /// members included) — empty at the top level. A receiver-less call to one
    /// of them is `SelfMethod`.
    pub(crate) members: &'a HashSet<String>,
    /// The enclosing type's simple name (`this@Name` is its receiver); `None`
    /// at the top level.
    pub(crate) type_name: Option<&'a str>,
}

/// One function body's walk context: its caller id, scope and locals.
pub(crate) struct Body<'a> {
    /// The declared FUNCTION / METHOD the calls belong to.
    pub(crate) from: NodeId,
    pub(crate) scope: CallScope<'a>,
    /// An extension function (`fun T.f()`): unlabeled `this` is the
    /// extension receiver, not the enclosing class.
    pub(crate) extension: bool,
    /// Names declared locally ([`local_names`]): a receiver-less call to one
    /// of them calls the local.
    pub(crate) locals: HashSet<String>,
}

/// Names of the methods a class / object / enum body declares — the set a
/// receiver-less call is checked against. Mirrors `walk_members`: a
/// `companion object`'s members attach to the type, and an `ERROR` node is a
/// transparent container.
pub(crate) fn member_fn_names(body: TsNode, src: &[u8]) -> HashSet<String> {
    let mut out = HashSet::new();
    let mut stack = vec![body];
    while let Some(container) = stack.pop() {
        let mut cursor = container.walk();
        for child in container.named_children(&mut cursor) {
            match child.kind() {
                "function_declaration" => {
                    if let Some(name) = child.child_by_field_name("name") {
                        out.insert(text_of(name, src).to_string());
                    }
                }
                "companion_object" => {
                    if let Some(b) = named_child_of_kind(child, &["class_body"]) {
                        stack.push(b);
                    }
                }
                "ERROR" => stack.push(child),
                _ => {}
            }
        }
    }
    out
}

/// Collect the calls of one declared function (`function_declaration`) into
/// `acc.calls`. A function with no body (abstract / interface) has none.
pub(crate) fn collect(fun: TsNode, from: NodeId, scope: CallScope, file: &File, acc: &mut Acc) {
    let Some(body) = named_child_of_kind(fun, &["function_body"]) else {
        return;
    };
    let ctx = Body {
        from,
        scope,
        extension: is_extension(fun),
        locals: local_names(fun, body, file.src),
    };
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        match node.kind() {
            // A local type declaration is another receiver's code.
            "class_declaration" | "object_declaration" => continue,
            "call_expression" => {
                if let Some(ktor) = routes::ktor_call(node, file.src) {
                    // A ROUTE, not a call: walk only what runs inside it. The
                    // lambda pushed first pops last: source order.
                    stack.push(ktor.lambda);
                    if let Some(args) = ktor.args {
                        stack.push(args);
                    }
                    continue;
                }
                visit_call(node, &ctx, file, acc);
            }
            _ => {}
        }
        let mut cursor = node.walk();
        let children: Vec<TsNode> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
}

/// The one per-call visit: classify `call` and push its CallSite (if it is a
/// call of a nameable callee). Every per-call detector hangs off here.
pub(crate) fn visit_call(call: TsNode, ctx: &Body, file: &File, acc: &mut Acc) {
    if let Some(qualifier) = classify(call, ctx, file.src) {
        acc.calls.push(CallSite {
            from: ctx.from,
            qualifier,
        });
    }
    spring_client(call, ctx.from, file, acc);
}

/// A14.6: a Spring HTTP client call → a client ENDPOINT + CALLS from the
/// enclosing function `from` (the Java parser's `try_detect_java_endpoint`
/// Spring arms, over the Kotlin chain):
///
/// - RestTemplate: `rest.getForObject(url, C::class.java)`,
///   `.postForEntity(url, body, C)` … — the verb from the method name
///   (`endpoint::rest_template_verb`); `.exchange(url, HttpMethod.GET, …)` /
///   `.execute(…)` from the `HttpMethod.<VERB>` argument. Bare `put` / `delete`
///   also name a Map's or a Javalin router's method (`app.delete("/x", h)` is
///   a SERVER route), so they count only on a receiver named like a
///   RestTemplate (`restTemplate`, `rest`, `template`, `restOperations`).
/// - WebClient / RestClient: `webClient.post().uri(url)…` fires on the
///   `.uri(url)` call, its verb walked down the fluent chain (`.get()` /
///   `.post()` / `.method(HttpMethod.PUT)`).
///
/// The URL is the first argument: a literal (Strong), a template or `+`
/// concatenation (Medium, interpolations as `${…}`); its path must start
/// with `/` after the authority is split off (`endpoint::client_url_split`,
/// the Java rule), so `map.put("k", v)` never emits.
fn spring_client(call: TsNode, from: NodeId, file: &File, acc: &mut Acc) {
    let src = file.src;
    let Some(head) = call.named_child(0).filter(|h| h.kind() == "navigation_expression") else {
        return;
    };
    let Some((base, name)) = nav_parts(head, src) else {
        return;
    };
    let args = named_child_of_kind(call, &["value_arguments"]);
    let verb = match name {
        "uri" => webclient_verb(base, src),
        "exchange" | "execute" => args.and_then(|a| http_method_arg_verb(a, src)),
        "put" | "delete" if !is_rest_template_receiver(base, src) => None,
        other => endpoint::rest_template_verb(other),
    };
    let Some(verb) = verb else {
        return;
    };
    let Some((raw, strong)) = args
        .and_then(first_arg_value)
        .and_then(|arg| android::client_url(arg, src))
    else {
        return;
    };
    let (host, path) = endpoint::client_url_split(&raw);
    let Some(path) = path else {
        return;
    };
    android::emit_client_endpoint(call, verb, path, host.as_deref(), strong, from, file, acc);
    acc.clients.spring_clients += 1;
}

/// `(receiver, method name)` of a `navigation_expression` call head: `a.b.m`
/// is `(a.b, "m")`. `None` when the member is not an identifier.
fn nav_parts<'a>(head: TsNode<'a>, src: &'a [u8]) -> Option<(TsNode<'a>, &'a str)> {
    let base = head.named_child(0)?;
    let last_ix = u32::try_from(head.named_child_count().checked_sub(1)?).ok()?;
    let last = head.named_child(last_ix)?;
    (last.kind() == "identifier" && last.id() != base.id()).then(|| (base, text_of(last, src)))
}

/// The verb of a WebClient / RestClient fluent chain, walked down from the
/// receiver of its `.uri(…)`: the first `.get()` / `.post()` / … call (any
/// case, the Java arm's rule) or `.method(HttpMethod.X)`. `None` when the
/// chain reaches its root without one (`HttpRequest.newBuilder().uri(…)`).
fn webclient_verb(receiver: TsNode, src: &[u8]) -> Option<&'static str> {
    let mut cur = receiver;
    while cur.kind() == "call_expression" {
        let head = cur.named_child(0)?;
        let (next, name) = match head.kind() {
            "navigation_expression" => {
                let (base, name) = nav_parts(head, src)?;
                (Some(base), name)
            }
            "identifier" => (None, text_of(head, src)),
            _ => return None,
        };
        if let Some(verb) = endpoint::jaxrs_verb(&name.to_ascii_uppercase()) {
            return Some(verb);
        }
        if name == "method"
            && let Some(verb) =
                named_child_of_kind(cur, &["value_arguments"]).and_then(|a| http_method_arg_verb(a, src))
        {
            return Some(verb);
        }
        cur = next?;
    }
    None
}

/// The verb of the first `HttpMethod.<VERB>` argument
/// (`endpoint::http_method_ref_verb`, shared with the Java arm).
fn http_method_arg_verb(args: TsNode, src: &[u8]) -> Option<&'static str> {
    let mut cursor = args.walk();
    args.named_children(&mut cursor)
        .filter(|a| a.kind() == "value_argument")
        .find_map(|a| endpoint::http_method_ref_verb(text_of(a, src)))
}

/// The expression of a call's first positional argument.
fn first_arg_value(args: TsNode) -> Option<TsNode> {
    let mut cursor = args.walk();
    let first = args
        .named_children(&mut cursor)
        .find(|a| a.kind() == "value_argument")?;
    let mut ac = first.walk();
    let has_key = first.children(&mut ac).any(|c| !c.is_named() && c.kind() == "=");
    if has_key {
        return None;
    }
    first.named_child(0)
}

/// A receiver named like a RestTemplate / RestOperations — its last `.`
/// segment, case-folded, contains `rest` or `template`.
fn is_rest_template_receiver(receiver: TsNode, src: &[u8]) -> bool {
    let text = text_of(receiver, src);
    let last = text.rsplit('.').next().unwrap_or(text).to_ascii_lowercase();
    last.contains("rest") || last.contains("template")
}

/// The qualifier of one `call_expression`, or `None` when it names no callee
/// the graph could bind: a call to a local, a call whose head is a call (the
/// inner call is the real one), a lambda or parenthesised head.
fn classify(call: TsNode, ctx: &Body, src: &[u8]) -> Option<CallQualifier> {
    let head = call.named_child(0)?;
    match head.kind() {
        "identifier" => {
            let name = text_of(head, src);
            if ctx.locals.contains(name) {
                None
            } else if ctx.scope.members.contains(name) {
                Some(CallQualifier::SelfMethod(name.to_string()))
            } else {
                Some(CallQualifier::Bare(name.to_string()))
            }
        }
        "navigation_expression" => {
            let base = head.named_child(0)?;
            let last_ix = u32::try_from(head.named_child_count().checked_sub(1)?).ok()?;
            let last = head.named_child(last_ix)?;
            if last.kind() != "identifier" || last.id() == base.id() {
                return None;
            }
            let name = text_of(last, src).to_string();
            Some(match base.kind() {
                "identifier" => CallQualifier::Attribute {
                    base: text_of(base, src).to_string(),
                    name,
                },
                "this_expression" if is_own_this(base, ctx, src) => CallQualifier::SelfMethod(name),
                "super_expression" => CallQualifier::SuperMethod(name),
                _ => CallQualifier::ComplexReceiver {
                    receiver: text_of(base, src).to_string(),
                    name,
                },
            })
        }
        _ => None,
    }
}

/// Whether a `this_expression` is the enclosing class's own receiver:
/// `this@Own`, or a plain `this` outside an extension function.
fn is_own_this(this: TsNode, ctx: &Body, src: &[u8]) -> bool {
    match named_child_of_kind(this, &["identifier"]) {
        Some(label) => ctx.scope.type_name == Some(text_of(label, src)),
        None => !ctx.extension,
    }
}

/// `fun T.f()`: a receiver type sits before the function's name.
fn is_extension(fun: TsNode) -> bool {
    let Some(name) = fun.child_by_field_name("name") else {
        return false;
    };
    let mut cursor = fun.walk();
    fun.named_children(&mut cursor)
        .take_while(|c| c.id() != name.id())
        .any(|c| {
            matches!(
                c.kind(),
                "user_type" | "nullable_type" | "parenthesized_type" | "function_type"
            )
        })
}

/// Every name the function declares locally: its parameters, and anywhere in
/// its body (local types excluded) each `val` / `var` / `for` / lambda /
/// destructuring variable and each `fun` (a local function, or a method of an
/// anonymous object). One pre-walk, so a declaration after the use still
/// shadows it — a dropped call, never a false edge.
fn local_names(fun: TsNode, body: TsNode, src: &[u8]) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Some(params) = named_child_of_kind(fun, &["function_value_parameters"]) {
        let mut cursor = params.walk();
        for p in params.named_children(&mut cursor) {
            if p.kind() == "parameter"
                && let Some(name) = named_child_of_kind(p, &["identifier"])
            {
                out.insert(text_of(name, src).to_string());
            }
        }
    }
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "class_declaration" | "object_declaration" => continue,
            "variable_declaration" => {
                if let Some(name) = named_child_of_kind(node, &["identifier"]) {
                    out.insert(text_of(name, src).to_string());
                }
            }
            "function_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    out.insert(text_of(name, src).to_string());
                }
            }
            _ => {}
        }
        let mut cursor = node.walk();
        stack.extend(node.named_children(&mut cursor));
    }
    out
}

/// A type declaration's supertypes as INHERITS_FROM / IMPLEMENTS refs from
/// `from` (see the module doc for the categories). `is_interface` is the
/// declaring type's kind.
pub(crate) fn heritage(decl: TsNode, from: NodeId, is_interface: bool, file: &File, acc: &mut Acc) {
    let Some(specs) = named_child_of_kind(decl, &["delegation_specifiers"]) else {
        return;
    };
    let mut cursor = specs.walk();
    let specs: Vec<TsNode> = specs
        .named_children(&mut cursor)
        .filter(|s| s.kind() == "delegation_specifier")
        .collect();
    let bare_types = specs
        .iter()
        .filter(|s| named_child_of_kind(**s, &["user_type"]).is_some())
        .count();
    let callless_superclass = !is_interface && bare_types == 1 && delegates_to_super(decl);
    for spec in specs {
        let Some(part) = spec.named_child(0) else {
            continue;
        };
        let (ty, category) = match part.kind() {
            "constructor_invocation" => (
                named_child_of_kind(part, &["user_type"]),
                edge_category::INHERITS_FROM,
            ),
            "user_type" if is_interface || callless_superclass => {
                (Some(part), edge_category::INHERITS_FROM)
            }
            "user_type" => (Some(part), edge_category::IMPLEMENTS),
            "explicit_delegation" => (
                named_child_of_kind(part, &["user_type"]),
                edge_category::IMPLEMENTS,
            ),
            _ => (None, edge_category::IMPLEMENTS),
        };
        let Some(name) = ty
            .map(|t| simple_type_name(t, file.src))
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        push_heritage_ref(name, category, from, file, acc);
    }
}

/// The Java parser's `emit_heritage_ref` shape: a `Bare(SimpleName)` ref the
/// graph's `resolve_refs` binds through imports, the module's own symbols or a
/// repo-unique type name.
fn push_heritage_ref(
    name: String,
    category: EdgeCategoryId,
    from: NodeId,
    file: &File,
    acc: &mut Acc,
) {
    acc.refs.push(UnresolvedRef {
        from,
        from_module: file.module_id,
        qualifier: CallQualifier::Bare(name),
        category,
    });
}

/// A class with no primary constructor whose body holds a secondary
/// constructor delegating to `super(args)` — its superclass is named without
/// a call in the supertype list.
fn delegates_to_super(decl: TsNode) -> bool {
    if named_child_of_kind(decl, &["primary_constructor"]).is_some() {
        return false;
    }
    let Some(body) = named_child_of_kind(decl, &["class_body"]) else {
        return false;
    };
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|c| c.kind() == "secondary_constructor")
        .filter_map(|c| named_child_of_kind(c, &["constructor_delegation_call"]))
        .any(|call| {
            let mut c = call.walk();
            let is_super = call
                .children(&mut c)
                .any(|t| !t.is_named() && t.kind() == "super");
            is_super
                && named_child_of_kind(call, &["value_arguments"])
                    .is_some_and(|args| args.named_child_count() > 0)
        })
}

/// Record a class's primary-constructor properties (`val` / `var`
/// parameters) as field types of `owner`.
pub(crate) fn record_ctor_field_types(decl: TsNode, owner: NodeId, file: &File, acc: &mut Acc) {
    let Some(params) = named_child_of_kind(decl, &["primary_constructor"])
        .and_then(|c| named_child_of_kind(c, &["class_parameters"]))
    else {
        return;
    };
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        if param.kind() != "class_parameter" {
            continue;
        }
        let mut c = param.walk();
        let is_property = param
            .children(&mut c)
            .any(|t| !t.is_named() && matches!(t.kind(), "val" | "var"));
        if !is_property {
            continue;
        }
        let Some(name) = named_child_of_kind(param, &["identifier"]) else {
            continue;
        };
        if let Some(ty) = field_type_name(param, file.src) {
            acc.nav
                .record_field_type(owner, text_of(name, file.src), &ty);
        }
    }
}

/// Record a class-body property's declared type as a field type of `owner`.
pub(crate) fn record_property_type(prop: TsNode, owner: NodeId, file: &File, acc: &mut Acc) {
    let Some(var) = named_child_of_kind(prop, &["variable_declaration"]) else {
        return;
    };
    let Some(name) = named_child_of_kind(var, &["identifier"]) else {
        return;
    };
    if let Some(ty) = field_type_name(var, file.src) {
        acc.nav
            .record_field_type(owner, text_of(name, file.src), &ty);
    }
}

/// The declared type of a parameter / variable as a field type: the simple
/// name of a non-generic `user_type` (`T?` is `T`), never a value type.
fn field_type_name(decl: TsNode, src: &[u8]) -> Option<String> {
    let ty = named_child_of_kind(decl, &["user_type", "nullable_type"])?;
    let ty = if ty.kind() == "nullable_type" {
        named_child_of_kind(ty, &["user_type"])?
    } else {
        ty
    };
    if named_child_of_kind(ty, &["type_arguments"]).is_some() {
        return None;
    }
    let name = simple_type_name(ty, src);
    (!name.is_empty() && !jvm::is_non_injectable_type(&name) && !jvm::is_kotlin_value_type(&name))
        .then_some(name)
}

/// The last `identifier` of a `user_type`: `a.b.C` is `C`, `Repo<User>` is
/// `Repo` (its type arguments are no identifiers).
fn simple_type_name(user_type: TsNode, src: &[u8]) -> String {
    let mut cursor = user_type.walk();
    user_type
        .named_children(&mut cursor)
        .filter(|c| c.kind() == "identifier")
        .last()
        .map(|c| text_of(c, src).trim().to_string())
        .unwrap_or_default()
}

/// The `[kotlin] refs:` marker line over a repo's parses (cache-served files
/// count too — everything is read off the `FileParse`s).
pub(crate) fn marker(parses: &[repo_graph_code_domain::FileParse], repo_label: &str) -> String {
    let calls: usize = parses.iter().map(|fp| fp.calls.len()).sum();
    let self_calls = parses
        .iter()
        .flat_map(|fp| &fp.calls)
        .filter(|c| matches!(c.qualifier, CallQualifier::SelfMethod(_)))
        .count();
    let refs = |cat: EdgeCategoryId| {
        parses
            .iter()
            .flat_map(|fp| &fp.refs)
            .filter(|r| r.category == cat)
            .count()
    };
    let field_types: usize = parses
        .iter()
        .flat_map(|fp| fp.nav.field_types.values())
        .map(|m| m.len())
        .sum();
    format!(
        "[kotlin] refs: calls={calls} self={self_calls} inherits={} implements={} field_types={field_types} repo={repo_label}",
        refs(edge_category::INHERITS_FROM),
        refs(edge_category::IMPLEMENTS),
    )
}

#[cfg(test)]
mod tests {
    use repo_graph_code_domain::{
        CallQualifier, FileParse, UnresolvedRef, edge_category, node_kind,
    };
    use repo_graph_core::{EdgeCategoryId, NodeId, RepoId};

    use crate::{GRAPH_TYPE, parse_file};

    fn parse(source: &str) -> FileParse {
        parse_file(source, "svc.kt", "svc", RepoId(1)).unwrap()
    }

    fn id(kind: repo_graph_core::NodeKindId, qname: &str) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, RepoId(1), kind, qname)
    }

    /// The qualifiers of the calls made by `from`, in source order.
    fn calls_of(fp: &FileParse, from: NodeId) -> Vec<CallQualifier> {
        fp.calls
            .iter()
            .filter(|c| c.from == from)
            .map(|c| c.qualifier.clone())
            .collect()
    }

    fn bare(n: &str) -> CallQualifier {
        CallQualifier::Bare(n.into())
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.into(),
            name: name.into(),
        }
    }

    fn heritage(fp: &FileParse, cat: EdgeCategoryId) -> Vec<(NodeId, String)> {
        fp.refs
            .iter()
            .filter(|r| r.category == cat)
            .map(
                |UnresolvedRef {
                     from, qualifier, ..
                 }| match qualifier {
                    CallQualifier::Bare(n) => (*from, n.clone()),
                    other => panic!("heritage ref must be Bare, got {other:?}"),
                },
            )
            .collect()
    }

    #[test]
    fn attribute_call_emits_attribute_qualifier() {
        let fp = parse(
            r#"
class UserController(private val userService: UserService) {
    fun getUser(id: Long): UserDto = userService.findById(id)
}
"#,
        );
        let get = id(node_kind::METHOD, "UserController::getUser");
        assert_eq!(calls_of(&fp, get), vec![attr("userService", "findById")]);
    }

    #[test]
    fn safe_call_classifies_as_attribute() {
        let fp = parse("fun load(r: Repo?) {\n    r?.load(\"y\")\n}\n");
        assert_eq!(
            calls_of(&fp, id(node_kind::FUNCTION, "svc::load")),
            vec![attr("r", "load")]
        );
    }

    #[test]
    fn bare_call_in_class_emits_self_for_a_member_else_bare() {
        // Kotlin: local > member > top-level. `logIt` is declared in the class
        // body: SelfMethod. `topLevelHelper` / `println` are not: Bare, so the
        // graph's module lookup binds the real top-level fun.
        let fp = parse(
            r#"
class UserController {
    fun createUser(dto: UserDto): UserDto {
        logIt(dto)
        topLevelHelper(2)
        println(dto)
        return dto
    }

    private fun logIt(dto: UserDto) {}

    companion object {
        fun create(): UserController = UserController()
    }

    fun fresh() = create()
}

fun topLevelHelper(n: Int): Int = n * 2

fun top() {
    logIt(1)
}
"#,
        );
        assert_eq!(
            calls_of(&fp, id(node_kind::METHOD, "UserController::createUser")),
            vec![
                CallQualifier::SelfMethod("logIt".into()),
                bare("topLevelHelper"),
                bare("println"),
            ]
        );
        // A companion member is a member of the class it attaches to.
        assert_eq!(
            calls_of(&fp, id(node_kind::METHOD, "UserController::fresh")),
            vec![CallQualifier::SelfMethod("create".into())]
        );
        // `create()` inside the companion names the class (a constructor call).
        assert_eq!(
            calls_of(&fp, id(node_kind::METHOD, "UserController::create")),
            vec![bare("UserController")]
        );
        // At the top level no class is in scope: always Bare.
        assert_eq!(
            calls_of(&fp, id(node_kind::FUNCTION, "svc::top")),
            vec![bare("logIt")]
        );
    }

    #[test]
    fn a_local_shadows_the_member_and_emits_nothing() {
        let fp = parse(
            r#"
class Job {
    fun run(done: () -> Unit) {
        fun log(s: String) {}
        val step = { x: Int -> x }
        log("a")
        step(1)
        done()
        tick()
    }

    fun log(s: String) {}
    fun step(n: Int) {}
    fun tick() {}
}
"#,
        );
        assert_eq!(
            calls_of(&fp, id(node_kind::METHOD, "Job::run")),
            vec![CallQualifier::SelfMethod("tick".into())]
        );
    }

    #[test]
    fn receivers_this_super_and_complex() {
        let fp = parse(
            r#"
class A : Base() {
    private val svc: Svc = Svc()

    fun go() {
        this.a()
        this@A.a()
        super.go()
        this.svc.find()
        Repo.Companion.create()
        "hello".slugify()
        foo()()
    }

    fun a() {}
}

fun String.ext() {
    this.lowercase()
}
"#,
        );
        let complex = |r: &str, n: &str| CallQualifier::ComplexReceiver {
            receiver: r.into(),
            name: n.into(),
        };
        assert_eq!(
            calls_of(&fp, id(node_kind::METHOD, "A::go")),
            vec![
                CallQualifier::SelfMethod("a".into()),
                CallQualifier::SelfMethod("a".into()),
                CallQualifier::SuperMethod("go".into()),
                complex("this.svc", "find"),
                complex("Repo.Companion", "create"),
                complex("\"hello\"", "slugify"),
                bare("foo"),
            ]
        );
        // In an extension fun `this` is the extension receiver, not a class.
        assert_eq!(
            calls_of(&fp, id(node_kind::FUNCTION, "svc::ext")),
            vec![complex("this", "lowercase")]
        );
    }

    #[test]
    fn calls_inside_trailing_lambda_are_collected() {
        // Three lambdas deep; the Ktor verb / route heads are ROUTEs, never
        // calls, but what runs inside them is the enclosing fun's.
        let fp = parse(
            r#"
fun Application.itemRoutes(store: ItemStore) {
    routing {
        route("/api") {
            get("/items") {
                call.respond(store.all())
            }
        }
        get {
            audit()
        }
    }
    run(store) {
        store.add("x")
    }
}
"#,
        );
        assert_eq!(
            calls_of(&fp, id(node_kind::FUNCTION, "svc::itemRoutes")),
            vec![
                bare("routing"),
                attr("call", "respond"),
                attr("store", "all"),
                bare("audit"),
                bare("run"),
                attr("store", "add"),
            ]
        );
        assert!(!fp.calls.iter().any(|c| matches!(
            &c.qualifier,
            CallQualifier::Bare(n) if n == "get" || n == "route"
        )));
    }

    #[test]
    fn local_types_are_not_walked_but_anonymous_objects_are() {
        let fp = parse(
            r#"
class Host {
    fun go() {
        class Local {
            fun inner() { hidden() }
        }
        val o = object : Runnable {
            override fun run() {
                work()
            }
        }
        o.run()
    }

    fun work() {}
}
"#,
        );
        assert_eq!(
            calls_of(&fp, id(node_kind::METHOD, "Host::go")),
            vec![CallQualifier::SelfMethod("work".into()), attr("o", "run")]
        );
    }

    #[test]
    fn superclass_vs_interface_delegation() {
        let fp = parse(
            r#"
open class BaseService

interface Auditable

interface Named : Auditable, com.acme.Tagged<String>

class UserService : BaseService(), Auditable, Repo<User> by store

class UserController(private val userService: UserService) : Auditable

object Registry : BaseService()

enum class Status : Named { ACTIVE }

class Custom : View {
    constructor(ctx: Context) : super(ctx)
}

class Plain : Runnable {
    constructor() : super()
}
"#,
        );
        let user_service = id(node_kind::CLASS, "UserService");
        let registry = id(node_kind::CLASS, "Registry");
        let named = id(node_kind::INTERFACE, "Named");
        let custom = id(node_kind::CLASS, "Custom");
        assert_eq!(
            heritage(&fp, edge_category::INHERITS_FROM),
            vec![
                (named, "Auditable".into()),
                (named, "Tagged".into()),
                (user_service, "BaseService".into()),
                (registry, "BaseService".into()),
                (custom, "View".into()),
            ]
        );
        assert_eq!(
            heritage(&fp, edge_category::IMPLEMENTS),
            vec![
                (user_service, "Auditable".into()),
                (user_service, "Repo".into()),
                (id(node_kind::CLASS, "UserController"), "Auditable".into()),
                (id(node_kind::ENUM, "Status"), "Named".into()),
                (id(node_kind::CLASS, "Plain"), "Runnable".into()),
            ]
        );
        // Every heritage ref carries the file module.
        let module = id(node_kind::MODULE, "svc");
        assert!(fp.refs.iter().all(|r| r.from_module == module));
    }

    #[test]
    fn field_types_from_ctor_properties_and_typed_members() {
        let fp = parse(
            r#"
class Q(private val repo: UserRepo, val cache: Cache?, var names: List<String>, plain: Plain, val n: Int) {
    lateinit var mailer: Mailer
    private val box: Box<User>? = null
    val label: String = ""
    val inferred = Inferred()
    var client: com.acme.ApiClient? = null

    companion object {
        val shared: Shared = Shared()
    }
}
"#,
        );
        let q = id(node_kind::CLASS, "Q");
        let mut fields: Vec<(&str, &str)> = fp.nav.field_types[&q]
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            vec![
                ("cache", "Cache"),
                ("client", "ApiClient"),
                ("mailer", "Mailer"),
                ("repo", "UserRepo"),
                ("shared", "Shared"),
            ]
        );
        assert_eq!(fp.nav.field_types.len(), 1, "only Q owns fields");
    }

    /// `(endpoint qname, caller qname)` of every CALLS edge into an ENDPOINT,
    /// in emission order.
    fn endpoint_calls(fp: &FileParse) -> Vec<(String, String)> {
        fp.edges
            .iter()
            .filter(|e| e.category == edge_category::CALLS)
            .filter(|e| fp.nav.kind_by_id.get(&e.to) == Some(&node_kind::ENDPOINT))
            .map(|e| {
                (
                    fp.nav.qname_by_id.get(&e.to).cloned().unwrap_or_default(),
                    fp.nav.qname_by_id.get(&e.from).cloned().unwrap_or_default(),
                )
            })
            .collect()
    }

    #[test]
    fn spring_rest_template_and_webclient_emit_endpoints() {
        // A14.6: kotlin-flip-guard's client.kt — the ENDPOINTs the Java
        // grammar found on .kt before A14.2's flip, now off the Kotlin chain —
        // plus the exchange / method / template / concatenation forms.
        let fp = parse(
            r#"
class UserClient {
    private val restTemplate = RestTemplate()

    fun fetch(): String {
        return restTemplate.getForObject("/api/users", String::class.java)
    }

    fun post(): String {
        val r = webClient.post().uri("/api/orders").retrieve()
        return "x"
    }

    fun more(id: Long) {
        restTemplate.exchange("/api/users/" + id, HttpMethod.PUT, null, String::class.java)
        webClient.method(HttpMethod.PATCH).uri("/api/items/$id").retrieve()
        this.restTemplate.delete("http://users.svc:8080/api/users/${id}")
        restClient.get().uri("/api/health").retrieve()
    }
}
"#,
        );
        assert_eq!(
            endpoint_calls(&fp),
            vec![
                ("endpoint:GET:/api/users".to_string(), "UserClient::fetch".to_string()),
                ("endpoint:POST:/api/orders".to_string(), "UserClient::post".to_string()),
                ("endpoint:PUT:/api/users/${…}".to_string(), "UserClient::more".to_string()),
                ("endpoint:PATCH:/api/items/${…}".to_string(), "UserClient::more".to_string()),
                ("endpoint:DELETE:/api/users/${…}".to_string(), "UserClient::more".to_string()),
                ("endpoint:GET:/api/health".to_string(), "UserClient::more".to_string()),
            ]
        );
        // The ordinary CallSites are still there beside the ENDPOINT edge.
        assert!(calls_of(&fp, id(node_kind::METHOD, "UserClient::fetch"))
            .contains(&attr("restTemplate", "getForObject")));
        // Literal: Strong; the absolute URL's authority rides as `host`.
        let hit = |q: &str| {
            let n = fp
                .nodes
                .iter()
                .find(|n| n.id == id(node_kind::ENDPOINT, q))
                .expect("endpoint");
            match &n.cells[0].payload {
                repo_graph_core::CellPayload::Json(s) => {
                    serde_json::from_str::<serde_json::Value>(s).unwrap()
                }
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(hit("endpoint:GET:/api/users")["confidence"], "strong");
        assert_eq!(hit("endpoint:GET:/api/users")["line"], 6, "1-based");
        assert_eq!(hit("endpoint:PUT:/api/users/${…}")["confidence"], "medium");
        assert_eq!(hit("endpoint:DELETE:/api/users/${…}")["host"], "users.svc:8080");
        let counts = crate::parse_all(
            "fun f() {\n    rest.getForEntity(\"/a\", A::class.java)\n}\n",
            "f.kt",
            "f",
            RepoId(1),
        )
        .unwrap()
        .clients;
        assert_eq!(counts.spring_clients, 1);
    }

    #[test]
    fn server_routes_and_map_calls_are_not_spring_clients() {
        // Javalin's `app.delete("/x", h)` registers a SERVER route; a Map's
        // `put`, a relative key, a JDK builder's `.uri(…)` and a Ktor DSL
        // `delete("/x") { }` are no client calls either.
        let fp = parse(
            r#"
fun main() {
    val app = Javalin.create()
    app.delete("/users/{id}", UserHandler::delete)
    app.put("/users", UserHandler::update)
    cache.put("/key", value)
    restTemplate.getForObject("users", String::class.java)
    val req = HttpRequest.newBuilder().uri(URI.create("http://x/y")).GET().build()
    routing {
        delete("/items/{id}") { call.respond("ok") }
    }
}
"#,
        );
        assert!(endpoint_calls(&fp).is_empty(), "{:?}", endpoint_calls(&fp));
    }

    #[test]
    fn refs_marker_counts_off_the_parses() {
        let fp = parse(
            r#"
class UserService : BaseService(), Auditable {
    private val repo: Repo? = null

    fun a() {
        b()
        repo?.find()
    }

    fun b() {}
}
"#,
        );
        assert_eq!(
            super::marker(&[fp], "r"),
            "[kotlin] refs: calls=2 self=1 inherits=1 implements=1 field_types=1 repo=r"
        );
    }
}
