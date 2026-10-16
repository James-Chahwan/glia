//! ABI literals (DT.10c): the entries a client's ABI declares, as
//! `glia_code_domain::evm::AbiEntry`s (`f:deposit/1`, `e:Released/2`).
//!
//! Two shapes are read, both as an array literal:
//! - **human-readable** (ethers): an array of strings, each a fragment
//!   `function <name>(<params>) ..` or `event <Name>(<params>)`. `error`,
//!   `constructor`, `fallback` and `receive` fragments are skipped. The
//!   parameters are split at depth 0, so a `tuple(..)` or `(uint256,address)`
//!   parameter counts once and `()` counts 0; the modifiers and the `returns
//!   (..)` clause after the parameter list are ignored. Any string that is no
//!   fragment makes the array no ABI (`['deposit', 'release']` is a list of
//!   names, not an ABI).
//! - **JSON** (solc / hardhat output, [`read_json_abi`]): an array of ABI
//!   objects, each with a `type` (`function`, `event`, `error`,
//!   `constructor`, `fallback`, `receive`, or absent for a function), a
//!   string `name` and an `inputs` array whose length is the arity. Any other
//!   item makes the array no ABI. In a TS / JS source the array may be a TS
//!   object literal (DT.10j): bare or quoted keys, single or double quotes,
//!   trailing commas and comments, normalised to JSON by
//!   [`object_literal_json`]; a value that is no literal (an identifier, a
//!   spread, a call, a template with `${..}`) makes it no ABI.
//!
//! viem's readers (DT.10j) wrap the array: `parseAbi([..])` reads the array
//! inside (a human-readable `struct` signature declares no entry), and
//! `parseAbiItem('<signature>')` / `parseAbiItem([..])` is the one-entry ABI
//! of its signature (the first function or event of an array: the rest are
//! the `struct`s it uses). Either may be read off a module (`viem.parseAbi`).
//! A trailing `as const` / `satisfies Abi` is never read, so it changes
//! nothing.
//!
//! An ABI needs at least one function or event. Entries are sorted and
//! deduplicated (functions first, then events, each by name and arity), so
//! two spellings of one ABI give one entry list.

use glia_code_domain::evm::{AbiEntry, EntryKind};

use super::scan::{ident_end, is_ident_byte, matching, skip_ws, split_args, string_literal};

/// The entries of the ABI literal at `start` (the same offset in `source`
/// and its code `view`): an array literal, or a `parseAbi(..)` /
/// `parseAbiItem(..)` call (module doc); `None` when it is no ABI.
pub(crate) fn read_abi(source: &str, view: &str, start: usize) -> Option<Vec<AbiEntry>> {
    let vb = view.as_bytes();
    if let Some((open, item)) = parse_abi_call(view, start) {
        let close = matching(view, open)?;
        let &(s, e) = split_args(view, open, close).first()?;
        if !item {
            return read_abi(source, view, s);
        }
        if matches!(vb[s], b'\'' | b'"' | b'`') {
            let (text, end) = string_literal(source, s)?;
            if end != e {
                return None;
            }
            return fragment(&text)?.map(|entry| vec![entry]);
        }
        if vb[s] != b'[' {
            return None;
        }
        let close = matching(view, s)?;
        let first = human_readable(source, &split_args(view, s, close))?
            .into_iter()
            .next()?;
        return Some(vec![first]);
    }
    if vb.get(start) != Some(&b'[') {
        return None;
    }
    let close = matching(view, start)?;
    let items = split_args(view, start, close);
    if items.is_empty() {
        return None;
    }
    if items
        .iter()
        .all(|&(s, _)| matches!(vb[s], b'\'' | b'"' | b'`'))
    {
        return canonical(human_readable(source, &items)?);
    }
    if items.iter().all(|&(s, _)| vb[s] == b'{') {
        return read_json_abi(source.get(start..=close)?).or_else(|| {
            object_literal_json(source, view, start, close).and_then(|j| read_json_abi(&j))
        });
    }
    None
}

/// `(<module>.)?parseAbi(` / `parseAbiItem(` at `start`: the offset of its
/// `(` and whether it is `parseAbiItem`.
fn parse_abi_call(view: &str, start: usize) -> Option<(usize, bool)> {
    let vb = view.as_bytes();
    let mut at = start;
    let mut end = ident_end(vb, at)?;
    let dot = skip_ws(vb, end);
    if vb.get(dot) == Some(&b'.') {
        at = skip_ws(vb, dot + 1);
        end = ident_end(vb, at)?;
    }
    let item = match &view[at..end] {
        "parseAbi" => false,
        "parseAbiItem" => true,
        _ => return None,
    };
    let open = skip_ws(vb, end);
    (vb.get(open) == Some(&b'(')).then_some((open, item))
}

/// The entries of human-readable fragments `items` (string literals), in
/// source order; `None` when any of them is no fragment.
fn human_readable(source: &str, items: &[(usize, usize)]) -> Option<Vec<AbiEntry>> {
    let mut entries = Vec::new();
    for &(s, e) in items {
        let (text, end) = string_literal(source, s)?;
        if end != e {
            return None;
        }
        if let Some(entry) = fragment(&text)? {
            entries.push(entry);
        }
    }
    Some(entries)
}

/// The TS / JS object-literal array between the brackets at `open` and
/// `close` as JSON text (module doc): keys quoted, strings re-quoted, trailing
/// commas dropped; `None` for any value JSON cannot hold.
fn object_literal_json(source: &str, view: &str, open: usize, close: usize) -> Option<String> {
    let vb = view.as_bytes();
    let mut out = String::new();
    let mut k = open;
    while k <= close {
        let c = vb[k];
        match c {
            b'[' | b']' | b'{' | b'}' | b':' => {
                out.push(char::from(c));
                k += 1;
            }
            b',' => {
                if !matches!(vb.get(skip_ws(vb, k + 1)), Some(b']' | b'}')) {
                    out.push(',');
                }
                k += 1;
            }
            b'\'' | b'"' | b'`' => {
                let (text, end) = string_literal(source, k)?;
                out.push_str(&serde_json::to_string(&text).ok()?);
                k = end;
            }
            c if c.is_ascii_whitespace() => k += 1,
            c if c.is_ascii_digit() || c == b'-' => {
                let end = (k + 1..vb.len())
                    .find(|&j| !(vb[j].is_ascii_digit() || vb[j] == b'.'))
                    .unwrap_or(vb.len());
                let number = &view[k..end];
                if number.parse::<f64>().is_err() || vb.get(end).is_some_and(|&n| is_ident_byte(n))
                {
                    return None;
                }
                out.push_str(number);
                k = end;
            }
            _ => {
                let end = ident_end(vb, k)?;
                let word = &view[k..end];
                if vb.get(skip_ws(vb, end)) == Some(&b':') {
                    out.push_str(&serde_json::to_string(word).ok()?);
                } else if matches!(word, "true" | "false" | "null") {
                    out.push_str(word);
                } else {
                    return None;
                }
                k = end;
            }
        }
    }
    Some(out)
}

/// The entries of a JSON ABI array (module doc); `None` for any other JSON
/// (an artifact object, an array of numbers, objects that are no ABI items).
pub fn read_json_abi(text: &str) -> Option<Vec<AbiEntry>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut entries = Vec::new();
    for item in value.as_array()? {
        let item = item.as_object()?;
        let kind = match item.get("type") {
            None => "function",
            Some(t) => t.as_str()?,
        };
        let kind = match kind {
            "function" => EntryKind::Fn,
            "event" => EntryKind::Event,
            "error" | "constructor" | "fallback" | "receive" => continue,
            _ => return None,
        };
        let name = item.get("name")?.as_str()?;
        if name.is_empty() {
            return None;
        }
        let arity = u32::try_from(item.get("inputs")?.as_array()?.len()).ok()?;
        entries.push(AbiEntry {
            kind,
            name: name.to_string(),
            arity: Some(arity),
        });
    }
    canonical(entries)
}

/// One human-readable fragment: `Some(Some(entry))` for a function or event,
/// `Some(None)` for a fragment kind that declares no entry, `None` for text
/// that is no fragment.
fn fragment(text: &str) -> Option<Option<AbiEntry>> {
    let t = text.trim();
    let kw_end = t
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(t.len());
    let (kw, rest) = t.split_at(kw_end);
    let kind = match kw {
        "function" => EntryKind::Fn,
        "event" => EntryKind::Event,
        "error" | "constructor" | "fallback" | "receive" | "struct" => return Some(None),
        _ => return None,
    };
    let rest = rest.trim_start();
    let name_end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .unwrap_or(rest.len());
    let name = &rest[..name_end];
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let params = rest[name_end..].trim_start();
    if !params.starts_with('(') {
        return None;
    }
    let mut depth = 0usize;
    let mut arity = 0u32;
    let mut piece = false;
    for c in params.chars() {
        match c {
            '(' => {
                if depth > 0 {
                    piece = true;
                }
                depth += 1;
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    arity += u32::from(piece);
                    return Some(Some(AbiEntry {
                        kind,
                        name: name.to_string(),
                        arity: Some(arity),
                    }));
                }
            }
            ',' if depth == 1 => {
                arity += u32::from(piece);
                piece = false;
            }
            c if !c.is_whitespace() => piece = true,
            _ => {}
        }
    }
    None
}

/// `entries` sorted and deduplicated; `None` when there are none.
fn canonical(mut entries: Vec<AbiEntry>) -> Option<Vec<AbiEntry>> {
    entries.sort();
    entries.dedup();
    (!entries.is_empty()).then_some(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evm::scan::code_view;

    fn keys(entries: &[AbiEntry]) -> Vec<String> {
        entries.iter().map(AbiEntry::to_key).collect()
    }

    /// The ABI literal of `const <name> = [..]` in `src`.
    fn read(src: &str) -> Option<Vec<String>> {
        let view = code_view(src, "typescript");
        let start = view.find('[')?;
        read_abi(src, &view, start).map(|e| keys(&e))
    }

    /// The ABI read at the first `needle` of `src`'s code view.
    fn read_at(src: &str, needle: &str) -> Option<Vec<String>> {
        let view = code_view(src, "typescript");
        let start = view.find(needle)?;
        read_abi(src, &view, start).map(|e| keys(&e))
    }

    #[test]
    fn human_readable_entries() {
        // Kina contracts/scripts/demo_trade.js ESCROW_ABI.
        let src = r#"const ESCROW_ABI = [
  "function createTrade(address buyer, uint256 amount) external returns (uint256)",
  "function confirmPayment(uint256 tradeId) external",
  "function releaseFunds(uint256 tradeId) external",
  "function raiseDispute(uint256 tradeId) external",
  "function proposeResolution(uint256 tradeId, uint8 outcome) external",
  "function getTrade(uint256 tradeId) view returns (tuple(uint256 id, address buyer, address seller, uint256 amount, uint8 state, uint256 createdAt, uint256 fundedAt, uint256 timeoutSeconds, uint256 disputeDeadlineAt, uint256 penaltyHoldUntil, uint8 resolutionMethod))",
  "event TradeCreated(uint256 indexed tradeId, address indexed seller, address indexed buyer, uint256 amount, uint256 timeoutSeconds)",
];"#;
        assert_eq!(
            read(src).expect("an ABI"),
            [
                "f:confirmPayment/1",
                "f:createTrade/2",
                "f:getTrade/1",
                "f:proposeResolution/2",
                "f:raiseDispute/1",
                "f:releaseFunds/1",
                "e:TradeCreated/5",
            ]
        );
    }

    #[test]
    fn unnamed_and_tuple_params() {
        let src = "const A = [
  'function deposits(bytes32) view returns (uint256)',
  'function settle(tuple(uint256 a, address b) t, (uint256,address) u, uint256[] v) payable',
  'function ping()',
  'event Tick()',
  'error Nope(uint256 code)',
  'constructor(address owner)',
  'receive() external payable',
  'function deposits(bytes32)',
];";
        assert_eq!(
            read(src).expect("an ABI"),
            ["f:deposits/1", "f:ping/0", "f:settle/3", "e:Tick/0"]
        );
        assert_eq!(
            read("const N = ['deposit', 'release'];"),
            None,
            "names are no ABI"
        );
        assert_eq!(
            read("const N = ['function deposit(bytes32)', 'oops'];"),
            None
        );
        assert_eq!(
            read("const N = ['error Only()'];"),
            None,
            "no function or event"
        );
        assert_eq!(read("const N = [];"), None);
        assert_eq!(read("const N = [a, 'function f()'];"), None);
    }

    #[test]
    fn json_array_literal() {
        let src = r#"const ESCROW_ABI = [
  {"type": "function", "name": "deposit", "inputs": [{"name": "orderId", "type": "bytes32"}], "stateMutability": "payable"},
  {"type": "event", "name": "Released", "inputs": [{"name": "orderId", "type": "bytes32", "indexed": true}, {"name": "seller", "type": "address"}]},
  {"type": "constructor", "inputs": []},
  {"name": "legacy", "inputs": []}
];"#;
        assert_eq!(
            read(src).expect("an ABI"),
            ["f:deposit/1", "f:legacy/0", "e:Released/2"]
        );
    }

    #[test]
    fn object_literal_bare_keys_single_quotes_trailing_commas() {
        // bench/substrate-gap/fixtures/ts-viem-web3-contract-calls/dapp/src/legacy.js.
        let src = "const ESCROW_JSON = [
  { type: 'function', name: 'release', inputs: [{ name: 'orderId', type: 'bytes32' }] },
  { type: 'function', name: 'deposit', inputs: [{ name: 'orderId', type: 'bytes32' }], stateMutability: 'payable' },
  // a comment between items
  { type: 'event', name: 'Released', inputs: [{ name: 'orderId', type: 'bytes32', indexed: true }, { name: 'seller', type: 'address' }], anonymous: false, },
  { 'type': \"function\", \"name\": `legacy`, inputs: [], gas: 21000 },
];";
        assert_eq!(
            read(src).expect("an ABI"),
            ["f:deposit/1", "f:legacy/0", "f:release/1", "e:Released/2"]
        );
        assert_eq!(
            read("const A = [{ type: 'function', name: 'deposit', inputs: [] }];").expect("an ABI"),
            ["f:deposit/0"]
        );
        for no_abi in [
            "const A = [{ type: 'function', name: NAME, inputs: [] }];",
            "const A = [{ ...base, name: 'f', inputs: [] }];",
            "const A = [{ type: 'function', name: `f${x}`, inputs: [] }];",
            "const A = [{ type: 'function', name: 'f', inputs: list }];",
            "const A = [{ type: 'function', name: 'f', inputs: [], gas: 1n }];",
            "const A = [{ name }];",
            "const A = [{ a: 1 }, { b: 2 }];",
        ] {
            assert_eq!(read(no_abi), None, "{no_abi}");
        }
    }

    #[test]
    fn parse_abi_and_as_const() {
        // ts-viem-web3-contract-calls dapp/src/escrow.ts and vault.ts.
        let parsed = "export const escrowAbi = parseAbi([
  'function deposit(bytes32 orderId) payable',
  'function release(bytes32 orderId)',
  'function deposits(bytes32) view returns (uint256)',
  'event Released(bytes32 indexed orderId, address seller)',
]);";
        assert_eq!(
            read_at(parsed, "parseAbi").expect("an ABI"),
            ["f:deposit/1", "f:deposits/1", "f:release/1", "e:Released/2"]
        );
        let structs = "const a = viem.parseAbi([
  'struct Order { bytes32 id; address seller; }',
  'function settle(Order order, uint256 fee)',
]);";
        assert_eq!(read_at(structs, "viem").expect("an ABI"), ["f:settle/2"]);
        let item = "const t = parseAbiItem('event Transfer(address indexed from, address indexed to, uint256 value)');";
        assert_eq!(
            read_at(item, "parseAbiItem").expect("an ABI"),
            ["e:Transfer/3"]
        );
        let item_array = "const t = parseAbiItem([
  'function settle(Order order)',
  'function other()',
  'struct Order { bytes32 id; }',
]);";
        assert_eq!(
            read_at(item_array, "parseAbiItem").expect("an ABI"),
            ["f:settle/1"]
        );
        let as_const = "export const vaultAbi = [
  { type: 'function', name: 'withdraw', stateMutability: 'nonpayable', inputs: [{ name: 'amount', type: 'uint256' }], outputs: [] },
  { type: 'function', name: 'release', stateMutability: 'nonpayable', inputs: [{ name: 'id', type: 'bytes32' }], outputs: [] },
] as const satisfies Abi;";
        assert_eq!(
            read(as_const).expect("an ABI"),
            ["f:release/1", "f:withdraw/1"]
        );
        for no_abi in [
            "const a = parseAbi(signatures);",
            "const a = parseAbi(['deposit']);",
            "const a = parseAbiItem('error Nope()');",
            "const a = parseAbiItem(sig);",
            "const a = parseAbiParameters(['uint256 x']);",
        ] {
            let at = no_abi.find('p').expect("a call");
            let view = code_view(no_abi, "typescript");
            assert_eq!(read_abi(no_abi, &view, at), None, "{no_abi}");
        }
    }

    #[test]
    fn read_json_abi_rejects_non_abi_arrays() {
        for text in [
            "[]",
            "[1, 2]",
            r#"[{"a": 1}]"#,
            r#"[{"type": "function", "inputs": []}]"#,
            r#"[{"type": "function", "name": "f"}]"#,
            r#"[{"type": "weird", "name": "f", "inputs": []}]"#,
            r#"[{"type": "constructor", "inputs": []}]"#,
            r#"{"abi": [{"type": "function", "name": "f", "inputs": []}]}"#,
            "not json",
        ] {
            assert_eq!(read_json_abi(text), None, "{text}");
        }
        let ok =
            read_json_abi(r#"[{"type":"function","name":"f","inputs":[{},{}]}]"#).expect("an ABI");
        assert_eq!(keys(&ok), ["f:f/2"]);
    }
}
