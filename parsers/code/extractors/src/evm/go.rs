//! The Go go-ethereum contract call and event reader (DT.10e, DT.10k).
//!
//! 1. **Gate** ([`geth_imports`]). The file imports a
//!    `github.com/ethereum/go-ethereum` package (single or grouped `import`,
//!    any alias); otherwise nothing is read. The imports name the packages the
//!    bindings are spelled with: `accounts/abi` (`abi`, or its alias) and
//!    `accounts/abi/bind` or `bind/v2` (`bind`, or its alias).
//! 2. **ABI declarations** ([`abi_decls`]). A package-level `const` / `var`
//!    spec (single or grouped, with or without a type) whose value is one
//!    string literal, raw or interpreted (unescaped per Go by
//!    `scan::go_string_literal`), that `abi::read_json_abi` reads: ABI `<N>`
//!    (abigen v1's `const <X>ABI = ".."` is one). The literal is read from
//!    the source at its offsets, so a JSON ABI's braces never unbalance the
//!    view. A function-local `const` is no ABI declaration. An abigen
//!    METADATA literal, `var <N> = &bind.MetaData{ .. ABI: <string> .. }`
//!    (v1) or `var <N> = bind.MetaData{ .. }` (v2), whose `ABI` field reads,
//!    is ABI `<N>` too ([`metadata_abi`]).
//! 3. **Functions** ([`func_decls`]). Every top-level `func (<recv>
//!    <Type>)? <Name>[<tparams>]?(<params>) <results> {..}` with its receiver
//!    name, receiver type name (`*` and type parameters dropped), parameter
//!    names and balanced body. Parameters split at depth-0 commas, so the
//!    grouped `chainName, contractAddr, method string` names every one, and a
//!    variadic `args ...any` is the last.
//! 4. **Bindings.** The first left-hand element of `<x>(, <err>)? := ..`,
//!    `var <x>(, <err>)? = ..` or `<x>(, <err>)? = ..`, or the field of a
//!    composite literal `<field>: ..`, binds the value of
//!    - `<N>.GetAbi()` / `<N>.ParseABI()` on a metadata declaration N of the
//!      file (abigen v1 / v2): `Scope::Abi(N)` with N's entries;
//!    - `abi.JSON(strings.NewReader(<N>))`: `Scope::Abi(N)` with N's entries
//!      (none when N is declared in another file of the package, so the
//!      resolver name-hints it); an inline literal that reads gives
//!      `Scope::Abi(<x>)` with its entries and counts as an ABI read;
//!    - `bind.NewBoundContract(<addr>, <v> | *<v> | &<v>, ..)`: the value v
//!      resolves to (step 6), else `Scope::Abi(<v>'s last segment)` with no
//!      entries.
//!
//!    The constructions bind in that order, so a bound contract built from a
//!    metadata binding reads it. Any other ABI argument (an `io.Reader`, a
//!    call) is DYNAMIC: counted, and the binding's sites mint nothing. A
//!    declared binding (`:=`, `var`) is
//!    visible from the end of its construction to the end of its innermost
//!    block (the DT.10c rule); a package-level one, and an assignment to a
//!    package `var`, in the whole file; any other assignment in its
//!    function's body. A function whose receiver or a parameter is named like
//!    a binding's root hides a binding declared outside it. A site reads the
//!    NEAREST visible binding of its receiver.
//! 5. **Hand-offs.** A bound identifier handed to a struct field, `<field>:
//!    <x>` / `<field>: *<x>` / `<field>: &<x>` in a composite literal or
//!    `<recv>.<field> = <x>` as a statement, gives the field x's value,
//!    file-wide by field name and one hop (a field never hands on a field), so
//!    abigen v2's `parsed, err := <N>.ParseABI(); return &<X>{abi: *parsed}`
//!    binds the field `abi`. A selector left-hand side (`s.abi, err =
//!    abi.JSON(..)`) binds its text and its last segment as a field. A field
//!    bound twice reads its first binding, deterministically.
//! 6. **Scope** ([`Reader::scope_of`]). A receiver is a selector chain `a.b.c`
//!    directly before the needle's dot. It resolves through its full text
//!    first (the visible binding of that text), then through its last segment
//!    as a field; a chain hanging off a call or an index (`f().escrow`)
//!    resolves through its field only. A bound receiver reads its binding's
//!    scope and entries. An unbound chain rooted at its method's receiver
//!    (abigen v1's `_Escrow.contract`, whose field no statement of the file
//!    binds) whose receiver type is `<X>` or `<X>` plus an abigen suffix
//!    ([`ABIGEN_SUFFIXES`]: `EscrowCaller`, `EscrowFilterer`, ..), with an
//!    ABI declaration `<X>MetaData` (else `<X>ABI`) in the same file, reads
//!    that declaration ([`Reader::receiver_type_abi`]); an `abigen --type`
//!    rename is honoured because the type and the metadata share its name.
//!    Any other unbound receiver is `Scope::Recv(<last segment>)` with no
//!    entries, which `resolver:evm` pairs by name_hint (HEURISTIC, Weak,
//!    arity-checked).
//! 7. **Sites.** Every needle is dot-preceded (so `RawTransact(` and
//!    `NewKeyedTransactorWithChainID(` are none) and needs a string literal at
//!    its name position. Calls:
//!    - `<r>.Pack("m", ..)`, `<r>.Methods["m"]`, `<r>.Transact(<opts>, "m", ..)`;
//!    - `<r>.Call(<opts>, <out>, "m", ..)`, the third argument; a `.Call(<out>,
//!      "eth_..")` with a string literal second is go-ethereum's `rpc.Client`
//!      and is not read.
//!
//!    Events: `<r>.Events["E"]`, `<r>.FilterLogs(<opts>, "E", ..)`,
//!    `<r>.WatchLogs(<opts>, "E", ..)`, `<r>.UnpackLog(<out>, "E", <log>)`
//!    and `<r>.UnpackLogIntoMap(<out>, "E", <log>)`; ethclient's
//!    `FilterLogs(ctx, query)` has no literal there and is none. The decoding
//!    forms `<r>.Unpack("n", ..)` / `<r>.UnpackIntoInterface(<out>, "n", ..)`
//!    are a call where r's ABI lists n as a function, else an event where it
//!    lists n as an event; with no ABI, no site.
//!
//!    `args` counts a call's arguments after the name; a trailing `xs...`
//!    spread, `Methods[..]` and the two Unpack forms (whose arguments are the
//!    bytes decoded, not the call's) give unknown, and an event passes none.
//!    `lib` is `geth`: exact arity, no overrides argument. With entries known,
//!    a member that is no function (a call) or no event (an event) of them
//!    mints nothing and counts `not_in_abi`; with none (`Scope::Recv`, an ABI
//!    of another file) every name-literal site mints and the resolver decides.
//!    A name in a JSON-RPC namespace ([`JSON_RPC`]) never mints and is not
//!    counted. A non-literal name on a bound receiver
//!    (`parsedABI.Pack(method, args...)`, Kina signer.go) mints nothing and
//!    counts `dynamic` (DT.10f reads those through their wrappers); the
//!    receiver-type rule binds nothing, so abigen's forwarding `Raw` methods
//!    (`_Escrow.Contract.EscrowCaller.contract.Call(opts, result, method,
//!    ..)`) count nothing.
//! 8. **Event switches** ([`Reader::switch_sites`]). A `for <k>(, <v>)? :?=
//!    range <r>.Events {` loop, and inside its block a `switch <k> {` or
//!    `switch <v>.Name {`: every string literal of a `case "A"(, "B")*:` list
//!    of that switch's own body (a nested switch's cases are deeper) is an
//!    event site of r's scope, read at the literal (Kina indexer.go
//!    processLog). A switch on any other tag contributes nothing.

use glia_code_domain::evm::{AbiEntry, ClientSide, EntryKind, Scope};

use super::Site;
use super::abi::read_json_abi;
use super::scan::{
    enclosing_block, go_string_literal, ident_end, ident_start_before, is_ident, matching,
    prev_non_ws, skip_ws, split_args, trim, word_hits,
};

/// The client library every site of this reader names.
const LIB: &str = "geth";

/// The go-ethereum module path the gate reads.
const GETH: &str = "github.com/ethereum/go-ethereum";

/// The go-ethereum package `abi.JSON` lives in.
const ABI_PKG: &str = "github.com/ethereum/go-ethereum/accounts/abi";

/// The go-ethereum packages `bind.NewBoundContract` lives in (v1, v2).
const BIND_PKGS: &[&str] = &[
    "github.com/ethereum/go-ethereum/accounts/abi/bind",
    "github.com/ethereum/go-ethereum/accounts/abi/bind/v2",
];

/// JSON-RPC namespaces: a name in one is an RPC method, never a contract
/// member.
const JSON_RPC: &[&str] = &["eth_", "net_", "web3_", "debug_", "personal_", "txpool_"];

/// Keywords that open a type, so `chan int` is no parameter named `chan`.
const TYPE_KEYWORDS: &[&str] = &["chan", "func", "map", "struct", "interface"];

/// Keywords a statement's left-hand side may start with.
const LHS_KEYWORDS: &[&str] = &["var", "if", "for", "switch", "else"];

/// Bytes before an `=` that make it no plain assignment (`==`, `!=`, `+=`..).
const NOT_ASSIGN: &[u8] = b":=!<>+-*/%&|^";

/// The suffixes abigen appends to a contract's type name `<X>` for its
/// wrapper types, longest first, so `EscrowCallerSession` is `Escrow`'s
/// before it could be read as `EscrowCaller`'s `Session` (step 6).
const ABIGEN_SUFFIXES: &[&str] = &[
    "TransactorSession",
    "FiltererSession",
    "CallerSession",
    "TransactorRaw",
    "Transactor",
    "CallerRaw",
    "Filterer",
    "Session",
    "Caller",
    "Raw",
];

/// The two metadata members that parse a declaration's ABI (step 4): abigen
/// v1's `GetAbi`, v2's `ParseABI`.
const METADATA_PARSERS: &[&str] = &["GetAbi", "ParseABI"];

/// What [`scan`] counted besides its sites.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct GoCounts {
    /// ABI declarations read (step 2), plus inline literals (step 4).
    pub(super) abis: usize,
    /// Members left out because the known ABI does not list them.
    pub(super) not_in_abi: usize,
    /// Bindings whose ABI argument could not be read, and non-literal names
    /// on bound receivers.
    pub(super) dynamic: usize,
}

/// One package-level ABI declaration (step 2).
#[derive(Debug)]
struct AbiDecl {
    name: String,
    entries: Vec<AbiEntry>,
    /// An abigen `<bind>.MetaData{ABI: ..}` literal, whose `GetAbi()` /
    /// `ParseABI()` bind (step 4).
    meta: bool,
}

/// One package-level `const` / `var` spec: `<names> (<type>)? (= <values>)?`.
#[derive(Debug)]
struct ValueSpec {
    var: bool,
    names: Vec<String>,
    /// The value ranges, in the names' order; empty without an `=`.
    values: Vec<(usize, usize)>,
}

/// One top-level function or method declaration (step 3).
#[derive(Debug, Clone, PartialEq, Eq)]
struct FuncDecl {
    /// The method receiver's name; `None` for a function or an unnamed
    /// receiver.
    receiver: Option<String>,
    /// The method receiver's type name, `*` and type parameters dropped
    /// (`(_Escrow *EscrowCaller)` is `EscrowCaller`); `None` for a function.
    receiver_type: Option<String>,
    /// The parameter names in order; empty when the parameters are unnamed.
    params: Vec<String>,
    /// The offsets of the body's braces.
    body: (usize, usize),
}

/// What a binding or a field holds (steps 4 and 5).
#[derive(Debug, Clone)]
struct Value {
    /// `None` for a dynamic construction: its sites mint nothing.
    scope: Option<Scope>,
    /// The ABI's entries; `None` when unknown.
    entries: Option<Vec<AbiEntry>>,
}

/// One binding of an identifier or a selector text (step 4).
#[derive(Debug)]
struct Binding {
    name: String,
    value: Value,
    /// The end of its construction: a site must come after it, unless it is
    /// visible in the whole file.
    at: usize,
    block: (usize, usize),
}

/// One field binding (step 5).
#[derive(Debug)]
struct Field {
    name: String,
    value: Value,
    at: usize,
}

/// What a construction's result is bound to (step 4).
#[derive(Debug)]
enum Target {
    /// An identifier or selector chain; `declared` by `:=` / `var`.
    Assign {
        text: String,
        at: usize,
        declared: bool,
    },
    /// A composite literal's field.
    Field(String),
}

/// The binding constructions (step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ctor {
    /// `<N>.GetAbi()` / `<N>.ParseABI()` on a metadata declaration.
    Meta,
    /// `abi.JSON(..)`.
    Json,
    /// `bind.NewBoundContract(..)`.
    Bound,
}

/// The site needles (step 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Needle {
    Pack,
    Methods,
    Transact,
    Call,
    Unpack,
    UnpackIntoInterface,
    Events,
    FilterLogs,
    WatchLogs,
    UnpackLog,
    UnpackLogIntoMap,
}

/// Every needle and the member it is spelled as.
const NEEDLES: &[(&str, Needle)] = &[
    ("Pack", Needle::Pack),
    ("Methods", Needle::Methods),
    ("Transact", Needle::Transact),
    ("Call", Needle::Call),
    ("Unpack", Needle::Unpack),
    ("UnpackIntoInterface", Needle::UnpackIntoInterface),
    ("Events", Needle::Events),
    ("FilterLogs", Needle::FilterLogs),
    ("WatchLogs", Needle::WatchLogs),
    ("UnpackLog", Needle::UnpackLog),
    ("UnpackLogIntoMap", Needle::UnpackLogIntoMap),
];

impl Needle {
    /// The index of the name argument.
    fn name_arg(self) -> usize {
        match self {
            Needle::Pack | Needle::Unpack | Needle::Methods | Needle::Events => 0,
            Needle::Transact
            | Needle::UnpackIntoInterface
            | Needle::FilterLogs
            | Needle::WatchLogs
            | Needle::UnpackLog
            | Needle::UnpackLogIntoMap => 1,
            Needle::Call => 2,
        }
    }

    /// The index forms `<r>.<needle>["n"]`.
    fn indexed(self) -> bool {
        matches!(self, Needle::Methods | Needle::Events)
    }

    /// The decoding forms: a call where the ABI lists a function, an event
    /// where it lists an event, and their arguments give no arity.
    fn decodes(self) -> bool {
        matches!(self, Needle::Unpack | Needle::UnpackIntoInterface)
    }

    /// The event forms.
    fn event(self) -> bool {
        matches!(
            self,
            Needle::Events
                | Needle::FilterLogs
                | Needle::WatchLogs
                | Needle::UnpackLog
                | Needle::UnpackLogIntoMap
        )
    }
}

/// A receiver selector chain (step 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chain<'v> {
    /// The whole chain, `s.escrow`.
    text: &'v str,
    /// Its last segment, `escrow`.
    last: &'v str,
    /// The chain's first byte.
    start: usize,
    /// False when the chain hangs off a call or an index (`f().escrow`).
    rooted: bool,
}

/// The sites of one Go file (module doc) over its code `view`.
pub(super) fn scan(source: &str, view: &str) -> (Vec<Site>, GoCounts) {
    let mut counts = GoCounts::default();
    let imports = geth_imports(source, view);
    if imports.is_empty() {
        return (Vec::new(), counts);
    }
    let package = |paths: &[&str]| {
        imports
            .iter()
            .find(|(path, _)| paths.contains(&path.as_str()))
            .map(|(_, name)| name.as_str())
    };
    let specs = package_specs(view);
    let decls = abi_decls(source, view, &specs, package(BIND_PKGS));
    counts.abis = decls.len();
    let package_vars = specs
        .iter()
        .filter(|s| s.var)
        .flat_map(|s| s.names.iter().cloned())
        .collect();
    // Step 4's constructions in binding order: metadata parsers first, so a
    // bound contract built from their result reads it.
    let mut ctors: Vec<(Ctor, &str, &str)> = Vec::new();
    for decl in decls.iter().filter(|d| d.meta) {
        for &member in METADATA_PARSERS {
            ctors.push((Ctor::Meta, decl.name.as_str(), member));
        }
    }
    ctors.extend(package(&[ABI_PKG]).map(|pkg| (Ctor::Json, pkg, "JSON")));
    ctors.extend(package(BIND_PKGS).map(|pkg| (Ctor::Bound, pkg, "NewBoundContract")));
    let mut reader = Reader {
        source,
        view,
        decls: &decls,
        package_vars,
        funcs: func_decls(view),
        locals: Vec::new(),
        fields: Vec::new(),
    };
    for (ctor, pkg, member) in ctors {
        let first = reader.locals.len();
        reader.bind(ctor, pkg, member, &mut counts);
        reader.hand_off(first);
    }
    let sites = reader.sites(&mut counts);
    (sites, counts)
}

/// Step 1: the `(import path, package name)` of every go-ethereum import;
/// empty when the file imports none.
fn geth_imports(source: &str, view: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !source.contains(GETH) {
        return out;
    }
    let vb = view.as_bytes();
    for at in top_level(view, word_hits(view, "import")) {
        if at > 0 && vb[at - 1] == b'.' {
            continue;
        }
        let k = skip_ws(vb, at + "import".len());
        let (from, to) = if vb.get(k) == Some(&b'(') {
            match matching(view, k) {
                Some(close) => (k + 1, close),
                None => continue,
            }
        } else {
            (k, line_end(vb, k))
        };
        let mut q = from;
        while q < to {
            if !matches!(vb[q], b'"' | b'`') {
                q += 1;
                continue;
            }
            let Some((path, end)) = go_string_literal(source, q) else {
                q += 1;
                continue;
            };
            if path == GETH || path.starts_with(&format!("{GETH}/")) {
                let name = import_alias(view, q).unwrap_or_else(|| package_name(&path));
                out.push((path, name));
            }
            q = end;
        }
    }
    out
}

/// The alias written before the import path whose quote is at `q`, on its
/// line (`gabi "…/abi"`); `None` without one.
fn import_alias(view: &str, q: usize) -> Option<String> {
    let vb = view.as_bytes();
    let mut end = q;
    while end > 0 && matches!(vb[end - 1], b' ' | b'\t') {
        end -= 1;
    }
    let start = ident_start_before(vb, end)?;
    let alias = &view[start..end];
    (alias != "import").then(|| alias.to_string())
}

/// The package name an import path binds without an alias: its last segment,
/// or the one before a major-version suffix (`bind/v2` is `bind`).
fn package_name(path: &str) -> String {
    let mut segments = path.rsplit('/');
    let last = segments.next().unwrap_or(path);
    let versioned =
        last.len() > 1 && last.starts_with('v') && last[1..].bytes().all(|c| c.is_ascii_digit());
    match segments.next() {
        Some(prev) if versioned => prev.to_string(),
        _ => last.to_string(),
    }
}

/// The offset of the `\n` ending the line at `from` (or the end).
fn line_end(vb: &[u8], from: usize) -> usize {
    vb.get(from..)
        .and_then(|rest| rest.iter().position(|&c| c == b'\n'))
        .map_or(vb.len(), |p| from + p)
}

/// The offsets of `hits` (ascending) outside every `{..}` of the view.
fn top_level(view: &str, hits: Vec<usize>) -> Vec<usize> {
    brace_depth0(view.as_bytes(), 0, view.len(), hits)
}

/// The offsets of `hits` (ascending) in `[from, to)` outside every `{..}`
/// that opens in that range.
fn brace_depth0(vb: &[u8], from: usize, to: usize, hits: Vec<usize>) -> Vec<usize> {
    let mut depth = 0usize;
    let mut k = from;
    let mut out = Vec::new();
    for h in hits.into_iter().filter(|&h| from <= h && h < to) {
        while k < h {
            match vb[k] {
                b'{' => depth += 1,
                b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
            k += 1;
        }
        if depth == 0 {
            out.push(h);
        }
    }
    out
}

/// True when only spaces and tabs stand between `at` and the start of its
/// statement (a newline, `;`, `{`, `}` or the file's start).
fn statement_starts(vb: &[u8], at: usize) -> bool {
    let mut k = at;
    while k > 0 && matches!(vb[k - 1], b' ' | b'\t') {
        k -= 1;
    }
    k == 0 || matches!(vb[k - 1], b'\n' | b';' | b'{' | b'}')
}

/// True when only spaces and tabs stand between `at` and the end of its
/// statement (a newline, `;`, `}` or the file's end).
fn statement_ends(vb: &[u8], at: usize) -> bool {
    let mut k = at;
    while k < vb.len() && matches!(vb[k], b' ' | b'\t') {
        k += 1;
    }
    k == vb.len() || matches!(vb[k], b'\n' | b';' | b'}')
}

// ---- step 2: ABI declarations ----------------------------------------------------

/// Every package-level `const` / `var` spec, single or grouped.
fn package_specs(view: &str) -> Vec<ValueSpec> {
    let vb = view.as_bytes();
    let mut out = Vec::new();
    for (word, var) in [("const", false), ("var", true)] {
        for at in top_level(view, word_hits(view, word)) {
            if at > 0 && vb[at - 1] == b'.' {
                continue;
            }
            let k = skip_ws(vb, at + word.len());
            if vb.get(k) == Some(&b'(') {
                let Some(close) = matching(view, k) else {
                    continue;
                };
                let mut s = skip_ws(vb, k + 1);
                while s < close {
                    let end = spec_end(vb, s, close);
                    out.extend(value_spec(view, s, end, var));
                    s = skip_ws(vb, end + 1);
                }
            } else {
                let end = spec_end(vb, k, vb.len());
                out.extend(value_spec(view, k, end, var));
            }
        }
    }
    out
}

/// The end of the spec starting at `from`: the first depth-0 newline or `;`
/// before `limit`, a literal skipped whole (a raw string's newlines are kept
/// in the view).
fn spec_end(vb: &[u8], from: usize, limit: usize) -> usize {
    let mut depth = 0usize;
    let mut k = from;
    while k < limit {
        match vb[k] {
            q @ (b'"' | b'\'' | b'`') => {
                // The view blanks a literal's contents, so its closing quote
                // is the next one.
                k = vb[k + 1..limit]
                    .iter()
                    .position(|&c| c == q)
                    .map_or(limit, |p| k + 1 + p + 1);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'\n' | b';' if depth == 0 => return k,
            _ => {}
        }
        k += 1;
    }
    limit
}

/// The spec `<names> (<type>)? (= <values>)?` in `[from, end)`; `None` when it
/// names nothing.
fn value_spec(view: &str, from: usize, end: usize, var: bool) -> Option<ValueSpec> {
    let vb = view.as_bytes();
    let mut names = Vec::new();
    let mut k = from;
    loop {
        let e = ident_end(vb, k).filter(|&e| e <= end)?;
        names.push(view[k..e].to_string());
        k = skip_ws(vb, e);
        if k < end && vb[k] == b',' {
            k = skip_ws(vb, k + 1);
            continue;
        }
        break;
    }
    let eq = (k..end).find(|&j| {
        vb[j] == b'='
            && vb.get(j + 1) != Some(&b'=')
            && (j == 0 || !NOT_ASSIGN.contains(&vb[j - 1]))
    });
    let values = eq.map_or_else(Vec::new, |eq| split_args(view, eq, end));
    Some(ValueSpec { var, names, values })
}

/// The ABI declarations among `specs` (step 2); `bind` is the imported name
/// of go-ethereum's bind package, which spells a metadata literal.
fn abi_decls(source: &str, view: &str, specs: &[ValueSpec], bind: Option<&str>) -> Vec<AbiDecl> {
    let mut out = Vec::new();
    for spec in specs {
        for (name, &(s, e)) in spec.names.iter().zip(&spec.values) {
            let (text, meta) = match literal(source, s, e) {
                Some(text) => (text, false),
                None => match bind.and_then(|b| metadata_abi(source, view, (s, e), b)) {
                    Some(text) => (text, true),
                    None => continue,
                },
            };
            if let Some(entries) = read_json_abi(&text) {
                out.push(AbiDecl {
                    name: name.clone(),
                    entries,
                    meta,
                });
            }
        }
    }
    out
}

/// The `ABI` field's string of the abigen literal `&<bind>.MetaData{..}`
/// (v1) or `<bind>.MetaData{..}` (v2) spanning exactly `value` (step 2);
/// `None` for any other value.
fn metadata_abi(source: &str, view: &str, value: (usize, usize), bind: &str) -> Option<String> {
    let vb = view.as_bytes();
    let (mut k, e) = value;
    if vb.get(k) == Some(&b'&') {
        k = skip_ws(vb, k + 1);
    }
    let pkg_end = ident_end(vb, k).filter(|&p| &view[k..p] == bind)?;
    let dot = skip_ws(vb, pkg_end);
    if vb.get(dot) != Some(&b'.') {
        return None;
    }
    let m = skip_ws(vb, dot + 1);
    let m_end = ident_end(vb, m).filter(|&p| &view[m..p] == "MetaData")?;
    let open = skip_ws(vb, m_end);
    if vb.get(open) != Some(&b'{') {
        return None;
    }
    let close = matching(view, open).filter(|&c| c + 1 == e)?;
    split_args(view, open, close)
        .into_iter()
        .find_map(|(ps, pe)| {
            let key_end = ident_end(vb, ps).filter(|&p| &view[ps..p] == "ABI")?;
            let colon = skip_ws(vb, key_end);
            if vb.get(colon) != Some(&b':') {
                return None;
            }
            literal(source, skip_ws(vb, colon + 1), pe)
        })
}

/// The value of the string literal spanning exactly `[s, e)` of `source`.
fn literal(source: &str, s: usize, e: usize) -> Option<String> {
    go_string_literal(source, s)
        .filter(|&(_, end)| end == e)
        .map(|(text, _)| text)
}

// ---- step 3: functions ------------------------------------------------------------

/// Every top-level function or method declaration with a body (step 3).
fn func_decls(view: &str) -> Vec<FuncDecl> {
    let vb = view.as_bytes();
    let mut out = Vec::new();
    for at in top_level(view, word_hits(view, "func")) {
        if !statement_starts(vb, at) {
            continue;
        }
        let mut k = skip_ws(vb, at + "func".len());
        let (mut receiver, mut receiver_type) = (None, None);
        if vb.get(k) == Some(&b'(') {
            let Some(close) = matching(view, k) else {
                continue;
            };
            if let Some(&(s, e)) = split_args(view, k, close).first() {
                receiver = named_piece(view, s, e);
                receiver_type = receiver_type_name(view, s, e);
            }
            k = skip_ws(vb, close + 1);
        }
        let Some(name_end) = ident_end(vb, k) else {
            continue;
        };
        k = skip_ws(vb, name_end);
        if vb.get(k) == Some(&b'[') {
            let Some(close) = matching(view, k) else {
                continue;
            };
            k = skip_ws(vb, close + 1);
        }
        if vb.get(k) != Some(&b'(') {
            continue;
        }
        let Some(p_close) = matching(view, k) else {
            continue;
        };
        let params = param_names(view, &split_args(view, k, p_close));
        let Some(open) = body_open(view, p_close + 1) else {
            continue;
        };
        let Some(close) = matching(view, open) else {
            continue;
        };
        out.push(FuncDecl {
            receiver,
            receiver_type,
            params,
            body: (open, close),
        });
    }
    out
}

/// The type name of a receiver piece `(<name>)? *?<Type>([..])?`.
fn receiver_type_name(view: &str, s: usize, e: usize) -> Option<String> {
    let vb = view.as_bytes();
    let mut k = s;
    if named_piece(view, s, e).is_some() {
        k = skip_ws(vb, ident_end(vb, s)?);
    }
    if vb.get(k) == Some(&b'*') {
        k = skip_ws(vb, k + 1);
    }
    let t_end = ident_end(vb, k).filter(|&t| t <= e)?;
    Some(view[k..t_end].to_string())
}

/// The name of a parameter piece `<name> <type>`; `None` for a piece that is
/// a type alone (`context.Context`, `*big.Int`, `chan int`) or one name.
fn named_piece(view: &str, s: usize, e: usize) -> Option<String> {
    let vb = view.as_bytes();
    let id_end = ident_end(vb, s).filter(|&k| k < e)?;
    let name = &view[s..id_end];
    let typed = vb[id_end].is_ascii_whitespace() && skip_ws(vb, id_end) < e;
    (typed && !TYPE_KEYWORDS.contains(&name)).then(|| name.to_string())
}

/// The names of a parameter list's pieces: a named piece's name, and a bare
/// identifier grouped with a later type (`a, b string`); none when no piece
/// is named.
fn param_names(view: &str, pieces: &[(usize, usize)]) -> Vec<String> {
    let named: Vec<Option<String>> = pieces
        .iter()
        .map(|&(s, e)| named_piece(view, s, e))
        .collect();
    if named.iter().all(Option::is_none) {
        return Vec::new();
    }
    pieces
        .iter()
        .zip(named)
        .filter_map(|(&(s, e), name)| {
            name.or_else(|| is_ident(&view[s..e]).then(|| view[s..e].to_string()))
        })
        .collect()
}

/// The `{` opening the body after a signature's parameters end at `from`;
/// `None` when the line ends first (a declaration without a body). A
/// `struct{..}` / `interface{..}` result type is stepped over.
fn body_open(view: &str, from: usize) -> Option<usize> {
    let vb = view.as_bytes();
    let mut k = from;
    while k < vb.len() {
        match vb[k] {
            b'\n' => return None,
            b'(' | b'[' => k = matching(view, k)? + 1,
            b'{' => {
                let word = prev_non_ws(vb, k)
                    .and_then(|p| ident_start_before(vb, p + 1).map(|s| &view[s..=p]));
                if matches!(word, Some("struct" | "interface")) {
                    k = matching(view, k)? + 1;
                } else {
                    return Some(k);
                }
            }
            _ => k += 1,
        }
    }
    None
}

// ---- steps 4-7: the reader ----------------------------------------------------------

/// One file's declarations, bindings and fields.
struct Reader<'a> {
    source: &'a str,
    view: &'a str,
    decls: &'a [AbiDecl],
    package_vars: Vec<String>,
    funcs: Vec<FuncDecl>,
    locals: Vec<Binding>,
    fields: Vec<Field>,
}

impl Reader<'_> {
    /// Step 4: every `<pkg>.<member>(..)` construction of `ctor` with a
    /// target.
    fn bind(&mut self, ctor: Ctor, pkg: &str, member: &str, counts: &mut GoCounts) {
        let view = self.view;
        let vb = view.as_bytes();
        for at in word_hits(view, pkg) {
            if at > 0 && vb[at - 1] == b'.' {
                continue;
            }
            let Some((open, close)) = call_after(view, at + pkg.len(), member) else {
                continue;
            };
            let Some(target) = lhs_target(view, at) else {
                continue;
            };
            let args = split_args(view, open, close);
            let value = match ctor {
                Ctor::Meta => self.meta_value(pkg),
                Ctor::Json => self.json_value(&args, &target, counts),
                Ctor::Bound => self.bound_value(&args, at),
            };
            if value.scope.is_none() {
                counts.dynamic += 1;
            }
            self.push_target(target, value, close);
        }
    }

    /// The value of `<name>.GetAbi()` / `<name>.ParseABI()` on the metadata
    /// declaration `name` (step 4).
    fn meta_value(&self, name: &str) -> Value {
        Value {
            scope: Some(Scope::Abi(name.to_string())),
            entries: self
                .decls
                .iter()
                .find(|d| d.name == name)
                .map(|d| d.entries.clone()),
        }
    }

    /// The value of `abi.JSON(<args>)` (step 4).
    fn json_value(&self, args: &[(usize, usize)], target: &Target, counts: &mut GoCounts) -> Value {
        let dynamic = Value {
            scope: None,
            entries: None,
        };
        let &[(s, e)] = args else {
            return dynamic;
        };
        let vb = self.view.as_bytes();
        let Some(pkg_end) = ident_end(vb, s).filter(|&k| &self.view[s..k] == "strings") else {
            return dynamic;
        };
        let Some((open, close)) = call_after(self.view, pkg_end, "NewReader") else {
            return dynamic;
        };
        if close + 1 != e {
            return dynamic;
        }
        let &[(is, ie)] = split_args(self.view, open, close).as_slice() else {
            return dynamic;
        };
        let text = &self.view[is..ie];
        if is_ident(text) {
            return Value {
                scope: Some(Scope::Abi(text.to_string())),
                entries: self
                    .decls
                    .iter()
                    .find(|d| d.name == text)
                    .map(|d| d.entries.clone()),
            };
        }
        if let Some(t) = literal(self.source, is, ie)
            && let Some(entries) = read_json_abi(&t)
        {
            counts.abis += 1;
            let name = match target {
                Target::Assign { text, .. } => text.rsplit('.').next().unwrap_or(text),
                Target::Field(name) => name,
            };
            return Value {
                scope: Some(Scope::Abi(name.to_string())),
                entries: Some(entries),
            };
        }
        dynamic
    }

    /// The value of `bind.NewBoundContract(<args>)` at `at` (step 4).
    fn bound_value(&self, args: &[(usize, usize)], at: usize) -> Value {
        let dynamic = Value {
            scope: None,
            entries: None,
        };
        let Some(&(s, e)) = args.get(1) else {
            return dynamic;
        };
        let vb = self.view.as_bytes();
        let s = if matches!(vb[s], b'*' | b'&') {
            skip_ws(vb, s + 1)
        } else {
            s
        };
        let Some(chain) = chain_before(self.view, e).filter(|c| c.start == s && c.rooted) else {
            return dynamic;
        };
        match self.binding_of(&chain, at) {
            Some(value) => value.clone(),
            None => Value {
                scope: Some(Scope::Abi(chain.last.to_string())),
                entries: None,
            },
        }
    }

    /// Record a construction's value under its target (steps 4 and 5).
    fn push_target(&mut self, target: Target, value: Value, close: usize) {
        match target {
            Target::Field(name) => self.fields.push(Field {
                name,
                value,
                at: close,
            }),
            Target::Assign { text, at, declared } => {
                let block = if declared {
                    enclosing_block(self.view, at)
                } else {
                    self.assign_block(&text, at)
                };
                if let Some((_, field)) = text.rsplit_once('.') {
                    self.fields.push(Field {
                        name: field.to_string(),
                        value: value.clone(),
                        at: close,
                    });
                }
                self.locals.push(Binding {
                    name: text,
                    value,
                    at: close,
                    block,
                });
            }
        }
    }

    /// Where an assignment `<text> = ..` at `at` is visible: the file for a
    /// package `var`, else its function's body (step 4).
    fn assign_block(&self, text: &str, at: usize) -> (usize, usize) {
        let whole = (0, self.view.len());
        if !text.contains('.') && self.package_vars.iter().any(|v| v == text) {
            return whole;
        }
        self.func_at(at).map_or(whole, |f| f.body)
    }

    /// The function whose body holds `at`.
    fn func_at(&self, at: usize) -> Option<&FuncDecl> {
        self.funcs.iter().find(|f| f.body.0 < at && at < f.body.1)
    }

    /// The nearest binding of `name` visible at `at` (step 4), by index.
    fn visible_local(&self, name: &str, at: usize) -> Option<usize> {
        let whole = (0, self.view.len());
        self.locals
            .iter()
            .enumerate()
            .filter(|(_, b)| {
                b.name == name
                    && (b.block == whole || b.at < at)
                    && b.block.0 <= at
                    && at <= b.block.1
                    && !self.shadowed(b, at)
            })
            .max_by_key(|(_, b)| b.at)
            .map(|(i, _)| i)
    }

    /// True when the function holding `at` names its receiver or a parameter
    /// like `b`'s root and `b` was declared outside it.
    fn shadowed(&self, b: &Binding, at: usize) -> bool {
        let Some(f) = self.func_at(at) else {
            return false;
        };
        let root = b.name.split('.').next().unwrap_or(&b.name);
        let outside = !(f.body.0 < b.at && b.at < f.body.1);
        outside && (f.receiver.as_deref() == Some(root) || f.params.iter().any(|p| p == root))
    }

    /// The value a receiver chain resolves to at `at` (step 6): its full
    /// text's binding, then its last segment's field.
    fn binding_of(&self, chain: &Chain<'_>, at: usize) -> Option<&Value> {
        if chain.rooted
            && let Some(i) = self.visible_local(chain.text, at)
        {
            return Some(&self.locals[i].value);
        }
        if !chain.rooted || chain.text.contains('.') {
            return self
                .fields
                .iter()
                .filter(|f| f.name == chain.last)
                .min_by_key(|f| f.at)
                .map(|f| &f.value);
        }
        None
    }

    /// The scope and entries a site on `chain` at `at` reads (step 6); `None`
    /// for a dynamic binding.
    fn scope_of(&self, chain: &Chain<'_>, at: usize) -> Option<(Scope, Option<Vec<AbiEntry>>)> {
        if let Some(value) = self.binding_of(chain, at) {
            return Some((value.scope.clone()?, value.entries.clone()));
        }
        Some(match self.receiver_type_abi(chain, at) {
            Some(decl) => (Scope::Abi(decl.name.clone()), Some(decl.entries.clone())),
            None => (Scope::Recv(chain.last.to_string()), None),
        })
    }

    /// Step 6's receiver-type rule: the ABI declaration an unbound `chain`
    /// at `at` reads when it is rooted at its method's receiver and that
    /// receiver's type is `<X>` or `<X>` plus an abigen suffix, with
    /// `<X>MetaData` (else `<X>ABI`) declared in the file.
    fn receiver_type_abi(&self, chain: &Chain<'_>, at: usize) -> Option<&AbiDecl> {
        let f = self.func_at(at)?;
        let root = chain.text.split('.').next()?;
        if !chain.rooted || f.receiver.as_deref() != Some(root) {
            return None;
        }
        let ty = f.receiver_type.as_deref()?;
        let bases = std::iter::once(ty).chain(
            ABIGEN_SUFFIXES
                .iter()
                .filter_map(|s| ty.strip_suffix(s).filter(|x| !x.is_empty())),
        );
        let decl = |name: String| self.decls.iter().find(|d| d.name == name);
        bases
            .into_iter()
            .find_map(|x| decl(format!("{x}MetaData")).or_else(|| decl(format!("{x}ABI"))))
    }

    /// Step 5: the fields the identifier bindings `locals[first..]` are handed
    /// to.
    fn hand_off(&mut self, first: usize) {
        let view = self.view;
        let vb = view.as_bytes();
        let mut names: Vec<String> = self.locals[first..]
            .iter()
            .filter(|b| is_ident(&b.name))
            .map(|b| b.name.clone())
            .collect();
        names.sort_unstable();
        names.dedup();
        let mut found = Vec::new();
        for name in names {
            for hit in word_hits(view, &name) {
                if hit > 0 && vb[hit - 1] == b'.' {
                    continue;
                }
                let Some(field) = handoff_field(view, hit, hit + name.len()) else {
                    continue;
                };
                let Some(i) = self.visible_local(&name, hit).filter(|&i| i >= first) else {
                    continue;
                };
                found.push(Field {
                    name: field,
                    value: self.locals[i].value.clone(),
                    at: hit,
                });
            }
        }
        self.fields.extend(found);
    }

    /// Step 7: every call site of the file.
    fn sites(&self, counts: &mut GoCounts) -> Vec<Site> {
        let (source, view) = (self.source, self.view);
        let vb = view.as_bytes();
        let mut sites = Vec::new();
        for &(word, needle) in NEEDLES {
            for m_at in word_hits(view, word) {
                let Some(dot) = prev_non_ws(vb, m_at).filter(|&d| vb[d] == b'.') else {
                    continue;
                };
                let Some(chain) = chain_before(view, dot) else {
                    continue;
                };
                let after = skip_ws(vb, m_at + word.len());
                let (name_at, args) = if needle.indexed() {
                    if vb.get(after) != Some(&b'[') {
                        continue;
                    }
                    let Some(close) = matching(view, after) else {
                        continue;
                    };
                    (trim(vb, after + 1, close), None)
                } else {
                    if vb.get(after) != Some(&b'(') {
                        continue;
                    }
                    let Some(close) = matching(view, after) else {
                        continue;
                    };
                    let pieces = split_args(view, after, close);
                    if needle == Needle::Call
                        && pieces
                            .get(1)
                            .is_some_and(|&(s, e)| literal(source, s, e).is_some())
                    {
                        // go-ethereum's rpc.Client: `.Call(<out>, "eth_..", ..)`.
                        continue;
                    }
                    let i = needle.name_arg();
                    let Some(&name_at) = pieces.get(i) else {
                        continue;
                    };
                    let args = if needle.decodes() || needle.event() {
                        None
                    } else {
                        arg_count(view, &pieces[i + 1..])
                    };
                    (name_at, args)
                };
                let Some(member) = literal(source, name_at.0, name_at.1) else {
                    if self
                        .binding_of(&chain, m_at)
                        .is_some_and(|v| v.scope.is_some())
                    {
                        counts.dynamic += 1;
                    }
                    continue;
                };
                if !is_ident(&member) || JSON_RPC.iter().any(|ns| member.starts_with(ns)) {
                    continue;
                }
                let Some((scope, entries)) = self.scope_of(&chain, m_at) else {
                    continue;
                };
                let lists = |kind: EntryKind| {
                    entries
                        .as_ref()
                        .is_some_and(|es| es.iter().any(|e| e.kind == kind && e.name == member))
                };
                let side = if needle.event() {
                    ClientSide::Event
                } else if !needle.decodes() || lists(EntryKind::Fn) {
                    ClientSide::Call
                } else if lists(EntryKind::Event) {
                    ClientSide::Event
                } else {
                    // A decoding form names a function or an event of the
                    // ABI it reads; no ABI, no site.
                    if entries.is_some() {
                        counts.not_in_abi += 1;
                    }
                    continue;
                };
                let kind = match side {
                    ClientSide::Call => EntryKind::Fn,
                    ClientSide::Event => EntryKind::Event,
                };
                if entries.is_some() && !lists(kind) {
                    counts.not_in_abi += 1;
                    continue;
                }
                sites.push(Site {
                    side,
                    scope,
                    member,
                    args,
                    lib: LIB,
                    abi: entries,
                    at: m_at,
                });
            }
        }
        sites.extend(self.switch_sites(counts));
        sites.sort_by(|a, b| (a.at, &a.member).cmp(&(b.at, &b.member)));
        sites
    }

    /// Step 8: the event sites of every `switch` on the key, or the value's
    /// `.Name`, of a `range <r>.Events` loop.
    fn switch_sites(&self, counts: &mut GoCounts) -> Vec<Site> {
        let (source, view) = (self.source, self.view);
        let vb = view.as_bytes();
        let mut sites = Vec::new();
        for e_at in word_hits(view, "Events") {
            let Some(dot) = prev_non_ws(vb, e_at).filter(|&d| vb[d] == b'.') else {
                continue;
            };
            let Some(chain) = chain_before(view, dot) else {
                continue;
            };
            let open = skip_ws(vb, e_at + "Events".len());
            if vb.get(open) != Some(&b'{') {
                continue;
            }
            let Some((key, value)) = range_vars(view, chain.start) else {
                continue;
            };
            let Some(close) = matching(view, open) else {
                continue;
            };
            let cases = switch_cases(view, (open, close), key.as_deref(), value.as_deref());
            if cases.is_empty() {
                continue;
            }
            let Some((scope, entries)) = self.scope_of(&chain, e_at) else {
                continue;
            };
            for (s, e) in cases {
                let Some(event) = literal(source, s, e).filter(|n| is_ident(n)) else {
                    continue;
                };
                if entries.as_ref().is_some_and(|es| {
                    !es.iter()
                        .any(|en| en.kind == EntryKind::Event && en.name == event)
                }) {
                    counts.not_in_abi += 1;
                    continue;
                }
                sites.push(Site {
                    side: ClientSide::Event,
                    scope: scope.clone(),
                    member: event,
                    args: None,
                    lib: LIB,
                    abi: entries.clone(),
                    at: s,
                });
            }
        }
        sites
    }
}

/// The key and value names of the `for <k>(, <v>)? :?= range` header whose
/// range expression starts at `at` (step 8); `_` names nothing. `None` when
/// no such header stands there.
fn range_vars(view: &str, at: usize) -> Option<(Option<String>, Option<String>)> {
    let vb = view.as_bytes();
    let r_end = prev_non_ws(vb, at)? + 1;
    let r_start = ident_start_before(vb, r_end).filter(|&s| &view[s..r_end] == "range")?;
    let eq = prev_non_ws(vb, r_start).filter(|&q| vb[q] == b'=')?;
    let lhs_end = if eq > 0 && vb[eq - 1] == b':' {
        eq - 1
    } else {
        eq
    };
    let start = vb[..lhs_end]
        .iter()
        .rposition(|c| matches!(c, b'\n' | b';' | b'{' | b'}'))
        .map_or(0, |k| k + 1);
    let lhs = view[start..lhs_end]
        .trim()
        .strip_prefix("for")
        .filter(|rest| rest.starts_with(char::is_whitespace))?;
    let names: Vec<&str> = lhs.split(',').map(str::trim).collect();
    if names.len() > 2 || !names.iter().all(|n| is_ident(n)) {
        return None;
    }
    let name = |i: usize| names.get(i).filter(|n| **n != "_").map(|n| n.to_string());
    Some((name(0), name(1)))
}

/// The case value ranges of every `switch <key> {` / `switch <value>.Name {`
/// inside the loop body `body` (its braces), each listed by a `case` of that
/// switch's own body (step 8).
fn switch_cases(
    view: &str,
    body: (usize, usize),
    key: Option<&str>,
    value: Option<&str>,
) -> Vec<(usize, usize)> {
    let vb = view.as_bytes();
    let tags: Vec<String> = key
        .map(str::to_string)
        .into_iter()
        .chain(value.map(|v| format!("{v}.Name")))
        .collect();
    let mut out = Vec::new();
    if tags.is_empty() {
        return out;
    }
    let cases = word_hits(view, "case");
    for at in word_hits(view, "switch") {
        if at <= body.0 || at >= body.1 {
            continue;
        }
        let from = at + "switch".len();
        let Some(brace) = vb[from..body.1].iter().position(|&c| c == b'{') else {
            continue;
        };
        let brace = from + brace;
        let tag = view[from..brace].trim();
        if !tags.iter().any(|t| t == tag) {
            continue;
        }
        let Some(s_close) = matching(view, brace) else {
            continue;
        };
        for kw in brace_depth0(vb, brace + 1, s_close, cases.clone()) {
            let list = kw + "case".len();
            if let Some(colon) = case_colon(vb, list, s_close) {
                out.extend(split_args(view, list - 1, colon));
            }
        }
    }
    out
}

/// The `:` closing a case list that starts at `from`, at bracket depth 0 and
/// before `limit`.
fn case_colon(vb: &[u8], from: usize, limit: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, &c) in vb.iter().enumerate().take(limit).skip(from) {
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            b':' if depth == 0 => return Some(k),
            _ => {}
        }
    }
    None
}

/// `.<member>(` after the identifier ending at `from`: the offsets of its
/// argument parentheses.
fn call_after(view: &str, from: usize, member: &str) -> Option<(usize, usize)> {
    let vb = view.as_bytes();
    let dot = skip_ws(vb, from);
    if vb.get(dot) != Some(&b'.') {
        return None;
    }
    let m_at = skip_ws(vb, dot + 1);
    let m_end = ident_end(vb, m_at)?;
    if &view[m_at..m_end] != member {
        return None;
    }
    let open = skip_ws(vb, m_end);
    if vb.get(open) != Some(&b'(') {
        return None;
    }
    Some((open, matching(view, open)?))
}

/// What the construction starting at `at` is bound to (step 4).
fn lhs_target(view: &str, at: usize) -> Option<Target> {
    let vb = view.as_bytes();
    let p = prev_non_ws(vb, at)?;
    match vb[p] {
        b':' => composite_field(view, p).map(Target::Field),
        b'=' => {
            let (eq, mut declared) = match p.checked_sub(1).map(|q| vb[q]) {
                Some(b':') => (p - 1, true),
                Some(c) if NOT_ASSIGN.contains(&c) => return None,
                _ => (p, false),
            };
            let start = vb[..eq]
                .iter()
                .rposition(|c| matches!(c, b'\n' | b';' | b'{' | b'}'))
                .map_or(0, |k| k + 1);
            let mut text = view[start..eq].trim();
            'strip: loop {
                for kw in LHS_KEYWORDS {
                    if let Some(rest) = text.strip_prefix(kw)
                        && rest.starts_with(char::is_whitespace)
                    {
                        declared |= *kw == "var";
                        text = rest.trim_start();
                        continue 'strip;
                    }
                }
                break;
            }
            let first = text.split(',').next()?.split_whitespace().next()?;
            let chain = first.split('.').all(is_ident);
            if !chain || first == "_" {
                return None;
            }
            // `first` is a slice of `view`, so its offset is the pointer gap.
            let first_at = first.as_ptr() as usize - view.as_ptr() as usize;
            Some(Target::Assign {
                text: first.to_string(),
                at: first_at,
                declared,
            })
        }
        _ => None,
    }
}

/// The field of a composite-literal element whose `:` is at `colon`
/// (`{<field>: ..` or `, <field>: ..`).
fn composite_field(view: &str, colon: usize) -> Option<String> {
    let vb = view.as_bytes();
    let f_end = prev_non_ws(vb, colon)? + 1;
    let f_start = ident_start_before(vb, f_end)?;
    let before = prev_non_ws(vb, f_start)?;
    matches!(vb[before], b'{' | b',').then(|| view[f_start..f_end].to_string())
}

/// The field a bound identifier at `[hit, end)` is handed to (step 5):
/// `<field>: (*|&)?<x>` closing its composite-literal element, or
/// `<recv>.<field> = (*|&)?<x>` as a whole statement.
fn handoff_field(view: &str, hit: usize, end: usize) -> Option<String> {
    let vb = view.as_bytes();
    let mut p = prev_non_ws(vb, hit)?;
    if matches!(vb[p], b'*' | b'&') {
        p = prev_non_ws(vb, p)?;
    }
    match vb[p] {
        b':' => {
            let next = skip_ws(vb, end);
            if !matches!(vb.get(next), Some(b',' | b'}')) {
                return None;
            }
            composite_field(view, p)
        }
        b'=' if p > 0 && !NOT_ASSIGN.contains(&vb[p - 1]) => {
            if !statement_ends(vb, end) {
                return None;
            }
            let lhs_end = prev_non_ws(vb, p)? + 1;
            let chain = chain_before(view, lhs_end)?;
            (chain.rooted && chain.text.contains('.') && statement_starts(vb, chain.start))
                .then(|| chain.last.to_string())
        }
        _ => None,
    }
}

/// The selector chain ending just before `end` (step 6); `None` when no
/// identifier ends there.
fn chain_before(view: &str, end: usize) -> Option<Chain<'_>> {
    let vb = view.as_bytes();
    let mut start = ident_start_before(vb, end)?;
    let last = &view[start..end];
    let rooted = loop {
        if start == 0 || vb[start - 1] != b'.' {
            break true;
        }
        match ident_start_before(vb, start - 1) {
            Some(s) => start = s,
            None => break false,
        }
    };
    Some(Chain {
        text: &view[start..end],
        last,
        start,
        rooted,
    })
}

/// The argument count of `rest`, the arguments after a site's name; `None`
/// when the last spreads a slice (`xs...`).
fn arg_count(view: &str, rest: &[(usize, usize)]) -> Option<u32> {
    if rest
        .last()
        .is_some_and(|&(s, e)| view[s..e].ends_with("..."))
    {
        return None;
    }
    u32::try_from(rest.len()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm::scan::code_view;

    /// `<call|event> <scope key>.<member> args=<n|-> abi=<keys|-> line=<l>`
    /// per site, in file order, and the counts.
    fn read(src: &str) -> (Vec<String>, GoCounts) {
        let (sites, counts) = scan(src, &code_view(src, "go"));
        let rows = sites
            .iter()
            .map(|s| {
                assert_eq!(s.lib, "geth");
                let side = match s.side {
                    ClientSide::Call => "call",
                    ClientSide::Event => {
                        assert_eq!(s.args, None, "an event passes no argument count");
                        "event"
                    }
                };
                let abi = s.abi.as_ref().map_or_else(
                    || "-".to_string(),
                    |e| e.iter().map(AbiEntry::to_key).collect::<Vec<_>>().join(","),
                );
                let args = s.args.map_or_else(|| "-".to_string(), |a| a.to_string());
                let line = src[..s.at].matches('\n').count() + 1;
                format!(
                    "{side} {}.{} args={args} abi={abi} line={line}",
                    s.scope.key(),
                    s.member
                )
            })
            .collect();
        (rows, counts)
    }

    fn counts(abis: usize, not_in_abi: usize, dynamic: usize) -> GoCounts {
        GoCounts {
            abis,
            not_in_abi,
            dynamic,
        }
    }

    const HEAD: &str = "package chain

import (
\t\"strings\"

\t\"github.com/ethereum/go-ethereum/accounts/abi\"
\t\"github.com/ethereum/go-ethereum/accounts/abi/bind\"
)

const escrowABI = `[
  {\"type\":\"function\",\"name\":\"deposit\",\"inputs\":[{\"name\":\"id\",\"type\":\"bytes32\"}]},
  {\"type\":\"function\",\"name\":\"release\",\"inputs\":[{\"name\":\"id\",\"type\":\"bytes32\"}]},
  {\"type\":\"event\",\"name\":\"Released\",\"inputs\":[{\"name\":\"id\",\"type\":\"bytes32\"}]}
]`
";

    /// HEAD's ABI as `read` prints it.
    const KEYS: &str = "f:deposit/1,f:release/1,e:Released/1";

    #[test]
    fn raw_and_interpreted_json_abi_consts() {
        let src = r#"package c

import "github.com/ethereum/go-ethereum/accounts/abi"

const rawABI = `[{"type":"function","name":"deposit","inputs":[{"name":"id","type":"bytes32"}]}]`

const (
	quotedABI = "[{\"type\":\"function\",\"name\":\"release\",\"inputs\":[{\"name\":\"id\",\"type\":\"bytes32\"}]},\n{\"type\":\"event\",\"name\":\"Released\",\"inputs\":[]}]"
	notABI    = "release"
	n, m      = 3, `[{"name":"grouped","inputs":[]}]`
)

var typedABI string = `[{"name":"ping","inputs":[]}]`

func local() {
	const localABI = `[{"name":"local","inputs":[]}]`
}
"#;
        let view = code_view(src, "go");
        let decls: Vec<(String, Vec<String>)> = abi_decls(src, &view, &package_specs(&view), None)
            .into_iter()
            .map(|d| (d.name, d.entries.iter().map(AbiEntry::to_key).collect()))
            .collect();
        let want = |name: &str, keys: &[&str]| {
            (
                name.to_string(),
                keys.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            decls,
            [
                want("rawABI", &["f:deposit/1"]),
                want("quotedABI", &["f:release/1", "e:Released/0"]),
                want("m", &["f:grouped/0"]),
                want("typedABI", &["f:ping/0"]),
            ],
            "package level only; an interpreted string is unescaped (\\u0065, \\n)"
        );
        assert_eq!(read(src), (Vec::new(), counts(4, 0, 0)));
    }

    #[test]
    fn abi_json_and_bound_contract_bindings() {
        let src = format!(
            "{}{}",
            HEAD.replace(
                "\t\"github.com/ethereum/go-ethereum/accounts/abi\"",
                "\tgabi \"github.com/ethereum/go-ethereum/accounts/abi\""
            ),
            r#"
func early(id [32]byte) ([]byte, error) { return pkgParsed.Pack("deposit", id) }

var pkgParsed, _ = gabi.JSON(strings.NewReader(escrowABI))

func a(addr common.Address, b bind.ContractBackend, paramABI gabi.ABI, file io.Reader) {
	parsed, err := gabi.JSON(strings.NewReader(escrowABI))
	bc := bind.NewBoundContract(addr, parsed, b, b, b)
	bc.Transact(opts, "release", id)
	var other *bind.BoundContract = bind.NewBoundContract(addr, &parsed, b, b, b)
	other.Call(nil, &out, "deposit", id)
	ext, _ := gabi.JSON(strings.NewReader(VaultABI))
	ext.Pack("withdraw", id)
	inl, _ := gabi.JSON(strings.NewReader(`[{"name":"ping","inputs":[]}]`))
	inl.Pack("ping")
	dyn, _ := gabi.JSON(file)
	dyn.Pack("deposit", id)
	pb := bind.NewBoundContract(addr, paramABI, b, b, b)
	pb.Transact(opts, "release", id)
}

func (s *Client) init() {
	s.abi, _ = gabi.JSON(strings.NewReader(escrowABI))
}

func (s *Client) data(id [32]byte) ([]byte, error) {
	return s.abi.Pack("release", id)
}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                format!("call abi:escrowABI.deposit args=1 abi={KEYS} line=16"),
                format!("call abi:escrowABI.release args=1 abi={KEYS} line=23"),
                format!("call abi:escrowABI.deposit args=1 abi={KEYS} line=25"),
                "call abi:VaultABI.withdraw args=1 abi=- line=27".to_string(),
                "call abi:inl.ping args=0 abi=f:ping/0 line=29".to_string(),
                "call abi:paramABI.release args=1 abi=- line=33".to_string(),
                format!("call abi:escrowABI.release args=1 abi={KEYS} line=41"),
            ]
        );
        assert_eq!(c, counts(2, 0, 1), "the const and the inline literal; dyn");
    }

    #[test]
    fn struct_field_hand_off() {
        // Kina backend/internal/indexer/indexer.go: `parsedABI: parsed`.
        let src = format!(
            "{HEAD}{}",
            r#"
type Indexer struct {
	parsedABI abi.ABI
}

func New(c *ethclient.Client) *Indexer {
	parsed, err := abi.JSON(strings.NewReader(escrowABI))
	if err != nil {
		return nil
	}
	return &Indexer{
		client:    c,
		parsedABI: parsed,
	}
}

func (idx *Indexer) data(id [32]byte) ([]byte, error) {
	m := idx.parsedABI.Methods["release"]
	_ = idx.parsedABI.Pack("withdraw", id)
	return idx.parsedABI.Pack("deposit", id)
}

func (s *Signer) setup(a common.Address, b bind.ContractBackend) {
	p, _ := abi.JSON(strings.NewReader(escrowABI))
	bc := bind.NewBoundContract(a, p, b, b, b)
	s.bound = bc
	s.other = bc.Address()
}

func (s *Signer) run(opts *bind.TransactOpts, id [32]byte) {
	s.bound.Transact(opts, "release", id)
	s.other.Transact(opts, "release", id)
}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                format!("call abi:escrowABI.release args=- abi={KEYS} line=32"),
                format!("call abi:escrowABI.deposit args=1 abi={KEYS} line=34"),
                format!("call abi:escrowABI.release args=1 abi={KEYS} line=45"),
                "call recv:other.release args=1 abi=- line=46".to_string(),
            ]
        );
        assert_eq!(c, counts(1, 1, 0), "withdraw is not in the ABI");
    }

    #[test]
    fn pointer_hand_off() {
        let src = format!(
            "{HEAD}{}",
            r#"
func NewEscrow() *Escrow {
	parsed, _ := abi.JSON(strings.NewReader(escrowABI))
	return &Escrow{abi: *parsed}
}

func (e *Escrow) Data(id [32]byte) ([]byte, error) { return e.abi.Pack("release", id) }

func wrap(id [32]byte) {
	parsed, _ := abi.JSON(strings.NewReader(escrowABI))
	w := &Wrap{ref: &parsed}
	w.ref.Pack("deposit", id)
	n := Note{text: parsed.String(), abi2: parsed.Methods}
	n.abi2.Pack("deposit", id)
}
"#
        );
        let (rows, _) = read(&src);
        assert_eq!(
            rows,
            [
                format!("call abi:escrowABI.release args=1 abi={KEYS} line=21"),
                format!("call abi:escrowABI.deposit args=1 abi={KEYS} line=26"),
                "call recv:abi2.deposit args=1 abi=- line=28".to_string(),
            ],
            "`abi2: parsed.Methods` hands nothing on"
        );
    }

    #[test]
    fn binding_is_block_scoped() {
        let src = format!(
            "{HEAD}{}",
            r#"
func a(ok bool, id [32]byte) {
	if ok {
		parsed, _ := abi.JSON(strings.NewReader(escrowABI))
		parsed.Pack("deposit", id)
	}
	parsed.Pack("release", id)
}

func b(parsed abi.ABI, id [32]byte) { parsed.Pack("deposit", id) }

func d(id [32]byte) { pkg.Pack("release", id) }

var pkg, _ = abi.JSON(strings.NewReader(escrowABI))

func c(pkg abi.ABI, id [32]byte) { pkg.Pack("deposit", id) }

func (pkg *Holder) e(id [32]byte) { pkg.Pack("deposit", id) }
"#
        );
        let (rows, _) = read(&src);
        assert_eq!(
            rows,
            [
                format!("call abi:escrowABI.deposit args=1 abi={KEYS} line=19"),
                "call recv:parsed.release args=1 abi=- line=21".to_string(),
                "call recv:parsed.deposit args=1 abi=- line=24".to_string(),
                format!("call abi:escrowABI.release args=1 abi={KEYS} line=26"),
                "call recv:pkg.deposit args=1 abi=- line=30".to_string(),
                "call recv:pkg.deposit args=1 abi=- line=32".to_string(),
            ],
            "a package binding is file-wide; a parameter or receiver of that name hides it"
        );
    }

    #[test]
    fn call_third_argument_vs_rpc_client_second_argument() {
        let src = format!(
            "{HEAD}{}",
            r#"
func f(c *bind.BoundContract, rc *rpc.Client, id [32]byte) {
	var out []interface{}
	c.Call(opts, &out, "deposits", id)
	c.Call(nil, &out, "getTrade")
	rc.Call(&out, "eth_getBalance", addr, "latest")
	rc.Call(&out, "custom_method", "x")
	rc.Call(&out)
	rc.CallContext(ctx, &out, "eth_chainId")
}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                "call recv:c.deposits args=1 abi=- line=18",
                "call recv:c.getTrade args=0 abi=- line=19",
            ]
        );
        assert_eq!(c, counts(1, 0, 0));
    }

    #[test]
    fn transact_needle_is_dot_preceded() {
        // Kina signer.go: NewKeyedTransactorWithChainID and RawTransact.
        let src = format!(
            "{HEAD}{}",
            r#"
func f(contract *bind.BoundContract, auth *bind.TransactOpts, data []byte) {
	auth, _ = bind.NewKeyedTransactorWithChainID(key, chainID)
	tx, _ := contract.RawTransact(auth, data)
	Transact(auth, "release")
	contract.TransactOpts("release")
	contract.Transact(auth, "release", id)
	getS().escrow.Transact(auth, "release", id)
	getS().Transact(auth, "release", id)
	x.(*bind.BoundContract).Transact(auth, "release", id)
	contract.
		Transact(auth, "deposit", id)
}
"#
        );
        let (rows, _) = read(&src);
        assert_eq!(
            rows,
            [
                "call recv:contract.release args=1 abi=- line=21",
                "call recv:escrow.release args=1 abi=- line=22",
                "call recv:contract.deposit args=1 abi=- line=26",
            ]
        );
    }

    #[test]
    fn json_rpc_namespaces_never_mint() {
        let src = format!(
            "{HEAD}{}",
            r#"
func f(c *bind.BoundContract, o *bind.TransactOpts) {
	c.Transact(o, "eth_sendTransaction")
	c.Call(o, &out, "net_version")
	c.Pack("web3_clientVersion")
	c.Methods["debug_traceTransaction"]
	c.Transact(o, "personal_sign")
	c.Transact(o, "txpool_content")
	c.Transact(o, "ethBalance")
	parsed, _ := abi.JSON(strings.NewReader(escrowABI))
	parsed.Pack("eth_call")
	parsed.Pack("")
	parsed.Pack("not a name")
}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(rows, ["call recv:c.ethBalance args=0 abi=- line=23"]);
        assert_eq!(c, counts(1, 0, 0), "a namespace is never counted");
    }

    #[test]
    fn unbound_receiver_is_recv_scope() {
        // examples-glia api/internal/chain/escrow.go.
        let src = "package chain

import (
\t\"context\"
\t\"math/big\"

\t\"github.com/ethereum/go-ethereum/accounts/abi/bind\"
\t\"github.com/ethereum/go-ethereum/common\"
\t\"github.com/ethereum/go-ethereum/ethclient\"
)

type Settler struct {
\tclient  *ethclient.Client
\tescrow  *bind.BoundContract
\taddress common.Address
}

func Dial(ctx context.Context, rpcURL string, address common.Address, contract *bind.BoundContract) (*Settler, error) {
\tclient, err := ethclient.DialContext(ctx, rpcURL)
\tif err != nil {
\t\treturn nil, err
\t}
\treturn &Settler{client: client, escrow: contract, address: address}, nil
}

func (s *Settler) Release(opts *bind.TransactOpts, orderID [32]byte) error {
\t_, err := s.escrow.Transact(opts, \"release\", orderID)
\tm := s.escrow.Methods[\"release\"]
\ts.escrow.Unpack(\"release\", data)
\treturn err
}

func (s *Settler) Balance(ctx context.Context) (*big.Int, error) {
\treturn s.client.BalanceAt(ctx, s.address, nil)
}
";
        let (rows, c) = read(src);
        assert_eq!(
            rows,
            [
                "call recv:escrow.release args=1 abi=- line=27",
                "call recv:escrow.release args=- abi=- line=28",
            ],
            "an Unpack without an ABI is no site"
        );
        assert_eq!(c, GoCounts::default());
    }

    #[test]
    fn non_literal_name_is_dynamic() {
        // Kina backend/internal/signer/signer.go submitTo.
        let src = format!(
            "{HEAD}{}",
            r#"
func (s *PlatformSigner) submitTo(ctx context.Context, chainName, contractAddr, method string, args ...any) (string, error) {
	parsedABI, err := abi.JSON(strings.NewReader(escrowABI))
	if err != nil {
		return "", fmt.Errorf("parse abi: %w", err)
	}
	data, err := parsedABI.Pack(method, args...)
	contract := bind.NewBoundContract(common.HexToAddress(contractAddr), parsedABI, client, client, client)
	tx, err := contract.RawTransact(auth, data)
	s.escrow.Transact(opts, method, data)
	return tx.Hash().Hex(), nil
}
"#
        );
        assert_eq!(
            read(&src),
            (Vec::new(), counts(1, 0, 1)),
            "an unbound receiver's non-literal name is not counted"
        );
    }

    #[test]
    fn unpack_reads_abi_functions_and_events() {
        let src = format!(
            "{HEAD}{}",
            r#"
func f(data []byte) {
	parsed, _ := abi.JSON(strings.NewReader(escrowABI))
	parsed.Unpack("deposit", data)
	parsed.UnpackIntoInterface(&out, "release", data)
	parsed.Unpack("Released", data)
	parsed.UnpackIntoInterface(&ev, "Released", data)
	parsed.Unpack("gone", data)
}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                format!("call abi:escrowABI.deposit args=- abi={KEYS} line=18"),
                format!("call abi:escrowABI.release args=- abi={KEYS} line=19"),
                format!("event abi:escrowABI.Released args=- abi={KEYS} line=20"),
                format!("event abi:escrowABI.Released args=- abi={KEYS} line=21"),
            ],
            "a decoding form of an ABI event is an event site (DT.10k)"
        );
        assert_eq!(c, counts(1, 1, 0), "gone is in neither list");
    }

    #[test]
    fn ungated_file_mints_nothing() {
        let body = r#"
const escrowABI = `[{"name":"release","inputs":[]}]`

func f(c *Contract) {
	parsed, _ := abi.JSON(strings.NewReader(escrowABI))
	parsed.Pack("release")
	c.Transact(opts, "release")
}
"#;
        for imports in [
            "import \"example.com/abi\"\n",
            "import \"github.com/ethereum/go-ethereumx/accounts/abi\"\n",
            "// import \"github.com/ethereum/go-ethereum/accounts/abi\"\nvar s = \"github.com/ethereum/go-ethereum\"\n",
        ] {
            let src = format!("package c\n\n{imports}{body}");
            assert_eq!(read(&src), (Vec::new(), GoCounts::default()), "{imports}");
        }
        let single =
            format!("package c\n\nimport \"github.com/ethereum/go-ethereum/accounts/abi\"\n{body}");
        assert_eq!(read(&single).0.len(), 2, "a single import gates");
    }

    #[test]
    fn spread_args_are_unknown() {
        let src = format!(
            "{HEAD}{}",
            r#"
func f(c *bind.BoundContract, o *bind.TransactOpts, args []any) {
	c.Transact(o, "release", args...)
	c.Pack("deposit", a, b...)
	c.Transact(o, "release")
	c.Call(o, &out, "deposits", ids ...)
}
"#
        );
        let (rows, _) = read(&src);
        assert_eq!(
            rows,
            [
                "call recv:c.release args=- abi=- line=17",
                "call recv:c.deposit args=- abi=- line=18",
                "call recv:c.release args=0 abi=- line=19",
                "call recv:c.deposits args=- abi=- line=20",
            ]
        );
    }

    #[test]
    fn func_decls_read_receivers_params_and_bodies() {
        let src = "package c

var handler = func(w int) {}

func (s *PlatformSigner) submitTo(ctx context.Context, chainName, contractAddr, method string, args ...any) (string, error) {
\treturn \"\", nil
}

func Map[K comparable, V any](m map[K]V, f func(K) V) map[K]V { return m }

func (*Anon) unnamed(context.Context, string) struct{ a int } {
\treturn struct{ a int }{}
}

func asm(x int) int

func chans(c chan int, d <-chan int) {}

func (_Escrow *EscrowCaller) Deposits(opts *bind.CallOpts) {}

func (c Box[T]) Len() {}
";
        let view = code_view(src, "go");
        let funcs = func_decls(&view);
        // (receiver, receiver type, parameters, body)
        type Decl<'a> = (Option<&'a str>, Option<&'a str>, Vec<&'a str>, &'a str);
        let decls: Vec<Decl<'_>> = funcs
            .iter()
            .map(|f| {
                (
                    f.receiver.as_deref(),
                    f.receiver_type.as_deref(),
                    f.params.iter().map(String::as_str).collect(),
                    &src[f.body.0..=f.body.1],
                )
            })
            .collect();
        assert_eq!(decls.len(), 6, "{decls:?}");
        assert_eq!(
            decls[0],
            (
                Some("s"),
                Some("PlatformSigner"),
                vec!["ctx", "chainName", "contractAddr", "method", "args"],
                "{\n\treturn \"\", nil\n}"
            )
        );
        assert_eq!(decls[1], (None, None, vec!["m", "f"], "{ return m }"));
        assert_eq!(
            decls[2],
            (
                None,
                Some("Anon"),
                vec![],
                "{\n\treturn struct{ a int }{}\n}"
            ),
            "unnamed parameters name nothing; a struct result is stepped over"
        );
        assert_eq!(decls[3], (None, None, vec!["c", "d"], "{}"));
        assert_eq!(
            decls[4],
            (Some("_Escrow"), Some("EscrowCaller"), vec!["opts"], "{}")
        );
        assert_eq!(
            decls[5],
            (Some("c"), Some("Box"), vec![], "{}"),
            "type parameters are dropped"
        );
    }

    #[test]
    fn events_index_filter_watch_unpack_log() {
        let src = format!(
            "{HEAD}{}",
            r#"
func f(c *bind.BoundContract, client *ethclient.Client, q ethereum.FilterQuery, name string) {
	parsed, _ := abi.JSON(strings.NewReader(escrowABI))
	id := parsed.Events["Released"].ID
	_ = parsed.Events["Gone"]
	logs, sub, _ := c.FilterLogs(fopts, "Released", rule)
	c.WatchLogs(wopts, "Deposited", rule)
	c.UnpackLog(&ev, "Released", log)
	c.UnpackLogIntoMap(m, "Released", log)
	client.FilterLogs(ctx, q)
	_ = parsed.Events[name]
	bc := bind.NewBoundContract(addr, parsed, b, b, b)
	bc.WatchLogs(wopts, "Released")
	bc.FilterLogs(fopts, "deposit")
	bc.Transact(opts, "Released")
}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                format!("event abi:escrowABI.Released args=- abi={KEYS} line=18"),
                "event recv:c.Released args=- abi=- line=20".to_string(),
                "event recv:c.Deposited args=- abi=- line=21".to_string(),
                "event recv:c.Released args=- abi=- line=22".to_string(),
                "event recv:c.Released args=- abi=- line=23".to_string(),
                format!("event abi:escrowABI.Released args=- abi={KEYS} line=27"),
            ],
            "ethclient's FilterLogs(ctx, q) has no name literal"
        );
        assert_eq!(
            c,
            counts(1, 3, 1),
            "Gone, the function deposit as an event and the event Released as a call \
             are not in the ABI; Events[name] on a bound receiver is dynamic"
        );
    }

    #[test]
    fn events_switch_over_range_keys_and_value_name() {
        let src = format!(
            "{HEAD}{}",
            r#"
type Indexer struct {
	parsedABI abi.ABI
}

func New() *Indexer {
	parsed, _ := abi.JSON(strings.NewReader(escrowABI))
	return &Indexer{parsedABI: parsed}
}

func (idx *Indexer) byKey(topic common.Hash, kind string) {
	for name, event := range idx.parsedABI.Events {
		if event.ID != topic {
			continue
		}
		switch name {
		case "Released", "Gone":
			switch kind {
			case "Deposited":
			}
		case "deposit":
		default:
		}
		switch kind {
		case "Released":
		}
	}
}

func (idx *Indexer) byName(c *bind.BoundContract, name string) {
	for _, ev := range idx.parsedABI.Events {
		switch ev.Name {
		case "Released":
		}
		switch ev.ID {
		case "Released":
		}
	}
	for k := range c.Events {
		switch k {
		case "TradeCancelled", "TimeoutClaimed":
		case cancelledName:
		}
	}
	for _, v := range c.Methods {
		switch v.Name {
		case "release":
		}
	}
	switch name {
	case "Released":
	}
}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                format!("event abi:escrowABI.Released args=- abi={KEYS} line=31"),
                format!("event abi:escrowABI.Released args=- abi={KEYS} line=47"),
                "event recv:c.TradeCancelled args=- abi=- line=55".to_string(),
                "event recv:c.TimeoutClaimed args=- abi=- line=55".to_string(),
            ],
            "a nested or unrelated switch, a range over Methods and a switch outside \
             the loop contribute nothing"
        );
        assert_eq!(c, counts(1, 2, 0), "Gone and the function deposit");
    }

    /// An abigen v1 binding's metadata ABI: deposit/1, deposits/1, the event
    /// Released/1.
    const META_ABI: &str = r#""[{\"type\":\"function\",\"name\":\"deposit\",\"inputs\":[{\"name\":\"orderId\",\"type\":\"bytes32\"}]},{\"type\":\"function\",\"name\":\"deposits\",\"inputs\":[{\"name\":\"\",\"type\":\"bytes32\"}]},{\"type\":\"event\",\"name\":\"Released\",\"inputs\":[{\"name\":\"orderId\",\"type\":\"bytes32\"}]}]""#;

    /// META_ABI as `read` prints it.
    const META_KEYS: &str = "f:deposit/1,f:deposits/1,e:Released/1";

    #[test]
    fn abigen_v1_metadata_and_receiver_type_scope() {
        let src = format!(
            r#"// Code generated - DO NOT EDIT.
package escrowbind

import (
	"github.com/ethereum/go-ethereum/accounts/abi/bind"
)

var EscrowMetaData = &bind.MetaData{{
	ABI: {META_ABI},
	Bin: "0x6080",
}}

var EscrowABI = EscrowMetaData.ABI

func bindEscrow(address common.Address, caller bind.ContractCaller, transactor bind.ContractTransactor, filterer bind.ContractFilterer) (*bind.BoundContract, error) {{
	parsed, err := EscrowMetaData.GetAbi()
	if err != nil {{
		return nil, err
	}}
	return bind.NewBoundContract(address, *parsed, caller, transactor, filterer), nil
}}

func (_Escrow *EscrowRaw) Call(opts *bind.CallOpts, result *[]interface{{}}, method string, params ...interface{{}}) error {{
	return _Escrow.Contract.EscrowCaller.contract.Call(opts, result, method, params...)
}}

func (_Escrow *EscrowCaller) Deposits(opts *bind.CallOpts, arg0 [32]byte) (*big.Int, error) {{
	var out []interface{{}}
	err := _Escrow.contract.Call(opts, &out, "deposits", arg0)
	return out[0].(*big.Int), err
}}

func (_Escrow *EscrowCallerSession) Deposits(arg0 [32]byte) (*big.Int, error) {{
	return _Escrow.Contract.Deposits(&_Escrow.CallOpts, arg0)
}}

func (_Escrow *EscrowTransactor) Deposit(opts *bind.TransactOpts, orderId [32]byte) (*types.Transaction, error) {{
	return _Escrow.contract.Transact(opts, "deposit", orderId)
}}

func (_Escrow *EscrowTransactor) Stray(opts *bind.TransactOpts) (*types.Transaction, error) {{
	return _Escrow.contract.Transact(opts, "withdraw")
}}

func (_Escrow *EscrowFilterer) WatchReleased(opts *bind.WatchOpts, sink chan<- *EscrowReleased) (event.Subscription, error) {{
	logs, sub, err := _Escrow.contract.WatchLogs(opts, "Released")
	return event.NewSubscription(func(quit <-chan struct{{}}) error {{
		for {{
			select {{
			case log := <-logs:
				ev := new(EscrowReleased)
				if err := _Escrow.contract.UnpackLog(ev, "Released", log); err != nil {{
					return err
				}}
			}}
		}}
	}}), nil
}}

func (it *EscrowReleasedIterator) Next() bool {{
	return it.contract.UnpackLog(it.Event, it.event, it.log) == nil
}}

func Data(id [32]byte) []byte {{
	parsed, _ := EscrowMetaData.GetAbi()
	b, _ := parsed.Pack("deposit", id)
	return b
}}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                format!("call abi:EscrowMetaData.deposits args=1 abi={META_KEYS} line=29"),
                format!("call abi:EscrowMetaData.deposit args=1 abi={META_KEYS} line=38"),
                format!("event abi:EscrowMetaData.Released args=- abi={META_KEYS} line=46"),
                format!("event abi:EscrowMetaData.Released args=- abi={META_KEYS} line=52"),
                format!("call abi:EscrowMetaData.deposit args=1 abi={META_KEYS} line=66"),
            ],
            "the iterator's UnpackLog names no literal; Raw's forwarding Call is no site"
        );
        assert_eq!(
            c,
            counts(1, 1, 0),
            "EscrowMetaData (EscrowABI = EscrowMetaData.ABI is no literal); withdraw"
        );
    }

    #[test]
    fn abigen_v1_abi_const() {
        let src = r#"// Code generated - DO NOT EDIT.
package escrowbind

import (
	"strings"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/accounts/abi/bind"
)

const EscrowABI = "[{\"type\":\"function\",\"name\":\"release\",\"inputs\":[{\"name\":\"orderId\",\"type\":\"bytes32\"}]}]"

func bindEscrow(address common.Address, caller bind.ContractCaller, transactor bind.ContractTransactor, filterer bind.ContractFilterer) (*bind.BoundContract, error) {
	parsed, err := abi.JSON(strings.NewReader(EscrowABI))
	if err != nil {
		return nil, err
	}
	return bind.NewBoundContract(address, parsed, caller, transactor, filterer), nil
}

func (_Escrow *EscrowTransactor) Release(opts *bind.TransactOpts, orderId [32]byte) (*types.Transaction, error) {
	return _Escrow.contract.Transact(opts, "release", orderId)
}

func (_Escrow *EscrowTransactorSession) Release(orderId [32]byte) (*types.Transaction, error) {
	return _Escrow.Contract.contract.Transact(&_Escrow.TransactOpts, "release", orderId)
}

func (_Settle *SettleTransactor) Release(opts *bind.TransactOpts, orderId [32]byte) (*types.Transaction, error) {
	return _Settle.contract.Transact(opts, "release", orderId)
}
"#;
        let (rows, c) = read(src);
        assert_eq!(
            rows,
            [
                "call abi:EscrowABI.release args=1 abi=f:release/1 line=22",
                "call abi:EscrowABI.release args=1 abi=f:release/1 line=26",
                "call recv:contract.release args=1 abi=- line=30",
            ],
            "the longest suffix first; Settle declares no SettleMetaData / SettleABI"
        );
        assert_eq!(c, counts(1, 0, 0));
    }

    #[test]
    fn abigen_v2_parse_abi_and_pointer_field() {
        let src = format!(
            r#"// Code generated via abigen V2 - DO NOT EDIT.
package escrow

import (
	"errors"

	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/accounts/abi/bind/v2"
)

var EscrowMetaData = bind.MetaData{{
	ABI: {META_ABI},
	ID:  "Escrow",
}}

type Escrow struct {{
	abi abi.ABI
}}

func NewEscrow() *Escrow {{
	parsed, err := EscrowMetaData.ParseABI()
	if err != nil {{
		panic(errors.New("invalid ABI: " + err.Error()))
	}}
	return &Escrow{{abi: *parsed}}
}}

func (c *Escrow) Instance(backend bind.ContractBackend, addr common.Address) *bind.BoundContract {{
	return bind.NewBoundContract(addr, c.abi, backend, backend, backend)
}}

func (escrow *Escrow) PackDeposit(orderId [32]byte) []byte {{
	enc, err := escrow.abi.Pack("deposit", orderId)
	if err != nil {{
		panic(err)
	}}
	return enc
}}

func (c *Escrow) TryPackDeposit(orderId [32]byte) ([]byte, error) {{
	return c.abi.Pack("deposit", orderId)
}}

func (escrow *Escrow) UnpackDeposits(data []byte) (*big.Int, error) {{
	out, err := escrow.abi.Unpack("deposits", data)
	return out[0].(*big.Int), err
}}

func (escrow *Escrow) UnpackReleasedEvent(log *types.Log) (*EscrowReleased, error) {{
	event := "Released"
	if log.Topics[0] != escrow.abi.Events[event].ID {{
		return nil, errors.New("event signature mismatch")
	}}
	out := new(EscrowReleased)
	if err := escrow.abi.UnpackIntoInterface(out, event, log.Data); err != nil {{
		return nil, err
	}}
	return out, nil
}}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                format!("call abi:EscrowMetaData.deposit args=1 abi={META_KEYS} line=33"),
                format!("call abi:EscrowMetaData.deposit args=1 abi={META_KEYS} line=41"),
                format!("call abi:EscrowMetaData.deposits args=- abi={META_KEYS} line=45"),
            ]
        );
        assert_eq!(
            c,
            counts(1, 0, 2),
            "the event unpacker names Released through a local: Events[event] and \
             UnpackIntoInterface(out, event, ..) on the bound field are dynamic"
        );
    }

    #[test]
    fn receiver_type_without_metadata_is_recv_scope() {
        let src = r#"package chain

import "github.com/ethereum/go-ethereum/accounts/abi/bind"

const VaultABI = `[{"type":"function","name":"withdraw","inputs":[]}]`

type EscrowCaller struct {
	contract *bind.BoundContract
}

func (_Escrow *EscrowCaller) Deposits(opts *bind.CallOpts, id [32]byte) error {
	return _Escrow.contract.Call(opts, &out, "deposits", id)
}

func (v *VaultCaller) Get(opts *bind.CallOpts, other *bind.BoundContract) error {
	other.Call(opts, &out, "withdraw")
	return v.contract.Call(opts, &out, "withdraw")
}

func Free(contract *bind.BoundContract, opts *bind.CallOpts) error {
	return contract.Call(opts, &out, "withdraw")
}
"#;
        let (rows, c) = read(src);
        assert_eq!(
            rows,
            [
                "call recv:contract.deposits args=1 abi=- line=12",
                "call recv:other.withdraw args=0 abi=- line=16",
                "call abi:VaultABI.withdraw args=0 abi=f:withdraw/0 line=17",
                "call recv:contract.withdraw args=0 abi=- line=21",
            ],
            "no EscrowMetaData / EscrowABI; a chain not rooted at the receiver; a function"
        );
        assert_eq!(c, counts(1, 0, 0));
    }

    #[test]
    fn kina_indexer_shape() {
        // Kina backend/internal/indexer/indexer.go processLog, its 13 events.
        let events = [
            "TradeCreated",
            "TradeFunded",
            "PaymentConfirmed",
            "FundsReleased",
            "DisputeRaised",
            "DisputeResolved",
            "TradeCancelled",
            "TimeoutClaimed",
            "ResolutionProposed",
            "ResolutionAccepted",
            "TradeFrozen",
            "TradeUnfrozen",
            "FundsSeized",
        ];
        let abi = events
            .iter()
            .map(|e| {
                format!(
                    r#"  {{"anonymous":false,"inputs":[{{"indexed":true,"name":"tradeId","type":"uint256"}}],"name":"{e}","type":"event"}}"#
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");
        let src = format!(
            r#"package indexer

import (
	"context"
	"strings"

	"github.com/ethereum/go-ethereum"
	"github.com/ethereum/go-ethereum/accounts/abi"
	"github.com/ethereum/go-ethereum/core/types"
	"github.com/ethereum/go-ethereum/ethclient"
)

const escrowABI = `[
{abi}
]`

type Indexer struct {{
	client    *ethclient.Client
	parsedABI abi.ABI
}}

func New(client *ethclient.Client) (*Indexer, error) {{
	parsed, err := abi.JSON(strings.NewReader(escrowABI))
	if err != nil {{
		return nil, err
	}}
	return &Indexer{{
		client:    client,
		parsedABI: parsed,
	}}, nil
}}

func (idx *Indexer) poll(ctx context.Context, query ethereum.FilterQuery) error {{
	logs, err := idx.client.FilterLogs(ctx, query)
	for _, vlog := range logs {{
		idx.processLog(ctx, vlog)
	}}
	return err
}}

func (idx *Indexer) processLog(ctx context.Context, vlog types.Log) error {{
	for name, event := range idx.parsedABI.Events {{
		if event.ID != vlog.Topics[0] {{
			continue
		}}
		switch name {{
		case "TradeCreated":
			return nil
		case "TradeFunded":
			return nil
		case "PaymentConfirmed":
			return nil
		case "FundsReleased":
			return nil
		case "DisputeRaised":
			return nil
		case "DisputeResolved":
			return nil
		case "TradeCancelled", "TimeoutClaimed":
			return nil
		case "ResolutionProposed":
			return nil
		case "ResolutionAccepted":
			if vals, derr := idx.parsedABI.Events["ResolutionAccepted"].Inputs.NonIndexed().Unpack(vlog.Data); derr == nil && len(vals) > 0 {{
				return nil
			}}
			return nil
		case "TradeFrozen", "TradeUnfrozen":
			log.Printf("%s", name)
			return nil
		case "FundsSeized":
			return nil
		}}
	}}
	return nil
}}
"#
        );
        let out =
            crate::evm::extract_evm_clients(&src, "go", glia_core::NodeId(1), glia_core::RepoId(1));
        assert_eq!(
            out.markers("backend/internal/indexer/indexer.go"),
            [
                "[evm-client] lang=go calls=0 events=13 sites=14 (ethers=0 hardhat=0 typechain=0 viem=0 web3=0 geth=14) abis=1 not_in_abi=0 dynamic=0 path=backend/internal/indexer/indexer.go"
            ]
        );
        let mut qnames: Vec<&str> = out.nav.qname_by_id.values().map(String::as_str).collect();
        qnames.sort_unstable();
        let mut want: Vec<String> = events
            .iter()
            .map(|e| format!("evm_event:abi:escrowABI.{e}"))
            .collect();
        want.sort_unstable();
        assert_eq!(qnames, want);
    }
}
