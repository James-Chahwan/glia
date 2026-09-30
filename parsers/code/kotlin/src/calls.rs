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
//! through `svc`'s field type in the graph.
//!
//! # Local receiver types (CA.6a)
//!
//! A receiver's type comes from the caller's own locals before the enclosing
//! type's properties: [`record_local_types`] records every local a function
//! binds on `CodeNav::local_types` (LA.35a's carrier, which the generic
//! `resolve_calls` reads innermost-first), so
//! `fun Application.itemRoutes(store: ItemStore) { store.all() }` binds
//! `ItemStore::all` through the Kotlin file's `import ..ItemStore`. A typed
//! parameter, a typed `val` / `var` and a constructor-initialised
//! `val log = AuditLog()` carry their type; `val r = repo` / `val r =
//! this.repo` naming a property of the enclosing type aliases it (`self.repo`,
//! read through the property's declared type); any other local is a local of
//! unknown type (`""`), which shadows a same-named property rather than bind
//! through it.
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
//! # HTTP client calls
//!
//! Two per-call detectors turn a client call into a client ENDPOINT + CALLS
//! from the enclosing function, through [`android`]'s shared sink: Spring's
//! RestTemplate / WebClient chain ([`spring_client`], A14.6) and, when that
//! did not fire, a Ktor-client verb call ([`ktor_client`], CA.6b) in a file
//! importing `io.ktor.client`. Both keep the call's ordinary CallSite.
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

use glia_code_domain::{CallQualifier, CallSite, UnresolvedRef, endpoint, jvm};
use glia_core::{EdgeCategoryId, NodeId};
use tree_sitter::Node as TsNode;

use crate::{Acc, File, android, edge_category, line_at, named_child_of_kind, routes, text_of};

/// The declaration a function belongs to, as its body's receiver-less calls
/// see it.
#[derive(Clone, Copy)]
pub(crate) struct CallScope<'a> {
    /// Methods declared in the enclosing class / object body (companion
    /// members included) — empty at the top level. A receiver-less call to one
    /// of them is `SelfMethod`.
    pub(crate) members: &'a HashSet<String>,
    /// Properties the enclosing type declares ([`member_prop_names`]) — empty
    /// at the top level. A local initialised from one (`val r = repo`)
    /// aliases it ([`record_local_types`]).
    pub(crate) fields: &'a HashSet<String>,
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
    for_each_member(body, |child| {
        if child.kind() == "function_declaration"
            && let Some(name) = child.child_by_field_name("name")
        {
            out.insert(text_of(name, src).to_string());
        }
    });
    out
}

/// Names of the properties a type declares: its primary-constructor `val` /
/// `var` parameters and every `val` / `var` of its body (companion members
/// included, as [`member_fn_names`]) — what a bare `repo` or `this.repo` in
/// one of its methods names when no local claims it.
pub(crate) fn member_prop_names(decl: TsNode, body: TsNode, src: &[u8]) -> HashSet<String> {
    let mut out: HashSet<String> = ctor_property_params(decl)
        .into_iter()
        .filter_map(|p| named_child_of_kind(p, &["identifier"]))
        .map(|name| text_of(name, src).to_string())
        .collect();
    for_each_member(body, |child| {
        if child.kind() == "property_declaration"
            && let Some(name) = named_child_of_kind(child, &["variable_declaration"])
                .and_then(|v| named_child_of_kind(v, &["identifier"]))
        {
            out.insert(text_of(name, src).to_string());
        }
    });
    out
}

/// Visit every member declaration of a class / object / enum body. Mirrors
/// `walk_members`: a `companion object`'s members attach to the type, and an
/// `ERROR` node is a transparent container.
fn for_each_member(body: TsNode, mut visit: impl FnMut(TsNode)) {
    let mut stack = vec![body];
    while let Some(container) = stack.pop() {
        let mut cursor = container.walk();
        for child in container.named_children(&mut cursor) {
            match child.kind() {
                "companion_object" => {
                    if let Some(b) = named_child_of_kind(child, &["class_body"]) {
                        stack.push(b);
                    }
                }
                "ERROR" => stack.push(child),
                _ => visit(child),
            }
        }
    }
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
    record_local_types(fun, body, &ctx, file, acc);
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
            line: line_at(call),
        });
    }
    if !spring_client(call, ctx.from, file, acc) {
        ktor_client(call, ctx.from, file, acc);
    }
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
///
/// Returns whether it emitted, so [`visit_call`] never reads the same call as
/// a Ktor-client call too (`restClient.delete("/x")` in a Ktor-importing file).
fn spring_client(call: TsNode, from: NodeId, file: &File, acc: &mut Acc) -> bool {
    let src = file.src;
    let Some(head) = call.named_child(0).filter(|h| h.kind() == "navigation_expression") else {
        return false;
    };
    let Some((base, name)) = nav_parts(head, src) else {
        return false;
    };
    let args = named_child_of_kind(call, &["value_arguments"]);
    let verb = match name {
        "uri" => webclient_verb(base, src),
        "exchange" | "execute" => args.and_then(|a| http_method_arg_verb(a, src)),
        "put" | "delete" if !is_rest_template_receiver(base, src) => None,
        other => endpoint::rest_template_verb(other),
    };
    let Some(verb) = verb else {
        return false;
    };
    if !emit_url_endpoint(call, verb, args, from, file, acc) {
        return false;
    }
    acc.clients.spring_clients += 1;
    true
}

/// CA.6b: a Ktor-client call → a client ENDPOINT + CALLS from the enclosing
/// function `from`, in a file importing `io.ktor.client`
/// ([`android::imports_ktor_client`]; without it nothing fires, so a Map's
/// `get`, Javalin's `app.get("/x", h)` and a Ktor server route are all
/// outside the gate).
///
/// - The call has a receiver: its head is a `navigation_expression`
///   (`client.get(..)`, `HttpClient().post(..)`). The server DSL's
///   `get("/x") { }` is receiver-less and stays a ROUTE ([`routes::ktor_call`]).
/// - The verb comes from the member name ([`ktor_verb`]): `get` / `post` /
///   `put` / `delete` / `patch` / `head` / `options` and their `prepare*`
///   forms; `request` / `prepareRequest` read the trailing lambda's
///   `method = HttpMethod.<X>` ([`ktor_block_method`]), GET (Ktor's default)
///   when it sets none, nothing when it sets one the parser cannot read.
/// - The URL is the first positional argument, read like Spring's: a literal
///   (Strong), a template or `+` concatenation (Medium, interpolations as
///   `${…}`), a path that must start with `/` after the authority is split
///   off. The builder form `client.get { url("..") }` has no positional URL
///   and emits nothing.
///
/// The trailing lambda of `client.post("/x") { .. }` is not a child of this
/// call: the grammar parses it as `call_expression [call_expression
/// client.post("/x"), annotated_lambda]`, so it is read off the parent.
fn ktor_client(call: TsNode, from: NodeId, file: &File, acc: &mut Acc) {
    let src = file.src;
    let Some(head) = call
        .named_child(0)
        .filter(|h| h.kind() == "navigation_expression")
    else {
        return;
    };
    let Some((_, name)) = nav_parts(head, src) else {
        return;
    };
    let Some(form) = ktor_verb(name) else {
        return;
    };
    if !android::imports_ktor_client(acc) {
        return;
    }
    let verb = match form {
        KtorVerb::Named(verb) => verb,
        KtorVerb::FromBlock => {
            match trailing_lambda(call).and_then(|l| ktor_block_method(l, src)) {
                None => "GET",
                Some(Some(verb)) => verb,
                Some(None) => return,
            }
        }
    };
    let args = named_child_of_kind(call, &["value_arguments"]);
    if emit_url_endpoint(call, verb, args, from, file, acc) {
        acc.clients.ktor_clients += 1;
    }
}

/// How a Ktor-client member name gives its HTTP verb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KtorVerb {
    /// The name is the verb: `get` / `prepareGet` → `GET`.
    Named(&'static str),
    /// `request` / `prepareRequest`: the verb is the block's `method =`.
    FromBlock,
}

/// The verb form of a Ktor-client request function: `get`, `post`, `put`,
/// `delete`, `patch`, `head`, `options` (lower-case, Ktor's spelling), the
/// same with a `prepare` prefix (`prepareGet`, …), and `request` /
/// `prepareRequest`. `None` for anything else (`submitForm`, `webSocket`,
/// `getForObject`, an upper-case `GET`).
fn ktor_verb(name: &str) -> Option<KtorVerb> {
    let bare = match name.strip_prefix("prepare") {
        Some(rest) => {
            let mut chars = rest.chars();
            let first = chars.next().filter(char::is_ascii_uppercase)?;
            format!("{}{}", first.to_ascii_lowercase(), chars.as_str())
        }
        None => name.to_string(),
    };
    if bare == "request" {
        return Some(KtorVerb::FromBlock);
    }
    if bare.chars().any(|c| c.is_ascii_uppercase()) {
        return None;
    }
    endpoint::jaxrs_verb(&bare.to_ascii_uppercase()).map(KtorVerb::Named)
}

/// The `annotated_lambda` a call is completed by: its own child, or — the
/// grammar's shape for `recv.m(args) { .. }` — its parent's, when the parent
/// is a `call_expression` headed by this call.
fn trailing_lambda(call: TsNode) -> Option<TsNode> {
    if let Some(own) = named_child_of_kind(call, &["annotated_lambda"]) {
        return Some(own);
    }
    let parent = call.parent().filter(|p| p.kind() == "call_expression")?;
    if parent.named_child(0)?.id() != call.id() {
        return None;
    }
    named_child_of_kind(parent, &["annotated_lambda"])
}

/// The last `method = …` statement of a Ktor request block (the lambda's own
/// statements, never a nested lambda's): `None` when it sets no method,
/// `Some(Some(verb))` for `method = HttpMethod.Put` (the Java / Spring
/// `HttpMethod.<X>` reader, case-folded), `Some(None)` for a value the parser
/// cannot read (`method = m`, `HttpMethod("PURGE")`).
fn ktor_block_method(lambda: TsNode, src: &[u8]) -> Option<Option<&'static str>> {
    // tree-sitter-kotlin-ng 1.1: a `lambda_literal`'s statements are its
    // direct children (there is no `statements` wrapper).
    let body = named_child_of_kind(lambda, &["lambda_literal"])?;
    let mut cursor = body.walk();
    body.named_children(&mut cursor)
        .filter(|stmt| stmt.kind() == "assignment")
        .filter(|stmt| {
            stmt.child_by_field_name("operator")
                .is_some_and(|op| op.kind() == "=")
                && stmt
                    .child_by_field_name("left")
                    .is_some_and(|l| matches!(text_of(l, src), "method" | "this.method"))
        })
        .last()
        .map(|stmt| {
            stmt.child_by_field_name("right")
                .and_then(|r| endpoint::http_method_ref_verb(text_of(r, src)))
        })
}

/// The client ENDPOINT a call's first positional argument names, emitted with
/// `verb` (+ CALLS from `from`) — the URL rules [`spring_client`] and
/// [`ktor_client`] share. `false` when the argument is no readable URL or its
/// path does not start with `/` after the authority is split off.
fn emit_url_endpoint(
    call: TsNode,
    verb: &str,
    args: Option<TsNode>,
    from: NodeId,
    file: &File,
    acc: &mut Acc,
) -> bool {
    let Some((raw, strong)) = args
        .and_then(first_arg_value)
        .and_then(|arg| android::client_url(arg, file.src))
    else {
        return false;
    };
    let (host, path) = endpoint::client_url_split(&raw);
    let Some(path) = path else {
        return false;
    };
    android::emit_client_endpoint(call, verb, path, host.as_deref(), strong, from, file, acc);
    true
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
    for p in params_of(fun) {
        if let Some(name) = named_child_of_kind(p, &["identifier"]) {
            out.insert(text_of(name, src).to_string());
        }
    }
    walk_body(body, |node| match node.kind() {
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
    });
    out
}

/// The `parameter`s of a `function_declaration` / anonymous function.
fn params_of(fun: TsNode) -> Vec<TsNode> {
    let Some(params) = named_child_of_kind(fun, &["function_value_parameters"]) else {
        return Vec::new();
    };
    let mut cursor = params.walk();
    params
        .named_children(&mut cursor)
        .filter(|p| p.kind() == "parameter")
        .collect()
}

/// Every node of a function body in source order, never entering a local
/// `class` / `object` declaration (another receiver's code) — the one walk
/// [`local_names`] and [`record_local_types`] share.
fn walk_body(body: TsNode, mut visit: impl FnMut(TsNode)) {
    let mut stack = vec![body];
    while let Some(node) = stack.pop() {
        if matches!(node.kind(), "class_declaration" | "object_declaration") {
            continue;
        }
        visit(node);
        let mut cursor = node.walk();
        let children: Vec<TsNode> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
}

/// CA.6a: record the receiver type of every local `ctx.from` binds on
/// `acc.nav.local_types`, which `resolve_calls`' receiver pass reads before
/// the enclosing type's property types (LA.35a):
///
/// - a parameter — of the function, and of a local `fun` or an anonymous
///   object's method, whose calls are this function's — and a
///   `catch (e: T)` parameter: its declared type through [`field_type_name`]
///   (a generic, value or function type is `""`);
/// - a `val` / `var` / `for` / lambda / destructuring variable with a declared
///   type: that type, the same way;
/// - an untyped local `val` / `var`: its initializer's type
///   ([`initializer_type`]);
/// - any other variable (`for (x in xs)`, `{ x -> .. }`, `val (a, b) = p`,
///   `val x by lazy { .. }`): `""`.
///
/// `""` is a local of unknown type: it shadows a same-named property in the
/// receiver pass rather than bind through it. The body is flattened into one
/// scope, so a name bound to two types (a nested block, a lambda parameter
/// reusing a name) becomes `""` through `record_local_type`: a lost edge,
/// never a guessed one.
fn record_local_types(fun: TsNode, body: TsNode, ctx: &Body, file: &File, acc: &mut Acc) {
    let src = file.src;
    for p in params_of(fun) {
        record_declared(p, ctx.from, src, acc);
    }
    walk_body(body, |node| match node.kind() {
        // A local `fun`'s or an anonymous object method's parameters.
        "function_value_parameters" => {
            let mut cursor = node.walk();
            for p in node.named_children(&mut cursor) {
                if p.kind() == "parameter" {
                    record_declared(p, ctx.from, src, acc);
                }
            }
        }
        "catch_block" => record_declared(node, ctx.from, src, acc),
        "variable_declaration" => {
            let Some(name) = named_child_of_kind(node, &["identifier"]) else {
                return;
            };
            let mut cursor = node.walk();
            let declared = node
                .named_children(&mut cursor)
                .any(|c| c.kind().ends_with("_type"));
            let ty = if declared {
                field_type_name(node, src).unwrap_or_default()
            } else {
                // Only a `val` / `var`'s own variable has an initializer: a
                // destructuring component's parent is the
                // `multi_variable_declaration`.
                node.parent()
                    .filter(|p| p.kind() == "property_declaration")
                    .and_then(crate::initializer)
                    .map(|init| initializer_type(init, text_of(name, src), ctx, src))
                    .unwrap_or_default()
            };
            acc.nav.record_local_type(ctx.from, text_of(name, src), &ty);
        }
        _ => {}
    });
}

/// A `parameter` / `catch_block`: its `identifier` bound to its declared type
/// ([`field_type_name`]; `""` when it names none a receiver can bind through).
fn record_declared(decl: TsNode, from: NodeId, src: &[u8], acc: &mut Acc) {
    if let Some(name) = named_child_of_kind(decl, &["identifier"]) {
        let ty = field_type_name(decl, src).unwrap_or_default();
        acc.nav.record_local_type(from, text_of(name, src), &ty);
    }
}

/// The type an untyped local `val <declared> = <init>` gets, or `""`:
///
/// - a constructor call `AuditLog(..)` — an upper-case `identifier` head that
///   names no local — is `AuditLog` (a generic `Box<T>()` and a value type
///   `String()` are `""`, the declared-type rule);
/// - `repo` / `this.repo` / `this@Own.repo` naming a property of the enclosing
///   type is the alias `self.repo`, which the receiver pass reads through the
///   property's declared type. A bare `repo` that is a local names the local
///   (`""`), except in its own initializer (`val repo = repo` reads the
///   property: a local is not in scope in its own initializer). An unlabeled
///   `this` in an extension function is the extension receiver: `""`;
/// - anything else (a call chain, a factory, another local): `""`.
fn initializer_type(init: TsNode, declared: &str, ctx: &Body, src: &[u8]) -> String {
    let property = |name: &str| ctx.scope.fields.contains(name).then(|| format!("self.{name}"));
    let ty = match init.kind() {
        "call_expression" => init
            .named_child(0)
            .filter(|head| head.kind() == "identifier")
            .filter(|_| named_child_of_kind(init, &["type_arguments"]).is_none())
            .map(|head| text_of(head, src))
            .filter(|name| {
                name.starts_with(char::is_uppercase)
                    && !ctx.locals.contains(*name)
                    && !jvm::is_non_injectable_type(name)
                    && !jvm::is_kotlin_value_type(name)
            })
            .map(str::to_string),
        "identifier" => {
            let name = text_of(init, src);
            (name == declared || !ctx.locals.contains(name))
                .then(|| property(name))
                .flatten()
        }
        "navigation_expression" => {
            let mut cursor = init.walk();
            let parts: Vec<TsNode> = init.named_children(&mut cursor).collect();
            match parts.as_slice() {
                [this, field]
                    if this.kind() == "this_expression"
                        && field.kind() == "identifier"
                        && is_own_this(*this, ctx, src) =>
                {
                    property(text_of(*field, src))
                }
                _ => None,
            }
        }
        _ => None,
    };
    ty.unwrap_or_default()
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
        push_heritage_ref(name, category, from, line_at(part), file, acc);
    }
}

/// The Java parser's `emit_heritage_ref` shape: a `Bare(SimpleName)` ref the
/// graph's `resolve_refs` binds through imports, the module's own symbols or a
/// repo-unique type name.
fn push_heritage_ref(
    name: String,
    category: EdgeCategoryId,
    from: NodeId,
    line: u32,
    file: &File,
    acc: &mut Acc,
) {
    acc.refs.push(UnresolvedRef {
        from,
        from_module: file.module_id,
        qualifier: CallQualifier::Bare(name),
        category,
        line,
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
    for param in ctor_property_params(decl) {
        let Some(name) = named_child_of_kind(param, &["identifier"]) else {
            continue;
        };
        if let Some(ty) = field_type_name(param, file.src) {
            acc.nav
                .record_field_type(owner, text_of(name, file.src), &ty);
        }
    }
}

/// The primary-constructor parameters that declare a property (`val` /
/// `var`); a plain constructor parameter is none.
fn ctor_property_params(decl: TsNode) -> Vec<TsNode> {
    let Some(params) = named_child_of_kind(decl, &["primary_constructor"])
        .and_then(|c| named_child_of_kind(c, &["class_parameters"]))
    else {
        return Vec::new();
    };
    let mut cursor = params.walk();
    params
        .named_children(&mut cursor)
        .filter(|param| param.kind() == "class_parameter")
        .filter(|param| {
            let mut c = param.walk();
            param
                .children(&mut c)
                .any(|t| !t.is_named() && matches!(t.kind(), "val" | "var"))
        })
        .collect()
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
pub(crate) fn marker(parses: &[glia_code_domain::FileParse], repo_label: &str) -> String {
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

/// CA.6a's `[kotlin] local types:` marker line over a repo's parses, counted
/// off `nav.local_types` so cache-served files count too: `scopes` function
/// bodies that bind a local, `typed` locals with a receiver type (an alias
/// `self.<p>` included), `unknown` locals of unknown type (`""`).
pub(crate) fn local_types_marker(parses: &[glia_code_domain::FileParse], repo_label: &str) -> String {
    let scopes: usize = parses.iter().map(|fp| fp.nav.local_types.len()).sum();
    let (typed, unknown) = parses
        .iter()
        .flat_map(|fp| fp.nav.local_types.values())
        .flat_map(|locals| locals.values())
        .fold((0usize, 0usize), |(t, u), ty| {
            if ty.is_empty() { (t, u + 1) } else { (t + 1, u) }
        });
    format!("[kotlin] local types: scopes={scopes} typed={typed} unknown={unknown} repo={repo_label}")
}

#[cfg(test)]
mod tests {
    use glia_code_domain::{
        CallQualifier, FileParse, UnresolvedRef, edge_category, node_kind,
    };
    use glia_core::{EdgeCategoryId, NodeId, RepoId};

    use crate::{GRAPH_TYPE, parse_file};

    fn parse(source: &str) -> FileParse {
        parse_file(source, "svc.kt", "svc", RepoId(1)).unwrap()
    }

    fn id(kind: glia_core::NodeKindId, qname: &str) -> NodeId {
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
                glia_core::CellPayload::Json(s) => {
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

    /// The ENDPOINT_HIT JSON of the ENDPOINT `qname`.
    fn hit_of(fp: &FileParse, qname: &str) -> serde_json::Value {
        let node = fp
            .nodes
            .iter()
            .find(|n| n.id == id(node_kind::ENDPOINT, qname))
            .unwrap_or_else(|| panic!("no endpoint {qname}"));
        match &node.cells[0].payload {
            glia_core::CellPayload::Json(s) => serde_json::from_str(s).unwrap(),
            other => panic!("{other:?}"),
        }
    }

    fn owned(want: &[(&str, &str)]) -> Vec<(String, String)> {
        want.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    /// CA.6b: the kotlin-ktor-client fixture's OrdersClient.kt.
    const ORDERS_CLIENT: &str = r#"
package com.acme.client

import io.ktor.client.HttpClient
import io.ktor.client.call.body
import io.ktor.client.request.get
import io.ktor.client.request.post
import io.ktor.client.request.setBody

class OrdersClient(private val client: HttpClient) {
    suspend fun listOrders(): String = client.get("http://orders:8080/api/orders").body()

    suspend fun createOrder(body: String): String =
        client.post("/api/orders") { setBody(body) }.body()

    suspend fun getOrder(id: String): String = client.get("/api/orders/$id").body()
}
"#;

    #[test]
    fn ktor_client_verbs_emit_endpoints() {
        let fp = parse(ORDERS_CLIENT);
        assert_eq!(
            endpoint_calls(&fp),
            owned(&[
                ("endpoint:GET:/api/orders", "OrdersClient::listOrders"),
                ("endpoint:POST:/api/orders", "OrdersClient::createOrder"),
                ("endpoint:GET:/api/orders/${…}", "OrdersClient::getOrder"),
            ])
        );
        // The ordinary CallSite stays beside the ENDPOINT edge.
        assert!(calls_of(&fp, id(node_kind::METHOD, "OrdersClient::listOrders"))
            .contains(&attr("client", "get")));
        // The literal authority rides as `host`; a literal is Strong, a
        // template Medium; the line is the call's, 1-based.
        let list = hit_of(&fp, "endpoint:GET:/api/orders");
        assert_eq!(list["host"], "orders:8080");
        assert_eq!(list["confidence"], "strong");
        assert_eq!(list["line"], 11);
        assert!(hit_of(&fp, "endpoint:POST:/api/orders").get("host").is_none());
        assert_eq!(hit_of(&fp, "endpoint:GET:/api/orders/${…}")["confidence"], "medium");
        // No ROUTE: a client call is never a server route.
        assert!(!fp.nav.kind_by_id.values().any(|k| *k == node_kind::ROUTE));
        let counts = crate::parse_all(ORDERS_CLIENT, "o.kt", "o", RepoId(1)).unwrap().clients;
        assert_eq!((counts.ktor_clients, counts.spring_clients), (3, 0));
    }

    #[test]
    fn ktor_request_reads_the_method() {
        let fp = parse(
            r#"
import io.ktor.client.*
import io.ktor.client.request.*
import io.ktor.http.HttpMethod

fun calls(client: HttpClient, m: HttpMethod) {
    client.request("/x") { method = HttpMethod.Put }
    client.request("/y")
    client.prepareRequest("/z") {
        header("A", "b")
        method = HttpMethod.Delete
    }
    client.request("/unread") { method = m }
    client.preparePost("/p").execute()
    HttpClient().patch("https://api.acme.io/items/${m.value}")
    client.get { url("/builder") }
    client.submitForm("/form", parameters)
    client.GET("/upper")
    client.get("relative")
    restClient.delete("/both")
}
"#,
        );
        assert_eq!(
            endpoint_calls(&fp),
            owned(&[
                ("endpoint:PUT:/x", "svc::calls"),
                ("endpoint:GET:/y", "svc::calls"),
                ("endpoint:DELETE:/z", "svc::calls"),
                ("endpoint:POST:/p", "svc::calls"),
                ("endpoint:PATCH:/items/${…}", "svc::calls"),
                ("endpoint:DELETE:/both", "svc::calls"),
            ])
        );
        assert_eq!(hit_of(&fp, "endpoint:PATCH:/items/${…}")["host"], "api.acme.io");
        // `restClient.delete` is Spring's (a rest-named receiver): counted
        // once, never as a Ktor call too.
        let src = "import io.ktor.client.HttpClient\n\nfun f() {\n    restClient.delete(\"/both\")\n    client.delete(\"/k\")\n}\n";
        let counts = crate::parse_all(src, "f.kt", "f", RepoId(1)).unwrap().clients;
        assert_eq!((counts.spring_clients, counts.ktor_clients), (1, 1));
    }

    #[test]
    fn no_ktor_import_no_endpoint() {
        // The fixture's Cache.kt: a map's get / put, no io.ktor.client import.
        let fp = parse(
            r#"
package com.acme.client

class Cache(private val entries: MutableMap<String, String>) {
    fun get(key: String): String? = entries.get("/api/orders")
    fun post(key: String, v: String) { entries.put(key, v) }
    fun more(client: Any) { client.post("/api/orders") }
}
"#,
        );
        assert!(endpoint_calls(&fp).is_empty(), "{:?}", endpoint_calls(&fp));
        // Only the server artifact imported: still no client.
        let server = parse(
            "import io.ktor.server.routing.*\n\nfun f(client: Any) {\n    client.get(\"/api/orders\")\n}\n",
        );
        assert!(endpoint_calls(&server).is_empty(), "{:?}", endpoint_calls(&server));
    }

    #[test]
    fn server_dsl_is_not_a_client() {
        // A server that proxies upstream: the receiver-less `get("/x") { }`
        // is its ROUTE, `client.get(..)` inside the handler its client call,
        // and `call.parameters.get("id")` (no `/` path) neither.
        let fp = parse(
            r#"
import io.ktor.client.HttpClient
import io.ktor.client.request.get
import io.ktor.server.application.Application
import io.ktor.server.routing.get
import io.ktor.server.routing.routing

fun Application.proxy(client: HttpClient) {
    routing {
        get("/x") {
            val id = call.parameters.get("id")
            call.respond(client.get("/upstream/$id").body<String>())
        }
        post("/y") { call.respond("ok") }
    }
}
"#,
        );
        assert_eq!(
            endpoint_calls(&fp),
            owned(&[("endpoint:GET:/upstream/${…}", "svc::proxy")])
        );
        for route in ["GET /x", "POST /y"] {
            assert!(
                fp.nodes.iter().any(|n| n.id == id(node_kind::ROUTE, route)),
                "{route} stays a ROUTE"
            );
        }
        assert!(!fp.nav.qname_by_id.values().any(|q| q == "endpoint:GET:/x"));
    }

    #[test]
    fn ktor_verb_names() {
        use super::{KtorVerb, ktor_verb};
        assert_eq!(ktor_verb("get"), Some(KtorVerb::Named("GET")));
        assert_eq!(ktor_verb("options"), Some(KtorVerb::Named("OPTIONS")));
        assert_eq!(ktor_verb("prepareHead"), Some(KtorVerb::Named("HEAD")));
        assert_eq!(ktor_verb("request"), Some(KtorVerb::FromBlock));
        assert_eq!(ktor_verb("prepareRequest"), Some(KtorVerb::FromBlock));
        for no in ["GET", "Get", "prepare", "prepareget", "preparerequest", "getForObject", "submitForm", "webSocket", "body"] {
            assert_eq!(ktor_verb(no), None, "{no}");
        }
    }

    /// `(name, type)` of the locals `local_types` records for `scope`, sorted.
    fn locals_of(fp: &FileParse, scope: NodeId) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = fp
            .nav
            .local_types
            .get(&scope)
            .map(|l| l.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        out.sort_unstable();
        out
    }

    fn pairs(want: &[(&str, &str)]) -> Vec<(String, String)> {
        want.iter().map(|(n, t)| (n.to_string(), t.to_string())).collect()
    }

    #[test]
    fn params_and_vals_record_local_types() {
        // CA.6a: fixture kotlin-local-receivers' service.kt.
        let fp = parse(
            r#"
package com.acme.app

import com.acme.store.AuditLog
import com.acme.store.ItemStore

class Service {
    fun listAll(store: ItemStore): List<String> = store.all()

    fun audited(): String {
        val log: AuditLog = AuditLog()
        return log.write("typed")
    }

    fun inferred(): String {
        val log = AuditLog()
        return log.write("inferred")
    }

    fun shadowed(store: ItemStore): String {
        val log = store
        return log.write("aliased")
    }
}
"#,
        );
        let m = |n: &str| id(node_kind::METHOD, &format!("Service::{n}"));
        assert_eq!(locals_of(&fp, m("listAll")), pairs(&[("store", "ItemStore")]));
        assert_eq!(locals_of(&fp, m("audited")), pairs(&[("log", "AuditLog")]));
        assert_eq!(locals_of(&fp, m("inferred")), pairs(&[("log", "AuditLog")]));
        assert_eq!(
            locals_of(&fp, m("shadowed")),
            pairs(&[("log", ""), ("store", "ItemStore")])
        );
        // The receivers stay Attribute CallSites: the graph reads the locals.
        assert_eq!(calls_of(&fp, m("listAll")), vec![attr("store", "all")]);
    }

    #[test]
    fn alias_of_a_property_is_self() {
        let fp = parse(
            r#"
class S(private val repo: Repo) {
    private val cache: Cache = Cache()

    fun f() {
        val r = repo
        r.x()
    }

    fun g(repo: Other) {
        val r = repo
    }

    fun h() {
        val repo = repo
        val t = this.repo
        val u = this@S.cache
        val v = other
        val w = this.missing
    }

    fun String.ext() {
        val t = this.repo
    }
}

fun top() {
    val r = repo
}
"#,
        );
        let m = |n: &str| id(node_kind::METHOD, &format!("S::{n}"));
        assert_eq!(locals_of(&fp, m("f")), pairs(&[("r", "self.repo")]));
        // A parameter named like the property is the parameter: `r` is a local
        // of unknown type.
        assert_eq!(locals_of(&fp, m("g")), pairs(&[("r", ""), ("repo", "Other")]));
        // `val repo = repo` reads the property (a local is not in scope in its
        // own initializer); `this.missing` / `other` name no property.
        assert_eq!(
            locals_of(&fp, m("h")),
            pairs(&[
                ("repo", "self.repo"),
                ("t", "self.repo"),
                ("u", "self.cache"),
                ("v", ""),
                ("w", ""),
            ])
        );
        // In an extension fun an unlabeled `this` is the extension receiver.
        assert_eq!(locals_of(&fp, m("ext")), pairs(&[("t", "")]));
        // At the top level no type is in scope.
        assert_eq!(
            locals_of(&fp, id(node_kind::FUNCTION, "svc::top")),
            pairs(&[("r", "")])
        );
    }

    #[test]
    fn generic_param_is_unknown() {
        let fp = parse(
            "fun f(xs: List<Item>, n: Int, cb: (Int) -> Repo, r: Repo?, p: Provider<Repo>, vararg ids: Long) {}\n",
        );
        assert_eq!(
            locals_of(&fp, id(node_kind::FUNCTION, "svc::f")),
            pairs(&[
                ("cb", ""),
                ("ids", ""),
                ("n", ""),
                ("p", ""),
                ("r", "Repo"),
                ("xs", ""),
            ])
        );
    }

    #[test]
    fn local_types_cover_every_binding_form() {
        let fp = parse(
            r#"
class Job {
    fun run(items: List<Item>) {
        val a: Repo? = null
        val b = Gen<Int>()
        val c = String()
        val d = com.acme.Repo()
        val e by lazy { Repo() }
        val later: Mailer
        var f = Repo()
        val (g, h) = pair
        val (i: Index, j) = pair
        for (k in items) { k.go() }
        items.forEach { l -> l.go() }
        items.forEach { m: Mailer -> m.send() }
        val n = make()
        try { } catch (o: IoFailure) { o.log() }
        fun local(p: Printer) { p.print() }
        val q = object : Runnable {
            override fun run(s: Sink) {}
        }
        class Hidden {
            fun inner(z: Zed) { val y = Zed() }
        }
    }
}
"#,
        );
        assert_eq!(
            locals_of(&fp, id(node_kind::METHOD, "Job::run")),
            pairs(&[
                ("a", "Repo"),
                ("b", ""),
                ("c", ""),
                ("d", ""),
                ("e", ""),
                ("f", "Repo"),
                ("g", ""),
                ("h", ""),
                ("i", "Index"),
                ("items", ""),
                ("j", ""),
                ("k", ""),
                ("l", ""),
                ("later", "Mailer"),
                ("m", "Mailer"),
                ("n", ""),
                ("o", "IoFailure"),
                ("p", "Printer"),
                ("q", ""),
                ("s", "Sink"),
            ])
        );
        // A local class is another receiver's code: its fun records nothing
        // on `run`, and is no declared function of this file's walk.
        assert!(!fp.nav.local_types.values().any(|l| l.contains_key("z") || l.contains_key("y")));
    }

    #[test]
    fn a_name_bound_twice_is_unknown() {
        // One flattened scope: two different types for one name is `""`; the
        // same type twice stays typed; a local fun named like a class is no
        // constructor.
        let fp = parse(
            r#"
fun f(r: Repo) {
    if (x) {
        val r = Index()
    }
    val s = Repo()
    run { val s = Repo() }
    fun Make(): Repo = Repo()
    val t = Make()
}
"#,
        );
        assert_eq!(
            locals_of(&fp, id(node_kind::FUNCTION, "svc::f")),
            pairs(&[("r", ""), ("s", "Repo"), ("t", "")])
        );
    }

    #[test]
    fn local_types_marker_counts_off_the_parses() {
        let fp = parse(
            r#"
class S(private val repo: Repo) {
    fun a(x: Repo, n: Int) {
        val r = repo
        val q = Queue()
    }

    fun b() {}
}
"#,
        );
        assert_eq!(
            super::local_types_marker(&[fp], "r"),
            "[kotlin] local types: scopes=1 typed=3 unknown=1 repo=r"
        );
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
