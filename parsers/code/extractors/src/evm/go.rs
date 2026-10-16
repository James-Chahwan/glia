//! The Go go-ethereum contract call reader (DT.10e).
//!
//! 1. **Gate** ([`geth_imports`]). The file imports a
//!    `github.com/ethereum/go-ethereum` package (single or grouped `import`,
//!    any alias); otherwise nothing is read. The imports name the packages the
//!    bindings are spelled with: `accounts/abi` (`abi`, or its alias) and
//!    `accounts/abi/bind` or `bind/v2` (`bind`, or its alias).
//! 2. **ABI declarations** ([`abi_decls`]). A package-level `const` / `var`
//!    spec (single or grouped, with or without a type) whose value is one
//!    string literal, raw or interpreted (unescaped per Go by
//!    `scan::go_string_literal`), that `abi::read_json_abi` reads: ABI `<N>`.
//!    The literal is read from the source at its offsets, so a JSON ABI's
//!    braces never unbalance the view. A function-local `const` is no ABI
//!    declaration.
//! 3. **Functions** ([`func_decls`]). Every top-level `func (<recv>
//!    <Type>)? <Name>[<tparams>]?(<params>) <results> {..}` with its receiver
//!    name, parameter names and balanced body. Parameters split at depth-0
//!    commas, so the grouped `chainName, contractAddr, method string` names
//!    every one, and a variadic `args ...any` is the last.
//! 4. **Bindings.** The first left-hand element of `<x>(, <err>)? := ..`,
//!    `var <x>(, <err>)? = ..` or `<x>(, <err>)? = ..`, or the field of a
//!    composite literal `<field>: ..`, binds the value of
//!    - `abi.JSON(strings.NewReader(<N>))`: `Scope::Abi(N)` with N's entries
//!      (none when N is declared in another file of the package, so the
//!      resolver name-hints it); an inline literal that reads gives
//!      `Scope::Abi(<x>)` with its entries and counts as an ABI read;
//!    - `bind.NewBoundContract(<addr>, <v> | *<v> | &<v>, ..)`: the value v
//!      resolves to (step 6), else `Scope::Abi(<v>'s last segment)` with no
//!      entries.
//!
//!    Any other ABI argument (an `io.Reader`, a call) is DYNAMIC: counted, and
//!    the binding's sites mint nothing. A declared binding (`:=`, `var`) is
//!    visible from the end of its construction to the end of its innermost
//!    block (the DT.10c rule); a package-level one, and an assignment to a
//!    package `var`, in the whole file; any other assignment in its
//!    function's body. A function whose receiver or a parameter is named like
//!    a binding's root hides a binding declared outside it. A site reads the
//!    NEAREST visible binding of its receiver.
//! 5. **Hand-offs.** A bound identifier handed to a struct field, `<field>:
//!    <x>` / `<field>: *<x>` / `<field>: &<x>` in a composite literal or
//!    `<recv>.<field> = <x>` as a statement, gives the field x's value,
//!    file-wide by field name and one hop (a field never hands on a field). A
//!    selector left-hand side (`s.abi, err = abi.JSON(..)`) binds its text
//!    and its last segment as a field. A field bound twice reads its first
//!    binding, deterministically.
//! 6. **Scope** ([`Reader::scope_of`]). A receiver is a selector chain `a.b.c`
//!    directly before the needle's dot. It resolves through its full text
//!    first (the visible binding of that text), then through its last segment
//!    as a field; a chain hanging off a call or an index (`f().escrow`)
//!    resolves through its field only. A bound receiver reads its binding's
//!    scope and entries; an unbound one is `Scope::Recv(<last segment>)` with
//!    no entries, which `resolver:evm` pairs by name_hint (HEURISTIC, Weak,
//!    arity-checked).
//! 7. **Sites.** Every needle is dot-preceded (so `RawTransact(` and
//!    `NewKeyedTransactorWithChainID(` are none) and needs a string literal at
//!    its name position:
//!    - `<r>.Pack("m", ..)`, `<r>.Methods["m"]`, `<r>.Transact(<opts>, "m", ..)`;
//!    - `<r>.Call(<opts>, <out>, "m", ..)`, the third argument; a `.Call(<out>,
//!      "eth_..")` with a string literal second is go-ethereum's `rpc.Client`
//!      and is not read;
//!    - `<r>.Unpack("m", ..)` / `<r>.UnpackIntoInterface(<out>, "m", ..)` when
//!      r's ABI lists m as a function (an event is DT.10k's; no ABI, no site).
//!
//!    `args` counts the arguments after the name; a trailing `xs...` spread,
//!    `Methods[..]` and the two Unpack forms (whose arguments are the bytes
//!    decoded, not the call's) give unknown. `lib` is `geth`: exact arity,
//!    no overrides argument. With entries known, a member that is no function
//!    of them mints nothing and counts `not_in_abi`; with none
//!    (`Scope::Recv`, an ABI of another file) every name-literal site mints and
//!    the resolver decides. A name in a JSON-RPC namespace ([`JSON_RPC`])
//!    never mints and is not counted. A non-literal name on a bound receiver
//!    (`parsedABI.Pack(method, args...)`, Kina signer.go) mints nothing and
//!    counts `dynamic` (DT.10f reads those through their wrappers).

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

/// The two binding constructions (step 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ctor {
    /// `abi.JSON(..)`.
    Json,
    /// `bind.NewBoundContract(..)`.
    Bound,
}

/// The call-site needles (step 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Needle {
    Pack,
    Methods,
    Transact,
    Call,
    Unpack,
    UnpackIntoInterface,
}

/// Every needle and the member it is spelled as.
const NEEDLES: &[(&str, Needle)] = &[
    ("Pack", Needle::Pack),
    ("Methods", Needle::Methods),
    ("Transact", Needle::Transact),
    ("Call", Needle::Call),
    ("Unpack", Needle::Unpack),
    ("UnpackIntoInterface", Needle::UnpackIntoInterface),
];

impl Needle {
    /// The index of the name argument.
    fn name_arg(self) -> usize {
        match self {
            Needle::Pack | Needle::Unpack | Needle::Methods => 0,
            Needle::Transact | Needle::UnpackIntoInterface => 1,
            Needle::Call => 2,
        }
    }

    /// The decoding forms: a site only where the ABI lists a function, and
    /// their arguments give no arity.
    fn decodes(self) -> bool {
        matches!(self, Needle::Unpack | Needle::UnpackIntoInterface)
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
    let specs = package_specs(view);
    let decls = abi_decls(source, &specs);
    counts.abis = decls.len();
    let package_vars = specs
        .iter()
        .filter(|s| s.var)
        .flat_map(|s| s.names.iter().cloned())
        .collect();
    let mut reader = Reader {
        source,
        view,
        decls,
        package_vars,
        funcs: func_decls(view),
        locals: Vec::new(),
        fields: Vec::new(),
    };
    let package = |paths: &[&str]| {
        imports
            .iter()
            .find(|(path, _)| paths.contains(&path.as_str()))
            .map(|(_, name)| name.as_str())
    };
    for (ctor, pkg, member) in [
        (Ctor::Json, package(&[ABI_PKG]), "JSON"),
        (Ctor::Bound, package(BIND_PKGS), "NewBoundContract"),
    ] {
        let Some(pkg) = pkg else {
            continue;
        };
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
    let vb = view.as_bytes();
    let mut depth = 0usize;
    let mut k = 0;
    let mut out = Vec::new();
    for h in hits {
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

/// The ABI declarations among `specs` (step 2).
fn abi_decls(source: &str, specs: &[ValueSpec]) -> Vec<AbiDecl> {
    let mut out = Vec::new();
    for spec in specs {
        for (name, &(s, e)) in spec.names.iter().zip(&spec.values) {
            if let Some(text) = literal(source, s, e)
                && let Some(entries) = read_json_abi(&text)
            {
                out.push(AbiDecl {
                    name: name.clone(),
                    entries,
                });
            }
        }
    }
    out
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
        let mut receiver = None;
        if vb.get(k) == Some(&b'(') {
            let Some(close) = matching(view, k) else {
                continue;
            };
            receiver = split_args(view, k, close)
                .first()
                .and_then(|&(s, e)| named_piece(view, s, e));
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
            params,
            body: (open, close),
        });
    }
    out
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
    decls: Vec<AbiDecl>,
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
                Ctor::Json => self.json_value(&args, &target, counts),
                Ctor::Bound => self.bound_value(&args, at),
            };
            if value.scope.is_none() {
                counts.dynamic += 1;
            }
            self.push_target(target, value, close);
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
        match self.binding_of(chain, at) {
            Some(value) => Some((value.scope.clone()?, value.entries.clone())),
            None => Some((Scope::Recv(chain.last.to_string()), None)),
        }
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
                let (name_at, args) = if needle == Needle::Methods {
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
                    let args = if needle.decodes() {
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
                if needle.decodes() && !lists(EntryKind::Fn) {
                    // An event is DT.10k's; no ABI, no site.
                    if entries.is_some() && !lists(EntryKind::Event) {
                        counts.not_in_abi += 1;
                    }
                    continue;
                }
                if entries.is_some() && !lists(EntryKind::Fn) {
                    counts.not_in_abi += 1;
                    continue;
                }
                sites.push(Site {
                    side: ClientSide::Call,
                    scope,
                    member,
                    args,
                    lib: LIB,
                    abi: entries,
                    at: m_at,
                });
            }
        }
        sites.sort_by(|a, b| (a.at, &a.member).cmp(&(b.at, &b.member)));
        sites
    }
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

    /// `call <scope key>.<member> args=<n|-> abi=<keys|-> line=<l>` per site,
    /// in file order, and the counts.
    fn read(src: &str) -> (Vec<String>, GoCounts) {
        let (sites, counts) = scan(src, &code_view(src, "go"));
        let rows = sites
            .iter()
            .map(|s| {
                assert_eq!((s.side, s.lib), (ClientSide::Call, "geth"));
                let abi = s.abi.as_ref().map_or_else(
                    || "-".to_string(),
                    |e| e.iter().map(AbiEntry::to_key).collect::<Vec<_>>().join(","),
                );
                let args = s.args.map_or_else(|| "-".to_string(), |a| a.to_string());
                let line = src[..s.at].matches('\n').count() + 1;
                format!(
                    "call {}.{} args={args} abi={abi} line={line}",
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
        let decls: Vec<(String, Vec<String>)> = abi_decls(src, &package_specs(&view))
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
    fn unpack_reads_abi_functions_only() {
        let src = format!(
            "{HEAD}{}",
            r#"
func f(data []byte) {
	parsed, _ := abi.JSON(strings.NewReader(escrowABI))
	parsed.Unpack("deposit", data)
	parsed.UnpackIntoInterface(&out, "release", data)
	parsed.Unpack("Released", data)
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
            ]
        );
        assert_eq!(c, counts(1, 1, 0), "an event is no miss; gone is");
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
";
        let view = code_view(src, "go");
        let funcs = func_decls(&view);
        let decls: Vec<(Option<&str>, Vec<&str>, &str)> = funcs
            .iter()
            .map(|f| {
                (
                    f.receiver.as_deref(),
                    f.params.iter().map(String::as_str).collect(),
                    &src[f.body.0..=f.body.1],
                )
            })
            .collect();
        assert_eq!(decls.len(), 4, "{decls:?}");
        assert_eq!(
            decls[0],
            (
                Some("s"),
                vec!["ctx", "chainName", "contractAddr", "method", "args"],
                "{\n\treturn \"\", nil\n}"
            )
        );
        assert_eq!(decls[1], (None, vec!["m", "f"], "{ return m }"));
        assert_eq!(
            decls[2],
            (None, vec![], "{\n\treturn struct{ a int }{}\n}"),
            "unnamed parameters name nothing; a struct result is stepped over"
        );
        assert_eq!(decls[3], (None, vec!["c", "d"], "{}"));
    }
}
