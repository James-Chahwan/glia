//! Title filter for `glia docs sync`.
//!
//! A Confluence space is mostly meeting notes and onboarding pages; ingesting
//! all of them buries the pages that actually document code under doc nodes
//! that document nothing. This is the (pure, offline) matcher that lets a sync
//! be scoped to the titles that matter.
//!
//! Filtering happens on the `Vec<Page>` that `confluence_rest::pull_space`
//! returns, *after* the fetch — the content endpoint has no title-glob
//! parameter, and keeping one network code path is worth more than the saved
//! bandwidth.

/// Title filter for `glia docs sync`. Patterns are case-insensitive; `*` is the
/// only metacharacter (it matches any run of characters). A pattern without `*`
/// is a plain substring test, so `--include payments` works unadorned.
pub struct TitleFilter {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl TitleFilter {
    /// Build a filter from the raw flag values. Both lists are lowercased once
    /// here so `keep` only lowercases the title.
    pub fn new(include: &[String], exclude: &[String]) -> Self {
        Self {
            include: include.iter().map(|p| p.to_lowercase()).collect(),
            exclude: exclude.iter().map(|p| p.to_lowercase()).collect(),
        }
    }

    /// `true` = keep the page. An empty include list keeps everything that is
    /// not excluded; any exclude hit wins over every include hit.
    pub fn keep(&self, title: &str) -> bool {
        let hay = title.to_lowercase();
        if self.exclude.iter().any(|p| glob_match(p, &hay)) {
            return false;
        }
        self.include.is_empty() || self.include.iter().any(|p| glob_match(p, &hay))
    }

    /// Whether any pattern was supplied at all (an inactive filter keeps every
    /// title, so the pre-filter sync path is unchanged).
    pub fn is_active(&self) -> bool {
        !self.include.is_empty() || !self.exclude.is_empty()
    }
}

/// Match `hay` against `pat`. Both MUST already be lowercased. No regex, no
/// dependency: `*` matches any run of characters, a `*`-free pattern is a
/// substring test, and empty segments (from `**` or a leading/trailing `*`)
/// are skipped.
fn glob_match(pat: &str, hay: &str) -> bool {
    if !pat.contains('*') {
        return hay.contains(pat);
    }
    let segs: Vec<&str> = pat.split('*').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return true; // `*` / `**` — keep everything
    }
    let anchor_start = !pat.starts_with('*');
    let anchor_end = !pat.ends_with('*');
    let last = segs.len() - 1;
    let mut cursor = 0usize;
    for (i, seg) in segs.iter().enumerate() {
        if i == 0 && anchor_start {
            if !hay.starts_with(seg) {
                return false;
            }
            cursor = seg.len();
            continue;
        }
        if i == last && anchor_end {
            // Must be a suffix, and must not overlap what earlier segments
            // already consumed (`a*a` must not match a lone "a").
            if !hay.ends_with(seg) || hay.len() - seg.len() < cursor {
                return false;
            }
            cursor = hay.len();
            continue;
        }
        match hay[cursor..].find(seg) {
            Some(off) => cursor += off + seg.len(),
            None => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pats(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn keep_all_when_no_patterns() {
        let f = TitleFilter::new(&[], &[]);
        assert!(!f.is_active());
        assert!(f.keep("Payments Runbook"));
        assert!(f.keep("2026-01-04 Weekly sync notes"));
        assert!(f.keep(""));
    }

    #[test]
    fn include_is_case_insensitive_substring() {
        let f = TitleFilter::new(&pats(&["payments"]), &[]);
        assert!(f.is_active());
        assert!(f.keep("Payments Runbook"));
        assert!(f.keep("PAYMENTS"));
        assert!(f.keep("Legacy payments gateway"));
        assert!(!f.keep("Onboarding"));
    }

    #[test]
    fn wildcard_matches_prefix_suffix_and_middle() {
        let f = TitleFilter::new(&pats(&["arch*design"]), &[]);
        assert!(f.keep("Architecture — Design"));
        assert!(!f.keep("Design Architecture"));

        let suffix = TitleFilter::new(&pats(&["*runbook"]), &[]);
        assert!(suffix.keep("Payments Runbook"));
        assert!(!suffix.keep("Runbook index page"));

        let prefix = TitleFilter::new(&pats(&["adr-*"]), &[]);
        assert!(prefix.keep("ADR-014 queue topology"));
        assert!(!prefix.keep("Draft ADR-014"));

        let middle = TitleFilter::new(&pats(&["*queue*"]), &[]);
        assert!(middle.keep("The queue topology"));
        assert!(!middle.keep("Topology"));
    }

    #[test]
    fn exclude_wins_over_include() {
        let f = TitleFilter::new(&pats(&["payments"]), &pats(&["*draft*"]));
        assert!(f.keep("Payments Runbook"));
        assert!(!f.keep("Payments Runbook (draft)"));
        // Exclude alone still keeps everything it does not match.
        let only_ex = TitleFilter::new(&[], &pats(&["meeting notes"]));
        assert!(only_ex.keep("Payments Runbook"));
        assert!(!only_ex.keep("2026-01-04 Meeting Notes"));
    }

    #[test]
    fn star_only_pattern_keeps_everything() {
        let inc = TitleFilter::new(&pats(&["*"]), &[]);
        assert!(inc.keep("anything at all"));
        assert!(inc.keep(""));
        // …and as an exclude it drops everything — which is why the CLI
        // refuses to write an empty manifest.
        let exc = TitleFilter::new(&[], &pats(&["*"]));
        assert!(!exc.keep("anything at all"));
        assert!(!exc.keep(""));
    }
}
