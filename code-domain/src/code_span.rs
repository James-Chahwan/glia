//! CODE cells stored as spans into the source (CD.7c, format 3).
//!
//! A CODE cell holds an entity's source text, and almost every one is a
//! verbatim slice of the file its POSITION cell names, starting on the
//! POSITION start line. The store writes such a cell as a [`CodeSpan`] - the
//! repo-relative file, the byte range and the xxh64 of the slice - instead of
//! a second copy of the text, and a reader with the repo at hand reads the
//! slice back out of the file. This module is the span and its two forms:
//!
//! - **On disk** ([`CodeSpan::encode`] / [`CodeSpan::decode`]): a `Bytes`
//!   payload `[0x02, varint(file_ix), varint(start), varint(len), xxh64 as 8 LE
//!   bytes]` (unsigned LEB128 varints). `file_ix` indexes the file's `"strings"`
//!   section, the table interned EVIDENCE (tag `0x01`) already uses, so the
//!   store owns the table and hands the codec an index / a lookup.
//! - **In memory, when the text cannot be read back** ([`CodeSpan::to_json`] /
//!   [`CodeSpan::from_payload`]): a `Json` payload
//!   `{"code_span":{"file":"src/a.rs","start":120,"end":480,"xxh64":"<16 hex>"}}`
//!   that a consumer can fetch itself: read `file` under the repo root and
//!   [`CodeSpan::slice`] it. The store hands this out when the source moved,
//!   changed or is absent, and from a decode given no source at all.
//!
//! Offsets are BYTE offsets, `end` exclusive; a slice is text only when its
//! bytes hash to `xxh64` and are valid UTF-8 ([`CodeSpan::slice`]). The hash is
//! xxhash64 seed 0, the store's shard and fingerprint hash.

use glia_core::CellPayload;

/// Tag byte that opens a CODE span payload on disk. `0x01` is interned
/// EVIDENCE (store `code_section.rs`); a CODE cell never carries `Bytes`
/// otherwise, so a CODE `Bytes` payload opening with this tag is a span.
pub const CODE_SPAN_TAG: u8 = 0x02;

/// A CODE cell's text as a range of its source file: `file` is repo-relative
/// with `/` separators (the POSITION `file`), `[start, end)` a BYTE range of
/// it and `xxh64` the xxhash64 (seed 0) of those bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CodeSpan {
    pub file: String,
    pub start: u64,
    pub end: u64,
    pub xxh64: u64,
}

impl CodeSpan {
    /// The span of `text` found at byte `start` of `file`, hashed.
    pub fn of(file: &str, start: u64, text: &[u8]) -> Self {
        let end = start.saturating_add(text.len() as u64);
        Self { file: file.to_string(), start, end, xxh64: xxh64(text) }
    }

    /// Length of the range in bytes (0 for a malformed `end < start`).
    pub fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    /// True for an empty range.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The text of this span in `source` (the whole file's bytes): the byte
    /// range, when it lies inside the file, hashes to `xxh64` and is UTF-8.
    /// `None` otherwise: the file changed since the span was taken.
    pub fn slice<'a>(&self, source: &'a [u8]) -> Option<&'a str> {
        if self.end < self.start {
            return None;
        }
        let start = usize::try_from(self.start).ok()?;
        let end = usize::try_from(self.end).ok()?;
        let bytes = source.get(start..end)?;
        if xxh64(bytes) != self.xxh64 {
            return None;
        }
        std::str::from_utf8(bytes).ok()
    }

    /// The JSON a consumer receives in place of the text:
    /// `{"code_span":{"file":..,"start":..,"end":..,"xxh64":"<16 hex>"}}`,
    /// fields in that order, `file` JSON-escaped.
    pub fn to_json(&self) -> String {
        let file = serde_json::Value::String(self.file.clone()).to_string();
        format!(
            r#"{{"code_span":{{"file":{file},"start":{},"end":{},"xxh64":"{:016x}"}}}}"#,
            self.start, self.end, self.xxh64
        )
    }

    /// [`CodeSpan::to_json`] as a `Json` cell payload.
    pub fn to_payload(&self) -> CellPayload {
        CellPayload::Json(self.to_json())
    }

    /// The span a `Json` payload holds, when it is a [`CodeSpan::to_json`]
    /// object: a `code_span` object with a string `file`, integer `start` /
    /// `end` (`start <= end`) and a 16-hex-digit `xxh64`. `None` for any other
    /// payload (text, other JSON such as the queue / cron `sites`, bytes).
    pub fn from_payload(payload: &CellPayload) -> Option<Self> {
        let CellPayload::Json(s) = payload else {
            return None;
        };
        if !s.starts_with(r#"{"code_span":"#) {
            return None;
        }
        let v: serde_json::Value = serde_json::from_str(s).ok()?;
        let top = v.as_object()?;
        if top.len() != 1 {
            return None;
        }
        let span = top.get("code_span")?.as_object()?;
        let file = span.get("file")?.as_str()?.to_string();
        let start = span.get("start")?.as_u64()?;
        let end = span.get("end")?.as_u64()?;
        let hex = span.get("xxh64")?.as_str()?;
        if hex.len() != 16 || end < start {
            return None;
        }
        let xxh64 = u64::from_str_radix(hex, 16).ok()?;
        Some(Self { file, start, end, xxh64 })
    }

    /// The on-disk payload, `file` given as its index in the file's string
    /// table: `[CODE_SPAN_TAG, varint(file_ix), varint(start), varint(len),
    /// xxh64 LE]`.
    pub fn encode(&self, file_ix: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(20);
        out.push(CODE_SPAN_TAG);
        put_varint(&mut out, file_ix);
        put_varint(&mut out, self.start);
        put_varint(&mut out, self.len());
        out.extend_from_slice(&self.xxh64.to_le_bytes());
        out
    }

    /// The inverse of [`CodeSpan::encode`]; `file(i)` is string-table entry
    /// `i` of a `table_len`-entry table. Any malformed payload (another tag,
    /// truncated, a file index outside the table, a range past `u64`,
    /// trailing bytes) is a reason string.
    pub fn decode<'s>(
        bytes: &[u8],
        table_len: usize,
        file: impl Fn(usize) -> Option<&'s str>,
    ) -> Result<Self, String> {
        if bytes.first() != Some(&CODE_SPAN_TAG) {
            return Err(format!("not a CODE span payload (tag {:?})", bytes.first()));
        }
        let mut at = 1usize;
        let ix = take_varint(bytes, &mut at, "file")?;
        let file = usize::try_from(ix)
            .ok()
            .and_then(file)
            .ok_or_else(|| format!("file index {ix} outside the {table_len}-entry strings table"))?
            .to_string();
        let start = take_varint(bytes, &mut at, "start")?;
        let len = take_varint(bytes, &mut at, "len")?;
        let end = start.checked_add(len).ok_or_else(|| format!("range {start}+{len} past u64"))?;
        let Some(hash) = bytes.get(at..at + 8) else {
            return Err(format!("truncated payload: no 8-byte xxh64 at byte {at}"));
        };
        let mut le = [0u8; 8];
        le.copy_from_slice(hash);
        if at + 8 != bytes.len() {
            return Err(format!("{} trailing byte(s)", bytes.len() - at - 8));
        }
        Ok(Self { file, start, end, xxh64: u64::from_le_bytes(le) })
    }
}

/// Is `payload` a code span handed out in place of CODE text
/// ([`CodeSpan::from_payload`])? A reader that wants the code itself (a body
/// hash, a preview) skips such a payload: it is a reference, not code.
pub fn is_span_payload(payload: &CellPayload) -> bool {
    CodeSpan::from_payload(payload).is_some()
}

/// xxhash64, seed 0, of `bytes`: the hash a [`CodeSpan`] records.
pub fn xxh64(bytes: &[u8]) -> u64 {
    use core::hash::Hasher;
    let mut h = twox_hash::XxHash64::with_seed(0);
    h.write(bytes);
    h.finish()
}

/// Unsigned LEB128.
fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v as u8) | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// One unsigned LEB128 at `*at`, advancing it; `what` names the field.
fn take_varint(bytes: &[u8], at: &mut usize, what: &str) -> Result<u64, String> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let Some(&b) = bytes.get(*at) else {
            return Err(format!("truncated varint ({what}) at byte {at}"));
        };
        *at += 1;
        let low = u64::from(b & 0x7f);
        if shift == 63 && low > 1 {
            return Err(format!("varint ({what}) overflows u64"));
        }
        v |= low << shift;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(format!("varint ({what}) overflows u64"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(i: usize) -> Option<&'static str> {
        ["src/a.rs", "b/ü \"q\".py"].get(i).copied()
    }

    #[test]
    fn bytes_round_trip_at_every_width() {
        for (start, len) in [(0u64, 1u64), (127, 128), (16_383, 16_384), (u64::from(u32::MAX), 3)] {
            let span = CodeSpan { file: "b/ü \"q\".py".into(), start, end: start + len, xxh64: 0xfeed };
            let bytes = span.encode(1);
            assert_eq!(bytes[0], CODE_SPAN_TAG);
            assert_eq!(CodeSpan::decode(&bytes, 2, table), Ok(span));
        }
        let span = CodeSpan::of("src/a.rs", 5, b"fn a() {}");
        assert_eq!(span.encode(0), [&[0x02, 0, 5, 9][..], &xxh64(b"fn a() {}").to_le_bytes()].concat());
    }

    #[test]
    fn malformed_bytes_are_reasons() {
        let good = CodeSpan::of("src/a.rs", 1, b"x").encode(0);
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (vec![0x01, 0], "not a CODE span payload"),
            (vec![0x02], "truncated varint (file)"),
            (vec![0x02, 5, 0, 0], "file index 5 outside the 2-entry strings table"),
            (vec![0x02, 0, 0, 1, 1, 2], "no 8-byte xxh64"),
            ([&good[..], &[0][..]].concat(), "1 trailing byte(s)"),
            (
                [&[0x02, 0][..], &[0xff; 9][..], &[0x01, 0x02][..], &[0; 8][..]].concat(),
                "past u64",
            ),
        ];
        for (bytes, want) in cases {
            let got = CodeSpan::decode(&bytes, 2, table).unwrap_err();
            assert!(got.contains(want), "{bytes:?}: {got:?} lacks {want:?}");
        }
    }

    #[test]
    fn json_round_trips_and_names_the_range() {
        let span = CodeSpan { file: "b/ü \"q\".py".into(), start: 120, end: 480, xxh64: 0xab };
        let json = span.to_json();
        assert_eq!(
            json,
            r#"{"code_span":{"file":"b/ü \"q\".py","start":120,"end":480,"xxh64":"00000000000000ab"}}"#
        );
        assert_eq!(CodeSpan::from_payload(&span.to_payload()), Some(span.clone()));
        assert!(is_span_payload(&span.to_payload()));
        // The same string as Text, other CODE JSON, and bent spans are not spans.
        assert!(!is_span_payload(&CellPayload::Text(json)));
        assert!(!is_span_payload(&CellPayload::Json(r#"{"sites":[{"line":4}]}"#.into())));
        for bent in [
            r#"{"code_span":{"file":"a","start":9,"end":1,"xxh64":"0000000000000000"}}"#,
            r#"{"code_span":{"file":"a","start":1,"end":9,"xxh64":"00"}}"#,
            r#"{"code_span":{"file":"a","start":1,"end":9,"xxh64":"0000000000000000"},"x":1}"#,
            r#"{"code_span":{"start":1,"end":9,"xxh64":"0000000000000000"}}"#,
        ] {
            assert!(!is_span_payload(&CellPayload::Json(bent.into())), "{bent}");
        }
    }

    #[test]
    fn slice_checks_range_hash_and_utf8() {
        let src = "héllo\nfn a() {}\n".as_bytes();
        let at = src.iter().position(|&b| b == b'f').unwrap() as u64;
        let span = CodeSpan::of("a.rs", at, b"fn a() {}");
        assert_eq!(span.slice(src), Some("fn a() {}"));
        let mut edited = src.to_vec();
        edited[at as usize + 3] = b'b';
        assert_eq!(span.slice(&edited), None, "hash mismatch");
        assert_eq!(span.slice(&src[..10]), None, "short file");
        // A range splitting the two-byte 'é' hashes right but is not UTF-8.
        let split = CodeSpan::of("a.rs", 0, &src[..2]);
        assert_eq!(split.slice(src), None);
    }
}
