//! The TypeScript / JavaScript ethers client reader (DT.10c, DT.10i).
//!
//! 1. **Gate.** The file imports or requires `ethers` (`ethers/..` and an
//!    `@ethersproject/` path too) or `hardhat` (whose `ethers` is ethers);
//!    otherwise nothing is read, so a local `class Contract` mints nothing.
//! 2. **ABI bindings.** `(export )?(const|let|var) <N>(: T)? = <abi literal>`
//!    at any depth, read by `abi::read_abi`. A binding whose literal does not
//!    read is no ABI. An ABI binding is visible file-wide (the nearest one
//!    before a use, else the first).
//! 3. **Contract bindings.** A construction `(await )?new
//!    (ethers.)?(Contract|BaseContract)(<addr>, <abi>, ..)`, optionally
//!    chained through `.connect(..)` / `.attach(..)`, bound by
//!    - `(const|let|var) <x>(: T)? = ..` and a bare `<x> = ..`, key `<x>`;
//!    - a class field `(private|public|protected|readonly|static|..
//!      )*<x>(?|!)?(: T)? = ..` (or `#<x> = ..`), key `this.<x>` (`this.#<x>`);
//!    - `this.<x> = ..` / `this.#<x> = ..` in a constructor or method, key
//!      `this.<x>`.
//!
//!    The ABI argument (an identifier may carry a TS `!` or `as T`): an
//!    identifier with an ABI binding is `Scope::Abi(<identifier>)` with its
//!    entries; any other identifier (an import) `Scope::Abi(<identifier>)`
//!    with no entries; an inline literal that reads `Scope::Abi(<x>)` with its
//!    entries; anything else (a call, a member path) binds nothing and counts
//!    `dynamic`. A rebinding `<y> = R.connect(..)` / `R.attach(..)` (any of
//!    the binding forms above, on a bound receiver `R`) binds `y` to R's
//!    scope and entries.
//! 4. **Scope.** A `const` / `let` binding is visible from the end of its
//!    initializer to the end of its innermost enclosing block; a `var`
//!    binding file-wide. A bare assignment binds in the block of the nearest
//!    preceding `let x` declaration that holds it, file-wide after a `var x`
//!    or with none; one in a parameter list or after a type annotation (`x: T
//!    = new ..` outside a class body, a default parameter) binds nothing. A
//!    `this.` binding is visible in its class body only, before its
//!    assignment too (methods run in any order), and never as a bare `<x>`:
//!    a class field is no variable in scope. A site reads the NEAREST binding
//!    of its receiver that precedes it and whose scope holds it (for a
//!    `this.` key with none before it, the first one in the class), so a
//!    later function's parameter `escrow` never reads an earlier function's
//!    contract.
//! 5. **Call sites** on a bound receiver `R` (a bare `R` is never a member,
//!    so `this.escrow` is not `escrow`), after any `.connect(..)` /
//!    `.attach(..)` links:
//!    - `R.<m>(` (and `R?.<m>(`, `R!.<m>(`), `R['<m>'](` / `R["<m>"](`; a
//!      signature key `'m(uint256,address)'` is member `m` whose `args` is
//!      the arity it spells;
//!    - ethers v6 `R.<m>.(staticCall|send|estimateGas|populateTransaction|
//!      staticCallResult)(` and v5 `R.(callStatic|estimateGas|
//!      populateTransaction|functions).<m>(`;
//!    - a chained `new (ethers.)?Contract(<addr>, <abi identifier>, ..)<links>`
//!      followed by any of those, with no binding.
//!
//!    [`ETHERS_MEMBERS`] are never sites and are not counted (the stop list
//!    applies to the member after the chain: `R.connect(s).getAddress()` is
//!    none). With ABI entries known, a member that is no function of them
//!    mints nothing and counts `not_in_abi`. `args` is the depth-0 argument
//!    count of the final call; `lib` is `ethers`. A non-literal element key
//!    `R[name](` is no site and counts `dynamic`. A chain on an unbound
//!    receiver is not read.
//! 6. **Event sites** on `R`: `R.(on|once|addListener|queryFilter)('<E>'` (a
//!    signature key `'E(uint256)'` reads as `E`) and every `R.filters.<E>(`
//!    (so `R.on(R.filters.E(), ..)` and `R.queryFilter(R.filters.E(), ..)`
//!    are one site each, at the filter). With entries known, only an event
//!    of them.

use std::collections::BTreeSet;

use glia_code_domain::evm::{AbiEntry, ClientSide, EntryKind, Scope};

use super::Site;
use super::abi::read_abi;
use super::scan::{
    enclosing_block, ident_end, ident_start_before, is_ident, is_ident_byte, matching, prev_non_ws,
    skip_ws, split_args, string_literal, trim, word_hits,
};

/// The client library every site of this reader names.
const LIB: &str = "ethers";

/// ethers `BaseContract` members and runtime methods: never an ABI call site,
/// skipped before the ABI check and never counted.
const ETHERS_MEMBERS: &[&str] = &[
    "connect",
    "attach",
    "getAddress",
    "getDeployedCode",
    "waitForDeployment",
    "deploymentTransaction",
    "deployTransaction",
    "deployed",
    "getFunction",
    "getEvent",
    "interface",
    "target",
    "runner",
    "address",
    "resolvedAddress",
    "signer",
    "provider",
    "filters",
    "on",
    "once",
    "off",
    "emit",
    "addListener",
    "removeListener",
    "removeAllListeners",
    "listeners",
    "listenerCount",
    "queryFilter",
    "fallback",
    "callStatic",
    "functions",
    "estimateGas",
    "populateTransaction",
    "then",
    "toString",
];

/// The members whose first argument names the event they read (step 6).
const EVENT_MEMBERS: &[&str] = &["on", "once", "addListener", "queryFilter"];

/// Chain links that keep the contract and its ABI (steps 3 and 5).
const LINKS: &[&str] = &["connect", "attach"];

/// ethers v5 namespaces: `R.<ns>.<m>(` calls or previews ABI function `m`.
const V5_NAMESPACES: &[&str] = &[
    "callStatic",
    "estimateGas",
    "populateTransaction",
    "functions",
];

/// ethers v6 method members: `R.<m>.<s>(` calls or previews ABI function `m`.
const V6_METHODS: &[&str] = &[
    "staticCall",
    "send",
    "estimateGas",
    "populateTransaction",
    "staticCallResult",
];

/// Field modifiers: a class member may lead with any number of them.
const FIELD_MODIFIERS: &[&str] = &[
    "private",
    "public",
    "protected",
    "readonly",
    "static",
    "declare",
    "override",
    "accessor",
];

/// Most bytes a declaration's type annotation is read over.
const MAX_TYPE: usize = 400;

/// What [`scan`] counted besides its sites.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct TsCounts {
    /// ABI bindings read (step 2, plus inline literals of step 3).
    pub(super) abis: usize,
    /// Members and events left out because the known ABI does not list them.
    pub(super) not_in_abi: usize,
    /// Contract constructions whose ABI argument is neither an identifier nor
    /// a literal that reads, and calls through a non-literal element key.
    pub(super) dynamic: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kw {
    Const,
    Let,
    Var,
}

/// One `const` / `let` / `var` declarator.
#[derive(Debug)]
struct Decl {
    kw: Kw,
    kw_at: usize,
    name: String,
    /// The first byte of the initializer after `=`; `None` without one.
    init: Option<usize>,
}

/// One ABI binding (step 2).
#[derive(Debug)]
struct AbiBinding {
    name: String,
    at: usize,
    entries: Vec<AbiEntry>,
}

/// One contract binding (step 3) and the range it is visible in (step 4).
#[derive(Debug)]
struct Binding {
    /// The receiver key: `<x>`, `this.<x>` or `this.#<x>`.
    name: String,
    scope: Scope,
    abi: Option<Vec<AbiEntry>>,
    /// The end of its initializer: a site must come after it, unless
    /// `hoisted`.
    at: usize,
    block: (usize, usize),
    /// A `this.` binding, visible in its whole class body (step 4).
    hoisted: bool,
}

/// What a construction or rebinding binds (step 3).
struct Target {
    key: String,
    block: (usize, usize),
    hoisted: bool,
}

/// The ABI argument of a construction.
enum AbiArg {
    Ident(String),
    Literal(Vec<AbiEntry>),
    Dynamic,
}

/// One member access after a receiver.
enum Step {
    /// `.m`, `?.m`, `!.m`, `['m']` or `["m(uint256)"]`: the member's offset
    /// and name, the arity a signature key spells, the offset just past it
    /// and whether it was dotted.
    Name {
        at: usize,
        name: String,
        arity: Option<u32>,
        end: usize,
        dotted: bool,
    },
    /// `[<key>]` naming no member: `literal` when the key is a string that
    /// is no member name, else a computed key; `end` is just past the `]`.
    Keyed { literal: bool, end: usize },
}

/// What follows a receiver (steps 5 and 6).
enum Access {
    /// A call of ABI function `member` whose name is at `at`.
    Call {
        member: String,
        at: usize,
        args: Option<u32>,
    },
    /// A subscription, query or filter of event `event`.
    Event { event: String, at: usize },
    /// `R[<expr>](`: a call through a non-literal key.
    Dynamic,
    /// A member access that is no site: a stop-listed member, a property, an
    /// event member without a literal name.
    Other,
}

/// The sites of one TS-family file (module doc) over its code `view`.
pub(super) fn scan(source: &str, view: &str) -> (Vec<Site>, TsCounts) {
    let mut counts = TsCounts::default();
    let mut sites = Vec::new();
    if !gated(source, view) {
        return (sites, counts);
    }
    let vb = view.as_bytes();
    let decls = declarations(view);
    let abis: Vec<AbiBinding> = decls
        .iter()
        .filter_map(|d| {
            let init = d.init?;
            let entries = read_abi(source, view, init)?;
            Some(AbiBinding {
                name: d.name.clone(),
                at: d.kw_at,
                entries,
            })
        })
        .collect();
    counts.abis = abis.len();

    let mut bindings: Vec<Binding> = Vec::new();
    for new_at in word_hits(view, "new") {
        if new_at > 0 && vb[new_at - 1] == b'.' {
            continue;
        }
        let Some((open, close)) = construction(view, new_at) else {
            continue;
        };
        let arg = abi_arg(source, view, &split_args(view, open, close));
        // A chained call on the construction (step 5).
        if let Some(access) = access(source, view, close + 1) {
            if matches!(access, Access::Other) {
                continue;
            }
            let AbiArg::Ident(name) = arg else {
                counts.dynamic += 1;
                continue;
            };
            let abi = abi_lookup(&abis, &name, new_at);
            let chained = Binding {
                name: String::new(),
                scope: Scope::Abi(name),
                abi,
                at: close,
                block: (0, vb.len()),
                hoisted: false,
            };
            record(&mut sites, &mut counts, &chained, access);
            continue;
        }
        let Some(target) = binding_target(view, &decls, new_at) else {
            continue;
        };
        let (scope, abi) = match arg {
            AbiArg::Ident(abi_name) => {
                let abi = abi_lookup(&abis, &abi_name, new_at);
                (Scope::Abi(abi_name), abi)
            }
            AbiArg::Literal(entries) => {
                counts.abis += 1;
                let base = target.key.rsplit(['.', '#']).next().unwrap_or_default();
                (Scope::Abi(base.to_string()), Some(entries))
            }
            AbiArg::Dynamic => {
                counts.dynamic += 1;
                continue;
            }
        };
        bindings.push(Binding {
            name: target.key,
            scope,
            abi,
            at: close,
            block: target.block,
            hoisted: target.hoisted,
        });
    }
    rebind(source, view, &decls, &mut bindings);

    for receiver in keys(&bindings) {
        for (start, end) in receiver_hits(view, &receiver) {
            let Some(access) = access(source, view, end) else {
                continue;
            };
            let Some(binding) = visible(&bindings, &receiver, start) else {
                continue;
            };
            record(&mut sites, &mut counts, binding, access);
        }
    }
    sites.sort_by(|a, b| (a.at, &a.member).cmp(&(b.at, &b.member)));
    (sites, counts)
}

/// Step 1: the file imports or requires an ethers or hardhat module.
fn gated(source: &str, view: &str) -> bool {
    if !source.contains("ethers") && !source.contains("hardhat") {
        return false;
    }
    let vb = view.as_bytes();
    for kw in ["from", "require", "import"] {
        for at in word_hits(view, kw) {
            if at > 0 && vb[at - 1] == b'.' {
                continue;
            }
            let mut k = skip_ws(vb, at + kw.len());
            if vb.get(k) == Some(&b'(') {
                k = skip_ws(vb, k + 1);
            } else if kw == "require" {
                continue;
            }
            if let Some((spec, _)) = string_literal(source, k)
                && is_client_module(&spec)
            {
                return true;
            }
        }
    }
    false
}

/// An import specifier that brings ethers in.
fn is_client_module(spec: &str) -> bool {
    spec == "ethers"
        || spec == "hardhat"
        || spec.starts_with("ethers/")
        || spec.starts_with("@ethersproject/")
}

/// Every `const` / `let` / `var` declarator with a plain identifier name.
fn declarations(view: &str) -> Vec<Decl> {
    let vb = view.as_bytes();
    let mut out = Vec::new();
    for (word, kw) in [("const", Kw::Const), ("let", Kw::Let), ("var", Kw::Var)] {
        for kw_at in word_hits(view, word) {
            if kw_at > 0 && vb[kw_at - 1] == b'.' {
                continue;
            }
            let n_at = skip_ws(vb, kw_at + word.len());
            let Some(n_end) = ident_end(vb, n_at) else {
                continue;
            };
            let mut k = skip_ws(vb, n_end);
            if vb.get(k) == Some(&b':') {
                match type_end(vb, k + 1) {
                    Some(eq) => k = eq,
                    None => {
                        out.push(Decl {
                            kw,
                            kw_at,
                            name: view[n_at..n_end].to_string(),
                            init: None,
                        });
                        continue;
                    }
                }
            }
            let init = (vb.get(k) == Some(&b'=') && !matches!(vb.get(k + 1), Some(b'=' | b'>')))
                .then(|| skip_ws(vb, k + 1));
            out.push(Decl {
                kw,
                kw_at,
                name: view[n_at..n_end].to_string(),
                init,
            });
        }
    }
    out.sort_by_key(|d| d.kw_at);
    out
}

/// The offset of the `=` ending the type annotation that starts at `from`;
/// `None` when the declarator has no initializer.
fn type_end(vb: &[u8], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut k = from;
    while k < vb.len() && k < from + MAX_TYPE {
        match vb[k] {
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            b'>' => depth = depth.saturating_sub(1),
            b'=' if vb.get(k + 1) == Some(&b'>') => k += 1,
            b'=' if depth == 0 && vb.get(k + 1) != Some(&b'=') => return Some(k),
            b';' | b',' if depth == 0 => return None,
            _ => {}
        }
        k += 1;
    }
    None
}

/// `new (ethers.)?(Contract|BaseContract)(` at `new_at`: the offsets of its
/// argument parentheses.
fn construction(view: &str, new_at: usize) -> Option<(usize, usize)> {
    let vb = view.as_bytes();
    let mut k = skip_ws(vb, new_at + "new".len());
    let mut end = ident_end(vb, k)?;
    if &view[k..end] == "ethers" {
        let dot = skip_ws(vb, end);
        if vb.get(dot) != Some(&b'.') {
            return None;
        }
        k = skip_ws(vb, dot + 1);
        end = ident_end(vb, k)?;
    }
    if !matches!(&view[k..end], "Contract" | "BaseContract") {
        return None;
    }
    let open = skip_ws(vb, end);
    if vb.get(open) != Some(&b'(') {
        return None;
    }
    Some((open, matching(view, open)?))
}

/// Classify a construction's second argument (step 3).
fn abi_arg(source: &str, view: &str, args: &[(usize, usize)]) -> AbiArg {
    let Some(&(s, e)) = args.get(1) else {
        return AbiArg::Dynamic;
    };
    let vb = view.as_bytes();
    if vb[s] == b'[' {
        return read_abi(source, view, s).map_or(AbiArg::Dynamic, AbiArg::Literal);
    }
    let Some(id_end) = ident_end(vb, s) else {
        return AbiArg::Dynamic;
    };
    // A TS non-null `!` or an `as T` cast keeps the identifier.
    let rest = view[id_end..e].trim_start();
    let rest = rest.strip_prefix('!').unwrap_or(rest).trim_start();
    let cast = rest
        .strip_prefix("as")
        .is_some_and(|t| t.starts_with(char::is_whitespace));
    if rest.is_empty() || cast {
        AbiArg::Ident(view[s..id_end].to_string())
    } else {
        AbiArg::Dynamic
    }
}

/// The entries of ABI binding `name` as a use at `at` sees it: the nearest
/// binding before `at`, else the first; `None` when the file binds no ABI of
/// that name.
fn abi_lookup(abis: &[AbiBinding], name: &str, at: usize) -> Option<Vec<AbiEntry>> {
    abis.iter()
        .filter(|a| a.name == name && a.at < at)
        .max_by_key(|a| a.at)
        .or_else(|| abis.iter().find(|a| a.name == name))
        .map(|a| a.entries.clone())
}

/// The member access starting at `from` (after a receiver or a previous
/// access); `None` when none starts there.
fn step(source: &str, view: &str, from: usize) -> Option<Step> {
    let vb = view.as_bytes();
    let mut k = skip_ws(vb, from);
    if vb.get(k) == Some(&b'!') && vb.get(k + 1) != Some(&b'=') {
        k = skip_ws(vb, k + 1);
    }
    if vb.get(k) == Some(&b'?') && vb.get(k + 1) == Some(&b'.') {
        k += 1;
        let after = skip_ws(vb, k + 1);
        if vb.get(after) == Some(&b'[') {
            k = after;
        }
    }
    match vb.get(k)? {
        b'.' => {
            let at = skip_ws(vb, k + 1);
            let end = ident_end(vb, at)?;
            Some(Step::Name {
                at,
                name: view[at..end].to_string(),
                arity: None,
                end,
                dotted: true,
            })
        }
        b'[' => {
            let close = matching(view, k)?;
            let (s, e) = trim(vb, k + 1, close);
            let end = close + 1;
            let Some((text, _)) = string_literal(source, s).filter(|(_, le)| *le == e) else {
                return Some(Step::Keyed {
                    literal: false,
                    end,
                });
            };
            Some(match signature_key(&text) {
                Some((name, arity)) => Step::Name {
                    at: s,
                    name,
                    arity,
                    end,
                    dotted: false,
                },
                None => Step::Keyed { literal: true, end },
            })
        }
        _ => None,
    }
}

/// A member name or a signature key `m(uint256,(address,bytes))`: the name
/// and, for a signature, the arity it spells (depth-0 parameters).
fn signature_key(text: &str) -> Option<(String, Option<u32>)> {
    let text = text.trim();
    let Some(open) = text.find('(') else {
        return is_ident(text).then(|| (text.to_string(), None));
    };
    let name = text[..open].trim();
    let inner = text[open + 1..].strip_suffix(')')?;
    if !is_ident(name) {
        return None;
    }
    let mut depth = 0usize;
    let mut arity = 0u32;
    let mut piece = false;
    for c in inner.chars() {
        match c {
            '(' | '[' => {
                depth += 1;
                piece = true;
            }
            ')' | ']' => depth = depth.checked_sub(1)?,
            ',' if depth == 0 => {
                if !piece {
                    return None;
                }
                arity += 1;
                piece = false;
            }
            c if !c.is_whitespace() => piece = true,
            _ => {}
        }
    }
    if depth != 0 || (!piece && arity > 0) {
        return None;
    }
    Some((name.to_string(), Some(arity + u32::from(piece))))
}

/// The `(` right after `end`, if one is there.
fn paren_after(vb: &[u8], end: usize) -> Option<usize> {
    let p = skip_ws(vb, end);
    (vb.get(p) == Some(&b'(')).then_some(p)
}

/// The offset just past any `.connect(..)` / `.attach(..)` links starting at
/// `from`; `from` itself when there are none.
fn skip_links(source: &str, view: &str, from: usize) -> usize {
    let vb = view.as_bytes();
    let mut k = from;
    while let Some(Step::Name {
        name,
        end,
        dotted: true,
        ..
    }) = step(source, view, k)
        && LINKS.contains(&name.as_str())
        && let Some(paren) = paren_after(vb, end)
        && let Some(close) = matching(view, paren)
    {
        k = close + 1;
    }
    k
}

/// What follows a receiver ending at `from`, past its links (steps 5 and 6);
/// `None` when no member access follows.
fn access(source: &str, view: &str, from: usize) -> Option<Access> {
    let vb = view.as_bytes();
    let k = skip_links(source, view, from);
    let (at, name, arity, end, dotted) = match step(source, view, k)? {
        Step::Name {
            at,
            name,
            arity,
            end,
            dotted,
        } => (at, name, arity, end, dotted),
        Step::Keyed { literal, end } => {
            let called = !literal && paren_after(vb, end).is_some();
            return Some(if called {
                Access::Dynamic
            } else {
                Access::Other
            });
        }
    };
    let next = step(source, view, end);
    if dotted && name == "filters" {
        return Some(match next {
            Some(Step::Name { at, name, end, .. }) if paren_after(vb, end).is_some() => {
                Access::Event { event: name, at }
            }
            _ => Access::Other,
        });
    }
    if dotted && EVENT_MEMBERS.contains(&name.as_str()) {
        let event = paren_after(vb, end).and_then(|paren| literal_event(source, vb, paren));
        return Some(event.map_or(Access::Other, |event| Access::Event { event, at }));
    }
    if dotted && V5_NAMESPACES.contains(&name.as_str()) {
        match &next {
            Some(Step::Name {
                at,
                name,
                arity,
                end,
                ..
            }) => {
                if let Some(paren) = paren_after(vb, *end) {
                    return Some(call(view, *at, name.clone(), *arity, paren));
                }
            }
            Some(Step::Keyed {
                literal: false,
                end,
            }) if paren_after(vb, *end).is_some() => return Some(Access::Dynamic),
            _ => {}
        }
    }
    if let Some(paren) = paren_after(vb, end) {
        return Some(call(view, at, name, arity, paren));
    }
    if let Some(Step::Name {
        name: method,
        end: m_end,
        dotted: true,
        ..
    }) = next
        && V6_METHODS.contains(&method.as_str())
        && let Some(paren) = paren_after(vb, m_end)
    {
        return Some(call(view, at, name, arity, paren));
    }
    Some(Access::Other)
}

/// A call of `member` (named at `at`) whose final `(` is at `paren`: a site
/// unless the member is stop-listed. `args` is a signature key's spelled
/// arity, else the final call's depth-0 argument count.
fn call(view: &str, at: usize, member: String, arity: Option<u32>, paren: usize) -> Access {
    if ETHERS_MEMBERS.contains(&member.as_str()) {
        return Access::Other;
    }
    let args = arity.or_else(|| {
        matching(view, paren)
            .map(|close| split_args(view, paren, close).len())
            .and_then(|n| u32::try_from(n).ok())
    });
    Access::Call { member, at, args }
}

/// The event a string first argument after `paren` names (`'E'` or
/// `'E(uint256)'`); `None` for any other first argument.
fn literal_event(source: &str, vb: &[u8], paren: usize) -> Option<String> {
    let q = skip_ws(vb, paren + 1);
    let (text, _) = string_literal(source, q)?;
    signature_key(&text).map(|(name, _)| name)
}

/// The variable a construction or rebinding whose initializer starts at
/// `start` binds, and the range it is visible in (steps 3 and 4); `None`
/// when it binds none.
fn binding_target(view: &str, decls: &[Decl], start: usize) -> Option<Target> {
    let vb = view.as_bytes();
    let mut init = start;
    let mut eq = prev_non_ws(vb, start)?;
    if let Some(s) = ident_start_before(vb, eq + 1)
        && &view[s..=eq] == "await"
    {
        init = s;
        eq = prev_non_ws(vb, s)?;
    }
    if let Some(d) = decls.iter().find(|d| d.init == Some(init)) {
        let block = match d.kw {
            Kw::Var => (0, vb.len()),
            Kw::Const | Kw::Let => enclosing_block(view, d.kw_at),
        };
        return Some(Target {
            key: d.name.clone(),
            block,
            hoisted: false,
        });
    }
    if vb[eq] != b'=' || eq == 0 || b"=!<>+-*/%&|^?".contains(&vb[eq - 1]) {
        return None;
    }
    // A class field: the statement right in a class body.
    let (open, close) = enclosing_block(view, eq);
    if close < vb.len() && is_class_body(view, open) {
        let name = class_field(view, open, eq)?;
        return Some(Target {
            key: format!("this.{name}"),
            block: (open, close),
            hoisted: true,
        });
    }
    let x_end = prev_non_ws(vb, eq)? + 1;
    let x_start = ident_start_before(vb, x_end)?;
    let name = &view[x_start..x_end];
    if let Some(before) = prev_non_ws(vb, x_start) {
        if matches!(vb[before], b'.' | b'#') {
            return this_target(view, before, x_start, name);
        }
        if matches!(vb[before], b'(' | b',' | b':') {
            return None;
        }
        if let Some(w) = ident_start_before(vb, before + 1)
            && FIELD_MODIFIERS.contains(&&view[w..=before])
        {
            return None;
        }
    }
    let block = decls
        .iter()
        .filter(|d| d.name == name && d.kw_at < x_start && d.kw != Kw::Const)
        .filter_map(|d| match d.kw {
            Kw::Var => Some((d.kw_at, (0, vb.len()))),
            _ => {
                let b = enclosing_block(view, d.kw_at);
                (b.0 <= x_start && x_start <= b.1).then_some((d.kw_at, b))
            }
        })
        .max_by_key(|&(at, _)| at)
        .map_or((0, vb.len()), |(_, b)| b);
    Some(Target {
        key: name.to_string(),
        block,
        hoisted: false,
    })
}

/// `this.<name> = ..` / `this.#<name> = ..` in a constructor or method, the
/// `.` or `#` before the name at `before`: key `this.<name>` scoped to the
/// enclosing class body; `None` for any other object or outside a class.
fn this_target(view: &str, before: usize, x_start: usize, name: &str) -> Option<Target> {
    let vb = view.as_bytes();
    let hash = vb[before] == b'#';
    let dot = if hash { before.checked_sub(1)? } else { before };
    if vb[dot] != b'.' {
        return None;
    }
    let this_end = prev_non_ws(vb, dot)? + 1;
    let this_start = ident_start_before(vb, this_end)?;
    if &view[this_start..this_end] != "this"
        || prev_non_ws(vb, this_start).is_some_and(|p| matches!(vb[p], b'.' | b'#'))
    {
        return None;
    }
    let block = class_body(view, x_start)?;
    Some(Target {
        key: format!("this.{}{name}", if hash { "#" } else { "" }),
        block,
        hoisted: true,
    })
}

/// The field a class-body statement ending in the `=` at `eq` (the class
/// body opening at `open`) declares: `(<modifier> )*(#)?<x>(?|!)?(: T)?`, as
/// `x` or `#x`; `None` for any other statement. Inside a parameter list
/// (a constructor's, a method's, an arrow field's) only a parameter property,
/// one with a modifier, is a field: a plain default parameter binds nothing.
fn class_field(view: &str, open: usize, eq: usize) -> Option<String> {
    let vb = view.as_bytes();
    let head = &vb[open + 1..eq];
    let depth = head.iter().fold(0usize, |d, &c| match c {
        b'(' | b'[' => d + 1,
        b')' | b']' => d.saturating_sub(1),
        _ => d,
    });
    let in_params = depth > 0;
    let start = head
        .iter()
        .rposition(|&c| {
            matches!(c, b';' | b'{' | b'}' | b'\n') || (in_params && matches!(c, b'(' | b','))
        })
        .map_or(open + 1, |p| open + 1 + p + 1);
    let mut k = skip_ws(vb, start);
    let mut modified = false;
    loop {
        let hash = vb.get(k) == Some(&b'#');
        let n_at = k + usize::from(hash);
        let n_end = ident_end(vb, n_at)?;
        let word = &view[n_at..n_end];
        let mut j = skip_ws(vb, n_end);
        let more = vb.get(j) == Some(&b'#') || ident_end(vb, j).is_some();
        if !hash && FIELD_MODIFIERS.contains(&word) && more {
            modified = true;
            k = j;
            continue;
        }
        if matches!(vb.get(j), Some(b'?' | b'!')) {
            j = skip_ws(vb, j + 1);
        }
        if vb.get(j) == Some(&b':') {
            j = type_end(vb, j + 1)?;
        }
        let field = j == eq && (modified || !in_params);
        return field.then(|| format!("{}{word}", if hash { "#" } else { "" }));
    }
}

/// True when the `{` at `open` opens a class body: its statement header
/// holds the word `class`.
fn is_class_body(view: &str, open: usize) -> bool {
    let vb = view.as_bytes();
    let start = vb[..open]
        .iter()
        .rposition(|&c| matches!(c, b';' | b'{' | b'}'))
        .map_or(0, |p| p + 1);
    view[start..open]
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .any(|w| w == "class")
}

/// The innermost class body holding `at`; `None` outside every class.
fn class_body(view: &str, at: usize) -> Option<(usize, usize)> {
    let mut pos = at;
    loop {
        let (open, close) = enclosing_block(view, pos);
        if close >= view.len() {
            return None;
        }
        if is_class_body(view, open) {
            return Some((open, close));
        }
        if open == 0 {
            return None;
        }
        pos = open;
    }
}

/// Rebindings `<y> = R.connect(..)` / `R.attach(..)` on a bound receiver
/// (step 3), followed until no new one binds (`y` may itself be rebound).
fn rebind(source: &str, view: &str, decls: &[Decl], bindings: &mut Vec<Binding>) {
    let mut rebound: BTreeSet<usize> = BTreeSet::new();
    loop {
        let mut added = Vec::new();
        for receiver in keys(bindings) {
            for (start, end) in receiver_hits(view, &receiver) {
                if rebound.contains(&start) {
                    continue;
                }
                let tail = skip_links(source, view, end);
                if tail == end || step(source, view, tail).is_some() {
                    continue;
                }
                let Some(from) = visible(bindings, &receiver, start) else {
                    continue;
                };
                let Some(target) = binding_target(view, decls, start) else {
                    continue;
                };
                rebound.insert(start);
                added.push(Binding {
                    name: target.key,
                    scope: from.scope.clone(),
                    abi: from.abi.clone(),
                    at: tail - 1,
                    block: target.block,
                    hoisted: target.hoisted,
                });
            }
        }
        if added.is_empty() {
            return;
        }
        bindings.extend(added);
    }
}

/// The distinct receiver keys of `bindings`, sorted.
fn keys(bindings: &[Binding]) -> Vec<String> {
    let set: BTreeSet<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
    set.into_iter().map(str::to_string).collect()
}

/// Every place receiver `key` stands in the view, as its `(start, end)`: a
/// bare `<x>` not itself a member (so `this.escrow` is not `escrow`), or
/// `this.<x>` / `this.#<x>`.
fn receiver_hits(view: &str, key: &str) -> Vec<(usize, usize)> {
    let vb = view.as_bytes();
    let ident = key.strip_prefix("this.").unwrap_or(key);
    let ident = ident.strip_prefix('#').unwrap_or(ident);
    let lead = &key.as_bytes()[..key.len() - ident.len()];
    word_hits(view, ident)
        .into_iter()
        .filter_map(|at| {
            let end = at + ident.len();
            let s = at.checked_sub(lead.len())?;
            if vb.get(s..at) != Some(lead) {
                return None;
            }
            let free = s == 0 || !(is_ident_byte(vb[s - 1]) || matches!(vb[s - 1], b'.' | b'#'));
            free.then_some((s, end))
        })
        .collect()
}

/// The binding of `receiver` a site at `at` reads (step 4).
fn visible<'a>(bindings: &'a [Binding], receiver: &str, at: usize) -> Option<&'a Binding> {
    let held = || {
        bindings
            .iter()
            .filter(move |b| b.name == receiver && b.block.0 <= at && at <= b.block.1)
    };
    held()
        .filter(|b| b.at < at)
        .max_by_key(|b| b.at)
        .or_else(|| held().filter(|b| b.hoisted).min_by_key(|b| b.at))
}

/// The site (or count) one access on `binding` gives.
fn record(sites: &mut Vec<Site>, counts: &mut TsCounts, binding: &Binding, access: Access) {
    match access {
        Access::Call { member, at, args } => push_call(sites, counts, binding, member, args, at),
        Access::Event { event, at } => push_event(sites, counts, binding, event, at),
        Access::Dynamic => counts.dynamic += 1,
        Access::Other => {}
    }
}

/// A call site of `member` on `binding`, named at `at` (step 5).
fn push_call(
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    binding: &Binding,
    member: String,
    args: Option<u32>,
    at: usize,
) {
    if let Some(entries) = &binding.abi
        && !entries
            .iter()
            .any(|e| e.kind == EntryKind::Fn && e.name == member)
    {
        counts.not_in_abi += 1;
        return;
    }
    sites.push(Site {
        side: ClientSide::Call,
        scope: binding.scope.clone(),
        member,
        args,
        lib: LIB,
        abi: binding.abi.clone(),
        at,
    });
}

/// An event site of `event` on `binding`, read at `at` (step 6).
fn push_event(
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    binding: &Binding,
    event: String,
    at: usize,
) {
    if let Some(entries) = &binding.abi
        && !entries
            .iter()
            .any(|en| en.kind == EntryKind::Event && en.name == event)
    {
        counts.not_in_abi += 1;
        return;
    }
    sites.push(Site {
        side: ClientSide::Event,
        scope: binding.scope.clone(),
        member: event,
        args: None,
        lib: LIB,
        abi: binding.abi.clone(),
        at,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm::scan::code_view;

    /// `(side, qname-ish key, args, abi keys)` per site, in file order.
    fn read(src: &str) -> (Vec<String>, TsCounts) {
        let (sites, counts) = scan(src, &code_view(src, "typescript"));
        let rows = sites
            .iter()
            .map(|s| {
                let side = match s.side {
                    ClientSide::Call => "call",
                    ClientSide::Event => "event",
                };
                let abi = s.abi.as_ref().map_or_else(
                    || "-".to_string(),
                    |e| e.iter().map(AbiEntry::to_key).collect::<Vec<_>>().join(","),
                );
                let args = s.args.map_or_else(|| "-".to_string(), |a| a.to_string());
                format!(
                    "{side} {}.{} args={args} abi={abi} line={}",
                    s.scope.key(),
                    s.member,
                    line(src, s.at)
                )
            })
            .collect();
        (rows, counts)
    }

    fn line(src: &str, at: usize) -> usize {
        src[..at].matches('\n').count() + 1
    }

    const ABI: &str = "const ESCROW_ABI = [
  'function deposit(bytes32 orderId) payable',
  'function release(bytes32 orderId)',
  'event Released(bytes32 indexed orderId, address seller)',
];
";

    #[test]
    fn new_contract_with_same_file_abi() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
export async function pay(addr: string, signer: any, id: string) {{
  const escrow: Contract = await new Contract(addr, ESCROW_ABI, signer);
  return escrow.deposit(id, {{ value: 1n }});
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            ["call abi:ESCROW_ABI.deposit args=2 abi=f:deposit/1,f:release/1,e:Released/2 line=9"]
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 0,
                dynamic: 0
            }
        );
    }

    #[test]
    fn stop_listed_and_not_in_abi_members_mint_nothing() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
function f(addr: string, p: any) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  escrow.getAddress();
  escrow.connect(p).release(addr);
  escrow.waitForDeployment();
  escrow.interface.parseLog(x);
  escrow.withdraw(addr);
  const tx = escrow.release(addr);
  tx.wait();
}}
"
        );
        let (rows, counts) = read(&src);
        // DT.10i: a `.connect(p)` link keeps the contract, so line 10 is a
        // site; the stop list reads the member after the chain.
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=10",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=14"
            ]
        );
        assert_eq!(counts.not_in_abi, 1, "withdraw only");
    }

    #[test]
    fn imported_abi_has_no_entries() {
        let src = "import { Contract } from 'ethers';
import { ESCROW_ABI } from './abi';
const escrow = new Contract(addr, ESCROW_ABI as any, p);
escrow.anything(1, 2);
const dyn = new Contract(addr, loadAbi(), p);
dyn.deposit(1);
const other = new Contract(addr, artifacts.Escrow.abi, p);
";
        let (rows, counts) = read(src);
        assert_eq!(rows, ["call abi:ESCROW_ABI.anything args=2 abi=- line=4"]);
        assert_eq!(
            counts,
            TsCounts {
                abis: 0,
                not_in_abi: 0,
                dynamic: 2
            }
        );
    }

    #[test]
    fn nearest_binding_wins() {
        let src = "const { ethers } = require('ethers');
const A_ABI = ['function a()'];
const B_ABI = ['function b()'];
async function main() {
  let c = new ethers.Contract(x, A_ABI, p);
  c.a();
  c = new ethers.Contract(x, B_ABI, p);
  c.b();
  c.a();
}
";
        let (rows, counts) = read(src);
        assert_eq!(
            rows,
            [
                "call abi:A_ABI.a args=0 abi=f:a/0 line=6",
                "call abi:B_ABI.b args=0 abi=f:b/0 line=8"
            ]
        );
        assert_eq!(counts.not_in_abi, 1, "the second c.a() reads B_ABI");
    }

    #[test]
    fn binding_does_not_leak_out_of_its_block() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
export function releaseNow(address: string, signer: any, id: string) {{
  if (id) {{
    const escrow = new Contract(address, ESCROW_ABI, signer);
    escrow.release(id);
    this.escrow.release(id);
    this.#escrow.release(id);
  }}
  escrow.deposit(id);
}}

export function noteRelease(escrow: {{ release(id: string): string }}, id: string) {{
  return escrow.release(id);
}}
"
        );
        let (rows, _) = read(&src);
        assert_eq!(
            rows,
            ["call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=10"]
        );
    }

    #[test]
    fn bare_assignment_binds_in_its_declaration_block() {
        let src = format!(
            "const {{ ethers }} = require('hardhat');\n{ABI}\
describe('Escrow', () => {{
  let escrow;
  beforeEach(async () => {{
    escrow = new ethers.Contract(addr, ESCROW_ABI, signer);
  }});
  it('deposits', async () => {{
    await escrow.deposit(id);
  }});
}});
describe('Other', () => {{
  it('releases', async () => {{
    await escrow.release(id);
  }});
}});
class Holder {{
  escrow = new ethers.Contract(addr, ESCROW_ABI, signer);
  private vault = new ethers.Contract(addr, ESCROW_ABI, signer);
  go() {{ return vault.release(id); }}
}}
"
        );
        let (rows, _) = read(&src);
        assert_eq!(
            rows,
            ["call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=13"]
        );
    }

    #[test]
    fn chained_new_contract_call() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
const held = (a: string) => new Contract(a, ESCROW_ABI, p).deposit(a);
const linked = (a: string) => new Contract(a, ESCROW_ABI, p).connect(s).release(a);
const gone = (a: string) => new Contract(a, ESCROW_ABI, p).withdraw(a);
const inline = (a: string) => new Contract(a, ['function f()'], p).f();
"
        );
        let (rows, counts) = read(&src);
        // DT.10i: a construction chained through `.connect(s)` is a site.
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=7",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=8"
            ]
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 1,
                dynamic: 1
            }
        );
    }

    #[test]
    fn on_and_once_events() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
function watch(p: any) {{
  const escrow = new Contract(a, ESCROW_ABI, p);
  escrow.on('Released', (id: string) => id);
  escrow.once(\"Released(bytes32,address)\", () => {{}});
  escrow.on('Funded', () => {{}});
  escrow.on(escrow.filters.Released(), () => {{}});
  const loose = new Contract(a, IMPORTED_ABI, p);
  loose.once('Anything', () => {{}});
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            [
                "event abi:ESCROW_ABI.Released args=- abi=f:deposit/1,f:release/1,e:Released/2 line=9",
                "event abi:ESCROW_ABI.Released args=- abi=f:deposit/1,f:release/1,e:Released/2 line=10",
                // DT.10i: the filter is the site, once.
                "event abi:ESCROW_ABI.Released args=- abi=f:deposit/1,f:release/1,e:Released/2 line=12",
                "event abi:IMPORTED_ABI.Anything args=- abi=- line=14",
            ]
        );
        assert_eq!(counts.not_in_abi, 1, "Funded");
    }

    #[test]
    fn comments_and_strings_never_match() {
        // Kina demo_trade.js: `step("seller calls escrow.createTrade")`.
        let src = format!(
            "const {{ ethers }} = require(\"ethers\");\n{ABI}\
async function main() {{
  const escrow = new ethers.Contract(dep.address, ESCROW_ABI, wallet);
  step(\"seller calls escrow.deposit\");
  // escrow.release(id) would revert
  /* escrow.release(id) */
  const msg = `escrow.deposit(${{escrow.release(id)}})`;
  const re = /escrow.release\\(/;
}}
"
        );
        let (rows, _) = read(&src);
        assert!(rows.is_empty(), "{rows:?}");
    }

    #[test]
    fn ungated_file_mints_nothing() {
        let src = "class Contract {
  constructor(private a: string, private b: string[], private c: unknown) {}
  deposit(x: string) { return x; }
}
const ABI = ['function deposit(bytes32)'];
export function localDeposit(id: string) {
  const escrow = new Contract('0x0', ABI, null);
  return escrow.deposit(id);
}
// import { Contract } from 'ethers';
";
        let (rows, counts) = read(src);
        assert!(rows.is_empty());
        assert_eq!(counts, TsCounts::default());
        let hh = "import { ethers } from 'hardhat';\nconst c = new ethers.Contract(a, ['function f()'], s);\nc.f();\n";
        assert_eq!(read(hh).0, ["call abi:c.f args=0 abi=f:f/0 line=3"]);
        let ep = "import { Contract } from '@ethersproject/contracts';\nconst c = new Contract(a, ['function f()'], s);\nc.f();\n";
        assert_eq!(read(ep).0.len(), 1);
    }

    #[test]
    fn element_access_and_signature_key() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
async function pay(addr: string, signer: any, id: string) {{
  const escrow = new Contract(addr, ESCROW_ABI, signer);
  await escrow['deposit'](id, {{ value: 1n }});
  await escrow[\"release(bytes32)\"](id, {{ gasLimit: 1 }});
  await escrow?.[`release`](id);
  await escrow['withdraw'](id);
  await escrow['getAddress']();
  await escrow['release(bytes32'](id);
  const pick = escrow['deposit'];
  const vault = new Contract(addr, VAULT_ABI, signer);
  await vault['settle((uint256,address),bytes32[])'](a, b, {{ gasLimit: 1 }});
  await vault['settle()']();
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.deposit args=2 abi=f:deposit/1,f:release/1,e:Released/2 line=9",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=10",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=11",
                "call abi:VAULT_ABI.settle args=2 abi=- line=17",
                "call abi:VAULT_ABI.settle args=0 abi=- line=18",
            ],
            "a signature key's spelled arity is the site's args"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 1,
                dynamic: 0
            },
            "withdraw is no ABI function; a stop-listed key and a malformed signature count nothing"
        );
        assert_eq!(
            signature_key(" f(uint256, address) "),
            Some(("f".to_string(), Some(2)))
        );
        assert_eq!(signature_key("f(uint256,)"), None);
        assert_eq!(signature_key("f(uint256))"), None);
        assert_eq!(signature_key("1f"), None);
    }

    #[test]
    fn non_literal_key_is_dynamic() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
function call(addr: string, p: any, name: string) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  escrow[name](addr);
  escrow[`${{name}}`](addr);
  escrow.callStatic[name](addr);
  escrow.connect(p)[name](addr);
  const f = escrow[name];
  other[name](addr);
}}
"
        );
        let (rows, counts) = read(&src);
        assert!(rows.is_empty(), "{rows:?}");
        assert_eq!(
            counts,
            TsCounts {
                abis: 1,
                not_in_abi: 0,
                dynamic: 4
            },
            "four calls through a computed key on a bound receiver; an uncalled one and an unbound receiver count nothing"
        );
    }

    #[test]
    fn connect_and_attach_chains_and_rebinding() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
async function settle(addr: string, p: any, s: any, id: string) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  await escrow.connect(s).release(id);
  await escrow.attach(addr).connect(s).deposit(id);
  await escrow.connect(s).getAddress();
  await escrow.connect(s).withdraw(id);
  const signed = escrow.connect(s);
  await signed.release(id);
  let moved;
  moved = await signed.attach(addr);
  moved.deposit(id);
  await stranger.connect(s).release(id);
  const linked = new Contract(addr, ESCROW_ABI, p).connect(s);
  linked.release(id);
}}
function later(signed: any, id: string) {{
  return signed.release(id);
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=9",
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=10",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=14",
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=17",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=20",
            ],
            "a rebinding is block-scoped: `later`'s parameter is no contract"
        );
        assert_eq!(
            counts.not_in_abi, 1,
            "withdraw only: getAddress is stop-listed"
        );
        assert_eq!(counts.dynamic, 0);
    }

    #[test]
    fn class_field_and_this_assignment_bindings_scope_to_the_class() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
const TOKEN_ABI = ['function decimals() view returns (uint8)'];
export class Escrows {{
  private readonly escrow: Contract = new Contract(A, ESCROW_ABI, P);
  #vault = new Contract(A, ESCROW_ABI, P);
  private token: Contract | null = null;
  constructor(private p: any) {{
    this.token = new Contract(A, TOKEN_ABI, p);
  }}
  early(id: string) {{
    return this.late.release(id);
  }}
  pay(id: string) {{
    this.escrow.deposit(id);
    this.#vault.release(id);
    this.token.decimals();
    escrow.release(id);
    vault.release(id);
  }}
  arm() {{
    this.late = new Contract(A, ESCROW_ABI, this.p);
    const signed = this.escrow.connect(this.p);
    return signed.release('x');
  }}
}}
class Other {{
  go(id: string) {{ return this.escrow.release(id); }}
}}
function loose(this: any, id: string) {{
  this.vault2 = new Contract(A, ESCROW_ABI, P);
  return this.vault2.release(id);
}}
class Params {{
  constructor(
    private held = new Contract(A, ESCROW_ABI, P),
    plain = new Contract(A, ESCROW_ABI, P),
  ) {{}}
  go(id: string, one = new Contract(A, ESCROW_ABI, P)) {{ this.held.deposit(id); this.one.release(id); return this.plain.release(id); }}
}}
class Inline {{
  constructor(public a: string, protected spare = new Contract(A, ESCROW_ABI, P)) {{}}
  go() {{ return this.spare.release('y'); }}
}}
"
        );
        let (rows, counts) = read(&src);
        assert_eq!(
            rows,
            [
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=16",
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=19",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=20",
                "call abi:TOKEN_ABI.decimals args=0 abi=f:decimals/0 line=21",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=28",
                "call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=43",
                "call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=47",
            ],
            "a `this.` binding reads in its whole class body (line 16 before its \
             assignment), never as a bare name, never in another class or outside one; \
             a parameter property is a field, a plain default parameter is none"
        );
        assert_eq!(
            counts,
            TsCounts {
                abis: 2,
                not_in_abi: 0,
                dynamic: 0
            }
        );
    }

    #[test]
    fn static_call_and_v5_namespace_forms() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
async function preview(addr: string, p: any, id: string) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  await escrow.release.staticCall(id);
  await escrow.deposit.send(id, {{ value: 1n }});
  await escrow.deposit.estimateGas(id);
  await escrow.release.populateTransaction(id);
  await escrow['release'].staticCallResult(id);
  await escrow.callStatic.release(id);
  await escrow.estimateGas.deposit(id, {{ value: 1n }});
  await escrow.populateTransaction.release(id);
  await escrow.functions.release(id);
  await escrow.callStatic.getAddress();
  await escrow.estimateGas.withdraw(id);
  await escrow.withdraw.staticCall(id);
  const fn = escrow.release.staticCall;
  await escrow.release.wait(id);
  await escrow.estimateGas(id);
}}
"
        );
        let (rows, counts) = read(&src);
        let abi = "abi=f:deposit/1,f:release/1,e:Released/2";
        let want: Vec<String> = [
            ("release", 1, 9),
            ("deposit", 2, 10),
            ("deposit", 1, 11),
            ("release", 1, 12),
            ("release", 1, 13),
            ("release", 1, 14),
            ("deposit", 2, 15),
            ("release", 1, 16),
            ("release", 1, 17),
        ]
        .iter()
        .map(|(m, args, line)| format!("call abi:ESCROW_ABI.{m} args={args} {abi} line={line}"))
        .collect();
        assert_eq!(rows, want, "args is the final call's argument count");
        assert_eq!(counts.not_in_abi, 2, "withdraw twice");
        assert_eq!(counts.dynamic, 0);
    }

    #[test]
    fn query_filter_filters_and_add_listener_events() {
        let src = format!(
            "import {{ Contract }} from 'ethers';\n{ABI}\
async function watch(addr: string, p: any, cb: any) {{
  const escrow = new Contract(addr, ESCROW_ABI, p);
  escrow.addListener('Released', cb);
  escrow.on(escrow.filters.Released(), cb);
  await escrow.queryFilter('Released(bytes32,address)', 0);
  await escrow.queryFilter(escrow.filters.Released(null), 0, 9);
  const topic = escrow.filters.Released(id);
  escrow.connect(p).once(escrow.filters['Released'](), cb);
  escrow.filters.Funded();
  escrow.queryFilter('Funded');
  escrow.off('Released', cb);
  escrow.removeListener('Released', cb);
  escrow.queryFilter(filter, 0);
  const f = escrow.filters.Released;
}}
"
        );
        let (rows, counts) = read(&src);
        let want: Vec<String> = (9..=14)
            .map(|line| {
                format!(
                    "event abi:ESCROW_ABI.Released args=- abi=f:deposit/1,f:release/1,e:Released/2 line={line}"
                )
            })
            .collect();
        assert_eq!(
            rows, want,
            "a filter inside on / once / queryFilter is one site"
        );
        assert_eq!(counts.not_in_abi, 2, "Funded twice");
    }

    #[test]
    fn kina_wallet_service_shape() {
        // Kina frontend/src/app/core/services/wallet.service.ts, condensed.
        let escrow_fns: [(&str, &str, &str); 10] = [
            (
                "createTrade",
                "address buyer, uint256 amount",
                "buyer, amountBN",
            ),
            ("confirmPayment", "uint256 tradeId", "id"),
            ("releaseFunds", "uint256 tradeId", "id"),
            ("raiseDispute", "uint256 tradeId", "id"),
            ("cancelTrade", "uint256 tradeId", "id"),
            ("claimTimeout", "uint256 tradeId", "id"),
            ("claimTimeoutForBuyer", "uint256 tradeId", "id"),
            (
                "proposeResolution",
                "uint256 tradeId, uint8 outcome",
                "id, outcome",
            ),
            (
                "acceptResolution",
                "uint256 tradeId, uint8 expectedOutcome",
                "id, outcome",
            ),
            ("enterPenaltyHold", "uint256 tradeId", "id"),
        ];
        let mut src = String::from(
            "import { Injectable, inject, signal } from '@angular/core';
import { BrowserProvider, Contract, ethers, formatUnits } from 'ethers';

// Minimal ERC-20 ABI - only what we need.
const ERC20_ABI = [
  'function balanceOf(address) view returns (uint256)',
  'function decimals() view returns (uint8)',
  'function approve(address spender, uint256 amount) returns (bool)',
];

const ESCROW_ABI = [
",
        );
        for (f, params, _) in escrow_fns {
            src.push_str(&format!("  'function {f}({params})',\n"));
        }
        src.push_str(
            "];

@Injectable({ providedIn: 'root' })
export class WalletService {
  private provider: BrowserProvider | null = null;

  async approveEscrow(chain: Chain, amount: string): Promise<string> {
    const signer = await this.getSigner();
    const token = new Contract(this.getStablecoinAddress(chain), ERC20_ABI, signer);
    const tx = await token['approve'](environment.chains[chain].contractAddress, BigInt(amount));
    await tx.wait();
    return tx.hash;
  }
",
        );
        for (f, _, args) in escrow_fns {
            src.push_str(&format!(
                "
  async {f}(chain: Chain, id: string, buyer: string, amountBN: bigint, outcome: number): Promise<string> {{
    const signer = await this.getSigner();
    const escrow = new Contract(environment.chains[chain].contractAddress, ESCROW_ABI, signer);
    const tx = await escrow['{f}']({args});
    await tx.wait();
    return tx.hash;
  }}
"
            ));
        }
        src.push_str(
            "
  async loadBalance(chain: Chain): Promise<void> {
    if (!this.provider || !this._state().address) return;
    const token = new Contract(this.getStablecoinAddress(chain), ERC20_ABI, this.provider);
    const decimals = await token['decimals']();
    const raw = await token['balanceOf'](this._state().address);
    this._state.update(s => ({ ...s, balance: formatUnits(raw, decimals) }));
  }
}
",
        );
        let out = crate::evm::extract_evm_clients(
            &src,
            "angular",
            glia_core::NodeId(1),
            glia_core::RepoId(1),
        );
        let path = "frontend/src/app/core/services/wallet.service.ts";
        assert_eq!(
            out.markers(path),
            [format!(
                "[evm-client] lang=ts calls=13 events=0 sites=13 (ethers=13 hardhat=0 typechain=0 viem=0 web3=0 geth=0) abis=2 not_in_abi=0 dynamic=0 path={path}"
            )]
        );
        let mut qnames: Vec<&str> = out.nav.qname_by_id.values().map(String::as_str).collect();
        qnames.sort_unstable();
        let mut want: Vec<String> = escrow_fns
            .iter()
            .map(|(f, _, _)| format!("evm_call:abi:ESCROW_ABI.{f}"))
            .chain(
                ["approve", "balanceOf", "decimals"]
                    .iter()
                    .map(|f| format!("evm_call:abi:ERC20_ABI.{f}")),
            )
            .collect();
        want.sort_unstable();
        assert_eq!(
            qnames, want,
            "ten escrow['..'] sites over a 10-entry ABI, three token['..']"
        );
        let (rows, _) = read(&src);
        assert!(
            rows.iter().any(|r| r.starts_with(
                "call abi:ERC20_ABI.decimals args=0 abi=f:approve/2,f:balanceOf/1,f:decimals/0 "
            )),
            "{rows:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.starts_with("call abi:ESCROW_ABI.createTrade args=2 ")),
            "{rows:?}"
        );
    }
}
