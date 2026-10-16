//! The TypeScript / JavaScript ethers client reader (DT.10c).
//!
//! 1. **Gate.** The file imports or requires `ethers` (`ethers/..` and an
//!    `@ethersproject/` path too) or `hardhat` (whose `ethers` is ethers);
//!    otherwise nothing is read, so a local `class Contract` mints nothing.
//! 2. **ABI bindings.** `(export )?(const|let|var) <N>(: T)? = <abi literal>`
//!    at any depth, read by `abi::read_abi`. A binding whose literal does not
//!    read is no ABI. An ABI binding is visible file-wide (the nearest one
//!    before a use, else the first).
//! 3. **Contract bindings.** `(const|let|var) <x>(: T)? = (await )?new
//!    (ethers.)?(Contract|BaseContract)(<addr>, <abi>, ..)` and a bare `<x> =
//!    new ..`. The ABI argument (an identifier may carry a TS `!` or `as T`):
//!    an identifier with an ABI binding is `Scope::Abi(<identifier>)` with its
//!    entries; any other identifier (an import) `Scope::Abi(<identifier>)`
//!    with no entries; an inline literal that reads `Scope::Abi(<x>)` with its
//!    entries; anything else (a call, a member path) binds nothing and counts
//!    `dynamic`.
//! 4. **Scope.** A `const` / `let` binding is visible from the end of its
//!    `new ..(..)` to the end of its innermost enclosing block; a `var`
//!    binding file-wide. A bare assignment binds in the block of the nearest
//!    preceding `let x` declaration that holds it, file-wide after a `var x`
//!    or with none; a bare assignment in a class body or after a field
//!    modifier is a class field (DT.10i's form) and binds nothing, as does one
//!    in a parameter list or after a type annotation (`x: T = new ..`, a
//!    default parameter). A site reads the NEAREST binding of its receiver
//!    that precedes it and whose scope holds it, so a later function's
//!    parameter `escrow` never reads an earlier function's contract.
//! 5. **Call sites** on a bound receiver `R` (not itself a member, so
//!    `this.escrow` is not `escrow`): `R.<m>(` (and `R?.<m>(`, `R!.<m>(`),
//!    and a chained `new (ethers.)?Contract(<addr>, <abi identifier>, ..).<m>(`
//!    with no binding. [`ETHERS_MEMBERS`] are never sites and are not
//!    counted. With ABI entries known, a member that is no function of them
//!    mints nothing and counts `not_in_abi`. `args` is the depth-0 argument
//!    count; `lib` is `ethers`.
//! 6. **Event sites** on `R`: `R.on('<E>'` / `R.once('<E>'` (a signature key
//!    `'E(uint256)'` reads as `E`). With entries known, only an event of them.

use glia_code_domain::evm::{AbiEntry, ClientSide, EntryKind, Scope};

use super::Site;
use super::abi::read_abi;
use super::scan::{
    enclosing_block, ident_end, ident_start_before, is_ident, matching, prev_non_ws, skip_ws,
    split_args, string_literal, trim, word_hits,
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

/// The subscription members read as event sites.
const EVENT_MEMBERS: &[&str] = &["on", "once"];

/// Field modifiers: an assignment after one is a class field.
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
    /// a literal that reads.
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
    name: String,
    scope: Scope,
    abi: Option<Vec<AbiEntry>>,
    /// The end of its `new ..(..)`: a site must come after it.
    at: usize,
    block: (usize, usize),
}

/// The ABI argument of a construction.
enum AbiArg {
    Ident(String),
    Literal(Vec<AbiEntry>),
    Dynamic,
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
        if let Some((m_at, member, paren)) = member_call(vb, close + 1) {
            if ETHERS_MEMBERS.contains(&member) {
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
            };
            push_call(&mut sites, &mut counts, view, &chained, member, m_at, paren);
            continue;
        }
        let Some((name, block)) = binding_target(view, &decls, new_at) else {
            continue;
        };
        let (scope, abi) = match arg {
            AbiArg::Ident(abi_name) => {
                let abi = abi_lookup(&abis, &abi_name, new_at);
                (Scope::Abi(abi_name), abi)
            }
            AbiArg::Literal(entries) => {
                counts.abis += 1;
                (Scope::Abi(name.clone()), Some(entries))
            }
            AbiArg::Dynamic => {
                counts.dynamic += 1;
                continue;
            }
        };
        bindings.push(Binding {
            name,
            scope,
            abi,
            at: close,
            block,
        });
    }

    let mut receivers: Vec<&str> = bindings.iter().map(|b| b.name.as_str()).collect();
    receivers.sort_unstable();
    receivers.dedup();
    for receiver in receivers {
        for at in word_hits(view, receiver) {
            // `this.escrow` / `this.#escrow` are members, not the binding.
            if at > 0 && matches!(vb[at - 1], b'.' | b'#') {
                continue;
            }
            let Some((m_at, member, paren)) = member_call(vb, at + receiver.len()) else {
                continue;
            };
            let Some(binding) = visible(&bindings, receiver, at) else {
                continue;
            };
            if EVENT_MEMBERS.contains(&member) {
                push_event(&mut sites, &mut counts, source, view, binding, m_at, paren);
            } else if !ETHERS_MEMBERS.contains(&member) {
                push_call(&mut sites, &mut counts, view, binding, member, m_at, paren);
            }
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

/// `.<m>(` after a receiver ending at `from` (also `?.` and a TS `!.`): the
/// member's offset, the member and its `(`.
fn member_call(vb: &[u8], from: usize) -> Option<(usize, &str, usize)> {
    let mut k = skip_ws(vb, from);
    if vb.get(k) == Some(&b'!') {
        k = skip_ws(vb, k + 1);
    }
    if vb.get(k) == Some(&b'?') && vb.get(k + 1) == Some(&b'.') {
        k += 1;
    }
    if vb.get(k) != Some(&b'.') {
        return None;
    }
    let m_at = skip_ws(vb, k + 1);
    let m_end = ident_end(vb, m_at)?;
    let paren = skip_ws(vb, m_end);
    if vb.get(paren) != Some(&b'(') {
        return None;
    }
    let member = std::str::from_utf8(&vb[m_at..m_end]).ok()?;
    Some((m_at, member, paren))
}

/// The variable a construction at `new_at` binds and the range it is visible
/// in (steps 3 and 4); `None` when the construction binds no variable.
fn binding_target(view: &str, decls: &[Decl], new_at: usize) -> Option<(String, (usize, usize))> {
    let vb = view.as_bytes();
    let mut init = new_at;
    let mut eq = prev_non_ws(vb, new_at)?;
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
        return Some((d.name.clone(), block));
    }
    // A bare assignment `<x> = new ..`.
    if vb[eq] != b'=' || eq == 0 || b"=!<>+-*/%&|^?".contains(&vb[eq - 1]) {
        return None;
    }
    let x_end = prev_non_ws(vb, eq)? + 1;
    let x_start = ident_start_before(vb, x_end)?;
    let name = &view[x_start..x_end];
    if let Some(before) = prev_non_ws(vb, x_start) {
        if matches!(vb[before], b'.' | b'#' | b'(' | b',' | b':') {
            return None;
        }
        if let Some(w) = ident_start_before(vb, before + 1)
            && FIELD_MODIFIERS.contains(&&view[w..=before])
        {
            return None;
        }
    }
    let (open, close) = enclosing_block(view, x_start);
    if close < vb.len() && is_class_body(view, open) {
        return None;
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
    Some((name.to_string(), block))
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

/// The binding of `receiver` a site at `at` reads (step 4).
fn visible<'a>(bindings: &'a [Binding], receiver: &str, at: usize) -> Option<&'a Binding> {
    bindings
        .iter()
        .filter(|b| b.name == receiver && b.at < at && b.block.0 <= at && at <= b.block.1)
        .max_by_key(|b| b.at)
}

/// A call site `<binding>.<member>(..)` whose `(` is at `paren` (step 5).
fn push_call(
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    view: &str,
    binding: &Binding,
    member: &str,
    m_at: usize,
    paren: usize,
) {
    if let Some(entries) = &binding.abi
        && !entries
            .iter()
            .any(|e| e.kind == EntryKind::Fn && e.name == member)
    {
        counts.not_in_abi += 1;
        return;
    }
    let args = matching(view, paren)
        .map(|close| split_args(view, paren, close).len())
        .and_then(|n| u32::try_from(n).ok());
    sites.push(Site {
        side: ClientSide::Call,
        scope: binding.scope.clone(),
        member: member.to_string(),
        args,
        lib: LIB,
        abi: binding.abi.clone(),
        at: m_at,
    });
}

/// An event site `<binding>.on('<E>', ..)` whose `(` is at `paren` (step 6).
fn push_event(
    sites: &mut Vec<Site>,
    counts: &mut TsCounts,
    source: &str,
    view: &str,
    binding: &Binding,
    m_at: usize,
    paren: usize,
) {
    let vb = view.as_bytes();
    let q = skip_ws(vb, paren + 1);
    let Some((text, _)) = string_literal(source, q) else {
        return;
    };
    let event = text.split('(').next().unwrap_or_default();
    let (s, e) = trim(event.as_bytes(), 0, event.len());
    let event = &event[s..e];
    if !is_ident(event) {
        return;
    }
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
        member: event.to_string(),
        args: None,
        lib: LIB,
        abi: binding.abi.clone(),
        at: m_at,
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
        assert_eq!(
            rows,
            ["call abi:ESCROW_ABI.release args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=14"]
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
        assert_eq!(
            rows,
            ["call abi:ESCROW_ABI.deposit args=1 abi=f:deposit/1,f:release/1,e:Released/2 line=7"]
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
}
