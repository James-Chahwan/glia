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
//!    names and types, and balanced body. Parameters split at depth-0 commas, so the
//!    grouped `chainName, contractAddr, method string` names every one, and a
//!    variadic `args ...any` is the last.
//! 4. **Bindings.** The first left-hand element of `<x>(, <err>)? := ..`,
//!    `var <x>(, <err>)? = ..` or `<x>(, <err>)? = ..`, or the field of a
//!    composite literal `<field>: ..`, binds the value of
//!    - `<N>.GetAbi()` / `<N>.ParseABI()` on a metadata declaration N of the
//!      file (abigen v1 / v2): `Scope::Abi(N)` with N's entries;
//!    - `abi.JSON(strings.NewReader(<N>))`: `Scope::Abi(N)` with N's entries;
//!      when the file does not declare N (and its function binds no
//!      parameter or local N, and N is no package `var` of the file) N is a
//!      constant of another file of the package: no entries, and every site
//!      of the binding carries the reference `AbiRef { name: N, from: "." }`
//!      (DT.10g), which the engine's join fills from that file's
//!      `NavFact::GoStringConst` ([`read_go_const_abi`]); unjoined, the
//!      resolver name-hints it. An inline literal that reads gives
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
//!    counted. A non-literal name mints nothing: a bare parameter of its
//!    function hands the name over (step 9); any other on a bound receiver
//!    counts `dynamic`, and on an unbound one nothing, so abigen's
//!    forwarding `Raw` methods (`_Escrow.Contract.EscrowCaller.contract.Call(
//!    opts, result, method, ..)`, named like a needle) count nothing.
//! 8. **Event switches** ([`Reader::switch_sites`]). A `for <k>(, <v>)? :?=
//!    range <r>.Events {` loop, and inside its block a `switch <k> {` or
//!    `switch <v>.Name {`: every string literal of a `case "A"(, "B")*:` list
//!    of that switch's own body (a nested switch's cases are deeper) is an
//!    event site of r's scope, read at the literal (Kina indexer.go
//!    processLog). A switch on any other tag contributes nothing.
//! 9. **Method-name wrappers** (DT.10f, [`Reader::infer_wrappers`]; the CA.4
//!    rule, read inside one file). A function whose `string` parameter is
//!    handed bare to a step-7 name position is a wrapper at that parameter's index
//!    (DIRECT), with the site's scope and entries (a `Scope::Recv` one
//!    keeps it, so the resolver name-hints its sites) and side: Pack /
//!    Methods / Transact / Call a call, Events / FilterLogs / WatchLogs /
//!    UnpackLog / UnpackLogIntoMap an event, Unpack / UnpackIntoInterface
//!    whichever the ABI lists at each site. A function handing its own
//!    parameter bare to a wrapper's index is a wrapper at that index too
//!    (FORWARDING), with the wrapper's scope and side, to depth
//!    [`MAX_DEPTH`] and never back into a wrapper already read. Kina
//!    signer.go: `submitTo(.., method string, ..)` packs `method`, `submit`
//!    forwards it, eight methods call `s.submit(ctx, chain, "freeze", ..)`.
//!    - A method wrapper is called `<x>.W(`, a function wrapper `W(`; its
//!      definition is no call. Every call whose argument at the index is ONE
//!      string literal is a site at the call: the wrapper's scope, entries
//!      and side, args unknown, owned by the calling function. A bare
//!      parameter of the caller forwards (never dynamic); anything else (a
//!      local, a concatenation, a call) is no site and counts `dynamic`.
//!    - PRECISION: no value is tracked through an assignment. A name that
//!      two functions of the file declare (two receiver types), or whose
//!      parameter reaches two scopes or both sides, is no wrapper and counts
//!      `ambiguous`; its geth sites fall back to the step-7 count. A
//!      function named like a needle ([`GETH_METHODS`]) is never a wrapper.
//!      Sites are same-file only.
//!
//!    Marker, once per file with a wrapper: `[evm-wrappers] wrappers=<w>
//!    (direct=<d> forwarded=<f> ambiguous=<a>) sites=<s> dynamic=<y>
//!    path=<file>`, one count per (function, parameter), w = d + f + a; s
//!    the wrapper sites minted, y the dynamic wrapper calls (also in the
//!    `[evm-client]` line's `dynamic`). Grep `^\[evm-wrappers\] `.

use std::collections::{BTreeMap, BTreeSet};

use glia_code_domain::evm::{self, AbiEntry, AbiRef, ClientSide, EntryKind, Scope};

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
    /// Bindings whose ABI argument could not be read, non-literal names on
    /// bound receivers that hand no parameter over, and the dynamic wrapper
    /// calls (step 9).
    pub(super) dynamic: usize,
    /// The `[evm-wrappers]` marker's counts (step 9).
    pub(super) wrappers: WrapperCounts,
}

/// The `[evm-wrappers]` marker's counts for one file (step 9).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct WrapperCounts {
    /// Wrappers whose parameter reaches a geth name position.
    pub(super) direct: usize,
    /// Wrappers whose parameter reaches another wrapper's.
    pub(super) forwarded: usize,
    /// Wrappers left out: a name two functions declare, or a parameter
    /// reaching two scopes or both sides.
    pub(super) ambiguous: usize,
    /// Sites minted at wrapper calls.
    pub(super) sites: usize,
    /// Wrapper calls whose name argument is neither one string literal nor
    /// a parameter of the caller.
    pub(super) dynamic: usize,
}

impl WrapperCounts {
    /// The `[evm-wrappers]` line of the file at `path`; `None` without a
    /// wrapper.
    pub(super) fn marker(&self, path: &str) -> Option<String> {
        let w = self.direct + self.forwarded + self.ambiguous;
        (w > 0).then(|| {
            format!(
                "[evm-wrappers] wrappers={w} (direct={} forwarded={} ambiguous={}) sites={} dynamic={} path={path}",
                self.direct, self.forwarded, self.ambiguous, self.sites, self.dynamic
            )
        })
    }
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
    /// The function or method name.
    name: String,
    /// The name's offset: the definition, never a call (step 9).
    name_at: usize,
    /// The method receiver's name; `None` for a function or an unnamed
    /// receiver.
    receiver: Option<String>,
    /// The method receiver's type name, `*` and type parameters dropped
    /// (`(_Escrow *EscrowCaller)` is `EscrowCaller`); `None` for a function.
    receiver_type: Option<String>,
    /// The parameter names in order; empty when the parameters are unnamed.
    params: Vec<String>,
    /// Each parameter's type text, a grouped name taking its group's
    /// (`chainName, method string` are both `string`).
    param_types: Vec<String>,
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

    /// The side the needle's name position reads.
    fn name_side(self) -> NameSide {
        if self.event() {
            NameSide::Event
        } else if self.decodes() {
            NameSide::Decode
        } else {
            NameSide::Call
        }
    }
}

/// What a name position reads (steps 7 and 9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NameSide {
    /// A function: a call site.
    Call,
    /// An event: an event site.
    Event,
    /// The decoding forms: a call where the ABI lists the name as a
    /// function, an event where it lists an event, else no site.
    Decode,
}

/// How deep a forwarding chain is read (step 9, CA.4's MAX_DEPTH): a direct
/// wrapper is depth 1, a function forwarding into it depth 2.
const MAX_DEPTH: usize = 3;

/// go-ethereum's own name-taking members: a function named like one is never
/// a wrapper (step 9). abigen's Raw bindings forward these, and their own
/// sites are read directly.
const GETH_METHODS: &[&str] = &[
    "Pack",
    "Unpack",
    "UnpackIntoInterface",
    "Transact",
    "Call",
    "FilterLogs",
    "WatchLogs",
    "UnpackLog",
    "UnpackLogIntoMap",
];

/// A parameter handed bare to a name position (step 9): a geth site's
/// (direct) or a wrapper call's (forwarded).
#[derive(Debug, Clone)]
struct Handover {
    /// The function whose parameter it is: an index into `Reader::funcs`.
    func: usize,
    /// The parameter's index.
    arg: usize,
    scope: Scope,
    entries: Option<Vec<AbiEntry>>,
    side: NameSide,
    /// A geth site on a bound receiver: step 7's `dynamic` when no wrapper
    /// forms.
    dynamic: bool,
}

/// One method-name wrapper (step 9).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Wrapper {
    name: String,
    /// A method, called `<x>.W(`; else a function, called `W(`.
    method: bool,
    /// The index of the parameter that names the member.
    arg: usize,
    scope: Scope,
    entries: Option<Vec<AbiEntry>>,
    side: NameSide,
    /// 1 for a direct wrapper, up to [`MAX_DEPTH`] for a forwarding one.
    depth: usize,
}

/// What a wrapper call hands at the wrapper's index (step 9).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Handed {
    /// One string literal: the member.
    Literal(String),
    /// A bare parameter of the caller, by index.
    Param(usize),
    /// Anything else: dynamic.
    Other,
}

/// One call of a wrapper (step 9).
#[derive(Debug)]
struct WrapperCall {
    /// The wrapper name's offset at the call.
    at: usize,
    /// The calling function: an index into `Reader::funcs`.
    caller: Option<usize>,
    handed: Handed,
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
    let (sites, counts, _) = read_file(source, view);
    (sites, counts)
}

/// [`scan`], with the file's method-name wrappers (step 9).
fn read_file(source: &str, view: &str) -> (Vec<Site>, GoCounts, Vec<Wrapper>) {
    let mut counts = GoCounts::default();
    let imports = geth_imports(source, view);
    if imports.is_empty() {
        return (Vec::new(), counts, Vec::new());
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
        package_refs: BTreeSet::new(),
    };
    for (ctor, pkg, member) in ctors {
        let first = reader.locals.len();
        reader.bind(ctor, pkg, member, &mut counts);
        reader.hand_off(first);
    }
    let (mut sites, handovers) = reader.sites(&mut counts);
    let wrappers = reader.infer_wrappers(handovers, &mut counts);
    sites.extend(reader.wrapper_sites(&wrappers, &mut counts));
    // DT.10g: every site scoped by a package constant of another file
    // (direct, wrapper and switch sites alike) names it for the engine's join.
    for site in &mut sites {
        if let Scope::Abi(name) = &site.scope
            && site.abi.is_none()
            && reader.package_refs.contains(name)
        {
            site.abi_ref = Some(AbiRef {
                name: name.clone(),
                from: evm::GO_PACKAGE.to_string(),
            });
        }
    }
    sites.sort_by(|a, b| (a.at, &a.member).cmp(&(b.at, &b.member)));
    (sites, counts, wrappers)
}

/// The entries of a Go package-level string constant's value that holds a
/// JSON ABI (DT.10g): `value` is the `NavFact::GoStringConst` text, a raw
/// string's verbatim or an interpreted string's with its escapes kept, so it
/// reads as JSON as it is or, failing that, unescaped per Go
/// (`scan::go_string_literal`). Only a value starting with `[` is tried;
/// `None` for anything that is no ABI.
pub fn read_go_const_abi(value: &str) -> Option<Vec<AbiEntry>> {
    if !value.trim_start().starts_with('[') {
        return None;
    }
    read_json_abi(value).or_else(|| {
        let quoted = format!("\"{value}\"");
        let (text, end) = go_string_literal(&quoted, 0)?;
        if end != quoted.len() {
            return None;
        }
        read_json_abi(&text)
    })
}

/// The side a site naming `member` reads with the ABI `entries` (step 7);
/// `None` when it mints nothing, counted `not_in_abi` when the ABI is known.
fn site_side(
    side: NameSide,
    entries: Option<&[AbiEntry]>,
    member: &str,
    counts: &mut GoCounts,
) -> Option<ClientSide> {
    let lists = |kind: EntryKind| {
        entries.is_some_and(|es| es.iter().any(|e| e.kind == kind && e.name == member))
    };
    let side = match side {
        NameSide::Call => ClientSide::Call,
        NameSide::Event => ClientSide::Event,
        NameSide::Decode if lists(EntryKind::Fn) => ClientSide::Call,
        NameSide::Decode if lists(EntryKind::Event) => ClientSide::Event,
        NameSide::Decode => {
            // A decoding form names a function or an event of the ABI it
            // reads; no ABI, no site.
            if entries.is_some() {
                counts.not_in_abi += 1;
            }
            return None;
        }
    };
    let kind = match side {
        ClientSide::Call => EntryKind::Fn,
        ClientSide::Event => EntryKind::Event,
    };
    if entries.is_some() && !lists(kind) {
        counts.not_in_abi += 1;
        return None;
    }
    Some(side)
}

/// The one scope and side the hand-overs `hs` of one (function, parameter)
/// agree on, with the entries one of them knows (step 9); `None` when they
/// reach two scopes or both sides. A decoding form joins either side.
fn merge_handovers(hs: &[Handover]) -> Option<(Scope, Option<Vec<AbiEntry>>, NameSide)> {
    let first = hs.first()?;
    if hs.iter().any(|h| h.scope != first.scope) {
        return None;
    }
    let mut side = NameSide::Decode;
    for h in hs {
        side = match (side, h.side) {
            (NameSide::Decode, s) | (s, NameSide::Decode) => s,
            (a, b) if a == b => a,
            _ => return None,
        };
    }
    let entries = hs.iter().find_map(|h| h.entries.clone());
    Some((first.scope.clone(), entries, side))
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
        let (name, name_at) = (view[k..name_end].to_string(), k);
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
        let (params, param_types) = param_names(view, &split_args(view, k, p_close))
            .into_iter()
            .unzip();
        let Some(open) = body_open(view, p_close + 1) else {
            continue;
        };
        let Some(close) = matching(view, open) else {
            continue;
        };
        out.push(FuncDecl {
            name,
            name_at,
            receiver,
            receiver_type,
            params,
            param_types,
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

/// The `(name, type)` of a parameter list's pieces: a named piece's name and
/// type, and a bare identifier grouped with a later type (`a, b string`)
/// with that type; none when no piece is named.
fn param_names(view: &str, pieces: &[(usize, usize)]) -> Vec<(String, String)> {
    let named: Vec<Option<String>> = pieces
        .iter()
        .map(|&(s, e)| named_piece(view, s, e))
        .collect();
    if named.iter().all(Option::is_none) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut ty = String::new();
    for (&(s, e), name) in pieces.iter().zip(named).rev() {
        match name {
            Some(name) => {
                ty = view[s + name.len()..e].trim().to_string();
                out.push((name, ty.clone()));
            }
            None if is_ident(&view[s..e]) => out.push((view[s..e].to_string(), ty.clone())),
            None => {}
        }
    }
    out.reverse();
    out
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
    /// DT.10g: the identifiers `abi.JSON(strings.NewReader(<N>))` reads that
    /// the file does not bind: constants of another file of the package.
    package_refs: BTreeSet<String>,
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
            if ctor == Ctor::Json
                && value.entries.is_none()
                && let Some(Scope::Abi(name)) = &value.scope
                && !self.binds_in_file(name, at)
            {
                self.package_refs.insert(name.clone());
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

    /// Step 7: every call site of the file, and the parameters handed bare
    /// to a name position (step 9's direct hand-overs).
    fn sites(&self, counts: &mut GoCounts) -> (Vec<Site>, Vec<Handover>) {
        let (source, view) = (self.source, self.view);
        let vb = view.as_bytes();
        let mut sites = Vec::new();
        let mut handovers = Vec::new();
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
                    if let Some(h) = self.handover(&chain, m_at, name_at, needle.name_side()) {
                        handovers.push(h);
                    } else if self
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
                let Some(side) = site_side(needle.name_side(), entries.as_deref(), &member, counts)
                else {
                    continue;
                };
                sites.push(Site {
                    side,
                    scope,
                    member,
                    args,
                    lib: LIB,
                    abi: entries,
                    abi_ref: None,
                    at: m_at,
                });
            }
        }
        sites.extend(self.switch_sites(counts));
        (sites, handovers)
    }

    /// Step 9's direct hand-over: the geth site on `chain` at `at` whose name
    /// argument `name` is a bare `string` parameter of its function; `None`
    /// when it is none, the function is named like a needle, or the site's
    /// binding is dynamic.
    fn handover(
        &self,
        chain: &Chain<'_>,
        at: usize,
        name: (usize, usize),
        side: NameSide,
    ) -> Option<Handover> {
        let text = &self.view[name.0..name.1];
        if !is_ident(text) {
            return None;
        }
        let func = self.func_index_at(at)?;
        let f = &self.funcs[func];
        if GETH_METHODS.contains(&f.name.as_str()) {
            return None;
        }
        let arg = f.params.iter().position(|p| p == text)?;
        if f.param_types.get(arg).map(String::as_str) != Some("string") {
            // ethclient's `FilterLogs(ctx, query)` hands a FilterQuery.
            return None;
        }
        let (scope, entries) = self.scope_of(chain, at)?;
        let dynamic = self
            .binding_of(chain, at)
            .is_some_and(|v| v.scope.is_some());
        Some(Handover {
            func,
            arg,
            scope,
            entries,
            side,
            dynamic,
        })
    }

    /// DT.10g: true when the file binds `name` where `at` sees it, so an
    /// `abi.JSON` argument named so is no constant of another file: a package
    /// `var` of the file, or the receiver, a parameter or a local (`name :=`,
    /// `name, err :=`, `var name`) of the function holding `at`.
    fn binds_in_file(&self, name: &str, at: usize) -> bool {
        if self.package_vars.iter().any(|v| v == name) {
            return true;
        }
        let Some(f) = self.func_at(at) else {
            return false;
        };
        if f.receiver.as_deref() == Some(name) || f.params.iter().any(|p| p == name) {
            return true;
        }
        let vb = self.view.as_bytes();
        word_hits(self.view, name)
            .into_iter()
            .filter(|&k| f.body.0 < k && k < f.body.1)
            .any(|k| {
                let before = prev_non_ws(vb, k).map(|p| vb[p]);
                let declared_by_var = prev_non_ws(vb, k)
                    .and_then(|p| ident_start_before(vb, p + 1).map(|w| &self.view[w..=p]))
                    == Some("var");
                let mut j = skip_ws(vb, k + name.len());
                // `name, err :=`: skip the rest of the left-hand side.
                while vb.get(j) == Some(&b',') {
                    j = skip_ws(vb, j + 1);
                    match ident_end(vb, j) {
                        Some(e) => j = skip_ws(vb, e),
                        None => break,
                    }
                }
                let short = vb.get(j) == Some(&b':') && vb.get(j + 1) == Some(&b'=');
                declared_by_var || (short && before != Some(b'.'))
            })
    }

    /// The index of the function whose body holds `at`.
    fn func_index_at(&self, at: usize) -> Option<usize> {
        self.funcs
            .iter()
            .position(|f| f.body.0 < at && at < f.body.1)
    }

    /// Step 9: the wrappers the `direct` hand-overs make, then those that
    /// forward into them, level by level to [`MAX_DEPTH`]. A (function,
    /// parameter) is read once (the cycle guard), at its shallowest depth.
    fn infer_wrappers(&self, direct: Vec<Handover>, counts: &mut GoCounts) -> Vec<Wrapper> {
        let mut wrappers: Vec<Wrapper> = Vec::new();
        let mut seen: Vec<(String, usize)> = Vec::new();
        let mut level = direct;
        for depth in 1..=MAX_DEPTH {
            let mut groups: BTreeMap<(String, usize), Vec<Handover>> = BTreeMap::new();
            for h in level {
                let key = (self.funcs[h.func].name.clone(), h.arg);
                if !seen.contains(&key) {
                    groups.entry(key).or_default().push(h);
                }
            }
            let mut fresh = Vec::new();
            for (key, hs) in groups {
                let unique = self.funcs.iter().filter(|f| f.name == key.0).count() == 1;
                match merge_handovers(&hs).filter(|_| unique) {
                    Some((scope, entries, side)) => fresh.push(Wrapper {
                        method: self.funcs[hs[0].func].receiver_type.is_some(),
                        name: key.0.clone(),
                        arg: key.1,
                        scope,
                        entries,
                        side,
                        depth,
                    }),
                    None => {
                        counts.wrappers.ambiguous += 1;
                        counts.dynamic += hs.iter().filter(|h| h.dynamic).count();
                    }
                }
                seen.push(key);
            }
            level = Vec::new();
            if depth < MAX_DEPTH {
                for w in &fresh {
                    for call in self.wrapper_calls(w) {
                        let (Handed::Param(arg), Some(func)) = (&call.handed, call.caller) else {
                            continue;
                        };
                        if GETH_METHODS.contains(&self.funcs[func].name.as_str()) {
                            continue;
                        }
                        level.push(Handover {
                            func,
                            arg: *arg,
                            scope: w.scope.clone(),
                            entries: w.entries.clone(),
                            side: w.side,
                            dynamic: false,
                        });
                    }
                }
            }
            wrappers.extend(fresh);
        }
        counts.wrappers.direct = wrappers.iter().filter(|w| w.depth == 1).count();
        counts.wrappers.forwarded = wrappers.len() - counts.wrappers.direct;
        wrappers
    }

    /// Every call of the wrapper `w` in the file (step 9): `<x>.W(` for a
    /// method, `W(` for a function, never its definition, with what it hands
    /// at `w`'s index. A call with fewer arguments is none.
    fn wrapper_calls(&self, w: &Wrapper) -> Vec<WrapperCall> {
        let view = self.view;
        let vb = view.as_bytes();
        let mut out = Vec::new();
        for at in word_hits(view, &w.name) {
            if self.funcs.iter().any(|f| f.name_at == at) {
                continue;
            }
            let dotted = prev_non_ws(vb, at).is_some_and(|p| vb[p] == b'.');
            if dotted != w.method {
                continue;
            }
            let open = skip_ws(vb, at + w.name.len());
            if vb.get(open) != Some(&b'(') {
                continue;
            }
            let Some(close) = matching(view, open) else {
                continue;
            };
            let Some(&(s, e)) = split_args(view, open, close).get(w.arg) else {
                continue;
            };
            let caller = self.func_index_at(at);
            let handed = match literal(self.source, s, e) {
                Some(member) => Handed::Literal(member),
                None => caller
                    .and_then(|c| self.funcs[c].params.iter().position(|p| *p == view[s..e]))
                    .map_or(Handed::Other, Handed::Param),
            };
            out.push(WrapperCall { at, caller, handed });
        }
        out
    }

    /// Step 9's sites: every wrapper call handing one string literal, read
    /// as a site of the wrapper's scope; a call handing neither a literal nor
    /// a parameter of its caller counts `dynamic`.
    fn wrapper_sites(&self, wrappers: &[Wrapper], counts: &mut GoCounts) -> Vec<Site> {
        let mut sites = Vec::new();
        for w in wrappers {
            for call in self.wrapper_calls(w) {
                let member = match call.handed {
                    Handed::Literal(member) => member,
                    Handed::Param(_) => continue,
                    Handed::Other => {
                        counts.wrappers.dynamic += 1;
                        counts.dynamic += 1;
                        continue;
                    }
                };
                if !is_ident(&member) || JSON_RPC.iter().any(|ns| member.starts_with(ns)) {
                    continue;
                }
                let Some(side) = site_side(w.side, w.entries.as_deref(), &member, counts) else {
                    continue;
                };
                counts.wrappers.sites += 1;
                sites.push(Site {
                    side,
                    scope: w.scope.clone(),
                    member,
                    args: None,
                    lib: LIB,
                    abi: w.entries.clone(),
                    abi_ref: None,
                    at: call.at,
                });
            }
        }
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
                    abi_ref: None,
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
            wrappers: WrapperCounts::default(),
        }
    }

    /// `(direct, forwarded, ambiguous, sites, dynamic)` as the marker counts.
    fn wrapped(w: (usize, usize, usize, usize, usize)) -> WrapperCounts {
        let (direct, forwarded, ambiguous, sites, dynamic) = w;
        WrapperCounts {
            direct,
            forwarded,
            ambiguous,
            sites,
            dynamic,
        }
    }

    /// `<name>/<arg> <scope key> <side> depth=<d> abi=<keys|->` per wrapper
    /// (step 9), in inference order.
    fn wrappers(src: &str) -> Vec<String> {
        let (_, _, ws) = read_file(src, &code_view(src, "go"));
        ws.iter()
            .map(|w| {
                let abi = w.entries.as_ref().map_or_else(
                    || "-".to_string(),
                    |e| e.iter().map(AbiEntry::to_key).collect::<Vec<_>>().join(","),
                );
                format!(
                    "{}/{} {} {:?} depth={} abi={abi}",
                    w.name,
                    w.arg,
                    w.scope.key(),
                    w.side,
                    w.depth
                )
            })
            .collect()
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

    /// DT.10g: `abi.JSON(strings.NewReader(<N>))` with `N` declared in no
    /// part of the file names the package constant `N` on every site of the
    /// binding (wrapper sites too); a file constant, a parameter, a local and
    /// a package var name none.
    #[test]
    fn go_package_const_reference_carries_abi_ref() {
        let src = "package chain

import (
	\"strings\"

	\"github.com/ethereum/go-ethereum/accounts/abi\"
)

const localABI = `[{\"type\":\"function\",\"name\":\"ping\",\"inputs\":[]}]`

var varABI = load()

func ReleaseCalldata(id [32]byte) ([]byte, error) {
	parsed, err := abi.JSON(strings.NewReader(shopABI))
	if err != nil {
		return nil, err
	}
	return parsed.Pack(\"release\", id)
}

func pack(method string) ([]byte, error) {
	shop, _ := abi.JSON(strings.NewReader(shopABI))
	return shop.Pack(method)
}

func Freeze() ([]byte, error) { return pack(\"freeze\") }

func Ping() ([]byte, error) {
	l, _ := abi.JSON(strings.NewReader(localABI))
	return l.Pack(\"ping\")
}

func FromParam(paramABI string) ([]byte, error) {
	p, _ := abi.JSON(strings.NewReader(paramABI))
	return p.Pack(\"a\")
}

func FromLocal() ([]byte, error) {
	localStr := fetch()
	q, _ := abi.JSON(strings.NewReader(localStr))
	return q.Pack(\"b\")
}

func FromVar() ([]byte, error) {
	v, _ := abi.JSON(strings.NewReader(varABI))
	return v.Pack(\"c\")
}
";
        let (sites, _) = scan(src, &code_view(src, "go"));
        let rows: Vec<String> = sites
            .iter()
            .map(|s| {
                let r = s
                    .abi_ref
                    .as_ref()
                    .map_or_else(|| "-".to_string(), |r| format!("{}@{}", r.name, r.from));
                format!("{}.{} ref={r}", s.scope.key(), s.member)
            })
            .collect();
        assert_eq!(
            rows,
            [
                "abi:shopABI.release ref=shopABI@.",
                "abi:shopABI.freeze ref=shopABI@.",
                "abi:localABI.ping ref=-",
                "abi:paramABI.a ref=-",
                "abi:localStr.b ref=-",
                "abi:varABI.c ref=-",
            ]
        );
    }

    /// DT.10g: a GoStringConst value reads as a JSON ABI as written (a raw
    /// string) or unescaped per Go (an interpreted one); anything else is no
    /// ABI.
    #[test]
    fn go_const_values_read_raw_or_unescaped() {
        let raw = r#"[{"type":"function","name":"release","inputs":[{"name":"id","type":"bytes32"}]}]"#;
        let want = Some(vec![AbiEntry { kind: EntryKind::Fn, name: "release".into(), arity: Some(1) }]);
        assert_eq!(read_go_const_abi(raw), want);
        assert_eq!(read_go_const_abi(&raw.replace('"', "\\\"")), want, "interpreted, escapes kept");
        for no in ["/api/v1", "[1, 2]", "", "{\"type\":\"function\"}", "[\\q]"] {
            assert_eq!(read_go_const_abi(no), None, "{no:?}");
        }
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
        // Kina backend/internal/signer/signer.go submitTo, its name read
        // through a local rather than handed over as its parameter.
        let src = format!(
            "{HEAD}{}",
            r#"
func (s *PlatformSigner) submitTo(ctx context.Context, chainName, contractAddr, method string, args ...any) (string, error) {
	parsedABI, err := abi.JSON(strings.NewReader(escrowABI))
	if err != nil {
		return "", fmt.Errorf("parse abi: %w", err)
	}
	name := strings.ToLower(method)
	data, err := parsedABI.Pack(name, args...)
	contract := bind.NewBoundContract(common.HexToAddress(contractAddr), parsedABI, client, client, client)
	tx, err := contract.RawTransact(auth, data)
	s.escrow.Transact(opts, name, data)
	return tx.Hash().Hex(), nil
}
"#
        );
        assert_eq!(
            read(&src),
            (Vec::new(), counts(1, 0, 1)),
            "no value is tracked through a local; an unbound receiver's non-literal name \
             is not counted"
        );
        assert!(wrappers(&src).is_empty());
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
        assert_eq!(
            funcs[0].param_types,
            ["context.Context", "string", "string", "string", "...any"],
            "a grouped name takes its group's type"
        );
        assert_eq!(funcs[1].param_types, ["map[K]V", "func(K) V"]);
        assert_eq!(funcs[3].param_types, ["chan int", "<-chan int"]);
        let names: Vec<&str> = funcs.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            ["submitTo", "Map", "unnamed", "chans", "Deposits", "Len"]
        );
        assert!(
            funcs.iter().all(|f| src[f.name_at..].starts_with(&f.name)
                && src[..f.name_at].trim_end().ends_with([')', 'c'])),
            "name_at is the definition's name, after `func` or the receiver"
        );
    }

    #[test]
    fn events_index_filter_watch_unpack_log() {
        let src = format!(
            "{HEAD}{}",
            r#"
func f(c *bind.BoundContract, client *ethclient.Client, q ethereum.FilterQuery) {
	parsed, _ := abi.JSON(strings.NewReader(escrowABI))
	id := parsed.Events["Released"].ID
	_ = parsed.Events["Gone"]
	logs, sub, _ := c.FilterLogs(fopts, "Released", rule)
	c.WatchLogs(wopts, "Deposited", rule)
	c.UnpackLog(&ev, "Released", log)
	c.UnpackLogIntoMap(m, "Released", log)
	client.FilterLogs(ctx, q)
	_ = parsed.Events[eventName(q)]
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
             are not in the ABI; Events[eventName(q)] on a bound receiver is dynamic"
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

    // ---- step 9: method-name wrappers (DT.10f) ----

    /// bench/substrate-gap/fixtures/go-ethereum-method-wrappers (Kina
    /// backend/internal/signer/signer.go's shape).
    const SIGNER: &str = include_str!(
        "../../../../../bench/substrate-gap/fixtures/go-ethereum-method-wrappers/backend/signer/signer.go"
    );

    /// SIGNER's ABI as `read` prints it.
    const SIGNER_KEYS: &str = "f:freeze/1,f:resolveDispute/3,f:unfreeze/1";

    #[test]
    fn direct_pack_wrapper() {
        let ws = wrappers(SIGNER);
        assert_eq!(
            ws[0],
            format!("submitTo/3 abi:escrowManagerABI Call depth=1 abi={SIGNER_KEYS}"),
            "parsedABI.Pack(method, args...) hands submitTo's fourth parameter over"
        );
        assert_eq!(
            read(SIGNER).1.wrappers.direct,
            1,
            "logCall hands its parameter to log.Printf, no name position"
        );
    }

    #[test]
    fn forwarding_wrapper() {
        assert_eq!(
            wrappers(SIGNER),
            [
                format!("submitTo/3 abi:escrowManagerABI Call depth=1 abi={SIGNER_KEYS}"),
                format!("submit/2 abi:escrowManagerABI Call depth=2 abi={SIGNER_KEYS}"),
                format!("Forward/2 abi:escrowManagerABI Call depth=3 abi={SIGNER_KEYS}"),
            ],
            "s.submitTo(ctx, chainName, \"\", method, args...) forwards submit's method; \
             Forward forwards its name into submit"
        );
    }

    #[test]
    fn literal_calls_are_sites_with_the_wrapper_scope() {
        let (rows, c) = read(SIGNER);
        assert_eq!(
            rows,
            [
                format!(
                    "call abi:escrowManagerABI.resolveDispute args=- abi={SIGNER_KEYS} line=39"
                ),
                format!("call abi:escrowManagerABI.freeze args=- abi={SIGNER_KEYS} line=43"),
                format!("call abi:escrowManagerABI.unfreeze args=- abi={SIGNER_KEYS} line=47"),
            ],
            "s.logCall(\"resolveDispute\") is no wrapper call"
        );
        assert_eq!(
            c,
            GoCounts {
                wrappers: wrapped((1, 2, 0, 3, 1)),
                ..counts(1, 0, 1)
            },
            "the Pack site hands its name over; Retry's local `m` is the one dynamic call"
        );
        let out = crate::evm::extract_evm_clients(
            SIGNER,
            "go",
            glia_core::NodeId(1),
            glia_core::RepoId(1),
        );
        assert_eq!(
            out.markers("signer/signer.go"),
            [
                "[evm-client] lang=go calls=3 events=0 sites=3 (ethers=0 hardhat=0 typechain=0 viem=0 web3=0 geth=3) abis=1 not_in_abi=0 dynamic=1 path=signer/signer.go",
                "[evm-wrappers] wrappers=3 (direct=1 forwarded=2 ambiguous=0) sites=3 dynamic=1 path=signer/signer.go",
            ]
        );
        let lines: Vec<u32> = out.anchors.iter().map(|a| a.line).collect();
        assert_eq!(lines, [42, 38, 46], "0-based, one per call, by node");
    }

    #[test]
    fn logging_function_is_not_a_wrapper() {
        let src = format!(
            "{HEAD}{}",
            r#"
type Signer struct{}

func (s *Signer) logCall(method string) {
	log.Printf("calling %s", method)
	fmt.Println(method)
}

func (s *Signer) describe(method string) string { return "call " + method }

func (s *Signer) Release(id [32]byte) {
	s.logCall("release")
	_ = s.describe("deposit")
}
"#
        );
        assert_eq!(read(&src), (Vec::new(), counts(1, 0, 0)));
        assert!(wrappers(&src).is_empty());
    }

    #[test]
    fn local_variable_argument_is_dynamic() {
        let src = format!(
            "{HEAD}{}",
            r#"
var parsed, _ = abi.JSON(strings.NewReader(escrowABI))

func pack(method string, args ...any) ([]byte, error) { return parsed.Pack(method, args...) }

func Calls(id [32]byte, names []string) {
	m := "deposit"
	pack(m, id)
	pack("de"+"posit", id)
	pack(names[0], id)
	pack(strings.ToLower("RELEASE"), id)
	pack("release", id)
	pack()
}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [format!(
                "call abi:escrowABI.release args=- abi={KEYS} line=26"
            )],
            "one string literal only; a call with too few arguments is none"
        );
        assert_eq!(
            c,
            GoCounts {
                wrappers: wrapped((1, 0, 0, 1, 4)),
                ..counts(1, 0, 4)
            }
        );
    }

    #[test]
    fn forward_is_not_dynamic() {
        let src = format!(
            "{HEAD}{}",
            r#"
var parsed, _ = abi.JSON(strings.NewReader(escrowABI))

func pack(method string) ([]byte, error) { return parsed.Pack(method) }

func relay(name string) ([]byte, error) { return pack(name) }

type Raw struct{}

func (r *Raw) Call(method string) ([]byte, error) { return pack(method) }

type A struct{}
type B struct{}

func (a *A) hop(name string) ([]byte, error) { return pack(name) }
func (b *B) hop(name string) ([]byte, error) { return pack(name) }

func Use() {
	relay("deposit")
	hop("release")
}
"#
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [format!(
                "call abi:escrowABI.deposit args=- abi={KEYS} line=33"
            )],
            "Raw.Call is named like a needle; hop is declared twice; a bare call of a \
             method is none"
        );
        assert_eq!(
            c,
            GoCounts {
                wrappers: wrapped((1, 1, 1, 1, 0)),
                ..counts(1, 0, 0)
            },
            "every parameter handed on is a forward, wrapper or not: never dynamic"
        );
    }

    #[test]
    fn two_receivers_same_name_is_ambiguous() {
        let src = format!(
            "{HEAD}{}",
            r#"
var parsed, _ = abi.JSON(strings.NewReader(escrowABI))

type A struct{}
type B struct{}

func (a *A) send(method string) ([]byte, error) { return parsed.Pack(method) }
func (b *B) send(method string) ([]byte, error) { return parsed.Pack(method) }

func both(c *bind.BoundContract, method string) {
	parsed.Pack(method)
	c.FilterLogs(opts, method)
}

func (a *A) Deposit() {
	a.send("deposit")
	both(nil, "release")
}
"#
        );
        assert_eq!(
            read(&src),
            (
                Vec::new(),
                GoCounts {
                    wrappers: wrapped((0, 0, 2, 0, 0)),
                    ..counts(1, 0, 3)
                }
            ),
            "send names two methods; both's parameter reaches the ABI as a call and the \
             receiver c as an event; their bound Pack sites keep step 7's count"
        );
        let out =
            crate::evm::extract_evm_clients(&src, "go", glia_core::NodeId(1), glia_core::RepoId(1));
        assert_eq!(
            out.markers("a.go"),
            [
                "[evm-client] lang=go calls=0 events=0 sites=0 (ethers=0 hardhat=0 typechain=0 viem=0 web3=0 geth=0) abis=1 not_in_abi=0 dynamic=3 path=a.go",
                "[evm-wrappers] wrappers=2 (direct=0 forwarded=0 ambiguous=2) sites=0 dynamic=0 path=a.go",
            ]
        );
    }

    #[test]
    fn geth_method_names_are_never_wrappers() {
        let src = format!(
            r#"// Code generated - DO NOT EDIT.
package escrowbind

import (
	"github.com/ethereum/go-ethereum/accounts/abi/bind"
)

var EscrowMetaData = &bind.MetaData{{
	ABI: {META_ABI},
}}

func (_Escrow *EscrowRaw) Call(opts *bind.CallOpts, result *[]interface{{}}, method string, params ...interface{{}}) error {{
	return _Escrow.Contract.EscrowCaller.contract.Call(opts, result, method, params...)
}}

func (_Escrow *EscrowRaw) Transact(opts *bind.TransactOpts, method string, params ...interface{{}}) (*types.Transaction, error) {{
	return _Escrow.Contract.EscrowTransactor.contract.Transact(opts, method, params...)
}}

func (_Escrow *EscrowTransactorRaw) Transact(opts *bind.TransactOpts, method string, params ...interface{{}}) (*types.Transaction, error) {{
	return _Escrow.Contract.contract.Transact(opts, method, params...)
}}

func (_Escrow *EscrowFilterer) FilterLogs(opts *bind.FilterOpts, name string) error {{
	_, _, err := _Escrow.contract.FilterLogs(opts, name)
	return err
}}

func Use(raw *EscrowRaw, opts *bind.TransactOpts) {{
	raw.Transact(opts, "deposit")
}}
"#
        );
        let (rows, c) = read(&src);
        assert!(wrappers(&src).is_empty());
        assert_eq!(
            rows,
            ["call recv:raw.deposit args=0 abi=- line=30"],
            "the Raw call is step 7's own site, on an unbound receiver"
        );
        assert_eq!(c, counts(1, 0, 0), "the Raw forwards count nothing");
    }

    #[test]
    fn depth_bound_and_cycle_guard() {
        let src = format!(
            "{HEAD}{}",
            r#"
var parsed, _ = abi.JSON(strings.NewReader(escrowABI))

func pack1(method string) ([]byte, error) { return parsed.Pack(method) }
func pack2(m string) ([]byte, error)      { return pack1(m) }
func pack3(m string) ([]byte, error)      { return pack2(m) }
func pack4(m string) ([]byte, error)      { return pack3(m) }

func ping(m string) ([]byte, error) { return pong(m) }
func pong(m string) ([]byte, error) {
	ping(m)
	return parsed.Pack(m)
}

func Use() {
	pack4("deposit")
	pack3("release")
	ping("deposit")
}
"#
        );
        assert_eq!(
            wrappers(&src),
            [
                format!("pack1/0 abi:escrowABI Call depth=1 abi={KEYS}"),
                format!("pong/0 abi:escrowABI Call depth=1 abi={KEYS}"),
                format!("pack2/0 abi:escrowABI Call depth=2 abi={KEYS}"),
                format!("ping/0 abi:escrowABI Call depth=2 abi={KEYS}"),
                format!("pack3/0 abi:escrowABI Call depth=3 abi={KEYS}"),
            ],
            "pack4 is depth 4; pong's call back into ping reads no new wrapper"
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                format!("call abi:escrowABI.release args=- abi={KEYS} line=31"),
                format!("call abi:escrowABI.deposit args=- abi={KEYS} line=32"),
            ]
        );
        assert_eq!(
            c,
            GoCounts {
                wrappers: wrapped((2, 3, 0, 2, 0)),
                ..counts(1, 0, 0)
            }
        );
    }

    #[test]
    fn event_wrapper() {
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

func (idx *Indexer) event(name string) abi.Event { return idx.parsedABI.Events[name] }

func (idx *Indexer) decode(out any, name string, data []byte) error {
	return idx.parsedABI.UnpackIntoInterface(out, name, data)
}

func (idx *Indexer) Handle(data []byte) {
	_ = idx.event("Released")
	_ = idx.event("deposit")
	_ = event("Released")
	idx.decode(&ev, "Released", data)
	idx.decode(&out, "deposit", data)
	idx.decode(&out, "gone", data)
}
"#
        );
        assert_eq!(
            wrappers(&src),
            [
                format!("decode/1 abi:escrowABI Decode depth=1 abi={KEYS}"),
                format!("event/0 abi:escrowABI Event depth=1 abi={KEYS}"),
            ]
        );
        let (rows, c) = read(&src);
        assert_eq!(
            rows,
            [
                format!("event abi:escrowABI.Released args=- abi={KEYS} line=32"),
                format!("event abi:escrowABI.Released args=- abi={KEYS} line=35"),
                format!("call abi:escrowABI.deposit args=- abi={KEYS} line=36"),
            ],
            "a bare event(..) calls no method; a decoding wrapper reads each name's side"
        );
        assert_eq!(
            c,
            GoCounts {
                wrappers: wrapped((2, 0, 0, 3, 0)),
                ..counts(1, 2, 0)
            },
            "deposit is no event; gone is in neither list"
        );
    }
}
