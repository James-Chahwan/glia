//! The SCIP wire decoder (CE.1c): a hand-written reader for the protobuf
//! subset of `scip.Index` that `glia scip import` needs, so the CLI takes no
//! protobuf dependency (Cargo.lock, and with it PARSER_STAMP, never moves for
//! it) and the engine never decodes protobuf.
//!
//! Field numbers follow scip.proto (a verbatim copy with its origin is under
//! `cli/tests/data/scip/`): `Index { 1 metadata, 2 documents, 3
//! external_symbols }`, `Metadata { 2 tool_info { 1 name, 2 version }, 3
//! project_root, 4 text_document_encoding }`, `Document { 1 relative_path, 2
//! occurrences, 3 symbols, 4 language, 6 position_encoding }`, `Occurrence { 1
//! range, 2 symbol, 3 symbol_roles, 8 single_line_range, 9 multi_line_range }`,
//! `SymbolInformation { 1 symbol, 4 relationships }`, `Relationship { 1 symbol,
//! 3 is_implementation }`. Every other field is skipped by its wire type, so a
//! schema addition upstream cannot break the decode; a field scip.proto does
//! not declare (or a declared one with another wire type) is counted in
//! [`DecodeStats::unknown_fields`].
//!
//! Streaming, as scip.proto recommends: the index is read one top-level field
//! at a time, and each `Document` is handed to the [`IndexSink`] as soon as it
//! is decoded, so memory holds one document, never the index.
//!
//! Hostile input is an `Err`, never a panic or an unbounded allocation: a
//! varint is at most 10 bytes, every length is checked against the bytes left
//! in its parent before anything is read, a top-level field is capped at
//! [`MAX_FIELD_BYTES`] and read incrementally (a length claiming more than the
//! stream holds allocates only what the stream holds), and nested messages
//! decode from slices of their parent's bytes, so recursion depth is fixed by
//! the schema (Index > Document > Occurrence > range), never by the input.

use std::io::{BufReader, ErrorKind, Read};

/// Largest top-level field (one `Document`, one `Metadata`) read into memory.
pub(crate) const MAX_FIELD_BYTES: u64 = 256 << 20;

const WIRE_VARINT: u8 = 0;
const WIRE_FIXED64: u8 = 1;
const WIRE_LEN: u8 = 2;
const WIRE_FIXED32: u8 = 5;

/// The (field, wire type) pairs scip.proto declares for each message read;
/// repeated numeric fields appear twice (packed LEN and unpacked VARINT).
/// A field outside its message's list is counted as unknown.
const METADATA_FIELDS: &[(u32, u8)] = &[
    (1, WIRE_VARINT),
    (2, WIRE_LEN),
    (3, WIRE_LEN),
    (4, WIRE_VARINT),
];
const TOOL_INFO_FIELDS: &[(u32, u8)] = &[(1, WIRE_LEN), (2, WIRE_LEN), (3, WIRE_LEN)];
const DOCUMENT_FIELDS: &[(u32, u8)] = &[
    (1, WIRE_LEN),
    (2, WIRE_LEN),
    (3, WIRE_LEN),
    (4, WIRE_LEN),
    (5, WIRE_LEN),
    (6, WIRE_VARINT),
];
const OCCURRENCE_FIELDS: &[(u32, u8)] = &[
    (1, WIRE_VARINT),
    (1, WIRE_LEN),
    (2, WIRE_LEN),
    (3, WIRE_VARINT),
    (4, WIRE_LEN),
    (5, WIRE_VARINT),
    (6, WIRE_LEN),
    (7, WIRE_VARINT),
    (7, WIRE_LEN),
    (8, WIRE_LEN),
    (9, WIRE_LEN),
    (10, WIRE_LEN),
    (11, WIRE_LEN),
];
const SYMBOL_FIELDS: &[(u32, u8)] = &[
    (1, WIRE_LEN),
    (3, WIRE_LEN),
    (4, WIRE_LEN),
    (5, WIRE_VARINT),
    (6, WIRE_LEN),
    (7, WIRE_LEN),
    (8, WIRE_LEN),
];
const RELATIONSHIP_FIELDS: &[(u32, u8)] = &[
    (1, WIRE_LEN),
    (2, WIRE_VARINT),
    (3, WIRE_VARINT),
    (4, WIRE_VARINT),
    (5, WIRE_VARINT),
];
const SINGLE_LINE_RANGE_FIELDS: &[(u32, u8)] =
    &[(1, WIRE_VARINT), (2, WIRE_VARINT), (3, WIRE_VARINT)];
const MULTI_LINE_RANGE_FIELDS: &[(u32, u8)] = &[
    (1, WIRE_VARINT),
    (2, WIRE_VARINT),
    (3, WIRE_VARINT),
    (4, WIRE_VARINT),
];

/// `Metadata`, as far as the import uses it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WireMetadata {
    /// `tool_info.name`.
    pub(crate) tool_name: String,
    /// `tool_info.version`.
    pub(crate) tool_version: String,
    /// `project_root`, a `file://` URI.
    pub(crate) project_root: String,
    /// `text_document_encoding`: 0 unspecified, 1 UTF-8, 2 UTF-16.
    pub(crate) text_document_encoding: i32,
}

/// One `Document`: its occurrences with a usable range, and its symbols.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WireDocument {
    pub(crate) relative_path: String,
    pub(crate) language: String,
    /// The proto enum value: 1 UTF-8, 2 UTF-16, 3 UTF-32 code units.
    pub(crate) position_encoding: i32,
    pub(crate) occurrences: Vec<WireOccurrence>,
    pub(crate) symbols: Vec<WireSymbol>,
}

/// One `Occurrence`: its range as (start line, start char, end line, end
/// char), whichever of the three range encodings carried it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WireOccurrence {
    pub(crate) sl: i32,
    pub(crate) sc: i32,
    pub(crate) el: i32,
    pub(crate) ec: i32,
    pub(crate) symbol: String,
    pub(crate) roles: i32,
}

/// One `SymbolInformation`: the symbol and the targets of its
/// `is_implementation` relationships (other relationships are dropped).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct WireSymbol {
    pub(crate) symbol: String,
    pub(crate) implements: Vec<String>,
}

/// What a decode saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DecodeStats {
    /// Documents handed to the sink.
    pub(crate) documents: usize,
    /// Occurrences handed to the sink (malformed ones excluded).
    pub(crate) occurrences: usize,
    /// `SymbolInformation`s inside documents.
    pub(crate) symbols: usize,
    /// `Index.external_symbols` entries, skipped unread: their symbols are
    /// defined outside the repo, so no glia node carries them.
    pub(crate) external_symbols: usize,
    /// Fields scip.proto does not declare where they appeared, skipped.
    pub(crate) unknown_fields: usize,
    /// Occurrences with no typed range and a `range` of neither 3 nor 4
    /// elements, dropped.
    pub(crate) malformed_ranges: usize,
    /// `metadata` fields that came after a document.
    pub(crate) late_metadata: usize,
}

/// Where decoded values go, in stream order.
pub(crate) trait IndexSink {
    fn metadata(&mut self, m: WireMetadata) -> Result<(), String>;
    fn document(&mut self, d: WireDocument) -> Result<(), String>;
}

/// Decode a `scip.Index` from `r` to its end, feeding `sink` one metadata /
/// document at a time. Errs on a malformed stream (truncated, an oversized or
/// out-of-bounds length, a varint over 10 bytes, a group or invalid wire
/// type, invalid UTF-8 in a string) and on the first sink error; what the
/// sink received before an error is the caller's to discard.
pub(crate) fn read_index<R: Read>(r: R, sink: &mut impl IndexSink) -> Result<DecodeStats, String> {
    let mut buf = Vec::new();
    read_index_into(r, sink, &mut buf)
}

/// [`read_index`] with the reused field buffer passed in (tests check its
/// capacity after a refused length).
fn read_index_into<R: Read>(
    r: R,
    sink: &mut impl IndexSink,
    buf: &mut Vec<u8>,
) -> Result<DecodeStats, String> {
    let mut stream = Stream {
        r: BufReader::new(r),
        pos: 0,
    };
    let mut stats = DecodeStats::default();
    loop {
        let tag_at = stream.pos;
        let Some(tag) = stream.varint("Index field tag")? else {
            break;
        };
        let (field, wire) = split_tag(tag, tag_at)?;
        match (field, wire) {
            (1, WIRE_LEN) => {
                let start = stream.read_len_field(field, buf, "Index.metadata")?;
                let m = decode_metadata(Cursor::new(buf.as_slice(), start), &mut stats)?;
                if stats.documents > 0 {
                    stats.late_metadata += 1;
                }
                sink.metadata(m)?;
            }
            (2, WIRE_LEN) => {
                let start = stream.read_len_field(field, buf, "Index.documents")?;
                let d = decode_document(Cursor::new(buf.as_slice(), start), &mut stats)?;
                stats.documents += 1;
                stats.occurrences += d.occurrences.len();
                stats.symbols += d.symbols.len();
                sink.document(d)?;
            }
            (3, WIRE_LEN) => {
                stream.skip_value(field, wire, "Index.external_symbols")?;
                stats.external_symbols += 1;
            }
            _ => {
                // Index declares fields 1..=3, all LEN, all matched above.
                stats.unknown_fields += 1;
                stream.skip_value(field, wire, "Index")?;
            }
        }
    }
    Ok(stats)
}

/// Folds byte `i` (0-based) of a varint into `value`; `Ok(true)` when it was
/// the last byte. The 10th byte may carry bit 0 only (bits 63.. of a u64).
fn varint_byte(i: usize, b: u8, value: &mut u64) -> Result<bool, ()> {
    if i == 9 && b > 1 {
        return Err(());
    }
    *value |= u64::from(b & 0x7f) << (7 * i);
    Ok(b & 0x80 == 0)
}

/// (field number, wire type) of a tag read at byte `at`.
fn split_tag(tag: u64, at: u64) -> Result<(u32, u8), String> {
    let field = tag >> 3;
    let wire = (tag & 7) as u8;
    if field == 0 || field > u64::from(u32::MAX >> 3) {
        return Err(format!("invalid field number {field} at byte {at}"));
    }
    match wire {
        WIRE_VARINT | WIRE_FIXED64 | WIRE_LEN | WIRE_FIXED32 => Ok((field as u32, wire)),
        3 | 4 => Err(format!(
            "group wire type {wire} (field {field}) at byte {at}: scip.proto declares no groups"
        )),
        _ => Err(format!(
            "invalid wire type {wire} (field {field}) at byte {at}"
        )),
    }
}

/// The top-level byte stream, with its read position for error messages.
struct Stream<R> {
    r: BufReader<R>,
    pos: u64,
}

impl<R: Read> Stream<R> {
    fn byte(&mut self) -> Result<Option<u8>, String> {
        let mut b = [0u8];
        loop {
            match self.r.read(&mut b) {
                Ok(0) => return Ok(None),
                Ok(_) => {
                    self.pos += 1;
                    return Ok(Some(b[0]));
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(format!("read error at byte {}: {e}", self.pos)),
            }
        }
    }

    /// A varint, or `None` at a clean end of stream before its first byte.
    fn varint(&mut self, what: &str) -> Result<Option<u64>, String> {
        let start = self.pos;
        let mut value = 0;
        for i in 0..10 {
            let Some(b) = self.byte()? else {
                if i == 0 {
                    return Ok(None);
                }
                return Err(format!("truncated {what} at byte {start}"));
            };
            if varint_byte(i, b, &mut value)
                .map_err(|()| format!("varint overflow in {what} at byte {start}"))?
            {
                return Ok(Some(value));
            }
        }
        Err(format!("varint overflow in {what} at byte {start}"))
    }

    /// A LEN field's length, checked against [`MAX_FIELD_BYTES`].
    fn len(&mut self, field: u32, what: &str) -> Result<u64, String> {
        let at = self.pos;
        let len = self
            .varint(what)?
            .ok_or_else(|| format!("truncated {what} at byte {at}"))?;
        if len > MAX_FIELD_BYTES {
            return Err(format!(
                "{what} (field {field}) at byte {at} declares {len} bytes, over the {} MiB field cap",
                MAX_FIELD_BYTES >> 20
            ));
        }
        Ok(len)
    }

    /// Read a LEN field's payload into `buf` (cleared first); returns the
    /// payload's stream offset. `buf` grows with the bytes actually read,
    /// never to the declared length up front.
    fn read_len_field(&mut self, field: u32, buf: &mut Vec<u8>, what: &str) -> Result<u64, String> {
        let len = self.len(field, what)?;
        let start = self.pos;
        buf.clear();
        let got = (&mut self.r)
            .take(len)
            .read_to_end(buf)
            .map_err(|e| format!("read error in {what} at byte {start}: {e}"))?;
        self.pos += got as u64;
        if (got as u64) < len {
            return Err(format!(
                "truncated {what} at byte {start}: {len} bytes declared, {got} present"
            ));
        }
        Ok(start)
    }

    fn skip_bytes(&mut self, n: u64, what: &str) -> Result<(), String> {
        let start = self.pos;
        let got = std::io::copy(&mut (&mut self.r).take(n), &mut std::io::sink())
            .map_err(|e| format!("read error in {what} at byte {start}: {e}"))?;
        self.pos += got;
        if got < n {
            return Err(format!(
                "truncated {what} at byte {start}: {n} bytes declared, {got} present"
            ));
        }
        Ok(())
    }

    /// Skip one field value of wire type `wire`.
    fn skip_value(&mut self, field: u32, wire: u8, what: &str) -> Result<(), String> {
        match wire {
            WIRE_VARINT => {
                let at = self.pos;
                self.varint(what)?
                    .ok_or_else(|| format!("truncated {what} at byte {at}"))?;
                Ok(())
            }
            WIRE_FIXED64 => self.skip_bytes(8, what),
            WIRE_FIXED32 => self.skip_bytes(4, what),
            _ => {
                let len = self.len(field, what)?;
                self.skip_bytes(len, what)
            }
        }
    }
}

/// A read position inside one already-read field's bytes; `base` is the
/// stream offset of `buf[0]`, so errors name absolute byte offsets.
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
    base: u64,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8], base: u64) -> Self {
        Cursor { buf, pos: 0, base }
    }

    fn at(&self) -> u64 {
        self.base + self.pos as u64
    }

    fn done(&self) -> bool {
        self.pos >= self.buf.len()
    }

    fn left(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn varint(&mut self, what: &str) -> Result<u64, String> {
        let start = self.at();
        let mut value = 0;
        for i in 0..10 {
            let Some(&b) = self.buf.get(self.pos) else {
                return Err(format!("truncated {what} at byte {start}"));
            };
            self.pos += 1;
            if varint_byte(i, b, &mut value)
                .map_err(|()| format!("varint overflow in {what} at byte {start}"))?
            {
                return Ok(value);
            }
        }
        Err(format!("varint overflow in {what} at byte {start}"))
    }

    /// An int32 / enum field: protobuf sends a negative int32 as a 10-byte
    /// varint, and a wider value truncates to its low 32 bits.
    fn int32(&mut self, what: &str) -> Result<i32, String> {
        Ok(self.varint(what)? as i64 as i32)
    }

    fn tag(&mut self, what: &str) -> Result<(u32, u8), String> {
        let at = self.at();
        let tag = self.varint(what)?;
        split_tag(tag, at)
    }

    /// A LEN field's payload as a sub-cursor, its length checked against
    /// the bytes left in this message before anything is taken.
    fn sub(&mut self, what: &str) -> Result<Cursor<'a>, String> {
        let at = self.at();
        let len = self.varint(what)?;
        let left = self.left();
        let start = self.pos;
        let slice = usize::try_from(len)
            .ok()
            .filter(|&n| n <= left)
            .and_then(|n| self.buf.get(start..start + n))
            .ok_or_else(|| {
                format!(
                    "truncated {what} at byte {at}: {len} bytes declared, {left} left in its parent"
                )
            })?;
        self.pos = start + slice.len();
        Ok(Cursor {
            buf: slice,
            pos: 0,
            base: self.base + start as u64,
        })
    }

    fn string(&mut self, what: &str) -> Result<String, String> {
        let sub = self.sub(what)?;
        std::str::from_utf8(sub.buf)
            .map(str::to_owned)
            .map_err(|_| format!("invalid UTF-8 in {what} at byte {}", sub.base))
    }

    fn skip_bytes(&mut self, n: usize, what: &str) -> Result<(), String> {
        if n > self.left() {
            return Err(format!("truncated {what} at byte {}", self.at()));
        }
        self.pos += n;
        Ok(())
    }

    /// Skip a field the decode does not read, counting it as unknown unless
    /// `known` (its message's scip.proto fields) declares it.
    fn skip_field(
        &mut self,
        field: u32,
        wire: u8,
        known: &[(u32, u8)],
        what: &str,
        stats: &mut DecodeStats,
    ) -> Result<(), String> {
        if !known.contains(&(field, wire)) {
            stats.unknown_fields += 1;
        }
        match wire {
            WIRE_VARINT => self.varint(what).map(drop),
            WIRE_FIXED64 => self.skip_bytes(8, what),
            WIRE_FIXED32 => self.skip_bytes(4, what),
            _ => self.sub(what).map(drop),
        }
    }
}

fn decode_metadata(mut c: Cursor<'_>, stats: &mut DecodeStats) -> Result<WireMetadata, String> {
    let mut m = WireMetadata::default();
    while !c.done() {
        match c.tag("Metadata field tag")? {
            (2, WIRE_LEN) => {
                let mut t = c.sub("Metadata.tool_info")?;
                while !t.done() {
                    match t.tag("ToolInfo field tag")? {
                        (1, WIRE_LEN) => m.tool_name = t.string("ToolInfo.name")?,
                        (2, WIRE_LEN) => m.tool_version = t.string("ToolInfo.version")?,
                        (f, w) => t.skip_field(f, w, TOOL_INFO_FIELDS, "ToolInfo", stats)?,
                    }
                }
            }
            (3, WIRE_LEN) => m.project_root = c.string("Metadata.project_root")?,
            (4, WIRE_VARINT) => {
                m.text_document_encoding = c.int32("Metadata.text_document_encoding")?
            }
            (f, w) => c.skip_field(f, w, METADATA_FIELDS, "Metadata", stats)?,
        }
    }
    Ok(m)
}

fn decode_document(mut c: Cursor<'_>, stats: &mut DecodeStats) -> Result<WireDocument, String> {
    let mut d = WireDocument::default();
    while !c.done() {
        match c.tag("Document field tag")? {
            (1, WIRE_LEN) => d.relative_path = c.string("Document.relative_path")?,
            (2, WIRE_LEN) => {
                let o = c.sub("Document.occurrences")?;
                match decode_occurrence(o, stats)? {
                    Some(o) => d.occurrences.push(o),
                    None => stats.malformed_ranges += 1,
                }
            }
            (3, WIRE_LEN) => {
                let s = c.sub("Document.symbols")?;
                d.symbols.push(decode_symbol(s, stats)?);
            }
            (4, WIRE_LEN) => d.language = c.string("Document.language")?,
            (6, WIRE_VARINT) => d.position_encoding = c.int32("Document.position_encoding")?,
            (f, w) => c.skip_field(f, w, DOCUMENT_FIELDS, "Document", stats)?,
        }
    }
    Ok(d)
}

/// The oneof `typed_range`: the member seen last wins, and a member seen
/// twice merges (protobuf's rules for a oneof of messages).
enum TypedRange {
    None,
    Single([i32; 3]),
    Multi([i32; 4]),
}

/// The deprecated `range`: its first four elements and how many it had.
#[derive(Default)]
struct PackedRange {
    first: [i32; 4],
    count: usize,
}

impl PackedRange {
    fn push(&mut self, v: i32) {
        if let Some(slot) = self.first.get_mut(self.count) {
            *slot = v;
        }
        self.count = self.count.saturating_add(1);
    }
}

/// `None` when the occurrence has no usable range: no typed range, and a
/// `range` of neither 3 (`[line, start, end]`) nor 4 elements.
fn decode_occurrence(
    mut c: Cursor<'_>,
    stats: &mut DecodeStats,
) -> Result<Option<WireOccurrence>, String> {
    let mut o = WireOccurrence::default();
    let mut typed = TypedRange::None;
    let mut range = PackedRange::default();
    while !c.done() {
        match c.tag("Occurrence field tag")? {
            (1, WIRE_VARINT) => range.push(c.int32("Occurrence.range")?),
            (1, WIRE_LEN) => {
                let mut p = c.sub("Occurrence.range")?;
                while !p.done() {
                    range.push(p.int32("Occurrence.range")?);
                }
            }
            (2, WIRE_LEN) => o.symbol = c.string("Occurrence.symbol")?,
            (3, WIRE_VARINT) => o.roles = c.int32("Occurrence.symbol_roles")?,
            (8, WIRE_LEN) => {
                let mut v = match typed {
                    TypedRange::Single(v) => v,
                    _ => [0; 3],
                };
                let mut r = c.sub("Occurrence.single_line_range")?;
                while !r.done() {
                    match r.tag("SingleLineRange field tag")? {
                        (f @ 1..=3, WIRE_VARINT) => {
                            let value = r.int32("SingleLineRange")?;
                            if let Some(slot) = v.get_mut(f as usize - 1) {
                                *slot = value;
                            }
                        }
                        (f, w) => {
                            r.skip_field(f, w, SINGLE_LINE_RANGE_FIELDS, "SingleLineRange", stats)?
                        }
                    }
                }
                typed = TypedRange::Single(v);
            }
            (9, WIRE_LEN) => {
                let mut v = match typed {
                    TypedRange::Multi(v) => v,
                    _ => [0; 4],
                };
                let mut r = c.sub("Occurrence.multi_line_range")?;
                while !r.done() {
                    match r.tag("MultiLineRange field tag")? {
                        (f @ 1..=4, WIRE_VARINT) => {
                            let value = r.int32("MultiLineRange")?;
                            if let Some(slot) = v.get_mut(f as usize - 1) {
                                *slot = value;
                            }
                        }
                        (f, w) => {
                            r.skip_field(f, w, MULTI_LINE_RANGE_FIELDS, "MultiLineRange", stats)?
                        }
                    }
                }
                typed = TypedRange::Multi(v);
            }
            (f, w) => c.skip_field(f, w, OCCURRENCE_FIELDS, "Occurrence", stats)?,
        }
    }
    let [a, b, cc, d] = range.first;
    (o.sl, o.sc, o.el, o.ec) = match typed {
        TypedRange::Single([line, start, end]) => (line, start, line, end),
        TypedRange::Multi([sl, sc, el, ec]) => (sl, sc, el, ec),
        TypedRange::None => match range.count {
            3 => (a, b, a, cc),
            4 => (a, b, cc, d),
            _ => return Ok(None),
        },
    };
    Ok(Some(o))
}

fn decode_symbol(mut c: Cursor<'_>, stats: &mut DecodeStats) -> Result<WireSymbol, String> {
    let mut s = WireSymbol::default();
    while !c.done() {
        match c.tag("SymbolInformation field tag")? {
            (1, WIRE_LEN) => s.symbol = c.string("SymbolInformation.symbol")?,
            (4, WIRE_LEN) => {
                let mut r = c.sub("SymbolInformation.relationships")?;
                let mut target = String::new();
                let mut is_implementation = false;
                while !r.done() {
                    match r.tag("Relationship field tag")? {
                        (1, WIRE_LEN) => target = r.string("Relationship.symbol")?,
                        (3, WIRE_VARINT) => {
                            is_implementation = r.varint("Relationship.is_implementation")? != 0
                        }
                        (f, w) => r.skip_field(f, w, RELATIONSHIP_FIELDS, "Relationship", stats)?,
                    }
                }
                if is_implementation {
                    s.implements.push(target);
                }
            }
            (f, w) => c.skip_field(f, w, SYMBOL_FIELDS, "SymbolInformation", stats)?,
        }
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE: &[u8] = include_bytes!("../../../../tests/data/scip/probe.scip");
    const USER_SAVE: &str = "scip-python python svc 0.1 `svc.repos`/UserRepo#save().";
    const ORDER_SAVE: &str = "scip-python python svc 0.1 `svc.repos`/OrderRepo#save().";
    const REPOSITORY_SAVE: &str =
        "scip-python python repokit 1.2 `repokit.base`/Repository#save().";

    /// A sink that keeps everything it is handed.
    #[derive(Default, Debug, PartialEq)]
    struct Collect {
        metadata: Vec<WireMetadata>,
        documents: Vec<WireDocument>,
    }

    impl IndexSink for Collect {
        fn metadata(&mut self, m: WireMetadata) -> Result<(), String> {
            self.metadata.push(m);
            Ok(())
        }
        fn document(&mut self, d: WireDocument) -> Result<(), String> {
            self.documents.push(d);
            Ok(())
        }
    }

    fn decode(bytes: &[u8]) -> Result<(Collect, DecodeStats), String> {
        let mut sink = Collect::default();
        let stats = read_index(bytes, &mut sink)?;
        Ok((sink, stats))
    }

    // --- a minimal encoder, for hand-built indexes -------------------------

    fn varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return out;
            }
            out.push(b | 0x80);
        }
    }

    fn tag(field: u32, wire: u8) -> Vec<u8> {
        varint(u64::from(field) << 3 | u64::from(wire))
    }

    fn vint(field: u32, v: i64) -> Vec<u8> {
        [tag(field, WIRE_VARINT), varint(v as u64)].concat()
    }

    fn len(field: u32, payload: &[u8]) -> Vec<u8> {
        [
            tag(field, WIRE_LEN),
            varint(payload.len() as u64),
            payload.to_vec(),
        ]
        .concat()
    }

    fn fixed32(field: u32) -> Vec<u8> {
        [tag(field, WIRE_FIXED32), vec![1, 2, 3, 4]].concat()
    }

    fn fixed64(field: u32) -> Vec<u8> {
        [tag(field, WIRE_FIXED64), vec![1, 2, 3, 4, 5, 6, 7, 8]].concat()
    }

    /// Fields 99 (varint), 98 (LEN) and 97 (fixed32), in no message's schema.
    fn unknown_trio() -> Vec<u8> {
        [vint(99, 7), len(98, b"future"), fixed32(97)].concat()
    }

    fn metadata_field() -> Vec<u8> {
        let tool = [len(1, b"scip-go"), len(2, b"0.1.0")].concat();
        len(
            1,
            &[len(2, &tool), len(3, b"file:///w"), vint(4, 1)].concat(),
        )
    }

    /// An occurrence with `range_fields` spliced in before its symbol.
    fn occurrence(range_fields: &[u8], extra: &[u8]) -> Vec<u8> {
        [range_fields, &len(2, b"s f()."), &vint(3, 8), extra].concat()
    }

    fn document_field(occurrences: &[Vec<u8>]) -> Vec<u8> {
        let mut body = len(1, b"a.go");
        for o in occurrences {
            body.extend(len(2, o));
        }
        body.extend(len(4, b"go"));
        body.extend(vint(6, 1));
        len(2, &body)
    }

    fn packed(values: &[i64]) -> Vec<u8> {
        let payload: Vec<u8> = values.iter().flat_map(|&v| varint(v as u64)).collect();
        len(1, &payload)
    }

    fn unpacked(values: &[i64]) -> Vec<u8> {
        values.iter().flat_map(|&v| vint(1, v)).collect()
    }

    fn single_line(line: i64, start: i64, end: i64) -> Vec<u8> {
        len(8, &[vint(1, line), vint(2, start), vint(3, end)].concat())
    }

    /// Stream offsets where a top-level field starts, plus the end.
    fn top_level_boundaries(bytes: &[u8]) -> Vec<(usize, u32)> {
        let read = |pos: &mut usize| {
            let mut v = 0u64;
            let mut shift = 0;
            loop {
                let b = bytes[*pos];
                *pos += 1;
                v |= u64::from(b & 0x7f) << shift;
                shift += 7;
                if b & 0x80 == 0 {
                    return v;
                }
            }
        };
        let mut out = Vec::new();
        let mut pos = 0;
        while pos < bytes.len() {
            let start = pos;
            let t = read(&mut pos);
            assert_eq!(
                t & 7,
                u64::from(WIRE_LEN),
                "the probe's top-level fields are all LEN"
            );
            let n = read(&mut pos) as usize;
            pos += n;
            out.push((start, (t >> 3) as u32));
        }
        out.push((bytes.len(), 0));
        out
    }

    #[test]
    fn decodes_the_committed_probe_index() {
        let (sink, stats) = decode(PROBE).expect("probe.scip decodes");
        assert_eq!(
            sink.metadata,
            [WireMetadata {
                tool_name: "scip-python".into(),
                tool_version: "0.6.0".into(),
                project_root: "file:///work/svc-probe".into(),
                text_document_encoding: 1,
            }]
        );
        let occ = |sl, sc, el, ec, symbol: &str, roles| WireOccurrence {
            sl,
            sc,
            el,
            ec,
            symbol: symbol.into(),
            roles,
        };
        assert_eq!(
            sink.documents,
            [
                WireDocument {
                    relative_path: "svc/repos.py".into(),
                    language: "python".into(),
                    position_encoding: 0,
                    occurrences: vec![
                        occ(1, 8, 1, 12, USER_SAVE, 1),
                        occ(6, 8, 6, 12, ORDER_SAVE, 1)
                    ],
                    symbols: vec![WireSymbol {
                        symbol: USER_SAVE.into(),
                        implements: vec![REPOSITORY_SAVE.into()]
                    }],
                },
                WireDocument {
                    relative_path: "svc/handlers.py".into(),
                    language: "python".into(),
                    position_encoding: 1,
                    occurrences: vec![occ(5, 16, 5, 20, USER_SAVE, 8)],
                    symbols: vec![],
                },
            ]
        );
        assert_eq!(
            stats,
            DecodeStats {
                documents: 2,
                occurrences: 3,
                symbols: 1,
                external_symbols: 1,
                unknown_fields: 0,
                malformed_ranges: 0,
                late_metadata: 0,
            }
        );
    }

    #[test]
    fn every_prefix_is_an_error_or_a_clean_prefix() {
        let boundaries = top_level_boundaries(PROBE);
        let (full, _) = decode(PROBE).expect("probe.scip decodes");
        for n in 0..PROBE.len() {
            match decode(&PROBE[..n]) {
                Ok((sink, _)) => {
                    assert!(
                        boundaries.iter().any(|&(b, _)| b == n),
                        "a prefix of {n} bytes decoded but is not a top-level field boundary"
                    );
                    let whole_docs = boundaries.iter().filter(|&&(b, f)| f == 2 && b < n).count();
                    assert_eq!(sink.documents.len(), whole_docs, "prefix {n}");
                    assert_eq!(
                        sink.documents[..],
                        full.documents[..whole_docs],
                        "prefix {n}"
                    );
                }
                Err(e) => assert!(
                    !boundaries.iter().any(|&(b, _)| b == n),
                    "a prefix of {n} bytes ends on a field boundary but failed: {e}"
                ),
            }
        }
    }

    #[test]
    fn unknown_fields_are_skipped() {
        let plain_occ = occurrence(&packed(&[3, 1, 4]), &[]);
        let base = [
            metadata_field(),
            document_field(std::slice::from_ref(&plain_occ)),
        ]
        .concat();
        let (want, stats) = decode(&base).expect("base decodes");
        assert_eq!(stats.unknown_fields, 0);

        // Top level: between the metadata and the document.
        let top = [
            metadata_field(),
            unknown_trio(),
            document_field(&[plain_occ]),
        ]
        .concat();
        let (got, stats) = decode(&top).expect("top-level unknowns decode");
        assert_eq!(got, want);
        assert_eq!(stats.unknown_fields, 3);

        // Inside an Occurrence.
        let inside = [
            metadata_field(),
            document_field(&[occurrence(&packed(&[3, 1, 4]), &unknown_trio())]),
        ]
        .concat();
        let (got, stats) = decode(&inside).expect("occurrence unknowns decode");
        assert_eq!(got, want);
        assert_eq!(stats.unknown_fields, 3);

        // A declared-but-unread field (Occurrence.syntax_kind, 5) is not
        // unknown; a fixed64 field 96, declared nowhere, is.
        let declared = [
            metadata_field(),
            document_field(&[occurrence(
                &packed(&[3, 1, 4]),
                &[vint(5, 6), fixed64(96)].concat(),
            )]),
        ]
        .concat();
        let (got, stats) = decode(&declared).expect("declared fields decode");
        assert_eq!(got, want);
        assert_eq!(
            stats.unknown_fields, 1,
            "only field 96 is outside scip.proto"
        );

        // A declared field with the wrong wire type is skipped and counted.
        let wrong_wire = [
            metadata_field(),
            document_field(&[occurrence(&packed(&[3, 1, 4]), &len(3, b"xx"))]),
        ]
        .concat();
        let (got, stats) = decode(&wrong_wire).expect("wrong wire type decodes");
        assert_eq!(got, want);
        assert_eq!(stats.unknown_fields, 1);
    }

    #[test]
    fn packed_and_unpacked_ranges_agree() {
        let with = |range: &[u8]| {
            let bytes = [metadata_field(), document_field(&[occurrence(range, &[])])].concat();
            let (sink, stats) = decode(&bytes).expect("decodes");
            (
                sink.documents[0].occurrences.clone(),
                stats.malformed_ranges,
            )
        };
        let expect3 = |sl, sc, el, ec| {
            vec![WireOccurrence {
                sl,
                sc,
                el,
                ec,
                symbol: "s f().".into(),
                roles: 8,
            }]
        };
        assert_eq!(with(&packed(&[3, 1, 4])), (expect3(3, 1, 3, 4), 0));
        assert_eq!(with(&unpacked(&[3, 1, 4])), (expect3(3, 1, 3, 4), 0));
        assert_eq!(with(&packed(&[2, 5, 7, 9])), (expect3(2, 5, 7, 9), 0));
        assert_eq!(with(&unpacked(&[2, 5, 7, 9])), (expect3(2, 5, 7, 9), 0));
        // Repeated elements concatenate across packed and unpacked runs.
        assert_eq!(
            with(&[packed(&[2, 5]), unpacked(&[7, 9])].concat()),
            (expect3(2, 5, 7, 9), 0)
        );

        // A typed single_line_range wins over a conflicting `range`, in either order.
        assert_eq!(
            with(&[packed(&[9, 9, 9]), single_line(4, 2, 6)].concat()),
            (expect3(4, 2, 4, 6), 0)
        );
        assert_eq!(
            with(&[single_line(4, 2, 6), packed(&[9, 9, 9])].concat()),
            (expect3(4, 2, 4, 6), 0)
        );
        // ... even when the `range` alone would be malformed.
        assert_eq!(
            with(&[packed(&[9, 9]), single_line(4, 2, 6)].concat()),
            (expect3(4, 2, 4, 6), 0)
        );
        // multi_line_range, and the last oneof member seen wins.
        let multi = len(
            9,
            &[vint(1, 1), vint(2, 2), vint(3, 3), vint(4, 4)].concat(),
        );
        assert_eq!(with(&multi), (expect3(1, 2, 3, 4), 0));
        assert_eq!(
            with(&[multi.clone(), single_line(4, 2, 6)].concat()),
            (expect3(4, 2, 4, 6), 0)
        );
        assert_eq!(
            with(&[single_line(4, 2, 6), multi].concat()),
            (expect3(1, 2, 3, 4), 0)
        );

        // A negative int32 is a 10-byte varint.
        assert_eq!(with(&unpacked(&[0, -1, 5])), (expect3(0, -1, 0, 5), 0));

        // Neither 3 nor 4 elements, and no typed range: dropped and counted.
        for bad in [
            packed(&[]),
            packed(&[1, 2]),
            packed(&[1, 2, 3, 4, 5]),
            Vec::new(),
        ] {
            assert_eq!(with(&bad), (vec![], 1));
        }
    }

    #[test]
    fn hostile_lengths_are_refused() {
        // A top-level LEN of 2^40 is over the field cap before any read.
        let mut buf = Vec::new();
        let huge = [tag(2, WIRE_LEN), varint(1 << 40)].concat();
        let err = read_index_into(&huge[..], &mut Collect::default(), &mut buf).unwrap_err();
        assert!(err.contains("over the 256 MiB field cap"), "{err}");
        assert_eq!(buf.capacity(), 0, "nothing allocated for a refused length");

        // Under the cap but past the end of the stream: truncated, and the
        // buffer grew only with the bytes that were there.
        let short = [tag(2, WIRE_LEN), varint(200 << 20), vec![0u8; 10]].concat();
        let err = read_index_into(&short[..], &mut Collect::default(), &mut buf).unwrap_err();
        assert!(err.contains("truncated Index.documents at byte 5"), "{err}");
        assert!(
            buf.capacity() < 4096,
            "buffer grew to {} for a 10-byte payload",
            buf.capacity()
        );

        // A nested LEN past its parent.
        let doc = [
            len(1, b"a.go"),
            tag(2, WIRE_LEN),
            varint(1000),
            vec![0u8; 4],
        ]
        .concat();
        let nested = [metadata_field(), len(2, &doc)].concat();
        let err = decode(&nested).unwrap_err();
        assert!(
            err.contains("truncated Document.occurrences at byte"),
            "{err}"
        );
        assert!(
            err.contains("1000 bytes declared, 4 left in its parent"),
            "{err}"
        );

        // An 11-byte varint, at top level and nested.
        let eleven = [vec![0x80u8; 10], vec![0x01]].concat();
        let err = decode(&eleven).unwrap_err();
        assert!(err.contains("varint overflow"), "{err}");
        let nested_eleven = [
            metadata_field(),
            len(2, &[tag(6, WIRE_VARINT), eleven.clone()].concat()),
        ]
        .concat();
        let err = decode(&nested_eleven).unwrap_err();
        assert!(
            err.contains("varint overflow in Document.position_encoding"),
            "{err}"
        );
        // A 10th byte carrying more than bit 0.
        let err = decode(&[vec![0xffu8; 9], vec![0x02]].concat()).unwrap_err();
        assert!(err.contains("varint overflow"), "{err}");

        // Group wire types, and invalid ones.
        for (wire, what) in [
            (3u8, "group wire type 3"),
            (4, "group wire type 4"),
            (6, "invalid wire type 6"),
        ] {
            let err = decode(&tag(5, wire)).unwrap_err();
            assert!(err.contains(what), "{err}");
            let inside = [metadata_field(), len(2, &tag(9, wire))].concat();
            let err = decode(&inside).unwrap_err();
            assert!(err.contains(what), "{err}");
        }
        // Field number 0.
        let err = decode(&[0x02, 0x00]).unwrap_err();
        assert!(err.contains("invalid field number 0"), "{err}");

        // Invalid UTF-8 in a string.
        let bad_utf8 = [metadata_field(), len(2, &len(1, &[0xff, 0xfe]))].concat();
        let err = decode(&bad_utf8).unwrap_err();
        assert!(
            err.contains("invalid UTF-8 in Document.relative_path"),
            "{err}"
        );
    }

    #[test]
    fn late_metadata_and_external_symbols_are_counted() {
        let bytes = [
            metadata_field(),
            document_field(&[occurrence(&packed(&[0, 0, 1]), &[])]),
            len(3, &len(1, b"ext")),
            metadata_field(),
        ]
        .concat();
        let (sink, stats) = decode(&bytes).expect("decodes");
        assert_eq!(sink.metadata.len(), 2);
        assert_eq!(
            (stats.late_metadata, stats.external_symbols, stats.documents),
            (1, 1, 1)
        );
    }

    #[test]
    fn only_implementation_relationships_are_kept() {
        let rel = |target: &[u8], implementation: bool, type_def: bool| {
            len(
                4,
                &[
                    len(1, target),
                    vint(3, i64::from(implementation)),
                    vint(4, i64::from(type_def)),
                ]
                .concat(),
            )
        };
        let symbol = [
            len(1, b"s A#"),
            rel(b"s I#", true, false),
            rel(b"s T#", false, true),
            rel(b"s J#", true, false),
        ]
        .concat();
        let doc = len(2, &[len(1, b"a.go"), len(3, &symbol)].concat());
        let (sink, stats) = decode(&[metadata_field(), doc].concat()).expect("decodes");
        assert_eq!(
            sink.documents[0].symbols,
            [WireSymbol {
                symbol: "s A#".into(),
                implements: vec!["s I#".into(), "s J#".into()]
            }]
        );
        assert_eq!(stats.unknown_fields, 0);
    }

    #[test]
    fn sink_errors_stop_the_decode() {
        struct Refuse(usize);
        impl IndexSink for Refuse {
            fn metadata(&mut self, _: WireMetadata) -> Result<(), String> {
                Ok(())
            }
            fn document(&mut self, _: WireDocument) -> Result<(), String> {
                self.0 += 1;
                Err("no".into())
            }
        }
        let mut sink = Refuse(0);
        assert_eq!(read_index(PROBE, &mut sink), Err("no".into()));
        assert_eq!(
            sink.0, 1,
            "no document is decoded after the sink refused one"
        );
    }

    #[test]
    fn protoc_reencodes_the_fixtures() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/scip");
        for name in ["probe", "utf16"] {
            let textproto =
                std::fs::read(dir.join(format!("{name}.textproto"))).expect("textproto");
            let committed =
                std::fs::read(dir.join(format!("{name}.scip"))).expect("committed .scip");
            let child = std::process::Command::new("protoc")
                .arg(format!("--proto_path={}", dir.display()))
                .arg("--encode=scip.Index")
                .arg("scip.proto")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn();
            let mut child = match child {
                Ok(child) => child,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    eprintln!("[scip] protoc absent - re-encode check skipped");
                    return;
                }
                Err(e) => panic!("protoc: {e}"),
            };
            {
                use std::io::Write as _;
                let mut stdin = child.stdin.take().expect("stdin");
                stdin.write_all(&textproto).expect("write textproto");
            }
            let out = child.wait_with_output().expect("protoc runs");
            assert!(
                out.status.success(),
                "protoc failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(
                out.stdout, committed,
                "{name}.scip is not what protoc encodes from {name}.textproto"
            );
            decode(&committed).expect("the committed index decodes");
        }
    }
}
