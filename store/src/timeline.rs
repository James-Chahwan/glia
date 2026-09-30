//! The timeline sidecar (CD.5b): `<layout>/timeline.gmap`, the history of a
//! repo's graph over a window of commits, persisted beside the layout so an
//! answer reads it without rebuilding N revs.
//!
//! The file is an ordinary `.gmap` container: the `GLIAGMAP` preamble, a
//! domain-free core with a code header (`Header::for_code`), the timeline's
//! repo and no nodes or edges, and ONE named section, [`TIMELINE_SECTION`],
//! holding the rkyv archive of a [`TimelineStore`]. So `FORMAT_VERSION`,
//! `OldFormat` / `FutureFormat` / `Corrupt` (with `needs_rebuild`) and
//! `glia inspect` all apply to it unchanged.
//!
//! What a [`TimelineStore`] holds - the store writes what it is given and
//! imposes no order (the builder, `activation::algo::timeline` via CD.5c, emits
//! spans sorted, so the bytes are a pure function of the input):
//! - `revs`, oldest first: the commit sha, the committer time and the subject
//!   line. No author or committer name or email is ever stored, and a subject
//!   passes through [`timeline_subject`] (first line, the A13.7 secret
//!   redaction, at most [`TIMELINE_SUBJECT_CAP`] chars) before it is stored:
//!   [`write_timeline`] refuses one that has not.
//! - `strings`: the qnames and file paths the node spans index.
//! - `nodes` / `edges`: validity spans `[from_rev, until_rev)` as rev indexes,
//!   `until_rev` [`TIMELINE_OPEN`] for a span still valid at the last rev.
//!   A node records its id as last seen plus `prior` = `(rev it changed at,
//!   id before)` for every move chained through it.
//!
//! The name survives a rebuild of the layout it sits in: persist's orphan
//! sweep removes only `repo-<u64>[-NN].gmap` / `cross_stack.gmap`, the layout
//! readers open only the files `manifest.json` names, and
//! `external_inputs_fingerprint` skips `*.gmap` and the layout dir, so the
//! sidecar neither goes away nor makes the layout stale.
//!
//! Marker, one line per [`write_timeline`]:
//! `[gmap] timeline <dir>/timeline.gmap: revs=<N> nodes=<n> edges=<e> written`
//! (`unchanged` in place of `written` when the file already held these bytes
//! and was left alone).

use std::path::Path;

use glia_code_domain::snapshots::redact_untrusted;
use glia_core::RepoId;
use rkyv::util::AlignedVec;

use crate::container::{
    ArchivedContainer, Container, FORMAT_VERSION, Header, MAGIC, PREAMBLE_LEN, encode_file,
    encode_section, split_file, write_atomic,
};
use crate::error::StoreError;
use crate::layout::on_disk_is;

/// File name of the timeline sidecar inside a layout directory.
pub const TIMELINE_FILE: &str = "timeline.gmap";
/// Name of the one section `timeline.gmap` carries.
pub const TIMELINE_SECTION: &str = "timeline";
/// `until_rev` of a span still valid at the last rev of the window.
pub const TIMELINE_OPEN: u32 = u32::MAX;
/// Most chars a stored [`TimelineRev::subject`] holds.
pub const TIMELINE_SUBJECT_CAP: usize = 120;

/// A repo's graph history over a window of commits: the sidecar's one section.
#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct TimelineStore {
    /// The `RepoId.0` of the repo the revs were built from; also the core's
    /// `repo`.
    pub repo: u64,
    /// The commits of the window, oldest first; a span's rev is an index here.
    pub revs: Vec<TimelineRev>,
    /// The qnames and file paths node spans index.
    pub strings: Vec<String>,
    pub nodes: Vec<TimelineNode>,
    pub edges: Vec<TimelineEdge>,
}

/// One commit of the window.
#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct TimelineRev {
    /// The full commit id: 40 (sha-1) or 64 (sha-256) lowercase hex chars.
    pub sha: String,
    /// Committer time, unix seconds.
    pub time: i64,
    /// The subject line as [`timeline_subject`] stores it.
    pub subject: String,
}

/// One node's validity span.
#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct TimelineNode {
    /// `NodeId.0` as last seen in the window.
    pub id: u64,
    /// `NodeKindId.0`.
    pub kind: u32,
    /// Index of the qname in [`TimelineStore::strings`].
    pub qname: u32,
    /// Index of the file in `strings` PLUS ONE; 0 = no file.
    pub file: u32,
    /// The 0-based POSITION row PLUS ONE (so the 1-based line); 0 = no line.
    pub line: u32,
    /// The rev the span opens at.
    pub from_rev: u32,
    /// The rev the node is gone at, or [`TIMELINE_OPEN`].
    pub until_rev: u32,
    /// `(rev the id changed at, NodeId.0 before)` for every move chained into
    /// this span, oldest first.
    pub prior: Vec<(u32, u64)>,
}

/// One edge's validity span.
#[derive(Debug, Clone, PartialEq, Default)]
#[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct TimelineEdge {
    /// `NodeId.0` of each end as last seen.
    pub from: u64,
    pub to: u64,
    /// `EdgeCategoryId.0`.
    pub category: u32,
    pub from_rev: u32,
    /// The rev the edge is gone at, or [`TIMELINE_OPEN`].
    pub until_rev: u32,
}

impl TimelineStore {
    /// The string at `ix`, `None` out of range.
    pub fn string(&self, ix: u32) -> Option<&str> {
        self.strings.get(ix as usize).map(String::as_str)
    }

    /// The qname of `node`.
    pub fn node_qname(&self, node: &TimelineNode) -> Option<&str> {
        self.string(node.qname)
    }

    /// The file of `node`, `None` when it records none.
    pub fn node_file(&self, node: &TimelineNode) -> Option<&str> {
        node.file.checked_sub(1).and_then(|ix| self.string(ix))
    }

    /// Every structural rule a stored timeline keeps, as the reason the first
    /// broken one gives: at most `u32::MAX - 1` revs (so [`TIMELINE_OPEN`] is
    /// never a rev index); each rev's sha 40 or 64 lowercase hex chars and its
    /// subject one line of at most [`TIMELINE_SUBJECT_CAP`] chars the A13.7
    /// redaction finds nothing in; each string index in range; each span
    /// opening at a rev of the window and closing after it, at a rev of the
    /// window or [`TIMELINE_OPEN`]; each prior id changed at a rev of the
    /// window. [`write_timeline`] refuses a store that breaks one
    /// (`Invalid`), [`decode_timeline`] a file that does (`Corrupt`).
    pub fn check(&self) -> Result<(), String> {
        let revs = u32::try_from(self.revs.len())
            .ok()
            .filter(|n| *n < TIMELINE_OPEN)
            .ok_or_else(|| format!("{} revs do not fit a u32 rev index", self.revs.len()))?;
        for (i, r) in self.revs.iter().enumerate() {
            if !is_oid(&r.sha) {
                return Err(format!("rev {i}: sha {:?} is not 40 or 64 lowercase hex chars", r.sha));
            }
            if r.subject.contains(['\n', '\r']) {
                return Err(format!("rev {i}: subject is more than one line"));
            }
            if r.subject.chars().count() > TIMELINE_SUBJECT_CAP {
                return Err(format!("rev {i}: subject is over {TIMELINE_SUBJECT_CAP} chars"));
            }
            if redact_untrusted(&r.subject).1 > 0 {
                return Err(format!("rev {i}: subject holds a secret (store timeline_subject(..))"));
            }
        }
        let span = |what: &str, from: u32, until: u32| -> Result<(), String> {
            if from >= revs {
                return Err(format!("{what}: from_rev {from} is outside the {revs} revs"));
            }
            if until != TIMELINE_OPEN && (until <= from || until >= revs) {
                return Err(format!("{what}: until_rev {until} does not close [{from}, ..) in {revs} revs"));
            }
            Ok(())
        };
        let strings = self.strings.len();
        for (i, n) in self.nodes.iter().enumerate() {
            let what = format!("node {i} ({})", n.id);
            if n.qname as usize >= strings {
                return Err(format!("{what}: qname index {} is outside the {strings} strings", n.qname));
            }
            if n.file as usize > strings {
                return Err(format!("{what}: file index {} is outside the {strings} strings", n.file - 1));
            }
            span(&what, n.from_rev, n.until_rev)?;
            if let Some((rev, _)) = n.prior.iter().find(|(rev, _)| *rev >= revs) {
                return Err(format!("{what}: prior id changed at rev {rev}, outside the {revs} revs"));
            }
        }
        for (i, e) in self.edges.iter().enumerate() {
            span(&format!("edge {i} ({} -> {})", e.from, e.to), e.from_rev, e.until_rev)?;
        }
        Ok(())
    }
}

impl TimelineNode {
    /// True when the node is valid at rev `rev`.
    pub fn covers(&self, rev: u32) -> bool {
        self.from_rev <= rev && rev < self.until_rev
    }

    /// The rev the node is gone at; `None` while it is still valid at the
    /// last rev.
    pub fn until(&self) -> Option<u32> {
        (self.until_rev != TIMELINE_OPEN).then_some(self.until_rev)
    }

    /// The 1-based line, `None` when the node records none.
    pub fn line(&self) -> Option<u32> {
        (self.line != 0).then_some(self.line)
    }
}

impl TimelineEdge {
    /// True when the edge is valid at rev `rev`.
    pub fn covers(&self, rev: u32) -> bool {
        self.from_rev <= rev && rev < self.until_rev
    }

    /// The rev the edge is gone at; `None` while it is still valid at the
    /// last rev.
    pub fn until(&self) -> Option<u32> {
        (self.until_rev != TIMELINE_OPEN).then_some(self.until_rev)
    }
}

/// A commit message as a [`TimelineRev::subject`] stores it: its first line
/// (up to the first `\n` or `\r`), the A13.7 redaction applied
/// (`redact_untrusted`: tokens, keyed secrets and URL passwords go), then cut
/// to [`TIMELINE_SUBJECT_CAP`] chars. A cut that leaves something the
/// redaction would take again (the middle of a `***` marker) is trimmed back
/// until it does not. Idempotent: its output passes through it unchanged.
pub fn timeline_subject(message: &str) -> String {
    let first = message.split(['\n', '\r']).next().unwrap_or("");
    let (redacted, _) = redact_untrusted(first);
    let mut out: String = redacted.chars().take(TIMELINE_SUBJECT_CAP).collect();
    while !out.is_empty() && redact_untrusted(&out).1 > 0 {
        out.pop();
    }
    out
}

/// 40 (sha-1) or 64 (sha-256) lowercase hex chars.
fn is_oid(s: &str) -> bool {
    matches!(s.len(), 40 | 64) && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The bytes of `timeline.gmap` for `t`: a code-header core with `t.repo` and
/// no nodes or edges, and `t` archived as the [`TIMELINE_SECTION`].
fn encode_timeline(t: &TimelineStore) -> Result<Vec<u8>, StoreError> {
    t.check().map_err(|why| StoreError::Invalid(format!("timeline: {why}")))?;
    let mut core = Container {
        header: Header::for_code(),
        repo: RepoId(t.repo),
        nodes: Vec::new(),
        edges: Vec::new(),
        node_kinds: Vec::new(),
        sections: Vec::new(),
    };
    let section = encode_section(TIMELINE_SECTION, t)?;
    encode_file(&mut core, &[section])
}

/// Write `t` as `<dir>/timeline.gmap` (`dir` is created if missing): atomic
/// (`.tmp` + rename) like every shard, and skipped when the file already
/// holds exactly these bytes, so an unchanged timeline keeps its inode and
/// mtime. A store that breaks [`TimelineStore::check`] is `Invalid` and
/// nothing is written. Prints the `[gmap] timeline` marker.
pub fn write_timeline(dir: &Path, t: &TimelineStore) -> Result<(), StoreError> {
    let bytes = encode_timeline(t)?;
    std::fs::create_dir_all(dir)?;
    let path = dir.join(TIMELINE_FILE);
    let unchanged = on_disk_is(&path, &bytes);
    if !unchanged {
        write_atomic(&path, &bytes)?;
    }
    eprintln!(
        "[gmap] timeline {}: revs={} nodes={} edges={} {}",
        path.display(),
        t.revs.len(),
        t.nodes.len(),
        t.edges.len(),
        if unchanged { "unchanged" } else { "written" },
    );
    Ok(())
}

/// The timeline of the layout at `dir`: `Ok(None)` when it has no
/// `timeline.gmap`, else the file decoded by [`decode_timeline`] (so an old,
/// future or damaged file is the `StoreError` whose `needs_rebuild` is true).
pub fn read_timeline(dir: &Path) -> Result<Option<TimelineStore>, StoreError> {
    match std::fs::read(dir.join(TIMELINE_FILE)) {
        Ok(bytes) => decode_timeline(&bytes).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Decode the bytes of a `timeline.gmap` file, touching no file system: the
/// preamble (`OldFormat` / `FutureFormat` before rkyv sees a byte), the core
/// (header magic and version), the one [`TIMELINE_SECTION`] (between the
/// preamble and the core, 16-aligned, named once) and its [`TimelineStore`],
/// which must match the core's repo and pass [`TimelineStore::check`]. Any
/// other shape is `Corrupt`. `bytes` need not be aligned: the core and the
/// section are copied into 16-aligned buffers before rkyv reads them.
pub fn decode_timeline(bytes: &[u8]) -> Result<TimelineStore, StoreError> {
    let corrupt = |detail: String| StoreError::Corrupt { detail: format!("{TIMELINE_FILE}: {detail}") };
    let core_range = split_file(bytes)?;
    let mut core_bytes = AlignedVec::<16>::new();
    core_bytes.extend_from_slice(&bytes[core_range.clone()]);
    let core = rkyv::access::<ArchivedContainer, rkyv::rancor::Error>(&core_bytes)
        .map_err(|_| corrupt("the core archive does not validate".to_string()))?;
    if core.header.magic != MAGIC {
        return Err(StoreError::BadMagic { expected: MAGIC, got: core.header.magic });
    }
    let version = core.header.version.to_native();
    if version != FORMAT_VERSION {
        return Err(StoreError::UnsupportedVersion(version, FORMAT_VERSION));
    }
    let mut named = core.sections.iter().filter(|s| s.name.as_str() == TIMELINE_SECTION);
    let entry = named.next().ok_or_else(|| corrupt(format!("no '{TIMELINE_SECTION}' section")))?;
    if named.next().is_some() {
        return Err(corrupt(format!("the '{TIMELINE_SECTION}' section is named twice")));
    }
    let (offset, len) = (entry.offset.to_native(), entry.len.to_native());
    let range = usize::try_from(offset)
        .ok()
        .zip(usize::try_from(len).ok())
        .and_then(|(start, len)| Some(start..start.checked_add(len)?))
        .filter(|r| r.start >= PREAMBLE_LEN && r.end <= core_range.start && r.start % 16 == 0)
        .ok_or_else(|| {
            corrupt(format!(
                "section '{TIMELINE_SECTION}' at {offset}+{len} is not an aligned range between \
                 the preamble and the core (at {})",
                core_range.start
            ))
        })?;
    let mut section = AlignedVec::<16>::new();
    section.extend_from_slice(&bytes[range]);
    let store = rkyv::from_bytes::<TimelineStore, rkyv::rancor::Error>(&section)
        .map_err(|_| corrupt(format!("section '{TIMELINE_SECTION}' does not validate")))?;
    let repo = core.repo.0.to_native();
    if store.repo != repo {
        return Err(corrupt(format!("the section's repo {} is not the core's {repo}", store.repo)));
    }
    store.check().map_err(corrupt)?;
    Ok(store)
}
