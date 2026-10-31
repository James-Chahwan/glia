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
//!                   [--rejudge <glob>].. [--adjudication <file>]
//!                   [--ci stratified_wilson|plugin] [--rejudge-rule matched|fd3c]
//! ```
//!
//! The report's agreement check (FD.3c): a verdict file named `rejudge.jsonl`
//! (or any `--rejudge` file) is a second, blind judge over a fixed subset
//! ([`rejudge_set`]); the report states Cohen's kappa per family (a first
//! verdict file's stem) and overall, and where the two disagree the row of
//! `adjudication.jsonl` (default: beside the plan) replaces the first verdict.
//!
//! GP.13: the default re-judge rule is [`RejudgeRule::Matched`] (every first
//! `wrong` of a family plus as many `correct` ids, so a family's kappa is
//! defined whenever its first judge found a wrong edge), and the category / tier
//! interval is [`CiMethod::StratifiedWilson`] (never `[1, 1]` from a sample).
//! `--ci plugin --rejudge-rule fd3c` reproduces FD.3c's committed report byte
//! for byte.
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
        let stratum = stratum_key(&category, tier.as_str());
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
    wilson_p(k as f64 / n as f64, n as f64)
}

/// The Wilson score interval at proportion `p` over a (possibly fractional)
/// sample size `n > 0`; [`wilson`] is this at `k / n`, bit for bit.
fn wilson_p(p: f64, n: f64) -> (f64, f64) {
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

/// How a category's / tier's stratified interval is formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CiMethod {
    /// FD.3c's plug-in normal interval, `p +- 1.96 sqrt(var)`: a category
    /// whose judged strata all sit at p 0 or 1 reads `[p, p]`.
    Plugin,
    /// [`stratified_wilson`] (GP.13, the default).
    StratifiedWilson,
}

impl CiMethod {
    fn as_str(self) -> &'static str {
        match self {
            CiMethod::Plugin => "plugin",
            CiMethod::StratifiedWilson => "stratified_wilson",
        }
    }
    fn parse(v: &str) -> Result<CiMethod, String> {
        match v {
            "plugin" => Ok(CiMethod::Plugin),
            "stratified_wilson" => Ok(CiMethod::StratifiedWilson),
            other => Err(format!(
                "report: --ci `{other}` is not stratified_wilson|plugin"
            )),
        }
    }
}

/// The stratified estimate of [`stratified`] with a Wilson interval: the
/// Wilson score at `p` over the effective sample size `n_eff = p (1 - p) /
/// var` (the stratified variance, finite population correction included);
/// when `var` is 0 (every judged stratum at p 0 or 1, or a census), the
/// strata's own bounds weighted by `W_h`: a sampled stratum's Wilson
/// interval, a census stratum's exact `p_h`. So an all-correct category's
/// lower bound is its strata's weighted Wilson lower bound, below 1 unless
/// every stratum is a census. `None` when no stratum is judged.
fn stratified_wilson(strata: &[(u64, u64, f64)]) -> Option<(f64, f64, f64)> {
    let judged: Vec<&(u64, u64, f64)> = strata.iter().filter(|s| s.1 > 0).collect();
    let total: u64 = judged.iter().map(|s| s.0).sum();
    if total == 0 {
        return None;
    }
    let (mut p, mut var, mut wlo, mut whi) = (0.0, 0.0, 0.0, 0.0);
    for &&(nn, n, ph) in &judged {
        let w = nn as f64 / total as f64;
        p += w * ph;
        let (lo, hi) = if n < nn {
            var += w * w * ph * (1.0 - ph) / n as f64 * (1.0 - n as f64 / nn as f64);
            wilson_p(ph, n as f64)
        } else {
            (ph, ph)
        };
        wlo += w * lo;
        whi += w * hi;
    }
    if var > 0.0 {
        let (lo, hi) = wilson_p(p, p * (1.0 - p) / var);
        Some((p, lo, hi))
    } else {
        Some((p, wlo.clamp(0.0, 1.0), whi.clamp(0.0, 1.0)))
    }
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
    /// The category / tier interval ([`CiMethod`]); left out for `plugin`,
    /// FD.3c's method, whose committed report predates the field.
    #[serde(skip_serializing_if = "is_plugin")]
    ci_method: &'static str,
    strata: Vec<StratumOut>,
    categories: Vec<AggOut>,
    tiers: Vec<AggOut>,
    repos: BTreeMap<String, RepoOut>,
    under_floor: Vec<String>,
    /// The blind re-judge's agreement with the first verdicts (None without one).
    agreement: Option<AgreementOut>,
}

fn is_plugin(m: &&'static str) -> bool {
    *m == CiMethod::Plugin.as_str()
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

fn agg(name: &str, strata: &[&StratumOut], ci: CiMethod) -> AggOut {
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
    let est = match ci {
        CiMethod::Plugin => stratified(&rows),
        CiMethod::StratifiedWilson => stratified_wilson(&rows),
    };
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

/// The verdict file of the blind re-judge (FD.3c), read for agreement only.
const REJUDGE_FILE: &str = "rejudge.jsonl";
/// The disagreements' adjudication, beside the plan by default.
const ADJUDICATION_FILE: &str = "adjudication.jsonl";
/// Cohen's kappa below this marks a family LOW AGREEMENT: its strata's
/// precision is provisional until the rubric is tightened and re-judged.
const LOW_KAPPA: f64 = 0.6;
/// The re-judge takes at least this many ids per family (all of a smaller one).
const MIN_REJUDGE: usize = 20;
/// The verdicts kappa counts, `unsure` its own category.
const VERDICTS: [&str; 3] = ["correct", "wrong", "unsure"];

/// One row of `adjudication.jsonl`: a disagreement between the first verdict
/// and the blind re-judge, re-read at source. `final` replaces the first
/// verdict in the report; `reason` is read by people, not the estimate.
#[derive(Debug, Clone, Deserialize)]
struct Adjudication {
    id: String,
    first: String,
    second: String,
    #[serde(rename = "final")]
    final_verdict: String,
}

/// Agreement between the first verdicts and the blind re-judge over one family
/// (a first-verdict file's stem) or all of them.
#[derive(Debug, Clone, Serialize)]
struct KappaOut {
    name: String,
    /// The re-judge rule's set size ([`rejudge_set`]).
    expected: u64,
    rejudged: u64,
    /// Ids of the rule's set with no re-judge verdict.
    missing: u64,
    /// Those ids, sorted: the re-judge's to-do list (left out when none).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    missing_ids: Vec<String>,
    /// Re-judged ids outside the rule's set (counted in kappa all the same).
    extra: u64,
    agree: u64,
    raw_agreement: Option<f64>,
    expected_agreement: Option<f64>,
    /// None when both judges used one category only (p_e = 1).
    kappa: Option<f64>,
    /// `ok`, `low_agreement` (kappa below [`LOW_KAPPA`]), `undefined` or `none`.
    status: String,
    /// `first/second` -> count, over the verdicts present.
    matrix: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize)]
struct AgreementOut {
    /// The re-judge rule ([`RejudgeRule`]); left out for `fd3c`, whose
    /// committed report predates the field.
    #[serde(skip_serializing_if = "is_fd3c")]
    rule: &'static str,
    families: Vec<KappaOut>,
    overall: KappaOut,
    disagreements: u64,
    adjudicated: u64,
    /// Disagreements with no adjudication row: their verdict is `unsure`.
    unadjudicated: u64,
    low_agreement: Vec<String>,
}

/// What the report counts per id, and the re-judge's agreement (None when no
/// re-judge file was read, or `--judge` picked one judge).
#[derive(Debug)]
struct Resolved {
    verdicts: BTreeMap<String, String>,
    agreement: Option<AgreementOut>,
}

/// Cohen's kappa = (p_o - p_e) / (1 - p_e) over (first, second) verdict pairs,
/// the three verdicts each a category; `expected` / `missing` / `extra` are
/// the caller's.
fn kappa_of(name: &str, pairs: &[(&str, &str)]) -> KappaOut {
    let n = pairs.len() as u64;
    let mut matrix: BTreeMap<String, u64> = BTreeMap::new();
    let (mut a, mut b) = ([0u64; 3], [0u64; 3]);
    let slot = |v: &str| VERDICTS.iter().position(|c| *c == v).unwrap_or(2);
    for (f, s) in pairs {
        *matrix.entry(format!("{f}/{s}")).or_default() += 1;
        a[slot(f)] += 1;
        b[slot(s)] += 1;
    }
    let agree = pairs.iter().filter(|(f, s)| f == s).count() as u64;
    let (raw, pe, kappa) = if n == 0 {
        (None, None, None)
    } else {
        let nf = n as f64;
        let po = agree as f64 / nf;
        let pe: f64 = (0..3)
            .map(|i| (a[i] as f64 / nf) * (b[i] as f64 / nf))
            .sum();
        let k = if pe < 1.0 {
            Some(r4((po - pe) / (1.0 - pe)))
        } else {
            None
        };
        (Some(r4(po)), Some(r4(pe)), k)
    };
    let status = match (n, kappa) {
        (0, _) => "none",
        (_, None) => "undefined",
        (_, Some(k)) if k < LOW_KAPPA => "low_agreement",
        _ => "ok",
    };
    KappaOut {
        name: name.to_string(),
        expected: 0,
        rejudged: n,
        missing: 0,
        missing_ids: Vec::new(),
        extra: 0,
        agree,
        raw_agreement: raw,
        expected_agreement: pe,
        kappa,
        status: status.to_string(),
        matrix,
    }
}

/// Which ids of a family the blind re-judge takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RejudgeRule {
    /// FD.3c's rule ([`rejudge_set_fd3c`]): blind to the first verdicts, so a
    /// family whose few wrong ids miss the set has an undefined kappa.
    Fd3c,
    /// GP.13's rule (the default), stratified by first verdict
    /// ([`rejudge_set`]).
    Matched,
}

impl RejudgeRule {
    fn as_str(self) -> &'static str {
        match self {
            RejudgeRule::Fd3c => "fd3c",
            RejudgeRule::Matched => "matched",
        }
    }
    fn parse(v: &str) -> Result<RejudgeRule, String> {
        match v {
            "fd3c" => Ok(RejudgeRule::Fd3c),
            "matched" => Ok(RejudgeRule::Matched),
            other => Err(format!(
                "report: --rejudge-rule `{other}` is not matched|fd3c"
            )),
        }
    }
    /// The family's re-judge set under this rule, from its sorted ids and
    /// their first verdicts.
    fn set(self, sorted_ids: &[String], first: &BTreeMap<String, String>) -> BTreeSet<String> {
        match self {
            RejudgeRule::Fd3c => rejudge_set_fd3c(sorted_ids),
            RejudgeRule::Matched => rejudge_set(sorted_ids, first),
        }
    }
}

fn is_fd3c(r: &&'static str) -> bool {
    *r == RejudgeRule::Fd3c.as_str()
}

/// The ids a family's blind re-judge takes (GP.13), from its ids sorted by id
/// (the xxh64 key, so smallest key first) and their first verdicts: every
/// `wrong` id, then as many `correct` ids, smallest first; while the set holds
/// fewer than [`MIN_REJUDGE`], more `correct` ids, then the family's other ids
/// (`unsure`), smallest first, until it does or the family is exhausted.
fn rejudge_set(sorted_ids: &[String], first: &BTreeMap<String, String>) -> BTreeSet<String> {
    let of = |v: &str| -> Vec<&String> {
        sorted_ids
            .iter()
            .filter(|id| first.get(*id).map(String::as_str) == Some(v))
            .collect()
    };
    let (wrong, correct) = (of("wrong"), of("correct"));
    let want_correct = wrong.len().max(MIN_REJUDGE.saturating_sub(wrong.len()));
    let mut set: BTreeSet<String> = wrong.into_iter().cloned().collect();
    set.extend(correct.into_iter().take(want_correct).cloned());
    for id in sorted_ids {
        if set.len() >= MIN_REJUDGE {
            break;
        }
        if !set.contains(id) {
            set.insert(id.clone());
        }
    }
    set
}

/// FD.3c's re-judge rule (`--rejudge-rule fd3c`), from a family's sorted ids:
/// those whose last hex digit is `0`, or the first 10% by id (rounded up, at
/// least [`MIN_REJUDGE`], at most all), whichever set is larger (the first 10%
/// on a tie).
fn rejudge_set_fd3c(sorted_ids: &[String]) -> BTreeSet<String> {
    let k = sorted_ids
        .len()
        .div_ceil(10)
        .max(MIN_REJUDGE)
        .min(sorted_ids.len());
    let zeros: BTreeSet<String> = sorted_ids
        .iter()
        .filter(|i| i.ends_with('0'))
        .cloned()
        .collect();
    if zeros.len() > k {
        zeros
    } else {
        sorted_ids[..k].iter().cloned().collect()
    }
}

/// One verdict file's rows, checked: a known verdict, a sampled id.
fn read_verdicts(p: &Path, ids: &BTreeSet<String>) -> Result<Vec<VerdictRow>, String> {
    let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: VerdictRow =
            serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", p.display(), i + 1))?;
        if !VERDICTS.contains(&v.verdict.as_str()) {
            return Err(format!(
                "verdict {}: `{}` is not correct|wrong|unsure",
                v.id, v.verdict
            ));
        }
        if !ids.contains(&v.id) {
            return Err(format!("verdict {}: id is in no samples file", v.id));
        }
        out.push(v);
    }
    Ok(out)
}

/// `adjudication.jsonl`'s rows; an absent file is none.
fn read_adjudication(p: &Path) -> Result<Vec<Adjudication>, String> {
    if !p.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(
            serde_json::from_str(line).map_err(|e| format!("{}:{}: {e}", p.display(), i + 1))?,
        );
    }
    Ok(out)
}

/// The shared verdict of several judges, `unsure` when they disagree.
fn shared(per: &BTreeMap<String, String>) -> String {
    let first = per
        .values()
        .next()
        .cloned()
        .unwrap_or_else(|| "unsure".to_string());
    if per.values().all(|v| *v == first) {
        first
    } else {
        "unsure".to_string()
    }
}

/// The verdict each sample id gets. With `judge`, that judge's verdict from
/// any file (first or re-judge), no agreement. Otherwise the first verdict
/// (several first judges: their shared verdict, `unsure` when they disagree);
/// where the blind re-judge (`rejudge` files) disagrees, the adjudication's
/// `final`, else `unsure`. An id with two verdicts from one judge, a re-judged
/// id with no first verdict, or an adjudication row that does not name a real
/// disagreement is an error naming the id.
fn resolve_verdicts(
    first_paths: &[PathBuf],
    rejudge_paths: &[PathBuf],
    adjudication: &[Adjudication],
    ids: &BTreeSet<String>,
    judge: Option<&str>,
    rule: RejudgeRule,
) -> Result<Resolved, String> {
    // id -> judge -> verdict, and each first id's family (its file's stem).
    let mut first: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut second: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut family: BTreeMap<String, String> = BTreeMap::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let files = first_paths
        .iter()
        .map(|p| (p, false))
        .chain(rejudge_paths.iter().map(|p| (p, true)));
    for (p, is_second) in files {
        let stem = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        for v in read_verdicts(p, ids)? {
            if judge.is_some_and(|j| j != v.judge) {
                continue;
            }
            if !seen.insert((v.id.clone(), v.judge.clone())) {
                return Err(format!(
                    "verdict {}: two verdicts from judge `{}`",
                    v.id, v.judge
                ));
            }
            if is_second && judge.is_none() {
                second.entry(v.id).or_default().insert(v.judge, v.verdict);
            } else {
                family.entry(v.id.clone()).or_insert_with(|| stem.clone());
                first.entry(v.id).or_default().insert(v.judge, v.verdict);
            }
        }
    }
    let mut verdicts: BTreeMap<String, String> = first
        .iter()
        .map(|(id, per)| (id.clone(), shared(per)))
        .collect();
    if judge.is_some() || rejudge_paths.is_empty() {
        return Ok(Resolved {
            verdicts,
            agreement: None,
        });
    }
    let second: BTreeMap<String, String> = second
        .iter()
        .map(|(id, per)| (id.clone(), shared(per)))
        .collect();
    if let Some(id) = second.keys().find(|id| !verdicts.contains_key(*id)) {
        return Err(format!("re-judge {id}: id has no first verdict"));
    }
    let mut adj: BTreeMap<&str, &Adjudication> = BTreeMap::new();
    for a in adjudication {
        let (Some(f), Some(s)) = (verdicts.get(&a.id), second.get(&a.id)) else {
            return Err(format!(
                "adjudication {}: id has no first and re-judge verdict",
                a.id
            ));
        };
        if *f != a.first || *s != a.second || f == s {
            return Err(format!(
                "adjudication {}: first/second `{}/{}` is not the disagreement `{f}/{s}`",
                a.id, a.first, a.second
            ));
        }
        if !VERDICTS.contains(&a.final_verdict.as_str()) {
            return Err(format!(
                "adjudication {}: final `{}` is not correct|wrong|unsure",
                a.id, a.final_verdict
            ));
        }
        if adj.insert(a.id.as_str(), a).is_some() {
            return Err(format!("adjudication {}: two rows", a.id));
        }
    }
    // Agreement per family, before any adjudication replaces a verdict.
    let mut by_family: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (id, f) in &family {
        by_family.entry(f.as_str()).or_default().push(id.clone());
    }
    let (mut families, mut all_pairs) = (Vec::new(), Vec::new());
    let (mut expected, mut missing, mut extra) = (0, 0, 0);
    for (name, fam_ids) in &by_family {
        let want = rule.set(fam_ids, &verdicts);
        let pairs: Vec<(&str, &str)> = fam_ids
            .iter()
            .filter_map(|id| Some((verdicts[id].as_str(), second.get(id)?.as_str())))
            .collect();
        let mut k = kappa_of(name, &pairs);
        k.expected = want.len() as u64;
        k.missing_ids = want
            .iter()
            .filter(|id| !second.contains_key(*id))
            .cloned()
            .collect();
        k.missing = k.missing_ids.len() as u64;
        k.extra = fam_ids
            .iter()
            .filter(|id| second.contains_key(*id) && !want.contains(*id))
            .count() as u64;
        (expected, missing, extra) = (expected + k.expected, missing + k.missing, extra + k.extra);
        all_pairs.extend(pairs);
        families.push(k);
    }
    let mut overall = kappa_of("overall", &all_pairs);
    (overall.expected, overall.missing, overall.extra) = (expected, missing, extra);
    let disagreements = all_pairs.iter().filter(|(f, s)| f != s).count() as u64;
    let (mut adjudicated, mut unadjudicated) = (0, 0);
    for (id, s) in &second {
        let Some(v) = verdicts.get_mut(id) else {
            continue;
        };
        if v == s {
            continue;
        }
        match adj.get(id.as_str()) {
            Some(a) => {
                adjudicated += 1;
                *v = a.final_verdict.clone();
            }
            None => {
                unadjudicated += 1;
                *v = "unsure".to_string();
            }
        }
    }
    let low_agreement = families
        .iter()
        .chain(std::iter::once(&overall))
        .filter(|k| k.status == "low_agreement")
        .map(|k| k.name.clone())
        .collect();
    Ok(Resolved {
        verdicts,
        agreement: Some(AgreementOut {
            rule: rule.as_str(),
            families,
            overall,
            disagreements,
            adjudicated,
            unadjudicated,
            low_agreement,
        }),
    })
}

fn build_report(
    headers: &[Header],
    rows: &[SampleRow],
    verdicts: &BTreeMap<String, String>,
    plan: &Plan,
    ci: CiMethod,
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
                ci,
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
                ci,
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
        let overall = agg(&h.label, &rs.iter().collect::<Vec<_>>(), ci);
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
        ci_method: ci.as_str(),
        strata,
        categories,
        tiers,
        repos,
        under_floor,
        agreement: None,
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

/// A kappa as `0.400`, `undefined` (one category) or `-` (nothing re-judged).
fn kappa_text(k: &KappaOut) -> String {
    match (k.kappa, k.status.as_str()) {
        (Some(v), _) => format!("{v:.3}"),
        (None, "undefined") => "undefined".to_string(),
        _ => "-".to_string(),
    }
}

fn agreement_section(md: &mut String, a: &AgreementOut) {
    md.push_str("## Agreement (blind re-judge)\n\n");
    let rule = if a.rule == RejudgeRule::Matched.as_str() {
        format!(
            "every id its first judge called `wrong` and as many of its `correct` ids, smallest \
             id first (topped up to at least {MIN_REJUDGE}, `correct` first), without reading \
             the first verdicts; kappa is over that verdict-stratified subset, not the family's \
             verdict mix"
        )
    } else {
        format!(
            "ids ending in hex `0` or its first 10% by id (whichever is larger, at least \
             {MIN_REJUDGE}) without reading the first verdicts"
        )
    };
    md.push_str(&format!(
        "A second judge re-judged each family's {rule}. \
         Cohen's kappa is over the three verdicts, `unsure` its own category; it is undefined \
         when both judges used one verdict only. A family below {LOW_KAPPA} is LOW AGREEMENT \
         and its strata's precision is provisional. A disagreement is re-read at source and \
         its adjudicated verdict replaces the first one; an unadjudicated one counts as \
         `unsure`.\n\n"
    ));
    md.push_str(&format!(
        "kappa: overall {} (raw agreement {}, {} ids); {}\n\n",
        kappa_text(&a.overall),
        f3(a.overall.raw_agreement),
        a.overall.rejudged,
        a.families
            .iter()
            .map(|k| format!("{} {}", k.name, kappa_text(k)))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    md.push_str(
        "| family | rule set | re-judged | missing | extra | agree | raw agreement | expected agreement | kappa | status |\n",
    );
    md.push_str("|---|---:|---:|---:|---:|---:|---:|---:|---:|---|\n");
    for k in a.families.iter().chain(std::iter::once(&a.overall)) {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            k.name,
            k.expected,
            k.rejudged,
            k.missing,
            k.extra,
            k.agree,
            f3(k.raw_agreement),
            f3(k.expected_agreement),
            kappa_text(k),
            if k.status == "low_agreement" {
                "LOW AGREEMENT"
            } else {
                k.status.as_str()
            }
        ));
    }
    md.push_str(&format!(
        "\ndisagreements {}, adjudicated {}, unadjudicated {}\n\n",
        a.disagreements, a.adjudicated, a.unadjudicated
    ));
    for f in &a.low_agreement {
        md.push_str(&format!(
            "LOW AGREEMENT: `{f}`: its strata's precision is provisional.\n\n"
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
         population correction (a census stratum adds no variance){}. A stratum is under floor \
         when p is below its tier's floor; insufficient when fewer than {} rows are judged \
         (a census excepted); noisy when unsure is over {}% of its verdicts. Floors: {}.\n\n",
        r.seed,
        if r.ci_method == CiMethod::StratifiedWilson.as_str() {
            "; its 95% CI (`ci_method=stratified_wilson`) is the Wilson score over the effective \
             sample size p (1 - p) / var, or, when var is 0 (every judged stratum at p 0 or 1), \
             the strata's Wilson bounds weighted N_h / N (a census stratum's exact p_h)"
        } else {
            ""
        },
        MIN_JUDGED,
        (NOISY_SHARE * 100.0) as u32,
        r.floors
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if let Some(a) = &r.agreement {
        agreement_section(&mut md, a);
    }
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
    let (mut samples, mut verdicts, mut rejudge) = (Vec::new(), Vec::new(), Vec::new());
    let (mut plan, mut out_json, mut out_md, mut judge, mut check) =
        (None, None, None, None, false);
    let (mut ci, mut rule) = (CiMethod::StratifiedWilson, RejudgeRule::Matched);
    let mut adjudication: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--samples" => samples.extend(expand(&value(args, &mut i, flag)?)?),
            "--verdicts" => verdicts.extend(expand(&value(args, &mut i, flag)?)?),
            "--rejudge" => rejudge.extend(expand(&value(args, &mut i, flag)?)?),
            "--adjudication" => adjudication = Some(value(args, &mut i, flag)?),
            "--plan" => plan = Some(value(args, &mut i, flag)?),
            "--out-json" => out_json = Some(value(args, &mut i, flag)?),
            "--out-md" => out_md = Some(value(args, &mut i, flag)?),
            "--judge" => judge = Some(value(args, &mut i, flag)?),
            "--check" => check = true,
            "--ci" => ci = CiMethod::parse(&value(args, &mut i, flag)?)?,
            "--rejudge-rule" => rule = RejudgeRule::parse(&value(args, &mut i, flag)?)?,
            other => return Err(format!("report: unknown argument {other}")),
        }
        i += 1;
    }
    let plan_path = plan.ok_or("report: --plan is required")?;
    let plan = Plan::read(&plan_path)?;
    // A verdict file named `rejudge.jsonl` is the blind re-judge, never a first
    // verdict; the adjudication defaults to `adjudication.jsonl` beside the plan.
    let (second, first): (Vec<PathBuf>, Vec<PathBuf>) = verdicts
        .into_iter()
        .partition(|p| p.file_name().is_some_and(|n| n == REJUDGE_FILE));
    let (verdicts, rejudge) = (first, [rejudge, second].concat());
    let adjudication = adjudication.map(PathBuf::from).unwrap_or_else(|| {
        Path::new(&plan_path)
            .parent()
            .unwrap_or(Path::new("."))
            .join(ADJUDICATION_FILE)
    });
    let (out_json, out_md) = (
        out_json.ok_or("report: --out-json is required")?,
        out_md.ok_or("report: --out-md is required")?,
    );
    let inputs = ReportInputs {
        samples,
        verdicts,
        rejudge,
        adjudication,
        judge,
        ci,
        rule,
    };
    let (report, json, md) = report_texts(inputs, &plan)?;
    eprintln!("{}", report_marker(&report));
    if let Some(a) = &report.agreement {
        for f in &a.low_agreement {
            eprintln!("[edge-audit] LOW AGREEMENT family={f}: its precision is provisional");
        }
    }
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

/// What `report` reads, its files expanded.
struct ReportInputs {
    samples: Vec<PathBuf>,
    verdicts: Vec<PathBuf>,
    rejudge: Vec<PathBuf>,
    adjudication: PathBuf,
    judge: Option<String>,
    ci: CiMethod,
    rule: RejudgeRule,
}

/// The report and its JSON and markdown texts, from the samples, verdicts,
/// re-judge, adjudication and plan.
fn report_texts(mut inp: ReportInputs, plan: &Plan) -> Result<(ReportOut, String, String), String> {
    inp.samples.sort();
    inp.samples.dedup();
    inp.verdicts.sort();
    inp.verdicts.dedup();
    inp.rejudge.sort();
    inp.rejudge.dedup();
    if inp.samples.is_empty() {
        return Err("report: no samples file matched".to_string());
    }
    let (headers, rows) = read_samples(&inp.samples)?;
    let mut ids = BTreeSet::new();
    for r in &rows {
        if !ids.insert(r.id.clone()) {
            return Err(format!("sample {}: id appears twice", r.id));
        }
    }
    let adjudication = read_adjudication(&inp.adjudication)?;
    let resolved = resolve_verdicts(
        &inp.verdicts,
        &inp.rejudge,
        &adjudication,
        &ids,
        inp.judge.as_deref(),
        inp.rule,
    )?;
    let mut report = build_report(&headers, &rows, &resolved.verdicts, plan, inp.ci);
    report.agreement = resolved.agreement;
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())? + "\n";
    let md = render_md(&report);
    Ok((report, json, md))
}

/// The report's fired_on line.
fn report_marker(report: &ReportOut) -> String {
    format!(
        "[edge-audit] report strata={} judged={} unsure={} under_floor={} insufficient={} kappa={} ci_method={}",
        report.strata.len(),
        report.strata.iter().map(|s| s.n).sum::<u64>(),
        report.strata.iter().map(|s| s.unsure).sum::<u64>(),
        report.under_floor.len(),
        report
            .strata
            .iter()
            .filter(|s| s.status == "insufficient")
            .count(),
        report
            .agreement
            .as_ref()
            .map_or("-".to_string(), |a| kappa_text(&a.overall)),
        report.ci_method
    )
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
        let r = build_report(&[header], &rows, &verdicts, &plan(), CiMethod::Plugin);
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
            resolve_verdicts(
                std::slice::from_ref(&p),
                &[],
                &[],
                &ids,
                None,
                RejudgeRule::Matched
            )
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
            resolve_verdicts(
                std::slice::from_ref(&p),
                &[],
                &[],
                &ids,
                None,
                RejudgeRule::Matched
            )
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
            resolve_verdicts(
                std::slice::from_ref(&p),
                &[],
                &[],
                &ids,
                None,
                RejudgeRule::Matched
            )
            .unwrap()
            .verdicts["01"],
            "unsure"
        );
        assert_eq!(
            resolve_verdicts(&[p], &[], &[], &ids, Some("k"), RejudgeRule::Matched)
                .unwrap()
                .verdicts["01"],
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

    #[test]
    fn kappa_identical_and_textbook() {
        // Identical verdicts over two categories: kappa 1.0.
        let same = [
            ("correct", "correct"),
            ("wrong", "wrong"),
            ("correct", "correct"),
            ("unsure", "unsure"),
        ];
        let k = kappa_of("t", &same);
        assert_eq!(k.kappa, Some(1.0));
        assert_eq!(k.raw_agreement, Some(1.0));
        assert_eq!(k.status, "ok");
        // The textbook 2x2 (50 items: yes/yes 20, yes/no 5, no/yes 10,
        // no/no 15): p_o = .7, p_e = .5*.6 + .5*.4 = .5, kappa = .4.
        let mut t = Vec::new();
        t.extend(std::iter::repeat_n(("correct", "correct"), 20));
        t.extend(std::iter::repeat_n(("correct", "wrong"), 5));
        t.extend(std::iter::repeat_n(("wrong", "correct"), 10));
        t.extend(std::iter::repeat_n(("wrong", "wrong"), 15));
        let k = kappa_of("t", &t);
        assert_eq!(k.raw_agreement, Some(0.7));
        assert_eq!(k.expected_agreement, Some(0.5));
        assert_eq!(k.kappa, Some(0.4));
        assert_eq!(k.status, "low_agreement");
        assert_eq!(k.matrix["correct/wrong"], 5);
        // Unsure is its own category: one unsure against correct disagrees.
        let k = kappa_of("t", &[("correct", "unsure"), ("wrong", "wrong")]);
        assert_eq!(k.agree, 1);
        // One category on both sides: p_e = 1, kappa undefined.
        let k = kappa_of("t", &[("correct", "correct"), ("correct", "correct")]);
        assert_eq!(k.kappa, None);
        assert_eq!(k.status, "undefined");
        assert_eq!(kappa_of("t", &[]).status, "none");
    }

    #[test]
    fn rejudge_set_fd3c_takes_the_larger_rule() {
        let ids = |n: usize| -> Vec<String> { (0..n).map(|i| format!("{i:04x}")).collect() };
        // 299 ids, every 16th ends in 0 (19 of them): the first max(30, 20) win.
        let a = ids(299);
        let s = rejudge_set_fd3c(&a);
        assert_eq!(s.len(), 30);
        assert!(s.contains("0000") && s.contains("001d") && !s.contains("001e"));
        // 50 ids: 10% is 5, the floor 20 wins over the 4 ending in 0.
        assert_eq!(rejudge_set_fd3c(&ids(50)).len(), 20);
        // 1000 ids ending 0 only on every 16th = 63 < 100: the first 100.
        assert_eq!(rejudge_set_fd3c(&ids(1000)).len(), 100);
        // When more ids end in 0 than the first 10%, the 0-ending ids win.
        let zeros: Vec<String> = (0..40).map(|i| format!("{i:03x}0")).collect();
        let mut mixed = zeros.clone();
        mixed.extend((0..20).map(|i| format!("{i:03x}1")));
        mixed.sort();
        assert_eq!(rejudge_set_fd3c(&mixed), zeros.into_iter().collect());
        // A family smaller than 20 is re-judged whole.
        assert_eq!(rejudge_set_fd3c(&ids(7)).len(), 7);
    }

    #[test]
    fn adjudication_replaces_the_first_verdict() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("structure.jsonl");
        let second = dir.path().join("rejudge.jsonl");
        let line = |id: &str, v: &str, j: &str| {
            format!("{{\"id\":\"{id}\",\"verdict\":\"{v}\",\"reason\":\"\",\"judge\":\"{j}\"}}\n")
        };
        std::fs::write(
            &first,
            [
                line("01", "correct", "a"),
                line("02", "wrong", "a"),
                line("03", "correct", "a"),
            ]
            .concat(),
        )
        .unwrap();
        std::fs::write(
            &second,
            [
                line("01", "correct", "b"),
                line("02", "correct", "b"),
                line("03", "wrong", "b"),
            ]
            .concat(),
        )
        .unwrap();
        let ids: BTreeSet<String> = ["01", "02", "03"].iter().map(|s| s.to_string()).collect();
        let adj = vec![Adjudication {
            id: "02".to_string(),
            first: "wrong".to_string(),
            second: "correct".to_string(),
            final_verdict: "correct".to_string(),
        }];
        let r = resolve_verdicts(
            std::slice::from_ref(&first),
            std::slice::from_ref(&second),
            &adj,
            &ids,
            None,
            RejudgeRule::Matched,
        )
        .unwrap();
        // 01 agrees; 02 is adjudicated; 03 disagrees unadjudicated: unsure.
        assert_eq!(r.verdicts["01"], "correct");
        assert_eq!(r.verdicts["02"], "correct");
        assert_eq!(r.verdicts["03"], "unsure");
        let a = r.agreement.unwrap();
        assert_eq!((a.disagreements, a.adjudicated, a.unadjudicated), (2, 1, 1));
        assert_eq!(a.families.len(), 1);
        assert_eq!(a.families[0].name, "structure");
        assert_eq!(a.families[0].agree, 1);
        // An adjudication whose `first` is not the first verdict is an error.
        let bad = vec![Adjudication {
            id: "02".to_string(),
            first: "correct".to_string(),
            second: "correct".to_string(),
            final_verdict: "wrong".to_string(),
        }];
        assert!(
            resolve_verdicts(
                std::slice::from_ref(&first),
                std::slice::from_ref(&second),
                &bad,
                &ids,
                None,
                RejudgeRule::Matched
            )
            .unwrap_err()
            .contains("02")
        );
        // A re-judged id with no first verdict is an error.
        std::fs::write(&second, line("04", "correct", "b")).unwrap();
        let ids4: BTreeSet<String> = ["01", "02", "03", "04"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(
            resolve_verdicts(
                std::slice::from_ref(&first),
                std::slice::from_ref(&second),
                &[],
                &ids4,
                None,
                RejudgeRule::Matched
            )
            .unwrap_err()
            .contains("04")
        );
    }

    #[test]
    fn rejudge_set_takes_every_wrong_and_matched_corrects() {
        let ids: Vec<String> = (0..300).map(|i| format!("{i:016x}")).collect();
        // 14 wrong (every 20th id from 5), 2 unsure, the rest correct.
        let first: BTreeMap<String, String> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let v = if i % 20 == 5 && i < 285 {
                    "wrong"
                } else if i == 1 || i == 2 {
                    "unsure"
                } else {
                    "correct"
                };
                (id.clone(), v.to_string())
            })
            .collect();
        let wrong: BTreeSet<String> = first
            .iter()
            .filter(|(_, v)| *v == "wrong")
            .map(|(id, _)| id.clone())
            .collect();
        assert_eq!(wrong.len(), 14);
        let s = rejudge_set(&ids, &first);
        // Every wrong plus 14 corrects, the smallest ids: 0, 3, 4, 6, ..
        assert_eq!(s.len(), 28);
        assert!(wrong.is_subset(&s));
        let corrects: Vec<&String> = s.iter().filter(|id| first[*id] == "correct").collect();
        assert_eq!(corrects.len(), 14);
        let want: Vec<&String> = ids
            .iter()
            .filter(|id| first[*id] == "correct")
            .take(14)
            .collect();
        assert_eq!(corrects, want);
        assert!(!s.contains(&ids[1]) && !s.contains(&ids[2]));
        // Deterministic.
        assert_eq!(rejudge_set(&ids, &first), s);
        // 3 wrong: topped up to 20 with the 17 smallest corrects.
        let few: BTreeMap<String, String> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let v = if [7, 100, 200].contains(&i) {
                    "wrong"
                } else {
                    "correct"
                };
                (id.clone(), v.to_string())
            })
            .collect();
        let s = rejudge_set(&ids, &few);
        assert_eq!(s.len(), 20);
        assert!(s.contains(&ids[200]) && s.contains(&ids[17]) && !s.contains(&ids[18]));
        // A family with too few corrects is topped up from its unsure ids.
        let small: Vec<String> = ids[..12].to_vec();
        let mixed: BTreeMap<String, String> = small
            .iter()
            .enumerate()
            .map(|(i, id)| {
                let v = match i % 3 {
                    0 => "wrong",
                    1 => "correct",
                    _ => "unsure",
                };
                (id.clone(), v.to_string())
            })
            .collect();
        assert_eq!(rejudge_set(&small, &mixed).len(), 12);
        // All correct, no wrong: the 20 smallest.
        let all: BTreeMap<String, String> = ids
            .iter()
            .map(|id| (id.clone(), "correct".to_string()))
            .collect();
        let s = rejudge_set(&ids, &all);
        assert_eq!(s, ids[..20].iter().cloned().collect());
    }

    #[test]
    fn stratified_wilson_is_not_one_at_p_one() {
        // Two all-correct sampled strata, W = .25 / .75: var is 0, so the
        // bounds are the strata's Wilson bounds weighted by W.
        let (p, lo, hi) = stratified_wilson(&[(100, 10, 1.0), (300, 30, 1.0)]).unwrap();
        let want = 0.25 * wilson(10, 10).0 + 0.75 * wilson(30, 30).0;
        assert!((p - 1.0).abs() < 1e-12);
        assert!((lo - want).abs() < 1e-12, "{lo} vs {want}");
        assert!(lo < 1.0);
        assert!((hi - 1.0).abs() < 1e-12);
        assert_eq!(format!("{lo:.4}"), "0.8455");
        // The plug-in interval it replaces reads [1, 1].
        assert_eq!(
            stratified(&[(100, 10, 1.0), (300, 30, 1.0)]),
            Some((1.0, 1.0, 1.0))
        );
        // One sampled stratum: exactly its own Wilson interval.
        let (_, lo, hi) = stratified_wilson(&[(96729, 32, 1.0)]).unwrap();
        assert_eq!((lo, hi), wilson(32, 32));
        assert_eq!(format!("{lo:.4}"), "0.8928");
        // A census stratum is exact: its p_h, not its Wilson bound.
        let (_, lo, _) = stratified_wilson(&[(20, 20, 1.0), (60, 30, 1.0)]).unwrap();
        assert!((lo - (0.25 + 0.75 * wilson(30, 30).0)).abs() < 1e-12);
        assert_eq!(stratified_wilson(&[(20, 20, 0.9)]), Some((0.9, 0.9, 0.9)));
        // var > 0: the Wilson score over n_eff = p (1 - p) / var.
        let rows = [(100, 100, 0.9), (300, 30, 0.8)];
        let (p, lo, hi) = stratified_wilson(&rows).unwrap();
        let var = 0.75f64.powi(2) * 0.8 * 0.2 / 30.0 * 0.9;
        let (wl, wh) = wilson_p(0.825, 0.825 * 0.175 / var);
        assert!((p - 0.825).abs() < 1e-12);
        assert!((lo - wl).abs() < 1e-12 && (hi - wh).abs() < 1e-12);
        assert!(lo > 0.0 && hi < 1.0 && lo < p && p < hi);
        assert!(stratified_wilson(&[(5, 0, 0.0)]).is_none());
        // wilson is wilson_p at k / n, bit for bit.
        assert_eq!(wilson(27, 30), wilson_p(27.0 / 30.0, 30.0));
    }

    /// The committed FD.3c audit (bench/edge-audit/) read as `report --check`
    /// reads it, under the given CI method and re-judge rule.
    fn committed(ci: CiMethod, rule: RejudgeRule) -> (ReportOut, String, String) {
        let ad = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/edge-audit");
        let all = expand(&ad.join("verdicts/*.jsonl").to_string_lossy()).unwrap();
        let (rejudge, verdicts): (Vec<PathBuf>, Vec<PathBuf>) = all
            .into_iter()
            .partition(|p| p.file_name().is_some_and(|n| n == REJUDGE_FILE));
        let plan = Plan::read(&ad.join("floors.json").to_string_lossy()).unwrap();
        let inputs = ReportInputs {
            samples: expand(&ad.join("samples/*.jsonl").to_string_lossy()).unwrap(),
            verdicts,
            rejudge,
            adjudication: ad.join(ADJUDICATION_FILE),
            judge: None,
            ci,
            rule,
        };
        report_texts(inputs, &plan).unwrap()
    }

    #[test]
    fn plugin_fd3c_reproduces_the_committed_report() {
        let ad = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/edge-audit");
        let (report, json, md) = committed(CiMethod::Plugin, RejudgeRule::Fd3c);
        assert!(
            json == std::fs::read_to_string(ad.join("report.json")).unwrap(),
            "report.json drifted under --ci plugin --rejudge-rule fd3c"
        );
        assert!(
            md == std::fs::read_to_string(ad.join("report.md")).unwrap(),
            "report.md drifted under --ci plugin --rejudge-rule fd3c"
        );
        assert!(
            report_marker(&report).ends_with(" kappa=1.000 ci_method=plugin"),
            "{}",
            report_marker(&report)
        );
    }

    #[test]
    fn default_report_reads_the_weakest_bounds_and_the_matched_set() {
        let (report, json, md) = committed(CiMethod::StratifiedWilson, RejudgeRule::Matched);
        let calls = report
            .categories
            .iter()
            .find(|c| c.name == "CALLS")
            .unwrap();
        assert_eq!((calls.p, calls.hi), (Some(1.0), Some(1.0)));
        assert_eq!(calls.lo, Some(0.8928));
        // Every sampled (not census) category reads a lower bound below 1; a
        // census category (INJECTS, GRPC_CALLS, ..) is exact.
        let sampled: Vec<&AggOut> = report.categories.iter().filter(|c| c.n < c.big_n).collect();
        assert!(sampled.len() >= 8);
        assert!(sampled.iter().all(|c| c.lo.is_some_and(|lo| lo < 1.0)));
        assert!(json.contains("\"ci_method\": \"stratified_wilson\""));
        assert!(json.contains("\"rule\": \"matched\""));
        assert!(md.contains("ci_method=stratified_wilson"));
        assert!(report_marker(&report).ends_with(" ci_method=stratified_wilson"));
        // Structure's first judge found 14 wrong edges: the matched set takes
        // all of them and 14 corrects, so the re-judge it names is 28 ids.
        let a = report.agreement.unwrap();
        let s = a.families.iter().find(|k| k.name == "structure").unwrap();
        assert_eq!(s.expected, 28);
        assert_eq!(s.missing, s.missing_ids.len() as u64);
        let m = a.families.iter().find(|k| k.name == "mechanism").unwrap();
        assert_eq!(m.expected, 102);
    }
}
