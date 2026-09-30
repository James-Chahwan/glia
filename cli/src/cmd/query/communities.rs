//! `glia communities` (CD.1e) — the human surface over the engine's
//! `communities::communities` (CD.1d): the graph's communities by seeded
//! Leiden (label propagation above Leiden's pair cap, or exactly `--method`),
//! each summarised from observed edges. A header line gives the method, the
//! partition's modularity, the communities listed of those found and the
//! isolated nodes (no weighted edge, in no community); then one
//! `## <id> <label> (<size> nodes, cohesion 0.xx)` block per community with
//! its node kinds, `glia arch` services, effect sinks, entry points
//! (`file:line`), top members by internal weight (`qname  kind  file:line`)
//! and its heaviest links to other communities
//! (`-> #<id> weight <w> (CALLS 3, HTTP_CALLS 1)`). Every summary is tier
//! heuristic: the grouping is an optimisation output, the counts under it are
//! read off edges.
//!
//! A report: it exits 0 whatever it finds, and an empty answer prints the
//! engine's absence (why none). Exit 2 is a build failure or a `--resolution`
//! that is not a finite number above 0. `--json` is the whole
//! `CommunitiesAnswer`. The engine's `[communities] ... surface=cli` stderr
//! line is the fired_on marker.

use glia_engine::communities::{
    CommunitiesAnswer, CommunityArgs, CommunityLink, CommunitySummary, DEFAULT_MEMBERS,
    DEFAULT_RESOLUTION, DEFAULT_SEED, DEFAULT_TOP, communities,
};

use crate::cmd::resolve::print_absence;
use crate::common::generate_for;

/// [`CommunityArgs::surface`] for the marker.
const SURFACE: &str = "cli";

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Path to the repo root.
    repo: String,
    /// Additional repos to merge in (cross-service). Repeatable.
    #[arg(long)]
    with: Vec<String>,
    /// Partition only the nodes under this repo-relative path or project
    /// label (see `glia projects`); edges leaving it are not in the view.
    #[arg(long)]
    scope: Option<String>,
    /// Seeds every random choice; another seed can escape a local optimum.
    #[arg(long, default_value_t = DEFAULT_SEED)]
    seed: u64,
    /// Modularity's gamma, a number above 0: above 1 favours more, smaller
    /// communities; below 1 fewer, larger ones.
    #[arg(long, default_value_t = DEFAULT_RESOLUTION)]
    resolution: f64,
    /// Communities listed, largest first; 0 lists every one.
    #[arg(long, default_value_t = DEFAULT_TOP)]
    top: usize,
    /// Top members per community; 0 lists every member.
    #[arg(long, default_value_t = DEFAULT_MEMBERS)]
    members: usize,
    /// `leiden` or `lpa` (label propagation). Default: Leiden up to its pair
    /// cap, label propagation above it.
    #[arg(long, value_parser = ["leiden", "lpa"])]
    method: Option<String>,
    /// Emit JSON instead of tables.
    #[arg(long)]
    json: bool,
}

/// Why `--resolution r` is refused, or `None` for a finite number above 0.
fn resolution_error(r: f64) -> Option<&'static str> {
    if r.is_nan() || r <= 0.0 {
        Some("--resolution must be > 0")
    } else if r.is_infinite() {
        Some("--resolution must be a finite number")
    } else {
        None
    }
}

pub(crate) fn run(args: Args) -> i32 {
    if let Some(e) = resolution_error(args.resolution) {
        eprintln!("error: {e}");
        return 2;
    }
    let result = match generate_for(&args.repo, &args.with) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    let mut query = CommunityArgs::default();
    query.scope = args.scope;
    query.seed = args.seed;
    query.resolution = args.resolution;
    query.top = args.top;
    query.members = args.members;
    query.method = args.method;
    query.surface = SURFACE;
    let mut answer = communities(&result.merged, &result.repo_labels, &query);
    if let Some(a) = answer.absence.as_mut() {
        a.unparsed_files = result.parse_errors.len();
    }
    if args.json {
        println!("{}", serde_json::to_string(&answer).unwrap_or_default());
        return 0;
    }

    println!("# glia communities `{}`", args.repo);
    println!();
    println!("{}", header(&answer));
    println!();
    if let Some(a) = &answer.absence {
        println!("_(no communities)_");
        println!();
        print_absence(a);
        return 0;
    }
    for c in &answer.communities {
        print_community(c);
    }
    0
}

/// `- method leiden, modularity 0.412, communities 2/3 (listed/total),
/// isolated 2 of 17 nodes (seed 42, resolution 1)`.
fn header(a: &CommunitiesAnswer) -> String {
    format!(
        "- method {}, modularity {:.3}, communities {}/{} (listed/total), isolated {} of {} nodes (seed {}, resolution {})",
        a.method,
        a.modularity,
        a.communities.len(),
        a.total,
        a.isolated,
        a.nodes,
        a.seed,
        a.resolution,
    )
}

fn print_community(c: &CommunitySummary) {
    println!(
        "## {} {} ({} nodes, cohesion {:.2})",
        c.id, c.label, c.size, c.cohesion
    );
    println!();
    println!("- kinds: {}", histogram(&c.kinds));
    println!("- services: {}", histogram(&c.services));
    println!("- sinks: {}", histogram(&c.sinks));
    println!("- files: {}", c.files);
    println!("- entries:{}", none_if_empty(c.entries.is_empty()));
    for e in &c.entries {
        println!("  - `{}`  {}", e.qname, at(e.file.as_deref(), e.line));
    }
    println!("- top members:{}", none_if_empty(c.top_members.is_empty()));
    for m in &c.top_members {
        println!(
            "  - `{}`  {}  {}",
            m.qname,
            m.kind,
            at(m.file.as_deref(), m.line)
        );
    }
    println!("- links:{}", none_if_empty(c.links.is_empty()));
    for l in &c.links {
        println!("  - {}", link(l));
    }
    println!();
}

/// ` —` after a list heading with no items under it, else nothing.
fn none_if_empty(empty: bool) -> &'static str {
    if empty { " —" } else { "" }
}

/// `FUNCTION 7, MODULE 1`, or `—` for none.
fn histogram<K: std::fmt::Display>(counts: &[(K, usize)]) -> String {
    if counts.is_empty() {
        return "—".to_string();
    }
    counts
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `-> #1 weight 7 (CALLS 1, IMPORTS 1)`.
fn link(l: &CommunityLink) -> String {
    format!(
        "-> #{} weight {} ({})",
        l.to,
        l.weight,
        histogram(&l.categories)
    )
}

/// `file:line`, the file alone, or `—` for an unlocated node.
fn at(file: Option<&str>, line: Option<i64>) -> String {
    match (file, line) {
        (Some(f), Some(l)) => format!("{f}:{l}"),
        (Some(f), None) => f.to_string(),
        _ => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{at, histogram, resolution_error};

    #[test]
    fn resolution_must_be_a_finite_number_above_zero() {
        for bad in [0.0, -1.0, f64::NAN] {
            assert_eq!(resolution_error(bad), Some("--resolution must be > 0"));
        }
        assert_eq!(
            resolution_error(f64::INFINITY),
            Some("--resolution must be a finite number")
        );
        for good in [0.001, 1.0, 2.5] {
            assert_eq!(resolution_error(good), None);
        }
    }

    #[test]
    fn histogram_cell() {
        assert_eq!(
            histogram(&[("CALLS", 3), ("HTTP_CALLS", 1)]),
            "CALLS 3, HTTP_CALLS 1"
        );
        assert_eq!(histogram::<&str>(&[]), "—");
    }

    #[test]
    fn at_cell() {
        assert_eq!(at(Some("a.py"), Some(3)), "a.py:3");
        assert_eq!(at(Some("a.py"), None), "a.py");
        assert_eq!(at(None, Some(3)), "—");
    }
}
