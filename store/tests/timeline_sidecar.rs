//! CD.5b: the timeline sidecar `<layout>/timeline.gmap` - a preamble'd
//! container with one `timeline` section - round-trips, is skipped when
//! unchanged, reads back `None` when absent, and reports an old / damaged file
//! the way every `.gmap` does (`needs_rebuild`).

use std::path::Path;

use glia_core::RepoId;
use glia_store::{
    FORMAT_VERSION, MmapContainer, StoreError, TIMELINE_FILE, TIMELINE_OPEN, TIMELINE_SECTION,
    TIMELINE_SUBJECT_CAP, TimelineEdge, TimelineNode, TimelineRev, TimelineStore, decode_timeline,
    inspect_path, read_timeline, timeline_subject, write_timeline,
};

const SHA_0: &str = "a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0";
const SHA_1: &str = "b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1";
const SHA_2: &str = "c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2c2";

fn rev(sha: &str, time: i64, subject: &str) -> TimelineRev {
    TimelineRev { sha: sha.to_string(), time, subject: timeline_subject(subject) }
}

/// Three revs: `f` calls `g` from rev 0 until rev 2, `f` calls `h` from rev 1
/// on; `f` moved (new id) at rev 2, `h` has no file.
fn sample() -> TimelineStore {
    TimelineStore {
        repo: 0x5eed,
        revs: vec![
            rev(SHA_0, 1_760_000_000, "first: f calls g"),
            rev(SHA_1, 1_760_000_100, "add h"),
            rev(SHA_2, 1_760_000_200, "move a.py to b.py\n\nbody text is not stored"),
        ],
        strings: vec!["app::b::f".into(), "app/b.py".into(), "app::a::g".into(), "app::a::h".into()],
        nodes: vec![
            TimelineNode {
                id: 11,
                kind: 3,
                qname: 0,
                file: 2,
                line: 1,
                from_rev: 0,
                until_rev: TIMELINE_OPEN,
                prior: vec![(2, 10)],
            },
            TimelineNode { id: 20, kind: 3, qname: 2, file: 0, line: 0, from_rev: 0, until_rev: 2, prior: vec![] },
            TimelineNode { id: 30, kind: 3, qname: 3, file: 0, line: 0, from_rev: 1, until_rev: TIMELINE_OPEN, prior: vec![] },
        ],
        edges: vec![
            TimelineEdge { from: 11, to: 20, category: 7, from_rev: 0, until_rev: 2 },
            TimelineEdge { from: 11, to: 30, category: 7, from_rev: 1, until_rev: TIMELINE_OPEN },
        ],
    }
}

#[cfg(unix)]
fn inode(p: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).unwrap().ino()
}

#[test]
fn round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("layout");
    let t = sample();
    write_timeline(&dir, &t).unwrap();
    assert_eq!(read_timeline(&dir).unwrap(), Some(t.clone()));

    let path = dir.join(TIMELINE_FILE);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..8], b"GLIAGMAP");
    assert_eq!(&bytes[8..12], &FORMAT_VERSION.to_le_bytes());
    assert_eq!(decode_timeline(&bytes).unwrap(), t, "FS-free decode agrees");
    // Unaligned input decodes too: the decoder copies into aligned buffers.
    let mut shifted = vec![0u8];
    shifted.extend_from_slice(&bytes);
    assert_eq!(decode_timeline(&shifted[1..]).unwrap(), t);

    let m = MmapContainer::open(&path).unwrap();
    let names: Vec<String> = m.section_names().unwrap().into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, vec![TIMELINE_SECTION.to_string()]);
    let core = m.archived().unwrap();
    assert_eq!(core.header.graph_type.as_str(), "code");
    assert_eq!(core.repo.0.to_native(), RepoId(0x5eed).0);
    assert_eq!((core.nodes.len(), core.edges.len()), (0, 0));

    let i = inspect_path(&path).unwrap();
    assert_eq!(i.shards.len(), 1);
    let sections: Vec<&str> = i.shards[0].sections.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(sections, vec!["timeline"]);
    assert_eq!(i.shards[0].graph_type, "code");
    assert_eq!(i.shards[0].format, FORMAT_VERSION);

    // Deterministic: the same store writes the same bytes anywhere.
    let other = tmp.path().join("other");
    write_timeline(&other, &t).unwrap();
    assert_eq!(std::fs::read(other.join(TIMELINE_FILE)).unwrap(), bytes);
}

#[test]
fn accessors_decode_the_store_encodings() {
    let t = sample();
    let (f, g, h) = (&t.nodes[0], &t.nodes[1], &t.nodes[2]);
    assert_eq!(t.node_qname(f), Some("app::b::f"));
    assert_eq!(t.node_file(f), Some("app/b.py"), "file is index + 1");
    assert_eq!(f.line(), Some(1), "line is the 0-based row + 1");
    assert_eq!((t.node_file(h), h.line()), (None, None), "0 = none");
    assert_eq!((f.until(), g.until()), (None, Some(2)));
    assert!(g.covers(0) && g.covers(1) && !g.covers(2), "[from_rev, until_rev)");
    assert!(!h.covers(0) && h.covers(1) && h.covers(2) && h.covers(u32::MAX - 1));
    let (fg, fh) = (&t.edges[0], &t.edges[1]);
    assert!(fg.covers(1) && !fg.covers(2) && fg.until() == Some(2));
    assert!(!fh.covers(0) && fh.covers(2) && fh.until().is_none());
    assert_eq!(t.string(99), None);
}

#[cfg(unix)]
#[test]
fn unchanged_not_rewritten() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let path = dir.join(TIMELINE_FILE);
    let t = sample();
    write_timeline(&dir, &t).unwrap();
    let (ino, mtime) = (inode(&path), std::fs::metadata(&path).unwrap().modified().unwrap());
    std::thread::sleep(std::time::Duration::from_millis(20));
    write_timeline(&dir, &t).unwrap();
    assert_eq!(inode(&path), ino, "an identical write keeps the inode");
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime, "and the mtime");

    // A changed timeline is rewritten (tmp + rename: a new inode).
    let mut changed = t.clone();
    changed.edges[1].until_rev = 2;
    write_timeline(&dir, &changed).unwrap();
    assert_ne!(inode(&path), ino);
    assert_eq!(read_timeline(&dir).unwrap(), Some(changed));

    // A damaged file of the same length is rewritten, not trusted.
    write_timeline(&dir, &t).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&path, &bytes).unwrap();
    write_timeline(&dir, &t).unwrap();
    assert_eq!(read_timeline(&dir).unwrap(), Some(t));
}

#[test]
fn missing_is_none() {
    let tmp = tempfile::tempdir().unwrap();
    assert_eq!(read_timeline(tmp.path()).unwrap(), None);
    assert_eq!(read_timeline(&tmp.path().join("no/such/layout")).unwrap(), None);
}

#[test]
fn old_future_and_damaged_files_need_a_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    write_timeline(&dir, &sample()).unwrap();
    let path = dir.join(TIMELINE_FILE);
    let good = std::fs::read(&path).unwrap();

    let with = |edit: &dyn Fn(&mut Vec<u8>)| {
        let mut b = good.clone();
        edit(&mut b);
        std::fs::write(&path, &b).unwrap();
        read_timeline(&dir).unwrap_err()
    };

    let old = with(&|b| b[8..12].copy_from_slice(&2u32.to_le_bytes()));
    assert!(matches!(old, StoreError::OldFormat { found: Some(2) }), "{old:?}");
    assert!(old.needs_rebuild());

    let future = with(&|b| b[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes()));
    assert!(matches!(future, StoreError::FutureFormat { .. }), "{future:?}");
    assert!(future.needs_rebuild());

    let pre_050 = with(&|b| b[..8].copy_from_slice(b"notgmap!"));
    assert!(matches!(pre_050, StoreError::OldFormat { found: None }), "{pre_050:?}");

    let truncated = with(&|b| b.truncate(b.len() - 9));
    assert!(matches!(truncated, StoreError::Corrupt { .. }), "{truncated:?}");
    assert!(truncated.needs_rebuild());

    // Every byte of the section (preamble end .. core_offset) flipped: the
    // section no longer validates - Corrupt, never a panic.
    let flipped = with(&|b| {
        let mut off = [0u8; 8];
        off.copy_from_slice(&b[16..24]);
        let core_offset = u64::from_le_bytes(off) as usize;
        for x in &mut b[32..core_offset] {
            *x ^= 0x5a;
        }
    });
    assert!(matches!(flipped, StoreError::Corrupt { .. }), "{flipped:?}");
    assert!(flipped.needs_rebuild());
}

#[test]
fn a_container_without_the_timeline_section_is_corrupt() {
    // A layout shard is a valid .gmap but not a timeline.
    let tmp = tempfile::tempdir().unwrap();
    let mut core = glia_store::Container {
        header: glia_store::Header::for_code(),
        repo: RepoId(1),
        nodes: vec![],
        edges: vec![],
        node_kinds: vec![],
        sections: vec![],
    };
    let path = tmp.path().join(TIMELINE_FILE);
    glia_store::write_container(&path, &mut core, &[]).unwrap();
    match read_timeline(tmp.path()) {
        Err(StoreError::Corrupt { detail }) => assert!(detail.contains("no 'timeline' section"), "{detail}"),
        other => panic!("expected Corrupt, got {other:?}"),
    }
}

#[test]
fn an_invalid_store_is_refused_and_nothing_is_written() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let cases: Vec<(&str, Box<dyn Fn(&mut TimelineStore)>)> = vec![
        ("qname index", Box::new(|t| t.nodes[0].qname = 4)),
        ("file index", Box::new(|t| t.nodes[0].file = 5)),
        ("from_rev", Box::new(|t| t.edges[0].from_rev = 3)),
        ("until_rev", Box::new(|t| t.edges[0].until_rev = 0)),
        ("until past the window", Box::new(|t| t.nodes[1].until_rev = 3)),
        ("prior rev", Box::new(|t| t.nodes[0].prior = vec![(3, 1)])),
        ("sha", Box::new(|t| t.revs[0].sha = "abc123".into())),
        ("upper-case sha", Box::new(|t| t.revs[0].sha = SHA_0.to_ascii_uppercase())),
        ("two-line subject", Box::new(|t| t.revs[0].subject = "a\nb".into())),
        ("long subject", Box::new(|t| t.revs[0].subject = "x".repeat(TIMELINE_SUBJECT_CAP + 1))),
        ("secret in subject", Box::new(|t| t.revs[0].subject = "rotate api_key=abcdef123456".into())),
    ];
    for (what, edit) in cases {
        let mut t = sample();
        edit(&mut t);
        assert!(t.check().is_err(), "{what}");
        match write_timeline(&dir, &t) {
            Err(StoreError::Invalid(why)) => assert!(why.starts_with("timeline: "), "{what}: {why}"),
            other => panic!("{what}: expected Invalid, got {other:?}"),
        }
        assert!(!dir.join(TIMELINE_FILE).exists(), "{what}: nothing written");
    }
    assert_eq!(sample().check(), Ok(()));
    assert_eq!(TimelineStore::default().check(), Ok(()), "an empty timeline is valid");
}

#[test]
fn subjects_are_one_redacted_capped_line() {
    assert_eq!(timeline_subject("fix: the thing\n\nlong body\nSigned-off-by: someone"), "fix: the thing");
    assert_eq!(timeline_subject("carriage\rreturn"), "carriage");
    assert_eq!(timeline_subject(""), "");
    let secret = timeline_subject("deploy with GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789");
    assert!(!secret.contains("ghp_abcdefghij"), "{secret}");
    assert!(secret.starts_with("deploy with GITHUB_TOKEN="), "{secret}");
    let long = "é".repeat(TIMELINE_SUBJECT_CAP * 2);
    assert_eq!(timeline_subject(&long).chars().count(), TIMELINE_SUBJECT_CAP, "chars, not bytes");
    // A cut through a redaction marker is trimmed back, never re-redacted:
    // the redacted line is `<pad> password=***`, the cap lands after its first
    // `*`, and `password=*` would be redacted again, so the `*` goes.
    let pad = "p".repeat(TIMELINE_SUBJECT_CAP - "password=".len() - 2);
    let cut = timeline_subject(&format!("{pad} password=hunter2hunter2"));
    assert_eq!(cut, format!("{pad} password="), "trimmed back to a clean prefix");
    for s in [
        "fix: the thing",
        "deploy with GITHUB_TOKEN=ghp_abcdefghijklmnopqrstuvwxyz0123456789",
        &format!("{pad} password=hunter2hunter2"),
        &long,
    ] {
        let once = timeline_subject(s);
        assert_eq!(timeline_subject(&once), once, "idempotent: {s:?}");
        let mut t = sample();
        t.revs[0].subject = once;
        assert_eq!(t.check(), Ok(()), "what timeline_subject returns is storable: {s:?}");
    }
}
