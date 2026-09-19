//! Domain-free inspection of a `.gmap` file or a sharded layout (LC.4).
//!
//! Everything here reads the domain-free core (`ArchivedContainer`: header,
//! nodes, edges, `node_kinds`, section table) and nothing else. Ids are named
//! from THAT file's header registries only (`Header::for_domain` /
//! `Header::for_code` fill them), never from a domain crate: this module must
//! not link code-domain, so a file written by a newer build with ids this build
//! has never heard of is still labelled by the names it carries, and a non-code
//! domain's file is labelled by its own. An id the file's header does not name
//! is shown as `#<id>` and counted in `unregistered`.

use std::collections::BTreeMap;
use std::path::Path;

use crate::container::{ArchivedContainer, ArchivedRegistryEntry, MmapContainer};
use crate::error::StoreError;
use crate::layout::MANIFEST_NAME;

/// One id of one table (node kind, edge category, cell type) with its name and
/// how often it occurs.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct NamedCount {
    pub id: u32,
    /// The name the file's header registers for `id`, or `#<id>` when it
    /// registers none.
    pub name: String,
    pub count: u64,
}

/// What one `.gmap` file holds, named from its own header.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct ShardInspection {
    /// The manifest's shard name (`cross_stack` for the cross-edge shard), or
    /// the file stem when a single file was inspected.
    pub name: String,
    pub graph_type: String,
    /// The archived header's format version.
    pub format: u32,
    pub nodes: u64,
    pub edges: u64,
    /// Node count per kind, from the core's `node_kinds`, sorted by id.
    pub kinds: Vec<NamedCount>,
    /// Edge count per category, sorted by id.
    pub categories: Vec<NamedCount>,
    /// Node-cell count per cell type, sorted by id.
    pub node_cells: Vec<NamedCount>,
    /// Edge-cell count per cell type (LC.2's edge cell slot), sorted by id.
    pub edge_cells: Vec<NamedCount>,
    /// The file's named sections as `(name, byte length)`, in table order.
    pub sections: Vec<(String, u64)>,
    /// How many distinct ids across the four tables above this file's header
    /// does not name.
    pub unregistered: u64,
}

/// The four tables summed over every shard. A name is the first one a shard's
/// header registers for that id (shards in inspection order); `unregistered`
/// counts the distinct ids no shard names.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct InspectionTotals {
    pub nodes: u64,
    pub edges: u64,
    pub kinds: Vec<NamedCount>,
    pub categories: Vec<NamedCount>,
    pub node_cells: Vec<NamedCount>,
    pub edge_cells: Vec<NamedCount>,
    pub unregistered: u64,
}

/// A `.gmap` file or sharded layout, decoded without any domain crate.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct Inspection {
    /// The path inspected, as given.
    pub path: String,
    /// The manifest's `schema_version`; `None` for a single file.
    pub manifest_schema: Option<u32>,
    /// The manifest's `build_stamp`; `None` for a single file or a manifest
    /// that records none.
    pub build_stamp: Option<String>,
    /// One entry per shard: the manifest's shards in order, then the
    /// cross-edge shard; or the one file inspected.
    pub shards: Vec<ShardInspection>,
    pub totals: InspectionTotals,
}

impl Inspection {
    /// The `[inspect]` fired_on line: `[inspect] <path>: shards=<n>
    /// graph_types=<a,b> format=<v> kinds=<k> categories=<c> node_cells=<n>
    /// edge_cells=<e> unregistered=<u>`, where kinds / categories / cells are
    /// the number of distinct ids in the totals and graph types / formats are
    /// the distinct values across shards (`-` when there are no shards).
    pub fn marker(&self) -> String {
        let joined = |mut v: Vec<String>| {
            v.sort();
            v.dedup();
            if v.is_empty() { "-".to_string() } else { v.join(",") }
        };
        let graph_types = joined(self.shards.iter().map(|s| s.graph_type.clone()).collect());
        let formats = joined(self.shards.iter().map(|s| s.format.to_string()).collect());
        let t = &self.totals;
        format!(
            "[inspect] {}: shards={} graph_types={graph_types} format={formats} kinds={} \
             categories={} node_cells={} edge_cells={} unregistered={}",
            self.path,
            self.shards.len(),
            t.kinds.len(),
            t.categories.len(),
            t.node_cells.len(),
            t.edge_cells.len(),
            t.unregistered,
        )
    }
}

/// Inspect `path`: a layout directory (its `manifest.json` lists the shards)
/// or a single `.gmap` file. Every shard is opened with `MmapContainer::open`,
/// so an old-format, future-format or damaged file fails with the same
/// `StoreError` (and "rebuild" text) a loader reports. The manifest is read
/// leniently - only its schema, build stamp and shard paths - so inspection
/// names the shard that cannot be read rather than stopping at the manifest;
/// content hashes are not checked (`ShardedMmap::open` does that).
pub fn inspect_path(path: &Path) -> Result<Inspection, StoreError> {
    let (manifest_schema, build_stamp, files) = if path.is_dir() {
        let bytes = std::fs::read(path.join(MANIFEST_NAME))?;
        let m: ManifestView = serde_json::from_slice(&bytes)?;
        let files: Vec<(String, std::path::PathBuf)> = m
            .shards
            .iter()
            .chain(m.cross.as_ref())
            .map(|s| (s.name.clone(), path.join(&s.path)))
            .collect();
        let stamp = Some(m.build_stamp).filter(|s| !s.is_empty());
        (Some(m.schema_version), stamp, files)
    } else {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        (None, None, vec![(name, path.to_path_buf())])
    };

    let mut shards = Vec::with_capacity(files.len());
    let mut totals = Tables::default();
    let (mut nodes, mut edges) = (0u64, 0u64);
    for (name, file) in files {
        let m = MmapContainer::open(&file)?;
        let archived = m.archived()?;
        let tables = Tables::of(archived);
        nodes += archived.nodes.len() as u64;
        edges += archived.edges.len() as u64;
        totals.absorb(&tables);
        let named = tables.named();
        shards.push(ShardInspection {
            name,
            graph_type: archived.header.graph_type.as_str().to_string(),
            format: archived.header.version.to_native(),
            nodes: archived.nodes.len() as u64,
            edges: archived.edges.len() as u64,
            kinds: named.kinds,
            categories: named.categories,
            node_cells: named.node_cells,
            edge_cells: named.edge_cells,
            sections: m.section_names()?,
            unregistered: named.unregistered,
        });
    }
    let named = totals.named();
    Ok(Inspection {
        path: path.display().to_string(),
        manifest_schema,
        build_stamp,
        shards,
        totals: InspectionTotals {
            nodes,
            edges,
            kinds: named.kinds,
            categories: named.categories,
            node_cells: named.node_cells,
            edge_cells: named.edge_cells,
            unregistered: named.unregistered,
        },
    })
}

/// The manifest fields inspection needs, parsed from a manifest of any schema.
#[derive(serde::Deserialize)]
struct ManifestView {
    schema_version: u32,
    #[serde(default)]
    build_stamp: String,
    #[serde(default)]
    shards: Vec<ShardRef>,
    #[serde(default)]
    cross: Option<ShardRef>,
}

#[derive(serde::Deserialize)]
struct ShardRef {
    name: String,
    path: String,
}

/// Counts per id for one table, plus the names a header registers for the
/// ids that occur.
#[derive(Default)]
struct Table {
    counts: BTreeMap<u32, u64>,
    names: BTreeMap<u32, String>,
}

impl Table {
    fn count(&mut self, id: u32) {
        *self.counts.entry(id).or_default() += 1;
    }

    /// Record, for every id counted, the name `registry` gives it (if any).
    fn name_from(&mut self, registry: &[ArchivedRegistryEntry]) {
        for e in registry {
            let id = e.id.to_native();
            if self.counts.contains_key(&id) {
                self.names.entry(id).or_insert_with(|| e.name.as_str().to_string());
            }
        }
    }

    /// Add `other`'s counts; keep this table's name where both have one.
    fn absorb(&mut self, other: &Table) {
        for (id, n) in &other.counts {
            *self.counts.entry(*id).or_default() += n;
        }
        for (id, name) in &other.names {
            self.names.entry(*id).or_insert_with(|| name.clone());
        }
    }

    /// The counts as `NamedCount`s sorted by id, and how many ids are unnamed.
    fn named(&self) -> (Vec<NamedCount>, u64) {
        let mut unregistered = 0u64;
        let out = self
            .counts
            .iter()
            .map(|(id, count)| {
                let name = match self.names.get(id) {
                    Some(n) => n.clone(),
                    None => {
                        unregistered += 1;
                        format!("#{id}")
                    }
                };
                NamedCount { id: *id, name, count: *count }
            })
            .collect();
        (out, unregistered)
    }
}

/// The four tables of one file (or of a running total).
#[derive(Default)]
struct Tables {
    kinds: Table,
    categories: Table,
    node_cells: Table,
    edge_cells: Table,
}

struct Named {
    kinds: Vec<NamedCount>,
    categories: Vec<NamedCount>,
    node_cells: Vec<NamedCount>,
    edge_cells: Vec<NamedCount>,
    unregistered: u64,
}

impl Tables {
    /// Count `a`'s kinds, categories and cells, and name them from `a`'s own
    /// header registries.
    fn of(a: &ArchivedContainer) -> Self {
        let mut t = Tables::default();
        for pair in a.node_kinds.iter() {
            t.kinds.count(pair.1.0.to_native());
        }
        for n in a.nodes.iter() {
            for c in n.cells.iter() {
                t.node_cells.count(c.kind.0.to_native());
            }
        }
        for e in a.edges.iter() {
            t.categories.count(e.category.0.to_native());
            for c in e.cells.iter() {
                t.edge_cells.count(c.kind.0.to_native());
            }
        }
        let h = &a.header;
        t.kinds.name_from(&h.node_kind_registry);
        t.categories.name_from(&h.edge_category_registry);
        t.node_cells.name_from(&h.cell_registry);
        t.edge_cells.name_from(&h.cell_registry);
        t
    }

    fn absorb(&mut self, other: &Tables) {
        self.kinds.absorb(&other.kinds);
        self.categories.absorb(&other.categories);
        self.node_cells.absorb(&other.node_cells);
        self.edge_cells.absorb(&other.edge_cells);
    }

    fn named(&self) -> Named {
        let (kinds, uk) = self.kinds.named();
        let (categories, uc) = self.categories.named();
        let (node_cells, un) = self.node_cells.named();
        let (edge_cells, ue) = self.edge_cells.named();
        Named { kinds, categories, node_cells, edge_cells, unregistered: uk + uc + un + ue }
    }
}
