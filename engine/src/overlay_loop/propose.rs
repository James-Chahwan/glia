//! The overlay loop's work list (CE.3d): every gap an overlay stanza could
//! close, with its id and the source around it, for the model step.
//!
//! [`propose`] builds the tree as `glia gaps` does (nothing persisted, no
//! parse cache written), runs [`gaps_report`] with every repo root, keeps the
//! chosen categories (by default every one whose `suggest` is not `none`,
//! i.e. all but `wrapped_sink`), cuts each to `top_k` rows (the counts stay
//! the totals) and attaches to each row a [`Snippet`]: lines `[line - k, line
//! + k]` (1-based, clamped to the file) of the row's file.
//!
//! A [`GapRow`] carries no repo, so the snippet is read from the one root of
//! `repo_paths` that holds the row's file: when two roots hold it the snippet
//! is omitted and counted `ambiguous_root`, never guessed. Only a relative
//! path with no `..` that stays inside its root (symlinks resolved) is read;
//! a file over 2 MiB is skipped; each line is cut at 240 chars. A rule row's
//! `.glia/overlay.toml` is read like any other file. The snippet is the
//! user's own source (what `read` shows); nothing is redacted or stored.
//!
//! Marker (the fired_on line), once per call:
//! `[overlay] propose repo=<primary> rows=<n> snippets=<s> ambiguous_root=<a> surface=<cli|py|engine>`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::build::{
    BuildOptions, GenerateResult, generate_many_opts, generate_one_with_cache_opts,
};
use crate::cache::ParseCache;
use crate::gaps::{CATEGORIES, GapRow, GapsOptions, WRAPPED_SINK, gaps_report};

/// Rows kept per category unless [`ProposeOptions::top_k`] says otherwise.
pub const DEFAULT_TOP_K: usize = 20;
/// Lines of context either side of a row's line.
pub const DEFAULT_SNIPPET_LINES: usize = 3;
/// A file larger than this is never read for a snippet.
const MAX_SNIPPET_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// A snippet line longer than this is cut (at a char boundary).
const MAX_LINE_CHARS: usize = 240;
/// The categories whose rows suggest `none` (the gaps module table): not a
/// repair an overlay makes, so not proposed by default.
const NO_REPAIR: [&str; 1] = [WRAPPED_SINK];

/// Where each `suggest` value is documented; the model step's instructions.
const GUIDE: &str = "Each row's `suggest` names what could close it (docs/overlay.md): \
`constants` pins a `${...}` base in [constants] and `route_prefix` mounts a project's routes \
under a gateway prefix in [[route_prefix]] (section \"Example\"); `wrapper` declares the \
project's own request / publish / collection helper so its call sites mint the sink \
(section \"[[wrapper]]: call sites of a project's own helpers\"); `edge` declares one \
[[edge]] between two qnames read from the code - a suspected_edge row's `draft` is a \
paste-ready stanza (section \"Example\"); `entrypoints` lists a symbol called from outside \
the build in [entrypoints] (section \"Example\"); `remove` marks an overlay rule that binds \
nothing or that the extractor now emits: remove it with `accept` and its gap id (section \
\"The loop: gaps -> overlay -> overlay delta\"); `glia cell ls --check --rekey` repairs a \
sidecar row, not the overlay. Write each stanza under a `# gap: <id>` comment naming its row, \
keep the file loadable (section \"Validation\": a syntax error or an unknown key ignores the \
whole file), try the candidate, and accept only what the verdict keeps.";

/// How [`propose`] runs. Made with `Default` plus field assignment.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposeOptions {
    /// Gap categories to list, each one of `gaps::CATEGORIES`; empty = the
    /// default: every category whose `suggest` is not `none` (all but
    /// `wrapped_sink`).
    pub categories: Vec<String>,
    /// Rows kept per category after ranking (default [`DEFAULT_TOP_K`]);
    /// [`Proposal::counts`] stay the totals.
    pub top_k: usize,
    /// Lines of context either side of a row's line (default
    /// [`DEFAULT_SNIPPET_LINES`]).
    pub snippet_lines: usize,
    /// The `surface=` of the `[overlay] propose` and `[gaps]` markers
    /// (`cli`, `py`); empty = `engine`.
    pub surface: &'static str,
}

impl Default for ProposeOptions {
    fn default() -> Self {
        Self {
            categories: Vec::new(),
            top_k: DEFAULT_TOP_K,
            snippet_lines: DEFAULT_SNIPPET_LINES,
            surface: "",
        }
    }
}

/// Source lines around a row's line, from the one root that holds its file.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Snippet {
    /// The row's file, repo-relative.
    pub file: String,
    /// 1-based line of `lines[0]`.
    pub start_line: usize,
    /// The lines, each cut at 240 chars.
    pub lines: Vec<String>,
}

/// One gap of the work list.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ProposedGap {
    /// The `glia gaps` row: its `id` is what a candidate's `# gap:` comment
    /// and `accept`'s removal name; a `suspected_edge` row's `draft` is a
    /// paste-ready `[[edge]]`. Its fields are the JSON row's.
    #[serde(flatten)]
    pub gap: GapRow,
    /// `None` when the row has no file or line, no root or two roots hold
    /// its file, the file is over 2 MiB, unreadable or shorter than `line`.
    pub snippet: Option<Snippet>,
}

/// [`propose`]'s answer.
#[non_exhaustive]
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// Per chosen category in report order, at most `top_k` rows each.
    pub rows: Vec<ProposedGap>,
    /// Per chosen category, its total before the `top_k` cut.
    pub counts: BTreeMap<&'static str, usize>,
    /// Rows that carry a snippet.
    pub snippets: usize,
    /// Rows whose file two or more roots hold: snippet omitted.
    pub ambiguous_root: usize,
    /// Where docs/overlay.md documents each `suggest` value.
    pub guide: &'static str,
}

/// The gap work list of `repo_paths` (the first is the primary repo; the
/// rest merge in): see the module doc. `Err` when there is no path, a
/// category is unknown, or the build fails.
pub fn propose(repo_paths: &[String], opts: &ProposeOptions) -> Result<Proposal, String> {
    let primary = repo_paths
        .first()
        .ok_or_else(|| "overlay propose: no repo paths".to_string())?;
    let chosen = chosen_categories(&opts.categories)?;
    let surface = if opts.surface.is_empty() {
        "engine"
    } else {
        opts.surface
    };
    let built = build_quiet(repo_paths)?;
    let gaps_opts = GapsOptions {
        top_k_per_category: Some(opts.top_k),
        surface,
        ..GapsOptions::default()
    };
    let report = gaps_report(&built.merged, &repo_roots(&built), &gaps_opts)?;

    let counts: BTreeMap<&'static str, usize> = report
        .counts
        .iter()
        .filter(|(c, _)| chosen.contains(*c))
        .map(|(c, n)| (*c, *n))
        .collect();
    let mut reader = SnippetReader::new(repo_paths, opts.snippet_lines);
    let rows: Vec<ProposedGap> = report
        .rows
        .into_iter()
        .filter(|r| chosen.contains(&r.category))
        .map(|gap| {
            let snippet = reader.snippet(&gap);
            ProposedGap { gap, snippet }
        })
        .collect();
    let snippets = rows.iter().filter(|r| r.snippet.is_some()).count();
    eprintln!(
        "[overlay] propose repo={primary} rows={} snippets={snippets} ambiguous_root={} surface={surface}",
        rows.len(),
        reader.ambiguous
    );
    Ok(Proposal {
        rows,
        counts,
        snippets,
        ambiguous_root: reader.ambiguous,
        guide: GUIDE,
    })
}

/// `categories` checked against `gaps::CATEGORIES`, in report order; empty
/// is every category but [`NO_REPAIR`].
fn chosen_categories(categories: &[String]) -> Result<Vec<&'static str>, String> {
    if let Some(c) = categories
        .iter()
        .find(|c| !CATEGORIES.contains(&c.as_str()))
    {
        return Err(format!(
            "unknown gaps category `{c}` (one of: {})",
            CATEGORIES.join(", ")
        ));
    }
    Ok(CATEGORIES
        .iter()
        .copied()
        .filter(|c| {
            if categories.is_empty() {
                !NO_REPAIR.contains(c)
            } else {
                categories.iter().any(|x| x == c)
            }
        })
        .collect())
}

/// Build `repo_paths` without writing anything: one repo on its parse cache
/// loaded read-only (never saved), several through a clean
/// `generate_many_opts`. The overlay applied is the file on disk.
pub(super) fn build_quiet(repo_paths: &[String]) -> Result<GenerateResult, String> {
    let opts = BuildOptions::default();
    match repo_paths {
        [one] => {
            let mut cache = ParseCache::load(one);
            generate_one_with_cache_opts(one, &mut cache, &opts)
        }
        many => generate_many_opts(many, false, &opts),
    }
}

/// A build's `(RepoId.0, root)` pairs, what [`gaps_report`] reads files under.
pub(super) fn repo_roots(built: &GenerateResult) -> Vec<(u64, PathBuf)> {
    built
        .repo_roots
        .iter()
        .map(|(id, p)| (*id, PathBuf::from(p)))
        .collect()
}

/// Reads snippets, each file at most once.
struct SnippetReader {
    /// Each root as given, and its canonical form (`None` when it has none).
    roots: Vec<(PathBuf, Option<PathBuf>)>,
    context: usize,
    /// The lines of each file read (`None`: skipped or unreadable).
    files: BTreeMap<PathBuf, Option<Vec<String>>>,
    ambiguous: usize,
}

impl SnippetReader {
    fn new(repo_paths: &[String], context: usize) -> Self {
        let roots = repo_paths
            .iter()
            .map(|p| {
                let root = PathBuf::from(p);
                let canonical = root.canonicalize().ok();
                (root, canonical)
            })
            .collect();
        Self {
            roots,
            context,
            files: BTreeMap::new(),
            ambiguous: 0,
        }
    }

    fn snippet(&mut self, row: &GapRow) -> Option<Snippet> {
        let (file, line) = (row.file.as_deref()?, row.line?);
        let line = usize::try_from(line).ok().filter(|l| *l >= 1)?;
        let rel = Path::new(file);
        let plain = rel
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if !plain {
            return None;
        }
        let holders: Vec<PathBuf> = self
            .roots
            .iter()
            .filter_map(|(root, canonical)| inside(root, canonical.as_deref(), rel))
            .collect();
        let path = match holders.as_slice() {
            [one] => one.clone(),
            [] => return None,
            _ => {
                self.ambiguous += 1;
                return None;
            }
        };
        let lines = self
            .files
            .entry(path)
            .or_insert_with_key(|p| read_lines(p))
            .as_deref()?;
        if line > lines.len() {
            return None;
        }
        let start = line.saturating_sub(self.context).max(1);
        let end = line.saturating_add(self.context).min(lines.len());
        Some(Snippet {
            file: file.to_string(),
            start_line: start,
            lines: lines[start - 1..end].iter().map(|l| cap(l)).collect(),
        })
    }
}

/// `root/rel` when it is a regular file whose real path stays under the
/// root's real path.
fn inside(root: &Path, canonical_root: Option<&Path>, rel: &Path) -> Option<PathBuf> {
    let path = root.join(rel);
    if !path.is_file() {
        return None;
    }
    let real = path.canonicalize().ok()?;
    real.starts_with(canonical_root?).then_some(path)
}

/// The file's lines (lossy UTF-8), or `None` when it is over
/// [`MAX_SNIPPET_FILE_BYTES`] or unreadable.
fn read_lines(path: &Path) -> Option<Vec<String>> {
    let len = std::fs::metadata(path).ok()?.len();
    if len > MAX_SNIPPET_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(
        String::from_utf8_lossy(&bytes)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

/// `line` cut at [`MAX_LINE_CHARS`] chars.
fn cap(line: &str) -> String {
    match line.char_indices().nth(MAX_LINE_CHARS) {
        Some((at, _)) => line[..at].to_string(),
        None => line.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cap_cuts_at_a_char_boundary() {
        let long = "é".repeat(MAX_LINE_CHARS + 5);
        let cut = cap(&long);
        assert_eq!(cut.chars().count(), MAX_LINE_CHARS);
        assert_eq!(cap("short"), "short");
    }

    #[test]
    fn default_categories_leave_out_none_suggestions() {
        let all = chosen_categories(&[]).expect("default");
        assert_eq!(all.len(), CATEGORIES.len() - NO_REPAIR.len());
        assert!(!all.contains(&WRAPPED_SINK));
        let err = chosen_categories(&["nope".to_string()]).expect_err("unknown");
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn snippet_reads_only_inside_one_root() {
        let base = std::env::temp_dir().join(format!("glia_ce3d_snip_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (a, b) = (base.join("a"), base.join("b"));
        std::fs::create_dir_all(a.join("src")).expect("mkdir");
        std::fs::create_dir_all(&b).expect("mkdir");
        let body: String = (1..=10).map(|i| format!("line {i}\n")).collect();
        std::fs::write(a.join("src/x.ts"), &body).expect("write");
        std::fs::write(base.join("outside.ts"), &body).expect("write");
        let paths = [
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned(),
        ];
        let mut r = SnippetReader::new(&paths, 2);
        let row = |file: &str, line: i64| GapRow {
            id: "gap:0000000000000000".into(),
            category: "dead_symbol",
            qname: "q".into(),
            kind: "FUNCTION",
            file: Some(file.into()),
            line: Some(line),
            detail: String::new(),
            suggest: "entrypoints",
            tier: "heuristic",
            draft: None,
        };
        let s = r.snippet(&row("src/x.ts", 10)).expect("one root");
        assert_eq!((s.start_line, s.lines.len()), (8, 3), "clamped at the end");
        assert_eq!(s.lines[2], "line 10");
        assert!(r.snippet(&row("src/x.ts", 11)).is_none(), "past the end");
        assert!(
            r.snippet(&row("../outside.ts", 1)).is_none(),
            "leaves the root"
        );
        assert!(r.snippet(&row("src/missing.ts", 1)).is_none());
        std::fs::create_dir_all(b.join("src")).expect("mkdir");
        std::fs::write(b.join("src/x.ts"), &body).expect("write");
        let mut r = SnippetReader::new(&paths, 2);
        assert!(
            r.snippet(&row("src/x.ts", 3)).is_none(),
            "two roots hold it"
        );
        assert_eq!(r.ambiguous, 1);
        std::fs::remove_dir_all(&base).ok();
    }
}
