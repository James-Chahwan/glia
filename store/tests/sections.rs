//! LC.5b: domain-owned container sections.
//!
//! The core archive (`Container`: header, repo, nodes, edges, node_kinds and
//! the section table) is domain-free; each domain writes its own state as a
//! named, 16-aligned byte range between the preamble and the core. The code
//! domain's nav / symbols / unresolved_* live in the `"code"` section; a toy
//! second domain proves the seam carries a section the store knows nothing
//! about.

use std::path::{Path, PathBuf};

use glia_core::{CellPayload, CellTypeId, Confidence, Node, NodeId, NodeKindId, RepoId};
use glia_graph::{RepoGraph, build_go};
use glia_parser_go::parse_file;
use glia_store::{
    CODE_SECTION, Container, EncodedSection, Header, MmapContainer, RegistryEntry, StoreError,
    code_section_of, decode_repo_graph, encode_repo_graph, encode_section, qname_of, read_to_owned,
    remove_cell, upsert_cell, write_container,
};

/// A second domain's navigation state, defined here and nowhere in the store.
#[derive(Debug, Clone, PartialEq)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
struct ToySection {
    frames: Vec<(u64, u32)>,
}

/// A second, differently-shaped section, so a file carries two.
#[derive(Debug, Clone, PartialEq)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
struct NoteSection {
    text: String,
}

const TOY_KIND_FRAME: NodeKindId = NodeKindId(9001);
const TOY_KIND_CLIP: NodeKindId = NodeKindId(9002);

fn toy_node(id: u64, repo: RepoId) -> Node {
    Node {
        id: NodeId(id),
        repo,
        confidence: Confidence::Strong,
        cells: vec![],
    }
}

/// A toy-domain core: its own `graph_type`, its own kinds, no code section.
fn toy_core() -> Container {
    let repo = RepoId::from_canonical("toy://clip-1");
    let mut header = Header::new("toy");
    header.node_kind_registry = vec![
        RegistryEntry { id: TOY_KIND_FRAME.0, name: "FRAME".into() },
        RegistryEntry { id: TOY_KIND_CLIP.0, name: "CLIP".into() },
    ];
    Container {
        header,
        repo,
        nodes: vec![toy_node(10, repo), toy_node(20, repo), toy_node(30, repo)],
        edges: Vec::new(),
        // Deliberately given out of order: the writer sorts by id.
        node_kinds: vec![
            (NodeId(30), TOY_KIND_FRAME),
            (NodeId(10), TOY_KIND_CLIP),
            (NodeId(20), TOY_KIND_FRAME),
        ],
        sections: Vec::new(),
    }
}

fn toy_section() -> ToySection {
    ToySection { frames: vec![(0, 7), (40, 9), (80, 11)] }
}

fn write_toy(dir: &Path, sections: &[EncodedSection]) -> PathBuf {
    let path = dir.join("toy.gmap");
    let mut core = toy_core();
    write_container(&path, &mut core, sections).unwrap();
    path
}

fn le_u64(bytes: &[u8], at: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(b)
}

#[test]
fn toy_domain_section_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let toy = encode_section("toy", &toy_section()).unwrap();
    let path = write_toy(tmp.path(), &[toy]);

    let m = MmapContainer::open(&path).unwrap();
    let archived = m.archived().unwrap();
    assert_eq!(archived.header.graph_type.as_str(), "toy");
    assert_eq!(archived.header.node_kind_registry.len(), 2);

    // The section comes back through the generic accessor, typed by the caller.
    let got = m
        .section::<ArchivedToySection>("toy")
        .unwrap()
        .expect("toy section present");
    let frames: ToySection = rkyv::deserialize::<ToySection, rkyv::rancor::Error>(got).unwrap();
    assert_eq!(frames, toy_section());

    // A toy file carries no code section, and the code accessors say so.
    assert_eq!(m.section_bytes(CODE_SECTION).unwrap(), None);
    assert!(code_section_of(&m).unwrap().is_none());
    assert_eq!(qname_of(&m, NodeId(10)).unwrap(), None);
    assert_eq!(
        m.section_names().unwrap().iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        vec!["toy"]
    );

    // node_kinds is core state: a domain-free reader names every node's kind.
    assert_eq!(archived.kind(NodeId(10)), Some(TOY_KIND_CLIP));
    assert_eq!(archived.kind(NodeId(20)), Some(TOY_KIND_FRAME));
    assert_eq!(archived.kind(NodeId(30)), Some(TOY_KIND_FRAME));
    assert_eq!(archived.kind(NodeId(99)), None);
    let ids: Vec<u64> = archived.node_kinds.iter().map(|e| e.0.0.to_native()).collect();
    assert_eq!(ids, vec![10, 20, 30], "node_kinds is written sorted by id");

    // The owned read-back carries the core and the section bytes verbatim.
    let owned = read_to_owned(&path).unwrap();
    assert_eq!(owned.core.nodes.len(), 3);
    assert_eq!(owned.core.header.graph_type, "toy");
    assert_eq!(owned.sections.len(), 1);
    assert_eq!(owned.sections[0].name, "toy");
    assert_eq!(
        owned.sections[0].bytes.as_slice(),
        m.section_bytes("toy").unwrap().unwrap()
    );
}

#[test]
fn sections_are_16_aligned() {
    let tmp = tempfile::tempdir().unwrap();
    let toy = encode_section("toy", &ToySection { frames: vec![(1, 1)] }).unwrap();
    let note = encode_section("note", &NoteSection { text: "abc".into() }).unwrap();
    let (toy_bytes, note_bytes) = (toy.bytes.to_vec(), note.bytes.to_vec());
    let path = tmp.path().join("two.gmap");
    let mut core = toy_core();
    write_container(&path, &mut core, &[toy, note]).unwrap();

    // The writer filled the table it laid out, in the caller's slice order.
    let names: Vec<&str> = core.sections.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["toy", "note"]);

    let raw = std::fs::read(&path).unwrap();
    let core_offset = le_u64(&raw, 16);
    let core_len = le_u64(&raw, 24);
    assert_eq!(core_offset % 16, 0, "core offset {core_offset} not 16-aligned");
    assert_eq!(core_offset + core_len, raw.len() as u64, "the core is the file's tail");

    let m = MmapContainer::open(&path).unwrap();
    let table = &m.archived().unwrap().sections;
    assert_eq!(table.len(), 2);
    let mut prev_end = 32u64;
    for (entry, owned) in table.iter().zip(&core.sections) {
        let (offset, len) = (entry.offset.to_native(), entry.len.to_native());
        assert_eq!(offset % 16, 0, "section {} offset {offset} not 16-aligned", entry.name);
        assert!(offset >= prev_end, "section {} overlaps its predecessor", entry.name);
        assert!(offset + len <= core_offset, "section {} runs into the core", entry.name);
        assert_eq!((offset, len), (owned.offset, owned.len));
        prev_end = offset + len;
    }
    assert_eq!(m.section_bytes("toy").unwrap().unwrap(), toy_bytes.as_slice());
    assert_eq!(m.section_bytes("note").unwrap().unwrap(), note_bytes.as_slice());
    let note = m.section::<ArchivedNoteSection>("note").unwrap().unwrap();
    assert_eq!(note.text.as_str(), "abc");
    assert!(m.section_bytes("absent").unwrap().is_none());

    // Deterministic: the same core and sections encode to the same bytes.
    let again = tmp.path().join("again.gmap");
    let mut core2 = toy_core();
    let toy2 = encode_section("toy", &ToySection { frames: vec![(1, 1)] }).unwrap();
    let note2 = encode_section("note", &NoteSection { text: "abc".into() }).unwrap();
    write_container(&again, &mut core2, &[toy2, note2]).unwrap();
    assert_eq!(std::fs::read(&again).unwrap(), raw);
}

#[test]
fn duplicate_section_names_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let a = encode_section("toy", &toy_section()).unwrap();
    let b = encode_section("toy", &ToySection { frames: vec![] }).unwrap();
    let path = tmp.path().join("dup.gmap");
    let mut core = toy_core();
    match write_container(&path, &mut core, &[a, b]) {
        Err(StoreError::Corrupt { detail }) => assert!(detail.contains("toy"), "{detail}"),
        other => panic!("duplicate section name: expected Corrupt, got {other:?}"),
    }
    assert!(!path.exists(), "nothing is written when the table is invalid");
}

#[test]
fn upsert_cell_preserves_sections() {
    let tmp = tempfile::tempdir().unwrap();
    let toy = encode_section("toy", &toy_section()).unwrap();
    let note = encode_section("note", &NoteSection { text: "keep me".into() }).unwrap();
    let path = write_toy(tmp.path(), &[toy, note]);
    let (toy_before, note_before) = {
        let m = MmapContainer::open(&path).unwrap();
        (
            m.section_bytes("toy").unwrap().unwrap().to_vec(),
            m.section_bytes("note").unwrap().unwrap().to_vec(),
        )
    };

    upsert_cell(&path, NodeId(20), CellTypeId(1), CellPayload::Text("frame 20".into())).unwrap();
    {
        let m = MmapContainer::open(&path).unwrap();
        let archived = m.archived().unwrap();
        let node = archived.nodes.iter().find(|n| n.id.0.to_native() == 20).unwrap();
        assert_eq!(node.cells.len(), 1, "the upsert landed");
        assert_eq!(m.section_bytes("toy").unwrap().unwrap(), toy_before.as_slice());
        assert_eq!(m.section_bytes("note").unwrap().unwrap(), note_before.as_slice());
        assert_eq!(archived.kind(NodeId(20)), Some(TOY_KIND_FRAME), "node_kinds survive");
    }

    assert!(remove_cell(&path, NodeId(20), CellTypeId(1)).unwrap());
    let m = MmapContainer::open(&path).unwrap();
    assert_eq!(m.section_bytes("toy").unwrap().unwrap(), toy_before.as_slice());
    assert_eq!(m.section_bytes("note").unwrap().unwrap(), note_before.as_slice());
    let toy = m.section::<ArchivedToySection>("toy").unwrap().unwrap();
    assert_eq!(toy.frames.len(), 3);
}

// ----------------------------------------------------------------------------
// The code domain through the same seam
// ----------------------------------------------------------------------------

const MODULE_PREFIX: &str = "example.com/backend";

fn backend_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/http_stack_smoke/backend")
}

fn go_repo() -> RepoId {
    RepoId::from_canonical("test://http_stack_smoke/backend")
}

fn build_backend() -> RepoGraph {
    let parses: Vec<_> = [("users/users.go", "users"), ("server/server.go", "server")]
        .iter()
        .map(|(rel, pkg)| {
            let src = std::fs::read_to_string(backend_root().join(rel)).unwrap();
            parse_file(&src, rel, pkg, MODULE_PREFIX, go_repo()).unwrap()
        })
        .collect();
    build_go(go_repo(), parses).unwrap()
}

#[test]
fn code_round_trip_unchanged() {
    let g = build_backend();
    assert!(!g.nodes.is_empty() && !g.edges.is_empty(), "fixture built an empty graph");
    assert!(!g.nav.parent_of.is_empty(), "fixture has no containment to round-trip");
    assert!(!g.symbols.module_by_qname.is_empty(), "fixture has no symbols to round-trip");

    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("backend.gmap");
    let bytes = encode_repo_graph(&g).unwrap();
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(bytes, encode_repo_graph(&g).unwrap(), "encoding is deterministic");

    let m = MmapContainer::open(&path).unwrap();
    assert_eq!(
        m.section_names().unwrap().iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        vec![CODE_SECTION]
    );
    assert_eq!(m.archived().unwrap().header.graph_type.as_str(), "code");
    assert_eq!(m.archived().unwrap().node_kinds.len(), g.nav.kind_by_id.len());

    let back = decode_repo_graph(&m).unwrap();
    assert_eq!(back.repo, g.repo);
    assert_eq!(back.nodes, g.nodes);
    assert_eq!(back.edges, g.edges);
    assert_eq!(back.nav.name_by_id, g.nav.name_by_id);
    assert_eq!(back.nav.qname_by_id, g.nav.qname_by_id);
    assert_eq!(back.nav.kind_by_id, g.nav.kind_by_id);
    assert_eq!(back.nav.parent_of, g.nav.parent_of);
    assert_eq!(back.nav.children_of, g.nav.children_of);
    assert_eq!(back.symbols.module_by_qname, g.symbols.module_by_qname);
    assert_eq!(back.symbols.module_symbols, g.symbols.module_symbols);
    assert_eq!(back.symbols.class_methods, g.symbols.class_methods);
    assert_eq!(back.symbols.module_import_bindings, g.symbols.module_import_bindings);
    assert_eq!(back.unresolved_calls, g.unresolved_calls);
    assert_eq!(back.unresolved_refs, g.unresolved_refs);

    // Point lookups: the qname reads the code section, the kind reads the core.
    let (&id, qname) = g.nav.qname_by_id.iter().next().unwrap();
    assert_eq!(qname_of(&m, id).unwrap().as_deref(), Some(qname.as_str()));
    assert_eq!(m.archived().unwrap().kind(id), g.nav.kind_by_id.get(&id).copied());
}

#[test]
fn a_nav_less_graph_writes_no_code_section() {
    // Nothing to carry, so nothing is written: the core alone round-trips it.
    let repo = RepoId::from_canonical("test://nav-less");
    let g = RepoGraph {
        repo,
        nodes: vec![toy_node(1, repo)],
        edges: vec![],
        nav: Default::default(),
        symbols: Default::default(),
        unresolved_calls: vec![],
        unresolved_refs: vec![],
        properties: Default::default(),
    };
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("bare.gmap");
    std::fs::write(&path, encode_repo_graph(&g).unwrap()).unwrap();
    let m = MmapContainer::open(&path).unwrap();
    assert!(m.section_names().unwrap().is_empty());
    let back = decode_repo_graph(&m).unwrap();
    assert_eq!(back.nodes, g.nodes);
    assert!(back.nav.qname_by_id.is_empty() && back.symbols.module_by_qname.is_empty());
}
