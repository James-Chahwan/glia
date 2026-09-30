//! `glia scip import <repo> <index.scip>` (CE.1c) — the CLI surface of the
//! SCIP snapshot step. A compiler-grade indexer (scip-python, scip-typescript,
//! scip-java, scip-go, rust-analyzer's `scip`) writes one protobuf
//! `scip.Index`; this command decodes it with the hand-written streaming
//! reader in `wire.rs` (no protobuf dependency) and feeds each document, as
//! soon as it is decoded, to `glia_snapshots::ScipImporter`, which reads the
//! document's source, keeps definition names and reference call flags, and
//! writes `<repo>/.glia/scip-snapshot/` (CE.1a's format) in `finish`.
//!
//! The build never runs an indexer or reads the index: it ingests whatever
//! snapshot is on disk (docs/overlay.md, "SCIP snapshot"). Transport only: the
//! snapshot rules and the `[scip] import ... surface=cli` marker live in the
//! snapshots crate; this module prints the decoder's own marker,
//! `[scip] decoded index=<file> documents=<d> occurrences=<o> symbols=<s>
//! external_symbols=<e> unknown_fields=<u> malformed_ranges=<m>`, after a clean
//! decode. Exits 0 on a written snapshot, 1 when the index cannot be decoded or
//! imported (nothing is written: only `finish`, after the whole index decoded,
//! writes), 2 on a usage error.

mod wire;

use std::fs::File;
use std::path::{Path, PathBuf};

use clap::Subcommand;
use glia_code_domain::snapshots::scip_dir;
use glia_snapshots::{
    PositionEncoding, ScipDocumentIn, ScipImportOptions, ScipImportSummary, ScipImporter,
    ScipIndexInfo, ScipOccurrenceIn, ScipSymbolIn,
};

use wire::{DecodeStats, IndexSink, WireDocument, WireMetadata, read_index};

/// `Metadata.text_document_encoding` UTF16.
const TEXT_ENCODING_UTF16: i32 = 2;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    action: ScipCmd,
}

#[derive(Subcommand, Debug)]
enum ScipCmd {
    /// Decode a SCIP index and write `<repo>/.glia/scip-snapshot/`, replacing
    /// any earlier snapshot. Then `glia build <repo>` ingests it.
    Import {
        /// Repo the index was made for.
        repo: PathBuf,
        /// The SCIP index file (a protobuf `scip.Index`, usually `index.scip`).
        index: PathBuf,
        /// Directory the index's document paths are relative to, relative to
        /// the repo (`.` is the repo root). Overrides the index's
        /// `project_root`; give it when the index was made on another machine
        /// or in a container.
        #[arg(long)]
        prefix: Option<String>,
    },
}

pub(crate) fn run(args: Args) -> i32 {
    match args.action {
        ScipCmd::Import {
            repo,
            index,
            prefix,
        } => match import(&repo, &index, prefix) {
            Ok(summary) => {
                println!(
                    "wrote {} documents, {} symbols -> {}",
                    summary.documents,
                    summary.symbols,
                    scip_dir(&repo).display()
                );
                println!("run `glia build {}` to ingest.", repo.display());
                0
            }
            Err(e) => {
                eprintln!("error: {e}");
                1
            }
        },
    }
}

/// Decode `index` into an importer for `repo`, then write the snapshot.
fn import(repo: &Path, index: &Path, prefix: Option<String>) -> Result<ScipImportSummary, String> {
    let file = File::open(index).map_err(|e| format!("{}: {e}", index.display()))?;
    let mut sink = ImportSink {
        repo,
        prefix,
        importer: None,
        extra_metadata: 0,
    };
    let stats = read_index(file, &mut sink)
        .map_err(|e| format!("{}: {e}; nothing written", index.display()))?;
    eprintln!("{}", decoded_marker(index, &stats));
    if sink.extra_metadata > 0 {
        eprintln!(
            "[scip] warning: the index repeats its metadata {} time(s), {} after a document; the first one stands",
            sink.extra_metadata, stats.late_metadata
        );
    }
    let importer = sink.importer.ok_or_else(|| {
        format!(
            "{}: the index has no metadata and no document; nothing written",
            index.display()
        )
    })?;
    importer.finish()
}

/// The fired_on line of a clean decode.
fn decoded_marker(index: &Path, s: &DecodeStats) -> String {
    let name: String = index
        .file_name()
        .map_or_else(
            || index.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
        .chars()
        .map(|c| if c.is_whitespace() { '_' } else { c })
        .collect();
    format!(
        "[scip] decoded index={name} documents={} occurrences={} symbols={} external_symbols={} \
         unknown_fields={} malformed_ranges={}",
        s.documents,
        s.occurrences,
        s.symbols,
        s.external_symbols,
        s.unknown_fields,
        s.malformed_ranges
    )
}

/// Adapts the wire decoder's output to the importer: the first metadata
/// creates it, each document goes straight in.
struct ImportSink<'a> {
    repo: &'a Path,
    prefix: Option<String>,
    importer: Option<ScipImporter>,
    /// `metadata` fields after the first, ignored.
    extra_metadata: usize,
}

impl IndexSink for ImportSink<'_> {
    fn metadata(&mut self, m: WireMetadata) -> Result<(), String> {
        if self.importer.is_some() {
            self.extra_metadata += 1;
            return Ok(());
        }
        if m.text_document_encoding == TEXT_ENCODING_UTF16 {
            eprintln!(
                "[scip] warning: the index declares UTF-16 source files; a document whose file is not UTF-8 is skipped"
            );
        }
        let info = ScipIndexInfo {
            tool: m.tool_name,
            tool_version: m.tool_version,
            project_root: m.project_root,
        };
        let mut o = ScipImportOptions::default();
        o.prefix = self.prefix.take();
        o.surface = "cli";
        self.importer = Some(ScipImporter::new(self.repo, info, o)?);
        Ok(())
    }

    fn document(&mut self, d: WireDocument) -> Result<(), String> {
        let importer = self
            .importer
            .as_mut()
            .ok_or("metadata must come first: the index has a document before its metadata")?;
        importer.document(document_in(d));
        Ok(())
    }
}

fn document_in(d: WireDocument) -> ScipDocumentIn {
    ScipDocumentIn {
        relative_path: d.relative_path,
        language: d.language,
        encoding: PositionEncoding::from_proto(d.position_encoding),
        occurrences: d
            .occurrences
            .into_iter()
            .map(|o| ScipOccurrenceIn {
                start_line: o.sl,
                start_char: o.sc,
                end_line: o.el,
                end_char: o.ec,
                symbol: o.symbol,
                roles: o.roles,
            })
            .collect(),
        symbols: d
            .symbols
            .into_iter()
            .map(|s| ScipSymbolIn {
                symbol: s.symbol,
                implements: s.implements,
            })
            .collect(),
    }
}
