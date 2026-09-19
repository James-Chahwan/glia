//! `glia arch` (A9.4) — the human surface over A9.2's service map: the
//! services in the stack and the cross-service links between them, as a
//! table, `--json` or `--mermaid`.

use std::collections::BTreeMap;

use crate::common::{edge_category_name, generate_for};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Emit JSON instead of a table.
    #[arg(long)]
    json: bool,
    /// Emit a Mermaid `graph LR` instead of a table.
    #[arg(long, conflicts_with = "json")]
    mermaid: bool,
    /// Also show non-flow links (SHARES_SCHEMA / SHARES_CONFIG /
    /// DOCUMENTS / …), hidden by default: the co-ownership ones are O(n²)
    /// across merged repos and none of them is a call between services.
    #[arg(long)]
    include_shared: bool,
}

pub(crate) fn run(args: Args) -> i32 {
    let repo = args.repo.as_str();
    let with = args.with.as_slice();
    let (json, mermaid, include_shared) = (args.json, args.mermaid, args.include_shared);
    let result = match generate_for(repo, with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    // The `[arch] …` fired_on marker is emitted here, inside the engine.
    let mut map = glia_engine::service_map(&result.merged, &result.repo_labels);
    if !include_shared {
        drop_non_flow_links(&mut map);
    }
    if json {
        println!("{}", serde_json::to_string(&map).unwrap_or_default());
    } else if mermaid {
        print_service_mermaid(&map);
    } else {
        print_service_table(repo, &map);
    }
    0
}

/// Keep only the directional flow links (`FLOW_MECHANISMS`) — a real call from
/// `from` to `to`. Everything else `cross_links` returns is co-ownership
/// (`SHARES_SCHEMA`, `SHARES_DEPENDENCY`, …) or documentation (`DOCUMENTS`):
/// true, but not traffic. The `SHARES_*` ones are also O(n²) across merged
/// repos, so one shared npm dependency would bury every real call.
///
/// Recomputes `inbound` / `outbound`: those count *surviving link rows*, so
/// leaving them at the unfiltered value would print an `in`/`out` column that
/// no visible row accounts for.
pub(crate) fn drop_non_flow_links(map: &mut glia_engine::ServiceMap) {
    let flows: Vec<&'static str> = glia_engine::arch::FLOW_MECHANISMS
        .iter()
        .map(|c| edge_category_name(*c))
        .collect();
    map.links.retain(|l| flows.contains(&l.mechanism));
    let mut io: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for l in &map.links {
        io.entry(l.from.as_str()).or_default().1 += 1;
        io.entry(l.to.as_str()).or_default().0 += 1;
    }
    for s in &mut map.services {
        let (inbound, outbound) = io.get(s.id.as_str()).copied().unwrap_or((0, 0));
        s.inbound = inbound;
        s.outbound = outbound;
    }
}

fn print_service_table(repo: &str, map: &glia_engine::ServiceMap) {
    println!("# glia arch `{repo}` (keying: {})", map.keying);
    println!();
    println!("| service | repo | languages | files | nodes | routes | endpoints | in | out |");
    println!("|---|---|---|--:|--:|--:|--:|--:|--:|");
    for s in &map.services {
        let langs = if s.languages.is_empty() {
            "—".to_string()
        } else {
            s.languages.join(", ")
        };
        println!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.id, s.repo, langs, s.files, s.nodes, s.routes, s.endpoints, s.inbound, s.outbound
        );
    }
    println!();
    if map.links.is_empty() {
        println!("_(no cross-service links)_");
    } else {
        println!("| from | → to | mechanism | channel | × |");
        println!("|---|---|---|---|--:|");
        for l in &map.links {
            let channel = if l.channel.is_empty() { "—" } else { &l.channel };
            println!(
                "| {} | {} | {} | {} | {} |",
                l.from, l.to, l.mechanism, channel, l.count
            );
        }
    }
    if map.self_links > 0 || map.unlocated_nodes > 0 {
        println!();
        println!(
            "_Dropped: {} self-link(s) (both ends in one service), {} unlocated node(s) (no file cell)._",
            map.self_links, map.unlocated_nodes
        );
    }
}

pub(crate) fn print_service_mermaid(map: &glia_engine::ServiceMap) {
    // Mermaid ids must be `[A-Za-z0-9_]`, and a service id is a directory path
    // — so the id is positional (`svc{i}` over the already-sorted `services`)
    // and the real name lives in the label.
    let idx: BTreeMap<&str, usize> = map
        .services
        .iter()
        .enumerate()
        .map(|(i, s)| (s.id.as_str(), i))
        .collect();
    println!("```mermaid");
    println!("graph LR");
    for (i, s) in map.services.iter().enumerate() {
        println!("    svc{i}[\"{}\"]", mermaid_label(&s.id));
    }
    // Collapse the per-channel rows to one arrow per (from, to, mechanism).
    let mut agg: BTreeMap<(usize, usize, &'static str), (usize, Vec<&str>)> = BTreeMap::new();
    for l in &map.links {
        let (Some(&f), Some(&t)) = (idx.get(l.from.as_str()), idx.get(l.to.as_str())) else {
            continue;
        };
        let e = agg.entry((f, t, l.mechanism)).or_insert((0, Vec::new()));
        e.0 += l.count;
        if !l.channel.is_empty() {
            e.1.push(l.channel.as_str());
        }
    }
    for ((f, t, mech), (count, channels)) in agg {
        let shown: Vec<String> = channels.iter().take(3).map(|c| mermaid_label(c)).collect();
        let mut label = format!("{mech} ×{count}");
        if !shown.is_empty() {
            label.push_str("<br/>");
            label.push_str(&shown.join(", "));
            if channels.len() > 3 {
                label.push_str(&format!(" +{} more", channels.len() - 3));
            }
        }
        println!("    svc{f} -->|\"{label}\"| svc{t}");
    }
    println!("```");
}

/// Escape the characters that break a `|"…"|` Mermaid edge label. Channels are
/// raw route templates and topic literals, so `"`, `|`, `<` and `>` all turn up
/// in practice (`${…}`, `{id}`, `<T>`), and a single raw `"` silently breaks the
/// whole diagram in the renderer rather than just that one edge.
fn mermaid_label(s: &str) -> String {
    s.replace('"', "#quot;")
        .replace('|', "#124;")
        .replace('<', "#lt;")
        .replace('>', "#gt;")
}

// ----------------------------------------------------------------------------
// tests (A9.4 — the two `arch` pieces with a real failure mode)
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use glia_engine::{ServiceLink, ServiceMap, ServiceSummary};

    // The service-map types are `#[non_exhaustive]` (LD.9): outside the engine
    // they are built by `Default` plus field assignment, never by a literal.
    fn svc(id: &str) -> ServiceSummary {
        let mut s = ServiceSummary::default();
        s.id = id.to_string();
        s.repo = "r".to_string();
        s.inbound = 9;
        s.outbound = 9;
        s
    }

    fn link(from: &str, to: &str, mechanism: &'static str) -> ServiceLink {
        let mut l = ServiceLink::default();
        l.from = from.to_string();
        l.to = to.to_string();
        l.mechanism = mechanism;
        l.channel = "GET /x".to_string();
        l.count = 1;
        l.confidence = "strong";
        l
    }

    #[test]
    fn non_flow_links_are_dropped_and_io_recounted() {
        let mut map = ServiceMap::default();
        map.keying = "top_level_dir";
        map.services = vec![svc("web"), svc("api"), svc("docs")];
        map.links = vec![
            link("web", "api", "HTTP_CALLS"),
            link("web", "api", "SHARES_SCHEMA"),
            link("docs", "api", "DOCUMENTS"),
        ];
        drop_non_flow_links(&mut map);
        assert_eq!(map.links.len(), 1, "only the HTTP_CALLS flow survives");
        assert_eq!(map.links[0].mechanism, "HTTP_CALLS");
        // in/out must describe the SURVIVING rows, not the seeded 9s — a stale
        // count here prints an `in`/`out` no visible table row accounts for.
        let by = |id: &str| {
            let s = map.services.iter().find(|s| s.id == id).unwrap();
            (s.inbound, s.outbound)
        };
        assert_eq!(by("web"), (0, 1));
        assert_eq!(by("api"), (1, 0));
        assert_eq!(by("docs"), (0, 0), "its only link was dropped");
    }

    #[test]
    fn mermaid_label_escapes_what_breaks_the_renderer() {
        // A raw `"` closes the `|"…"|` label early and breaks the WHOLE
        // diagram, not just that edge; `${…}` and `{id}` are ordinary route
        // channels, so this is the common case, not the corner one.
        assert_eq!(
            mermaid_label(r#"GET /u/${id}/"a"|b<c>"#),
            "GET /u/${id}/#quot;a#quot;#124;b#lt;c#gt;"
        );
        assert_eq!(mermaid_label("GET /users"), "GET /users");
    }
}
