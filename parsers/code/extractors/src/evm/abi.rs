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
//!   objects in strict JSON (quoted keys), each with a `type` (`function`,
//!   `event`, `error`, `constructor`, `fallback`, `receive`, or absent for a
//!   function), a string `name` and an `inputs` array whose length is the
//!   arity. Any other item makes the array no ABI.
//!
//! An ABI needs at least one function or event. Entries are sorted and
//! deduplicated (functions first, then events, each by name and arity), so
//! two spellings of one ABI give one entry list.

use glia_code_domain::evm::{AbiEntry, EntryKind};

use super::scan::{matching, split_args, string_literal};

/// The entries of the ABI array literal whose `[` is at `start` (the same
/// offset in `source` and its code `view`); `None` when the literal is no ABI.
pub(crate) fn read_abi(source: &str, view: &str, start: usize) -> Option<Vec<AbiEntry>> {
    if view.as_bytes().get(start) != Some(&b'[') {
        return None;
    }
    let close = matching(view, start)?;
    let items = split_args(view, start, close);
    if items.is_empty() {
        return None;
    }
    let vb = view.as_bytes();
    if items
        .iter()
        .all(|&(s, _)| matches!(vb[s], b'\'' | b'"' | b'`'))
    {
        let mut entries = Vec::new();
        for &(s, e) in &items {
            let (text, end) = string_literal(source, s)?;
            if end != e {
                return None;
            }
            if let Some(entry) = fragment(&text)? {
                entries.push(entry);
            }
        }
        return canonical(entries);
    }
    if items.iter().all(|&(s, _)| vb[s] == b'{') {
        return read_json_abi(source.get(start..=close)?);
    }
    None
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
        "error" | "constructor" | "fallback" | "receive" => return Some(None),
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
        let unquoted = "const A = [{ type: 'function', name: 'deposit', inputs: [] }];";
        assert_eq!(read(unquoted), None, "strict JSON only");
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
