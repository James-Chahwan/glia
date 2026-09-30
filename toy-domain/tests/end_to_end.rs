//! LD.16: the toy-reel domain through every layer of the seam - built, passes
//! run, reachability, written to a `.gmap` and read back, activated - using
//! only core, activation and store.
//!
//! The toy registry reuses code's numbers (SHOT = 2 is code's CLASS,
//! FEATURES = 3 code's IMPORTS, TIMECODE = 1 code's CODE), so every name
//! asserted after the read-back proves the decode came from the file's header,
//! not from a compiled-in code table.
//!
//! fired_on marker (with `--nocapture`), grep tokens `[toy] reel:` and
//! `[passes] domain=toy-reel`:
//!   [toy] reel: built nodes=9 edges=9
//!   [passes] domain=toy-reel resolve=1 post=1 finalize=1
//!   [toy] reel: wrote sections=1 bytes=<n>
//!   [toy] reel: read graph_type=toy-reel kinds=3
//!   [toy] reel: activated kept=3 synth=3
//!
//! `toy_reel_communities` (CD.1c), grep token `[toy] reel: communities=`:
//!   [toy] reel: communities=<k> method=leiden pairs=10

use std::collections::HashSet;
use std::path::Path;

use glia_activation::algo::community::{CommunityOptions, Method, WeightedGraph, communities};
use glia_activation::algo::reach::reachable;
use glia_activation::algo::{Adjacency, Walk};
use glia_activation::{ActivatedView, ActivationPlan, Direction};
use glia_core::{CellPayload, CellTypeId, EdgeCategoryId, NodeId, NodeKindId};
use glia_store::{Header, MmapContainer, remove_cell, upsert_cell, write_container};
use glia_toy_domain::cell_type::{LABEL, SCREEN_TIME, TIMECODE};
use glia_toy_domain::edge_category::{CONTAINS_SHOT, FEATURES, NEXT_SHOT, SAME_OBJECT};
use glia_toy_domain::node_kind::{OBJECT, SCENE, SHOT};
use glia_toy_domain::{
    KindIs, NAV_SECTION, ReelNav, ScreenTimeSummary, TOY_PASSES, TOY_PROFILE, TOY_TABLES, ToyGraph,
    build, read_gmap,
};

const REEL: &str = include_str!("../fixtures/reel.txt");

fn id(g: &ToyGraph, kind: NodeKindId, index: u32) -> NodeId {
    g.id(kind, index)
        .unwrap_or_else(|| panic!("reel has no {kind:?} {index}"))
}

fn count(g: &ToyGraph, c: EdgeCategoryId) -> usize {
    g.edges.iter().filter(|e| e.category == c).count()
}

/// The fixture after its passes, and the pass report.
fn built_and_passed() -> ToyGraph {
    let mut g = build(REEL).unwrap();
    TOY_PROFILE.run_passes(&mut g, &());
    g
}

fn activate(g: &ToyGraph, seed: NodeId) -> ActivatedView {
    let mut cfg = TOY_TABLES.activation_config(Some("objects"));
    cfg.direction = Direction::Undirected;
    let (kind_is, summary) = (KindIs(OBJECT), ScreenTimeSummary);
    ActivationPlan::<ToyGraph>::new(cfg)
        .filter(&kind_is)
        .synth(&summary)
        .run(g, &[seed])
}

#[test]
fn reel_end_to_end() {
    // (1) built.
    let mut g = build(REEL).unwrap();
    assert_eq!((g.nodes.len(), g.edges.len()), (9, 9));
    assert_eq!(
        (
            count(&g, CONTAINS_SHOT),
            count(&g, NEXT_SHOT),
            count(&g, FEATURES)
        ),
        (3, 2, 4)
    );
    assert_eq!(g.nav.len(), 9);
    let (cat0, dog, cat2, bird) = (
        id(&g, OBJECT, 0),
        id(&g, OBJECT, 1),
        id(&g, OBJECT, 2),
        id(&g, OBJECT, 9),
    );
    let (shot0, shot1, shot9) = (id(&g, SHOT, 0), id(&g, SHOT, 1), id(&g, SHOT, 9));
    assert_eq!(g.timecode(shot1), Some((1200, 2600)));
    assert_eq!(g.label(cat2), Some("cat"));
    assert_eq!(
        g.cell(shot0, TIMECODE),
        Some(&CellPayload::Json(
            "{\"start_ms\":0,\"end_ms\":1200}".into()
        ))
    );

    assert_eq!(TOY_PROFILE.validate(), Ok(()));
    assert_eq!(
        TOY_PROFILE.cell_populators(),
        [("screen_time", &[SCREEN_TIME][..])]
    );
    let before = g.edges.clone();
    let report = TOY_PROFILE.run_passes(&mut g, &());
    assert_eq!((report.resolve, report.post, report.finalize), (1, 1, 1));
    assert_eq!(
        report.ran,
        ["reidentify_objects", "screen_time", "sort_edges"]
    );
    let added: Vec<_> = g
        .edges
        .iter()
        .filter(|e| !before.contains(e))
        .map(|e| e.key())
        .collect();
    // The pass pairs lower id -> higher; on this fixture's ids that is 0 -> 2.
    assert!(
        cat0.0 < cat2.0,
        "fixture ids: object/0 sorts before object/2"
    );
    assert_eq!(added, [(cat0, cat2, SAME_OBJECT)]);
    assert_eq!(g.edges.len(), 10);
    let keys: Vec<_> = g
        .edges
        .iter()
        .map(|e| (e.from.0, e.to.0, e.category.0))
        .collect();
    assert!(keys.is_sorted(), "sort_edges ran last");
    let json = |s: &str| Some(CellPayload::Json(s.to_string()));
    assert_eq!(
        g.cell(cat0, SCREEN_TIME).cloned(),
        json("{\"ms\":3400,\"shots\":2}")
    );
    assert_eq!(
        g.cell(cat2, SCREEN_TIME).cloned(),
        json("{\"ms\":3400,\"shots\":2}")
    );
    assert_eq!(
        g.cell(dog, SCREEN_TIME).cloned(),
        json("{\"ms\":1400,\"shots\":1}")
    );
    assert_eq!(
        g.cell(bird, SCREEN_TIME).cloned(),
        json("{\"ms\":500,\"shots\":1}")
    );
    // Passes are idempotent: a second run adds no edge and changes no cell.
    let once = g.clone();
    TOY_PROFILE.passes.run(&mut g, &());
    assert_eq!(g, once);

    // (2) live: forward reachability over the carry edges from the entries.
    let entries: Vec<NodeId> = g
        .nodes
        .iter()
        .map(|n| n.id)
        .filter(|n| TOY_TABLES.entry.is_entry(g.nav.kind(*n), "", &[]))
        .collect();
    assert_eq!(entries, [id(&g, SCENE, 0)]);
    let live = reachable(&Adjacency::carry(&g, &TOY_TABLES), &entries, Walk::Forward);
    let want: HashSet<NodeId> = [
        id(&g, SCENE, 0),
        shot0,
        shot1,
        id(&g, SHOT, 2),
        cat0,
        dog,
        cat2,
    ]
    .into_iter()
    .collect();
    assert_eq!(live, want);
    assert!(!live.contains(&shot9) && !live.contains(&bird));

    // (3) written + read back.
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (dir.path().join("reel.gmap"), dir.path().join("again.gmap"));
    let bytes = g.write_gmap(&a).unwrap();
    g.write_gmap(&b).unwrap();
    let (fa, fb) = (std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());
    assert_eq!(fa.len() as u64, bytes);
    assert!(fa == fb, "writing twice gives byte-identical files");

    let back = read_gmap(&a).unwrap();
    assert_eq!(back.header.graph_type, "toy-reel");
    // Names from the file. Code-domain would say CLASS / IMPORTS / CODE.
    assert_eq!(back.kind_name(shot1), Some("SHOT"));
    assert_eq!(back.category_name(FEATURES), Some("FEATURES"));
    assert_eq!(back.cell_name(TIMECODE), Some("TIMECODE"));
    let registry = |r: &[glia_store::RegistryEntry]| -> Vec<(u32, String)> {
        r.iter().map(|e| (e.id, e.name.clone())).collect()
    };
    assert_eq!(
        registry(&back.header.node_kind_registry)[1],
        (2, "SHOT".to_string())
    );
    assert_eq!(
        registry(&back.header.edge_category_registry)[2],
        (3, "FEATURES".to_string())
    );
    assert_eq!(
        registry(&back.header.cell_registry)[0],
        (1, "TIMECODE".to_string())
    );
    let names: HashSet<&str> = back
        .graph
        .nodes
        .iter()
        .filter_map(|n| back.kind_name(n.id))
        .collect();
    assert_eq!(
        names,
        ["SCENE", "SHOT", "OBJECT"]
            .into_iter()
            .collect::<HashSet<_>>()
    );
    assert_eq!(
        back.graph
            .nodes
            .iter()
            .filter(|n| back.kind_name(n.id).is_none())
            .count(),
        0
    );
    assert_eq!(
        back.graph, g,
        "nodes (cells incl.), edges, repo and ReelNav survive the round trip"
    );

    // The section is the domain's own opaque bytes; the core names every kind.
    let m = MmapContainer::open(&a).unwrap();
    let encoded = g.nav.encode();
    assert_eq!(
        m.section_names().unwrap(),
        [(NAV_SECTION.to_string(), encoded.len() as u64)]
    );
    assert_eq!(m.section_bytes(NAV_SECTION).unwrap(), Some(&encoded[..]));
    assert_eq!(ReelNav::decode(&encoded), Ok(g.nav.clone()));
    let archived = m.archived().unwrap();
    assert!(
        g.nodes
            .iter()
            .all(|n| archived.kind(n.id) == g.nav.kind(n.id))
    );

    // (4) activated, from shot/1 over the read-back graph.
    let view = activate(&back.graph, shot1);
    let kept: HashSet<NodeId> = view.ids().into_iter().collect();
    assert_eq!(kept, [cat0, dog, cat2].into_iter().collect::<HashSet<_>>());
    assert!(
        view.score_of(bird).is_none(),
        "object/9 is in another component"
    );
    assert_eq!(
        view.dropped,
        [("kind_is", 4)],
        "scene/0 and shots 0-2 dropped"
    );
    let texts: Vec<&str> = view.synth.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts.len(), 3);
    assert!(texts.contains(&"cat: 3400 ms across 2 shots"), "{texts:?}");
    assert!(texts.contains(&"dog: 1400 ms across 1 shots"), "{texts:?}");
    let anchors: Vec<Option<NodeId>> = view.synth.iter().map(|c| c.anchor).collect();
    assert_eq!(
        anchors,
        view.ids().into_iter().map(Some).collect::<Vec<_>>(),
        "one cell per kept object, view order"
    );

    let live_view = activate(&g, shot1);
    assert_eq!(view, live_view);
    let bits = |v: &ActivatedView| -> Vec<(u64, u64)> {
        v.scores.iter().map(|(n, s)| (n.0, s.to_bits())).collect()
    };
    assert_eq!(bits(&view), bits(&live_view), "scores to_bits-equal");
    eprintln!(
        "[toy] reel: activated kept={} synth={}",
        view.scores.len(),
        view.synth.len()
    );
}

/// CD.1c: communities over the toy reel, weighted by the domain's own
/// `community_weights` - the community algorithm runs on a non-code domain.
/// Shot 9 and the bird (object 9) share no edge with scene 0's nodes, so
/// they form a community of their own and never mix with scene 0's.
#[test]
fn toy_reel_communities() {
    let g = built_and_passed();
    assert_eq!(TOY_TABLES.validate(), Ok(()));
    let view = WeightedGraph::from_source(&g, TOY_TABLES.community_weights);
    // Every node of the reel, and every edge: 3 CONTAINS_SHOT (1) +
    // 2 NEXT_SHOT (3) + 4 FEATURES (2) + 1 SAME_OBJECT (4) = 21, so 2m = 42.
    assert_eq!(
        (view.len(), view.pair_count(), view.total_weight()),
        (9, 10, 42)
    );

    let opts = CommunityOptions::default();
    let part = communities(&view, &opts);
    assert_eq!(
        part.method,
        Method::Leiden,
        "a 10-pair view is under the Leiden cap"
    );
    assert_eq!(part.membership.len(), 9);
    let community_of = |n: NodeId| {
        let ix = view
            .index_of(n)
            .unwrap_or_else(|| panic!("{n:?} is in the view"));
        part.membership[ix as usize]
    };
    let (shot9, bird) = (id(&g, SHOT, 9), id(&g, OBJECT, 9));
    let stray = community_of(shot9);
    assert_eq!(community_of(bird), stray, "shot 9 features the bird");
    let members: HashSet<NodeId> = (0..view.len() as u32)
        .filter(|&ix| part.membership[ix as usize] == stray)
        .map(|ix| view.id(ix))
        .collect();
    assert_eq!(members, [shot9, bird].into_iter().collect::<HashSet<_>>());
    let scene0 = [
        id(&g, SCENE, 0),
        id(&g, SHOT, 0),
        id(&g, SHOT, 1),
        id(&g, SHOT, 2),
        id(&g, OBJECT, 0),
        id(&g, OBJECT, 1),
        id(&g, OBJECT, 2),
    ];
    for n in scene0 {
        assert_ne!(
            community_of(n),
            stray,
            "{n:?} is scene 0's, never the stray shot's"
        );
    }
    assert!(part.communities >= 2 && part.modularity > 0.0, "{part:?}");
    // One seed, one answer.
    assert_eq!(communities(&view, &opts), part);
    eprintln!(
        "[toy] reel: communities={} method={} pairs={}",
        part.communities,
        part.method.name(),
        view.pair_count()
    );
}

/// The reader names ids from the file's header and nothing else: a header that
/// calls kind 2 by code's name reads back as CLASS, and a header with empty
/// registries is refused rather than filled from a compiled-in table.
#[test]
fn names_come_from_the_file_header() {
    let g = built_and_passed();
    let shot1 = id(&g, SHOT, 1);
    let dir = tempfile::tempdir().unwrap();
    let write_with = |header: Header, name: &str| {
        let (mut core, sections) = g.to_container().unwrap();
        core.header = header;
        let path = dir.path().join(name);
        write_container(&path, &mut core, &sections).unwrap();
        path
    };

    let code_names = Header::for_domain(
        "toy-reel",
        &[(1, "MODULE"), (2, "CLASS"), (3, "FUNCTION")],
        &[
            (1, "DEFINES"),
            (2, "CONTAINS"),
            (3, "IMPORTS"),
            (4, "CALLS"),
        ],
        &[(1, "CODE"), (2, "DOC"), (3, "POSITION")],
    )
    .unwrap();
    let back = read_gmap(&write_with(code_names, "code-names.gmap")).unwrap();
    assert_eq!(back.kind_name(shot1), Some("CLASS"));
    assert_eq!(back.category_name(FEATURES), Some("IMPORTS"));
    assert_eq!(back.cell_name(TIMECODE), Some("CODE"));
    assert_eq!(
        back.graph, g,
        "the graph itself never depended on the names"
    );

    let empty = write_with(Header::new("toy-reel"), "empty.gmap");
    let e = read_gmap(&empty).unwrap_err();
    assert!(e.contains("kind not named by the file"), "{e}");
    let other = write_with(Header::new("code"), "code.gmap");
    assert!(read_gmap(&other).unwrap_err().contains("not \"toy-reel\""));
}

/// The store's own cell mutation over a toy file rewrites the core and carries
/// the domain's section through byte for byte.
#[test]
fn store_cell_mutation_keeps_the_section() {
    let g = built_and_passed();
    let bird = id(&g, OBJECT, 9);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reel.gmap");
    g.write_gmap(&path).unwrap();
    let section = |p: &Path| {
        let m = MmapContainer::open(p).unwrap();
        m.section_bytes(NAV_SECTION).unwrap().map(<[u8]>::to_vec)
    };
    let before = section(&path);

    let payload = CellPayload::Json("{\"ms\":0,\"shots\":0}".to_string());
    upsert_cell(&path, bird, SCREEN_TIME, payload.clone()).unwrap();
    assert_eq!(section(&path), before);
    let back = read_gmap(&path).unwrap();
    assert_eq!(back.graph.cell(bird, SCREEN_TIME), Some(&payload));
    assert_eq!(back.graph.screen_time(bird), Some((0, 0)));
    assert_eq!(back.graph.nav, g.nav);

    assert!(remove_cell(&path, bird, SCREEN_TIME).unwrap());
    assert_eq!(section(&path), before);
    assert_eq!(
        read_gmap(&path).unwrap().graph.cell(bird, SCREEN_TIME),
        None
    );
}

/// Each pass writes exactly the cell types it declares: none undeclared, and
/// every declared type observed on the fixture.
#[test]
fn passes_populate_exactly_what_they_declare() {
    let cell_set = |g: &ToyGraph| -> HashSet<(u64, u32, String)> {
        g.nodes
            .iter()
            .flat_map(|n| {
                n.cells
                    .iter()
                    .map(move |c| (n.id.0, c.kind.0, format!("{:?}", c.payload)))
            })
            .collect()
    };
    let mut g = build(REEL).unwrap();
    for spec in TOY_PASSES.order() {
        let before = cell_set(&g);
        (spec.run)(&mut g, &());
        let written: HashSet<CellTypeId> = cell_set(&g)
            .difference(&before)
            .map(|(_, t, _)| CellTypeId(*t))
            .collect();
        let declared: HashSet<CellTypeId> = spec.populates.iter().copied().collect();
        assert_eq!(written, declared, "pass {}", spec.name);
    }
    assert!(
        g.nodes
            .iter()
            .all(|n| n.cells.iter().all(|c| c.kind != SCREEN_TIME)
                || g.nav.kind(n.id) == Some(OBJECT))
    );
    assert_eq!(
        built_and_passed(),
        g,
        "one pass at a time equals run_passes"
    );
    assert_eq!(
        g.ids_of(OBJECT)
            .iter()
            .filter(|o| g.cell(**o, LABEL).is_some())
            .count(),
        4
    );
}

/// The dependency set is exactly the domain-free layers, and no other crate of
/// the workspace depends on this one.
#[test]
fn dependency_guard() {
    let deps = table_keys(include_str!("../Cargo.toml"), "dependencies");
    assert_eq!(
        deps,
        [
            "glia-activation",
            "glia-core",
            "glia-store"
        ]
    );

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let members = string_array(&manifest, "members");
    let excluded = string_array(&manifest, "exclude");
    assert!(
        members.iter().any(|m| m == "toy-domain"),
        "toy-domain is a workspace member"
    );
    let mut checked = 0;
    for crate_dir in members
        .iter()
        .chain(&excluded)
        .filter(|m| *m != "toy-domain")
    {
        let Ok(text) = std::fs::read_to_string(root.join(crate_dir).join("Cargo.toml")) else {
            continue;
        };
        checked += 1;
        assert!(
            !text.contains("toy-domain"),
            "{crate_dir}/Cargo.toml names toy-domain"
        );
    }
    assert!(
        checked >= members.len() - 1,
        "every other member's manifest was read"
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

/// The quoted strings of the top-level array `key = [ ... ]`.
fn string_array(manifest: &str, key: &str) -> Vec<String> {
    let Some(start) = manifest
        .lines()
        .position(|l| l.trim_start().starts_with(&format!("{key} =")))
    else {
        return Vec::new();
    };
    let body: Vec<&str> = manifest.lines().skip(start).collect();
    let mut out = Vec::new();
    for line in body {
        let code = line.split('#').next().unwrap_or("");
        out.extend(code.split('"').skip(1).step_by(2).map(str::to_string));
        if code.contains(']') {
            break;
        }
    }
    out
}
