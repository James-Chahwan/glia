//! The edge-precision audit (FD.2, seed §F item 12): a sampled, stratified
//! audit of the built graph's edges per (edge category x evidence tier),
//! judged against source. `bench/edge-audit/README.md` is the method,
//! `RUBRIC.md` what makes an edge correct; `scripts/edge-audit.sh` drives it.
//!
//! ```text
//! edge_audit sample --repo <dir> --label <l> --rev <sha> [--skip <prefix>].. [--repo ..]..
//!                   --plan bench/edge-audit/floors.json --out <samples.jsonl>
//! edge_audit show   <samples.jsonl> --root <dir> [--label <l>] [--id <hex>]
//! edge_audit report --samples <glob>.. --verdicts <glob>.. --plan <floors.json>
//!                   --out-json <f> --out-md <f> [--judge <name>] [--check]
//! ```
//!
//! The tier is [`glia_engine::why::edge_tier`], the reading `glia why`, `check`
//! and `glia query` print. Selection is deterministic: each candidate edge's
//! key is xxh64 over (seed, label, category, from qname, to qname, evidence
//! rule, file, line); a stratum takes its smallest keys. No HashMap is iterated
//! into an output; every output map is a BTreeMap and every list is sorted.
//! No source text is written: `show` reads the snapshot live.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use glia_code_domain::evidence::Evidence;
use glia_code_domain::{edge_category, node_kind};
use glia_core::Edge;
use glia_engine::{Located, Locator, generate_one, why};
use serde::{Deserialize, Serialize};
use twox_hash::XxHash64;

/// z for a two-sided 95% interval.
const Z: f64 = 1.959964;
/// A stratum judged on fewer rows (unsure excluded) is INSUFFICIENT, unless it
/// is a census (every edge of the stratum judged).
const MIN_JUDGED: u64 = 10;
/// A stratum whose unsure share of its verdicts exceeds this is NOISY.
const NOISY_SHARE: f64 = 0.10;

#[derive(Debug, Clone, Deserialize)]
struct Plan {
    floors: BTreeMap<String, f64>,
    n_per_stratum: u64,
    n_structural: u64,
    structural: Vec<String>,
    seed: String,
}

impl Plan {
    fn read(path: &str) -> Result<Plan, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("plan {path}: {e}"))?;
        serde_json::from_str(&text).map_err(|e| format!("plan {path}: {e}"))
    }
    fn target(&self, category: &str) -> u64 {
        if self.structural.iter().any(|c| c == category) {
            self.n_structural
        } else {
            self.n_per_stratum
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct End {
    qname: String,
    kind: String,
    file: Option<String>,
    line: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Site {
    file: Option<String>,
    line: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SampleRow {
    id: String,
    label: String,
    rev: String,
    category: String,
    tier: String,
    tier_note: Option<String>,
    emitter: Option<String>,
    rule: Option<String>,
    site: Site,
    from: End,
    to: End,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Header {
    label: String,
    rev: String,
    seed: String,
    edges: u64,
    stratum_sizes: BTreeMap<String, u64>,
    unjudgeable: BTreeMap<String, u64>,
    excluded: BTreeMap<String, u64>,
}

/// One tree to sample: `--repo` with its `--label`, `--rev` and `--skip`s.
#[derive(Debug, Clone, Default)]
struct RepoSpec {
    dir: String,
    label: String,
    rev: String,
    skips: Vec<String>,
}

/// One repo's population: candidates by stratum (sorted by key, deduped).
struct Population {
    header: Header,
    by_stratum: BTreeMap<String, Vec<(u64, SampleRow)>>,
}

fn stratum_key(category: &str, tier: &str) -> String {
    format!("{category}|{tier}")
}

fn end_of(l: &Located) -> End {
    End {
        qname: l.qname.clone(),
        kind: l.kind.to_string(),
        file: l.file.clone(),
        line: l.line,
    }
}

fn edge_key(
    seed: &str,
    label: &str,
    category: &str,
    from: &str,
    to: &str,
    ev: Option<&Evidence>,
) -> u64 {
    let mut h = XxHash64::with_seed(0);
    let line = ev
        .and_then(|e| e.line)
        .map(|l| l.to_string())
        .unwrap_or_default();
    let parts = [
        seed,
        label,
        category,
        from,
        to,
        ev.and_then(|e| e.rule.as_deref()).unwrap_or(""),
        ev.and_then(|e| e.file.as_deref()).unwrap_or(""),
        line.as_str(),
    ];
    for p in parts {
        h.write(p.as_bytes());
        h.write(&[0x1f]);
    }
    h.finish()
}

/// Why an edge is out of the population, if it is: the 0.5.4 layers gated by
/// their own audits (TRACES, GOAL_UNIT ends, org-package routing) and the
/// `--skip` prefixes.
fn exclusion(
    e: &Edge,
    from: &Located,
    to: &Located,
    ev: Option<&Evidence>,
    skips: &[String],
) -> Option<&'static str> {
    if e.category == edge_category::TRACES {
        return Some("traces");
    }
    let goal = node_kind::name(node_kind::GOAL_UNIT);
    if from.kind == goal || to.kind == goal {
        return Some("goal_unit");
    }
    if ev.is_some_and(|ev| ev.emitter == "resolver:org_packages") {
        return Some("org_packages");
    }
    let file = from
        .file
        .as_deref()
        .or(ev.and_then(|ev| ev.file.as_deref()));
    if let Some(file) = file
        && skips.iter().any(|s| file.starts_with(s.as_str()))
    {
        return Some("skip");
    }
    None
}

fn population(spec: &RepoSpec, seed: &str) -> Result<Population, String> {
    let built = generate_one(&spec.dir).map_err(|e| format!("build {}: {e}", spec.dir))?;
    let merged = &built.merged;
    let loc = Locator::new(merged);
    let mut header = Header {
        label: spec.label.clone(),
        rev: spec.rev.clone(),
        seed: seed.to_string(),
        edges: 0,
        stratum_sizes: BTreeMap::new(),
        unjudgeable: BTreeMap::new(),
        excluded: BTreeMap::new(),
    };
    let mut cands: Vec<(u64, String, SampleRow)> = Vec::new();
    let all = merged
        .graphs
        .iter()
        .flat_map(|g| g.edges.iter())
        .chain(merged.cross_edges.iter());
    for e in all {
        header.edges += 1;
        let from = loc.locate(e.from);
        let to = loc.locate(e.to);
        let ev = Evidence::of(e);
        if let Some(why) = exclusion(e, &from, &to, ev.as_ref(), &spec.skips) {
            *header.excluded.entry(why.to_string()).or_default() += 1;
            continue;
        }
        let category = edge_category::name(e.category).to_string();
        let (tier, note) = why::edge_tier(e);
        let stratum = stratum_key(&category, tier);
        let located = |l: &Located| l.file.is_some() && l.line.is_some();
        let ev_file = ev.as_ref().and_then(|ev| ev.file.clone());
        if !(located(&from) && located(&to)) && ev_file.is_none() {
            *header.unjudgeable.entry(stratum).or_default() += 1;
            continue;
        }
        let key = edge_key(
            seed,
            &spec.label,
            &category,
            &from.qname,
            &to.qname,
            ev.as_ref(),
        );
        let site = match &ev_file {
            Some(f) => Site {
                file: Some(f.clone()),
                line: ev.as_ref().and_then(|ev| ev.line).map(|l| i64::from(l) + 1),
            },
            None => Site {
                file: from.file.clone(),
                line: from.line,
            },
        };
        let row = SampleRow {
            id: format!("{key:016x}"),
            label: spec.label.clone(),
            rev: spec.rev.clone(),
            category,
            tier: tier.to_string(),
            tier_note: note,
            emitter: ev.as_ref().map(|ev| ev.emitter.clone()),
            rule: ev.as_ref().and_then(|ev| ev.rule.clone()),
            site,
            from: end_of(&from),
            to: end_of(&to),
        };
        cands.push((key, stratum, row));
    }
    // One key is one judgeable claim: an edge stored twice (two per-language
    // graphs) is one candidate, the first in graph order.
    cands.sort_by_key(|c| c.0);
    cands.dedup_by(|b, a| a.0 == b.0);
    let mut by_stratum: BTreeMap<String, Vec<(u64, SampleRow)>> = BTreeMap::new();
    for (key, stratum, row) in cands {
        by_stratum.entry(stratum).or_default().push((key, row));
    }
    for (s, rows) in &by_stratum {
        header.stratum_sizes.insert(s.clone(), rows.len() as u64);
    }
    Ok(Population { header, by_stratum })
}

/// Split a stratum's target `n` across repos proportionally to their sizes
/// `sizes[r]`, every repo holding the stratum at least 1, largest remainder
/// first (ties by label), each capped at its size. When the pooled stratum
/// holds at most `n` edges every repo is censused.
fn allocate(n: u64, sizes: &[(String, u64)]) -> Vec<u64> {
    let total: u64 = sizes.iter().map(|s| s.1).sum();
    if total <= n {
        return sizes.iter().map(|s| s.1).collect();
    }
    let mut quota: Vec<u64> = Vec::with_capacity(sizes.len());
    let mut rema: Vec<(u128, &str, usize)> = Vec::new();
    for (i, (label, size)) in sizes.iter().enumerate() {
        let exact = u128::from(n) * u128::from(*size);
        let base = (exact / u128::from(total)) as u64;
        let base = if *size > 0 { base.max(1) } else { 0 };
        quota.push(base.min(*size));
        rema.push((exact % u128::from(total), label.as_str(), i));
    }
    // Largest remainder first, ties by label.
    rema.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let mut given: u64 = quota.iter().sum();
    while given < n {
        let mut moved = false;
        for &(_, _, i) in &rema {
            if given >= n {
                break;
            }
            if quota[i] < sizes[i].1 {
                quota[i] += 1;
                given += 1;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    quota
}

/// Sample every spec into one JSONL text (per label: its header line, then its
/// rows sorted by (category, tier, key)) and the per-repo marker lines.
fn sample_text(specs: &[RepoSpec], plan: &Plan) -> Result<(String, Vec<String>), String> {
    let mut pops = Vec::with_capacity(specs.len());
    for spec in specs {
        pops.push(population(spec, &plan.seed)?);
    }
    let strata: BTreeSet<String> = pops
        .iter()
        .flat_map(|p| p.by_stratum.keys().cloned())
        .collect();
    let mut picked: Vec<Vec<(String, String, u64, SampleRow)>> = vec![Vec::new(); pops.len()];
    for s in &strata {
        let category = s.split('|').next().unwrap_or("");
        let sizes: Vec<(String, u64)> = pops
            .iter()
            .map(|p| {
                (
                    p.header.label.clone(),
                    p.by_stratum.get(s).map_or(0, |v| v.len() as u64),
                )
            })
            .collect();
        let quota = allocate(plan.target(category), &sizes);
        for (ri, p) in pops.iter().enumerate() {
            let Some(rows) = p.by_stratum.get(s) else {
                continue;
            };
            for (key, row) in rows.iter().take(quota[ri] as usize) {
                picked[ri].push((row.category.clone(), row.tier.clone(), *key, row.clone()));
            }
        }
    }
    let mut out = String::new();
    let mut markers = Vec::new();
    for (ri, p) in pops.iter().enumerate() {
        let rows = &mut picked[ri];
        rows.sort_by(|a, b| (&a.0, &a.1, a.2).cmp(&(&b.0, &b.1, b.2)));
        out.push_str(&serde_json::to_string(&p.header).map_err(|e| e.to_string())?);
        out.push('\n');
        for (_, _, _, row) in rows.iter() {
            out.push_str(&serde_json::to_string(row).map_err(|e| e.to_string())?);
            out.push('\n');
        }
        markers.push(format!(
            "[edge-audit] sample label={} edges={} strata={} sampled={} unjudgeable={} excluded={} seed={}",
            p.header.label,
            p.header.edges,
            p.header.stratum_sizes.len(),
            rows.len(),
            p.header.unjudgeable.values().sum::<u64>(),
            p.header.excluded.values().sum::<u64>(),
            plan.seed
        ));
    }
    Ok((out, markers))
}

// ---------------------------------------------------------------- estimators

/// Wilson score interval for `k` successes of `n`.
fn wilson(k: u64, n: u64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let n = n as f64;
    let p = k as f64 / n;
    let z2 = Z * Z;
    let denom = 1.0 + z2 / n;
    let center = (p + z2 / (2.0 * n)) / denom;
    let half = Z / denom * (p * (1.0 - p) / n + z2 / (4.0 * n * n)).sqrt();
    ((center - half).max(0.0), (center + half).min(1.0))
}

/// The stratified estimate over strata `(N_h, n_h, p_h)` with `n_h > 0`:
/// `p = sum W_h p_h`, `var = sum W_h^2 p_h (1 - p_h) / n_h (1 - n_h / N_h)`,
/// `W_h = N_h / N`; a census stratum (`n_h >= N_h`) adds no variance.
/// `None` when no stratum is judged.
fn stratified(strata: &[(u64, u64, f64)]) -> Option<(f64, f64, f64)> {
    let judged: Vec<&(u64, u64, f64)> = strata.iter().filter(|s| s.1 > 0).collect();
    let total: u64 = judged.iter().map(|s| s.0).sum();
    if total == 0 {
        return None;
    }
    let (mut p, mut var) = (0.0, 0.0);
    for &&(nn, n, ph) in &judged {
        let w = nn as f64 / total as f64;
        p += w * ph;
        if n < nn {
            var += w * w * ph * (1.0 - ph) / n as f64 * (1.0 - n as f64 / nn as f64);
        }
    }
    let half = 1.96 * var.sqrt();
    Some((p, (p - half).max(0.0), (p + half).min(1.0)))
}

fn r4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

// -------------------------------------------------------------------- report

#[derive(Debug, Clone, Deserialize)]
struct VerdictRow {
    id: String,
    verdict: String,
    /// `reason` (the judge's one line) is read by people, not the estimate;
    /// serde skips it.
    judge: String,
}

#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    sampled: u64,
    correct: u64,
    wrong: u64,
    unsure: u64,
}

impl Counts {
    fn add(&mut self, verdict: Option<&str>) {
        self.sampled += 1;
        match verdict {
            Some("correct") => self.correct += 1,
            Some("wrong") => self.wrong += 1,
            Some(_) => self.unsure += 1,
            None => {}
        }
    }
    fn judged(&self) -> u64 {
        self.correct + self.wrong
    }
    fn pending(&self) -> u64 {
        self.sampled - self.correct - self.wrong - self.unsure
    }
}

#[derive(Debug, Serialize)]
struct StratumOut {
    category: String,
    tier: String,
    #[serde(rename = "N")]
    big_n: u64,
    sampled: u64,
    pending: u64,
    n: u64,
    correct: u64,
    wrong: u64,
    unsure: u64,
    p: Option<f64>,
    lo: Option<f64>,
    hi: Option<f64>,
    floor: Option<f64>,
    status: String,
    flags: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AggOut {
    name: String,
    #[serde(rename = "N")]
    big_n: u64,
    covered: u64,
    n: u64,
    p: Option<f64>,
    lo: Option<f64>,
    hi: Option<f64>,
}

#[derive(Debug, Serialize)]
struct RepoOut {
    edges: u64,
    unjudgeable: u64,
    excluded: BTreeMap<String, u64>,
    overall: AggOut,
    strata: Vec<StratumOut>,
}

#[derive(Debug, Serialize)]
struct ReportOut {
    floors: BTreeMap<String, f64>,
    seed: String,
    strata: Vec<StratumOut>,
    categories: Vec<AggOut>,
    tiers: Vec<AggOut>,
    repos: BTreeMap<String, RepoOut>,
    under_floor: Vec<String>,
}

fn stratum_out(category: &str, tier: &str, big_n: u64, c: Counts, plan: &Plan) -> StratumOut {
    let n = c.judged();
    let floor = plan.floors.get(&tier.to_ascii_lowercase()).copied();
    let (p, lo, hi) = if n > 0 {
        let (lo, hi) = wilson(c.correct, n);
        (
            Some(r4(c.correct as f64 / n as f64)),
            Some(r4(lo)),
            Some(r4(hi)),
        )
    } else {
        (None, None, None)
    };
    let mut flags = Vec::new();
    if let Some(f) = floor
        && n > 0
        && (c.correct as f64 / n as f64) < f
    {
        flags.push("under_floor".to_string());
    }
    if n > 0 && n < MIN_JUDGED && n < big_n {
        flags.push("insufficient".to_string());
    }
    let verdicts = c.correct + c.wrong + c.unsure;
    if verdicts > 0 && c.unsure as f64 > NOISY_SHARE * verdicts as f64 {
        flags.push("noisy".to_string());
    }
    let status = if n == 0 {
        "unjudged".to_string()
    } else if flags.iter().any(|f| f == "insufficient") {
        "insufficient".to_string()
    } else if flags.iter().any(|f| f == "under_floor") {
        "under_floor".to_string()
    } else {
        "ok".to_string()
    };
    StratumOut {
        category: category.to_string(),
        tier: tier.to_string(),
        big_n,
        sampled: c.sampled,
        pending: c.pending(),
        n,
        correct: c.correct,
        wrong: c.wrong,
        unsure: c.unsure,
        p,
        lo,
        hi,
        floor,
        status,
        flags,
    }
}

fn agg(name: &str, strata: &[&StratumOut]) -> AggOut {
    let rows: Vec<(u64, u64, f64)> = strata
        .iter()
        .map(|s| {
            let ph = if s.n > 0 {
                s.correct as f64 / s.n as f64
            } else {
                0.0
            };
            (s.big_n, s.n, ph)
        })
        .collect();
    let est = stratified(&rows);
    AggOut {
        name: name.to_string(),
        big_n: strata.iter().map(|s| s.big_n).sum(),
        covered: strata.iter().filter(|s| s.n > 0).map(|s| s.big_n).sum(),
        n: strata.iter().map(|s| s.n).sum(),
        p: est.map(|e| r4(e.0)),
        lo: est.map(|e| r4(e.1)),
        hi: est.map(|e| r4(e.2)),
    }
}

/// `pattern` with a `*` in its file name matched in its directory, sorted;
/// otherwise the path itself when it exists.
fn expand(pattern: &str) -> Result<Vec<PathBuf>, String> {
    let path = Path::new(pattern);
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !name.contains('*') {
        return Ok(if path.exists() {
            vec![path.to_path_buf()]
        } else {
            Vec::new()
        });
    }
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Ok(Vec::new());
    };
    let mut out: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_str().is_some_and(|f| wildcard(name, f)))
        .map(|e| e.path())
        .collect();
    out.sort();
    Ok(out)
}

fn wildcard(pat: &str, s: &str) -> bool {
    match pat.split_once('*') {
        None => pat == s,
        Some((head, rest)) => {
            let Some(s) = s.strip_prefix(head) else {
                return false;
            };
            if rest.is_empty() {
                return true;
            }
            (0..=s.len()).any(|i| s.is_char_boundary(i) && wildcard(rest, &s[i..]))
        }
    }
}

fn read_samples(paths: &[PathBuf]) -> Result<(Vec<Header>, Vec<SampleRow>), String> {
    let (mut headers, mut rows) = (Vec::new(), Vec::new());
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .map_err(|e| format!("{}:{}: {e}", p.display(), i + 1))?;
            if v.get("stratum_sizes").is_some() {
                headers.push(
                    serde_json::from_value(v)
                        .map_err(|e| format!("{}:{}: {e}", p.display(), i + 1))?,
                );
            } else {
                rows.push(
                    serde_json::from_value(v)
                        .map_err(|e| format!("{}:{}: {e}", p.display(), i + 1))?,
                );
            }
        }
    }
    Ok((headers, rows))
}

/// The verdict each sample id gets: one judge's verdict, or (several judges)
/// the shared verdict, `unsure` when they disagree. Errors name the id.
fn resolve_verdicts(
    paths: &[PathBuf],
    ids: &BTreeSet<String>,
    judge: Option<&str>,
) -> Result<BTreeMap<String, String>, String> {
    let mut by_id: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        for (i, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let v: VerdictRow = serde_json::from_str(line)
                .map_err(|e| format!("{}:{}: {e}", p.display(), i + 1))?;
            if !matches!(v.verdict.as_str(), "correct" | "wrong" | "unsure") {
                return Err(format!(
                    "verdict {}: `{}` is not correct|wrong|unsure",
                    v.id, v.verdict
                ));
            }
            if !ids.contains(&v.id) {
                return Err(format!("verdict {}: id is in no samples file", v.id));
            }
            if judge.is_some_and(|j| j != v.judge) {
                continue;
            }
            let per = by_id.entry(v.id.clone()).or_default();
            if per.insert(v.judge.clone(), v.verdict).is_some() {
                return Err(format!(
                    "verdict {}: two verdicts from judge `{}`",
                    v.id, v.judge
                ));
            }
        }
    }
    Ok(by_id
        .into_iter()
        .map(|(id, per)| {
            let first = per
                .values()
                .next()
                .cloned()
                .unwrap_or_else(|| "unsure".to_string());
            let one = if per.values().all(|v| *v == first) {
                first
            } else {
                "unsure".to_string()
            };
            (id, one)
        })
        .collect())
}

fn build_report(
    headers: &[Header],
    rows: &[SampleRow],
    verdicts: &BTreeMap<String, String>,
    plan: &Plan,
) -> ReportOut {
    // Pooled stratum sizes and per-repo sizes.
    let mut pooled: BTreeMap<(String, String), u64> = BTreeMap::new();
    for h in headers {
        for (s, n) in &h.stratum_sizes {
            let (c, t) = s.split_once('|').unwrap_or((s.as_str(), ""));
            *pooled.entry((c.to_string(), t.to_string())).or_default() += n;
        }
    }
    let mut counts: BTreeMap<(String, String), Counts> = BTreeMap::new();
    let mut repo_counts: BTreeMap<(String, String, String), Counts> = BTreeMap::new();
    for r in rows {
        let v = verdicts.get(&r.id).map(String::as_str);
        counts
            .entry((r.category.clone(), r.tier.clone()))
            .or_default()
            .add(v);
        repo_counts
            .entry((r.label.clone(), r.category.clone(), r.tier.clone()))
            .or_default()
            .add(v);
    }
    let strata: Vec<StratumOut> = pooled
        .iter()
        .map(|((c, t), n)| {
            stratum_out(
                c,
                t,
                *n,
                counts
                    .get(&(c.clone(), t.clone()))
                    .copied()
                    .unwrap_or_default(),
                plan,
            )
        })
        .collect();
    let cats: BTreeSet<&str> = strata.iter().map(|s| s.category.as_str()).collect();
    let categories = cats
        .iter()
        .map(|c| {
            agg(
                c,
                &strata
                    .iter()
                    .filter(|s| s.category == *c)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    let tier_names: BTreeSet<&str> = strata.iter().map(|s| s.tier.as_str()).collect();
    let tiers = tier_names
        .iter()
        .map(|t| {
            agg(
                t,
                &strata.iter().filter(|s| s.tier == *t).collect::<Vec<_>>(),
            )
        })
        .collect();
    let mut repos = BTreeMap::new();
    for h in headers {
        let rs: Vec<StratumOut> = h
            .stratum_sizes
            .iter()
            .map(|(s, n)| {
                let (c, t) = s.split_once('|').unwrap_or((s.as_str(), ""));
                let k = (h.label.clone(), c.to_string(), t.to_string());
                stratum_out(
                    c,
                    t,
                    *n,
                    repo_counts.get(&k).copied().unwrap_or_default(),
                    plan,
                )
            })
            .collect();
        let overall = agg(&h.label, &rs.iter().collect::<Vec<_>>());
        repos.insert(
            h.label.clone(),
            RepoOut {
                edges: h.edges,
                unjudgeable: h.unjudgeable.values().sum(),
                excluded: h.excluded.clone(),
                overall,
                strata: rs,
            },
        );
    }
    let under_floor = strata
        .iter()
        .filter(|s| s.flags.iter().any(|f| f == "under_floor"))
        .map(|s| stratum_key(&s.category, &s.tier))
        .collect();
    ReportOut {
        floors: plan.floors.clone(),
        seed: plan.seed.clone(),
        strata,
        categories,
        tiers,
        repos,
        under_floor,
    }
}

fn f3(x: Option<f64>) -> String {
    x.map_or("-".to_string(), |v| format!("{v:.3}"))
}

fn strata_table(md: &mut String, strata: &[StratumOut]) {
    md.push_str("| category | tier | N | sampled | judged | correct | wrong | unsure | p | 95% CI | floor | status |\n");
    md.push_str("|---|---|---:|---:|---:|---:|---:|---:|---:|---|---:|---|\n");
    for s in strata {
        // The status, then any flag it does not already name.
        let extra: Vec<&str> = s
            .flags
            .iter()
            .map(String::as_str)
            .filter(|f| *f != s.status)
            .collect();
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | [{}, {}] | {} | {} |\n",
            s.category,
            s.tier,
            s.big_n,
            s.sampled,
            s.n,
            s.correct,
            s.wrong,
            s.unsure,
            f3(s.p),
            f3(s.lo),
            f3(s.hi),
            f3(s.floor),
            if extra.is_empty() {
                s.status.clone()
            } else {
                format!("{} ({})", s.status, extra.join(", "))
            }
        ));
    }
}

fn agg_table(md: &mut String, head: &str, rows: &[AggOut]) {
    md.push_str(&format!(
        "| {head} | N | N judged strata | judged | p (stratified) | 95% CI |\n"
    ));
    md.push_str("|---|---:|---:|---:|---:|---|\n");
    for a in rows {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | [{}, {}] |\n",
            a.name,
            a.big_n,
            a.covered,
            a.n,
            f3(a.p),
            f3(a.lo),
            f3(a.hi)
        ));
    }
}

fn render_md(r: &ReportOut) -> String {
    let mut md = String::from("# Edge-precision audit\n\n");
    md.push_str(
        "Derived by `scripts/edge-audit.sh report` from the committed samples and verdicts; \
         never edit by hand (`report --check` fails on any difference).\n\n",
    );
    md.push_str("## Method\n\n");
    md.push_str(&format!(
        "Strata are (edge category x evidence tier), the tier `glia why` prints. Each stratum's \
         sample is the edges with the smallest xxh64 keys (seed `{}`), split across repos in \
         proportion to their stratum sizes. p is correct / (correct + wrong) with a Wilson 95% \
         interval; `unsure` verdicts are excluded and counted. Per category and per tier the \
         estimate is stratified, weights N_h / N over the judged strata, with the finite \
         population correction (a census stratum adds no variance). A stratum is under floor \
         when p is below its tier's floor; insufficient when fewer than {} rows are judged \
         (a census excepted); noisy when unsure is over {}% of its verdicts. Floors: {}.\n\n",
        r.seed,
        MIN_JUDGED,
        (NOISY_SHARE * 100.0) as u32,
        r.floors
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    md.push_str("## Strata (pooled)\n\n");
    strata_table(&mut md, &r.strata);
    md.push_str("\n## Categories\n\n");
    agg_table(&mut md, "category", &r.categories);
    md.push_str("\n## Tiers\n\n");
    agg_table(&mut md, "tier", &r.tiers);
    md.push_str("\n## Under floor\n\n");
    if r.under_floor.is_empty() {
        md.push_str("None.\n");
    } else {
        for s in &r.under_floor {
            md.push_str(&format!("- `{s}`\n"));
        }
    }
    md.push_str("\n## Per repo (reported, not gated)\n");
    for (label, repo) in &r.repos {
        md.push_str(&format!(
            "\n### {label}\n\nedges {}, unjudgeable {}, excluded {}; stratified p {} [{}, {}]\n\n",
            repo.edges,
            repo.unjudgeable,
            if repo.excluded.is_empty() {
                "0".to_string()
            } else {
                repo.excluded
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            },
            f3(repo.overall.p),
            f3(repo.overall.lo),
            f3(repo.overall.hi)
        ));
        strata_table(&mut md, &repo.strata);
    }
    md
}

// ---------------------------------------------------------------------- show

fn excerpt(root: &Path, file: Option<&str>, line: Option<i64>, radius: i64, what: &str) -> String {
    let (Some(file), Some(line)) = (file, line) else {
        return format!("  {what}: (no location)\n");
    };
    let Ok(text) = std::fs::read_to_string(root.join(file)) else {
        return format!(
            "  {what}: {file}:{line} (unreadable under {})\n",
            root.display()
        );
    };
    let mut out = format!("  {what}: {file}:{line}\n");
    let lines: Vec<&str> = text.lines().collect();
    let lo = (line - radius).max(1);
    let hi = (line + radius).min(lines.len() as i64);
    for n in lo..=hi {
        let mark = if n == line { '>' } else { ' ' };
        let src = lines.get((n - 1) as usize).copied().unwrap_or("");
        out.push_str(&format!("  {mark}{n:>6} | {src}\n"));
    }
    out
}

fn show_text(rows: &[SampleRow], root: &Path, label: Option<&str>, id: Option<&str>) -> String {
    let mut out = String::new();
    for r in rows {
        if label.is_some_and(|l| l != r.label) || id.is_some_and(|i| i != r.id) {
            continue;
        }
        out.push_str(&format!(
            "{} {} {} {} -> {}\n",
            r.id, r.category, r.tier, r.from.qname, r.to.qname
        ));
        out.push_str(&format!(
            "  emitter {} rule {}{}\n",
            r.emitter.as_deref().unwrap_or("-"),
            r.rule.as_deref().unwrap_or("-"),
            r.tier_note
                .as_deref()
                .map(|n| format!(" ({n})"))
                .unwrap_or_default()
        ));
        out.push_str(&excerpt(
            root,
            r.site.file.as_deref(),
            r.site.line,
            4,
            "site",
        ));
        out.push_str(&excerpt(
            root,
            r.to.file.as_deref(),
            r.to.line,
            2,
            &format!("target {}", r.to.kind),
        ));
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------- main

fn value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn run_sample(args: &[String]) -> Result<ExitCode, String> {
    let (mut specs, mut plan, mut out) = (Vec::<RepoSpec>::new(), None, None);
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--repo" => specs.push(RepoSpec {
                dir: value(args, &mut i, flag)?,
                ..RepoSpec::default()
            }),
            "--label" | "--rev" | "--skip" => {
                let v = value(args, &mut i, flag)?;
                let spec = specs
                    .last_mut()
                    .ok_or_else(|| format!("{flag} before any --repo"))?;
                match flag {
                    "--label" => spec.label = v,
                    "--rev" => spec.rev = v,
                    _ => spec.skips.push(v),
                }
            }
            "--plan" => plan = Some(value(args, &mut i, flag)?),
            "--out" => out = Some(value(args, &mut i, flag)?),
            other => return Err(format!("sample: unknown argument {other}")),
        }
        i += 1;
    }
    let plan = Plan::read(&plan.ok_or("sample: --plan is required")?)?;
    let out = out.ok_or("sample: --out is required")?;
    if specs.is_empty() || specs.iter().any(|s| s.label.is_empty() || s.rev.is_empty()) {
        return Err("sample: every --repo needs --label and --rev".to_string());
    }
    let (text, markers) = sample_text(&specs, &plan)?;
    if let Some(dir) = Path::new(&out)
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
    {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&out, text).map_err(|e| format!("{out}: {e}"))?;
    for m in markers {
        eprintln!("{m}");
    }
    Ok(ExitCode::SUCCESS)
}

fn run_show(args: &[String]) -> Result<ExitCode, String> {
    let (mut file, mut root, mut label, mut id) = (None, None, None, None);
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--root" => root = Some(value(args, &mut i, flag)?),
            "--label" => label = Some(value(args, &mut i, flag)?),
            "--id" => id = Some(value(args, &mut i, flag)?),
            f if !f.starts_with("--") && file.is_none() => file = Some(f.to_string()),
            other => return Err(format!("show: unknown argument {other}")),
        }
        i += 1;
    }
    let file = file.ok_or("show: <samples.jsonl> is required")?;
    let root = root.ok_or("show: --root is required")?;
    let (_, rows) = read_samples(&[PathBuf::from(file)])?;
    print!(
        "{}",
        show_text(&rows, Path::new(&root), label.as_deref(), id.as_deref())
    );
    Ok(ExitCode::SUCCESS)
}

fn run_report(args: &[String]) -> Result<ExitCode, String> {
    let (mut samples, mut verdicts) = (Vec::new(), Vec::new());
    let (mut plan, mut out_json, mut out_md, mut judge, mut check) =
        (None, None, None, None, false);
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--samples" => samples.extend(expand(&value(args, &mut i, flag)?)?),
            "--verdicts" => verdicts.extend(expand(&value(args, &mut i, flag)?)?),
            "--plan" => plan = Some(value(args, &mut i, flag)?),
            "--out-json" => out_json = Some(value(args, &mut i, flag)?),
            "--out-md" => out_md = Some(value(args, &mut i, flag)?),
            "--judge" => judge = Some(value(args, &mut i, flag)?),
            "--check" => check = true,
            other => return Err(format!("report: unknown argument {other}")),
        }
        i += 1;
    }
    let plan = Plan::read(&plan.ok_or("report: --plan is required")?)?;
    let (out_json, out_md) = (
        out_json.ok_or("report: --out-json is required")?,
        out_md.ok_or("report: --out-md is required")?,
    );
    samples.sort();
    samples.dedup();
    verdicts.sort();
    verdicts.dedup();
    if samples.is_empty() {
        return Err("report: no samples file matched".to_string());
    }
    let (headers, rows) = read_samples(&samples)?;
    let mut ids = BTreeSet::new();
    for r in &rows {
        if !ids.insert(r.id.clone()) {
            return Err(format!("sample {}: id appears twice", r.id));
        }
    }
    let resolved = resolve_verdicts(&verdicts, &ids, judge.as_deref())?;
    let report = build_report(&headers, &rows, &resolved, &plan);
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())? + "\n";
    let md = render_md(&report);
    eprintln!(
        "[edge-audit] report strata={} judged={} unsure={} under_floor={} insufficient={}",
        report.strata.len(),
        report.strata.iter().map(|s| s.n).sum::<u64>(),
        report.strata.iter().map(|s| s.unsure).sum::<u64>(),
        report.under_floor.len(),
        report
            .strata
            .iter()
            .filter(|s| s.status == "insufficient")
            .count()
    );
    if check {
        for (path, want) in [(&out_json, &json), (&out_md, &md)] {
            let have = std::fs::read_to_string(path).unwrap_or_default();
            if have != *want {
                eprintln!(
                    "[edge-audit] report check: FAILED ({path} differs from the samples + verdicts)"
                );
                return Ok(ExitCode::from(1));
            }
        }
        eprintln!("[edge-audit] report check: ok");
        return Ok(ExitCode::SUCCESS);
    }
    print!("{md}");
    std::fs::write(&out_json, json).map_err(|e| format!("{out_json}: {e}"))?;
    std::fs::write(&out_md, md).map_err(|e| format!("{out_md}: {e}"))?;
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        Some("sample") => run_sample(rest),
        Some("show") => run_show(rest),
        Some("report") => run_report(rest),
        _ => Err("usage: edge_audit sample|show|report ..".to_string()),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("[edge-audit] error: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Plan {
        Plan {
            floors: BTreeMap::from([
                ("fact".to_string(), 0.95),
                ("derived".to_string(), 0.90),
                ("heuristic".to_string(), 0.70),
            ]),
            n_per_stratum: 30,
            n_structural: 10,
            structural: vec![
                "DEFINES".to_string(),
                "CONTAINS".to_string(),
                "HAS_ATTRIBUTE".to_string(),
            ],
            seed: "glia-edge-audit-0.5.4".to_string(),
        }
    }

    #[test]
    fn wilson_27_of_30() {
        let (lo, hi) = wilson(27, 30);
        assert_eq!(format!("{lo:.3} {hi:.3}"), "0.744 0.965");
        assert_eq!(wilson(0, 0), (0.0, 1.0));
    }

    #[test]
    fn stratified_census_adds_no_variance() {
        // One census stratum (n = N): the estimate is exact.
        let (p, lo, hi) = stratified(&[(20, 20, 0.9)]).unwrap();
        assert!((p - 0.9).abs() < 1e-12 && (lo - 0.9).abs() < 1e-12 && (hi - 0.9).abs() < 1e-12);
        // Census (100, all judged, p .9) + sampled (300 of 30, p .8):
        // p = .25*.9 + .75*.8 = .825; var = .75^2 * .8*.2/30 * (1 - 30/300).
        let (p, lo, hi) = stratified(&[(100, 100, 0.9), (300, 30, 0.8)]).unwrap();
        let var = 0.75f64.powi(2) * 0.8 * 0.2 / 30.0 * 0.9;
        assert!((p - 0.825).abs() < 1e-12);
        assert!((hi - lo - 2.0 * 1.96 * var.sqrt()).abs() < 1e-12);
        // An unjudged stratum is left out of the weights.
        let (p, _, _) = stratified(&[(100, 10, 1.0), (900, 0, 0.0)]).unwrap();
        assert!((p - 1.0).abs() < 1e-12);
        assert!(stratified(&[(5, 0, 0.0)]).is_none());
    }

    #[test]
    fn allocation_is_proportional_with_at_least_one() {
        let s = |v: &[(&str, u64)]| {
            v.iter()
                .map(|(l, n)| (l.to_string(), *n))
                .collect::<Vec<_>>()
        };
        // Pooled 4000 > 30: 30 * (3000, 990, 10) / 4000 = 22.5, 7.425, 0.075.
        let q = allocate(30, &s(&[("a", 3000), ("b", 990), ("c", 10)]));
        assert_eq!(q.iter().sum::<u64>(), 30);
        assert_eq!(q, vec![22, 7, 1]);
        // A repo without the stratum gets nothing; a census when pooled <= n.
        assert_eq!(
            allocate(30, &s(&[("a", 12), ("b", 0), ("c", 3)])),
            vec![12, 0, 3]
        );
        // Equal remainders break by label.
        assert_eq!(allocate(3, &s(&[("b", 10), ("a", 10)])), vec![1, 2]);
        // A capped repo's share moves to the others.
        let q = allocate(10, &s(&[("a", 1000), ("b", 2)]));
        assert_eq!(q.iter().sum::<u64>(), 10);
        assert!(q[1] <= 2);
    }

    #[test]
    fn wildcard_matches_file_names() {
        assert!(wildcard("*.jsonl", "audit.jsonl"));
        assert!(wildcard("v-*-x.jsonl", "v-judge-x.jsonl"));
        assert!(!wildcard("*.jsonl", "audit.json"));
    }

    #[test]
    fn report_counts_and_rejects_bad_verdicts() {
        let row = |id: &str, cat: &str, tier: &str| SampleRow {
            id: id.to_string(),
            label: "t".to_string(),
            rev: "r".to_string(),
            category: cat.to_string(),
            tier: tier.to_string(),
            tier_note: None,
            emitter: None,
            rule: None,
            site: Site {
                file: None,
                line: None,
            },
            from: End {
                qname: "a".to_string(),
                kind: "FUNCTION".to_string(),
                file: None,
                line: None,
            },
            to: End {
                qname: "b".to_string(),
                kind: "FUNCTION".to_string(),
                file: None,
                line: None,
            },
        };
        let header = Header {
            label: "t".to_string(),
            rev: "r".to_string(),
            seed: "s".to_string(),
            edges: 10,
            stratum_sizes: BTreeMap::from([
                ("CALLS|fact".to_string(), 2),
                ("USES|derived".to_string(), 1),
            ]),
            unjudgeable: BTreeMap::new(),
            excluded: BTreeMap::new(),
        };
        let rows = vec![
            row("01", "CALLS", "fact"),
            row("02", "CALLS", "fact"),
            row("03", "USES", "derived"),
        ];
        let verdicts = BTreeMap::from([
            ("01".to_string(), "correct".to_string()),
            ("02".to_string(), "wrong".to_string()),
            ("03".to_string(), "correct".to_string()),
        ]);
        let r = build_report(&[header], &rows, &verdicts, &plan());
        assert_eq!(r.strata.len(), 2);
        assert_eq!((r.strata[0].correct, r.strata[0].wrong), (1, 1));
        // Census strata (n = N) are never insufficient; .5 < .95 is under floor.
        assert_eq!(r.strata[0].status, "under_floor");
        assert_eq!(r.under_floor, vec!["CALLS|fact".to_string()]);
        assert_eq!(r.strata[1].status, "ok");

        let dir = tempfile::tempdir().unwrap();
        let ids: BTreeSet<String> = ["01".to_string()].into();
        let p = dir.path().join("v.jsonl");
        std::fs::write(
            &p,
            "{\"id\":\"99\",\"verdict\":\"correct\",\"reason\":\"\",\"judge\":\"j\"}\n",
        )
        .unwrap();
        assert!(
            resolve_verdicts(&[p.clone()], &ids, None)
                .unwrap_err()
                .contains("99")
        );
        std::fs::write(
            &p,
            "{\"id\":\"01\",\"verdict\":\"correct\",\"reason\":\"\",\"judge\":\"j\"}\n\
             {\"id\":\"01\",\"verdict\":\"wrong\",\"reason\":\"\",\"judge\":\"j\"}\n",
        )
        .unwrap();
        assert!(
            resolve_verdicts(&[p.clone()], &ids, None)
                .unwrap_err()
                .contains("01")
        );
        std::fs::write(
            &p,
            "{\"id\":\"01\",\"verdict\":\"correct\",\"reason\":\"\",\"judge\":\"j\"}\n\
             {\"id\":\"01\",\"verdict\":\"wrong\",\"reason\":\"\",\"judge\":\"k\"}\n",
        )
        .unwrap();
        assert_eq!(
            resolve_verdicts(&[p.clone()], &ids, None).unwrap()["01"],
            "unsure"
        );
        assert_eq!(
            resolve_verdicts(&[p], &ids, Some("k")).unwrap()["01"],
            "wrong"
        );
    }

    #[test]
    fn sample_is_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("tree");
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        std::fs::create_dir_all(root.join("bench")).unwrap();
        std::fs::write(
            root.join("pkg/app.py"),
            "from pkg.util import helper\n\nclass Service:\n    def run(self):\n        return helper(1)\n\n    def go(self):\n        return self.run()\n",
        )
        .unwrap();
        std::fs::write(
            root.join("pkg/util.py"),
            "def helper(x):\n    return x + 1\n",
        )
        .unwrap();
        std::fs::write(
            root.join("bench/fixture.py"),
            "def f():\n    return g()\n\ndef g():\n    return 1\n",
        )
        .unwrap();
        let spec = RepoSpec {
            dir: root.to_string_lossy().into_owned(),
            label: "tree".to_string(),
            rev: "0".repeat(40),
            skips: vec!["bench/".to_string()],
        };
        let (a, ma) = sample_text(std::slice::from_ref(&spec), &plan()).unwrap();
        let (b, mb) = sample_text(std::slice::from_ref(&spec), &plan()).unwrap();
        assert_eq!(a, b);
        assert_eq!(ma, mb);
        assert!(
            ma[0].starts_with("[edge-audit] sample label=tree edges="),
            "{}",
            ma[0]
        );
        assert!(
            !a.contains("bench/fixture.py"),
            "the skip prefix leaks: {a}"
        );
        let (headers, rows) = read_samples(&[{
            let p = dir.path().join("s.jsonl");
            std::fs::write(&p, &a).unwrap();
            p
        }])
        .unwrap();
        assert_eq!(headers.len(), 1);
        assert!(!rows.is_empty());
        assert!(headers[0].excluded.get("skip").copied().unwrap_or(0) > 0);
    }
}
