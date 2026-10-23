//! BM25 text ranking over the history snapshot's past commit messages: a
//! query text scores the files the matching commits touched, indexed through
//! the one candidate file space (`rank::FileIndex`). Owner: EB.3.
//!
//! This is rank_eval.rs's T1s, reproduced to the bit: [`tokens`] and
//! [`subject`] are the harness's functions, [`TextIndex::scores`] its
//! `Bm25::scores` then `Bm25::file_scores` (BM25 k1 1.2, b 0.75, the unique
//! query tokens in sorted order, the top 20 matching commits by score then
//! newest first, each adding its score to each of its files in that order).
//! Digit tokens are kept (round 3: dropping them is the measured loss).
//!
//! The candidate files are one repo's slice of [`FileIndex`] ([`Candidates`]);
//! a commit's files map through `HistoryFile.p` with NO rename fold (the
//! harness's `git log --no-renames`), so a commit made under a file's old
//! name credits nothing. A commit trains (is a document) iff it maps
//! 1..=30 candidate files, as rank_eval's `train_ok`.
//!
//! Query-time only: the index is read from `.glia/history-snapshot/`
//! (`glia history sync`, its messages sidecar since EB.1), never built into
//! a `.gmap`. A caller ranking a change excludes the change's own commits
//! ([`exclude_range`], `git rev-list --no-merges <base>..<head>`), so a
//! review never reads the PR it ranks (rank_eval reads strictly older
//! commits).
//!
//! Fired_on, stderr: `[intent] text repo=<label> surface=<s> docs=<d>
//! skipped_wide=<w> excluded=<e> tokens=<q> matched_docs=<m> files=<f>`
//! ([`TextIndex::report`]), or `[intent] text repo=<label> surface=<s>
//! history=absent` / `messages=absent` ([`TextAbsent::report`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use glia_code_domain::snapshots::{HistoryMessage, HistorySnapshot, read_history, read_history_messages};

use crate::rank::FileIndex;

/// rank_eval's `MAX_FILES`: a commit of more candidate files is no document.
const MAX_DOC_FILES: usize = 30;
/// rank_eval's `TOP_TASKS`: the matching commits that score files.
const TOP_DOCS: usize = 20;
/// rank_eval's `BM25_K1`.
const BM25_K1: f64 = 1.2;
/// rank_eval's `BM25_B`.
const BM25_B: f64 = 0.75;

/// rank_eval.rs `STOP`, verbatim and in its order.
const STOP: &[&str] = &[
    "about", "above", "after", "again", "against", "all", "also", "and", "any", "are", "because", "been",
    "before", "being", "below", "between", "both", "but", "can", "could", "did", "does", "doing", "down",
    "during", "each", "few", "for", "from", "further", "had", "has", "have", "having", "her", "here", "hers",
    "him", "his", "how", "into", "its", "itself", "just", "more", "most", "not", "now", "off", "once", "only",
    "other", "our", "ours", "out", "over", "own", "same", "she", "should", "some", "such", "than", "that",
    "the", "their", "them", "then", "there", "these", "they", "this", "those", "through", "too", "under",
    "until", "very", "was", "were", "what", "when", "where", "which", "while", "who", "whom", "why", "will",
    "with", "would", "you", "your", "yours", "yet", "via", "per", "one", "two", "new", "use", "used",
    "uses", "using", "get", "got", "let", "may", "might", "must", "shall", "upon", "within", "without",
    "onto", "etc",
];

/// rank_eval's `tokens`: per line, `trim_start` then ASCII-lowercase (a
/// non-ASCII capital survives); a line starting `co-authored-by:`,
/// `claude-session:` or `signed-off-by:` is skipped; the line splits on every
/// non-alphanumeric char and keeps tokens of at least 3 chars not in the
/// stop list. Digit tokens are kept.
pub fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim_start().to_ascii_lowercase();
        if l.starts_with("co-authored-by:") || l.starts_with("claude-session:") || l.starts_with("signed-off-by:") {
            continue;
        }
        for t in l.split(|c: char| !c.is_alphanumeric()) {
            if t.chars().count() >= 3 && !STOP.contains(&t) {
                out.push(t.to_string());
            }
        }
    }
    out
}

/// rank_eval's `subject`: the first non-empty trimmed line, else `""`.
pub fn subject(text: &str) -> &str {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

/// The candidate files of one repo: its slice of a [`FileIndex`], in index
/// order (paths ascending), the tie order every fusion uses.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct Candidates {
    /// The repo's candidate paths, ascending.
    pub paths: Vec<String>,
    /// Per path, [`FileIndex::degree`]: every edge endpoint on its nodes.
    pub deg: Vec<f64>,
    /// Per path, its index in the [`FileIndex`].
    pub files: Vec<u32>,
}

impl Candidates {
    /// Repo `repo`'s candidates in `files`.
    pub fn of(files: &FileIndex, repo: u64) -> Candidates {
        let mut out = Candidates::default();
        for f in 0..files.len() as u32 {
            if files.repo(f) == repo {
                out.paths.push(files.path(f).to_string());
                out.deg.push(files.degree(f));
                out.files.push(f);
            }
        }
        out
    }

    /// `path`'s position in [`Candidates::paths`].
    pub fn position(&self, path: &str) -> Option<usize> {
        self.paths.binary_search_by(|p| p.as_str().cmp(path)).ok()
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

/// What a [`TextIndex`] holds and left out.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextStats {
    /// Commits indexed (1..=30 candidate files).
    pub docs: usize,
    /// Commits of more than 30 candidate files.
    pub skipped_wide: usize,
    /// Commits of no candidate file.
    pub skipped_empty: usize,
    /// Commits left out by sha (the change being ranked).
    pub excluded: usize,
}

/// Why no [`TextIndex`] could be read.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAbsent {
    /// No complete `.glia/history-snapshot/`.
    History,
    /// A snapshot with no (or an incomplete) messages sidecar.
    Messages,
}

impl TextAbsent {
    /// The marker's absent form: `history=absent` / `messages=absent`.
    pub fn as_str(self) -> &'static str {
        match self {
            TextAbsent::History => "history=absent",
            TextAbsent::Messages => "messages=absent",
        }
    }

    /// Print `[intent] text repo=<label> surface=<s> <history|messages>=absent`.
    pub fn report(self, label: &str, surface: &str) {
        eprintln!("[intent] text repo={label} surface={surface} {}", self.as_str());
    }
}

/// BM25 over past commit messages, documents oldest first, each scoring the
/// candidate files its commit touched.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct TextIndex {
    post: BTreeMap<String, Vec<(u32, u32)>>,
    dl: Vec<u32>,
    total: u64,
    doc_files: Vec<Vec<u32>>,
    doc_sha: Vec<String>,
    n_files: usize,
    stats: TextStats,
}

impl TextIndex {
    /// An index over `docs`, OLDEST first: `(sha, message text, candidate
    /// indices)`. A document of 0 or more than 30 distinct candidates is
    /// skipped and counted. `n_files` is the candidate count the scores
    /// cover (`Candidates::len`).
    pub fn from_docs<'a>(n_files: usize, docs: impl Iterator<Item = (&'a str, &'a str, Vec<u32>)>) -> TextIndex {
        let mut idx = TextIndex { n_files, ..TextIndex::default() };
        for (sha, text, mut files) in docs {
            files.sort_unstable();
            files.dedup();
            if files.is_empty() {
                idx.stats.skipped_empty += 1;
                continue;
            }
            if files.len() > MAX_DOC_FILES {
                idx.stats.skipped_wide += 1;
                continue;
            }
            idx.add(sha, tokens(text), files);
        }
        idx.stats.docs = idx.dl.len();
        idx
    }

    /// rank_eval's `Bm25::add`.
    fn add(&mut self, sha: &str, toks: Vec<String>, files: Vec<u32>) {
        let d = self.dl.len() as u32;
        let mut tf: BTreeMap<&str, u32> = BTreeMap::new();
        for t in &toks {
            *tf.entry(t.as_str()).or_default() += 1;
        }
        for (t, c) in tf {
            self.post.entry(t.to_string()).or_default().push((d, c));
        }
        self.dl.push(toks.len() as u32);
        self.total += toks.len() as u64;
        self.doc_files.push(files);
        self.doc_sha.push(sha.to_string());
    }

    /// The index of a history snapshot and its messages (row i of
    /// `messages` belongs to `snapshot.commits[i]`): the commits oldest first
    /// (the snapshot lists them newest first), each commit whose sha is in
    /// `exclude` left out, each one's files mapped by `HistoryFile.p` (no
    /// rename fold), its text the subject, `\n`, the body.
    pub fn build(
        snapshot: &HistorySnapshot,
        messages: &[HistoryMessage],
        cands: &Candidates,
        exclude: &BTreeSet<String>,
    ) -> TextIndex {
        let mut excluded = 0usize;
        let mut docs: Vec<(&str, String, Vec<u32>)> = Vec::new();
        for (c, m) in snapshot.commits.iter().zip(messages).rev() {
            if exclude.contains(&c.c) {
                excluded += 1;
                continue;
            }
            let files: Vec<u32> = c.files.iter().filter_map(|f| cands.position(&f.p)).map(|i| i as u32).collect();
            docs.push((c.c.as_str(), format!("{}\n{}", m.s, m.b), files));
        }
        let mut idx = TextIndex::from_docs(cands.len(), docs.iter().map(|(s, t, f)| (*s, t.as_str(), f.clone())));
        idx.stats.excluded = excluded;
        idx
    }

    /// The index under `<root>/.glia/history-snapshot/` over `cands`, or why
    /// there is none.
    pub fn load(root: &Path, cands: &Candidates, exclude: &BTreeSet<String>) -> Result<TextIndex, TextAbsent> {
        let snap = read_history(root).ok_or(TextAbsent::History)?;
        let messages = read_history_messages(root, &snap).ok_or(TextAbsent::Messages)?;
        Ok(TextIndex::build(&snap, &messages, cands, exclude))
    }

    /// rank_eval's `Bm25::scores` of `query`'s tokens: one score per document.
    fn doc_scores(&self, query: &str) -> Vec<f64> {
        let n = self.dl.len();
        let mut s = vec![0.0; n];
        if n == 0 {
            return s;
        }
        let q = tokens(query);
        let avg = (self.total as f64 / n as f64).max(1e-9);
        let uniq: BTreeSet<&String> = q.iter().collect();
        for t in uniq {
            let Some(pl) = self.post.get(t) else { continue };
            let df = pl.len() as f64;
            let idf = (1.0 + (n as f64 - df + 0.5) / (df + 0.5)).ln();
            for &(d, tf) in pl {
                let tf = f64::from(tf);
                let dl = f64::from(self.dl[d as usize]);
                s[d as usize] += idf * tf * (BM25_K1 + 1.0) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / avg));
            }
        }
        s
    }

    /// The top documents of `s`: score > 0, by score descending, then the
    /// newer (later) document first, at most 20.
    fn top(s: &[f64]) -> Vec<(usize, f64)> {
        let mut top: Vec<(usize, f64)> = s.iter().copied().enumerate().filter(|&(_, v)| v > 0.0).collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
        top.truncate(TOP_DOCS);
        top
    }

    /// One score per candidate: each of `query`'s top documents, in top
    /// order, adds its score to each of its files (rank_eval's
    /// `file_scores`). All zeros with no document.
    pub fn scores(&self, query: &str) -> Vec<f64> {
        let mut acc = vec![0.0f64; self.n_files];
        for (d, v) in TextIndex::top(&self.doc_scores(query)) {
            for &f in &self.doc_files[d] {
                if let Some(slot) = acc.get_mut(f as usize) {
                    *slot += v;
                }
            }
        }
        acc
    }

    /// The documents `query` matched (score > 0), uncapped.
    pub fn matched_docs(&self, query: &str) -> usize {
        self.doc_scores(query).iter().filter(|&&v| v > 0.0).count()
    }

    /// `query`'s top documents as (commit sha, score), best first: the
    /// commits [`TextIndex::scores`] read.
    pub fn top_commits(&self, query: &str) -> Vec<(&str, f64)> {
        TextIndex::top(&self.doc_scores(query)).into_iter().map(|(d, v)| (self.doc_sha[d].as_str(), v)).collect()
    }

    pub fn stats(&self) -> TextStats {
        self.stats
    }

    /// Print the fired_on line for one query: `[intent] text repo=<label>
    /// surface=<s> docs=<d> skipped_wide=<w> excluded=<e> tokens=<q>
    /// matched_docs=<m> files=<f>`, `q` the query's distinct tokens, `f` the
    /// candidates it scores above 0.
    pub fn report(&self, label: &str, surface: &str, query: &str) {
        let q = tokens(query).into_iter().collect::<BTreeSet<String>>().len();
        let files = self.scores(query).iter().filter(|&&v| v > 0.0).count();
        eprintln!(
            "[intent] text repo={label} surface={surface} docs={} skipped_wide={} excluded={} tokens={q} matched_docs={} files={files}",
            self.stats.docs,
            self.stats.skipped_wide,
            self.stats.excluded,
            self.matched_docs(query),
        );
    }
}

/// The commits of `base..head` in the git repository `repo` sits in (`git
/// rev-list --no-merges`, full shas): the change a review ranks, to pass as
/// `exclude` so its own messages score nothing.
pub fn exclude_range(repo: &Path, base: &str, head: &str) -> Result<BTreeSet<String>, String> {
    crate::git_rev::rev_list(repo, base, head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_the_harness_rule() {
        let t = tokens("Fix CODEC-42 in the Über parser\nCo-Authored-By: x <y>\n  Signed-off-by: z\nab cd_ef");
        assert_eq!(t, ["fix", "codec", "Über", "parser"]);
        assert_eq!(subject("\n  \n  Hello world \nbody"), "Hello world");
        assert_eq!(subject(""), "");
    }

    #[test]
    fn top_docs_tie_to_the_newer() {
        let idx = TextIndex::from_docs(
            3,
            [("a", "codec alpha", vec![0]), ("b", "codec alpha", vec![1]), ("c", "nothing", vec![2])].into_iter(),
        );
        let top = idx.top_commits("codec");
        assert_eq!(top.iter().map(|x| x.0).collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(top[0].1.to_bits(), top[1].1.to_bits());
        assert_eq!(TextIndex::from_docs(2, std::iter::empty()).scores("codec"), [0.0, 0.0]);
    }
}
