//! `glia cache push|pull --layout` (CE.2d): the whole-layout object, packed
//! from and unpacked into the engine's layout export / verified install
//! (`glia_engine::shared_cache::{export_layout, install_layout}`) and moved
//! through CE.2c's [`ObjectStore`] under CE.2c's [`Signing`].
//!
//! The layout is one BODY, split into PARTS that each travel as an ordinary
//! CE.2c object (`object::encode`: key, length, MAC), so a layout larger than
//! the store's per-object read bound (`object::MAX_OBJECT_BYTES`, 64 MiB;
//! glia's own layout is 22 MB, a larger repo's passes the bound) still moves,
//! and every part is authenticated on its own:
//!
//! ```text
//! body   = b"GLIALY01" | entry_count u32 LE | per entry: name_len u16 LE | name | data_len u64 LE | data
//! part i = part_count u32 LE | body_len u64 LE | body[i * PART_BYTES ..][.. PART_BYTES]
//!          as the CE.2c object of key part_key(layout key, i)
//!          = blake3 derive_key("glia layout object part v1", key | i u32 LE)
//! store  = v1/<stamp>/layout/<key>.gla (part 0), v1/<stamp>/layout/<key>.<i>.gla (i >= 1)
//! ```
//!
//! A part's key binds it to its layout and its index, so a store cannot serve
//! one layout's part for another's, reorder parts or answer a key with another
//! key's layout (the object's key and MAC refuse it); every part repeats the
//! part count and body length, so a dropped or truncated part is caught. A
//! push writes part 0 last, so a part 0 in the store means every part was
//! written. [`fetch`] trusts nothing it can check: each part is bounded by the
//! store read and `object::decode`, the body is grown part by part (never
//! allocated from a stated length) up to [`MAX_BODY`], and every count, name
//! length and data length is checked against the engine's caps
//! (`LAYOUT_MAX_ENTRIES`, `LAYOUT_MAX_NAME`, `LAYOUT_MAX_ENTRY_BYTES`,
//! `LAYOUT_MAX_BYTES`) before it is sliced by. The engine then checks the
//! names again, re-derives the key from the checkout and loads the result
//! before it replaces anything.
//!
//! `gc` counts only `.gpc` objects: a stamp's layout parts go with the stamp,
//! and are outside `--max-bytes`.
//!
//! fired_on marker, once per layout step:
//! `[cache] layout <push|pull> repo=<label> key=<12 hex|-> tree=<12 hex|-> result=<pushed|present|stale|dirty|hit|miss|rejected>`

use glia_engine::BUILD_STAMP;
use glia_engine::shared_cache::{
    CacheKey, LAYOUT_MAX_BYTES, LAYOUT_MAX_ENTRIES, LAYOUT_MAX_ENTRY_BYTES, LAYOUT_MAX_NAME,
    LayoutExport, LayoutInstall, LayoutKey, export_layout, install_layout, layout_key,
};
use serde_json::{Value, json};

use super::object::{Signing, decode, encode};
use super::store::ObjectStore;

/// The body's magic and version.
const BODY_MAGIC: &[u8; 8] = b"GLIALY01";
/// blake3 `derive_key` context of a part's object key.
const PART_CONTEXT: &str = "glia layout object part v1";
/// A part's own header: part count and body length.
const PART_HEADER: usize = 4 + 8;
/// Most body bytes one part carries. With its part header and the object's
/// header and MAC it stays well under `object::MAX_OBJECT_BYTES`. The unit
/// tests split at 4 KiB so a multi-part layout needs no 48 MiB file.
const PART_BYTES: usize = if cfg!(test) { 4096 } else { 48 << 20 };
/// The largest body the engine's caps allow: magic, count, the framing of
/// every entry at the longest name, and the data at the total cap.
const MAX_BODY: u64 =
    8 + 4 + (LAYOUT_MAX_ENTRIES as u64) * (2 + LAYOUT_MAX_NAME as u64 + 8) + LAYOUT_MAX_BYTES;

/// The store path of part `part` of `key`'s layout under build stamp `stamp`.
pub(crate) fn layout_rel(stamp: &str, key: &CacheKey, part: u32) -> String {
    let hex = key.to_hex();
    if part == 0 {
        format!("v1/{stamp}/layout/{hex}.gla")
    } else {
        format!("v1/{stamp}/layout/{hex}.{part}.gla")
    }
}

/// The object key part `part` of `key`'s layout travels under.
fn part_key(key: &CacheKey, part: u32) -> CacheKey {
    let mut h = blake3::Hasher::new_derive_key(PART_CONTEXT);
    h.update(key.as_bytes());
    h.update(&part.to_le_bytes());
    CacheKey::from_bytes(*h.finalize().as_bytes())
}

/// The body of `entries` (names and sizes are within the engine's caps: it
/// exported them).
fn pack(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let size: usize = entries.iter().map(|(n, d)| 2 + n.len() + 8 + d.len()).sum();
    let mut out = Vec::with_capacity(BODY_MAGIC.len() + 4 + size);
    out.extend_from_slice(BODY_MAGIC);
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    for (name, data) in entries {
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

/// How many parts a body of `len` bytes travels in.
fn part_count(len: u64) -> u64 {
    len.div_ceil(PART_BYTES as u64).max(1)
}

/// The body length part `part` carries, of a body of `len` bytes.
fn chunk_len(len: u64, part: u64) -> u64 {
    len.saturating_sub(part * PART_BYTES as u64)
        .min(PART_BYTES as u64)
}

/// A reader over bytes that refuses to read past their end.
struct Cursor<'a>(&'a [u8]);

impl<'a> Cursor<'a> {
    fn take(&mut self, n: u64) -> Result<&'a [u8], &'static str> {
        let n = usize::try_from(n).map_err(|_| "a length larger than memory")?;
        if n > self.0.len() {
            return Err("a length past the end of the body");
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }

    fn le<const N: usize>(&mut self) -> Result<[u8; N], &'static str> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.take(N as u64)?);
        Ok(out)
    }
}

/// The entries of a body, every length checked against the caps and the
/// bytes present before it is used.
fn unpack(body: &[u8]) -> Result<Vec<(String, Vec<u8>)>, &'static str> {
    let mut r = Cursor(body);
    if r.take(BODY_MAGIC.len() as u64)? != BODY_MAGIC {
        return Err("bad magic");
    }
    let count = u32::from_le_bytes(r.le()?) as usize;
    if count > LAYOUT_MAX_ENTRIES {
        return Err("more entries than the cap");
    }
    let mut entries = Vec::with_capacity(count);
    let mut total = 0u64;
    for _ in 0..count {
        let name_len = u16::from_le_bytes(r.le()?) as usize;
        if name_len == 0 || name_len > LAYOUT_MAX_NAME {
            return Err("a name length outside 1..=128");
        }
        let name = std::str::from_utf8(r.take(name_len as u64)?)
            .map_err(|_| "a name that is not UTF-8")?;
        let data_len = u64::from_le_bytes(r.le()?);
        total = total.saturating_add(data_len);
        if data_len > LAYOUT_MAX_ENTRY_BYTES || total > LAYOUT_MAX_BYTES {
            return Err("an entry over the size caps");
        }
        entries.push((name.to_string(), r.take(data_len)?.to_vec()));
    }
    if !r.0.is_empty() {
        return Err("trailing bytes after the last entry");
    }
    Ok(entries)
}

/// Part `part` of `key`'s layout out of its object: `(part count, body
/// length, chunk)`, or why it is refused.
fn open_part(
    bytes: &[u8],
    key: &CacheKey,
    part: u32,
    signing: &Signing,
) -> Result<(u64, u64, Vec<u8>), String> {
    let mut payload = decode(bytes, &part_key(key, part), signing).map_err(|r| r.to_string())?;
    if payload.len() < PART_HEADER {
        return Err("not a layout part (shorter than its header)".to_string());
    }
    let mut count = [0u8; 4];
    count.copy_from_slice(&payload[..4]);
    let mut len = [0u8; 8];
    len.copy_from_slice(&payload[4..PART_HEADER]);
    payload.drain(..PART_HEADER);
    Ok((
        u64::from(u32::from_le_bytes(count)),
        u64::from_le_bytes(len),
        payload,
    ))
}

/// What fetching a layout came to.
enum Fetched {
    Missing,
    Rejected(String),
    Entries(Vec<(String, Vec<u8>)>),
}

/// Fetch, check and unpack `key`'s layout. `Err` only for a store failure.
fn fetch(store: &dyn ObjectStore, key: &CacheKey, signing: &Signing) -> Result<Fetched, String> {
    let head = layout_rel(BUILD_STAMP, key, 0);
    let Some(bytes) = store.get(&head)? else {
        return Ok(Fetched::Missing);
    };
    let reject =
        |rel: &str, why: &dyn std::fmt::Display| Ok(Fetched::Rejected(format!("{rel}: {why}")));
    let (count, len, mut body) = match open_part(&bytes, key, 0, signing) {
        Ok(p) => p,
        Err(why) => return reject(&head, &why),
    };
    drop(bytes);
    if len > MAX_BODY || count != part_count(len) || body.len() as u64 != chunk_len(len, 0) {
        return reject(&head, &"a part count or body length outside the caps");
    }
    for part in 1..count {
        let rel = layout_rel(BUILD_STAMP, key, part as u32);
        let Some(bytes) = store.get(&rel)? else {
            return reject(&rel, &"part missing");
        };
        let (c, l, chunk) = match open_part(&bytes, key, part as u32, signing) {
            Ok(p) => p,
            Err(why) => return reject(&rel, &why),
        };
        if c != count || l != len || chunk.len() as u64 != chunk_len(len, part) {
            return reject(&rel, &"a part that disagrees with the layout's length");
        }
        body.extend_from_slice(&chunk);
    }
    match unpack(&body) {
        Ok(entries) => Ok(Fetched::Entries(entries)),
        Err(why) => reject(&head, &format!("not a layout body ({why})")),
    }
}

/// What one layout step did: its result word, the key and tree it was about
/// (none for a dirty checkout), why when it did not move a layout, and the
/// size of what it moved.
pub(crate) struct Outcome {
    verb: &'static str,
    result: &'static str,
    key: Option<CacheKey>,
    tree: Option<String>,
    reason: Option<String>,
    files: usize,
    bytes: u64,
    parts: u64,
}

impl Outcome {
    fn new(
        verb: &'static str,
        result: &'static str,
        key: Option<CacheKey>,
        tree: Option<String>,
    ) -> Self {
        Outcome {
            verb,
            result,
            key,
            tree,
            reason: None,
            files: 0,
            bytes: 0,
            parts: 0,
        }
    }

    fn because(mut self, reason: String) -> Self {
        self.reason = Some(reason);
        self
    }

    fn sized(mut self, entries: &[(String, Vec<u8>)]) -> Self {
        self.files = entries.len();
        self.bytes = entries.iter().map(|(_, d)| d.len() as u64).sum();
        self
    }

    /// The fired_on marker (module doc).
    fn marker(&self, repo_label: &str) -> String {
        let short =
            |s: Option<String>| s.map_or_else(|| "-".to_string(), |s| s.chars().take(12).collect());
        format!(
            "[cache] layout {} repo={repo_label} key={} tree={} result={}",
            self.verb,
            short(self.key.map(|k| k.to_hex())),
            short(self.tree.clone()),
            self.result
        )
    }

    /// The `layout` field of the push / pull `--json` summary.
    pub(crate) fn json(&self) -> Value {
        json!({
            "result": self.result,
            "key": self.key,
            "tree": self.tree,
            "reason": self.reason,
            "files": self.files,
            "bytes": self.bytes,
            "parts": self.parts,
        })
    }

    /// The human summary line.
    pub(crate) fn line(&self, repo_label: &str, store_label: &str) -> String {
        let arrow = if self.verb == "push" { "->" } else { "<-" };
        let mut line = format!("layout {repo_label} {arrow} {store_label}: {}", self.result);
        if self.files > 0 {
            line.push_str(&format!(" ({} files, {} bytes)", self.files, self.bytes));
        }
        if let Some(reason) = &self.reason {
            line.push_str(&format!(" - {reason}"));
        }
        line
    }
}

/// `push --layout`: export the checkout's layout and upload it unless the
/// store holds it. `Err` for an export or store failure.
pub(crate) fn push(
    repo: &str,
    store: &dyn ObjectStore,
    signing: &Signing,
) -> Result<Outcome, String> {
    let out = match export_layout(repo)? {
        LayoutExport::Dirty(reason) => Outcome::new("push", "dirty", None, None).because(reason),
        LayoutExport::Stale { key, tree, reason } => {
            Outcome::new("push", "stale", Some(key), Some(tree)).because(reason)
        }
        LayoutExport::Ready { key, tree, entries } => {
            if store.has(&layout_rel(BUILD_STAMP, &key, 0))? {
                Outcome::new("push", "present", Some(key), Some(tree)).sized(&entries)
            } else {
                let body = pack(&entries);
                let len = body.len() as u64;
                let count = part_count(len);
                // Part 0 last: its presence means every part is there.
                for part in (0..count).rev() {
                    let start = (part * PART_BYTES as u64) as usize;
                    let chunk = &body[start..start + chunk_len(len, part) as usize];
                    let mut payload = Vec::with_capacity(PART_HEADER + chunk.len());
                    payload.extend_from_slice(&(count as u32).to_le_bytes());
                    payload.extend_from_slice(&len.to_le_bytes());
                    payload.extend_from_slice(chunk);
                    let object = encode(&part_key(&key, part as u32), &payload, signing);
                    store.put(&layout_rel(BUILD_STAMP, &key, part as u32), &object)?;
                }
                let mut out = Outcome::new("push", "pushed", Some(key), Some(tree)).sized(&entries);
                out.parts = count;
                out
            }
        }
        _ => return Err("an export outcome this glia does not know".to_string()),
    };
    eprintln!("{}", out.marker(&glia_engine::arch::repo_label_for(repo)));
    Ok(out)
}

/// `pull --layout`: fetch the layout of the checkout's key and hand it to the
/// engine's verified install. `Err` for a store failure, or an install that
/// could not put the previous layout back.
pub(crate) fn pull(
    repo: &str,
    store: &dyn ObjectStore,
    signing: &Signing,
) -> Result<Outcome, String> {
    let out = match layout_key(repo)? {
        LayoutKey::Dirty(reason) => Outcome::new("pull", "dirty", None, None).because(reason),
        LayoutKey::Clean { key, tree } => {
            let at = |result| Outcome::new("pull", result, Some(key), Some(tree.clone()));
            match fetch(store, &key, signing)? {
                Fetched::Missing => at("miss"),
                Fetched::Rejected(why) => {
                    eprintln!("[cache] rejected {why}");
                    at("rejected").because(why)
                }
                Fetched::Entries(entries) => {
                    let (files, bytes) = (
                        entries.len(),
                        entries.iter().map(|(_, d)| d.len() as u64).sum(),
                    );
                    let parts = part_count(pack_len(&entries));
                    let sized = |result| {
                        let mut out = at(result);
                        (out.files, out.bytes, out.parts) = (files, bytes, parts);
                        out
                    };
                    match install_layout(repo, &key, entries)? {
                        LayoutInstall::Installed { .. } => sized("hit"),
                        LayoutInstall::Rejected(reason) => {
                            eprintln!(
                                "[cache] rejected {}: {reason}",
                                layout_rel(BUILD_STAMP, &key, 0)
                            );
                            sized("rejected").because(reason)
                        }
                        LayoutInstall::Dirty(reason) => at("dirty").because(reason),
                        _ => return Err("an install outcome this glia does not know".to_string()),
                    }
                }
            }
        }
        _ => return Err("a layout key this glia does not know".to_string()),
    };
    eprintln!("{}", out.marker(&glia_engine::arch::repo_label_for(repo)));
    Ok(out)
}

/// The body length of `entries` ([`pack`]'s output length).
fn pack_len(entries: &[(String, Vec<u8>)]) -> u64 {
    (BODY_MAGIC.len() + 4) as u64
        + entries
            .iter()
            .map(|(n, d)| (2 + n.len() + 8 + d.len()) as u64)
            .sum::<u64>()
}

#[cfg(test)]
mod tests {
    use super::super::store::DirStore;
    use super::*;

    fn key(b: u8) -> CacheKey {
        CacheKey::from_bytes([b; 32])
    }

    fn entries() -> Vec<(String, Vec<u8>)> {
        vec![
            ("manifest.json".to_string(), b"{}".to_vec()),
            ("repo-1-00.gmap".to_string(), vec![7u8; 1000]),
        ]
    }

    #[test]
    fn bodies_round_trip_and_refuse_lies() {
        let body = pack(&entries());
        assert_eq!(body.len() as u64, pack_len(&entries()));
        assert_eq!(unpack(&body), Ok(entries()));
        assert_eq!(
            unpack(&body[..body.len() - 1]),
            Err("a length past the end of the body")
        );
        let mut trailing = body.clone();
        trailing.push(0);
        assert_eq!(
            unpack(&trailing),
            Err("trailing bytes after the last entry")
        );
        let mut many = body.clone();
        many[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(unpack(&many), Err("more entries than the cap"));
        // The second entry claims u64::MAX bytes: refused before any slice.
        let mut huge = body.clone();
        let at = 12 + 2 + "manifest.json".len() + 8 + 2 + 2 + "repo-1-00.gmap".len();
        huge[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(unpack(&huge), Err("an entry over the size caps"));
        let mut magic = body;
        magic[0] = b'X';
        assert_eq!(unpack(&magic), Err("bad magic"));
    }

    #[test]
    fn parts_split_bodies_at_the_part_size() {
        assert_eq!(part_count(1), 1);
        assert_eq!(part_count(PART_BYTES as u64), 1);
        assert_eq!(part_count(PART_BYTES as u64 + 1), 2);
        assert_eq!(chunk_len(PART_BYTES as u64 + 5, 0), PART_BYTES as u64);
        assert_eq!(chunk_len(PART_BYTES as u64 + 5, 1), 5);
        assert!(MAX_BODY / PART_BYTES as u64 + 1 < u64::from(u32::MAX));
        assert_ne!(part_key(&key(1), 0), part_key(&key(1), 1));
        assert_ne!(part_key(&key(1), 0), key(1));
        assert_eq!(
            layout_rel("0.5.1+p0123", &key(0xab), 2),
            format!("v1/0.5.1+p0123/layout/{}.2.gla", "ab".repeat(32))
        );
    }

    /// A multi-part layout round-trips through a directory store; a part
    /// served at another index, a missing part and a foreign key are refused.
    #[test]
    fn multi_part_layouts_travel_and_are_bound_to_their_key() {
        let dir = std::env::temp_dir().join(format!("glia-ce2d-parts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = DirStore::new(&dir);
        let k = Signing::from_hex(&"42".repeat(32)).expect("key");
        let big = vec![
            ("manifest.json".to_string(), b"{}".to_vec()),
            ("repo-1-00.gmap".to_string(), vec![3u8; PART_BYTES + 100]),
        ];
        let body = pack(&big);
        let len = body.len() as u64;
        assert_eq!(part_count(len), 2);
        for part in (0..2u64).rev() {
            let start = (part * PART_BYTES as u64) as usize;
            let chunk = &body[start..start + chunk_len(len, part) as usize];
            let mut payload = 2u32.to_le_bytes().to_vec();
            payload.extend_from_slice(&len.to_le_bytes());
            payload.extend_from_slice(chunk);
            let rel = layout_rel(BUILD_STAMP, &key(9), part as u32);
            store
                .put(&rel, &encode(&part_key(&key(9), part as u32), &payload, &k))
                .expect("put");
        }
        match fetch(&store, &key(9), &k).expect("fetch") {
            Fetched::Entries(e) => assert_eq!(e, big),
            _ => panic!("the layout did not round-trip"),
        }
        assert!(matches!(
            fetch(&store, &key(8), &k).expect("fetch"),
            Fetched::Missing
        ));
        // Part 1 copied over part 0: its key names index 1, so it is refused.
        let p0 = layout_rel(BUILD_STAMP, &key(9), 0);
        let p1 = layout_rel(BUILD_STAMP, &key(9), 1);
        let part1 = std::fs::read(dir.join(&p1)).expect("part 1");
        let part0 = std::fs::read(dir.join(&p0)).expect("part 0");
        std::fs::write(dir.join(&p0), &part1).expect("swap");
        assert!(matches!(
            fetch(&store, &key(9), &k).expect("fetch"),
            Fetched::Rejected(_)
        ));
        std::fs::write(dir.join(&p0), &part0).expect("restore");
        std::fs::remove_file(dir.join(&p1)).expect("drop part 1");
        match fetch(&store, &key(9), &k).expect("fetch") {
            Fetched::Rejected(why) => assert!(why.contains("part missing"), "{why}"),
            _ => panic!("a layout with a missing part was accepted"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
