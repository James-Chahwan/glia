//! lcov tracefiles (LF.6a), the coverage common denominator: coverage.py
//! `coverage lcov`, c8 / nyc / istanbul, `cargo llvm-cov --lcov`, gcov2lcov.
//!
//! Records run from `SF:<path>` to `end_of_record`; each `DA:<line>,<hits>`
//! (an optional third checksum field is ignored) is one line's hit count.
//! `FN`, `FNDA`, `BRDA`, `LF` and `LH` are skipped: the build recomputes
//! function and total coverage from the `DA` lines. A file named by several
//! records (one per test run) is merged into one, hits summed.

use std::collections::BTreeMap;
use std::path::Path;

use repo_graph_code_domain::snapshots::LcovFileRecord;

/// Parse one lcov tracefile. `rel` is the `SF:` path relative to `repo_root`
/// (its canonical form, or as given), or the `SF:` path itself when already
/// relative; `None` for a file outside the repo.
pub fn parse_lcov(text: &str, repo_root: &Path) -> Vec<LcovFileRecord> {
    let mut files: BTreeMap<String, BTreeMap<u32, u32>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(sf) = line.strip_prefix("SF:") {
            let sf = sf.trim().to_string();
            files.entry(sf.clone()).or_default();
            current = Some(sf);
        } else if line == "end_of_record" {
            current = None;
        } else if let (Some(da), Some(sf)) = (line.strip_prefix("DA:"), &current)
            && let Some((line_no, hits)) = parse_da(da)
            && let Some(lines) = files.get_mut(sf)
        {
            let slot = lines.entry(line_no).or_insert(0);
            *slot = slot.saturating_add(hits);
        }
    }
    let roots = root_prefixes(repo_root);
    files
        .into_iter()
        .map(|(sf, lines)| LcovFileRecord {
            rel: rel_path(&sf, &roots),
            lines: lines.into_iter().map(|(l, h)| [l, h]).collect(),
            sf,
        })
        .collect()
}

/// Merge records naming the same `SF:` path (from several tracefiles), hits
/// summed, sorted by `sf`.
pub(crate) fn merge(records: Vec<LcovFileRecord>) -> Vec<LcovFileRecord> {
    let mut by_sf: BTreeMap<String, (Option<String>, BTreeMap<u32, u32>)> = BTreeMap::new();
    for record in records {
        let (rel, lines) = by_sf.entry(record.sf).or_default();
        if rel.is_none() {
            *rel = record.rel;
        }
        for [line, hits] in record.lines {
            let slot = lines.entry(line).or_insert(0);
            *slot = slot.saturating_add(hits);
        }
    }
    by_sf
        .into_iter()
        .map(|(sf, (rel, lines))| LcovFileRecord { sf, rel, lines: lines.into_iter().map(|(l, h)| [l, h]).collect() })
        .collect()
}

/// `<line>,<hits>[,<checksum>]`: a 1-based line and its hits, saturating to
/// `u32::MAX`. A negative, empty or non-numeric field is not a DA line.
fn parse_da(da: &str) -> Option<(u32, u32)> {
    let mut fields = da.split(',');
    let line = fields.next()?.trim().parse::<u32>().ok().filter(|&l| l > 0)?;
    let hits = fields.next()?.trim();
    if hits.is_empty() || !hits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hits = hits.parse::<u64>().map_or(u32::MAX, |h| u32::try_from(h).unwrap_or(u32::MAX));
    Some((line, hits))
}

/// The repo root as `/`-terminated prefixes: canonical first, then as given.
fn root_prefixes(repo_root: &Path) -> Vec<String> {
    let mut roots = Vec::new();
    for root in [std::fs::canonicalize(repo_root).ok(), Some(repo_root.to_path_buf())].into_iter().flatten() {
        let mut s = root.to_string_lossy().replace('\\', "/");
        if !s.ends_with('/') {
            s.push('/');
        }
        if !roots.contains(&s) {
            roots.push(s);
        }
    }
    roots
}

fn rel_path(sf: &str, roots: &[String]) -> Option<String> {
    let norm = sf.replace('\\', "/");
    let absolute = norm.starts_with('/') || norm.as_bytes().get(1) == Some(&b':');
    if !absolute {
        let rel = norm.trim_start_matches("./");
        return (!rel.is_empty() && !rel.starts_with("../")).then(|| rel.to_string());
    }
    let strip = |path: &str| roots.iter().find_map(|root| path.strip_prefix(root.as_str())).map(str::to_string);
    strip(&norm).or_else(|| {
        let canonical = std::fs::canonicalize(sf).ok()?;
        strip(&canonical.to_string_lossy().replace('\\', "/"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_records_and_lines_merge_by_summing() {
        let text = "SF:a.py\nDA:2,1\nDA:1,0\nDA:2,3\nend_of_record\nSF:a.py\nDA:1,4294967295\nDA:1,9\nend_of_record\n\
                    SF:b.py\nDA:x,1\nDA:3,-1\nDA:0,1\nend_of_record\n";
        let rows = parse_lcov(text, Path::new("/nowhere"));
        assert_eq!(rows[0].lines, vec![[1, u32::MAX], [2, 4]]);
        assert!(rows[1].lines.is_empty(), "malformed DA lines are skipped: {:?}", rows[1]);
        let merged = merge(vec![rows[0].clone(), rows[0].clone()]);
        assert_eq!(merged[0].lines, vec![[1, u32::MAX], [2, 8]]);
    }

    #[test]
    fn outside_and_escaping_paths_have_no_rel() {
        let roots = root_prefixes(Path::new("/ci/work/repo"));
        assert_eq!(rel_path("/usr/lib/python3/x.py", &roots), None);
        assert_eq!(rel_path("../other/x.py", &roots), None);
        assert_eq!(rel_path("./api/app.py", &roots).as_deref(), Some("api/app.py"));
        assert_eq!(rel_path("/ci/work/repository/x.py", &roots), None, "a sibling that shares the prefix");
    }
}
