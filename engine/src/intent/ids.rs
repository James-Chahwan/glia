//! The repo's id vocabulary (`[ids]`, EB.4): the identifier shapes a repo's
//! PR titles, commit subjects and task docs use, so a text's ids can be told
//! from its prose. Read from `.glia/overlay.toml [ids]` (user config like
//! `[walk]`: query-time, applied under `--no-overlay`, never an overlay-loop
//! candidate, never in a graph), else the defaults.
//!
//! The shape grammar (`{U}`, `{N}`, `{N<k>}`, `{l?}`, literals) and its
//! matcher live once, in `glia_code_domain::glia_config::IdPattern`, so the
//! loader and this reader never disagree; this module only reads the config
//! and resolves overlapping matches across shapes.
//!
//! Marker (stderr, once per [`IdVocab::report`]):
//! `[ids] vocab repo=<label> surface=<s> patterns=<n> source=<config|default|off>
//! trackers=<t> rejected=<r> markdown=<m>`.

use std::path::Path;

use glia_code_domain::glia_config::{self, IdMatch, IdPattern, LoadedConfig, PathGlobs, TrackerDecl};

/// The id shapes a repo with no `[ids] patterns` uses: tickets and spec-kit
/// requirement / success ids (`LANG-1234`, `FR-001`), spec-kit task ids (`T012`).
pub const DEFAULT_PATTERNS: [&str; 2] = ["{U}-{N}", "T{N3}"];

/// The findings words a repo with no `[ids] local_words` uses.
pub const DEFAULT_LOCAL_WORDS: [&str; 5] = ["analysis", "finding", "remediation", "review", "audit"];

/// Where a vocabulary's patterns came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VocabSource {
    /// `[ids] patterns` set to at least one valid shape.
    Config,
    /// No `[ids] patterns`: [`DEFAULT_PATTERNS`].
    Default,
    /// `[ids] patterns` set but empty (or every shape dropped): id routing off.
    Off,
}

impl VocabSource {
    /// The marker's `source=` value.
    pub fn as_str(self) -> &'static str {
        match self {
            VocabSource::Config => "config",
            VocabSource::Default => "default",
            VocabSource::Off => "off",
        }
    }
}

/// One id a text names.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct IdMention {
    pub id: String,
    /// The matched text before the digit group of the shape's first `{N}`:
    /// `T` (`T012`), `FR-` (`FR-001`), `DT.` (`DT.5h`), `E0.` (`E0.1`).
    pub family: String,
    /// Byte offsets into the text. A letter-shorthand mention (`b` of
    /// `X.5a/b`) sits at the shorthand's bytes.
    pub start: usize,
    pub end: usize,
}

/// A repo's id vocabulary.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IdVocab {
    /// The shapes, in config (or default) order: the overlap tie-break.
    pub patterns: Vec<IdPattern>,
    /// The findings words.
    pub local_words: Vec<String>,
    /// `[[ids.tracker]]` JSON trackers.
    pub trackers: Vec<TrackerDecl>,
    /// `[ids] markdown` tracker globs (EB-5).
    pub markdown: Vec<String>,
    pub source: VocabSource,
    /// `[ids]` entries the loader dropped.
    pub rejected: usize,
}

impl Default for IdVocab {
    /// The defaults: [`DEFAULT_PATTERNS`], [`DEFAULT_LOCAL_WORDS`], no trackers.
    fn default() -> Self {
        IdVocab {
            patterns: DEFAULT_PATTERNS.iter().filter_map(|p| IdPattern::parse(p).ok()).collect(),
            local_words: DEFAULT_LOCAL_WORDS.iter().map(|w| (*w).to_string()).collect(),
            trackers: Vec::new(),
            markdown: Vec::new(),
            source: VocabSource::Default,
            rejected: 0,
        }
    }
}

impl IdVocab {
    /// The vocabulary `<root>/.glia/overlay.toml` declares, else the defaults.
    pub fn for_root(root: &Path) -> IdVocab {
        IdVocab::from_config(glia_config::load(root).as_ref())
    }

    /// The vocabulary a loaded config declares (`None`: the defaults). Set
    /// `patterns` / `local_words` REPLACE the defaults; `patterns = []` is off.
    pub fn from_config(loaded: Option<&LoadedConfig>) -> IdVocab {
        let mut v = IdVocab::default();
        let Some(loaded) = loaded else {
            return v;
        };
        let ids = &loaded.config.ids;
        if let Some(patterns) = &ids.patterns {
            v.patterns = patterns.iter().filter_map(|p| IdPattern::parse(p.get_ref()).ok()).collect();
            v.source = if v.patterns.is_empty() { VocabSource::Off } else { VocabSource::Config };
        }
        if let Some(words) = &ids.local_words {
            v.local_words = words.iter().map(|w| w.get_ref().clone()).collect();
        }
        v.trackers = ids.tracker.iter().map(|t| t.get_ref().clone()).collect();
        v.markdown = ids.markdown_patterns();
        v.rejected = loaded.ids_rejected();
        v
    }

    /// A vocabulary of exactly these shapes (default words, no trackers);
    /// the first shape that does not parse is the error.
    pub fn from_patterns(patterns: &[&str]) -> Result<IdVocab, String> {
        let parsed = patterns
            .iter()
            .map(|p| IdPattern::parse(p).map_err(|e| format!("{p:?}: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
        let source = if parsed.is_empty() { VocabSource::Off } else { VocabSource::Config };
        Ok(IdVocab { patterns: parsed, source, ..IdVocab::default() })
    }

    /// The `[ids] markdown` matcher, `None` when none is declared.
    pub fn markdown_globs(&self) -> Option<PathGlobs> {
        PathGlobs::new(&self.markdown)
    }

    /// The path matcher of tracker `i`, `None` past the list.
    pub fn tracker_globs(&self, i: usize) -> Option<PathGlobs> {
        PathGlobs::new(std::slice::from_ref(&self.trackers.get(i)?.path))
    }

    /// True when `word` (already lowercased) is a findings word.
    pub fn is_local_word(&self, word: &str) -> bool {
        self.local_words.iter().any(|w| w == word)
    }

    /// Every id `text` names: every shape's matches, overlapping ones
    /// resolved longest claimed span first, then earliest start, then shape
    /// order; a kept match's letter-shorthand ids follow it. Sorted by
    /// (start, end, id), exact duplicates removed.
    pub fn mentions(&self, text: &str) -> Vec<IdMention> {
        let mut found: Vec<(usize, IdMatch)> = Vec::new();
        for (i, p) in self.patterns.iter().enumerate() {
            found.extend(p.find_all(text).into_iter().map(|m| (i, m)));
        }
        found.sort_by(|(ia, a), (ib, b)| {
            (b.claim_end - b.start)
                .cmp(&(a.claim_end - a.start))
                .then(a.start.cmp(&b.start))
                .then(ia.cmp(ib))
        });
        let mut kept: Vec<IdMatch> = Vec::new();
        for (_, m) in found {
            if kept.iter().all(|k| m.claim_end <= k.start || k.claim_end <= m.start) {
                kept.push(m);
            }
        }
        let mut out = Vec::new();
        for m in kept {
            let (Some(id), Some(family)) = (text.get(m.start..m.end), text.get(m.start..m.family_end)) else {
                continue;
            };
            out.push(IdMention { id: id.to_string(), family: family.to_string(), start: m.start, end: m.end });
            for (extra, s, e) in &m.extra {
                out.push(IdMention { id: extra.clone(), family: family.to_string(), start: *s, end: *e });
            }
        }
        out.sort_by(|a, b| (a.start, a.end, &a.id).cmp(&(b.start, b.end, &b.id)));
        out.dedup();
        out
    }

    /// Print the `[ids] vocab` marker for repo `label` on `surface`.
    pub fn report(&self, label: &str, surface: &str) {
        eprintln!(
            "[ids] vocab repo={label} surface={surface} patterns={} source={} trackers={} rejected={} markdown={}",
            self.patterns.len(),
            self.source.as_str(),
            self.trackers.len(),
            self.rejected,
            self.markdown.len()
        );
    }
}
