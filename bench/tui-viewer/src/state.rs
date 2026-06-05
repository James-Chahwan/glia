//! Shared app state — graph, current mode, animation frame, seeds.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use ratatui::Frame;
use repo_graph_code_domain::node_kind;
use repo_graph_core::{Edge, NodeId, NodeKindId};
use repo_graph_engine::generate_one;
use repo_graph_graph::MergedGraph;

use crate::modes;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Ppr,
    Flow,
    Build,
}

pub struct NodeMeta {
    pub kind: NodeKindId,
    pub name: String,
    pub qname: String,
}

pub struct AppState {
    pub repo: PathBuf,
    pub mode: Mode,
    pub paused: bool,
    pub frame: u64,
    pub graph: MergedGraph,
    pub all_nodes: Vec<NodeId>,
    pub all_edges: Vec<Edge>,
    pub meta: HashMap<NodeId, NodeMeta>,
    pub adjacency_out: HashMap<NodeId, Vec<(NodeId, repo_graph_core::EdgeCategoryId)>>,
    pub seeds: Vec<NodeId>,
    pub seed_idx: usize,
    pub reload_count: u32,
    pub last_message: Option<String>,
    // Tree cursor — visible row index across the flattened expanded tree.
    pub tree_cursor: usize,
    pub tree_scroll: usize,
    // Tiers default expanded — set holds explicitly-collapsed tier labels.
    pub collapsed_tiers: std::collections::HashSet<String>,
    // Groups default collapsed — set holds explicitly-expanded group keys
    // (`format!("{tier}::{group}")`).
    pub expanded_groups: std::collections::HashSet<String>,
    // Last-rendered flat tree (cached so input handlers can resolve cursor).
    pub last_tree: Vec<TreeRow>,
    // Query — typed text used as multi-seed activator and filter.
    pub query: String,
    pub query_focused: bool,
}

/// One visible row in the tree.
#[derive(Clone, Debug)]
pub enum TreeRow {
    Tier {
        label: &'static str,
        count: usize,
        expanded: bool,
    },
    Group {
        tier_label: &'static str,
        label: String,
        count: usize,
        expanded: bool,
        depth: u8,
    },
    Node {
        id: NodeId,
        score: f64,
        depth: u8,
    },
}

impl AppState {
    pub fn new(repo: PathBuf, mode: Mode, seed: Option<String>) -> Result<Self> {
        let mut s = Self {
            repo: repo.clone(),
            mode,
            paused: false,
            frame: 0,
            graph: MergedGraph {
                graphs: Vec::new(),
                cross_edges: Vec::new(),
            },
            all_nodes: Vec::new(),
            all_edges: Vec::new(),
            meta: HashMap::new(),
            adjacency_out: HashMap::new(),
            seeds: Vec::new(),
            seed_idx: 0,
            reload_count: 0,
            last_message: None,
            tree_cursor: 0,
            tree_scroll: 0,
            collapsed_tiers: std::collections::HashSet::new(),
            expanded_groups: std::collections::HashSet::new(),
            last_tree: Vec::new(),
            query: String::new(),
            query_focused: false,
        };
        s.rebuild()?;
        if let Some(p) = seed {
            if let Some(idx) = s.seeds.iter().position(|id| {
                s.meta.get(id).is_some_and(|m| m.qname == p || m.name == p)
            }) {
                s.seed_idx = idx;
            }
        }
        Ok(s)
    }

    pub fn reload(&mut self) -> Result<()> {
        self.rebuild()?;
        self.frame = 0;
        self.reload_count += 1;
        self.last_message = Some(format!(
            "reloaded — {} nodes, {} edges",
            self.all_nodes.len(),
            self.all_edges.len()
        ));
        Ok(())
    }

    fn rebuild(&mut self) -> Result<()> {
        let path = self
            .repo
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("non-utf8 repo path"))?;
        let result = generate_one(path).map_err(|e| anyhow::anyhow!("generate failed: {e}"))?;
        let merged = result.merged;

        let mut all_nodes: Vec<NodeId> = Vec::new();
        let mut meta: HashMap<NodeId, NodeMeta> = HashMap::new();
        for g in &merged.graphs {
            for n in &g.nodes {
                let kind = g
                    .nav
                    .kind_by_id
                    .get(&n.id)
                    .copied()
                    .unwrap_or(node_kind::MODULE);
                let name = g
                    .nav
                    .name_by_id
                    .get(&n.id)
                    .cloned()
                    .unwrap_or_default();
                let qname = g
                    .nav
                    .qname_by_id
                    .get(&n.id)
                    .cloned()
                    .unwrap_or_default();
                meta.entry(n.id).or_insert(NodeMeta { kind, name, qname });
                all_nodes.push(n.id);
            }
        }
        all_nodes.sort_unstable_by_key(|id| id.0);
        all_nodes.dedup();

        let mut all_edges: Vec<Edge> = merged
            .graphs
            .iter()
            .flat_map(|g| g.edges.iter().cloned())
            .collect();
        all_edges.extend(merged.cross_edges.iter().cloned());

        let seeds = collect_seed_candidates(&meta, &all_nodes);

        let mut adjacency_out: HashMap<NodeId, Vec<(NodeId, repo_graph_core::EdgeCategoryId)>> =
            HashMap::new();
        for e in &all_edges {
            adjacency_out
                .entry(e.from)
                .or_default()
                .push((e.to, e.category));
        }

        self.graph = merged;
        self.all_nodes = all_nodes;
        self.all_edges = all_edges;
        self.meta = meta;
        self.adjacency_out = adjacency_out;
        self.seeds = seeds;
        if self.seed_idx >= self.seeds.len() {
            self.seed_idx = 0;
        }
        Ok(())
    }

    /// Group nodes into tier columns, descending by an arbitrary score.
    /// Kept for legacy callers (currently unused after the tree refactor).
    #[allow(dead_code)]
    pub fn tiered(&self, scores: &HashMap<NodeId, f64>) -> [Vec<(NodeId, f64)>; 4] {
        use crate::style::{tier_of, Tier};
        let mut out: [Vec<(NodeId, f64)>; 4] = Default::default();
        for &id in &self.all_nodes {
            let Some(meta) = self.meta.get(&id) else { continue };
            let s = scores.get(&id).copied().unwrap_or(0.0);
            let idx = match tier_of(meta.kind) {
                Tier::Entry => 0,
                Tier::Service => 1,
                Tier::Handler => 2,
                Tier::Data => 3,
            };
            out[idx].push((id, s));
        }
        for col in &mut out {
            col.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        }
        out
    }

    pub fn move_cursor(&mut self, dy: i32) {
        let next = (self.tree_cursor as i32 + dy).max(0) as usize;
        self.tree_cursor = next;
    }

    pub fn toggle_collapsed_at_cursor(&mut self) {
        let Some(row) = self.last_tree.get(self.tree_cursor).cloned() else {
            return;
        };
        match row {
            TreeRow::Tier { label, .. } => {
                let key = label.to_string();
                if !self.collapsed_tiers.remove(&key) {
                    self.collapsed_tiers.insert(key);
                }
            }
            TreeRow::Group { tier_label, label, .. } => {
                let key = format!("{}::{}", tier_label, label);
                if !self.expanded_groups.remove(&key) {
                    self.expanded_groups.insert(key);
                }
            }
            TreeRow::Node { id, .. } => {
                // Treat Enter-or-Space on a node as "promote to seed".
                if let Some(i) = self.seeds.iter().position(|&s| s == id) {
                    self.seed_idx = i;
                } else {
                    self.seeds.insert(0, id);
                    self.seed_idx = 0;
                }
                self.frame = 0;
                let q = self.meta.get(&id).map(|m| m.qname.as_str()).unwrap_or("?");
                self.last_message = Some(format!("seed → {}", q));
            }
        }
    }

    /// Build a flat list of visible tree rows, given per-node scores.
    /// Tiers are top-level; nodes are grouped by qname prefix (everything
    /// before the last `::`, or `(misc)` if no `::`).
    pub fn build_tree(&self, scores: &HashMap<NodeId, f64>) -> Vec<TreeRow> {
        use crate::style::{tier_of, Tier};
        let tier_labels = ["ENTRY", "SERVICE", "HANDLER", "DATA"];
        let mut by_tier: [Vec<NodeId>; 4] = Default::default();
        for &id in &self.all_nodes {
            let Some(meta) = self.meta.get(&id) else { continue };
            let idx = match tier_of(meta.kind) {
                Tier::Entry => 0,
                Tier::Service => 1,
                Tier::Handler => 2,
                Tier::Data => 3,
            };
            by_tier[idx].push(id);
        }

        let mut rows = Vec::new();
        for (i, tier_nodes) in by_tier.iter().enumerate() {
            let tier_label = tier_labels[i];
            let tier_collapsed = self.collapsed_tiers.contains(tier_label);
            rows.push(TreeRow::Tier {
                label: tier_label,
                count: tier_nodes.len(),
                expanded: !tier_collapsed,
            });
            if tier_collapsed {
                continue;
            }

            // Group nodes by namespace prefix.
            let mut groups: HashMap<String, Vec<NodeId>> = HashMap::new();
            for &id in tier_nodes {
                let Some(meta) = self.meta.get(&id) else { continue };
                let group = group_of(&meta.qname);
                groups.entry(group).or_default().push(id);
            }
            // Sort groups by total node count desc, then alpha.
            let mut group_list: Vec<(String, Vec<NodeId>)> = groups.into_iter().collect();
            group_list.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

            for (g_label, mut g_nodes) in group_list {
                let g_key = format!("{}::{}", tier_label, g_label);
                let g_expanded = self.expanded_groups.contains(&g_key);
                rows.push(TreeRow::Group {
                    tier_label,
                    label: g_label.clone(),
                    count: g_nodes.len(),
                    expanded: g_expanded,
                    depth: 1,
                });
                if !g_expanded {
                    continue;
                }
                // Sort nodes within group by score desc, then by name.
                g_nodes.sort_by(|a, b| {
                    let sa = scores.get(a).copied().unwrap_or(0.0);
                    let sb = scores.get(b).copied().unwrap_or(0.0);
                    sb.partial_cmp(&sa)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| {
                            let na = self.meta.get(a).map(|m| m.name.as_str()).unwrap_or("");
                            let nb = self.meta.get(b).map(|m| m.name.as_str()).unwrap_or("");
                            na.cmp(nb)
                        })
                });
                for id in g_nodes {
                    let s = scores.get(&id).copied().unwrap_or(0.0);
                    rows.push(TreeRow::Node { id, score: s, depth: 2 });
                }
            }
        }
        rows
    }

    pub fn group_key_for(row: &TreeRow) -> Option<String> {
        match row {
            TreeRow::Tier { label, .. } => Some(label.to_string()),
            TreeRow::Group { tier_label, label, .. } => Some(format!("{}::{}", tier_label, label)),
            _ => None,
        }
    }

    /// Selected NodeId from the tree cursor (only valid if cursor sits on a Node row).
    pub fn selected_node_in_tree(&self) -> Option<NodeId> {
        match self.last_tree.get(self.tree_cursor)? {
            TreeRow::Node { id, .. } => Some(*id),
            _ => None,
        }
    }

    /// Clamp tree_cursor + tree_scroll for a freshly built tree.
    pub fn clamp_tree(&mut self, tree_len: usize, viewport_height: usize) {
        if tree_len == 0 {
            self.tree_cursor = 0;
            self.tree_scroll = 0;
            return;
        }
        if self.tree_cursor >= tree_len {
            self.tree_cursor = tree_len - 1;
        }
        // Auto-scroll: keep cursor in viewport.
        if self.tree_cursor < self.tree_scroll {
            self.tree_scroll = self.tree_cursor;
        } else if self.tree_cursor >= self.tree_scroll + viewport_height {
            self.tree_scroll = self.tree_cursor + 1 - viewport_height;
        }
    }


    /// Tokenise the query and return NodeIds whose qname or name contains
    /// any token (case-insensitive). Empty query → empty result.
    pub fn query_matches(&self) -> Vec<NodeId> {
        let q = self.query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let tokens: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<NodeId> = Vec::new();
        for (id, m) in &self.meta {
            let qname_l = m.qname.to_lowercase();
            let name_l = m.name.to_lowercase();
            if tokens
                .iter()
                .all(|t| qname_l.contains(t) || name_l.contains(t))
            {
                hits.push(*id);
            }
        }
        // Stable order: by qname.
        hits.sort_by(|a, b| {
            let qa = self.meta.get(a).map(|m| m.qname.as_str()).unwrap_or("");
            let qb = self.meta.get(b).map(|m| m.qname.as_str()).unwrap_or("");
            qa.cmp(qb)
        });
        hits
    }

    /// Outgoing neighbours of `id` as (other_id, edge_category).
    pub fn out_neighbours(&self, id: NodeId) -> Vec<(NodeId, repo_graph_core::EdgeCategoryId)> {
        self.adjacency_out
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    /// Incoming neighbours of `id`. We don't cache these; build on demand.
    pub fn in_neighbours(&self, id: NodeId) -> Vec<(NodeId, repo_graph_core::EdgeCategoryId)> {
        self.all_edges
            .iter()
            .filter(|e| e.to == id)
            .map(|e| (e.from, e.category))
            .collect()
    }

    pub fn query_push(&mut self, c: char) {
        self.query.push(c);
        self.frame = 0;
    }

    pub fn query_pop(&mut self) {
        self.query.pop();
        self.frame = 0;
    }

    pub fn query_clear(&mut self) {
        self.query.clear();
        self.frame = 0;
    }

    pub fn query_focus(&mut self, focused: bool) {
        self.query_focused = focused;
    }

    /// Promote the cursor's selected node to the active seed. Triggered by Enter.
    pub fn use_selected_as_seed(&mut self) {
        let Some(id) = self.selected_node_in_tree() else {
            self.last_message = Some("cursor not on a node".into());
            return;
        };
        if let Some(i) = self.seeds.iter().position(|&s| s == id) {
            self.seed_idx = i;
        } else {
            self.seeds.insert(0, id);
            self.seed_idx = 0;
        }
        self.frame = 0;
        let q = self
            .meta
            .get(&id)
            .map(|m| m.qname.as_str())
            .unwrap_or("?");
        self.last_message = Some(format!("seed → {}", q));
    }

    pub fn tick(&mut self) {
        if !self.paused {
            self.frame = self.frame.saturating_add(1);
        }
    }

    pub fn set_mode(&mut self, m: Mode) {
        if self.mode != m {
            self.mode = m;
            self.frame = 0;
        }
    }

    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
    }

    pub fn next_seed(&mut self) {
        if !self.seeds.is_empty() {
            self.seed_idx = (self.seed_idx + 1) % self.seeds.len();
            self.frame = 0;
        }
    }

    pub fn current_seed(&self) -> Option<NodeId> {
        self.seeds.get(self.seed_idx).copied()
    }

    pub fn render(&mut self, f: &mut Frame) {
        match self.mode {
            Mode::Ppr => modes::ppr::render(f, self),
            Mode::Flow => modes::flow::render(f, self),
            Mode::Build => modes::build::render(f, self),
        }
    }
}

fn collect_seed_candidates(meta: &HashMap<NodeId, NodeMeta>, nodes: &[NodeId]) -> Vec<NodeId> {
    let entry_kinds = [
        node_kind::ROUTE,
        node_kind::GRPC_SERVICE,
        node_kind::QUEUE_CONSUMER,
        node_kind::GRAPHQL_RESOLVER,
        node_kind::WS_HANDLER,
        node_kind::EVENT_HANDLER,
        node_kind::CLI_COMMAND,
        node_kind::CRON_JOB,
    ];
    let mut seeds: Vec<NodeId> = nodes
        .iter()
        .copied()
        .filter(|id| {
            meta.get(id)
                .is_some_and(|m| entry_kinds.contains(&m.kind))
        })
        .collect();
    seeds.sort_by(|a, b| {
        let qa = meta.get(a).map(|m| m.qname.as_str()).unwrap_or("");
        let qb = meta.get(b).map(|m| m.qname.as_str()).unwrap_or("");
        qa.cmp(qb)
    });
    seeds
}

/// Group label for a qname: everything up to and including the second-to-last
/// `::` segment, or the URL/path prefix for routes, or `(misc)`.
fn group_of(qname: &str) -> String {
    if qname.starts_with("route:") {
        // route:GET /auth/login → group "/auth"
        if let Some(after) = qname.split_whitespace().nth(1) {
            let segs: Vec<&str> = after.split('/').filter(|s| !s.is_empty()).collect();
            if let Some(first) = segs.first() {
                return format!("/{}", first);
            }
        }
        return "(routes)".to_string();
    }
    if qname.starts_with("package:") {
        // package:gomod:github.com/x/y → group "package:gomod"
        if let Some(idx) = qname.find(':') {
            if let Some(idx2) = qname[idx + 1..].find(':') {
                return qname[..idx + 1 + idx2].to_string();
            }
        }
        return "(packages)".to_string();
    }
    let parts: Vec<&str> = qname.split("::").collect();
    if parts.len() >= 2 {
        parts[..parts.len() - 1].join("::")
    } else if let Some(idx) = qname.find(':') {
        qname[..idx].to_string()
    } else {
        "(misc)".to_string()
    }
}
