//! EVM contract clients (DT.10c): the cross-cutting extractor that reads a
//! file's contract calls and event subscriptions and mints the client side of
//! `glia_code_domain::evm` (DT.10a), which `resolver:evm` (DT.10b) pairs to
//! the Solidity procedures and events.
//!
//! A language reader (TypeScript / JavaScript ethers: [`ts`]) scans the
//! file's code view ([`scan::code_view`]: comments, regex literals and the
//! contents of string and template literals blanked, offsets kept) and
//! returns one [`Site`] per call or subscription it can name. [`mint`] then
//! groups the sites by `(side, scope, member)`, one node per group:
//! - a call is an RPC_CALL `evm_call:<scope>.<member>` and a subscription an
//!   EVENT_HANDLER `evm_event:<scope>.<event>`, Confidence::Medium, nav name
//!   `<scope key>.<member>`, nav parent the file MODULE;
//! - its cells are one `via:evm` ENDPOINT_HIT ([`evm::client_cell`]) per
//!   distinct `(lib, abi)` of its sites, carrying those sites' argument
//!   counts;
//! - every site is an [`Anchor`], so the engine's A5.8 pass gives the node a
//!   first-site POSITION and its owner edges (`function -USES-> call`,
//!   `subscription -HANDLED_BY-> function`, module CONTAINS when no function
//!   holds a site).
//!
//! The qname carries no file, so two files that bind one ABI name share one
//! client node; the resolver unions its cells and pairs it only when every
//! variant agrees.
//!
//! Marker, once per file with a site or a dynamic construction ([`EvmNodes::markers`]):
//! `[evm-client] lang=<ts|go> calls=<c> events=<e> sites=<s> (ethers=<n>
//! hardhat=<n> typechain=<n> viem=<n> web3=<n> geth=<n>) abis=<a>
//! not_in_abi=<i> dynamic=<d> path=<file>`; c / e count the file's nodes, s
//! its sites (split by library in the parenthesis), a the ABI bindings read.
//! Grep `^\[evm-client\] `.

mod abi;
mod scan;
mod ts;

use std::collections::BTreeMap;

use glia_code_domain::evm::{self, AbiEntry, ClientFacts, ClientSide, Scope};
use glia_code_domain::{CodeNav, GRAPH_TYPE, node_kind};
use glia_core::{Confidence, Node, NodeId, RepoId};

use crate::anchor::{self, Anchor};

pub use abi::read_json_abi;

/// The language tags the TypeScript / JavaScript reader scans.
const TS_FAMILY: &[&str] = &["typescript", "react", "angular", "vue"];

/// How many client libraries the marker counts.
const N_LIBS: usize = 6;

/// The client libraries a site names, in the marker's order.
const LIBS: [&str; N_LIBS] = ["ethers", "hardhat", "typechain", "viem", "web3", "geth"];

/// What [`extract_evm_clients`] read from one file.
#[derive(Default)]
pub struct EvmNodes {
    pub nodes: Vec<Node>,
    pub nav: CodeNav,
    /// One per site, for the engine's A5.8 anchor pass.
    pub anchors: Vec<Anchor>,
    pub counts: EvmCounts,
}

/// The `[evm-client]` marker's counts for one file.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EvmCounts {
    /// The reader's language in the marker (`ts`); empty when none ran.
    pub lang: &'static str,
    /// RPC_CALL nodes minted.
    pub calls: usize,
    /// EVENT_HANDLER nodes minted.
    pub events: usize,
    /// Sites read (every site is one anchor).
    pub sites: usize,
    /// Sites per library, in the marker's order: ethers, hardhat, typechain,
    /// viem, web3, geth.
    pub by_lib: [usize; N_LIBS],
    /// ABI bindings read.
    pub abis: usize,
    /// Members and events a known ABI does not list.
    pub not_in_abi: usize,
    /// Contract constructions whose ABI argument could not be read.
    pub dynamic: usize,
}

/// One call or subscription a language reader found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Site {
    pub(crate) side: ClientSide,
    pub(crate) scope: Scope,
    /// The called member or the subscribed event.
    pub(crate) member: String,
    /// The site's depth-0 argument count; `None` when it has none to give
    /// (an event subscription).
    pub(crate) args: Option<u32>,
    /// The client library, one of [`LIBS`] (the cell's `lib`).
    pub(crate) lib: &'static str,
    /// The ABI the receiver was built from; `None` when unknown.
    pub(crate) abi: Option<Vec<AbiEntry>>,
    /// The byte offset the anchor's line is read at.
    pub(crate) at: usize,
}

/// The EVM client nodes of one file of language `lang` (module doc). A
/// language without a reader gives none.
pub fn extract_evm_clients(source: &str, lang: &str, module_id: NodeId, repo: RepoId) -> EvmNodes {
    if !TS_FAMILY.contains(&lang) {
        return EvmNodes::default();
    }
    let view = scan::code_view(source, lang);
    let (sites, ts_counts) = ts::scan(source, &view);
    let mut out = mint(&sites, source, module_id, repo);
    out.counts.lang = "ts";
    out.counts.abis = ts_counts.abis;
    out.counts.not_in_abi = ts_counts.not_in_abi;
    out.counts.dynamic = ts_counts.dynamic;
    out
}

impl EvmNodes {
    /// The file's marker lines (module doc): the `[evm-client]` line when the
    /// file had a site or a dynamic construction, else none.
    pub fn markers(&self, path: &str) -> Vec<String> {
        let c = &self.counts;
        if c.sites + c.dynamic == 0 {
            return Vec::new();
        }
        let libs: Vec<String> = LIBS
            .iter()
            .zip(c.by_lib)
            .map(|(lib, n)| format!("{lib}={n}"))
            .collect();
        vec![format!(
            "[evm-client] lang={} calls={} events={} sites={} ({}) abis={} not_in_abi={} dynamic={} path={path}",
            c.lang,
            c.calls,
            c.events,
            c.sites,
            libs.join(" "),
            c.abis,
            c.not_in_abi,
            c.dynamic
        )]
    }
}

/// One node per `(side, scope key, member)` group of `sites`, with one cell
/// per distinct `(lib, abi)` and one anchor per site (module doc).
fn mint(sites: &[Site], source: &str, module_id: NodeId, repo: RepoId) -> EvmNodes {
    let mut groups: BTreeMap<(ClientSide, String, &str), Vec<&Site>> = BTreeMap::new();
    for site in sites {
        groups
            .entry((site.side, site.scope.key(), &site.member))
            .or_default()
            .push(site);
    }
    let mut out = EvmNodes::default();
    for ((side, key, member), group) in groups {
        let scope = &group[0].scope;
        let (kind, qname) = match side {
            ClientSide::Call => (node_kind::RPC_CALL, evm::call_qname(scope, member)),
            ClientSide::Event => (node_kind::EVENT_HANDLER, evm::event_qname(scope, member)),
        };
        let id = NodeId::from_parts(GRAPH_TYPE, repo, kind, &qname);
        let mut variants: BTreeMap<(&str, Option<Vec<String>>), Vec<u32>> = BTreeMap::new();
        for site in &group {
            let abi = site
                .abi
                .as_ref()
                .map(|e| e.iter().map(AbiEntry::to_key).collect());
            let args = variants.entry((site.lib, abi)).or_default();
            args.extend(site.args);
            out.anchors.push(Anchor {
                node: id,
                line: anchor::line_of(source, site.at),
            });
        }
        let cells = variants
            .into_iter()
            .map(|((lib, abi), args)| {
                evm::client_cell(&ClientFacts {
                    lib: lib.to_string(),
                    args,
                    abi,
                })
            })
            .collect();
        out.nodes.push(Node {
            id,
            repo,
            confidence: Confidence::Medium,
            cells,
        });
        out.nav.record(
            id,
            &format!("{key}.{member}"),
            &qname,
            kind,
            Some(module_id),
        );
        match side {
            ClientSide::Call => out.counts.calls += 1,
            ClientSide::Event => out.counts.events += 1,
        }
    }
    out.counts.sites = sites.len();
    for site in sites {
        if let Some(i) = LIBS.iter().position(|l| *l == site.lib) {
            out.counts.by_lib[i] += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use glia_code_domain::cell_type;
    use glia_code_domain::evm::{EntryKind, read_client_cells};
    use glia_core::CellPayload;

    fn entry(kind: EntryKind, name: &str, arity: u32) -> AbiEntry {
        AbiEntry {
            kind,
            name: name.to_string(),
            arity: Some(arity),
        }
    }

    fn site(
        side: ClientSide,
        scope: &str,
        member: &str,
        args: Option<u32>,
        lib: &'static str,
        abi: Option<Vec<AbiEntry>>,
        at: usize,
    ) -> Site {
        Site {
            side,
            scope: Scope::Abi(scope.to_string()),
            member: member.to_string(),
            args,
            lib,
            abi,
            at,
        }
    }

    #[test]
    fn mint_groups_one_node_per_scope_member_with_one_cell_per_variant() {
        let source = "a\nb\nc\nd\ne\nf\n";
        let escrow = vec![
            entry(EntryKind::Fn, "release", 1),
            entry(EntryKind::Event, "Released", 2),
        ];
        let sites = vec![
            site(
                ClientSide::Call,
                "ESCROW_ABI",
                "release",
                Some(1),
                "ethers",
                Some(escrow.clone()),
                0,
            ),
            site(
                ClientSide::Call,
                "ESCROW_ABI",
                "release",
                Some(2),
                "ethers",
                Some(escrow.clone()),
                2,
            ),
            site(
                ClientSide::Call,
                "ESCROW_ABI",
                "release",
                Some(1),
                "ethers",
                None,
                4,
            ),
            site(
                ClientSide::Event,
                "ESCROW_ABI",
                "Released",
                None,
                "ethers",
                Some(escrow),
                6,
            ),
            site(
                ClientSide::Call,
                "ERC20_ABI",
                "decimals",
                Some(0),
                "ethers",
                None,
                8,
            ),
        ];
        let module = NodeId(7);
        let out = mint(&sites, source, module, RepoId(1));
        assert_eq!(
            (out.counts.calls, out.counts.events, out.counts.sites),
            (2, 1, 5)
        );
        assert_eq!(out.counts.by_lib, [5, 0, 0, 0, 0, 0]);
        let qnames: Vec<&str> = out
            .nodes
            .iter()
            .map(|n| out.nav.qname_by_id[&n.id].as_str())
            .collect();
        assert_eq!(
            qnames,
            [
                "evm_call:abi:ERC20_ABI.decimals",
                "evm_call:abi:ESCROW_ABI.release",
                "evm_event:abi:ESCROW_ABI.Released",
            ]
        );
        let release = &out.nodes[1];
        assert_eq!(release.confidence, Confidence::Medium);
        assert!(
            release
                .cells
                .iter()
                .all(|c| c.kind == cell_type::ENDPOINT_HIT)
        );
        let jsons: Vec<&str> = release
            .cells
            .iter()
            .map(|c| match &c.payload {
                CellPayload::Json(j) => j.as_str(),
                other => panic!("not JSON: {other:?}"),
            })
            .collect();
        assert_eq!(
            jsons,
            [
                r#"{"via":"evm","lib":"ethers","args":[1]}"#,
                r#"{"via":"evm","lib":"ethers","args":[1,2],"abi":["e:Released/2","f:release/1"]}"#,
            ]
        );
        assert_eq!(read_client_cells(&release.cells).len(), 2);
        assert_eq!(out.nav.name_by_id[&release.id], "abi:ESCROW_ABI.release");
        assert_eq!(out.nav.parent_of.get(&release.id), Some(&module));
        let lines: Vec<(NodeId, u32)> = out.anchors.iter().map(|a| (a.node, a.line)).collect();
        assert_eq!(
            lines,
            [
                (out.nodes[0].id, 4),
                (release.id, 0),
                (release.id, 1),
                (release.id, 2),
                (out.nodes[2].id, 3),
            ]
        );
        let event = &out.nodes[2];
        assert_eq!(out.nav.kind_by_id[&event.id], node_kind::EVENT_HANDLER);
        assert_eq!(
            read_client_cells(&event.cells)[0].args,
            Vec::<u32>::new(),
            "a subscription passes no argument count"
        );
    }

    #[test]
    fn marker_prints_on_a_dynamic_only_file() {
        let src = "import { Contract } from 'ethers';\nconst c = new Contract(addr, load(), signer);\nc.deposit(1);\n";
        let out = extract_evm_clients(src, "typescript", NodeId(1), RepoId(1));
        assert!(out.nodes.is_empty() && out.anchors.is_empty());
        assert_eq!(
            out.markers("src/a.ts"),
            [
                "[evm-client] lang=ts calls=0 events=0 sites=0 (ethers=0 hardhat=0 typechain=0 viem=0 web3=0 geth=0) abis=0 not_in_abi=0 dynamic=1 path=src/a.ts"
            ]
        );
        let quiet = extract_evm_clients(
            "import { Contract } from 'ethers';\n",
            "typescript",
            NodeId(1),
            RepoId(1),
        );
        assert!(
            quiet.markers("src/b.ts").is_empty(),
            "no site and nothing dynamic: no line"
        );
        let go = extract_evm_clients(src, "go", NodeId(1), RepoId(1));
        assert!(
            go.nodes.is_empty() && go.markers("a.go").is_empty(),
            "no Go reader yet"
        );
    }

    #[test]
    fn extract_reads_the_fixture_shape() {
        let src = "import { BrowserProvider, Contract } from 'ethers';

const ESCROW_ABI = [
  'function deposit(bytes32 orderId) payable',
  'function release(bytes32 orderId)',
  'function deposits(bytes32) view returns (uint256)',
  'event Released(bytes32 indexed orderId, address seller)',
];

const ERC20_ABI = [
  'function balanceOf(address) view returns (uint256)',
  'function decimals() view returns (uint8)',
];

export class EscrowClient {
  constructor(private provider: BrowserProvider, private address: string, private token: string) {}

  async pay(orderId: string, value: bigint) {
    const signer = await this.provider.getSigner();
    const escrow = new Contract(this.address, ESCROW_ABI, signer);
    const tx = await escrow.deposit(orderId, { value });
    console.log(await escrow.getAddress());
    return tx.wait();
  }

  held(orderId: string) {
    return new Contract(this.address, ESCROW_ABI, this.provider).deposits(orderId);
  }

  watch(onRelease: (id: string) => void) {
    const escrow = new Contract(this.address, ESCROW_ABI, this.provider);
    escrow.on('Released', (orderId: string) => onRelease(orderId));
  }

  async tokenDecimals() {
    const token = new Contract(this.token, ERC20_ABI, this.provider);
    return token.decimals();
  }

  async stray(orderId: string) {
    const escrow = new Contract(this.address, ESCROW_ABI, this.provider);
    return escrow.withdraw(orderId);
  }
}
";
        let out = extract_evm_clients(src, "angular", NodeId(1), RepoId(1));
        assert_eq!(
            out.markers("src/escrow-client.ts"),
            [
                "[evm-client] lang=ts calls=3 events=1 sites=4 (ethers=4 hardhat=0 typechain=0 viem=0 web3=0 geth=0) abis=2 not_in_abi=1 dynamic=0 path=src/escrow-client.ts"
            ]
        );
        let mut qnames: Vec<&str> = out.nav.qname_by_id.values().map(String::as_str).collect();
        qnames.sort_unstable();
        assert_eq!(
            qnames,
            [
                "evm_call:abi:ERC20_ABI.decimals",
                "evm_call:abi:ESCROW_ABI.deposit",
                "evm_call:abi:ESCROW_ABI.deposits",
                "evm_event:abi:ESCROW_ABI.Released",
            ]
        );
    }
}
