//! HT.4: the build-target domain through the seam - a graph of bazelbuild's
//! cpp-tutorial/stage3 shape built by hand, its passes run, reachability over
//! the carry edges, written to a `targets.gmap` and read back - using only
//! core, activation and store.
//!
//! The ids sit in the `build` block (`0x0002`), so every name asserted after
//! the read-back comes from the file's own header: TARGET is `0x00020001`,
//! an id no code table holds.
//!
//! fired_on markers (with `--nocapture`), grep tokens `[passes]
//! domain=build-targets` and `[build-targets] gmap`:
//!   [passes] domain=build-targets resolve=0 post=1 finalize=1
//!   [build-targets] gmap wrote nodes=10 edges=15 bytes=<n>
//!   [build-targets] gmap read nodes=10 edges=15

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use glia_activation::algo::reach::reachable;
use glia_activation::algo::{Adjacency, Walk};
use glia_build_domain::cell_type::{ATTRS, LOCATION, ROLE, VIA};
use glia_build_domain::edge_category::{DEPENDS_ON, HAS_SRC, IN_PACKAGE};
use glia_build_domain::node_kind::{PACKAGE, SOURCE, TARGET};
use glia_build_domain::{
    BUILD_PASSES, BUILD_PROFILE, BUILD_TABLES, BuildGraph, LABELS_SECTION, read_gmap,
};
use glia_core::{Cell, CellPayload, CellTypeId, NodeId, RepoId};
use glia_store::{Header, MmapContainer, write_container};

const WS: &str = "cpp";

fn json(s: &str) -> CellPayload {
    CellPayload::Json(s.to_string())
}

fn cell(kind: CellTypeId, payload: CellPayload) -> Cell {
    Cell { kind, payload }
}

fn location(file: &str, line: u32) -> Cell {
    cell(
        LOCATION,
        json(&format!("{{\"file\":\"{file}\",\"line\":{line}}}")),
    )
}

fn attrs(rule_class: &str, testonly: bool, visibility: &[&str]) -> Cell {
    let vis: Vec<String> = visibility.iter().map(|v| format!("\"{v}\"")).collect();
    cell(
        ATTRS,
        json(&format!(
            "{{\"rule_class\":\"{rule_class}\",\"tags\":[],\"testonly\":{testonly},\"visibility\":[{}],\"workspace\":\"{WS}\"}}",
            vis.join(",")
        )),
    )
}

/// cpp-tutorial/stage3 (bazelbuild/examples), workspace `cpp`: hello-world
/// (cc_binary) depends on hello-greet (cc_library, //main) and hello-time
/// (cc_library, //lib); every source file a SOURCE, every target and source
/// IN_PACKAGE its package. Passes not yet run.
fn stage3() -> BuildGraph {
    let mut g = BuildGraph::new(RepoId::from_canonical("build://cpp-tutorial"));
    let main = g.add_node(PACKAGE, WS, "//main", Vec::new());
    let lib = g.add_node(PACKAGE, WS, "//lib", Vec::new());
    let target = |g: &mut BuildGraph, label: &str, class: &str, file: &str, line, vis: &[&str]| {
        g.add_node(
            TARGET,
            WS,
            label,
            vec![location(file, line), attrs(class, false, vis)],
        )
    };
    let world = target(
        &mut g,
        "//main:hello-world",
        "cc_binary",
        "cpp/main/BUILD",
        11,
        &[],
    );
    let greet = target(
        &mut g,
        "//main:hello-greet",
        "cc_library",
        "cpp/main/BUILD",
        3,
        &[],
    );
    let time = target(
        &mut g,
        "//lib:hello-time",
        "cc_library",
        "cpp/lib/BUILD",
        3,
        &["//main:__pkg__"],
    );
    let source = |g: &mut BuildGraph, pkg: &str, name: &str| {
        g.add_node(
            SOURCE,
            WS,
            &format!("//{pkg}:{name}"),
            vec![location(&format!("cpp/{pkg}/{name}"), 0)],
        )
    };
    let world_cc = source(&mut g, "main", "hello-world.cc");
    let greet_cc = source(&mut g, "main", "hello-greet.cc");
    let greet_h = source(&mut g, "main", "hello-greet.h");
    let time_cc = source(&mut g, "lib", "hello-time.cc");
    let time_h = source(&mut g, "lib", "hello-time.h");

    g.add_edge(world, greet, DEPENDS_ON, &["deps"]);
    g.add_edge(world, time, DEPENDS_ON, &["deps"]);
    g.add_edge(world, world_cc, HAS_SRC, &["srcs"]);
    g.add_edge(greet, greet_cc, HAS_SRC, &["srcs"]);
    g.add_edge(greet, greet_h, HAS_SRC, &["hdrs"]);
    g.add_edge(time, time_cc, HAS_SRC, &["srcs"]);
    g.add_edge(time, time_h, HAS_SRC, &["hdrs"]);
    for (n, pkg) in [
        (world, main),
        (greet, main),
        (world_cc, main),
        (greet_cc, main),
        (greet_h, main),
        (time, lib),
        (time_cc, lib),
        (time_h, lib),
    ] {
        g.add_edge(n, pkg, IN_PACKAGE, &[]);
    }
    g.meta.insert("data_hash".into(), "0123abcd".into());
    g
}

fn id(g: &BuildGraph, kind: glia_core::NodeKindId, label: &str) -> NodeId {
    g.id(kind, WS, label)
        .unwrap_or_else(|| panic!("no {label} in the graph"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("glia-build-domain-round-trip");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(format!("{}-{name}", std::process::id()))
}

#[test]
fn stage3_round_trip() {
    let mut g = stage3();
    assert_eq!((g.nodes.len(), g.edges.len()), (10, 15));
    assert_eq!(BUILD_PROFILE.validate(), Ok(()));
    assert_eq!(BUILD_TABLES.validate_blocks(true), Ok(()));

    let report = BUILD_PROFILE.run_passes(&mut g, &());
    assert_eq!((report.resolve, report.post, report.finalize), (0, 1, 1));
    assert_eq!(report.ran, ["role_cells", "sort_edges"]);
    let keys: Vec<_> = g
        .edges
        .iter()
        .map(|e| (e.from.0, e.to.0, e.category.0))
        .collect();
    assert!(keys.is_sorted(), "sort_edges ran last");

    let world = id(&g, TARGET, "//main:hello-world");
    let greet = id(&g, TARGET, "//main:hello-greet");
    let time = id(&g, TARGET, "//lib:hello-time");
    assert_eq!(g.role(world), Some("binary"));
    assert_eq!(g.role(greet), Some("library"));
    assert_eq!(g.role(time), Some("library"));
    assert_eq!(g.role(id(&g, SOURCE, "//main:hello-greet.h")), None);

    // Passes are idempotent.
    let once = g.clone();
    BUILD_PASSES.run(&mut g, &());
    assert_eq!(g, once);

    // Live: forward reachability over the carry edges from the entries.
    let entries: Vec<NodeId> = g
        .nodes
        .iter()
        .map(|n| n.id)
        .filter(|n| g.is_entry(*n))
        .collect();
    assert_eq!(entries, [world]);
    let live = reachable(
        &Adjacency::carry(&g, &BUILD_TABLES),
        &entries,
        Walk::Forward,
    );
    let want: HashSet<NodeId> = [
        world,
        greet,
        time,
        id(&g, SOURCE, "//main:hello-world.cc"),
        id(&g, SOURCE, "//main:hello-greet.cc"),
        id(&g, SOURCE, "//main:hello-greet.h"),
        id(&g, SOURCE, "//lib:hello-time.cc"),
        id(&g, SOURCE, "//lib:hello-time.h"),
    ]
    .into_iter()
    .collect();
    assert_eq!(live, want, "IN_PACKAGE carries nothing");

    // Backward from a header: what a change to it affects.
    let time_h = id(&g, SOURCE, "//lib:hello-time.h");
    let affected = reachable(
        &Adjacency::carry(&g, &BUILD_TABLES),
        &[time_h],
        Walk::Backward,
    );
    let want: HashSet<NodeId> = [time_h, time, world].into_iter().collect();
    assert_eq!(affected, want);

    // Written twice, byte-identical, and read back whole.
    let (a, b) = (scratch("stage3.gmap"), scratch("again.gmap"));
    let bytes = g.write_gmap(&a).unwrap();
    g.write_gmap(&b).unwrap();
    let (fa, fb) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert_eq!(fa.len() as u64, bytes);
    assert!(fa == fb, "writing twice gives byte-identical files");

    let back = read_gmap(&a).unwrap();
    assert_eq!(back.graph, g, "nodes, edges, cells, labels and meta");
    assert_eq!(back.graph.labels, g.labels);
    assert_eq!(
        back.graph.meta.get("data_hash").map(String::as_str),
        Some("0123abcd")
    );
    let via: Vec<Vec<&str>> = back
        .graph
        .edges
        .iter()
        .filter(|e| e.from == greet && e.category == HAS_SRC)
        .map(BuildGraph::via)
        .collect();
    let mut via = via;
    via.sort();
    assert_eq!(via, [vec!["hdrs"], vec!["srcs"]]);

    // Names from the FILE: the header's registries, read straight off disk.
    assert_eq!(back.header.graph_type, "build-targets");
    let m = MmapContainer::open(&a).unwrap();
    let kinds: Vec<(u32, String)> = m
        .archived()
        .unwrap()
        .header
        .node_kind_registry
        .iter()
        .map(|e| (e.id.to_native(), e.name.as_str().to_string()))
        .collect();
    assert_eq!(
        kinds,
        [
            (0x0002_0001, "TARGET".to_string()),
            (0x0002_0002, "PACKAGE".to_string()),
            (0x0002_0003, "SOURCE".to_string()),
            (0x0002_0004, "EXTERNAL".to_string()),
        ]
    );
    assert!(m.section_bytes(LABELS_SECTION).unwrap().is_some());
    assert_eq!(back.kind_name(world), Some("TARGET"));
    assert_eq!(back.category_name(HAS_SRC), Some("HAS_SRC"));
    assert_eq!(back.cell_name(VIA), Some("VIA"));
    assert_eq!(back.cell_name(ROLE), Some("ROLE"));

    for p in [a, b] {
        let _ = std::fs::remove_file(p);
    }
}

/// A file of another graph type, or whose header does not name its ids, is
/// refused rather than read through a compiled-in table.
#[test]
fn reader_refuses_foreign_files() {
    let mut g = stage3();
    BUILD_PROFILE.run_passes(&mut g, &());
    let write_with = |header: Header, name: &str| {
        let (mut core, sections) = g.to_container().unwrap();
        core.header = header;
        let path = scratch(name);
        write_container(&path, &mut core, &sections).unwrap();
        path
    };
    let empty = write_with(Header::new("build-targets"), "empty.gmap");
    let e = read_gmap(&empty).unwrap_err();
    assert!(e.contains("kind not named by the file"), "{e}");
    let code = write_with(Header::new("code"), "code.gmap");
    let e = read_gmap(&code).unwrap_err();
    assert!(e.contains("not \"build-targets\""), "{e}");

    let (mut core, _) = g.to_container().unwrap();
    let bare = scratch("bare.gmap");
    write_container(&bare, &mut core, &[]).unwrap();
    let e = read_gmap(&bare).unwrap_err();
    assert!(e.contains("no build-targets.labels section"), "{e}");
    for p in [empty, code, bare] {
        let _ = std::fs::remove_file(p);
    }
}

/// The dependency set is exactly the domain-free layers and serde: no
/// code-domain, graph, engine or parser crate.
#[test]
fn dependency_guard() {
    let deps = table_keys(include_str!("../Cargo.toml"), "dependencies");
    assert_eq!(
        deps,
        [
            "glia-activation",
            "glia-core",
            "glia-store",
            "serde",
            "serde_json"
        ]
    );
}

/// The keys of `[name]` in a Cargo manifest, sorted.
fn table_keys(manifest: &str, name: &str) -> Vec<String> {
    let header = format!("[{name}]");
    let mut keys: Vec<String> = manifest
        .lines()
        .map(str::trim)
        .skip_while(|l| *l != header)
        .skip(1)
        .take_while(|l| !l.starts_with('['))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, _)| k.trim().to_string()))
        .collect();
    keys.sort();
    keys
}
