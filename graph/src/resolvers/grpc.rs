//! gRPC stack resolver — client → service by service name, package-aware (A5.4).
//!
//! A5.1 package-qualified service qnames (`grpc:billing.PaymentsService`), but a
//! client stub reconstructed from generated code usually only knows the bare
//! name (`grpc_client:PaymentsService`). When that bare name is declared in more
//! than one proto package, pairing it with every one of them is a wrong edge
//! nobody can see, so the resolver narrows by the package evidence the client's
//! file carries (its RPC_PACKAGE cell, see
//! `repo_graph_code_extractors::grpc::client_package_evidence`) and DROPS the
//! pairing when that evidence does not single out one package. Precision first:
//! in a repo whose client names no package, the right edge goes too.
//!
//! A5.3 adds the server half: every GRPC_SERVER marker (`grpc_server:<Service>`,
//! minted where a class extends / embeds / registers the generated base) is
//! paired back to its service as `service --HANDLED_BY--> marker`, through the
//! same index and the same package narrowing.

use std::collections::{HashMap, HashSet};

use repo_graph_code_domain::{cell_type, edge_category, node_kind};
use repo_graph_code_extractors::grpc::RpcPackageCell;
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, NodeId};

use super::{CrossGraphResolver, weakest};
use crate::merged::MergedGraph;
use crate::types::RepoGraph;

// ============================================================================
// GrpcStackResolver — matches gRPC client → service by service name
// ============================================================================

pub struct GrpcStackResolver;

impl CrossGraphResolver for GrpcStackResolver {
    fn resolve(&self, merged: &mut MergedGraph) {
        let index = build_grpc_service_index(&merged.graphs);
        let mut stats = PairStats::default();
        let mut edges = Vec::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRPC_CLIENT) {
                    continue;
                }
                let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
                let Some(svc_name) = qname.strip_prefix("grpc_client:") else { continue };
                stats.clients += 1;
                let Some((targets, name_pkg)) = grpc_candidate_keys(svc_name)
                    .into_iter()
                    .find_map(|c| index.get(&c.key).filter(|t| !t.is_empty()).map(|t| (t, c.package)))
                else {
                    continue;
                };
                let chosen: Vec<&GrpcTarget> =
                    match pick_targets(targets, name_pkg.as_deref(), &client_evidence(&n.cells)) {
                        Pick::All => targets.iter().collect(),
                        Pick::Narrowed(v) => {
                            stats.narrowed += 1;
                            v
                        }
                        Pick::Ambiguous => {
                            stats.ambiguous += 1;
                            continue;
                        }
                    };
                for t in chosen {
                    edges.push(Edge {
                        from: n.id,
                        to: t.id,
                        category: edge_category::GRPC_CALLS,
                        confidence: weakest(n.confidence, t.confidence),
                    });
                }
            }
        }
        // A5.4 fired_on marker, once per resolve. Silent on a build with no gRPC.
        if index.services > 0 || stats.clients > 0 {
            eprintln!(
                "[grpc-index] {} services ({} qualified, {} bare) -> {} edges, {} dropped ambiguous, {} narrowed by package evidence",
                index.services,
                index.qualified,
                index.services - index.qualified,
                edges.len(),
                stats.ambiguous,
                stats.narrowed
            );
        }
        merged.cross_edges.extend(edges);
        let served = pair_servers(merged, &index);
        merged.cross_edges.extend(served);
    }
}

/// A5.3: `grpc:<Service> --HANDLED_BY--> grpc_server:<Service>` for every
/// server-impl marker, keyed on the same index and narrowed by the same package
/// evidence as the client loop. The direction mirrors `ROUTE --HANDLED_BY-->
/// handler`: the contract is handled by the code that serves it, and
/// HANDLED_BY is a blast carry edge, so a proto change reaches the impl.
fn pair_servers(merged: &MergedGraph, index: &GrpcIndex) -> Vec<Edge> {
    let mut edges = Vec::new();
    let mut seen: HashSet<NodeId> = HashSet::new();
    let mut handled: HashSet<NodeId> = HashSet::new();
    let (mut matched, mut unmatched) = (0usize, 0usize);
    for g in &merged.graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRPC_SERVER) || !seen.insert(n.id) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(svc_name) = qname.strip_prefix("grpc_server:") else { continue };
            let found = grpc_candidate_keys(svc_name)
                .into_iter()
                .find_map(|c| index.get(&c.key).filter(|t| !t.is_empty()).map(|t| (t, c.package)));
            let chosen: Vec<&GrpcTarget> = match found {
                None => Vec::new(),
                Some((targets, name_pkg)) => {
                    match pick_targets(targets, name_pkg.as_deref(), &client_evidence(&n.cells)) {
                        Pick::All => targets.iter().collect(),
                        Pick::Narrowed(v) => v,
                        Pick::Ambiguous => Vec::new(),
                    }
                }
            };
            if chosen.is_empty() {
                unmatched += 1;
                continue;
            }
            matched += 1;
            for t in chosen {
                handled.insert(t.id);
                edges.push(Edge {
                    from: t.id,
                    to: n.id,
                    category: edge_category::HANDLED_BY,
                    confidence: weakest(n.confidence, t.confidence),
                });
            }
        }
    }
    // A5.3 fired_on marker, once per resolve that sees a server marker.
    if !seen.is_empty() {
        eprintln!(
            "[grpc-server] {matched} impls matched ({} services, {unmatched} unmatched)",
            handled.len()
        );
    }
    edges
}

#[derive(Default)]
struct PairStats {
    clients: usize,
    ambiguous: usize,
    narrowed: usize,
}

/// One indexed GRPC_SERVICE, with the package identity its RPC_PACKAGE cell
/// declares (all `None` for a service that carries no such cell).
#[derive(Clone)]
struct GrpcTarget {
    id: NodeId,
    confidence: Confidence,
    pkg: RpcPackageCell,
}

struct GrpcIndex {
    by_key: HashMap<String, Vec<GrpcTarget>>,
    services: usize,
    /// Services whose qname carries a proto package (`grpc:<pkg>.<Svc>`).
    qualified: usize,
}

impl GrpcIndex {
    fn get(&self, key: &str) -> Option<&Vec<GrpcTarget>> {
        self.by_key.get(key)
    }
}

/// The service's package identity: the first RPC_PACKAGE cell's fields, with
/// any option a later duplicate cell adds (a service declared twice in one repo
/// shares one node id, and the graph builder appends both files' cells).
fn service_package(cells: &[Cell]) -> RpcPackageCell {
    let mut out: Option<RpcPackageCell> = None;
    for p in rpc_package_cells(cells) {
        match out.as_mut() {
            None => out = Some(p),
            Some(o) => {
                o.go_package = o.go_package.take().or(p.go_package);
                o.java_package = o.java_package.take().or(p.java_package);
                o.csharp_namespace = o.csharp_namespace.take().or(p.csharp_namespace);
            }
        }
    }
    out.unwrap_or_default()
}

fn rpc_package_cells(cells: &[Cell]) -> impl Iterator<Item = RpcPackageCell> + '_ {
    cells.iter().filter_map(|c| match &c.payload {
        CellPayload::Json(j) if c.kind == cell_type::RPC_PACKAGE => RpcPackageCell::parse(j),
        _ => None,
    })
}

fn build_grpc_service_index(graphs: &[RepoGraph]) -> GrpcIndex {
    let mut index = GrpcIndex {
        by_key: HashMap::new(),
        services: 0,
        qualified: 0,
    };
    for g in graphs {
        for n in &g.nodes {
            if g.nav.kind_by_id.get(&n.id) != Some(&node_kind::GRPC_SERVICE) {
                continue;
            }
            let Some(qname) = g.nav.qname_by_id.get(&n.id) else { continue };
            let Some(svc_name) = qname.strip_prefix("grpc:") else { continue };
            let mut pkg = service_package(&n.cells);
            // A5.1 writes `grpc:<package>.<Service>` exactly when the proto has
            // a package, and a proto service name has no dot, so the qname
            // prefix IS the package for a service that carries no cell.
            if pkg.package.is_none() {
                pkg.package = svc_name.rsplit_once('.').map(|(p, _)| p.to_string());
            }
            let target = GrpcTarget {
                id: n.id,
                confidence: n.confidence,
                pkg,
            };
            index.services += 1;
            // A5.1: proto service qnames are package-qualified
            // (`grpc:user.UserService`), but a client stub reconstructed from
            // generated code only ever knows the bare last segment
            // (`grpc_client:UserService`). Index both so the pairing survives
            // the qualification; A5.4 disambiguates the bare key at lookup.
            if let Some(bare) = svc_name.rsplit('.').next()
                && bare != svc_name
            {
                index.qualified += 1;
                index.by_key.entry(bare.to_string()).or_default().push(target.clone());
            }
            index.by_key.entry(svc_name.to_string()).or_default().push(target);
        }
    }
    index
}

/// One lookup key for a client name, plus the package the client's own name
/// asserts when the key is its bare last segment.
#[derive(Debug, PartialEq, Eq)]
struct Candidate {
    key: String,
    package: Option<String>,
}

/// Lookup keys for a client's service name, longest first: the full dotted
/// name, then successive prefixes dropping the last segment, then the last
/// segment alone. The first key with targets wins.
///
/// `OrderService.PlaceOrder` → `OrderService.PlaceOrder`, `OrderService`,
/// `PlaceOrder` (the method-level shape resolves on its second key).
/// `billing.PaymentsService` → `billing.PaymentsService`, `billing`,
/// `PaymentsService` — and the last carries `billing` as the package the
/// client asserts, so a bare-key hit can still be narrowed to it.
fn grpc_candidate_keys(name: &str) -> Vec<Candidate> {
    let segs: Vec<&str> = name.split('.').filter(|s| !s.is_empty()).collect();
    let mut out: Vec<Candidate> = (1..=segs.len())
        .rev()
        .map(|end| Candidate {
            key: segs[..end].join("."),
            package: None,
        })
        .collect();
    if segs.len() > 1 {
        out.push(Candidate {
            key: segs[segs.len() - 1].to_string(),
            package: Some(segs[..segs.len() - 1].join(".")),
        });
    }
    out
}

/// Import paths as identifier-token runs: `example.com/gen/billing` →
/// `[example, com, gen, billing]`. Both sides of a match are tokenised the same
/// way, so `/`, `.`, `::`, `\` and `-` are interchangeable separators.
fn path_tokens(s: &str) -> Vec<&str> {
    s.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .collect()
}

fn contains_run(hay: &[&str], needle: &[&str], ignore_case: bool) -> bool {
    !needle.is_empty()
        && hay.windows(needle.len()).any(|w| {
            w.iter().zip(needle).all(|(a, b)| {
                if ignore_case { a.eq_ignore_ascii_case(b) } else { a == b }
            })
        })
}

/// Each RPC_PACKAGE cell on the client is one file's evidence (two files that
/// construct the same stub share a node id), kept apart so each file decides on
/// its own.
fn client_evidence(cells: &[Cell]) -> Vec<Vec<String>> {
    rpc_package_cells(cells)
        .map(|p| p.imports)
        .filter(|imports| !imports.is_empty())
        .collect()
}

enum Pick<'a> {
    /// Every target shares one package: pair with all of them, as before A5.4.
    All,
    /// Several packages, and the evidence named exactly one per file.
    Narrowed(Vec<&'a GrpcTarget>),
    /// Several packages and nothing to choose between them: emit nothing.
    Ambiguous,
}

/// How strongly one file's imports name the proto package `package`, lower is
/// stronger: 1 when a generated-namespace option (`go_package`, `java_package`,
/// `csharp_namespace`) of any target in that package appears as a token run in
/// an import, case-sensitively; 2 when the proto package itself does,
/// case-insensitively (C# PascalCases it, Ruby capitalises it). The option is
/// mapped back to the target's `package` — `using GreeterApi;` selects the
/// service whose `csharp_namespace` is `GreeterApi`, whatever its package is.
fn evidence_tier(package: Option<&str>, targets: &[GrpcTarget], imports: &[Vec<&str>]) -> Option<u8> {
    let options = targets
        .iter()
        .filter(|t| t.pkg.package.as_deref() == package)
        .flat_map(|t| {
            [
                // `go_package = "example.com/gen/billing;billingpb"` — the path
                // is what an importer writes; the alias after `;` is not.
                t.pkg.go_package.as_deref().map(|g| g.split(';').next().unwrap_or(g)),
                t.pkg.java_package.as_deref(),
                t.pkg.csharp_namespace.as_deref(),
            ]
        })
        .flatten();
    for opt in options {
        let run = path_tokens(opt);
        if imports.iter().any(|i| contains_run(i, &run, false)) {
            return Some(1);
        }
    }
    let run = path_tokens(package?);
    imports.iter().any(|i| contains_run(i, &run, true)).then_some(2)
}

fn pick_targets<'a>(
    targets: &'a [GrpcTarget],
    name_pkg: Option<&str>,
    evidence: &[Vec<String>],
) -> Pick<'a> {
    let mut packages: Vec<Option<&str>> = targets.iter().map(|t| t.pkg.package.as_deref()).collect();
    packages.sort_unstable();
    packages.dedup();
    if packages.len() <= 1 {
        return Pick::All;
    }

    let mut chosen: Vec<Option<&str>> = Vec::new();
    if let Some(p) = name_pkg {
        // The client's own qualified name is the strongest evidence there is.
        if packages.contains(&Some(p)) {
            chosen.push(Some(p));
        }
    } else {
        for file in evidence {
            let imports: Vec<Vec<&str>> = file.iter().map(|i| path_tokens(i)).collect();
            let tiers: Vec<(Option<&str>, u8)> = packages
                .iter()
                .filter_map(|p| evidence_tier(*p, targets, &imports).map(|t| (*p, t)))
                .collect();
            let Some(best) = tiers.iter().map(|(_, t)| *t).min() else { continue };
            let mut winners = tiers.iter().filter(|(_, t)| *t == best).map(|(p, _)| *p);
            if let (Some(w), None) = (winners.next(), winners.next())
                && !chosen.contains(&w)
            {
                chosen.push(w);
            }
        }
    }
    if chosen.is_empty() {
        return Pick::Ambiguous;
    }
    Pick::Narrowed(
        targets
            .iter()
            .filter(|t| chosen.contains(&t.pkg.package.as_deref()))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(name: &str) -> Vec<(String, Option<String>)> {
        grpc_candidate_keys(name)
            .into_iter()
            .map(|c| (c.key, c.package))
            .collect()
    }

    #[test]
    fn candidate_chain_is_longest_first_and_ends_on_the_bare_segment() {
        let s = |x: &str| x.to_string();
        assert_eq!(keys("UserService"), vec![(s("UserService"), None)]);
        assert_eq!(
            keys("OrderService.PlaceOrder"),
            vec![
                (s("OrderService.PlaceOrder"), None),
                (s("OrderService"), None),
                (s("PlaceOrder"), Some(s("OrderService"))),
            ]
        );
        assert_eq!(
            keys("billing.v1.PaymentsService"),
            vec![
                (s("billing.v1.PaymentsService"), None),
                (s("billing.v1"), None),
                (s("billing"), None),
                (s("PaymentsService"), Some(s("billing.v1"))),
            ]
        );
    }

    #[test]
    fn token_runs_match_across_separators_and_respect_case_only_when_asked() {
        let hay = path_tokens("example.com/gen/billing");
        assert!(contains_run(&hay, &path_tokens("example.com/gen/billing"), false));
        assert!(contains_run(&hay, &path_tokens("gen::billing"), false));
        assert!(!contains_run(&hay, &path_tokens("gen/legacy"), false));
        assert!(!contains_run(&hay, &path_tokens("Billing"), false));
        assert!(contains_run(&hay, &path_tokens("Billing"), true));
        assert!(!contains_run(&hay, &[], true), "an empty needle names nothing");
        // A token is whole: `billing` is not `billingpb`.
        assert!(!contains_run(&path_tokens("gen/billingpb"), &path_tokens("billing"), true));
    }
}
