//! The EVM contract vocabulary (DT.10a): one spelling of the qnames, scopes,
//! name key, ABI entries and cells that the Solidity parser (the server side),
//! the TypeScript / Go client readers and the EVM resolver share. The graph
//! crate has no serde_json, so every cell it reads is read here.
//!
//! **Procedures** (the server side). A contract function an ABI client can
//! reach (an `external` / `public` function, an interface member, a public
//! state-variable getter) is an RPC_PROCEDURE `evm:<Contract>.<member>`
//! ([`procedure_qname`] / [`split_procedure`]). The `evm:` prefix keeps it
//! away from the JSON-RPC family's `rpc:` (RpcStackResolver and the owner
//! pass key on that prefix). Its facts are a JSON CODE cell
//! ([`procedure_cell`] / [`read_procedure_cell`]):
//!
//! ```text
//! {"via":"evm","arities":[1,2],"mutability":"nonpayable","getter":false,"abstract":false}
//! ```
//!
//! **Clients.** A contract call site is an RPC_CALL `evm_call:<scope>.<member>`
//! and an event subscription an EVENT_HANDLER `evm_event:<scope>.<event>`
//! ([`call_qname`] / [`event_qname`] / [`split_client`]). The [`Scope`] says
//! what the site knows of the contract: its name (`Escrow`), the ABI binding it
//! was built from (`abi:ESCROW_ABI`) or only the receiver it was called on
//! (`recv:escrow`). Its facts are an ENDPOINT_HIT cell
//! ([`client_cell`] / [`read_client_cells`]):
//!
//! ```text
//! {"via":"evm","lib":"ethers","args":[2],"abi":["e:Released/2","f:deposit/1"]}
//! ```
//!
//! `abi` is omitted when the site's ABI is unknown. There is no `file` and no
//! `host` key, so every file-keyed ENDPOINT_HIT reader skips it.
//!
//! **Cross-file ABIs (DT.10g).** A site whose ABI binding another file
//! declares (a TS import, a Go package constant of another file) carries an
//! [`AbiRef`] (`abi_ref`: the exported name and the import specifier, `"."`
//! for the Go package) and no `abi`. The engine's post-cache join fills `abi`
//! with the declaring file's entries and `abi_from` with that file, so the
//! resolver identifies the call by ABI ([`ClientHit`], [`client_hit_cell`],
//! [`read_client_hits`]):
//!
//! ```text
//! {"via":"evm","lib":"ethers","args":[1],"abi":["f:deposit/1","f:release/1"],
//!  "abi_ref":{"name":"SHOP_ABI","from":"./abi"},"abi_from":"src/abi.ts"}
//! ```
//!
//! **Names.** [`name_key`] is the one normal form a client hint (an ABI const,
//! a receiver, an address const) and a contract name are compared in.
//!
//! **Byte stability.** Both cells are written from derived structs in the
//! field order above, so the JSON never depends on a map's order; the writers
//! sort and dedup every list (`arities`, `args`, `abi`), so two writers that
//! saw the same facts in another order write the same bytes. Readers are
//! strict about the type of every key they know and ignore keys they do not
//! (a later stamp may add one, as CB.24 adds `hosts` to a client hit).

use glia_core::{Cell, CellPayload};
use serde::{Deserialize, Serialize};

use crate::cell_type;

/// The qname prefix of a Solidity procedure: `evm:<Contract>.<member>`.
pub const PROCEDURE_PREFIX: &str = "evm:";
/// The qname prefix of a client contract call: `evm_call:<scope>.<member>`.
pub const CALL_PREFIX: &str = "evm_call:";
/// The qname prefix of a client event subscription: `evm_event:<scope>.<event>`.
pub const EVENT_PREFIX: &str = "evm_event:";
/// The `via` of every evm cell.
pub const VIA: &str = "evm";
/// The mutability a function without a `state_mutability` keyword has.
pub const NONPAYABLE: &str = "nonpayable";

/// The scope-key prefix of an ABI binding.
const ABI_SCOPE: &str = "abi:";
/// The scope-key prefix of a bare receiver.
const RECV_SCOPE: &str = "recv:";

// ---- scopes and qnames -------------------------------------------------------

/// What a client site knows of the contract it calls.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    /// The contract by name (a hardhat factory, a TypeChain factory, an
    /// artifact import): key `Escrow`.
    Contract(String),
    /// The ABI binding the client was built from (a const, an import, an
    /// inline literal's binding): key `abi:ESCROW_ABI`.
    Abi(String),
    /// Only the receiver the call was made on: key `recv:escrow`.
    Recv(String),
}

impl Scope {
    /// The scope as it is spelled inside a client qname.
    pub fn key(&self) -> String {
        match self {
            Scope::Contract(n) => n.clone(),
            Scope::Abi(n) => format!("{ABI_SCOPE}{n}"),
            Scope::Recv(n) => format!("{RECV_SCOPE}{n}"),
        }
    }

    /// The name the scope carries (the contract, the ABI binding or the
    /// receiver), without its key prefix.
    pub fn name(&self) -> &str {
        match self {
            Scope::Contract(n) | Scope::Abi(n) | Scope::Recv(n) => n,
        }
    }

    /// Read a scope key back. `None` for an empty name or a name holding the
    /// member separator `.`; a contract name never holds a `:`.
    pub fn parse(key: &str) -> Option<Scope> {
        let scope = if let Some(n) = key.strip_prefix(ABI_SCOPE) {
            Scope::Abi(n.to_string())
        } else if let Some(n) = key.strip_prefix(RECV_SCOPE) {
            Scope::Recv(n.to_string())
        } else if key.contains(':') {
            return None;
        } else {
            Scope::Contract(key.to_string())
        };
        let name = scope.name();
        (!name.is_empty() && !name.contains('.')).then_some(scope)
    }
}

/// Which client side a qname names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClientSide {
    /// A contract call: an RPC_CALL `evm_call:..`.
    Call,
    /// An event subscription: an EVENT_HANDLER `evm_event:..`.
    Event,
}

impl ClientSide {
    /// The qname prefix of this side.
    pub fn prefix(self) -> &'static str {
        match self {
            ClientSide::Call => CALL_PREFIX,
            ClientSide::Event => EVENT_PREFIX,
        }
    }
}

/// `evm:<contract>.<member>`.
pub fn procedure_qname(contract: &str, member: &str) -> String {
    format!("{PROCEDURE_PREFIX}{contract}.{member}")
}

/// `evm_call:<scope key>.<member>`.
pub fn call_qname(scope: &Scope, member: &str) -> String {
    format!("{CALL_PREFIX}{}.{member}", scope.key())
}

/// `evm_event:<scope key>.<event>`.
pub fn event_qname(scope: &Scope, event: &str) -> String {
    format!("{EVENT_PREFIX}{}.{event}", scope.key())
}

/// `(contract, member)` of a procedure qname; `None` for any other qname.
pub fn split_procedure(qname: &str) -> Option<(&str, &str)> {
    let rest = qname.strip_prefix(PROCEDURE_PREFIX)?;
    let (contract, member) = rest.rsplit_once('.')?;
    (!contract.is_empty() && !member.is_empty() && !contract.contains(['.', ':']))
        .then_some((contract, member))
}

/// `(side, scope, member)` of a client qname; the member is the text after
/// the last `.`. `None` for any other qname.
pub fn split_client(qname: &str) -> Option<(ClientSide, Scope, &str)> {
    let (side, rest) = if let Some(rest) = qname.strip_prefix(CALL_PREFIX) {
        (ClientSide::Call, rest)
    } else {
        (ClientSide::Event, qname.strip_prefix(EVENT_PREFIX)?)
    };
    let (key, member) = rest.rsplit_once('.')?;
    if member.is_empty() {
        return None;
    }
    Some((side, Scope::parse(key)?, member))
}

// ---- the name key -------------------------------------------------------------

/// The suffixes [`name_key`] strips, tried in this order; one at most.
const NAME_SUFFIXES: &[&str] = &["metadata", "abi", "contract", "address", "addr", "instance"];

/// The fewest characters [`name_key`] leaves after stripping a suffix.
const NAME_FLOOR: usize = 3;

/// The normal form a client hint and a contract name are compared in: the
/// ASCII alphanumerics of `ident`, lower-cased, then ONE trailing suffix of
/// [`NAME_SUFFIXES`] stripped when at least three characters remain.
/// `ESCROW_ABI`, `escrowContract`, `EscrowMetaData` and `ESCROW_ADDRESS` are
/// all `escrow`; `escrowManagerABI` is `escrowmanager`; `ERC20_ABI` is
/// `erc20`; `contract` stays `contract`.
pub fn name_key(ident: &str) -> String {
    let flat: String = ident
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    for suffix in NAME_SUFFIXES {
        if let Some(stem) = flat.strip_suffix(suffix)
            && stem.len() >= NAME_FLOOR
        {
            return stem.to_string();
        }
    }
    flat
}

// ---- ABI entries ----------------------------------------------------------------

/// What an ABI entry declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryKind {
    /// A function: key tag `f`.
    Fn,
    /// An event: key tag `e`.
    Event,
}

impl EntryKind {
    fn tag(self) -> &'static str {
        match self {
            EntryKind::Fn => "f",
            EntryKind::Event => "e",
        }
    }
}

/// One function or event of a client's ABI: key `f:deposit/1`, `e:Released/2`,
/// or `f:deposit/?` when the arity is unknown. A cell lists entries by key, in
/// key (byte) order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AbiEntry {
    pub kind: EntryKind,
    pub name: String,
    /// The parameter count; `None` when the source does not spell it.
    pub arity: Option<u32>,
}

impl AbiEntry {
    /// `f:<name>/<arity>` or `e:<name>/<arity>`, `?` for an unknown arity.
    pub fn to_key(&self) -> String {
        let arity = self.arity.map_or_else(|| "?".to_string(), |a| a.to_string());
        format!("{}:{}/{arity}", self.kind.tag(), self.name)
    }

    /// Read a key [`AbiEntry::to_key`] wrote; `None` for anything else.
    pub fn from_key(key: &str) -> Option<AbiEntry> {
        let (tag, rest) = key.split_once(':')?;
        let kind = match tag {
            "f" => EntryKind::Fn,
            "e" => EntryKind::Event,
            _ => return None,
        };
        let (name, arity) = rest.rsplit_once('/')?;
        if name.is_empty() {
            return None;
        }
        let arity = match arity {
            "?" => None,
            n => Some(n.parse::<u32>().ok()?),
        };
        Some(AbiEntry { kind, name: name.to_string(), arity })
    }
}

/// Sorted, deduplicated copy of `v`.
fn canonical<T: Ord + Clone>(v: &[T]) -> Vec<T> {
    let mut out = v.to_vec();
    out.sort();
    out.dedup();
    out
}

/// The text of a JSON (or text) payload.
fn payload_text(cell: &Cell) -> Option<&str> {
    match &cell.payload {
        CellPayload::Json(s) | CellPayload::Text(s) => Some(s),
        CellPayload::Bytes(_) => None,
    }
}

// ---- the procedure cell -----------------------------------------------------------

/// The facts of one Solidity procedure.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProcedureFacts {
    /// The parameter count of every declaration (overloads share one
    /// procedure), ascending and deduplicated in the cell.
    pub arities: Vec<u32>,
    /// The `state_mutability` keyword of the first declaration
    /// (`payable` / `view` / `pure` / ..), [`NONPAYABLE`] when it has none.
    pub mutability: String,
    /// A public state variable's getter.
    pub getter: bool,
    /// An interface member or a function without a body.
    pub abstract_: bool,
}

#[derive(Serialize)]
struct ProcedureOut<'a> {
    via: &'a str,
    arities: Vec<u32>,
    mutability: &'a str,
    getter: bool,
    #[serde(rename = "abstract")]
    abstract_: bool,
}

#[derive(Deserialize)]
struct ProcedureIn {
    via: String,
    arities: Vec<u32>,
    mutability: String,
    getter: bool,
    #[serde(rename = "abstract")]
    abstract_: bool,
}

/// The CODE cell of a procedure:
/// `{"via":"evm","arities":[..],"mutability":"..","getter":..,"abstract":..}`.
pub fn procedure_cell(facts: &ProcedureFacts) -> Cell {
    let out = ProcedureOut {
        via: VIA,
        arities: canonical(&facts.arities),
        mutability: &facts.mutability,
        getter: facts.getter,
        abstract_: facts.abstract_,
    };
    // A derived struct of strings, integers and booleans always serialises.
    let json = serde_json::to_string(&out).unwrap_or_default();
    Cell { kind: cell_type::CODE, payload: CellPayload::Json(json) }
}

/// The facts of the first `via:evm` CODE cell in `cells`; `None` when there is
/// none (a METHOD's CODE is its source text, which never reads as one).
pub fn read_procedure_cell(cells: &[Cell]) -> Option<ProcedureFacts> {
    cells.iter().filter(|c| c.kind == cell_type::CODE).find_map(|c| {
        let p: ProcedureIn = serde_json::from_str(payload_text(c)?).ok()?;
        (p.via == VIA).then_some(ProcedureFacts {
            arities: p.arities,
            mutability: p.mutability,
            getter: p.getter,
            abstract_: p.abstract_,
        })
    })
}

// ---- the client cell ----------------------------------------------------------------

/// The facts of one client site group (one cell per distinct `(lib, abi)`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientFacts {
    /// The client library (`ethers`, `viem`, `web3`, `hardhat`, `typechain`,
    /// `geth`, ..).
    pub lib: String,
    /// The argument counts the sites pass, ascending and deduplicated in the
    /// cell; empty when no site's count is known.
    pub args: Vec<u32>,
    /// The ABI the client was built from, as [`AbiEntry`] keys sorted and
    /// deduplicated in the cell; `None` when unknown.
    pub abi: Option<Vec<String>>,
}

/// The ABI binding a client site names when another file declares it
/// (DT.10g): `name` is the binding as the declaring file exports it (a TS
/// `import { Y as X }` names `Y`, a default import `default`) and `from` the
/// specifier it is imported by (`./abi`), or `"."` for a constant of the
/// site's Go package.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
pub struct AbiRef {
    pub name: String,
    pub from: String,
}

/// The `from` of a Go [`AbiRef`]: the site's own package directory.
pub const GO_PACKAGE: &str = ".";

/// One client cell whole (DT.10g): its [`ClientFacts`] plus the cross-file
/// ABI fields, which [`read_client_cells`] leaves out.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientHit {
    pub facts: ClientFacts,
    /// The ABI binding another file declares; `None` when the site's file
    /// binds its ABI itself, or none is known.
    pub abi_ref: Option<AbiRef>,
    /// The repo-relative file the engine's join read `facts.abi` from;
    /// `None` until a join fills it.
    pub abi_from: Option<String>,
}

#[derive(Serialize)]
struct ClientOut<'a> {
    via: &'a str,
    lib: &'a str,
    args: Vec<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    abi: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    abi_ref: Option<&'a AbiRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    abi_from: Option<&'a str>,
}

#[derive(Deserialize)]
struct ClientIn {
    via: String,
    lib: String,
    args: Vec<u32>,
    #[serde(default)]
    abi: Option<Vec<String>>,
    #[serde(default)]
    abi_ref: Option<AbiRef>,
    #[serde(default)]
    abi_from: Option<String>,
}

/// The ENDPOINT_HIT cell of a client site group:
/// `{"via":"evm","lib":"..","args":[..][,"abi":[..]]}`.
pub fn client_cell(facts: &ClientFacts) -> Cell {
    client_hit_cell(&ClientHit { facts: facts.clone(), abi_ref: None, abi_from: None })
}

/// The ENDPOINT_HIT cell of a client site group with its cross-file ABI
/// fields: `{"via":"evm","lib":"..","args":[..][,"abi":[..]][,"abi_ref":{"name":"..","from":".."}][,"abi_from":".."]}`.
/// Without either field it is [`client_cell`]'s bytes.
pub fn client_hit_cell(hit: &ClientHit) -> Cell {
    let out = ClientOut {
        via: VIA,
        lib: &hit.facts.lib,
        args: canonical(&hit.facts.args),
        abi: hit.facts.abi.as_deref().map(canonical),
        abi_ref: hit.abi_ref.as_ref(),
        abi_from: hit.abi_from.as_deref(),
    };
    // A derived struct of strings and integers always serialises.
    let json = serde_json::to_string(&out).unwrap_or_default();
    Cell { kind: cell_type::ENDPOINT_HIT, payload: CellPayload::Json(json) }
}

/// One [`ClientFacts`] per `via:evm` ENDPOINT_HIT cell of `cells`, in cell
/// order; every other cell (an HTTP hit, a channel hit) is skipped.
pub fn read_client_cells(cells: &[Cell]) -> Vec<ClientFacts> {
    cells.iter().filter_map(read_client_hit).map(|h| h.facts).collect()
}

/// One [`ClientHit`] per `via:evm` ENDPOINT_HIT cell of `cells`, in cell
/// order, as [`read_client_cells`] reads them.
pub fn read_client_hits(cells: &[Cell]) -> Vec<ClientHit> {
    cells.iter().filter_map(read_client_hit).collect()
}

/// The [`ClientHit`] of one cell; `None` for any cell that is no `via:evm`
/// ENDPOINT_HIT.
pub fn read_client_hit(cell: &Cell) -> Option<ClientHit> {
    if cell.kind != cell_type::ENDPOINT_HIT {
        return None;
    }
    let p: ClientIn = serde_json::from_str(payload_text(cell)?).ok()?;
    (p.via == VIA).then_some(ClientHit {
        facts: ClientFacts { lib: p.lib, args: p.args, abi: p.abi },
        abi_ref: p.abi_ref,
        abi_from: p.abi_from,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_of(cell: &Cell) -> &str {
        match &cell.payload {
            CellPayload::Json(s) => s,
            other => panic!("not a Json payload: {other:?}"),
        }
    }

    #[test]
    fn evm_scope_keys_round_trip() {
        let scopes = [
            (Scope::Contract("Escrow".into()), "Escrow"),
            (Scope::Abi("ESCROW_ABI".into()), "abi:ESCROW_ABI"),
            (Scope::Recv("escrow".into()), "recv:escrow"),
        ];
        for (scope, key) in scopes {
            assert_eq!(scope.key(), key);
            assert_eq!(Scope::parse(key), Some(scope));
        }
        assert_eq!(Scope::parse(""), None);
        assert_eq!(Scope::parse("abi:"), None);
        assert_eq!(Scope::parse("recv:a.b"), None);
        assert_eq!(Scope::parse("x:Escrow"), None, "a contract name holds no `:`");
    }

    #[test]
    fn evm_procedure_qnames_round_trip() {
        let q = procedure_qname("Escrow", "deposit");
        assert_eq!(q, "evm:Escrow.deposit");
        assert_eq!(split_procedure(&q), Some(("Escrow", "deposit")));
        assert_eq!(split_procedure("rpc:Escrow.deposit"), None);
        assert_eq!(split_procedure("evm:Escrow"), None);
        assert_eq!(split_procedure("evm:.deposit"), None);
        assert_eq!(split_procedure("evm:Escrow."), None);
        assert_eq!(split_procedure("evm_call:Escrow.deposit"), None);
        assert_eq!(split_procedure("contracts::Escrow::Escrow::deposit"), None);
    }

    #[test]
    fn evm_split_client_reads_every_scope() {
        let cases = [
            (Scope::Contract("Escrow".into()), "evm_call:Escrow.deposit"),
            (Scope::Abi("ESCROW_ABI".into()), "evm_call:abi:ESCROW_ABI.deposit"),
            (Scope::Recv("escrow".into()), "evm_call:recv:escrow.deposit"),
        ];
        for (scope, want) in cases {
            let q = call_qname(&scope, "deposit");
            assert_eq!(q, want);
            assert_eq!(split_client(&q), Some((ClientSide::Call, scope.clone(), "deposit")));
            let e = event_qname(&scope, "Released");
            assert!(e.starts_with(EVENT_PREFIX), "{e}");
            assert_eq!(split_client(&e), Some((ClientSide::Event, scope, "Released")));
        }
        assert_eq!(ClientSide::Call.prefix(), CALL_PREFIX);
        assert_eq!(ClientSide::Event.prefix(), EVENT_PREFIX);
        assert_eq!(split_client("evm:Escrow.deposit"), None, "a procedure is no client");
        assert_eq!(split_client("rpc_call:user.list"), None);
        assert_eq!(split_client("evm_call:Escrow"), None);
        assert_eq!(split_client("evm_call:Escrow."), None);
        assert_eq!(split_client("evm_event:.Released"), None);
    }

    #[test]
    fn evm_name_key_normalises_hints_and_contracts() {
        for (ident, want) in [
            ("ESCROW_ABI", "escrow"),
            ("escrowContract", "escrow"),
            ("EscrowMetaData", "escrow"),
            ("ESCROW_ADDRESS", "escrow"),
            ("escrowAddr", "escrow"),
            ("escrowInstance", "escrow"),
            ("Escrow", "escrow"),
            ("escrowManagerABI", "escrowmanager"),
            ("ERC20_ABI", "erc20"),
            ("contract", "contract"),
            ("abi", "abi"),
            ("myABI", "myabi"),
            ("ercABI", "erc"),
            ("tokenAddressABI", "tokenaddress"),
        ] {
            assert_eq!(name_key(ident), want, "{ident}");
        }
    }

    #[test]
    fn evm_abi_entry_keys_round_trip() {
        let entries = [
            (AbiEntry { kind: EntryKind::Fn, name: "deposit".into(), arity: Some(1) }, "f:deposit/1"),
            (AbiEntry { kind: EntryKind::Event, name: "Released".into(), arity: Some(2) }, "e:Released/2"),
            (AbiEntry { kind: EntryKind::Fn, name: "deposit".into(), arity: None }, "f:deposit/?"),
        ];
        for (entry, key) in entries {
            assert_eq!(entry.to_key(), key);
            assert_eq!(AbiEntry::from_key(key), Some(entry));
        }
        for bad in ["deposit/1", "x:deposit/1", "f:/1", "f:deposit", "f:deposit/-1", "f:deposit/a"] {
            assert_eq!(AbiEntry::from_key(bad), None, "{bad}");
        }
    }

    #[test]
    fn evm_procedure_cell_writes_and_reads() {
        let facts = ProcedureFacts {
            arities: vec![2, 1, 2],
            mutability: NONPAYABLE.into(),
            getter: false,
            abstract_: false,
        };
        let cell = procedure_cell(&facts);
        assert_eq!(cell.kind, cell_type::CODE);
        assert_eq!(
            json_of(&cell),
            r#"{"via":"evm","arities":[1,2],"mutability":"nonpayable","getter":false,"abstract":false}"#
        );
        let source = Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Text("function deposit() external {}".into()),
        };
        let read = read_procedure_cell(&[source, cell]).expect("evm CODE cell");
        assert_eq!(read, ProcedureFacts { arities: vec![1, 2], ..facts });
        let foreign = Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Json(
                r#"{"via":"rpc","arities":[1],"mutability":"view","getter":true,"abstract":true}"#.into(),
            ),
        };
        assert_eq!(read_procedure_cell(&[foreign]), None);
        let mistyped = Cell {
            kind: cell_type::CODE,
            payload: CellPayload::Json(
                r#"{"via":"evm","arities":"1","mutability":"view","getter":true,"abstract":true}"#.into(),
            ),
        };
        assert_eq!(read_procedure_cell(&[mistyped]), None);
    }

    #[test]
    fn evm_client_cell_writes_and_reads_and_skips_other_hits() {
        let facts = ClientFacts {
            lib: "ethers".into(),
            args: vec![2],
            abi: Some(vec!["f:deposit/1".into(), "e:Released/2".into(), "f:deposit/1".into()]),
        };
        let cell = client_cell(&facts);
        assert_eq!(cell.kind, cell_type::ENDPOINT_HIT);
        assert_eq!(
            json_of(&cell),
            r#"{"via":"evm","lib":"ethers","args":[2],"abi":["e:Released/2","f:deposit/1"]}"#
        );
        let unknown = client_cell(&ClientFacts { lib: "geth".into(), args: vec![], abi: None });
        assert_eq!(json_of(&unknown), r#"{"via":"evm","lib":"geth","args":[]}"#);

        let http = Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(
                r#"{"method":"GET","path":"/a","file":"a.ts","line":1,"col":1,"confidence":"strong"}"#
                    .into(),
            ),
        };
        let ws = Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(r#"{"via":"ws","host":"x:1"}"#.into()),
        };
        // A later stamp's key (CB.24's `hosts`) is ignored, not a rejection.
        let stamped = Cell {
            kind: cell_type::ENDPOINT_HIT,
            payload: CellPayload::Json(r#"{"via":"evm","lib":"viem","args":[0,1],"hosts":["a"]}"#.into()),
        };
        let code = procedure_cell(&ProcedureFacts::default());
        let read = read_client_cells(&[http, cell, ws, code, unknown, stamped]);
        assert_eq!(
            read,
            vec![
                ClientFacts {
                    lib: "ethers".into(),
                    args: vec![2],
                    abi: Some(vec!["e:Released/2".into(), "f:deposit/1".into()]),
                },
                ClientFacts { lib: "geth".into(), args: vec![], abi: None },
                ClientFacts { lib: "viem".into(), args: vec![0, 1], abi: None },
            ]
        );
    }

    /// DT.10g: a client cell with a cross-file ABI reference writes `abi_ref`
    /// (and, once joined, `abi_from`) after `abi`, reads back whole through
    /// [`read_client_hits`], and [`read_client_cells`] keeps reading its facts.
    #[test]
    fn evm_client_hit_cell_round_trips_the_abi_ref_and_join() {
        let abi_ref = AbiRef { name: "SHOP_ABI".into(), from: "./abi".into() };
        let unjoined = ClientHit {
            facts: ClientFacts { lib: "ethers".into(), args: vec![1], abi: None },
            abi_ref: Some(abi_ref.clone()),
            abi_from: None,
        };
        let cell = client_hit_cell(&unjoined);
        assert_eq!(
            json_of(&cell),
            r#"{"via":"evm","lib":"ethers","args":[1],"abi_ref":{"name":"SHOP_ABI","from":"./abi"}}"#
        );
        assert_eq!(read_client_hits(std::slice::from_ref(&cell)), vec![unjoined.clone()]);
        let joined = ClientHit {
            facts: ClientFacts {
                abi: Some(vec!["f:release/1".into(), "f:deposit/1".into()]),
                ..unjoined.facts.clone()
            },
            abi_from: Some("src/abi.ts".into()),
            ..unjoined
        };
        let cell = client_hit_cell(&joined);
        assert_eq!(
            json_of(&cell),
            r#"{"via":"evm","lib":"ethers","args":[1],"abi":["f:deposit/1","f:release/1"],"abi_ref":{"name":"SHOP_ABI","from":"./abi"},"abi_from":"src/abi.ts"}"#
        );
        let read = read_client_hits(std::slice::from_ref(&cell));
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].abi_ref, Some(abi_ref));
        assert_eq!(read[0].abi_from.as_deref(), Some("src/abi.ts"));
        assert_eq!(
            read_client_cells(&[cell])[0].abi,
            Some(vec!["f:deposit/1".to_string(), "f:release/1".to_string()])
        );
        let plain = ClientFacts { lib: "geth".into(), args: vec![], abi: None };
        assert_eq!(
            client_hit_cell(&ClientHit { facts: plain.clone(), ..ClientHit::default() }),
            client_cell(&plain),
            "no cross-file field: client_cell's bytes"
        );
    }
}
