//! The edge-precision audit (FD.2, seed §F item 12): a sampled, stratified
//! audit of the built graph's edges per (edge category x evidence tier),
//! judged against source. `bench/edge-audit/README.md` is the method,
//! `RUBRIC.md` what makes an edge correct; `scripts/edge-audit.sh` drives it.
//!
//! ```text
//! edge_audit sample --repo <dir> --label <l> --rev <sha> [--skip <prefix>].. [--repo ..]..
//!                   --plan bench/edge-audit/floors.json --out <samples.jsonl>
//!                   [--panel fit|heldout]
//! edge_audit show   <samples.jsonl> --root <dir> [--label <l>] [--id <hex>]
//! edge_audit report --samples <glob>.. --verdicts <glob>.. --plan <floors.json>
//!                   --out-json <f> --out-md <f> [--judge <name>] [--check]
//!                   [--rejudge <glob>].. [--adjudication <file>]
//!                   [--ci stratified_wilson|plugin] [--rejudge-rule matched|fd3c]
//! edge_audit report --calibration --fit <report.json> [--fit-samples <glob>]..
//!                   [--fit-verdicts <glob>].. [--fit-adjudication <file>]
//!                   --samples <glob>.. --verdicts <glob>.. [--adjudication <file>]
//!                   [--cohort-tasks <tasks.jsonl>] --plan <floors.json>
//!                   --out-json <f> --out-md <f> [--check]
//!                   [--emit-table [<table.json>]]
//! ```
//!
//! Calibrated confidence (HC.2): `sample --panel heldout` draws the held-out
//! panel with the fit's key, seed and exclusions at [`HELDOUT_N`] per pooled
//! stratum, every header marked `"panel":"heldout"`; the fit report reads the
//! fit panel only ([`split_panel`], [`resolve_verdicts_scoped`]), so held-out
//! samples and verdicts beside it never move it. `report --calibration` judges
//! the held-out strata against the fit report by the rule committed in
//! [`CAL_RULE`] ([`compare_strata`]: K, R1, R2, R3) and prints
//! `[edge-audit] calibration ..` and `CONFIDENCE: .. -> PASS|FAIL|INSUFFICIENT`.
//!
//! HC.4: on that rule's PASS, `report --calibration --emit-table [<path>]`
//! (default `engine/src/confidence/table.json` beside the plan's checkout)
//! writes the precision table `glia_engine::confidence` reads at query time
//! ([`precision_table`]): every (category, tier) stratum and every
//! (category, tier, emitter/rule) sub-stratum pooled over the fit and the
//! held-out panel's judged ids, kept when it holds at least [`CAL_MIN_JUDGED`]
//! of them or is a census, each with its Wilson 95% interval and an
//! `under_floor` flag (pooled p below its tier's floor), plus the fit
//! report's per-tier estimates; `audit` is the first 12 hex of the blake3 of
//! the calibration.json text it was generated with. With `--check` the
//! committed table must equal what the inputs generate, as calibration.json
//! and .md must. `[edge-audit] table strata=<s> sub_strata=<b> tiers=<t>
//! under_floor=<f> audit=<12 hex> path=<p>`.
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
//! and `glia query` print, so a pairing that rests on a model-written overlay
//! stanza (HC.1: rule `<base>/overlay_llm`) is sampled in its category's
//! heuristic stratum, its row naming the full rule; the audited snapshots
//! carry no overlay, so no committed sample moves. Selection is deterministic: each candidate edge's
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
/// The held-out panel's name in a samples header (HC.2).
const PANEL_HELDOUT: &str = "heldout";
/// The held-out sample's n per pooled (category, tier) stratum (HC.2).
const HELDOUT_N: u64 = 10;

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
    /// The panel the label belongs to (HC.2): absent for the fit panel (so the
    /// fit's committed samples keep their bytes), [`PANEL_HELDOUT`] for the
    /// calibration's held-out repos. A label belongs to one panel only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    panel: Option<String>,
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
        panel: None,
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
/// rows sorted by (category, tier, key)) and the per-repo marker lines. A
/// `panel` (HC.2: [`PANEL_HELDOUT`]) is written into every header and named on
/// every marker; the fit panel writes none, so its bytes are FD.2's.
fn sample_text(
    specs: &[RepoSpec],
    plan: &Plan,
    panel: Option<&str>,
) -> Result<(String, Vec<String>), String> {
    let mut pops = Vec::with_capacity(specs.len());
    for spec in specs {
        let mut pop = population(spec, &plan.seed)?;
        pop.header.panel = panel.map(str::to_string);
        pops.push(pop);
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
            "[edge-audit] sample label={} edges={} strata={} sampled={} unjudgeable={} excluded={} seed={}{}",
            p.header.label,
            p.header.edges,
            p.header.stratum_sizes.len(),
            rows.len(),
            p.header.unjudgeable.values().sum::<u64>(),
            p.header.excluded.values().sum::<u64>(),
            plan.seed,
            panel.map_or(String::new(), |p| format!(" panel={p}"))
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
///
/// Scoped to one panel (HC.2): every verdict and adjudication row must name a
/// `known` id (any panel's), and only the rows whose id is in `scope` are read,
/// so one verdict directory serves the fit report and the held-out calibration
/// without either seeing the other's ids.
fn resolve_verdicts_scoped(
    first_paths: &[PathBuf],
    rejudge_paths: &[PathBuf],
    adjudication: &[Adjudication],
    known: &BTreeSet<String>,
    scope: &BTreeSet<String>,
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
        for v in read_verdicts(p, known)? {
            if !scope.contains(&v.id) || judge.is_some_and(|j| j != v.judge) {
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
        if known.contains(&a.id) && !scope.contains(&a.id) {
            continue;
        }
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
    let mut panel: Option<String> = None;
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
            "--panel" => {
                let v = value(args, &mut i, flag)?;
                panel = match v.as_str() {
                    "fit" => None,
                    PANEL_HELDOUT => Some(v),
                    other => {
                        return Err(format!("sample: --panel `{other}` is not fit|heldout"));
                    }
                };
            }
            other => return Err(format!("sample: unknown argument {other}")),
        }
        i += 1;
    }
    let mut plan = Plan::read(&plan.ok_or("sample: --plan is required")?)?;
    if panel.is_some() {
        // The held-out panel (HC.2): HELDOUT_N per pooled stratum, structural
        // categories too; key, seed and exclusions are the fit's.
        plan.n_per_stratum = HELDOUT_N;
        plan.n_structural = HELDOUT_N;
    }
    let out = out.ok_or("sample: --out is required")?;
    if specs.is_empty() || specs.iter().any(|s| s.label.is_empty() || s.rev.is_empty()) {
        return Err("sample: every --repo needs --label and --rev".to_string());
    }
    let (text, markers) = sample_text(&specs, &plan, panel.as_deref())?;
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
    if args.iter().any(|a| a == "--calibration") {
        return run_calibration(args);
    }
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
    // The fit report reads the fit panel only (HC.2): held-out samples and
    // their verdicts may sit beside it, and never enter it.
    let split = split_panel(headers, rows, None)?;
    let (headers, rows) = (split.headers, split.rows);
    let adjudication = read_adjudication(&inp.adjudication)?;
    let resolved = resolve_verdicts_scoped(
        &inp.verdicts,
        &inp.rejudge,
        &adjudication,
        &split.known,
        &split.scope,
        inp.judge.as_deref(),
        inp.rule,
    )?;
    let mut report = build_report(&headers, &rows, &resolved.verdicts, plan, inp.ci);
    report.agreement = resolved.agreement;
    let json = serde_json::to_string_pretty(&report).map_err(|e| e.to_string())? + "\n";
    let md = render_md(&report);
    Ok((report, json, md))
}

/// One panel's samples out of every samples file read.
#[derive(Debug)]
struct PanelSplit {
    /// The panel's headers and rows.
    headers: Vec<Header>,
    rows: Vec<SampleRow>,
    /// Every sampled id of any panel: a verdict naming one is no error.
    known: BTreeSet<String>,
    /// The panel's ids: the verdicts read.
    scope: BTreeSet<String>,
}

/// The headers and rows of one panel (`None` = the fit panel, a header with
/// no `panel`), every id checked unique across all panels; a row belongs to
/// its label's panel, and a label with headers in two panels is an error.
fn split_panel(
    headers: Vec<Header>,
    rows: Vec<SampleRow>,
    want: Option<&str>,
) -> Result<PanelSplit, String> {
    let mut panel_of: BTreeMap<String, Option<String>> = BTreeMap::new();
    for h in &headers {
        if let Some(prev) = panel_of.insert(h.label.clone(), h.panel.clone())
            && prev != h.panel
        {
            return Err(format!("sample label {}: headers in two panels", h.label));
        }
    }
    let mut known = BTreeSet::new();
    for r in &rows {
        if !known.insert(r.id.clone()) {
            return Err(format!("sample {}: id appears twice", r.id));
        }
    }
    let in_panel = |label: &str| panel_of.get(label).cloned().flatten().as_deref() == want;
    let headers: Vec<Header> = headers
        .into_iter()
        .filter(|h| h.panel.as_deref() == want)
        .collect();
    let rows: Vec<SampleRow> = rows.into_iter().filter(|r| in_panel(&r.label)).collect();
    let scope = rows.iter().map(|r| r.id.clone()).collect();
    Ok(PanelSplit {
        headers,
        rows,
        known,
        scope,
    })
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

// ------------------------------------------------------- calibration (HC.2)

/// The rule the calibration judges by, committed before any held-out id was
/// sampled or judged.
const CAL_RULE: &str = "dev-notes/confidence-gate-0.5.7.md";
/// R1: the held-out Wilson upper bound reaches the fit's Wilson lower bound
/// on at least `CAL_R1_NUM / CAL_R1_DEN` of K (compared in integers).
const CAL_R1_NUM: usize = 9;
const CAL_R1_DEN: usize = 10;
/// R2: the ceiling on the held-out-weighted calibration error.
const CAL_MAX_ERROR: f64 = 0.05;
/// R3: K holds at least this many strata, else INSUFFICIENT.
const CAL_MIN_STRATA: usize = 15;
/// A stratum enters K, and a sub-stratum the comparison, with this many
/// judged ids on its held-out side (a sub-stratum on its fit side too).
const CAL_MIN_JUDGED: u64 = 10;
/// The largest |p_fit - p_held| strata of K the report names.
const CAL_GAPS: usize = 5;

/// The fit report's rows the calibration reads (report.json, any version).
#[derive(Debug, Clone, Deserialize)]
struct FitStratum {
    category: String,
    tier: String,
    /// The fit panel's population of the stratum (HC.4: a census reads
    /// `n == N`); absent in a report that predates the field.
    #[serde(rename = "N", default)]
    big_n: Option<u64>,
    n: u64,
    correct: u64,
}

/// A fit report's per-tier estimate, re-printed beside the held-out one.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct FitAgg {
    name: String,
    n: u64,
    p: Option<f64>,
    lo: Option<f64>,
    hi: Option<f64>,
}

#[derive(Debug, Clone, Deserialize)]
struct FitReport {
    strata: Vec<FitStratum>,
    #[serde(default)]
    tiers: Vec<FitAgg>,
}

/// One side's judged ids of a stratum: `n` = correct + wrong.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Judged {
    correct: u64,
    n: u64,
}

impl Judged {
    fn add(&mut self, verdict: Option<&str>) {
        match verdict {
            Some("correct") => {
                self.correct += 1;
                self.n += 1;
            }
            Some("wrong") => self.n += 1,
            _ => {}
        }
    }
    fn p(self) -> Option<f64> {
        (self.n > 0).then(|| self.correct as f64 / self.n as f64)
    }
    /// The Wilson 95% interval, a census included (`None` when unjudged).
    fn interval(self) -> Option<(f64, f64)> {
        (self.n > 0).then(|| wilson(self.correct, self.n))
    }
}

/// One stratum's comparison input: its key, the held-out pooled population
/// `N_h` (None for a sub-stratum), the fit side (None when the fit never saw
/// it) and the held-out side.
#[derive(Debug, Clone)]
struct CalIn {
    key: String,
    big_n: Option<u64>,
    fit: Option<Judged>,
    held: Judged,
}

/// One compared stratum (or sub-stratum) of the calibration.
#[derive(Debug, Clone, Serialize)]
struct CalRow {
    stratum: String,
    /// The held-out pooled population; absent for a sub-stratum.
    #[serde(rename = "N", skip_serializing_if = "Option::is_none")]
    big_n: Option<u64>,
    fit_n: u64,
    fit_correct: u64,
    fit_p: Option<f64>,
    fit_lo: Option<f64>,
    fit_hi: Option<f64>,
    heldout_n: u64,
    heldout_correct: u64,
    heldout_p: Option<f64>,
    heldout_lo: Option<f64>,
    heldout_hi: Option<f64>,
    /// In K (a stratum) or compared (a sub-stratum: both sides n >= 10).
    compared: bool,
    /// R1 on this row: the held-out upper bound reaches the fit's lower bound.
    holds: Option<bool>,
    /// |p_fit - p_held|.
    diff: Option<f64>,
    /// w_h = N_h / sum over K of N_h (strata of K only).
    weight: Option<f64>,
}

/// The rule's reading of K.
#[derive(Debug, Clone, Serialize)]
struct GateOut {
    k: usize,
    covered: usize,
    fit_n: u64,
    heldout_n: u64,
    cal_error: f64,
    r1: bool,
    r2: bool,
    r3: bool,
    verdict: &'static str,
}

/// Compare `ins` (strata when `gated`, else sub-strata) and, for strata,
/// judge K by R1-R3. A stratum is in K with >= [`CAL_MIN_JUDGED`] held-out
/// judged ids and a fit p; a sub-stratum is compared with >= that many on
/// both sides. Rows keep `ins`' order; the gate reads full precision, the
/// rows print 4 decimals.
fn compare_strata(ins: &[CalIn], gated: bool) -> (Vec<CalRow>, GateOut) {
    let min_fit = if gated { 1 } else { CAL_MIN_JUDGED };
    let in_k = |c: &CalIn| c.held.n >= CAL_MIN_JUDGED && c.fit.is_some_and(|f| f.n >= min_fit);
    let mass: u64 = ins.iter().filter(|c| in_k(c)).filter_map(|c| c.big_n).sum();
    let (mut k, mut covered, mut fit_n, mut held_n, mut err) = (0usize, 0usize, 0u64, 0u64, 0.0);
    let mut rows = Vec::with_capacity(ins.len());
    for c in ins {
        let fit = c.fit.unwrap_or_default();
        let (flo, fhi) = fit.interval().unzip();
        let (hlo, hhi) = c.held.interval().unzip();
        let compared = in_k(c);
        let (mut holds, mut diff, mut weight) = (None, None, None);
        if compared
            && let (Some(pf), Some(ph), Some(lo), Some(hi)) = (fit.p(), c.held.p(), flo, hhi)
        {
            let d = (pf - ph).abs();
            holds = Some(hi >= lo);
            diff = Some(r4(d));
            if gated {
                let w = match c.big_n {
                    Some(nh) if mass > 0 => nh as f64 / mass as f64,
                    _ => 0.0,
                };
                weight = Some(r4(w));
                err += w * d;
                k += 1;
                covered += usize::from(hi >= lo);
                fit_n += fit.n;
                held_n += c.held.n;
            }
        }
        rows.push(CalRow {
            stratum: c.key.clone(),
            big_n: c.big_n,
            fit_n: fit.n,
            fit_correct: fit.correct,
            fit_p: fit.p().map(r4),
            fit_lo: flo.map(r4),
            fit_hi: fhi.map(r4),
            heldout_n: c.held.n,
            heldout_correct: c.held.correct,
            heldout_p: c.held.p().map(r4),
            heldout_lo: hlo.map(r4),
            heldout_hi: hhi.map(r4),
            compared,
            holds,
            diff,
            weight,
        });
    }
    let r1 = covered * CAL_R1_DEN >= CAL_R1_NUM * k;
    let r2 = err <= CAL_MAX_ERROR;
    let r3 = k >= CAL_MIN_STRATA;
    let verdict = if !r3 {
        "INSUFFICIENT"
    } else if r1 && r2 {
        "PASS"
    } else {
        "FAIL"
    };
    let gate = GateOut {
        k,
        covered,
        fit_n,
        heldout_n: held_n,
        cal_error: err,
        r1,
        r2,
        r3,
        verdict,
    };
    (rows, gate)
}

/// The two fired_on lines of a calibration.
fn calibration_markers(g: &GateOut) -> [String; 2] {
    let yn = |b: bool| if b { "yes" } else { "no" };
    [
        format!(
            "[edge-audit] calibration strata={} fit_n={} heldout_n={} covered={}/{} cal_error={:.4}",
            g.k, g.fit_n, g.heldout_n, g.covered, g.k, g.cal_error
        ),
        format!(
            "CONFIDENCE: R1={} ({}/{}) R2={} ({:.4}) R3={} ({}) -> {}",
            yn(g.r1),
            g.covered,
            g.k,
            yn(g.r2),
            g.cal_error,
            yn(g.r3),
            g.k,
            g.verdict
        ),
    ]
}

/// A sub-stratum's key: `CATEGORY|tier|emitter/rule` (`-` for a missing one).
fn sub_key(r: &SampleRow) -> String {
    format!(
        "{}|{}|{}/{}",
        r.category,
        r.tier,
        r.emitter.as_deref().unwrap_or("-"),
        r.rule.as_deref().unwrap_or("-")
    )
}

/// Judged counts by stratum or sub-stratum key.
type JudgedBy = BTreeMap<String, Judged>;

/// Judged counts per (category, tier) and per sub-stratum.
fn judged_by(rows: &[SampleRow], verdicts: &BTreeMap<String, String>) -> (JudgedBy, JudgedBy) {
    let (mut strata, mut subs) = (JudgedBy::new(), JudgedBy::new());
    for r in rows {
        let v = verdicts.get(&r.id).map(String::as_str);
        strata
            .entry(stratum_key(&r.category, &r.tier))
            .or_default()
            .add(v);
        subs.entry(sub_key(r)).or_default().add(v);
    }
    (strata, subs)
}

/// One repo's answer-level precision for the cohort's name-drift rows.
#[derive(Debug, Clone, Serialize)]
struct DriftRow {
    repo: String,
    heldout: bool,
    tasks: u64,
    flagged: u64,
    truth: u64,
    p: Option<f64>,
    lo: Option<f64>,
    hi: Option<f64>,
}

/// The cohort's name-drift answer precision from its tasks file: per repo,
/// over the drift tasks whose start state ran the names probe (`S.names`
/// with `flagged`), flagged files that are truth files / flagged files; the
/// repos sorted, then one `all` row.
fn name_drift(text: &str, heldout: &BTreeSet<String>) -> Result<Vec<DriftRow>, String> {
    let mut per: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: serde_json::Value =
            serde_json::from_str(line).map_err(|e| format!("cohort tasks:{}: {e}", i + 1))?;
        if v.get("shape").and_then(|s| s.as_str()) != Some("drift") {
            continue;
        }
        let names = v.get("S").and_then(|s| s.get("names"));
        let Some(flagged) = names
            .and_then(|n| n.get("flagged"))
            .and_then(|f| f.as_u64())
        else {
            continue;
        };
        let truth = names
            .and_then(|n| n.get("flagged_truth"))
            .and_then(|t| t.as_array())
            .map_or(0, |t| t.len() as u64);
        let repo = v
            .get("repo")
            .and_then(|r| r.as_str())
            .unwrap_or("?")
            .to_string();
        let e = per.entry(repo).or_default();
        *e = (e.0 + 1, e.1 + flagged, e.2 + truth);
    }
    let row = |repo: &str, (tasks, flagged, truth): (u64, u64, u64)| {
        let j = Judged {
            correct: truth,
            n: flagged,
        };
        let (lo, hi) = j.interval().unzip();
        DriftRow {
            repo: repo.to_string(),
            heldout: heldout.contains(repo),
            tasks,
            flagged,
            truth,
            p: j.p().map(r4),
            lo: lo.map(r4),
            hi: hi.map(r4),
        }
    };
    let mut out: Vec<DriftRow> = per.iter().map(|(r, c)| row(r, *c)).collect();
    let all = per
        .values()
        .fold((0, 0, 0), |a, c| (a.0 + c.0, a.1 + c.1, a.2 + c.2));
    out.push(row("all", all));
    Ok(out)
}

/// One held-out repo's line.
#[derive(Debug, Serialize)]
struct RepoCal {
    edges: u64,
    unjudgeable: u64,
    sampled: u64,
    n: u64,
    correct: u64,
    wrong: u64,
    unsure: u64,
    p: Option<f64>,
    lo: Option<f64>,
    hi: Option<f64>,
}

#[derive(Debug, Serialize)]
struct FitSummary {
    /// The fit report's strata and judged ids.
    strata: usize,
    judged: u64,
    tiers: Vec<FitAgg>,
}

#[derive(Debug, Serialize)]
struct HeldSummary {
    repos: Vec<String>,
    strata: usize,
    sampled: u64,
    judged: u64,
    unsure: u64,
    pending: u64,
    tiers: Vec<AggOut>,
    /// Held-out strata under their tier's floor (each a precision-backlog
    /// follow-up on any verdict).
    under_floor: Vec<String>,
}

#[derive(Debug, Serialize)]
struct CalOut {
    rule: &'static str,
    seed: String,
    heldout_n_per_stratum: u64,
    fit: FitSummary,
    heldout: HeldSummary,
    gate: GateOut,
    strata: Vec<CalRow>,
    /// Strata of K whose held-out upper bound misses the fit's lower bound.
    not_transferred: Vec<String>,
    /// The strata of K with the largest |p_fit - p_held| (ties by key).
    largest_gaps: Vec<String>,
    repos: BTreeMap<String, RepoCal>,
    sub_strata: Vec<CalRow>,
    /// The held-out panel's re-judge agreement (None without a re-judge file).
    agreement: Option<AgreementOut>,
    name_drift: Option<Vec<DriftRow>>,
    /// HC.4: the fit and held-out judged ids per stratum and sub-stratum,
    /// the input of [`precision_table`]; never in calibration.json.
    #[serde(skip)]
    pooled: Pooled,
}

/// One stratum's (or sub-stratum's) judged ids on both panels, and whether
/// each panel judged every edge of it (HC.4).
#[derive(Debug, Clone, Default)]
struct PoolIn {
    key: String,
    tier: String,
    fit: Judged,
    held: Judged,
    census: bool,
}

/// [`precision_table`]'s input, gathered by [`calibration_texts`].
#[derive(Debug, Clone, Default)]
struct Pooled {
    strata: Vec<PoolIn>,
    subs: Vec<PoolIn>,
    fit_tiers: Vec<FitAgg>,
}

/// One row of the precision table (HC.4).
#[derive(Debug, Serialize)]
struct TableRow {
    stratum: String,
    n: u64,
    correct: u64,
    p: f64,
    lo: f64,
    hi: f64,
    census: bool,
    under_floor: bool,
    fit_n: u64,
    heldout_n: u64,
}

/// One tier's estimate in the precision table: the fit report's.
#[derive(Debug, Serialize)]
struct TableTier {
    tier: String,
    n: u64,
    p: Option<f64>,
    lo: Option<f64>,
    hi: Option<f64>,
    under_floor: bool,
}

#[derive(Debug, Serialize)]
struct TableOut {
    audit: String,
    source: &'static str,
    rule: &'static str,
    seed: String,
    min_judged: u64,
    floors: BTreeMap<String, f64>,
    tiers: Vec<TableTier>,
    strata: Vec<TableRow>,
    sub_strata: Vec<TableRow>,
}

/// The calibration.json path the table's `audit` digest names.
const TABLE_SOURCE: &str = "bench/edge-audit/calibration.json";
/// Where `--emit-table` writes by default, relative to the checkout that
/// holds the plan (`<root>/bench/edge-audit/floors.json`).
const TABLE_PATH: &str = "engine/src/confidence/table.json";

/// The first 12 hex of the blake3 of `calibration_json`: the audit a
/// precision row names.
fn audit_digest(calibration_json: &str) -> String {
    blake3::hash(calibration_json.as_bytes()).to_hex()[..12].to_string()
}

/// One pooled row, when it holds [`CAL_MIN_JUDGED`] judged ids or is a census.
fn table_row(p: &PoolIn, plan: &Plan) -> Option<TableRow> {
    let correct = p.fit.correct + p.held.correct;
    let n = p.fit.n + p.held.n;
    if n == 0 || (n < CAL_MIN_JUDGED && !p.census) {
        return None;
    }
    let (lo, hi) = wilson(correct, n);
    let pr = correct as f64 / n as f64;
    let under_floor = plan
        .floors
        .get(&p.tier.to_ascii_lowercase())
        .is_some_and(|f| pr < *f);
    Some(TableRow {
        stratum: p.key.clone(),
        n,
        correct,
        p: r4(pr),
        lo: r4(lo),
        hi: r4(hi),
        census: p.census,
        under_floor,
        fit_n: p.fit.n,
        heldout_n: p.held.n,
    })
}

/// The precision table's text (HC.4; module doc): the pooled strata and
/// sub-strata by key, the fit report's tiers, the audit digest of
/// `calibration_json`.
fn precision_table(cal: &CalOut, calibration_json: &str, plan: &Plan) -> Result<String, String> {
    let rows = |v: &[PoolIn]| v.iter().filter_map(|p| table_row(p, plan)).collect();
    let tiers = cal
        .pooled
        .fit_tiers
        .iter()
        .map(|t| TableTier {
            tier: t.name.clone(),
            n: t.n,
            p: t.p,
            lo: t.lo,
            hi: t.hi,
            under_floor: match (t.p, plan.floors.get(&t.name)) {
                (Some(p), Some(f)) => p < *f,
                _ => false,
            },
        })
        .collect();
    let out = TableOut {
        audit: audit_digest(calibration_json),
        source: TABLE_SOURCE,
        rule: CAL_RULE,
        seed: plan.seed.clone(),
        min_judged: CAL_MIN_JUDGED,
        floors: plan.floors.clone(),
        tiers,
        strata: rows(&cal.pooled.strata),
        sub_strata: rows(&cal.pooled.subs),
    };
    Ok(serde_json::to_string_pretty(&out).map_err(|e| e.to_string())? + "\n")
}

/// The table's fired_on line.
fn table_marker(table: &str, path: &str) -> String {
    let v: serde_json::Value = serde_json::from_str(table).unwrap_or_default();
    let len = |k: &str| v[k].as_array().map_or(0, Vec::len);
    let under = ["strata", "sub_strata"]
        .iter()
        .flat_map(|k| v[*k].as_array().into_iter().flatten())
        .filter(|r| r["under_floor"] == true)
        .count();
    format!(
        "[edge-audit] table strata={} sub_strata={} tiers={} under_floor={under} audit={} path={path}",
        len("strata"),
        len("sub_strata"),
        len("tiers"),
        v["audit"].as_str().unwrap_or("-"),
    )
}

/// What `report --calibration` reads, its files expanded.
struct CalInputs {
    /// The fit report (report.json at the rule's sha).
    fit: PathBuf,
    /// The fit's samples, verdicts and adjudication at that sha, for the
    /// sub-strata (they must reproduce the fit report's strata).
    fit_samples: Vec<PathBuf>,
    fit_verdicts: Vec<PathBuf>,
    fit_adjudication: Option<PathBuf>,
    /// Every samples file of the tree (the held-out panel is read from it).
    samples: Vec<PathBuf>,
    verdicts: Vec<PathBuf>,
    adjudication: PathBuf,
    cohort_tasks: Option<PathBuf>,
}

fn sorted(mut v: Vec<PathBuf>) -> Vec<PathBuf> {
    v.sort();
    v.dedup();
    v
}

/// Verdict files split into first verdicts and the blind re-judge.
fn split_rejudge(paths: Vec<PathBuf>) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (second, first): (Vec<PathBuf>, Vec<PathBuf>) = paths
        .into_iter()
        .partition(|p| p.file_name().is_some_and(|n| n == REJUDGE_FILE));
    (first, second)
}

/// The calibration and its JSON and markdown texts.
fn calibration_texts(inp: CalInputs, plan: &Plan) -> Result<(CalOut, String, String), String> {
    let fit_text = std::fs::read_to_string(&inp.fit)
        .map_err(|e| format!("calibration: fit {}: {e}", inp.fit.display()))?;
    let fit: FitReport = serde_json::from_str(&fit_text)
        .map_err(|e| format!("calibration: fit {}: {e}", inp.fit.display()))?;
    let fit_strata: JudgedBy = fit
        .strata
        .iter()
        .map(|s| {
            (
                stratum_key(&s.category, &s.tier),
                Judged {
                    correct: s.correct,
                    n: s.n,
                },
            )
        })
        .collect();

    // The fit's sub-strata, from its own samples and verdicts, checked to
    // reproduce the fit report's strata.
    let mut fit_subs = JudgedBy::new();
    let fit_samples = sorted(inp.fit_samples);
    if !fit_samples.is_empty() {
        let (h, r) = read_samples(&fit_samples)?;
        let split = split_panel(h, r, None)?;
        let (first, second) = split_rejudge(sorted(inp.fit_verdicts));
        let adj = match &inp.fit_adjudication {
            Some(p) => read_adjudication(p)?,
            None => Vec::new(),
        };
        let resolved = resolve_verdicts_scoped(
            &first,
            &second,
            &adj,
            &split.known,
            &split.scope,
            None,
            RejudgeRule::Matched,
        )?;
        let (strata, subs) = judged_by(&split.rows, &resolved.verdicts);
        for (key, want) in &fit_strata {
            let have = strata.get(key).copied().unwrap_or_default();
            if have != *want {
                return Err(format!(
                    "calibration: the fit samples + verdicts do not reproduce the fit report at {key}: {}/{} judged correct, the report {}/{}",
                    have.correct, have.n, want.correct, want.n
                ));
            }
        }
        if let Some(extra) = strata
            .iter()
            .find(|(k, j)| j.n > 0 && !fit_strata.contains_key(*k))
        {
            return Err(format!(
                "calibration: the fit samples judge {} the fit report does not hold",
                extra.0
            ));
        }
        fit_subs = subs;
    }

    // The held-out panel, out of every samples file of the tree.
    let samples = sorted(inp.samples);
    if samples.is_empty() {
        return Err("calibration: no samples file matched".to_string());
    }
    let (h, r) = read_samples(&samples)?;
    let split = split_panel(h, r, Some(PANEL_HELDOUT))?;
    if split.headers.is_empty() {
        return Err(
            "calibration: no held-out samples (run `scripts/edge-audit.sh sample-heldout`)"
                .to_string(),
        );
    }
    let (first, second) = split_rejudge(sorted(inp.verdicts));
    let adj = read_adjudication(&inp.adjudication)?;
    let resolved = resolve_verdicts_scoped(
        &first,
        &second,
        &adj,
        &split.known,
        &split.scope,
        None,
        RejudgeRule::Matched,
    )?;
    let held = build_report(
        &split.headers,
        &split.rows,
        &resolved.verdicts,
        plan,
        CiMethod::StratifiedWilson,
    );
    let (held_strata, held_subs) = judged_by(&split.rows, &resolved.verdicts);

    // Strata: every key either side holds, by key.
    let mut keys: BTreeSet<String> = fit_strata.keys().cloned().collect();
    let held_n: BTreeMap<String, u64> = held
        .strata
        .iter()
        .map(|s| (stratum_key(&s.category, &s.tier), s.big_n))
        .collect();
    keys.extend(held_n.keys().cloned());
    let ins: Vec<CalIn> = keys
        .iter()
        .map(|k| CalIn {
            key: k.clone(),
            big_n: Some(held_n.get(k).copied().unwrap_or(0)),
            fit: fit_strata.get(k).copied(),
            held: held_strata.get(k).copied().unwrap_or_default(),
        })
        .collect();
    let (strata, gate) = compare_strata(&ins, true);
    let not_transferred = strata
        .iter()
        .filter(|r| r.holds == Some(false))
        .map(|r| r.stratum.clone())
        .collect();
    let mut gaps: Vec<&CalRow> = strata.iter().filter(|r| r.compared).collect();
    gaps.sort_by(|a, b| {
        b.diff
            .unwrap_or(0.0)
            .total_cmp(&a.diff.unwrap_or(0.0))
            .then(a.stratum.cmp(&b.stratum))
    });
    let largest_gaps = gaps
        .iter()
        .take(CAL_GAPS)
        .filter(|r| r.diff.is_some_and(|d| d > 0.0))
        .map(|r| r.stratum.clone())
        .collect();

    // Sub-strata compared where both sides hold n >= 10.
    let sub_keys: BTreeSet<&String> = fit_subs.keys().chain(held_subs.keys()).collect();
    let sub_ins: Vec<CalIn> = sub_keys
        .into_iter()
        .map(|k| CalIn {
            key: k.clone(),
            big_n: None,
            fit: fit_subs.get(k).copied(),
            held: held_subs.get(k).copied().unwrap_or_default(),
        })
        .collect();
    let (subs, _) = compare_strata(&sub_ins, false);
    let sub_strata = subs.into_iter().filter(|r| r.compared).collect();

    // HC.4: both panels' judged ids per key, for the precision table. A panel
    // censuses a stratum when it judged every edge of it (a panel without the
    // stratum holds none to judge); a sub-stratum is a census when its
    // stratum is.
    let fit_census: BTreeMap<String, bool> = fit
        .strata
        .iter()
        .map(|s| (stratum_key(&s.category, &s.tier), s.big_n == Some(s.n)))
        .collect();
    let census_of = |k: &str| {
        let held = held_strata.get(k).copied().unwrap_or_default();
        fit_census.get(k).copied().unwrap_or(true) && held_n.get(k).copied().unwrap_or(0) == held.n
    };
    let tier_of = |k: &str| k.split('|').nth(1).unwrap_or("").to_string();
    let parent = |k: &str| k.splitn(3, '|').take(2).collect::<Vec<_>>().join("|");
    let pooled = Pooled {
        strata: keys
            .iter()
            .map(|k| PoolIn {
                key: k.clone(),
                tier: tier_of(k),
                fit: fit_strata.get(k).copied().unwrap_or_default(),
                held: held_strata.get(k).copied().unwrap_or_default(),
                census: census_of(k),
            })
            .collect(),
        subs: sub_ins
            .iter()
            .map(|c| PoolIn {
                key: c.key.clone(),
                tier: tier_of(&c.key),
                fit: c.fit.unwrap_or_default(),
                held: c.held,
                census: census_of(&parent(&c.key)),
            })
            .collect(),
        fit_tiers: fit.tiers.clone(),
    };

    let heldout_labels: BTreeSet<String> = split.headers.iter().map(|h| h.label.clone()).collect();
    let repos = held
        .repos
        .iter()
        .map(|(label, r)| {
            let sum = |f: fn(&StratumOut) -> u64| r.strata.iter().map(f).sum::<u64>();
            (
                label.clone(),
                RepoCal {
                    edges: r.edges,
                    unjudgeable: r.unjudgeable,
                    sampled: sum(|s| s.sampled),
                    n: sum(|s| s.n),
                    correct: sum(|s| s.correct),
                    wrong: sum(|s| s.wrong),
                    unsure: sum(|s| s.unsure),
                    p: r.overall.p,
                    lo: r.overall.lo,
                    hi: r.overall.hi,
                },
            )
        })
        .collect();
    let name_drift = match &inp.cohort_tasks {
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .map_err(|e| format!("calibration: cohort tasks {}: {e}", p.display()))?;
            Some(name_drift(&text, &heldout_labels)?)
        }
        None => None,
    };
    let out = CalOut {
        rule: CAL_RULE,
        seed: plan.seed.clone(),
        heldout_n_per_stratum: HELDOUT_N,
        fit: FitSummary {
            strata: fit.strata.len(),
            judged: fit.strata.iter().map(|s| s.n).sum(),
            tiers: fit.tiers.clone(),
        },
        heldout: HeldSummary {
            repos: heldout_labels.into_iter().collect(),
            strata: held.strata.len(),
            sampled: held.strata.iter().map(|s| s.sampled).sum(),
            judged: held.strata.iter().map(|s| s.n).sum(),
            unsure: held.strata.iter().map(|s| s.unsure).sum(),
            pending: held.strata.iter().map(|s| s.pending).sum(),
            tiers: held.tiers,
            under_floor: held.under_floor,
        },
        gate: GateOut {
            cal_error: r4(gate.cal_error),
            ..gate.clone()
        },
        strata,
        not_transferred,
        largest_gaps,
        repos,
        sub_strata,
        agreement: resolved.agreement,
        name_drift,
        pooled,
    };
    let json = serde_json::to_string_pretty(&out).map_err(|e| e.to_string())? + "\n";
    let md = render_cal_md(&out, &gate);
    Ok((CalOut { gate, ..out }, json, md))
}

fn yes_no(b: Option<bool>) -> &'static str {
    match b {
        Some(true) => "yes",
        Some(false) => "no",
        None => "-",
    }
}

fn cal_table(md: &mut String, rows: &[CalRow], population: bool) {
    md.push_str(if population {
        "| stratum | N held-out | fit k/n | fit p [lo, hi] | held-out k/n | held-out p [lo, hi] | in K | R1 holds | abs diff | w |\n|---|---:|---:|---|---:|---|---|---|---:|---:|\n"
    } else {
        "| sub-stratum | fit k/n | fit p [lo, hi] | held-out k/n | held-out p [lo, hi] | R1 holds | abs diff |\n|---|---:|---|---:|---|---|---:|\n"
    });
    for r in rows {
        let iv = |p, lo, hi| format!("{} [{}, {}]", f3(p), f3(lo), f3(hi));
        let fit = iv(r.fit_p, r.fit_lo, r.fit_hi);
        let held = iv(r.heldout_p, r.heldout_lo, r.heldout_hi);
        // A stratum key's `|` would split the cell.
        let key = r.stratum.replace('|', "\\|");
        if population {
            md.push_str(&format!(
                "| {} | {} | {}/{} | {} | {}/{} | {} | {} | {} | {} | {} |\n",
                key,
                r.big_n.unwrap_or(0),
                r.fit_correct,
                r.fit_n,
                fit,
                r.heldout_correct,
                r.heldout_n,
                held,
                if r.compared { "yes" } else { "no" },
                yes_no(r.holds),
                f3(r.diff),
                f3(r.weight)
            ));
        } else {
            md.push_str(&format!(
                "| {} | {}/{} | {} | {}/{} | {} | {} | {} |\n",
                key,
                r.fit_correct,
                r.fit_n,
                fit,
                r.heldout_correct,
                r.heldout_n,
                held,
                yes_no(r.holds),
                f3(r.diff)
            ));
        }
    }
}

/// calibration.md: the verdict, the strata, the information rows.
fn render_cal_md(c: &CalOut, g: &GateOut) -> String {
    let [marker, verdict] = calibration_markers(g);
    let mut md = String::new();
    md.push_str("# Calibrated confidence: the held-out calibration (HC.2 / HC.3)\n\n");
    md.push_str(&format!(
        "Rule: `{}` (committed before any held-out id was judged). Fit: the 0.5.6 audit, {} strata, {} judged. Held-out: {} ({} strata, {} sampled at n {} per stratum, {} judged, {} unsure, {} pending). Seed `{}`.\n\n",
        c.rule,
        c.fit.strata,
        c.fit.judged,
        c.heldout.repos.join(", "),
        c.heldout.strata,
        c.heldout.sampled,
        c.heldout_n_per_stratum,
        c.heldout.judged,
        c.heldout.unsure,
        c.heldout.pending,
        c.seed
    ));
    md.push_str(&format!("```\n{marker}\n{verdict}\n```\n\n"));
    md.push_str(&format!(
        "K = {} strata with >= {CAL_MIN_JUDGED} held-out judged ids and a fit p. R1: the held-out Wilson upper bound reaches the fit's Wilson lower bound on {}/{} (needs >= {CAL_R1_NUM}/{CAL_R1_DEN}). R2: the held-out-weighted mean |p_fit - p_held| is {:.4} (needs <= {CAL_MAX_ERROR}). R3: |K| = {} (needs >= {CAL_MIN_STRATA}).\n\n",
        g.k, g.covered, g.k, g.cal_error, g.k
    ));
    md.push_str("## Strata\n\n");
    cal_table(&mut md, &c.strata, true);
    md.push_str(&format!(
        "\nNot transferred (R1 fails): {}.\nLargest gaps in K: {}.\nHeld-out strata under their tier floor: {}.\n\n",
        list_or_none(&c.not_transferred),
        list_or_none(&c.largest_gaps),
        list_or_none(&c.heldout.under_floor)
    ));
    md.push_str("## Per tier (information)\n\n| tier | fit n | fit p [lo, hi] | held-out n | held-out p [lo, hi] |\n|---|---:|---|---:|---|\n");
    let tiers: BTreeSet<&str> = c
        .fit
        .tiers
        .iter()
        .map(|t| t.name.as_str())
        .chain(c.heldout.tiers.iter().map(|t| t.name.as_str()))
        .collect();
    for t in tiers {
        let f = c.fit.tiers.iter().find(|x| x.name == t);
        let h = c.heldout.tiers.iter().find(|x| x.name == t);
        md.push_str(&format!(
            "| {t} | {} | {} [{}, {}] | {} | {} [{}, {}] |\n",
            f.map_or(0, |x| x.n),
            f3(f.and_then(|x| x.p)),
            f3(f.and_then(|x| x.lo)),
            f3(f.and_then(|x| x.hi)),
            h.map_or(0, |x| x.n),
            f3(h.and_then(|x| x.p)),
            f3(h.and_then(|x| x.lo)),
            f3(h.and_then(|x| x.hi)),
        ));
    }
    md.push_str("\n## Per held-out repo (information)\n\n| repo | edges | unjudgeable | sampled | judged | wrong | unsure | p [lo, hi] |\n|---|---:|---:|---:|---:|---:|---:|---|\n");
    for (label, r) in &c.repos {
        md.push_str(&format!(
            "| {label} | {} | {} | {} | {} | {} | {} | {} [{}, {}] |\n",
            r.edges,
            r.unjudgeable,
            r.sampled,
            r.n,
            r.wrong,
            r.unsure,
            f3(r.p),
            f3(r.lo),
            f3(r.hi)
        ));
    }
    md.push_str(&format!(
        "\n## Sub-strata by emitter / rule (information: both sides n >= {CAL_MIN_JUDGED})\n\n"
    ));
    if c.sub_strata.is_empty() {
        md.push_str("None compared.\n");
    } else {
        cal_table(&mut md, &c.sub_strata, false);
    }
    md.push_str("\n## Held-out re-judge agreement (information)\n\n");
    match &c.agreement {
        Some(a) => {
            md.push_str(&format!(
                "Overall: {} ({} re-judged of {} expected, {} missing); disagreements {}, adjudicated {}, unadjudicated {}.\n",
                kappa_text(&a.overall),
                a.overall.rejudged,
                a.overall.expected,
                a.overall.missing,
                a.disagreements,
                a.adjudicated,
                a.unadjudicated
            ));
            for k in &a.families {
                md.push_str(&format!(
                    "- {}: {} ({} re-judged of {} expected)\n",
                    k.name,
                    kappa_text(k),
                    k.rejudged,
                    k.expected
                ));
            }
        }
        None => md.push_str("No re-judge file read.\n"),
    }
    if let Some(rows) = &c.name_drift {
        md.push_str("\n## Name-drift answer precision (information: the cohort's names probe)\n\nFlagged files that are truth files / flagged files, over the drift tasks whose start ran the names probe.\n\n| repo | held-out | tasks | flagged | truth | p [lo, hi] |\n|---|---|---:|---:|---:|---|\n");
        for r in rows {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} [{}, {}] |\n",
                r.repo,
                if r.heldout { "yes" } else { "no" },
                r.tasks,
                r.flagged,
                r.truth,
                f3(r.p),
                f3(r.lo),
                f3(r.hi)
            ));
        }
    }
    md
}

fn list_or_none(v: &[String]) -> String {
    if v.is_empty() {
        "none".to_string()
    } else {
        v.join(", ")
    }
}

/// `report --calibration`: the held-out panel judged against the fit by the
/// committed rule ([`CAL_RULE`]).
fn run_calibration(args: &[String]) -> Result<ExitCode, String> {
    let (mut fit, mut fit_samples, mut fit_verdicts, mut fit_adjudication) =
        (None, Vec::new(), Vec::new(), None);
    let (mut samples, mut verdicts, mut adjudication) = (Vec::new(), Vec::new(), None);
    let (mut plan, mut out_json, mut out_md, mut cohort_tasks, mut check) =
        (None, None, None, None, false);
    let mut emit_table: Option<Option<String>> = None;
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        match flag {
            "--calibration" => {}
            "--fit" => fit = Some(PathBuf::from(value(args, &mut i, flag)?)),
            "--fit-samples" => fit_samples.extend(expand(&value(args, &mut i, flag)?)?),
            "--fit-verdicts" => fit_verdicts.extend(expand(&value(args, &mut i, flag)?)?),
            "--fit-adjudication" => {
                fit_adjudication = Some(PathBuf::from(value(args, &mut i, flag)?))
            }
            "--samples" | "--heldout" => samples.extend(expand(&value(args, &mut i, flag)?)?),
            "--verdicts" => verdicts.extend(expand(&value(args, &mut i, flag)?)?),
            "--adjudication" => adjudication = Some(value(args, &mut i, flag)?),
            "--cohort-tasks" => cohort_tasks = Some(PathBuf::from(value(args, &mut i, flag)?)),
            "--plan" => plan = Some(value(args, &mut i, flag)?),
            "--out-json" => out_json = Some(value(args, &mut i, flag)?),
            "--out-md" => out_md = Some(value(args, &mut i, flag)?),
            "--check" => check = true,
            "--emit-table" => {
                emit_table = Some(match args.get(i + 1).filter(|a| !a.starts_with("--")) {
                    Some(p) => {
                        i += 1;
                        Some(p.clone())
                    }
                    None => None,
                })
            }
            other => return Err(format!("report --calibration: unknown argument {other}")),
        }
        i += 1;
    }
    let plan_path = plan.ok_or("report --calibration: --plan is required")?;
    let plan = Plan::read(&plan_path)?;
    let adjudication = adjudication.map(PathBuf::from).unwrap_or_else(|| {
        Path::new(&plan_path)
            .parent()
            .unwrap_or(Path::new("."))
            .join(ADJUDICATION_FILE)
    });
    let (out_json, out_md) = (
        out_json.ok_or("report --calibration: --out-json is required")?,
        out_md.ok_or("report --calibration: --out-md is required")?,
    );
    let inputs = CalInputs {
        fit: fit.ok_or("report --calibration: --fit is required")?,
        fit_samples,
        fit_verdicts,
        fit_adjudication,
        samples,
        verdicts,
        adjudication,
        cohort_tasks,
    };
    let (cal, json, md) = calibration_texts(inputs, &plan)?;
    for m in calibration_markers(&cal.gate) {
        eprintln!("{m}");
    }
    // HC.4: the precision table, only from a PASS.
    let table = match emit_table {
        Some(path) => {
            if cal.gate.verdict != "PASS" {
                return Err(format!(
                    "report --calibration: --emit-table needs the rule's PASS; the calibration reads {}",
                    cal.gate.verdict
                ));
            }
            let path = path.unwrap_or_else(|| {
                Path::new(&plan_path)
                    .ancestors()
                    .nth(3)
                    .unwrap_or(Path::new("."))
                    .join(TABLE_PATH)
                    .to_string_lossy()
                    .into_owned()
            });
            let text = precision_table(&cal, &json, &plan)?;
            eprintln!("{}", table_marker(&text, &path));
            Some((path, text))
        }
        None => None,
    };
    if check {
        let table_check = table.as_ref().map(|(p, t)| (p, t));
        for (path, want) in [(&out_json, &json), (&out_md, &md)]
            .into_iter()
            .chain(table_check)
        {
            let have = std::fs::read_to_string(path).unwrap_or_default();
            if have != *want {
                eprintln!(
                    "[edge-audit] calibration check: FAILED ({path} differs from the fit, samples and verdicts)"
                );
                return Ok(ExitCode::from(1));
            }
        }
        eprintln!("[edge-audit] calibration check: ok");
        return Ok(ExitCode::SUCCESS);
    }
    print!("{md}");
    std::fs::write(&out_json, json).map_err(|e| format!("{out_json}: {e}"))?;
    std::fs::write(&out_md, md).map_err(|e| format!("{out_md}: {e}"))?;
    if let Some((path, text)) = table {
        std::fs::write(&path, text).map_err(|e| format!("{path}: {e}"))?;
    }
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

    /// [`resolve_verdicts_scoped`] over one panel holding every id.
    fn resolve_verdicts(
        first_paths: &[PathBuf],
        rejudge_paths: &[PathBuf],
        adjudication: &[Adjudication],
        ids: &BTreeSet<String>,
        judge: Option<&str>,
        rule: RejudgeRule,
    ) -> Result<Resolved, String> {
        resolve_verdicts_scoped(
            first_paths,
            rejudge_paths,
            adjudication,
            ids,
            ids,
            judge,
            rule,
        )
    }

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

    /// HC.4: a pooled row needs 10 judged ids or a census, pools both
    /// panels, and is under floor below its tier's floor.
    #[test]
    fn table_rows_pool_both_panels() {
        let p = |key: &str, fit: (u64, u64), held: (u64, u64), census| PoolIn {
            key: key.to_string(),
            tier: key.split('|').nth(1).unwrap_or("").to_string(),
            fit: Judged {
                correct: fit.0,
                n: fit.1,
            },
            held: Judged {
                correct: held.0,
                n: held.1,
            },
            census,
        };
        let r = table_row(&p("CALLS|fact", (30, 30), (9, 10), false), &plan()).unwrap();
        assert_eq!((r.n, r.correct, r.fit_n, r.heldout_n), (40, 39, 30, 10));
        assert_eq!(r.p, r4(39.0 / 40.0));
        assert!(!r.under_floor, "0.975 >= 0.95");
        assert!(table_row(&p("USES|derived", (0, 0), (4, 5), false), &plan()).is_none());
        let census = table_row(&p("USES|derived", (0, 0), (4, 5), true), &plan()).unwrap();
        assert!(census.census && census.under_floor, "0.8 < 0.90");
        assert!(table_row(&p("X|fact", (0, 0), (0, 0), true), &plan()).is_none());
        assert_eq!(audit_digest("{}\n").len(), 12);
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
            panel: None,
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
        let (a, ma) = sample_text(std::slice::from_ref(&spec), &plan(), None).unwrap();
        let (b, mb) = sample_text(std::slice::from_ref(&spec), &plan(), None).unwrap();
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
    fn default_reproduces_the_committed_report() {
        // GP.14 (cd541240) re-drew the sample and wrote report.{json,md} with
        // the defaults; FD.3c's plugin / fd3c report is no longer committed,
        // so `--ci plugin --rejudge-rule fd3c` is read here only for its marker.
        let ad = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/edge-audit");
        let (report, json, md) = committed(CiMethod::StratifiedWilson, RejudgeRule::Matched);
        assert!(
            json == std::fs::read_to_string(ad.join("report.json")).unwrap(),
            "report.json drifted under the defaults"
        );
        assert!(
            md == std::fs::read_to_string(ad.join("report.md")).unwrap(),
            "report.md drifted under the defaults"
        );
        assert!(
            report_marker(&report).ends_with(" kappa=1.000 ci_method=stratified_wilson"),
            "{}",
            report_marker(&report)
        );
        let (plugin, json, _) = committed(CiMethod::Plugin, RejudgeRule::Fd3c);
        assert!(report_marker(&plugin).ends_with(" ci_method=plugin"));
        assert!(!json.contains("\"ci_method\""));
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
        assert_eq!(calls.lo, Some(0.8897));
        // Every sampled (not census) category reads a lower bound below 1; a
        // census category (INJECTS, GRPC_CALLS, ..) is exact.
        let sampled: Vec<&AggOut> = report.categories.iter().filter(|c| c.n < c.big_n).collect();
        assert!(sampled.len() >= 8);
        assert!(sampled.iter().all(|c| c.lo.is_some_and(|lo| lo < 1.0)));
        assert!(json.contains("\"ci_method\": \"stratified_wilson\""));
        assert!(json.contains("\"rule\": \"matched\""));
        assert!(md.contains("ci_method=stratified_wilson"));
        assert!(report_marker(&report).ends_with(" ci_method=stratified_wilson"));
        // GP.14's first judges found fewer than 10 wrong edges per family:
        // the matched set takes all of them and tops up with corrects to
        // MIN_REJUDGE, so each family's re-judge names 20 ids.
        let a = report.agreement.unwrap();
        let s = a.families.iter().find(|k| k.name == "structure").unwrap();
        assert_eq!(s.expected, 20);
        assert_eq!(s.missing, s.missing_ids.len() as u64);
        let m = a.families.iter().find(|k| k.name == "mechanism").unwrap();
        assert_eq!(m.expected, 20);
    }

    // ------------------------------------------------- calibration (HC.2)

    fn cin(key: &str, big_n: u64, fit: (u64, u64), held: (u64, u64)) -> CalIn {
        CalIn {
            key: key.to_string(),
            big_n: Some(big_n),
            fit: Some(Judged {
                correct: fit.0,
                n: fit.1,
            }),
            held: Judged {
                correct: held.0,
                n: held.1,
            },
        }
    }

    /// 15 strata at fit 30/30 and held-out 10/10 over N 100 each.
    fn transferring(n: usize) -> Vec<CalIn> {
        (0..n)
            .map(|i| cin(&format!("C{i:02}|fact"), 100, (30, 30), (10, 10)))
            .collect()
    }

    #[test]
    fn calibration_k_takes_ten_held_out_judged_and_a_fit_p() {
        let mut ins = transferring(15);
        // A census of 12 (N = n): in K, read by Wilson like any sample.
        ins.push(cin("CENSUS12|derived", 12, (18, 18), (12, 12)));
        // A census of 6: below 10 held-out judged, out of K.
        ins.push(cin("CENSUS6|derived", 6, (6, 6), (6, 6)));
        // No fit p: out of K whatever the held-out side holds.
        ins.push(CalIn {
            key: "NEW|heuristic".to_string(),
            big_n: Some(40),
            fit: None,
            held: Judged { correct: 2, n: 10 },
        });
        let (rows, g) = compare_strata(&ins, true);
        assert_eq!((g.k, g.covered, g.verdict), (16, 16, "PASS"));
        assert_eq!(g.fit_n, 15 * 30 + 18);
        assert_eq!(g.heldout_n, 15 * 10 + 12);
        assert!(g.cal_error.abs() < 1e-12);
        let census = rows
            .iter()
            .find(|r| r.stratum == "CENSUS12|derived")
            .unwrap();
        assert!(census.compared);
        // Wilson at 12/12 (0.7575), never the census's exact 1.
        assert_eq!(census.heldout_lo, Some(0.7575));
        assert_eq!(census.weight, Some(r4(12.0 / 1512.0)));
        let small = rows
            .iter()
            .find(|r| r.stratum == "CENSUS6|derived")
            .unwrap();
        assert!(!small.compared && small.holds.is_none() && small.weight.is_none());
        let new = rows.iter().find(|r| r.stratum == "NEW|heuristic").unwrap();
        assert!(!new.compared);
        let [m, v] = calibration_markers(&g);
        assert_eq!(
            m,
            "[edge-audit] calibration strata=16 fit_n=468 heldout_n=162 covered=16/16 cal_error=0.0000"
        );
        assert_eq!(
            v,
            "CONFIDENCE: R1=yes (16/16) R2=yes (0.0000) R3=yes (16) -> PASS"
        );
    }

    #[test]
    fn calibration_r1_needs_nine_tenths_of_k() {
        // 20 strata, 2 not transferred (held-out 5/10: hi 0.763 < fit lo
        // 0.886): 18/20 = 90% holds; a third drops it to 17/20.
        let mut ins = transferring(18);
        ins.push(cin("BAD1|derived", 1, (30, 30), (5, 10)));
        ins.push(cin("BAD2|derived", 1, (30, 30), (5, 10)));
        let (rows, g) = compare_strata(&ins, true);
        assert_eq!((g.k, g.covered, g.r1), (20, 18, true));
        assert_eq!(
            rows.iter().filter(|r| r.holds == Some(false)).count(),
            2,
            "both BAD strata fail R1"
        );
        // R2 weights the two small strata by their tiny held-out share.
        let want = 2.0 * (1.0 / 1802.0) * 0.5;
        assert!((g.cal_error - want).abs() < 1e-12, "{}", g.cal_error);
        assert_eq!(g.verdict, "PASS");
        ins.push(cin("BAD3|derived", 1, (30, 30), (5, 10)));
        let (_, g) = compare_strata(&ins, true);
        assert_eq!((g.k, g.covered, g.r1, g.verdict), (21, 18, false, "FAIL"));
    }

    #[test]
    fn calibration_r2_is_the_held_out_weighted_gap() {
        // 15 strata; one big stratum (N 900 of 2300) at fit 30/30 vs held-out
        // 8/10: its interval still overlaps (hi 0.943 >= lo 0.886), so R1
        // holds, but its 0.2 gap at weight 900/2300 is 0.078 > 0.05.
        let mut ins = transferring(14);
        ins.push(cin("BIG|fact", 900, (30, 30), (8, 10)));
        let (rows, g) = compare_strata(&ins, true);
        let big = rows.iter().find(|r| r.stratum == "BIG|fact").unwrap();
        assert_eq!(big.holds, Some(true));
        assert_eq!(big.diff, Some(0.2));
        assert!((g.cal_error - 0.2 * 900.0 / 2300.0).abs() < 1e-12);
        assert_eq!((g.r1, g.r2, g.r3, g.verdict), (true, false, true, "FAIL"));
        assert!(calibration_markers(&g)[1].ends_with("R2=no (0.0783) R3=yes (15) -> FAIL"));
    }

    #[test]
    fn calibration_under_fifteen_strata_is_insufficient() {
        let (_, g) = compare_strata(&transferring(14), true);
        assert_eq!(
            (g.r1, g.r2, g.r3, g.verdict),
            (true, true, false, "INSUFFICIENT")
        );
        let (_, g) = compare_strata(&[], true);
        assert_eq!((g.k, g.verdict), (0, "INSUFFICIENT"));
        assert_eq!(
            calibration_markers(&g)[1],
            "CONFIDENCE: R1=yes (0/0) R2=yes (0.0000) R3=no (0) -> INSUFFICIENT"
        );
    }

    #[test]
    fn sub_strata_compare_only_with_ten_on_both_sides() {
        let mut a = cin("CALLS|fact|graph:calls/x", 0, (12, 12), (10, 10));
        a.big_n = None;
        let mut b = cin("CALLS|fact|graph:calls/y", 0, (9, 9), (10, 10));
        b.big_n = None;
        let (rows, g) = compare_strata(&[a, b], false);
        assert!(rows[0].compared && !rows[1].compared);
        assert_eq!(rows[0].weight, None);
        assert_eq!(g.k, 0, "sub-strata never enter K");
    }

    fn sample_row(id: &str, label: &str, cat: &str, tier: &str, rule: &str) -> SampleRow {
        SampleRow {
            id: id.to_string(),
            label: label.to_string(),
            rev: "r".to_string(),
            category: cat.to_string(),
            tier: tier.to_string(),
            tier_note: None,
            emitter: Some("graph:calls".to_string()),
            rule: Some(rule.to_string()),
            site: Site {
                file: Some("a.py".to_string()),
                line: Some(1),
            },
            from: End {
                qname: "a".to_string(),
                kind: "FUNCTION".to_string(),
                file: Some("a.py".to_string()),
                line: Some(1),
            },
            to: End {
                qname: "b".to_string(),
                kind: "FUNCTION".to_string(),
                file: Some("a.py".to_string()),
                line: Some(2),
            },
        }
    }

    fn header(label: &str, sizes: &[(&str, u64)], panel: Option<&str>) -> Header {
        Header {
            label: label.to_string(),
            rev: "r".to_string(),
            seed: "s".to_string(),
            edges: 100,
            stratum_sizes: sizes.iter().map(|(k, n)| (k.to_string(), *n)).collect(),
            unjudgeable: BTreeMap::new(),
            excluded: BTreeMap::new(),
            panel: panel.map(str::to_string),
        }
    }

    fn jsonl<T: Serialize>(items: &[T]) -> String {
        items
            .iter()
            .map(|i| serde_json::to_string(i).unwrap() + "\n")
            .collect()
    }

    fn verdict_line(id: &str, v: &str, judge: &str) -> String {
        format!("{{\"id\":\"{id}\",\"verdict\":\"{v}\",\"reason\":\"\",\"judge\":\"{judge}\"}}\n")
    }

    #[test]
    fn panels_split_by_header_and_scope_the_verdicts() {
        let headers = vec![
            header("fit", &[("CALLS|fact", 5)], None),
            header("held", &[("CALLS|fact", 5)], Some(PANEL_HELDOUT)),
        ];
        let rows = vec![
            sample_row("f1", "fit", "CALLS", "fact", "x"),
            sample_row("h1", "held", "CALLS", "fact", "x"),
        ];
        let fit = split_panel(headers.clone(), rows.clone(), None).unwrap();
        assert_eq!(fit.headers.len(), 1);
        assert_eq!(fit.scope, BTreeSet::from(["f1".to_string()]));
        assert_eq!(fit.known.len(), 2);
        let held = split_panel(headers.clone(), rows.clone(), Some(PANEL_HELDOUT)).unwrap();
        assert_eq!(held.scope, BTreeSet::from(["h1".to_string()]));
        // The held-out panel's first verdict and its re-judge never reach the
        // fit's resolution; an unknown id still errors.
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("heldout.jsonl");
        let fitv = dir.path().join("structure.jsonl");
        let second = dir.path().join(REJUDGE_FILE);
        std::fs::write(&first, verdict_line("h1", "wrong", "a")).unwrap();
        std::fs::write(&fitv, verdict_line("f1", "correct", "a")).unwrap();
        std::fs::write(&second, verdict_line("h1", "correct", "b")).unwrap();
        let adj = vec![Adjudication {
            id: "h1".to_string(),
            first: "wrong".to_string(),
            second: "correct".to_string(),
            final_verdict: "correct".to_string(),
        }];
        let firsts = [first.clone(), fitv.clone()];
        let r = resolve_verdicts_scoped(
            &firsts,
            std::slice::from_ref(&second),
            &adj,
            &fit.known,
            &fit.scope,
            None,
            RejudgeRule::Matched,
        )
        .unwrap();
        assert_eq!(r.verdicts.len(), 1);
        assert_eq!(r.verdicts["f1"], "correct");
        let a = r.agreement.unwrap();
        assert_eq!(a.families.len(), 1);
        assert_eq!((a.overall.rejudged, a.disagreements), (0, 0));
        let r = resolve_verdicts_scoped(
            &firsts,
            std::slice::from_ref(&second),
            &adj,
            &held.known,
            &held.scope,
            None,
            RejudgeRule::Matched,
        )
        .unwrap();
        assert_eq!(r.verdicts.len(), 1);
        assert_eq!(r.verdicts["h1"], "correct", "the adjudication's final");
        std::fs::write(&first, verdict_line("zz", "wrong", "a")).unwrap();
        assert!(
            resolve_verdicts_scoped(
                &firsts,
                &[],
                &[],
                &held.known,
                &held.scope,
                None,
                RejudgeRule::Matched,
            )
            .unwrap_err()
            .contains("zz")
        );
        // One label in two panels is an error.
        let mut two = headers;
        two.push(header("fit", &[], Some(PANEL_HELDOUT)));
        assert!(
            split_panel(two, rows, None)
                .unwrap_err()
                .contains("two panels")
        );
    }

    #[test]
    fn name_drift_counts_flagged_truth_over_flagged() {
        let text = [
            r#"{"repo":"a","shape":"drift","S":{"names":{"flagged":4,"flagged_truth":["x"]}}}"#,
            r#"{"repo":"a","shape":"drift","S":{"names":{"flagged":0,"flagged_truth":[]}}}"#,
            r#"{"repo":"b","shape":"drift","S":{"names":{"error":"boom"}}}"#,
            r#"{"repo":"b","shape":"ripple","S":{"names":{"flagged":9,"flagged_truth":["y"]}}}"#,
            r#"{"repo":"c","shape":"drift","S":{"names":{"flagged":6,"flagged_truth":["p","q","r"]}}}"#,
        ]
        .join("\n");
        let rows = name_drift(&text, &BTreeSet::from(["c".to_string()])).unwrap();
        let got: Vec<(&str, bool, u64, u64, u64)> = rows
            .iter()
            .map(|r| (r.repo.as_str(), r.heldout, r.tasks, r.flagged, r.truth))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a", false, 2, 4, 1),
                ("c", true, 1, 6, 3),
                ("all", false, 3, 10, 4)
            ]
        );
        assert_eq!(rows[2].p, Some(0.4));
    }

    #[test]
    fn calibration_end_to_end_is_derived_and_deterministic() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // The fit: 16 CALLS-like strata at 12/12 judged, from fit samples.
        let (mut fit_rows, mut held_rows) = (Vec::new(), Vec::new());
        let (mut fit_v, mut held_v) = (String::new(), String::new());
        let mut fit_sizes = Vec::new();
        let mut held_sizes = Vec::new();
        let mut strata_json = Vec::new();
        for s in 0..16 {
            let cat = format!("C{s:02}");
            for i in 0..12 {
                let id = format!("f{s:02}{i:02}");
                fit_rows.push(sample_row(&id, "fit", &cat, "fact", "r"));
                fit_v.push_str(&verdict_line(&id, "correct", "a"));
                let id = format!("h{s:02}{i:02}");
                held_rows.push(sample_row(&id, "held", &cat, "fact", "r"));
                // One wrong edge in the last stratum.
                let v = if s == 15 && i == 0 {
                    "wrong"
                } else {
                    "correct"
                };
                held_v.push_str(&verdict_line(&id, v, "a"));
            }
            fit_sizes.push((format!("{cat}|fact"), 50u64));
            held_sizes.push((format!("{cat}|fact"), 40u64));
            strata_json.push(format!(
                "{{\"category\":\"{cat}\",\"tier\":\"fact\",\"N\":50,\"n\":12,\"correct\":12}}"
            ));
        }
        let fit_report = d.join("report.json");
        std::fs::write(
            &fit_report,
            format!(
                "{{\"strata\":[{}],\"tiers\":[{{\"name\":\"fact\",\"n\":192,\"p\":1.0,\"lo\":0.98,\"hi\":1.0}}]}}",
                strata_json.join(",")
            ),
        )
        .unwrap();
        fn sizes(v: &[(String, u64)]) -> Vec<(&str, u64)> {
            v.iter().map(|(k, n)| (k.as_str(), *n)).collect()
        }
        std::fs::create_dir_all(d.join("fit/samples")).unwrap();
        std::fs::create_dir_all(d.join("samples")).unwrap();
        std::fs::create_dir_all(d.join("verdicts")).unwrap();
        let fit_samples = d.join("fit/samples/audit.jsonl");
        std::fs::write(
            &fit_samples,
            jsonl(&[header("fit", &sizes(&fit_sizes), None)]) + &jsonl(&fit_rows),
        )
        .unwrap();
        std::fs::write(d.join("fit/structure.jsonl"), &fit_v).unwrap();
        // The tree: the fit samples beside the held-out ones.
        std::fs::copy(&fit_samples, d.join("samples/audit.jsonl")).unwrap();
        std::fs::write(
            d.join("samples/heldout.jsonl"),
            jsonl(&[header("held", &sizes(&held_sizes), Some(PANEL_HELDOUT))]) + &jsonl(&held_rows),
        )
        .unwrap();
        std::fs::write(d.join("verdicts/structure.jsonl"), &fit_v).unwrap();
        std::fs::write(d.join("verdicts/heldout.jsonl"), &held_v).unwrap();
        let tasks = d.join("tasks.jsonl");
        std::fs::write(
            &tasks,
            r#"{"repo":"held","shape":"drift","S":{"names":{"flagged":2,"flagged_truth":["x"]}}}"#,
        )
        .unwrap();
        let inputs = || CalInputs {
            fit: fit_report.clone(),
            fit_samples: vec![fit_samples.clone()],
            fit_verdicts: vec![d.join("fit/structure.jsonl")],
            fit_adjudication: None,
            samples: expand(&d.join("samples/*.jsonl").to_string_lossy()).unwrap(),
            verdicts: expand(&d.join("verdicts/*.jsonl").to_string_lossy()).unwrap(),
            adjudication: d.join(ADJUDICATION_FILE),
            cohort_tasks: Some(tasks.clone()),
        };
        let (cal, json, md) = calibration_texts(inputs(), &plan()).unwrap();
        let (_, json2, md2) = calibration_texts(inputs(), &plan()).unwrap();
        assert_eq!((&json, &md), (&json2, &md2));
        assert_eq!((cal.gate.k, cal.gate.covered), (16, 16));
        assert_eq!(cal.gate.verdict, "PASS");
        // 1/12 off in one stratum of 16 equal shares.
        assert!((cal.gate.cal_error - (1.0 / 12.0) / 16.0).abs() < 1e-12);
        assert_eq!(cal.heldout.repos, vec!["held".to_string()]);
        assert_eq!(cal.sub_strata.len(), 16);
        assert_eq!(cal.largest_gaps, vec!["C15|fact".to_string()]);
        assert_eq!(cal.repos["held"].wrong, 1);
        assert!(md.contains("CONFIDENCE: R1=yes (16/16) R2=yes (0.0052) R3=yes (16) -> PASS"));
        assert!(json.contains("\"rule\": \"dev-notes/confidence-gate-0.5.7.md\""));
        assert_eq!(cal.name_drift.as_ref().unwrap()[0].truth, 1);

        // The fit report of the same tree never sees the held-out panel.
        let (first, second) =
            split_rejudge(expand(&d.join("verdicts/*.jsonl").to_string_lossy()).unwrap());
        let (fit_only, _, _) = report_texts(
            ReportInputs {
                samples: expand(&d.join("samples/*.jsonl").to_string_lossy()).unwrap(),
                verdicts: first,
                rejudge: second,
                adjudication: d.join(ADJUDICATION_FILE),
                judge: None,
                ci: CiMethod::StratifiedWilson,
                rule: RejudgeRule::Matched,
            },
            &plan(),
        )
        .unwrap();
        assert_eq!(fit_only.repos.keys().collect::<Vec<_>>(), vec!["fit"]);
        assert_eq!(fit_only.strata.iter().map(|s| s.n).sum::<u64>(), 192);

        // Fit samples that do not reproduce the fit report are refused.
        std::fs::write(
            d.join("fit/structure.jsonl"),
            fit_v.replacen("\"correct\"", "\"wrong\"", 1),
        )
        .unwrap();
        assert!(
            calibration_texts(inputs(), &plan())
                .unwrap_err()
                .contains("do not reproduce the fit report")
        );
    }

    #[test]
    fn committed_fit_report_ignores_a_held_out_panel_beside_it() {
        // The committed samples and verdicts plus a held-out samples file, its
        // first verdicts and re-judge rows: the fit report is byte for byte
        // the committed one.
        let ad = Path::new(env!("CARGO_MANIFEST_DIR")).join("../bench/edge-audit");
        let dir = tempfile::tempdir().unwrap();
        let held = dir.path().join("heldout.jsonl");
        std::fs::write(
            &held,
            jsonl(&[header("held", &[("CALLS|fact", 3)], Some(PANEL_HELDOUT))])
                + &jsonl(&[sample_row("ffffffffffffff01", "held", "CALLS", "fact", "x")]),
        )
        .unwrap();
        let hv = dir.path().join("heldout-verdicts.jsonl");
        std::fs::write(&hv, verdict_line("ffffffffffffff01", "wrong", "a")).unwrap();
        let rj = dir.path().join(REJUDGE_FILE);
        let mut rejudge_text = std::fs::read_to_string(ad.join("verdicts/rejudge.jsonl")).unwrap();
        rejudge_text.push_str(&verdict_line("ffffffffffffff01", "correct", "b"));
        std::fs::write(&rj, rejudge_text).unwrap();
        let all = expand(&ad.join("verdicts/*.jsonl").to_string_lossy()).unwrap();
        let (mut verdicts, _) = split_rejudge(all);
        verdicts.push(hv);
        let mut samples = expand(&ad.join("samples/*.jsonl").to_string_lossy()).unwrap();
        samples.push(held);
        let plan = Plan::read(&ad.join("floors.json").to_string_lossy()).unwrap();
        let (_, json, md) = report_texts(
            ReportInputs {
                samples,
                verdicts,
                rejudge: vec![rj],
                adjudication: ad.join(ADJUDICATION_FILE),
                judge: None,
                ci: CiMethod::StratifiedWilson,
                rule: RejudgeRule::Matched,
            },
            &plan,
        )
        .unwrap();
        assert!(json == std::fs::read_to_string(ad.join("report.json")).unwrap());
        assert!(md == std::fs::read_to_string(ad.join("report.md")).unwrap());
    }
}
