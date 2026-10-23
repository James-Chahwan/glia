//! Ranking experiment, 2026-10-18 (write-up: dev-notes/ranking-eval-2026-10-18.md).
//!
//! The question: which file ranking should the change / find / pack answers use?
//! Today they order by Personalised PageRank (damping 0.5), never measured
//! against a baseline. This builds ONE repo's graph in memory (no layout, cache
//! or file is written), replays that repo's git history read-only, and scores
//! every ranker on one task: given one changed file of a commit, rank every
//! other code file so the commit's other changed files come first.
//!
//! ```text
//! GLIA_NO_PERSIST=1 cargo run --release -p glia-engine --example rank_eval -- \
//!     --repo <git-archive copy> --git <live checkout> --rev <sha> --label <name> \
//!     --out <result.json> --md <tables.md> [--max-commits 300] [--max-seeds 5] \
//!     [--timing 40] [--boot 1000] [--push-eps 1e-6]
//! ```
//!
//! Leakage: co-change counts, edge co-change support (U*) and the BM25 past-task
//! corpus (T*) for a test commit are built from commits strictly older than it
//! (oldest -> newest replay over `git log --topo-order`). The graph is the
//! snapshot's for every ranker alike.
//!
//! Third run (`--law-impact kina`, 2026-10-18): instead of the replay, does a change
//! in the law predict the code change through the goal layer? A section-level diff
//! of two versions of one instrument (pdftotext), the doc units citing the changed
//! sections, the ids they define and the code citing those ids, all at the start of
//! the work implementing the change (`--rev`), scored against the files the
//! implementing commits (`--law-tips`) changed. See `law_impact_main`.
//!
//! ```text
//! GLIA_NO_PERSIST=1 target/release/examples/rank_eval --law-impact kina --private <private.json> \
//!     --repo <git-archive copy at the start> --git <Kina> --rev <start> --wiki <Kina/.wiki> \
//!     --law-sources <dir of the instrument PDFs> --law-cache <scratch dir> --law-out <tables dir> \
//!     --law-tips <mainline>,<branch>,.. [--law-end <rev>]   # GLIA_LAW_DEBUG=<section> prints that section's reach
//! ```
//!
//! The private config: Kina's domain data (the instruments' titles and file stems, the
//! regime's vocabulary and jurisdiction marks, the id patterns, the target spec and the
//! report labels) is not in this public file. The Kina-only modes (`--vertical kina`,
//! `--law-impact kina`, `--pre kina`) read it from a JSON file named by `--private <path>`
//! or `GLIA_RANK_PRIVATE`, and refuse to run without one; every other mode never reads
//! it. See `Private`.
//!
//! Fourth run (`--keys on`, 2026-10-18): the cross-layer links resolved instead of
//! matched as raw text. Each layer emits key mentions; one resolver per key type
//! (path, scoped id, id lineage, citation, table row, text) normalises them; one
//! joiner links the mentions of one resolved key. Added to `--vertical kina|glia`
//! (the VR* rankers, with ablations and controls) and to `--law-impact kina` (the
//! R* chains); `--keys-out <dir>` gets the audit samples and the law tables. Kina's
//! vertical mode also needs `--law-sources` / `--law-cache` (each instrument's
//! section list). See `key_sig_names` and `keys_law`.
//!
//! Round 2 (2026-10-18): `--round2 on` (with `--round1 on`) routes each test commit's
//! SUBJECT ids through the keyed resolvers at the parent revision (IDR: the defining
//! units' names-file edges, pending / done, and the code citing the id; glia: packet ->
//! files_touched) and adds SELt v2; see `r2_build`. The keyed vertical run gains the
//! `+ns` (namespaced bare ids, `KNs`), `+roles` (role-aware joining, `KROLES`) and
//! `+trank` (text as a ranker behind the typed links) variants, and the law run their
//! chains. `--law-impact kina --law-event tr --keys on` traces a second law event (a
//! second instrument, as made -> its first consolidated version); see `law_event_tr`.
//!
//! Round 3 (2026-10-19): `--round3 on` (with `--round1 on --round2 on`) adds id history (IDH),
//! per-query text gating, SELt3 / SELs0 and the review / find transfer shapes;
//! `--r3-anc dir@rev=prefix@import,..` prepends pre-merge histories (`--r3-anc-map` controls),
//! `--r3-perq on` dumps per-query metrics and `--r3-compare a.json,b.json` pairs two such runs.
//! See the "Round 3" section near the end of this file.
//!
//! The shipped answers (0.5.3, EG.1): `--ans on` adds the ANS rankers, which call the engine's
//! own answers instead of harness code: `ANS:impact` (`diff_impact::diff_impact_from_diff` over
//! the seed files as a changed-file list) and `ANS:pack` (`pack::pack_ids` over the seed files'
//! nodes) on the single-seed and review shapes, `ANS:find` (`find::find_nodes` on the test
//! commit's subject) on the find shape. With `--r3-perq on` the JSON also gains `perq_shapes`,
//! every round-3 transfer shape per query. The reference rankers' PPR is pinned (`REF_DAMPING`).
//!
//! The rank gate (0.5.3, EG.2): `--gate-trim <gate.json> --gate-new <run.json> --out <base.json>`
//! keeps only a gate's rankers of a `--r3-perq on` run (compact JSON, the committed baseline);
//! `--gate-check <gate.json> --gate-base <base.json> --gate-new <run.json> --out .. --md ..`
//! judges every gated (shape, ranker, metric) row new - base with the commit-cluster bootstrap
//! of `paired` and Holm over the changed rows (exit 1 on a drop, 2 on drift or a missing
//! ranker); see `gate_check_main`.
//!
//! Engine intent parity (0.5.3, EB.9): `--engine on` (with `--round1 on --round2 on`) adds the
//! ENG-* rows, held apart and appended after every other row (selections and gates included),
//! so no earlier row's value or position moves: ENG-T1s-raw (`intent::text::TextIndex::from_docs`
//! over the strictly older commits' raw `%B`, which must equal T1s bit for bit), ENG-T1s (the
//! messages `clean_message`ed as `history sync` stores them), ENG-T1s-plain (plain `git log`
//! order), ENG-IDR (`intent::idr::route` at each parent, the parent's `[ids]` or
//! `--eng-overlay <file>`'s), and the `algo::fuse` LX fusions over the harness's own P1 / T1s
//! (single, review and find shapes). Fired_on: `[rank-eval] engine parity repo=<label>
//! t1s_raw_mismatch=<q>/<n> ..`; the round-1 md gains `### Engine parity`, the JSON
//! `engine_intent_parity`. Parity rows, not gated.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::Instant;

use glia_activation::algo::community::{self as comm, CommunityOptions, WeightedGraph};
use glia_activation::algo::fuse;
use glia_activation::algo::ppr::PushPpr;
use glia_activation::algo::reach;
use glia_activation::algo::{Adjacency, CategorySet, GraphSource, Walk};
use glia_activation::{ActivationConfig, ActivationPlan, Direction, Specificity};
use glia_code_domain::glia_config::parse_str;
use glia_code_domain::profile::CODE_TABLES;
use glia_code_domain::snapshots::clean_message;
use glia_code_domain::{cell_type, edge_category as ec, node_kind};
use glia_core::{CellPayload, Edge, EdgeCategoryId, NodeId};
use glia_engine::find::FindOptions;
use glia_engine::intent::defs::{Tree, read_defs};
use glia_engine::intent::idr as eidr;
use glia_engine::intent::ids::IdVocab;
use glia_engine::intent::paths::PathResolver;
use glia_engine::intent::text::{Candidates, TextIndex};
use glia_engine::pack::PackArgs;
use glia_engine::rank::{CochangeIndex, FileIndex, NOT_CODE_EXTS, RankCache, Ranking, STRUCTURE_CANDIDATES};
use glia_engine::{BlastOptions, Locator, default_keying, entrypoint_reachable, generate_one, service_of};
use glia_graph::MergedGraph;
use rayon::prelude::*;
use serde_json::json;

/// A ranker's sort key per file: compared lexicographically, descending; an
/// exact tie goes to the smaller path.
type Key = [f64; 3];
const NONE: u32 = u32::MAX;
/// A commit with more code files than this is neither a test commit nor
/// training data (co-change or past task).
const MAX_FILES: usize = 30;
const TOP_TASKS: usize = 20;
const RRF_K: f64 = 60.0;
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;
const U_BETAS: [f64; 2] = [1.0, 5.0];
const ALPHAS: [f64; 3] = [0.25, 0.5, 0.75];

/// The 0.5.2 PPR the reference rankers measure (damping, iteration cap, convergence epsilon,
/// node specificity): pinned so an engine default change never moves the reference the rank
/// gate compares against. They equal `ActivationConfig::default()` at 0.5.2, so pinning moves
/// no ranker. Edge weights stay `CODE_TABLES`': a weight change is a graph-ranking change the
/// gate is meant to see, on the ANS rankers and the reference alike.
const REF_DAMPING: f64 = 0.5;
const REF_MAX_ITER: usize = 100;
const REF_EPS: f64 = 1e-6;
const REF_SPECIFICITY: Specificity = Specificity::None;

/// `CODE_TABLES.activation_config(preset)` with the reference pin applied (`REF_DAMPING`).
fn ref_config(preset: Option<&str>) -> ActivationConfig {
    let mut c = CODE_TABLES.activation_config(preset);
    c.damping = REF_DAMPING;
    c.max_iterations = REF_MAX_ITER;
    c.epsilon = REF_EPS;
    c.node_specificity = REF_SPECIFICITY;
    c
}

/// Extensions a MODULE node may sit on that are not code (docs, data,
/// lockfiles, manifests). Everything else with a MODULE node is a candidate.
const NOT_CODE: &[&str] = &[
    "md", "mdx", "markdown", "rst", "txt", "json", "jsonc", "lock", "yaml", "yml", "toml", "csv", "tsv",
    "map", "svg", "png", "jpg", "gif", "ico", "xml", "ini", "cfg", "conf", "env", "example", "sum", "mod",
    "gitkeep", "pdf", "snap", "golden", "out", "log", "html", "css", "scss",
];

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

// ============================================================================
// Args
// ============================================================================

struct Args {
    repo: String,
    git: String,
    rev: String,
    label: String,
    out: String,
    md: String,
    max_commits: usize,
    max_seeds: usize,
    timing: usize,
    boot: usize,
    push_eps: f64,
    /// Second run: `kina`, `glia` or `quokka` adds the vertical (upper-layer) rankers.
    vertical: String,
    /// Kina only: the wiki clone (its own git history), the doc layer after the move out.
    wiki: String,
    /// The vertical tables (markdown).
    vmd: String,
    /// Third run: `kina` runs the law-impact experiment instead of the ranking replay.
    law_impact: String,
    /// The directory holding the law PDFs (read-only).
    law_sources: String,
    /// Where pdftotext writes its text (a scratch copy).
    law_cache: String,
    /// The output directory for the law-impact tables.
    law_out: String,
    /// Comma-separated revisions whose commits since `--rev` are the implementing-commit candidates.
    law_tips: String,
    /// The revision whose code citations give the leaky END variant (default: the first tip).
    law_end: String,
    /// Fourth run: `on` adds the resolved-key rankers (vertical) or chains (law impact).
    keys: String,
    /// Where the keyed run writes its audit samples and law tables.
    keys_out: String,
    /// Round 1 (2026-10-18): `on` adds the realistic-intent, work-neighbourhood and milestone rankers.
    round1: String,
    /// Round 1: the pre-change intent text source, `glia` (packet title + rationale) or `kina` (task rows).
    pre: String,
    /// Round 1: `on` adds the PPR tuning grid (edge weights x damping x pooling).
    tune: String,
    /// Round 1: the round-1 tables (markdown).
    r1md: String,
    /// Round 1: the PPR config picked on the tuning repos (`<weights>,d<damping>`, sum-pooled), for the hybrids.
    pick: String,
    /// Round 2: `on` adds the subject's ids as typed links (IDR) and SELt v2 (needs `--round1 on`).
    round2: String,
    /// Round 2: the law event of `--law-impact`: `act` (the primary instrument, its old version ->
    /// its new one) or `tr` (a second instrument, as made -> its first consolidated version), traced
    /// through the resolved goal layer.
    law_event: String,
    /// Round 3: `on` adds id history (IDH), text gating, SELt3 and the transfer shapes (needs `--round2 on`).
    round3: String,
    /// Round 3: the round-3 tables (markdown).
    r3md: String,
    /// Round 3 (C): pre-merge histories, `dir@rev=prefix@import,..`, prepended by author time.
    r3_anc: String,
    /// Round 3 (C): the ancestor path map, `true` (default), `within` or `perm:<i,j,..>` (controls).
    r3_anc_map: String,
    /// Round 3: `on` writes every query's recall@10 / MRR per ranker (for `--r3-compare`).
    r3_perq: String,
    /// `on` adds the ANS rankers (the engine's own answers, EG.1); empty (default) leaves them out.
    ans: String,
    /// Round 3: `<a.json>,<b.json>` compares two `--r3-perq on` runs of the same queries (b - a).
    r3_compare: String,
    /// Rank gate (EG.2): `--gate-check <gate.json>` judges `--gate-new` against `--gate-base`.
    gate_check: String,
    /// Rank gate: the trimmed baseline `--gate-check` reads (`--gate-base`).
    gate_base: String,
    /// Rank gate: the new `--r3-perq on` run (`--gate-new`), checked or trimmed.
    gate_new: String,
    /// Rank gate: `--gate-trim <gate.json>` writes `--gate-new` trimmed to the gate's rankers to `--out`.
    gate_trim: String,
    /// Round 3: extra SELt3x candidates (comma-separated; chosen after a first run, so post-hoc).
    r3_selx: String,
    /// `on` (EA.9): the engine's FileIndex and `rank::rank_files` keys checked against this
    /// harness's candidate files and P1 / PT* / B0 / H10 / H10[PT*] / H10[B0] per query, fed the
    /// replay's leak-free prefix co-change counts (needs `--pick base,d85`); default off.
    engine_parity: String,
    /// `on` (EB.9, needs `--round1 on --round2 on`): the ENG-* rankers, T1s, IDR and the LX
    /// fusions recomputed through `glia_engine::intent` + `algo::fuse` (parity rows, not gated).
    engine: String,
    /// `--engine on`: an overlay.toml whose `[ids]` ENG-IDR reads at a parent with none of its
    /// own (default: none, the engine's default vocabulary).
    eng_overlay: String,
    /// The private config (JSON) the Kina-only modes read; `GLIA_RANK_PRIVATE` when unset.
    private: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        repo: String::new(),
        git: String::new(),
        rev: "HEAD".into(),
        label: "repo".into(),
        out: "rank_eval.json".into(),
        md: "rank_eval.md".into(),
        max_commits: 300,
        max_seeds: 5,
        timing: 40,
        boot: 1000,
        push_eps: 1e-6,
        vertical: String::new(),
        wiki: String::new(),
        vmd: "rank_eval_vertical.md".into(),
        law_impact: String::new(),
        law_sources: String::new(),
        law_cache: String::new(),
        law_out: String::new(),
        law_tips: String::new(),
        law_end: String::new(),
        keys: String::new(),
        keys_out: String::new(),
        round1: String::new(),
        pre: String::new(),
        tune: String::new(),
        r1md: "rank_eval_round1.md".into(),
        pick: String::new(),
        round2: String::new(),
        law_event: "act".into(),
        round3: String::new(),
        r3md: "rank_eval_round3.md".into(),
        r3_anc: String::new(),
        r3_anc_map: "true".into(),
        r3_perq: String::new(),
        ans: String::new(),
        r3_compare: String::new(),
        gate_check: String::new(),
        gate_base: String::new(),
        gate_new: String::new(),
        gate_trim: String::new(),
        r3_selx: String::new(),
        engine_parity: "off".into(),
        engine: "off".into(),
        eng_overlay: String::new(),
        private: std::env::var("GLIA_RANK_PRIVATE").unwrap_or_default(),
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let v = argv.get(i + 1).cloned().unwrap_or_default();
        match argv[i].as_str() {
            "--repo" => a.repo = v,
            "--git" => a.git = v,
            "--rev" => a.rev = v,
            "--label" => a.label = v,
            "--out" => a.out = v,
            "--md" => a.md = v,
            "--max-commits" => a.max_commits = v.parse().expect("--max-commits"),
            "--max-seeds" => a.max_seeds = v.parse().expect("--max-seeds"),
            "--timing" => a.timing = v.parse().expect("--timing"),
            "--boot" => a.boot = v.parse().expect("--boot"),
            "--push-eps" => a.push_eps = v.parse().expect("--push-eps"),
            "--vertical" => a.vertical = v,
            "--wiki" => a.wiki = v,
            "--vmd" => a.vmd = v,
            "--law-impact" => a.law_impact = v,
            "--law-sources" => a.law_sources = v,
            "--law-cache" => a.law_cache = v,
            "--law-out" => a.law_out = v,
            "--law-tips" => a.law_tips = v,
            "--law-end" => a.law_end = v,
            "--keys" => a.keys = v,
            "--keys-out" => a.keys_out = v,
            "--round1" => a.round1 = v,
            "--pre" => a.pre = v,
            "--tune" => a.tune = v,
            "--r1md" => a.r1md = v,
            "--pick" => a.pick = v,
            "--round2" => a.round2 = v,
            "--law-event" => a.law_event = v,
            "--round3" => a.round3 = v,
            "--r3md" => a.r3md = v,
            "--r3-anc" => a.r3_anc = v,
            "--r3-anc-map" => a.r3_anc_map = v,
            "--r3-perq" => a.r3_perq = v,
            "--ans" => a.ans = v,
            "--r3-compare" => a.r3_compare = v,
            "--r3-selx" => a.r3_selx = v,
            "--engine-parity" => a.engine_parity = v,
            "--engine" => a.engine = v,
            "--eng-overlay" => a.eng_overlay = v,
            "--gate-check" => a.gate_check = v,
            "--gate-base" => a.gate_base = v,
            "--gate-new" => a.gate_new = v,
            "--gate-trim" => a.gate_trim = v,
            "--private" => a.private = v,
            other => panic!("unknown argument {other}"),
        }
        i += 2;
    }
    assert!(a.engine_parity == "on" || a.engine_parity == "off", "--engine-parity: expected on or off");
    assert!(a.engine == "on" || a.engine == "off", "--engine: expected on or off");
    assert!(a.engine == "off" || (a.round1 == "on" && a.round2 == "on"), "--engine on needs --round1 on --round2 on");
    let offline = !a.r3_compare.is_empty() || !a.gate_check.is_empty() || !a.gate_trim.is_empty();
    assert!(offline || (!a.repo.is_empty() && !a.git.is_empty()), "--repo and --git are required");
    a
}

// ============================================================================
// The private config (the Kina-only modes)
// ============================================================================

/// Kina's domain data, kept out of this public repository: the id patterns, the instruments
/// the law runs diff (titles, file stems, page furniture), the regime's vocabulary, the two
/// jurisdictions' marks, the key prefixes and route names of the citation resolver, the target
/// spec of the truth sets, and the report labels (`text`). Read once from the JSON file named
/// by `--private` (or `GLIA_RANK_PRIVATE`) when a Kina-only mode runs; no other mode touches it.
#[derive(serde::Deserialize)]
struct Private {
    /// Hyphenated prefixes that name a standard, not an id (`ISO-8601`).
    id_standards: Vec<String>,
    /// The hyphen-less id letters (`T014`, `R4`, ...).
    id_bare: String,
    /// The `git grep` pattern for code lines that may cite an id or an instrument.
    code_pat: String,
    /// The same for the keyed model (ids, user stories, sections, numbered rows).
    keys_code_pat: String,
    /// Spellings of the regime instrument's name a law key unifies to `law_unify_to`.
    law_unify: Vec<String>,
    law_unify_to: String,
    /// Substrings removed from a law key.
    law_strip: Vec<String>,
    /// Jurisdiction words dropped from a law name (beside the generic `LAW_DROP`).
    law_drop: Vec<String>,
    /// The primary instrument's title, and the words its long title (the body) starts with.
    act_title: String,
    act_body_start: String,
    /// File stems (no `.pdf`) under `--law-sources`: the primary instrument's old and new
    /// versions, its amending instrument, the rules, the second instrument (consolidated, as made).
    pdf_act_old: String,
    pdf_act_new: String,
    pdf_amend: String,
    pdf_rules: String,
    pdf_trules: String,
    pdf_trules_made: String,
    /// The second instrument's title (its running page title).
    trules_title: String,
    /// Page furniture: line prefixes, line substrings (primary / second instrument), whole lines
    /// and substring groups (the amending instrument), and the drafter-code prefix.
    furn_prefix: Vec<String>,
    furn_act_contains: Vec<String>,
    furn_trules_contains: Vec<String>,
    furn_amend_equals: Vec<String>,
    furn_amend_all: Vec<Vec<String>>,
    furn_drafter_prefix: String,
    /// Words and phrases that mark an implementing task or commit as work for the regime.
    regime_words: Vec<String>,
    regime_phrases: Vec<String>,
    /// Words that mark a heading or line as about the regime (short all-caps ones whole-word).
    regime_line_words: Vec<String>,
    /// Path and title words that make a page a regime page.
    regime_page_path: Vec<String>,
    regime_page_title: Vec<String>,
    /// Name fragments of the regime instrument under a spelling the name list lacks.
    regime_name_frags: Vec<String>,
    /// Lower-case spellings of the regime instrument's name, and the tails (year, jurisdiction)
    /// that make a mention the home or the other jurisdiction's instrument.
    act_names: Vec<String>,
    act_home_tails: Vec<String>,
    act_other_tails: Vec<String>,
    /// Lower-case rules names, each with its class (`RULES` or `TRULES`).
    rules_names: Vec<(String, String)>,
    /// The word that puts a line, heading or page in the second instrument's context.
    trules_word: String,
    /// Abbreviations and words that name some other instrument.
    other_abbr: Vec<String>,
    /// The two jurisdictions' class labels, and the marks that count for each on a text, on a
    /// line (narrow / wide) and in a page path.
    jur_home: String,
    jur_other: String,
    marks_home: Vec<String>,
    marks_other: Vec<String>,
    other_line_marks: Vec<String>,
    other_line_marks_wide: Vec<String>,
    path_home: Vec<String>,
    path_other: Vec<String>,
    /// Resolved key prefixes: the primary instrument, the rules, the second instrument, the other
    /// jurisdiction's instrument.
    key_act: String,
    key_rules: String,
    key_trules: String,
    key_other_act: String,
    /// The spec unit the law runs' truth implements, and the truth labels (that spec; its
    /// regime-work subset).
    target_spec: String,
    truth_spec: String,
    truth_regime: String,
    /// Labels and report text: `t(key)` reads one; `tx` fills each `«key»` in a finished text.
    text: BTreeMap<String, String>,
}

static PRIVATE: std::sync::OnceLock<Private> = std::sync::OnceLock::new();

fn load_private(path: &str, why: &str) {
    assert!(
        !path.is_empty(),
        "{why} reads Kina's private config (kept outside this public repository): pass --private <path> or set GLIA_RANK_PRIVATE"
    );
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("--private {path}: {e}"));
    let p: Private = serde_json::from_str(&text).unwrap_or_else(|e| panic!("--private {path}: {e}"));
    let _ = PRIVATE.set(p);
}

fn prv() -> &'static Private {
    PRIVATE.get().expect("the private config is not loaded (a Kina-only path ran without --private)")
}

/// One label or report phrase from the private config.
fn t(key: &str) -> &'static str {
    prv().text.get(key).map_or_else(|| panic!("private config: no text `{key}`"), String::as_str)
}

/// A finished text with each `«key»` filled from the private config: a `text` entry, or one of
/// `home` / `other` (the jurisdictions), `ts` / `tr` (the truth labels).
fn tx(s: &str) -> String {
    let p = prv();
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(a) = rest.find('\u{ab}') {
        out.push_str(&rest[..a]);
        let after = &rest[a + '\u{ab}'.len_utf8()..];
        let val = after.find('\u{bb}').and_then(|b| {
            let key = &after[..b];
            let v = match key {
                "home" => Some(p.jur_home.as_str()),
                "other" => Some(p.jur_other.as_str()),
                "ts" => Some(p.truth_spec.as_str()),
                "tr" => Some(p.truth_regime.as_str()),
                _ => p.text.get(key).map(String::as_str),
            };
            v.map(|v| (v, b))
        });
        match val {
            Some((v, b)) => {
                out.push_str(v);
                rest = &after[b + '\u{bb}'.len_utf8()..];
            }
            None => {
                out.push('\u{ab}');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `eprintln!` through `tx`.
macro_rules! keprintln {
    ($($a:tt)*) => {
        eprintln!("{}", tx(&format!($($a)*)))
    };
}

/// Write a private-mode output file through `tx`.
fn kwrite(path: impl AsRef<std::path::Path>, s: &str) -> std::io::Result<()> {
    std::fs::write(path, tx(s))
}

fn any_eq(v: &[String], x: &str) -> bool {
    v.iter().any(|w| w == x)
}

fn home() -> &'static str {
    prv().jur_home.as_str()
}

fn oth() -> &'static str {
    prv().jur_other.as_str()
}

fn bare_kinds() -> BTreeSet<u8> {
    prv().id_bare.bytes().collect()
}

// ============================================================================
// Small helpers
// ============================================================================

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn h64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    mix(h)
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix(self.0)
    }
}

fn ext_of(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rsplit_once('.') {
        Some((stem, e)) if !stem.is_empty() => e.to_ascii_lowercase(),
        _ => String::new(),
    }
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn top2(path: &str) -> String {
    let segs: Vec<&str> = dir_of(path).split('/').filter(|s| !s.is_empty()).collect();
    segs.iter().take(2).copied().collect::<Vec<_>>().join("/")
}

fn shared_dir_segments(a: &str, b: &str) -> usize {
    dir_of(a).split('/').zip(dir_of(b).split('/')).take_while(|(x, y)| x == y && !x.is_empty()).count()
}

fn tokens(msg: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in msg.lines() {
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

fn payload_text(p: &CellPayload) -> &str {
    match p {
        CellPayload::Text(s) | CellPayload::Json(s) => s,
        CellPayload::Bytes(_) => "",
    }
}

fn better(a: &Key, ia: usize, b: &Key, ib: usize) -> bool {
    for i in 0..3 {
        if a[i] > b[i] {
            return true;
        }
        if a[i] < b[i] {
            return false;
        }
    }
    ia < ib
}

/// File indices other than `seed`, best first.
fn order(key: &[Key], seed: usize) -> Vec<usize> {
    let mut ix: Vec<usize> = (0..key.len()).filter(|&f| f != seed).collect();
    ix.sort_by(|&a, &b| {
        if better(&key[a], a, &key[b], b) {
            std::cmp::Ordering::Less
        } else if a == b {
            std::cmp::Ordering::Equal
        } else {
            std::cmp::Ordering::Greater
        }
    });
    ix
}

fn scalar(v: &[f64]) -> Vec<Key> {
    v.iter().map(|&s| [s, 0.0, 0.0]).collect()
}

/// Min-max over the candidates (every file but the seed); a flat vector is 0.
fn minmax(v: &[f64], seed: usize) -> Vec<f64> {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (f, &s) in v.iter().enumerate() {
        if f != seed {
            lo = lo.min(s);
            hi = hi.max(s);
        }
    }
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return vec![0.0; v.len()];
    }
    v.iter().map(|&s| (s - lo) / (hi - lo)).collect()
}

/// Rank normalisation: the best candidate 1.0, the worst 0.0.
fn ranknorm(key: &[Key], seed: usize) -> Vec<f64> {
    let ord = order(key, seed);
    let m = ord.len().max(2) as f64 - 1.0;
    let mut out = vec![0.0; key.len()];
    for (pos, &f) in ord.iter().enumerate() {
        out[f] = (m - pos as f64) / m;
    }
    out
}

/// Reciprocal-rank fusion: each list contributes 1 / (60 + rank) for the
/// files it ranks positively (`positive[f]`), nothing for the rest.
fn rrf(lists: &[(&[Key], &[bool])], seed: usize) -> Vec<f64> {
    let n = lists.first().map_or(0, |l| l.0.len());
    let mut out = vec![0.0; n];
    for (key, pos) in lists {
        let mut r = 0usize;
        for f in order(key, seed) {
            if pos[f] {
                r += 1;
                out[f] += 1.0 / (RRF_K + r as f64);
            }
        }
    }
    out
}

fn positive(v: &[f64]) -> Vec<bool> {
    v.iter().map(|&s| s > 0.0).collect()
}

fn blend(a: f64, x: &[f64], y: &[f64]) -> Vec<f64> {
    x.iter().zip(y).map(|(p, q)| a * p + (1.0 - a) * q).collect()
}

/// recall@5, recall@10, recall@20, MRR, nDCG@20 for one query.
fn evaluate(key: &[Key], seed: usize, targets: &[u32]) -> [f64; 5] {
    let mut ranks: Vec<usize> = targets
        .iter()
        .map(|&t| {
            let t = t as usize;
            let kt = key[t];
            let mut r = 1;
            for (f, kf) in key.iter().enumerate() {
                if f != seed && f != t && better(kf, f, &kt, t) {
                    r += 1;
                }
            }
            r
        })
        .collect();
    ranks.sort_unstable();
    let nt = targets.len() as f64;
    let rec = |k: usize| ranks.iter().filter(|&&r| r <= k).count() as f64 / nt;
    let mrr = 1.0 / ranks[0] as f64;
    let dcg: f64 = ranks.iter().filter(|&&r| r <= 20).map(|&r| 1.0 / ((r + 1) as f64).log2()).sum();
    let idcg: f64 = (1..=targets.len().min(20)).map(|i| 1.0 / ((i + 1) as f64).log2()).sum();
    [rec(5), rec(10), rec(20), mrr, dcg / idcg]
}

// ============================================================================
// Graph index
// ============================================================================

struct Graph {
    paths: Vec<String>,
    file_nodes: Vec<Vec<u32>>,
    file_deg: Vec<f64>,
    ids: Vec<NodeId>,
    node_file: Vec<u32>,
    /// The node's index in `merged.node_ids()` that PPR reads (last occurrence).
    node_ppr: Vec<usize>,
    node_by_id: HashMap<NodeId, u32>,
    deg_all: Vec<f64>,
    excluded_modules: BTreeMap<String, usize>,
    generated_modules: usize,
}

fn index_graph(merged: &MergedGraph, all_ids: &[NodeId], id_to_idx: &HashMap<NodeId, usize>, edges: &[&Edge]) -> Graph {
    let loc = Locator::new(merged);
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    let mut files: Vec<Option<String>> = Vec::new();
    let mut module_files: BTreeSet<String> = BTreeSet::new();
    let mut generated: BTreeSet<String> = BTreeSet::new();
    for g in &merged.graphs {
        for n in &g.nodes {
            if !seen.insert(n.id) {
                continue;
            }
            let file = loc.locate(n.id).file;
            if g.nav.kind_by_id.get(&n.id).copied() == Some(node_kind::MODULE)
                && let Some(f) = &file
            {
                let gen_origin = n
                    .cells
                    .iter()
                    .any(|c| c.kind == cell_type::ORIGIN && payload_text(&c.payload).contains("generated"));
                if gen_origin {
                    generated.insert(f.clone());
                } else {
                    module_files.insert(f.clone());
                }
            }
            ids.push(n.id);
            files.push(file);
        }
    }
    let mut excluded_modules: BTreeMap<String, usize> = BTreeMap::new();
    let mut paths: Vec<String> = Vec::new();
    for f in &module_files {
        let e = ext_of(f);
        if generated.contains(f) {
            continue;
        }
        if NOT_CODE.contains(&e.as_str()) || e.is_empty() {
            *excluded_modules.entry(if e.is_empty() { "(none)".into() } else { e }).or_default() += 1;
        } else {
            paths.push(f.clone());
        }
    }
    paths.sort();
    let pidx: HashMap<&str, u32> = paths.iter().enumerate().map(|(i, p)| (p.as_str(), i as u32)).collect();
    let mut file_nodes = vec![Vec::new(); paths.len()];
    let mut node_file = Vec::with_capacity(ids.len());
    for (u, f) in files.iter().enumerate() {
        let fi = f.as_deref().and_then(|p| pidx.get(p).copied()).unwrap_or(NONE);
        if fi != NONE {
            file_nodes[fi as usize].push(u as u32);
        }
        node_file.push(fi);
    }
    let node_by_id: HashMap<NodeId, u32> = ids.iter().enumerate().map(|(u, &id)| (id, u as u32)).collect();
    let node_ppr: Vec<usize> = ids.iter().map(|id| id_to_idx[id]).collect();
    let mut deg_all = vec![0.0; ids.len()];
    for e in edges {
        if let Some(&u) = node_by_id.get(&e.from) {
            deg_all[u as usize] += 1.0;
        }
        if let Some(&u) = node_by_id.get(&e.to) {
            deg_all[u as usize] += 1.0;
        }
    }
    let file_deg: Vec<f64> = file_nodes.iter().map(|ns| ns.iter().map(|&u| deg_all[u as usize]).sum()).collect();
    let _ = all_ids;
    Graph {
        paths,
        file_nodes,
        file_deg,
        ids,
        node_file,
        node_ppr,
        node_by_id,
        deg_all,
        excluded_modules,
        generated_modules: generated.len(),
    }
}

// ============================================================================
// PPR: the production power iteration, replicated over a prebuilt index
// ============================================================================

/// `glia_activation::ppr_vector`'s arithmetic in its operation order over an
/// index built once per (direction, weights), so a query costs the iterations
/// only. Verified against `ActivationPlan::run` in the timing pass.
struct Ppr {
    n: usize,
    damping: f64,
    max_iter: usize,
    eps: f64,
    start: Vec<usize>,
    inc: Vec<(usize, f64)>,
    /// Per incidence: the inter-file pair of its edge, or NONE.
    pair: Vec<u32>,
    out_w: Vec<f64>,
    dangling: Vec<usize>,
}

impl Ppr {
    fn build(n: usize, id_to_idx: &HashMap<NodeId, usize>, edges: &[&Edge], edge_pair: &[u32], cfg: &ActivationConfig) -> Self {
        let mut events: Vec<(usize, usize, f64, u32)> = Vec::new();
        let mut out_w = vec![0.0; n];
        for (k, e) in edges.iter().enumerate() {
            let w = cfg.edge_weights.get(&e.category).copied().unwrap_or(1.0);
            if w <= 0.0 {
                continue;
            }
            let (Some(&fi), Some(&ti)) = (id_to_idx.get(&e.from), id_to_idx.get(&e.to)) else {
                continue;
            };
            let p = edge_pair[k];
            match cfg.direction {
                Direction::Forward => {
                    events.push((ti, fi, w, p));
                    out_w[fi] += w;
                }
                Direction::Backward => {
                    events.push((fi, ti, w, p));
                    out_w[ti] += w;
                }
                Direction::Undirected => {
                    events.push((ti, fi, w, p));
                    events.push((fi, ti, w, p));
                    out_w[fi] += w;
                    out_w[ti] += w;
                }
            }
        }
        let mut start = vec![0usize; n + 1];
        for e in &events {
            start[e.0 + 1] += 1;
        }
        for i in 0..n {
            start[i + 1] += start[i];
        }
        let mut cursor = start.clone();
        let mut inc = vec![(0usize, 0.0f64); events.len()];
        let mut pair = vec![NONE; events.len()];
        for e in &events {
            let c = cursor[e.0];
            inc[c] = (e.1, e.2);
            pair[c] = e.3;
            cursor[e.0] += 1;
        }
        let dangling = (0..n).filter(|&j| out_w[j] == 0.0).collect();
        Ppr { n, damping: cfg.damping, max_iter: cfg.max_iterations, eps: cfg.epsilon, start, inc, pair, out_w, dangling }
    }

    fn uniform(seeds: &[usize]) -> Vec<(usize, f64)> {
        let w = 1.0 / seeds.len() as f64;
        seeds.iter().map(|&s| (s, w)).collect()
    }

    fn run(&self, pers: &[(usize, f64)]) -> Vec<f64> {
        self.power(pers, &self.inc, &self.out_w)
    }

    /// The incidences reweighted by `w * (1 + beta * s(pair))`, and the
    /// out-weights they sum to.
    fn reweight(&self, s: &[f32], beta: f64) -> (Vec<(usize, f64)>, Vec<f64>) {
        let mut out_w = vec![0.0; self.n];
        let inc: Vec<(usize, f64)> = self
            .inc
            .iter()
            .zip(&self.pair)
            .map(|(&(j, w), &p)| {
                let w2 = if p == NONE { w } else { w * (1.0 + beta * f64::from(s[p as usize])) };
                out_w[j] += w2;
                (j, w2)
            })
            .collect();
        (inc, out_w)
    }

    fn power(&self, pers: &[(usize, f64)], inc: &[(usize, f64)], out_w: &[f64]) -> Vec<f64> {
        let n = self.n;
        if n == 0 || pers.is_empty() {
            return vec![0.0; n];
        }
        let mut personalization = vec![0.0; n];
        for &(i, w) in pers {
            personalization[i] = w;
        }
        let d = self.damping.clamp(0.01, 0.99);
        let mut scores = personalization.clone();
        let mut new_scores = vec![0.0; n];
        for _ in 0..self.max_iter {
            let dangling_sum: f64 = self.dangling.iter().map(|&j| scores[j]).sum();
            for i in 0..n {
                let propagated: f64 = inc[self.start[i]..self.start[i + 1]]
                    .iter()
                    .map(|&(j, w)| if out_w[j] > 0.0 { scores[j] * w / out_w[j] } else { 0.0 })
                    .sum();
                new_scores[i] = (1.0 - d) * personalization[i] + d * (propagated + dangling_sum * personalization[i]);
            }
            let diff: f64 = scores.iter().zip(new_scores.iter()).map(|(a, b)| (a - b).abs()).sum();
            std::mem::swap(&mut scores, &mut new_scores);
            if diff < self.eps {
                break;
            }
        }
        scores
    }
}

// ============================================================================
// History replay
// ============================================================================

struct Commit {
    sha: String,
    msg: String,
    files: Vec<u32>,
}

struct Query {
    tc: usize,
    seed: u32,
    targets: Vec<u32>,
    c0: Vec<(u32, f64)>,
    t2: Vec<(u32, f64)>,
}

struct TestCommit {
    t1: Vec<(u32, f64)>,
    t0: Vec<(u32, f64)>,
    /// Edge-pair co-change support (Jaccard) over strictly older commits.
    s_pairs: Vec<(u32, f32)>,
}

#[derive(Default)]
struct Bm25 {
    post: HashMap<String, Vec<(u32, u32)>>,
    dl: Vec<u32>,
    total: u64,
    files: Vec<Vec<u32>>,
    toks: Vec<Vec<String>>,
}

impl Bm25 {
    fn add(&mut self, toks: Vec<String>, files: Vec<u32>) {
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
        self.files.push(files);
        self.toks.push(toks);
    }

    fn scores(&self, q: &[String]) -> Vec<f64> {
        let n = self.dl.len();
        let mut s = vec![0.0; n];
        if n == 0 {
            return s;
        }
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

    /// The top tasks (score > 0, `keep`), by score then newest first, as
    /// file scores: the sum over those tasks of the task's score.
    fn file_scores(&self, s: &[f64], keep: impl Fn(usize) -> bool) -> Vec<(u32, f64)> {
        let mut top: Vec<(usize, f64)> = s.iter().copied().enumerate().filter(|&(d, v)| v > 0.0 && keep(d)).collect();
        top.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| b.0.cmp(&a.0)));
        top.truncate(TOP_TASKS);
        let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
        for (d, v) in top {
            for &f in &self.files[d] {
                *acc.entry(f).or_default() += v;
            }
        }
        acc.into_iter().collect()
    }
}

fn git_log(git: &str, rev: &str, pidx: &HashMap<&str, u32>) -> (Vec<Commit>, usize) {
    let out = Command::new("git")
        .args([
            "-C",
            git,
            "log",
            "--topo-order",
            "--no-merges",
            "--no-renames",
            "--name-only",
            "--format=%x1e%H%x1f%B%x1f",
            rev,
        ])
        .output()
        .expect("git log");
    assert!(out.status.success(), "git log failed: {}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    let mut commits = Vec::new();
    let mut raw = 0;
    for rec in text.split('\x1e').skip(1) {
        raw += 1;
        let mut parts = rec.splitn(3, '\x1f');
        let sha = parts.next().unwrap_or("").trim().to_string();
        let msg = parts.next().unwrap_or("").to_string();
        let names = parts.next().unwrap_or("");
        let mut files: Vec<u32> = names.lines().filter_map(|l| pidx.get(l.trim()).copied()).collect();
        files.sort_unstable();
        files.dedup();
        commits.push(Commit { sha, msg, files });
    }
    commits.reverse();
    (commits, raw)
}

// ============================================================================
// Rankers per query
// ============================================================================

struct Ctx<'a> {
    merged: &'a MergedGraph,
    g: &'a Graph,
    ppr: Vec<(&'static str, Ppr)>,
    upr: Ppr,
    adj_carry: Adjacency,
    adj_imports: Adjacency,
    adj_push: Adjacency,
    push_w: HashMap<EdgeCategoryId, f64>,
    clusters: Vec<(&'static str, Vec<u32>, usize)>,
    path_hash: Vec<f64>,
    hubs: Vec<bool>,
    n_pairs: usize,
    push_eps: f64,
    /// Inter-file pairs joined by any edge (the vertical pair diagnostics).
    direct: HashSet<(u32, u32)>,
    /// Round 1: the PPR tuning grid (weight set x damping), empty without `--tune on`.
    tune: Vec<(String, Ppr)>,
    /// Round 1: the picked config's position in `tune`.
    pick: Option<usize>,
    /// `--ans on`: the ANS rankers run (EG.1).
    ans: bool,
    /// Candidate file path -> its index (ANS rankers); only ever looked up, never iterated.
    path_ix: HashMap<String, u32>,
    /// `entrypoint_reachable` of the graph, computed once for `find_nodes_with_live` (empty
    /// without `--ans on`).
    live: HashSet<NodeId>,
    /// Wall nanoseconds spent inside the engine answers' calls, summed over every thread.
    ans_ns: AtomicU64,
    /// Per test commit (its ordinal), the leak-free co-change prefix the ANS rankers and the
    /// engine parity read (EA.9); empty unless `--ans on` or `--engine-parity on`.
    prefix: Vec<Prefix>,
}

/// The replay's co-change counts at one test commit, strictly older training commits only (the
/// state `Query::c0` and `r3_tqueries` read), for the engine's `rank::CochangeIndex::from_counts`
/// (EA.9). `count` is every file's; `rows` keeps the pair rows of the test commit's own files
/// only, sorted by other file: C0 reads nothing but its seed files' rows (`rank::c0`), and every
/// seed of a single-seed or review query of this commit is one of its files, so the index answers
/// those queries exactly as the full counts would, at a fraction of the memory.
struct Prefix {
    count: Vec<u32>,
    rows: Vec<(u32, Vec<(u32, u32)>)>,
}

impl Prefix {
    fn at(cnt: &[u32], co: &[HashMap<u32, u32>], files: &[u32]) -> Prefix {
        let mut fs: Vec<u32> = files.to_vec();
        fs.sort_unstable();
        fs.dedup();
        let rows = fs
            .into_iter()
            .map(|f| {
                let mut r: Vec<(u32, u32)> = co[f as usize].iter().map(|(&g, &n)| (g, n)).collect();
                r.sort_unstable();
                (f, r)
            })
            .collect();
        Prefix { count: cnt.to_vec(), rows }
    }

    /// The engine's co-change index over the candidate file space (`nf` files).
    fn index(&self, nf: usize) -> CochangeIndex {
        let mut pairs: Vec<Vec<(u32, u32)>> = vec![Vec::new(); nf];
        for (f, r) in &self.rows {
            pairs[*f as usize] = r.clone();
        }
        CochangeIndex::from_counts(self.count.clone(), pairs)
    }
}

/// The ANS rankers' RankCache at test commit `tc` (EA.9): the transient cache the answers build
/// when given none ("answer", no roots, no labels), guarded, then given the commit's prefix
/// co-change, so the gate scores the shipped co-change path (the `--repo` copy holds no
/// `.glia/history-snapshot/`). One per call, as the answers' own transient cache is.
fn ans_cache(ctx: &Ctx, tc: usize) -> Option<Arc<RankCache>> {
    let p = ctx.prefix.get(tc)?;
    let c = RankCache::new("answer", &BTreeMap::new(), &BTreeMap::new());
    c.guard(ctx.merged);
    Some(Arc::new(c.with_cochange(p.index(ctx.g.paths.len()))))
}

#[derive(Default)]
struct Out {
    names: Vec<String>,
    m: Vec<[f64; 5]>,
    hub_share: Vec<(String, f64)>,
    /// Per vertical signal: [linked targets, targets, linked targets with no direct edge, targets with no direct
    /// edge, linked other candidates, other candidates].
    vdiag: Vec<(String, [f64; 6])>,
    /// Per (resolved, raw) signal pair: [co-change targets linked by the resolved signal, by
    /// the raw one, by the resolved only, by the raw only, co-change targets].
    vgain: Vec<(String, [f64; 5])>,
    /// Round 1: per signal, whether this query's seed has it (the covered subsets).
    r1cov: Vec<(String, bool)>,
    /// Round 3: earlier rankers the round-3 bank recomputed differently (must stay 0).
    r3_bad: f64,
    /// `--engine on` (EB.9): the ENG-* rows, held apart until `eng_append` puts them after every
    /// other row (the selections and gates included), so no earlier row's position moves.
    eng: Vec<(String, [f64; 5])>,
}

impl Out {
    fn push(&mut self, name: impl Into<String>, key: &[Key], seed: usize, targets: &[u32]) {
        self.names.push(name.into());
        self.m.push(evaluate(key, seed, targets));
    }

    fn push_eng(&mut self, name: &str, key: &[Key], seed: usize, targets: &[u32]) {
        self.eng.push((name.to_string(), evaluate(key, seed, targets)));
    }
}

/// Moves every held ENG-* row (`Out::eng`) after the last row of each query.
fn eng_append(outs: &mut [Out]) {
    for o in outs {
        for (n, m) in std::mem::take(&mut o.eng) {
            o.names.push(n);
            o.m.push(m);
        }
    }
}

fn dense(nf: usize, sparse: &[(u32, f64)], seed: usize) -> Vec<f64> {
    let mut v = vec![0.0; nf];
    for &(f, s) in sparse {
        if f as usize != seed {
            v[f as usize] = s;
        }
    }
    v
}

/// File score = max (or sum) over the file's nodes of `score(node)`.
fn file_pool(g: &Graph, score: impl Fn(usize) -> f64, sum: bool) -> Vec<f64> {
    g.file_nodes
        .iter()
        .map(|ns| {
            let mut acc = 0.0f64;
            for &u in ns {
                let s = score(u as usize);
                if sum {
                    acc += s;
                } else {
                    acc = acc.max(s);
                }
            }
            acc
        })
        .collect()
}

fn hub_share(v: &[f64], hubs: &[bool], seed: usize) -> f64 {
    let ord = order(&scalar(v), seed);
    let top: Vec<usize> = ord.into_iter().take(20).collect();
    let mass: f64 = top.iter().map(|&f| v[f]).sum();
    if mass <= 0.0 {
        return 0.0;
    }
    top.iter().filter(|&&f| hubs[f]).map(|&f| v[f]).sum::<f64>() / mass
}

fn bfs_key(ctx: &Ctx, adj: &Adjacency, seed_ids: &[NodeId]) -> (Vec<Key>, Vec<f64>) {
    let g = ctx.g;
    let nf = g.paths.len();
    let b = reach::bfs(adj, seed_ids, Walk::Both, usize::MAX);
    let mut depth = vec![f64::INFINITY; nf];
    for r in &b.reached {
        if let Some(&u) = g.node_by_id.get(&r.id) {
            let f = g.node_file[u as usize];
            if f != NONE {
                let d = &mut depth[f as usize];
                *d = d.min(r.depth as f64);
            }
        }
    }
    let key = (0..nf).map(|f| [if depth[f].is_finite() { -depth[f] } else { -1e9 }, g.file_deg[f], 0.0]).collect();
    (key, depth)
}

fn push_scores(ctx: &Ctx, push: &mut PushPpr, seed_ids: &[NodeId]) -> Vec<f64> {
    let g = ctx.g;
    let mut acc = vec![0.0; g.ids.len()];
    let k = seed_ids.len().max(1) as f64;
    for &s in seed_ids {
        let (scores, _) = push.run(s, 0.5, ctx.push_eps);
        for (id, p) in scores {
            if let Some(&u) = g.node_by_id.get(&id) {
                acc[u as usize] += p / k;
            }
        }
    }
    file_pool(g, |u| acc[u], false)
}

fn pooled(base: &[f64], cl: &[u32], m: usize) -> Vec<Key> {
    let mut cs = vec![0.0; m];
    for (f, &s) in base.iter().enumerate() {
        cs[cl[f] as usize] += s;
    }
    base.iter().enumerate().map(|(f, &s)| [cs[cl[f] as usize], s, 0.0]).collect()
}

/// The ANS rankers' file keys from an answer's node rows, given in the answer's rank order.
/// The first row whose `file` is a candidate gives that file key `[-k, 0, 0]`, k its order of
/// first appearance (0, 1, ...): a max-style roll-up of node rows to files, since no 0.5.2
/// answer exposes a file-level ranking (BlastAnswer / PackedNode / FoundNode are node rows).
/// Every file no row reaches keys `[-inf; 3]`, so the harness's tie rule orders those by path.
///
/// Contract for the porting packets: a packet that adds a file-level roll-up to an answer
/// (item 1's sum pooling in blast radius / pack) switches that answer's ANS ranker to read the
/// roll-up in the same packet.
fn rows_to_keys(g: &Graph, path_ix: &HashMap<String, u32>, files: impl Iterator<Item = Option<String>>) -> Vec<Key> {
    let mut key: Vec<Key> = vec![[f64::NEG_INFINITY; 3]; g.paths.len()];
    let mut k = 0usize;
    for f in files.flatten() {
        if let Some(&ix) = path_ix.get(&f) {
            let slot = &mut key[ix as usize];
            if slot[0] == f64::NEG_INFINITY {
                *slot = [-(k as f64), 0.0, 0.0];
                k += 1;
            }
        }
    }
    key
}

/// ANS:impact and ANS:pack for seed files `seeds`: the engine's diff impact over the seed files
/// as a changed-file list (its seeds are the answer's own: every node of each file) and its pack
/// over every node of the seed files, each with default options but the RankCache: test commit
/// `tc`'s leak-free prefix co-change (`ans_cache`, EA.9), so the default Ranking (H10) reads C0.
fn ans_keys_seed(ctx: &Ctx, tc: usize, seeds: &[u32]) -> Vec<(String, Vec<Key>)> {
    let g = ctx.g;
    let t0 = Instant::now();
    let cache = ans_cache(ctx, tc);
    let text = seeds.iter().map(|&f| g.paths[f as usize].as_str()).collect::<Vec<_>>().join("\n");
    let mut bo = BlastOptions::default();
    bo.rank_cache = cache.clone();
    let d = glia_engine::diff_impact::diff_impact_from_diff(ctx.merged, &text, &bo);
    let impact = rows_to_keys(g, &ctx.path_ix, d.impact.results.into_iter().map(|r| r.file));
    let seed_ids: Vec<NodeId> = seeds.iter().flat_map(|&f| g.file_nodes[f as usize].iter().map(|&u| g.ids[u as usize])).collect();
    let mut pa = PackArgs::default();
    pa.rank_cache = cache;
    let mut p = glia_engine::pack::pack_ids(ctx.merged, &seed_ids, &pa);
    p.nodes.sort_by_key(|n| n.rank);
    let pack = rows_to_keys(g, &ctx.path_ix, p.nodes.into_iter().map(|n| n.file));
    ctx.ans_ns.fetch_add(t0.elapsed().as_nanos() as u64, AtomicOrdering::Relaxed);
    vec![("ANS:impact".into(), impact), ("ANS:pack".into(), pack)]
}

/// ANS:find for a test commit's subject: the engine's find over the whole subject, every match
/// kept (`top_k = 0`). `find_nodes_with_live` is `find_nodes` over the live set computed once.
fn ans_keys_find(ctx: &Ctx, subject: &str) -> Vec<(String, Vec<Key>)> {
    let t0 = Instant::now();
    let mut o = FindOptions::default();
    o.top_k = 0;
    let a = glia_engine::find::find_nodes_with_live(ctx.merged, &ctx.live, subject, &o);
    let keys = rows_to_keys(ctx.g, &ctx.path_ix, a.results.into_iter().map(|r| r.file));
    ctx.ans_ns.fetch_add(t0.elapsed().as_nanos() as u64, AtomicOrdering::Relaxed);
    vec![("ANS:find".into(), keys)]
}

// ============================================================================
// Engine parity (EA.9): the engine's ranking against this harness's
// ============================================================================

/// The engine's candidate file space (`rank::FileIndex`, no labels: one repo) against
/// `index_graph`'s, and `rank::NOT_CODE_EXTS` against `NOT_CODE`. Prints `[rank-parity] files ..`
/// once per run; with `--engine-parity on` a mismatch panics at the first differing index (the
/// key parity is meaningless over two file spaces).
fn file_space_parity(merged: &MergedGraph, g: &Graph, strict: bool) -> serde_json::Value {
    let fi = FileIndex::of(merged, &BTreeMap::new());
    let first_diff = (0..g.paths.len().max(fi.len())).find(|&i| {
        let e = (i < fi.len()).then(|| fi.path(i as u32));
        e != g.paths.get(i).map(String::as_str)
    });
    let equal = first_diff.is_none();
    let not_code_equal = NOT_CODE_EXTS.as_slice() == NOT_CODE;
    eprintln!(
        "[rank-parity] files harness={} engine={} equal={equal} not_code_equal={not_code_equal}",
        g.paths.len(),
        fi.len()
    );
    if strict {
        if let Some(i) = first_diff {
            let e = (i < fi.len()).then(|| fi.path(i as u32).to_string());
            panic!("[rank-parity] file spaces differ at index {i}: harness={:?} engine={e:?}", g.paths.get(i));
        }
        assert!(not_code_equal, "[rank-parity] NOT_CODE_EXTS differs from rank_eval's NOT_CODE");
    }
    json!({"harness": g.paths.len(), "engine": fi.len(), "equal": equal, "not_code_equal": not_code_equal})
}

/// Files of `key` not excluded, best first (`order` with a set of excluded files).
fn order_ex(key: &[Key], excl: &[bool]) -> Vec<usize> {
    let mut ix: Vec<usize> = (0..key.len()).filter(|&f| !excl[f]).collect();
    ix.sort_by(|&a, &b| {
        if better(&key[a], a, &key[b], b) {
            std::cmp::Ordering::Less
        } else if a == b {
            std::cmp::Ordering::Equal
        } else {
            std::cmp::Ordering::Greater
        }
    });
    ix
}

/// The six rankings the engine ports, in STRUCTURE_CANDIDATES x {structure, co-change first}
/// order.
fn parity_rankings() -> Vec<Ranking> {
    [false, true].iter().flat_map(|&c| STRUCTURE_CANDIDATES.iter().map(move |&b| Ranking::new(b, c))).collect()
}

/// One parity query's verdict per ranking: (key mismatch, order mismatch, first key mismatch).
type ParityRow = Vec<(bool, bool, Option<(usize, f64, f64)>)>;

/// The engine's `rank::rank_files_with_c0` keys and order for one query against the harness's own
/// keys for the same seeds. `excl` marks the seed files; `c0h` is the harness's C0 (`Query::c0` /
/// `TQuery::c0`, dense); `c0e` the engine's `rank::c0` over the prefix `CochangeIndex`; `tol0`
/// compares the C0 component of a co-change-first key with a relative tolerance of 1e-12 (the
/// review shapes' k-seed sums), every other component in f64 bits.
fn parity_query(ctx: &Ctx, cache: &RankCache, seeds: &[u32], excl: &[bool], c0h: &[f64], c0e: &[f64], tol0: bool) -> ParityRow {
    let g = ctx.g;
    let nf = g.paths.len();
    let nodes: Vec<u32> = seeds.iter().flat_map(|&f| g.file_nodes[f as usize].iter().copied()).collect();
    let seed_ids: Vec<NodeId> = nodes.iter().map(|&u| g.ids[u as usize]).collect();
    let seed_ppr: Vec<usize> = nodes.iter().map(|&u| g.node_ppr[u as usize]).collect();
    let pers = Ppr::uniform(&seed_ppr);
    let run = |idx: &Ppr, sum: bool| -> Vec<f64> {
        if pers.is_empty() {
            return vec![0.0; nf];
        }
        let v = idx.run(&pers);
        file_pool(g, |u| v[g.node_ppr[u]], sum)
    };
    let p1 = run(&ctx.ppr[0].1, false);
    let pick = ctx.pick.expect("--engine-parity on needs --pick base,d85");
    let pt = run(&ctx.tune[pick].1, true);
    let (b0, _) = bfs_key(ctx, &ctx.adj_carry, &seed_ids);
    let harness = |r: Ranking| -> Vec<Key> {
        // The harness rankers by name: run_query's P1 / H10, round 1's PT* / H10[PT*] (`lex2`),
        // `bfs_key`'s B0 and round 3's H10[B0].
        match r.harness_name() {
            "P1" => scalar(&p1),
            "PT*" => scalar(&pt),
            "B0" => b0.clone(),
            "H10" => (0..nf).map(|f| [c0h[f], p1[f], 0.0]).collect(),
            "H10[PT*]" => lex2(c0h, &pt),
            "H10[B0]" => (0..nf).map(|f| [c0h[f], b0[f][0], b0[f][1]]).collect(),
            other => panic!("[rank-parity] no harness ranker for {other}"),
        }
    };
    parity_rankings()
        .into_iter()
        .map(|r| {
            let hk = harness(r);
            let fr = glia_engine::rank::rank_files_with_c0(ctx.merged, cache, seeds, &[], r, Some(c0e));
            let mut first: Option<(usize, f64, f64)> = None;
            if fr.keys.len() != nf {
                first = Some((usize::MAX, nf as f64, fr.keys.len() as f64));
            } else {
                'files: for f in (0..nf).filter(|&f| !excl[f]) {
                    for (i, (&a, &b)) in hk[f].iter().zip(fr.keys[f].iter()).enumerate() {
                        let ok = if i == 0 && tol0 && r.cochange_first {
                            a.to_bits() == b.to_bits() || (a - b).abs() <= 1e-12 * a.abs().max(b.abs())
                        } else {
                            a.to_bits() == b.to_bits()
                        };
                        if !ok {
                            first = Some((f, a, b));
                            break 'files;
                        }
                    }
                }
            }
            let ho = order_ex(&hk, excl);
            let order_bad = ho.len() != fr.order.len() || ho.iter().zip(&fr.order).any(|(&a, &b)| a != b as usize);
            (first.is_some(), order_bad, first)
        })
        .collect()
}

/// The parity report (`--engine-parity on`): per shape (single, k2, k3, half) and ranking, the
/// queries checked and those whose keys / order differ from the harness's; plus the first single-
/// shape key mismatch, `(query id, file, ranking, harness, engine)`.
struct ParityReport {
    json: serde_json::Value,
    md: String,
    first_single: Option<String>,
}

fn engine_parity(ctx: &Ctx, queries: &[Query], tqs: &[TQuery], tc_sha: &[String], label: &str) -> ParityReport {
    let g = ctx.g;
    let nf = g.paths.len();
    let t0 = Instant::now();
    // One shared cache (no roots, no labels): its PPR indexes and carry adjacency serve every
    // query; each query's C0 comes from its test commit's prefix `CochangeIndex` through the
    // engine's own `rank::c0`, given to `rank_files_with_c0` (what `rank_files` computes from a
    // cache's index), so the indexes are built once instead of once per test commit.
    let cache = RankCache::new("parity", &BTreeMap::new(), &BTreeMap::new());
    cache.guard(ctx.merged);
    if nf > 0 {
        // Warm the indexes on one thread, so the parallel queries hit them instead of racing.
        for r in parity_rankings() {
            let _ = glia_engine::rank::rank_files_with_c0(ctx.merged, &cache, &[0], &[], r, Some(&[]));
        }
    }
    let idx_of = |tc: usize| ctx.prefix.get(tc).map(|p| p.index(nf));
    // (shape, query id, rows)
    let single: Vec<(usize, String, ParityRow)> = queries
        .par_iter()
        .map(|q| {
            let seed = q.seed as usize;
            let mut excl = vec![false; nf];
            excl[seed] = true;
            let c0h = dense(nf, &q.c0, seed);
            let c0e = idx_of(q.tc).map_or_else(|| vec![0.0; nf], |ix| glia_engine::rank::c0(&ix, &[q.seed], nf));
            let id = format!("{}:{}", tc_sha.get(q.tc).map_or("?", String::as_str), g.paths[seed]);
            (0usize, id, parity_query(ctx, &cache, &[q.seed], &excl, &c0h, &c0e, false))
        })
        .collect();
    let review: Vec<(usize, String, ParityRow)> = tqs
        .par_iter()
        .filter(|q| q.shape < 3)
        .map(|q| {
            let mut excl = vec![false; nf];
            for &s in &q.seeds {
                excl[s as usize] = true;
            }
            let mut c0h = vec![0.0; nf];
            for &(f, v) in &q.c0 {
                c0h[f as usize] = v;
            }
            let c0e = idx_of(q.tc).map_or_else(|| vec![0.0; nf], |ix| glia_engine::rank::c0(&ix, &q.seeds, nf));
            let id = format!("{}:{}", tc_sha.get(q.tc).map_or("?", String::as_str), R3_SHAPES[q.shape]);
            (q.shape + 1, id, parity_query(ctx, &cache, &q.seeds, &excl, &c0h, &c0e, true))
        })
        .collect();
    let shapes = ["single", "k2", "k3", "half"];
    let rankings = parity_rankings();
    // [shape][ranking] -> (queries, key mismatches, order mismatches)
    let mut tally = vec![vec![(0usize, 0usize, 0usize); rankings.len()]; shapes.len()];
    let mut first_single: Option<String> = None;
    for (shape, id, row) in single.iter().chain(review.iter()) {
        for (ri, (kb, ob, first)) in row.iter().enumerate() {
            let t = &mut tally[*shape][ri];
            t.0 += 1;
            t.1 += usize::from(*kb);
            t.2 += usize::from(*ob);
            if *shape == 0
                && first_single.is_none()
                && let Some((f, a, b)) = first
            {
                let file = g.paths.get(*f).map_or("<key length>", String::as_str);
                first_single = Some(format!("query {id} file {file} ranking {}: harness={a:e} engine={b:e}", rankings[ri].harness_name()));
            }
        }
    }
    let mut js = serde_json::Map::new();
    let mut md = String::from("\n## Engine parity\n\n`--engine-parity on` (EA.9): the engine's `rank::rank_files` keys (f64 bits; the review shapes' C0 within 1e-12 relative) and order against this harness's, per query, fed the replay's leak-free prefix co-change.\n\n| shape | ranking | queries | key mismatches | order mismatches |\n|---|---|---:|---:|---:|\n");
    for (si, shape) in shapes.iter().enumerate() {
        let mut per = serde_json::Map::new();
        for (ri, r) in rankings.iter().enumerate() {
            let (n, k, o) = tally[si][ri];
            if n == 0 {
                continue;
            }
            eprintln!(
                "[rank-parity] label={label} shape={shape} ranking={} queries={n} key_mismatch={k} order_mismatch={o}",
                r.harness_name()
            );
            per.insert(r.harness_name().into(), json!({"queries": n, "key_mismatch": k, "order_mismatch": o}));
            md.push_str(&format!("| {shape} | {} | {n} | {k} | {o} |\n", r.harness_name()));
        }
        if !per.is_empty() {
            js.insert((*shape).into(), serde_json::Value::Object(per));
        }
    }
    eprintln!("[rank-parity] label={label} done in {:.1}s", t0.elapsed().as_secs_f64());
    ParityReport { json: serde_json::Value::Object(js), md, first_single }
}

// ============================================================================
// Engine intent parity (EB.9): `--engine on`
// ============================================================================

/// `glia history sync`'s default `--max-commits`: the history snapshot holds only the newest
/// this-many non-merge commits, so the engine's rename map (`intent::paths::rename_pairs`) covers
/// only their renames, where `git_renames` reads all history. ENG-LX[IDRw,..] measures that.
const ENG_RENAME_WINDOW: usize = 2000;

/// One test commit's intent signals computed by the engine (`glia_engine::intent`), dense over
/// the candidate files, from inputs this harness already reads (the strictly older commits, the
/// test commit's parent): the ENG-* rankers of `--engine on`.
struct EngCommit {
    /// `TextIndex::from_docs` over the older commits' raw `%B`, `--topo-order` (T1s' own corpus).
    t1s_raw: Vec<f64>,
    /// The same docs as `history sync` stores them (`clean_message`: subject `\n` body).
    t1s_clean: Vec<f64>,
    /// The raw docs in plain `git log` order (the snapshot's), not `--topo-order`.
    t1s_plain: Vec<f64>,
    /// `intent::idr::route` of the subject at the parent, renames over all history.
    idr: Vec<f64>,
    /// The same with only the snapshot window's renames (`ENG_RENAME_WINDOW`).
    idr_win: Vec<f64>,
    /// The route of every id row `route` returned (`RoutedId::route`).
    routes: Vec<&'static str>,
    has_id: bool,
    /// ENG-T1s-raw's top 20 files or their score bits differ from the harness's T1s.
    mismatch: bool,
}

/// The positive scores' top 20 as (file, score bits), by score then file: the parity unit.
fn eng_top20(v: &[(u32, f64)]) -> Vec<(u32, u64)> {
    let mut s: Vec<(u32, f64)> = v.iter().copied().filter(|x| x.1 > 0.0).collect();
    s.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    s.truncate(20);
    s.into_iter().map(|(f, x)| (f, x.to_bits())).collect()
}

/// A fused order (`algo::fuse`, best first) as harness keys: position i of n keys n - i, so
/// `better` reproduces the order; a file outside it (an excluded seed) keys 0.
fn eng_keys(order: &[usize], nf: usize) -> Vec<Key> {
    let mut k = vec![[0.0; 3]; nf];
    for (pos, &f) in order.iter().enumerate() {
        k[f] = [(order.len() - pos) as f64, 0.0, 0.0];
    }
    k
}

/// `(old, new)` renames, oldest first, as `git_renames`, kept only from commits in `win`.
fn git_renames_in(git: &str, rev: &str, win: &HashSet<String>) -> Vec<(String, String)> {
    let text = git_text(git, &["log", "--no-merges", "-M", "--diff-filter=R", "--name-status", "--format=%x1e%H", rev]).unwrap_or_default();
    let mut out = Vec::new();
    for rec in text.split('\x1e').skip(1) {
        let mut lines = rec.lines();
        if !lines.next().is_some_and(|sha| win.contains(sha.trim())) {
            continue;
        }
        for l in lines {
            let mut p = l.split('\t');
            if p.next().is_some_and(|st| st.starts_with('R'))
                && let (Some(a), Some(b)) = (p.next(), p.next())
            {
                out.push((a.to_string(), b.to_string()));
            }
        }
    }
    out.reverse();
    out
}

/// `TextIndex::from_docs` over `commits[ix]` in that order (oldest first), the text of commit
/// `j` `text(j)`, scored for `q`.
fn eng_t1s<'a>(nf: usize, commits: &'a [Commit], ix: &[usize], text: impl Fn(usize) -> &'a str, q: &str) -> Vec<f64> {
    TextIndex::from_docs(nf, ix.iter().map(|&j| (commits[j].sha.as_str(), text(j), commits[j].files.clone()))).scores(q)
}

/// The ENG-* inputs of every test commit (aligned with `test_idx`). Text: `TextIndex::from_docs`
/// over `commits[..k]` (from_docs skips what `train_ok` skips), queried with the subject. IDR:
/// `read_defs` at the parent with the vocabulary of the parent's `.glia/overlay.toml` (else
/// `--eng-overlay`'s file, else the defaults), `PathResolver` over the engine's `Candidates`
/// and `git_renames`, recent scopes over the earlier test commits' subjects.
fn eng_build(args: &Args, merged: &MergedGraph, g: &Graph, commits: &[Commit], test_idx: &[usize]) -> Vec<EngCommit> {
    let t0 = Instant::now();
    let nf = g.paths.len();
    let fi = FileIndex::of(merged, &BTreeMap::new());
    let repo = merged.graphs.first().map_or(0, |gr| gr.repo.0);
    let cands = Candidates::of(&fi, repo);
    assert!(cands.paths == g.paths, "[rank-eval] engine parity: Candidates differ from the harness's candidate files");
    let deg_differ = cands.deg.iter().zip(&g.file_deg).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
    let cleaned: Vec<String> = commits
        .par_iter()
        .map(|c| {
            let (s, b, _) = clean_message(&c.msg);
            format!("{s}\n{b}")
        })
        .collect();
    let plain = git_text(&args.git, &["log", "--no-merges", "--format=%H", &args.rev]).expect("git log plain order");
    let plain_pos: HashMap<&str, usize> = plain.lines().enumerate().map(|(i, s)| (s, i)).collect();
    let parents = git_parents(&args.git, &args.rev);
    let renames = git_renames(&args.git, &args.rev);
    let win: HashSet<String> = git_text(&args.git, &["rev-list", "--no-merges", &format!("--max-count={ENG_RENAME_WINDOW}"), &args.rev])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    let renames_win = git_renames_in(&args.git, &args.rev, &win);
    let fallback: Option<String> =
        (!args.eng_overlay.is_empty()).then(|| std::fs::read_to_string(&args.eng_overlay).expect("--eng-overlay: read"));
    let vocab_at = |p: &str| -> IdVocab {
        match git_text(&args.git, &["show", &format!("{p}:.glia/overlay.toml")]).or_else(|| fallback.clone()) {
            Some(text) => IdVocab::from_config(Some(&parse_str(&text))),
            None => IdVocab::default(),
        }
    };
    let out: Vec<EngCommit> = (0..test_idx.len())
        .into_par_iter()
        .map(|o| {
            let k = test_idx[o];
            let c = &commits[k];
            let q = subject(&c.msg);
            let topo: Vec<usize> = (0..k).collect();
            let mut by_plain = topo.clone();
            by_plain.sort_by_key(|&j| std::cmp::Reverse(plain_pos.get(commits[j].sha.as_str()).copied().unwrap_or(usize::MAX)));
            let t1s_raw = eng_t1s(nf, commits, &topo, |j| commits[j].msg.as_str(), q);
            let t1s_clean = eng_t1s(nf, commits, &topo, |j| cleaned[j].as_str(), q);
            let t1s_plain = eng_t1s(nf, commits, &by_plain, |j| commits[j].msg.as_str(), q);
            let Some(p) = parents.get(&c.sha).and_then(|x| x.0.clone()) else {
                let z = vec![0.0; nf];
                return EngCommit { t1s_raw, t1s_clean, t1s_plain, idr: z.clone(), idr_win: z, routes: Vec::new(), has_id: false, mismatch: false };
            };
            let tree = Tree::Rev { repo: std::path::Path::new(&args.git), sha: p.clone() };
            let vocab = vocab_at(&p);
            let defs = read_defs(&tree, &[], &vocab, None);
            let ns = defs.namespaces(&vocab.local_words);
            let earlier: Vec<&str> = test_idx[..o].iter().rev().map(|&j| subject(&commits[j].msg)).collect();
            let recent = eidr::recent_scopes(&earlier, &ns, &vocab);
            let paths = PathResolver::new(&cands, &renames);
            let side = eidr::IdrSide::new(&cands, &paths, &defs, &ns, &vocab, &tree).labelled(&args.label, "rank_eval");
            let x = eidr::route(q, &side, &recent);
            let idr_win = if renames_win == renames {
                x.idr.clone()
            } else {
                let pw = PathResolver::new(&cands, &renames_win);
                let sw = eidr::IdrSide::new(&cands, &pw, &defs, &ns, &vocab, &tree).labelled(&args.label, "rank_eval-window");
                eidr::route(q, &sw, &recent).idr
            };
            let routes = x.ids.iter().map(|r| r.route).collect();
            EngCommit { t1s_raw, t1s_clean, t1s_plain, idr: x.idr, idr_win, routes, has_id: x.has_id, mismatch: false }
        })
        .collect();
    eprintln!(
        "[rank-eval] engine inputs repo={} test_commits={} degree_differ={deg_differ} renames={} window_renames={} s={:.1}",
        args.label,
        out.len(),
        renames.len(),
        renames_win.len(),
        t0.elapsed().as_secs_f64()
    );
    out
}

/// ENG-* rankers of the single-seed shape (appended after every other ranker): the engine's
/// T1s variants and IDR, and the LX fusions through `algo::fuse` over the harness's own P1 and
/// T1s, so a difference from LX[IDR,P1] / LX[IDR,H9s50] isolates IDR.
fn eng_rankers_single(out: &mut Out, e: &EngCommit, r: &R1Commit, seed: usize, t: &[u32], p1: &[f64]) {
    let nf = p1.len();
    let tie: Vec<u32> = (0..nf as u32).collect();
    let mut excl = vec![false; nf];
    excl[seed] = true;
    let z = |v: &[f64]| -> Vec<f64> {
        let mut x = v.to_vec();
        x[seed] = 0.0;
        x
    };
    let t1s = dense(nf, &r.t1s, seed);
    let (idr, idr_win) = (z(&e.idr), z(&e.idr_win));
    let h9s50 = fuse::blend(0.5, &fuse::minmax(&t1s, &excl), &fuse::minmax(p1, &excl));
    out.push_eng("ENG-T1s-raw", &scalar(&z(&e.t1s_raw)), seed, t);
    out.push_eng("ENG-T1s", &scalar(&z(&e.t1s_clean)), seed, t);
    out.push_eng("ENG-T1s-plain", &scalar(&z(&e.t1s_plain)), seed, t);
    out.push_eng("ENG-IDR", &scalar(&idr), seed, t);
    out.push_eng("ENG-LX[IDR,P1]", &eng_keys(&fuse::lx(&idr, &[p1], &tie, &excl), nf), seed, t);
    out.push_eng("ENG-LX[IDR,H9s50]", &eng_keys(&fuse::lx(&idr, &[&h9s50], &tie, &excl), nf), seed, t);
    out.push_eng("ENG-LX[IDRw,H9s50]", &eng_keys(&fuse::lx(&idr_win, &[&h9s50], &tie, &excl), nf), seed, t);
}

/// ENG-* rankers of a transfer query (appended after the ANS rankers): review shapes
/// ENG-LX[IDR,P1] / ENG-LX[IDR,H9s50], the find shape ENG-T1s and ENG-LX[IDR,T1s] with the
/// find shape's tie rule (`r3_bank_find`: the file's degree after the last key; the engine's
/// `find_files` is `lex_order([idr, text, degree])`).
fn eng_bank_transfer(e: &EngCommit, excl: &[bool], p1: &[f64], t1s: &[f64], deg: &[f64], find: bool) -> Vec<(String, Vec<Key>)> {
    let nf = p1.len();
    let tie: Vec<u32> = (0..nf as u32).collect();
    if find {
        return vec![
            ("ENG-T1s".into(), eng_keys(&fuse::lex_order(&[&e.t1s_clean, deg], &tie, excl), nf)),
            ("ENG-LX[IDR,T1s]".into(), eng_keys(&fuse::lex_order(&[&e.idr, t1s, deg], &tie, excl), nf)),
        ];
    }
    let h9s50 = fuse::blend(0.5, &fuse::minmax(t1s, excl), &fuse::minmax(p1, excl));
    vec![
        ("ENG-LX[IDR,P1]".into(), eng_keys(&fuse::lx(&e.idr, &[p1], &tie, excl), nf)),
        ("ENG-LX[IDR,H9s50]".into(), eng_keys(&fuse::lx(&e.idr, &[&h9s50], &tie, excl), nf)),
    ]
}

/// The contrasts `--engine on` reports, per shape: (ranker, against).
const ENG_CONTRASTS: [(&str, &[(&str, &str)]); 3] = [
    (
        "single",
        &[
            ("ENG-T1s-raw", "T1s"),
            ("ENG-T1s", "T1s"),
            ("ENG-T1s-plain", "T1s"),
            ("ENG-IDR", "IDR"),
            ("ENG-LX[IDR,P1]", "LX[IDR,P1]"),
            ("ENG-LX[IDR,P1]", "P1"),
            ("ENG-LX[IDR,H9s50]", "LX[IDR,H9s50]"),
            ("ENG-LX[IDR,H9s50]", "H9s50"),
            ("ENG-LX[IDRw,H9s50]", "ENG-LX[IDR,H9s50]"),
        ],
    ),
    ("review", &[("ENG-LX[IDR,P1]", "LX[IDR,P1]"), ("ENG-LX[IDR,P1]", "P1"), ("ENG-LX[IDR,H9s50]", "LX[IDR,H9s50]"), ("ENG-LX[IDR,H9s50]", "H9s50")]),
    ("find", &[("ENG-T1s", "T1s"), ("ENG-LX[IDR,T1s]", "LX[IDR,T1s]"), ("ENG-LX[IDR,T1s]", "T1s")]),
];

/// The `--engine on` report: the fired_on line, the JSON and the round-1 md's
/// `### Engine parity` table (each contrast's r10 and MRR difference with its commit-cluster CI,
/// and the queries where the two differ at all).
fn eng_report(label: &str, results: &[Out], r1s: &[R1Commit], transfer: &[(Vec<Out>, Vec<usize>)], bi: &BootIn) -> (serde_json::Value, String) {
    let ntc = r1s.len();
    let eng: Vec<&EngCommit> = r1s.iter().filter_map(|r| r.eng.as_ref()).collect();
    let mismatch = eng.iter().filter(|e| e.mismatch).count();
    let mut routes: BTreeMap<&str, usize> = BTreeMap::new();
    for e in &eng {
        for r in &e.routes {
            *routes.entry(r).or_default() += 1;
        }
    }
    let routes_s = if routes.is_empty() { "none".to_string() } else { routes.iter().map(|(r, n)| format!("{r}:{n}")).collect::<Vec<_>>().join(",") };
    let has_id = eng.iter().filter(|e| e.has_id).count();
    let mean_of = |outs: &[Out], n: &str, mi: usize| -> Option<f64> {
        let i = outs.first()?.names.iter().position(|x| x == n)?;
        Some(outs.iter().map(|o| o.m[i][mi]).sum::<f64>() / outs.len() as f64)
    };
    let fmt = |v: Option<f64>| v.map_or_else(|| "-".to_string(), |x| format!("{x:.4}"));
    eprintln!(
        "[rank-eval] engine parity repo={label} t1s_raw_mismatch={mismatch}/{} t1s_clean_r10={} t1s_r10={} idr_r10={} harness_idr_r10={} routes={routes_s}",
        eng.len(),
        fmt(mean_of(results, "ENG-T1s", 1)),
        fmt(mean_of(results, "T1s", 1)),
        fmt(mean_of(results, "ENG-IDR", 1)),
        fmt(mean_of(results, "IDR", 1)),
    );
    let mut md = format!(
        "\n### Engine parity\n\n`--engine on` (EB.9): T1s, IDR and the LX fusions recomputed through `glia_engine::intent` and `algo::fuse` from the replay's own inputs. ENG-T1s-raw is `TextIndex::from_docs` over the strictly older commits' raw `%B`: its top 20 files and score bits differ from T1s on **{mismatch} of {} test commits**. ENG-T1s indexes the messages as `history sync` stores them (`clean_message`), ENG-T1s-plain the raw messages in plain `git log` order; ENG-IDR is `intent::idr::route` at each parent (the parent's `[ids]`, {has_id} test commits with a routed id or scope; routes {routes_s}); ENG-LX[IDRw,..] reads only the newest {ENG_RENAME_WINDOW} commits' renames. Each row: the ranker's mean, then ranker - against with its commit-cluster bootstrap 95% CI, and the queries where they differ.\n\n| shape | ranker | against | queries | r10 | d r10 [CI] | d MRR [CI] | differ |\n|---|---|---|---:|---:|---|---|---:|\n",
        eng.len()
    );
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut emit = |shape: &str, outs: &[Out], commit_of: &[usize], a: &str, b: &str| {
        let Some(first) = outs.first() else { return };
        let (Some(ia), Some(ib)) = (first.names.iter().position(|x| x == a), first.names.iter().position(|x| x == b)) else { return };
        let col = |i: usize, mi: usize| -> Vec<f64> { outs.iter().map(|o| o.m[i][mi]).collect() };
        let p10 = paired(&col(ia, 1), &col(ib, 1), commit_of, ntc, bi.boot, bi.lf);
        let pm = paired(&col(ia, 3), &col(ib, 3), commit_of, ntc, bi.boot, bi.lf);
        let differ = outs.iter().filter(|o| o.m[ia] != o.m[ib]).count();
        let r10 = col(ia, 1).iter().sum::<f64>() / outs.len() as f64;
        md.push_str(&format!(
            "| {shape} | {a} | {b} | {} | {r10:.4} | {:+.4} [{:+.4}, {:+.4}] | {:+.4} [{:+.4}, {:+.4}] | {differ} |\n",
            outs.len(),
            p10.mean,
            p10.lo,
            p10.hi,
            pm.mean,
            pm.lo,
            pm.hi
        ));
        rows.push(json!({"shape": shape, "ranker": a, "vs": b, "queries": outs.len(), "r10": r10,
            "d_r10": p10.mean, "d_r10_lo": p10.lo, "d_r10_hi": p10.hi, "d_mrr": pm.mean, "d_mrr_lo": pm.lo, "d_mrr_hi": pm.hi, "differ": differ}));
    };
    for (shape, pairs) in ENG_CONTRASTS {
        let sets: Vec<(&str, &[Out], &[usize])> = match shape {
            "single" => vec![("single", results, bi.commit_of)],
            "review" => (0..3).filter_map(|si| transfer.get(si).map(|(o, c)| (R3_SHAPES[si], o.as_slice(), c.as_slice()))).collect(),
            "find" => transfer.get(3).map(|(o, c)| ("find", o.as_slice(), c.as_slice())).into_iter().collect(),
            _ => Vec::new(),
        };
        for (sname, outs, commit_of) in sets {
            for (a, b) in pairs.iter() {
                emit(sname, outs, commit_of, a, b);
            }
        }
    }
    let js = json!({"test_commits": eng.len(), "t1s_raw_mismatch": mismatch, "with_id": has_id, "routes": routes, "rename_window": ENG_RENAME_WINDOW, "contrasts": rows});
    (js, md)
}

fn run_query(ctx: &Ctx, q: &Query, tc: &TestCommit, push: &mut PushPpr, vl: Option<&VLayer>, r1: Option<&R1Commit>) -> Out {
    let g = ctx.g;
    let nf = g.paths.len();
    let seed = q.seed as usize;
    let t = &q.targets;
    let seed_nodes: Vec<u32> = g.file_nodes[seed].clone();
    let seed_ids: Vec<NodeId> = seed_nodes.iter().map(|&u| g.ids[u as usize]).collect();
    let seed_ppr: Vec<usize> = seed_nodes.iter().map(|&u| g.node_ppr[u as usize]).collect();
    let mut out = Out::default();

    // ---- structural rankers (graph or path only) ----
    // (name, key, is_scalar)
    let mut structural: Vec<(String, Vec<Key>, bool)> = Vec::new();
    let (b0, depth) = bfs_key(ctx, &ctx.adj_carry, &seed_ids);
    structural.push(("B0".into(), b0, false));
    let (b0i, _) = bfs_key(ctx, &ctx.adj_imports, &seed_ids);
    structural.push(("B0i".into(), b0i, false));
    structural.push(("B1".into(), scalar(&g.file_deg), true));
    let b3: Vec<Key> =
        (0..nf).map(|f| [shared_dir_segments(&g.paths[seed], &g.paths[f]) as f64, g.file_deg[f], 0.0]).collect();
    structural.push(("B3".into(), b3, false));

    let pers = Ppr::uniform(&seed_ppr);
    let mut p1 = Vec::new();
    let mut p1sum_v = Vec::new();
    let mut p1_vec = Vec::new();
    for (name, idx) in &ctx.ppr {
        let v = idx.run(&pers);
        let fs = file_pool(g, |u| v[g.node_ppr[u]], false);
        if *name == "P1" {
            let fsum = file_pool(g, |u| v[g.node_ppr[u]], true);
            structural.push(("P1".into(), scalar(&fs), true));
            structural.push(("P1sum".into(), scalar(&fsum), true));
            p1sum_v = fsum;
            p1 = fs;
            p1_vec = v;
        } else {
            structural.push(((*name).into(), scalar(&fs), true));
        }
    }
    let p2 = file_pool(g, |u| p1_vec[g.node_ppr[u]] / (1.0 + g.deg_all[u]), false);
    let p2i = file_pool(g, |u| p1_vec[g.node_ppr[u]] * (1.0 + g.deg_all[u]).ln_1p(), false);
    structural.push(("P2".into(), scalar(&p2), true));
    structural.push(("P2i".into(), scalar(&p2i), true));
    let p3 = push_scores(ctx, push, &seed_ids);
    structural.push(("P3".into(), scalar(&p3), true));
    for (cname, cl, m) in &ctx.clusters {
        structural.push((format!("K1{cname}"), pooled(&p1, cl, *m), false));
    }
    for (cname, cl, m) in &ctx.clusters {
        structural.push((format!("K2{cname}"), pooled(&p2, cl, *m), false));
    }
    structural.push(("B2".into(), scalar(&ctx.path_hash), true));
    for (name, key, _) in &structural {
        out.push(name.clone(), key, seed, t);
    }

    // ---- co-change and its late-fusion hybrids ----
    let c0 = dense(nf, &q.c0, seed);
    out.push("C0", &scalar(&c0), seed, t);
    let mm_p1 = minmax(&p1, seed);
    let mm_p2 = minmax(&p2, seed);
    let mm_c0 = minmax(&c0, seed);
    let rn_p1 = ranknorm(&scalar(&p1), seed);
    let rn_c0 = ranknorm(&scalar(&c0), seed);
    for a in ALPHAS {
        out.push(format!("H1a{}", (a * 100.0) as u32), &scalar(&blend(a, &mm_p1, &mm_c0)), seed, t);
    }
    for a in ALPHAS {
        out.push(format!("H1r{}", (a * 100.0) as u32), &scalar(&blend(a, &rn_p1, &rn_c0)), seed, t);
    }
    for a in ALPHAS {
        out.push(format!("H2a{}", (a * 100.0) as u32), &scalar(&blend(a, &mm_p2, &mm_c0)), seed, t);
    }
    let k_p1 = scalar(&p1);
    let k_c0 = scalar(&c0);
    let pos_p1 = positive(&p1);
    let pos_c0 = positive(&c0);
    out.push("H3", &scalar(&rrf(&[(&k_p1, &pos_p1), (&k_c0, &pos_c0)], seed)), seed, t);
    let h4: Vec<Key> = (0..nf)
        .map(|f| if depth[f] <= 3.0 { [1.0, c0[f], p1[f]] } else { [0.0, 0.0, p1[f]] })
        .collect();
    out.push("H4", &h4, seed, t);
    for (name, key, _) in structural.iter().filter(|(n, _, _)| n.starts_with('K')) {
        let pos: Vec<bool> = key.iter().map(|k| k[0] > 0.0 || k[1] > 0.0).collect();
        out.push(format!("H5[{name}]"), &scalar(&rrf(&[(key, &pos), (&k_c0, &pos_c0)], seed)), seed, t);
    }
    let h10: Vec<Key> = (0..nf).map(|f| [c0[f], p1[f], 0.0]).collect();
    out.push("H10", &h10, seed, t);

    // ---- past tasks (intent) ----
    let t1 = dense(nf, &tc.t1, seed);
    let t0 = dense(nf, &tc.t0, seed);
    let t2 = dense(nf, &q.t2, seed);
    out.push("T0", &scalar(&t0), seed, t);
    out.push("T1", &scalar(&t1), seed, t);
    out.push("T2", &scalar(&t2), seed, t);
    let k_t1 = scalar(&t1);
    let pos_t1 = positive(&t1);
    out.push("H6", &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_p1, &pos_p1)], seed)), seed, t);
    out.push("H7", &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_c0, &pos_c0)], seed)), seed, t);
    out.push(
        "H8",
        &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_c0, &pos_c0), (&k_p1, &pos_p1)], seed)),
        seed,
        t,
    );
    let mm_t1 = minmax(&t1, seed);
    for (name, key, is_scalar) in &structural {
        if name == "B2" {
            continue;
        }
        let norm = if *is_scalar { minmax(&key.iter().map(|k| k[0]).collect::<Vec<_>>(), seed) } else { ranknorm(key, seed) };
        for a in ALPHAS {
            out.push(format!("H9a{}[{name}]", (a * 100.0) as u32), &scalar(&blend(a, &mm_t1, &norm)), seed, t);
        }
    }
    // Post-hoc: equal-weight min-max blend of T1, C0 and P1.
    let h18: Vec<f64> = (0..nf).map(|f| (mm_t1[f] + mm_c0[f] + mm_p1[f]) / 3.0).collect();
    out.push("H18", &scalar(&h18), seed, t);
    // T1 then C0 then P1, lexicographic; and the three-way RRF with T2.
    let h11: Vec<Key> = (0..nf).map(|f| [t1[f], c0[f], p1[f]]).collect();
    out.push("H11", &h11, seed, t);
    let k_t2 = scalar(&t2);
    let pos_t2 = positive(&t2);
    out.push(
        "H12",
        &scalar(&rrf(&[(&k_t2, &pos_t2), (&k_c0, &pos_c0), (&k_p1, &pos_p1)], seed)),
        seed,
        t,
    );

    // ---- use-weighted PPR (early fusion) ----
    let mut s = vec![0.0f32; ctx.n_pairs];
    for &(p, v) in &tc.s_pairs {
        s[p as usize] = v;
    }
    let mut t1_top: Vec<usize> = order(&k_t1, seed).into_iter().filter(|&f| t1[f] > 0.0).take(10).collect();
    t1_top.sort_unstable();
    // U2: every seed node at 1, every node of a top-10 T1 file at 0.25.
    let mut pers_node: Vec<(usize, f64)> = seed_ppr.iter().map(|&i| (i, 1.0)).collect();
    // U2f: the seed file at 1 and each top-10 T1 file at 0.25, spread over its nodes.
    let mut pers_file: Vec<(usize, f64)> = seed_ppr.iter().map(|&i| (i, 1.0 / seed_ppr.len() as f64)).collect();
    for &f in &t1_top {
        let ns = &g.file_nodes[f];
        for &u in ns {
            pers_node.push((g.node_ppr[u as usize], 0.25));
            pers_file.push((g.node_ppr[u as usize], 0.25 / ns.len() as f64));
        }
    }
    let norm_pers = |p: &[(usize, f64)]| -> Vec<(usize, f64)> {
        let tot: f64 = p.iter().map(|x| x.1).sum();
        p.iter().map(|&(i, w)| (i, w / tot)).collect()
    };
    let pers_node = norm_pers(&pers_node);
    let pers_file = norm_pers(&pers_file);
    let mut u_scores: Vec<(String, Vec<f64>)> = Vec::new();
    for beta in U_BETAS {
        let (inc, ow) = ctx.upr.reweight(&s, beta);
        let b = beta as u32;
        let v1 = ctx.upr.power(&pers, &inc, &ow);
        u_scores.push((format!("U1b{b}"), file_pool(g, |u| v1[g.node_ppr[u]], false)));
        let v2 = ctx.upr.power(&pers_node, &inc, &ow);
        u_scores.push((format!("U2b{b}"), file_pool(g, |u| v2[g.node_ppr[u]], false)));
        let v3 = ctx.upr.power(&pers_file, &inc, &ow);
        u_scores.push((format!("U2fb{b}"), file_pool(g, |u| v3[g.node_ppr[u]], false)));
    }
    // U3 (post-hoc, after the first run): U1b5 with the top-10 T1 files in the
    // restart set weighted by their T1 score (0.25 x t1 / max t1, spread over
    // the file's nodes; the seed file at 1, spread), max- and sum-pooled.
    {
        let t1_max = t1_top.iter().map(|&f| t1[f]).fold(0.0f64, f64::max);
        let mut pers_t: Vec<(usize, f64)> = seed_ppr.iter().map(|&i| (i, 1.0 / seed_ppr.len() as f64)).collect();
        for &f in &t1_top {
            let ns = &g.file_nodes[f];
            for &u in ns {
                pers_t.push((g.node_ppr[u as usize], 0.25 * t1[f] / t1_max / ns.len() as f64));
            }
        }
        let pers_t = norm_pers(&pers_t);
        let (inc, ow) = ctx.upr.reweight(&s, 5.0);
        let v = ctx.upr.power(&pers_t, &inc, &ow);
        u_scores.push(("U3b5".into(), file_pool(g, |u| v[g.node_ppr[u]], false)));
        u_scores.push(("U3sb5".into(), file_pool(g, |u| v[g.node_ppr[u]], true)));
    }
    for (name, v) in &u_scores {
        out.push(name.clone(), &scalar(v), seed, t);
    }
    // Post-hoc late fusions: RRF(T1, C0, X) for stronger structural X.
    for (hname, base) in [("H13", "P1sum"), ("H14", "K1svc"), ("H16", "B0"), ("H17", "P2")] {
        let (_, key, _) = structural.iter().find(|(n, _, _)| n == base).expect("base");
        let pos: Vec<bool> = if base == "B0" {
            depth.iter().map(|d| d.is_finite()).collect()
        } else {
            key.iter().map(|k| k[0] > 0.0 || k[1] > 0.0).collect()
        };
        out.push(hname, &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_c0, &pos_c0), (key, &pos)], seed)), seed, t);
    }
    if let Some((_, v)) = u_scores.iter().find(|(n, _)| n == "U1b5") {
        let k = scalar(v);
        let pos = positive(v);
        out.push("H15", &scalar(&rrf(&[(&k_t1, &pos_t1), (&k_c0, &pos_c0), (&k, &pos)], seed)), seed, t);
    }

    // ---- vertical layers (second run; appended, so every ranker above is unchanged) ----
    if let Some(vl) = vl {
        vertical_rankers(&mut out, vl, seed, t, &p1, &t1, &ctx.direct);
    }

    // ---- round 1 (appended, so every ranker above is unchanged) ----
    if let Some(r1) = r1 {
        round1_rankers(&mut out, ctx, r1, q, t, &p1, &p1sum_v, &c0, &t1, &pers, &structural[0].1);
    }

    // ---- the shipped answers (`--ans on`, appended last, so every ranker above is unchanged) ----
    if ctx.ans {
        for (name, key) in ans_keys_seed(ctx, q.tc, &[q.seed]) {
            out.push(name, &key, seed, t);
        }
    }

    // ---- the engine's intent signals (`--engine on`, EB.9; after the ANS rankers) ----
    if let Some(r) = r1
        && let Some(e) = &r.eng
    {
        eng_rankers_single(&mut out, e, r, seed, t, &p1);
    }

    // ---- hub flooding ----
    out.hub_share.push(("P1".into(), hub_share(&p1, &ctx.hubs, seed)));
    out.hub_share.push(("P2".into(), hub_share(&p2, &ctx.hubs, seed)));
    for (name, key, _) in &structural {
        if name == "P1f" || name == "P1b" || name == "P1review" {
            out.hub_share.push((name.clone(), hub_share(&key.iter().map(|k| k[0]).collect::<Vec<_>>(), &ctx.hubs, seed)));
        }
    }
    if let Some((_, v)) = u_scores.iter().find(|(n, _)| n == "U1b5") {
        out.hub_share.push(("U1b5".into(), hub_share(v, &ctx.hubs, seed)));
    }
    let _ = ctx.merged;
    out
}

// ============================================================================
// Paired statistics
// ============================================================================

struct Paired {
    mean: f64,
    lo: f64,
    hi: f64,
    p: f64,
    wins: usize,
    losses: usize,
}

fn ln_fact_table(n: usize) -> Vec<f64> {
    let mut t = vec![0.0; n + 1];
    for i in 1..=n {
        t[i] = t[i - 1] + (i as f64).ln();
    }
    t
}

fn sign_test(w: usize, l: usize, lf: &[f64]) -> f64 {
    let n = w + l;
    if n == 0 {
        return 1.0;
    }
    let k = w.min(l);
    let ln2n = n as f64 * std::f64::consts::LN_2;
    let tail: f64 = (0..=k).map(|i| (lf[n] - lf[i] - lf[n - i] - ln2n).exp()).sum();
    (2.0 * tail).min(1.0)
}

fn paired(a: &[f64], b: &[f64], commit_of: &[usize], n_commits: usize, boot: usize, lf: &[f64]) -> Paired {
    paired_boot(a, b, commit_of, n_commits, boot, lf).0
}

/// `paired` with its sorted bootstrap means (the rank gate's one-sided p-values read them).
fn paired_boot(a: &[f64], b: &[f64], commit_of: &[usize], n_commits: usize, boot: usize, lf: &[f64]) -> (Paired, Vec<f64>) {
    let diffs: Vec<f64> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    let mean = diffs.iter().sum::<f64>() / diffs.len() as f64;
    let wins = diffs.iter().filter(|&&d| d > 1e-12).count();
    let losses = diffs.iter().filter(|&&d| d < -1e-12).count();
    let mut csum = vec![0.0; n_commits];
    let mut ccnt = vec![0usize; n_commits];
    for (q, &d) in diffs.iter().enumerate() {
        csum[commit_of[q]] += d;
        ccnt[commit_of[q]] += 1;
    }
    let mut rng = Rng(0x5eed_2026_1018);
    let mut means = Vec::with_capacity(boot);
    for _ in 0..boot {
        let (mut s, mut c) = (0.0, 0usize);
        for _ in 0..n_commits {
            let k = (rng.next() % n_commits as u64) as usize;
            s += csum[k];
            c += ccnt[k];
        }
        means.push(if c > 0 { s / c as f64 } else { 0.0 });
    }
    means.sort_by(f64::total_cmp);
    let lo = means[((boot as f64) * 0.025) as usize];
    let hi = means[(((boot as f64) * 0.975) as usize).min(boot - 1)];
    (Paired { mean, lo, hi, p: sign_test(wins, losses, lf), wins, losses }, means)
}

// ============================================================================
// Main
// ============================================================================

fn main() {
    let args = parse_args();
    let kina = [(args.vertical == "kina", "--vertical kina"), (!args.law_impact.is_empty(), "--law-impact"), (args.pre == "kina", "--pre kina")];
    if let Some((_, why)) = kina.iter().find(|x| x.0) {
        load_private(&args.private, why);
    }
    if !args.law_impact.is_empty() {
        match args.law_event.as_str() {
            "act" => law_impact_main(&args),
            "tr" => law_event_tr(&args),
            other => panic!("--law-event {other}: expected act or tr"),
        }
        return;
    }
    if !args.gate_trim.is_empty() {
        std::process::exit(gate_trim_main(&args));
    }
    if !args.gate_check.is_empty() {
        std::process::exit(gate_check_main(&args));
    }
    if !args.r3_compare.is_empty() {
        r3_compare_main(&args);
        return;
    }
    let t_start = Instant::now();
    eprintln!("[rank_eval] {}: building {}", args.label, args.repo);
    let t_build = Instant::now();
    let res = generate_one(&args.repo).expect("build");
    let build_s = t_build.elapsed().as_secs_f64();
    let merged = &res.merged;
    let all_ids = merged.node_ids();
    let id_to_idx: HashMap<NodeId, usize> = all_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let edges: Vec<&Edge> = merged.all_edges().collect();
    let g = index_graph(merged, &all_ids, &id_to_idx, &edges);
    let nf = g.paths.len();
    eprintln!(
        "[rank_eval] {}: built in {build_s:.1}s nodes={} edges={} candidate_files={nf}",
        args.label,
        g.ids.len(),
        edges.len()
    );
    let files_parity = file_space_parity(merged, &g, args.engine_parity == "on");
    let pidx: HashMap<&str, u32> = g.paths.iter().enumerate().map(|(i, p)| (p.as_str(), i as u32)).collect();
    let mut ext_hist: BTreeMap<String, usize> = BTreeMap::new();
    for p in &g.paths {
        *ext_hist.entry(ext_of(p)).or_default() += 1;
    }

    // ---- inter-file pairs joined by an edge ----
    let mut pair_index: HashMap<(u32, u32), u32> = HashMap::new();
    let mut pair_list: Vec<(u32, u32)> = Vec::new();
    let mut edge_pair: Vec<u32> = Vec::with_capacity(edges.len());
    let mut cat_pairs: BTreeMap<u32, HashSet<u32>> = BTreeMap::new();
    for e in &edges {
        let fa = g.node_by_id.get(&e.from).map_or(NONE, |&u| g.node_file[u as usize]);
        let fb = g.node_by_id.get(&e.to).map_or(NONE, |&u| g.node_file[u as usize]);
        if fa == NONE || fb == NONE || fa == fb {
            edge_pair.push(NONE);
            continue;
        }
        let key = (fa.min(fb), fa.max(fb));
        let next = pair_list.len() as u32;
        let p = *pair_index.entry(key).or_insert_with(|| {
            pair_list.push(key);
            next
        });
        edge_pair.push(p);
        cat_pairs.entry(e.category.0).or_default().insert(p);
    }

    // ---- PPR indexes ----
    let n_all = all_ids.len();
    let undirected = |preset: Option<&str>, dir: Direction| {
        let mut c = ref_config(preset);
        c.direction = dir;
        c.top_k = usize::MAX;
        c
    };
    let base_cfg = undirected(None, Direction::Undirected);
    let ppr: Vec<(&'static str, Ppr)> = vec![
        ("P1", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &base_cfg)),
        ("P1f", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(None, Direction::Forward))),
        ("P1b", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(None, Direction::Backward))),
        ("P1repair", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(Some("repair"), Direction::Undirected))),
        ("P1review", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(Some("review"), Direction::Undirected))),
        ("P1onboard", Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &undirected(Some("onboard"), Direction::Undirected))),
    ];
    let upr = Ppr::build(n_all, &id_to_idx, &edges, &edge_pair, &base_cfg);
    let adj_carry = Adjacency::carry(merged, &CODE_TABLES);
    let mut carry_imp: Vec<EdgeCategoryId> = CODE_TABLES.carry_edges.to_vec();
    carry_imp.push(ec::IMPORTS);
    let adj_imports = Adjacency::build(merged, &CategorySet::of(&carry_imp));
    let push_w: HashMap<EdgeCategoryId, f64> = base_cfg.edge_weights.clone();
    let push_cats: Vec<EdgeCategoryId> =
        ec::ALL.iter().map(|(c, _)| *c).filter(|c| push_w.get(c).copied().unwrap_or(1.0) > 0.0).collect();
    let adj_push = Adjacency::build(merged, &CategorySet::of(&push_cats));

    // ---- clusters ----
    let t_leiden = Instant::now();
    let view = WeightedGraph::from_source(merged, CODE_TABLES.community_weights);
    let mut opts = CommunityOptions::default();
    opts.seed = 42;
    let part = comm::communities(&view, &opts);
    let leiden_s = t_leiden.elapsed().as_secs_f64();
    let mut votes: Vec<BTreeMap<u32, usize>> = vec![BTreeMap::new(); nf];
    for ix in 0..view.len() as u32 {
        if view.strength(ix) == 0 {
            continue;
        }
        if let Some(&u) = g.node_by_id.get(&view.id(ix)) {
            let f = g.node_file[u as usize];
            if f != NONE {
                *votes[f as usize].entry(part.membership[ix as usize]).or_default() += 1;
            }
        }
    }
    let mut leiden_cl: Vec<u32> = Vec::with_capacity(nf);
    let mut next_single = part.communities as u32;
    for v in &votes {
        let best = v.iter().max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0))).map(|(c, _)| *c);
        leiden_cl.push(best.unwrap_or_else(|| {
            next_single += 1;
            next_single - 1
        }));
    }
    let keying = default_keying(merged);
    let repo = merged.graphs.first().map_or(0, |g| g.repo.0);
    let label_ids = |labels: Vec<String>| -> (Vec<u32>, usize) {
        let mut ix: BTreeMap<String, u32> = BTreeMap::new();
        let v = labels
            .into_iter()
            .map(|l| {
                let n = ix.len() as u32;
                *ix.entry(l).or_insert(n)
            })
            .collect();
        (v, ix.len())
    };
    let (dir_cl, dir_m) = label_ids(g.paths.iter().map(|p| dir_of(p).to_string()).collect());
    let (top_cl, top_m) = label_ids(g.paths.iter().map(|p| top2(p)).collect());
    let (svc_cl, svc_m) =
        label_ids(g.paths.iter().map(|p| service_of(p, repo, &keying, &res.repo_labels)).collect());
    let leiden_m = next_single as usize;
    let n_leiden_used = leiden_cl.iter().collect::<BTreeSet<_>>().len();
    let clusters = vec![
        ("dir", dir_cl, dir_m),
        ("top2", top_cl, top_m),
        ("leiden", leiden_cl, leiden_m),
        ("svc", svc_cl, svc_m),
    ];
    let path_hash: Vec<f64> = g.paths.iter().map(|p| (h64(p) >> 11) as f64).collect();
    let mut by_deg: Vec<usize> = (0..nf).collect();
    by_deg.sort_by(|&a, &b| g.file_deg[b].total_cmp(&g.file_deg[a]).then_with(|| a.cmp(&b)));
    let mut hubs = vec![false; nf];
    for &f in by_deg.iter().take(5) {
        hubs[f] = true;
    }
    let hub_files: Vec<String> = by_deg.iter().take(5).map(|&f| format!("{} ({})", g.paths[f], g.file_deg[f])).collect();

    // ---- history replay ----
    let (commits, raw_commits) = git_log(&args.git, &args.rev, &pidx);
    // round 3 (C): pre-merge histories prepended by author time (their commits only warm selection up)
    let (commits, r3anc) = if args.r3_anc.is_empty() {
        (commits, None)
    } else {
        let (c, a) = r3_ancestors(&args, commits, &g.paths, &pidx);
        (c, Some(a))
    };
    let r1_on = args.round1 == "on";
    assert!(!r1_on || args.vertical.is_empty(), "--round1 and --vertical are separate runs");
    let train_ok = |c: &Commit| !c.files.is_empty() && c.files.len() <= MAX_FILES;
    let test_ok = |c: &Commit| c.files.len() >= 2 && c.files.len() <= MAX_FILES;
    let eligible: Vec<usize> = (0..commits.len()).filter(|&k| test_ok(&commits[k])).collect();
    let mut warm_k: HashSet<usize> = HashSet::new();
    let test_idx: Vec<usize> = match &r3anc {
        None => eligible[eligible.len().saturating_sub(args.max_commits)..].to_vec(),
        Some(a) => {
            let own_el: Vec<usize> = eligible.iter().copied().filter(|&k| a.own[k]).collect();
            let mut v: Vec<usize> = eligible.iter().copied().filter(|&k| !a.own[k]).collect();
            warm_k.extend(v.iter().copied());
            v.extend(own_el[own_el.len().saturating_sub(args.max_commits)..].iter().copied());
            v.sort_unstable();
            v
        }
    };
    let mut test_ord: HashMap<usize, usize> = HashMap::new();
    for (o, &k) in test_idx.iter().enumerate() {
        test_ord.insert(k, o);
    }
    let mut hist: Option<R1Hist> = r1_on.then(|| r1_hist(&args, &commits, &test_idx.iter().copied().collect(), r3anc.as_ref().map(|a| &a.at)));
    assert!(args.round2.is_empty() || r1_on, "--round2 needs --round1 on");
    let r3_on = args.round3 == "on";
    assert!(!r3_on || args.round2 == "on", "--round3 needs --round1 on --round2 on");
    let mut r3stats: BTreeMap<String, f64> = BTreeMap::new();
    let r3keys: Vec<BTreeSet<String>> = if r3_on { r3_keys(&args, &commits, &g.paths, &mut r3stats) } else { Vec::new() };
    let mut r3idh = R3Idh::default();
    let mut tqs: Vec<TQuery> = Vec::new();
    let mut r1 = R1State::default();
    let mut r1s: Vec<R1Commit> = Vec::new();
    let mut bm_s = Bm25::default();
    let mut cnt = vec![0u32; nf];
    let mut co: Vec<HashMap<u32, u32>> = vec![HashMap::new(); nf];
    let mut bm = Bm25::default();
    let mut queries: Vec<Query> = Vec::new();
    let mut tcs: Vec<TestCommit> = Vec::new();
    // EA.9: per test commit, the leak-free co-change prefix (aligned with `tcs`).
    let parity_on = args.engine_parity == "on";
    let want_prefix = parity_on || args.ans == "on";
    let mut prefix: Vec<Prefix> = Vec::new();
    let mut t_task = 0.0f64;
    let mut n_train = 0usize;
    for (k, c) in commits.iter().enumerate() {
        if let Some(&o) = test_ord.get(&k) {
            let t0i = Instant::now();
            let qtok = tokens(&c.msg);
            let sc = bm.scores(&qtok);
            let t1 = bm.file_scores(&sc, |_| true);
            let t0 = if bm.dl.is_empty() {
                Vec::new()
            } else {
                let pick = (h64(&format!("{}:t0", c.sha)) % bm.dl.len() as u64) as usize;
                let s0 = bm.scores(&bm.toks[pick].clone());
                bm.file_scores(&s0, |_| true)
            };
            let mut s_pairs = Vec::new();
            for (p, &(a, b)) in pair_list.iter().enumerate() {
                if let Some(&both) = co[a as usize].get(&b) {
                    let either = cnt[a as usize] + cnt[b as usize] - both;
                    s_pairs.push((p as u32, both as f32 / either as f32));
                }
            }
            let mut seeds: Vec<u32> = c.files.clone();
            seeds.sort_by_key(|&f| h64(&format!("{}:{}", c.sha, g.paths[f as usize])));
            seeds.truncate(args.max_seeds);
            for &sd in &seeds {
                let targets: Vec<u32> = c.files.iter().copied().filter(|&f| f != sd).collect();
                let denom = f64::from(cnt[sd as usize]) + 1.0;
                let mut c0: Vec<(u32, f64)> = co[sd as usize].iter().map(|(&f, &n)| (f, f64::from(n) / denom)).collect();
                c0.sort_unstable_by_key(|x| x.0);
                let t2 = bm.file_scores(&sc, |d| bm.files[d].binary_search(&sd).is_ok());
                queries.push(Query { tc: o, seed: sd, targets, c0, t2 });
            }
            tcs.push(TestCommit { t1, t0, s_pairs });
            if want_prefix {
                prefix.push(Prefix::at(&cnt, &co, &c.files));
            }
            if let Some(h) = &hist {
                let mut rc = r1.test(h, k, &c.sha);
                for q in queries.iter().filter(|q| q.tc == o) {
                    let (cr, cm) = r1.seed_sigs(q.seed, &rc.m_keys);
                    rc.c0r.push((q.seed, cr));
                    rc.c0m.push((q.seed, cm));
                }
                let stoks = tokens(subject(&c.msg));
                rc.t1s = bm.file_scores(&bm.scores(&stoks), |_| true);
                rc.t1ss = bm_s.file_scores(&bm_s.scores(&stoks), |_| true);
                rc.t1s_nd = bm.file_scores(&bm.scores(&no_digits(&stoks)), |_| true);
                let ptoks = h.pre[k].as_deref().map(tokens).unwrap_or_default();
                rc.pre_cov = !ptoks.is_empty();
                if rc.pre_cov {
                    rc.t1pre = bm.file_scores(&bm.scores(&ptoks), |_| true);
                    rc.t1pre_nd = bm.file_scores(&bm.scores(&no_digits(&ptoks)), |_| true);
                }
                let sp: Vec<String> = stoks.iter().chain(ptoks.iter()).cloned().collect();
                rc.t1sp = bm.file_scores(&bm.scores(&sp), |_| true);
                if r3_on {
                    let (idh, idh_c, idh_u, n_hist) = r3idh.test(&r3keys[k], &c.sha);
                    let conf = r3_conf(&bm, &stoks, &rc.t1s);
                    rc.r3 = Some(R3Commit { n_keys: r3keys[k].len(), n_hist, idh, idh_c, idh_u, conf, warm: warm_k.contains(&k) });
                    r3_tqueries(o, c, &g.paths, &co, &cnt, &rc.t1s, &mut tqs);
                }
                r1s.push(rc);
            }
            t_task += t0i.elapsed().as_secs_f64();
        }
        if train_ok(c) {
            n_train += 1;
            for &a in &c.files {
                cnt[a as usize] += 1;
                for &b in &c.files {
                    if a != b {
                        *co[a as usize].entry(b).or_default() += 1;
                    }
                }
            }
            bm.add(tokens(&c.msg), c.files.clone());
            if let Some(h) = &hist {
                r1.train(h, k, c);
                bm_s.add(tokens(subject(&c.msg)), c.files.clone());
            }
            if r3_on {
                r3idh.train(&r3keys[k], &c.files);
            }
        }
        if let Some(h) = &hist {
            r1.step(h, k);
        }
    }
    if args.round2 == "on" {
        let (r2, st) = r2_build(&args, &commits, &test_idx, &g.paths, &pidx);
        for (o, x) in r2.into_iter().enumerate() {
            r1s[o].r2 = Some(x);
        }
        if let Some(h) = hist.as_mut() {
            h.stats.extend(st);
        }
    }
    if args.engine == "on" {
        for (o, mut e) in eng_build(&args, merged, &g, &commits, &test_idx).into_iter().enumerate() {
            let raw: Vec<(u32, f64)> = e.t1s_raw.iter().enumerate().map(|(f, &x)| (f as u32, x)).collect();
            e.mismatch = eng_top20(&raw) != eng_top20(&r1s[o].t1s);
            r1s[o].eng = Some(e);
        }
    }
    eprintln!(
        "[rank_eval] {}: commits={} train={} test_eligible={} test={} queries={}",
        args.label,
        commits.len(),
        n_train,
        eligible.len(),
        test_idx.len(),
        queries.len()
    );

    // ---- vertical layers (second run), read at each test commit's parent ----
    let vmode = VMode::parse(&args.vertical);
    let layers: Vec<VLayer> = match vmode {
        Some(m) => {
            let t_layers = Instant::now();
            let tests: Vec<(String, String)> =
                test_idx.iter().map(|&k| (commits[k].sha.clone(), commits[k].msg.clone())).collect();
            let l = build_layers(m, &args, &g.paths, &pidx, &tests);
            eprintln!("[rank_eval] {}: vertical layers ({m:?}) for {} test commits in {:.1}s", args.label, l.len(), t_layers.elapsed().as_secs_f64());
            if !args.keys.is_empty()
                && let Some(last) = l.last()
            {
                write_audit(&args.keys_out, &format!("vertical_{}", args.label), &last.kaudit);
            }
            // GLIA_RANK_VDUMP=<file>: the newest test commit's layer, file -> nodes per signal (inspection only).
            if let (Ok(path), Some(last)) = (std::env::var("GLIA_RANK_VDUMP"), l.last()) {
                let mut d = serde_json::Map::new();
                for (n, sig) in last.sigs.iter().chain(last.ctrls.iter()) {
                    let mut per = serde_json::Map::new();
                    for (f, set) in sig.sets.iter().enumerate() {
                        if !set.is_empty() {
                            per.insert(g.paths[f].clone(), json!(set.iter().map(|&x| sig.names[x as usize].clone()).collect::<Vec<_>>()));
                        }
                    }
                    d.insert(n.clone(), serde_json::Value::Object(per));
                }
                d.insert("_sha".into(), json!(tests.last().map(|t| t.0.clone())));
                d.insert("_vocab".into(), json!(last.vocab));
                std::fs::write(path, serde_json::to_string_pretty(&serde_json::Value::Object(d)).unwrap()).expect("vdump");
            }
            l
        }
        None => Vec::new(),
    };

    let ctx = Ctx {
        merged,
        g: &g,
        ppr,
        upr,
        adj_carry,
        adj_imports,
        adj_push,
        push_w,
        clusters,
        path_hash,
        hubs,
        n_pairs: pair_list.len(),
        direct: pair_list.iter().copied().collect(),
        push_eps: args.push_eps,
        tune: if args.tune == "on" { r1_tune_indexes(n_all, &id_to_idx, &edges, &edge_pair) } else { Vec::new() },
        pick: None,
        ans: args.ans == "on",
        path_ix: g.paths.iter().enumerate().map(|(i, p)| (p.clone(), i as u32)).collect(),
        live: if args.ans == "on" { entrypoint_reachable(merged) } else { HashSet::new() },
        ans_ns: AtomicU64::new(0),
        prefix,
    };
    let mut ctx = ctx;
    if !args.pick.is_empty() {
        ctx.pick = Some(ctx.tune.iter().position(|(n, _)| *n == args.pick).unwrap_or_else(|| panic!("--pick {}: not in the grid", args.pick)));
    }
    let ctx = ctx;
    if parity_on {
        let named = ctx.pick.map(|i| ctx.tune[i].0.as_str());
        assert!(named == Some("base,d85"), "--engine-parity on needs --pick base,d85 (PT* is base,d85), got {named:?}");
    }

    // ---- timing pass (sequential) + replica check against production ----
    let timing = timing_pass(&ctx, &queries, args.timing, &base_cfg);

    // ---- the evaluation (parallel over queries) ----
    let t_eval = Instant::now();
    let wf = |c: EdgeCategoryId| ctx.push_w.get(&c).copied().unwrap_or(1.0);
    let mut results: Vec<Out> = queries
        .par_iter()
        .map_init(|| PushPpr::new(&ctx.adj_push, &wf), |push, q| run_query(&ctx, q, &tcs[q.tc], push, layers.get(q.tc), r1s.get(q.tc)))
        .collect();
    let eval_s = t_eval.elapsed().as_secs_f64();
    // ---- round 3: the gates (B), the transfer shapes (T) ----
    let tc_of_main: Vec<usize> = queries.iter().map(|q| q.tc).collect();
    let mut tc_sha: Vec<String> = test_idx.iter().map(|&k| commits[k].sha.clone()).collect();
    let mut tc_files: Vec<Vec<u32>> = test_idx.iter().map(|&k| commits[k].files.clone()).collect();
    let mut r3_info = serde_json::Map::new();
    let mut transfer: Vec<(Vec<Out>, Vec<usize>)> = Vec::new();
    // per shape, each transfer query's id (`--r3-perq on`'s `perq_shapes`), aligned with `transfer`
    let mut transfer_ids: Vec<Vec<String>> = Vec::new();
    let r3_conf_tc: Vec<[f64; 4]> = r1s.iter().map(|r| r.r3.as_ref().map_or([0.0; 4], |x| x.conf)).collect();
    let r3_cands_b: Vec<String> = R3_SELT2.iter().chain(["P1", "PT*"].iter()).map(|s| s.to_string()).collect();
    if r3_on {
        let t3 = Instant::now();
        r3_info.insert("gates".into(), r3_gates(&mut results, &tc_of_main, &r3_conf_tc, &tc_sha, &R3_GATE_PAIRS, &r3_cands_b));
        let subjects: Vec<String> = test_idx.iter().map(|&k| subject(&commits[k].msg).to_string()).collect();
        transfer = r3_transfer(&ctx, &tqs, &r1s, &subjects);
        transfer_ids = vec![Vec::new(); R3_SHAPES.len()];
        for q in &tqs {
            let sha = &commits[test_idx[q.tc]].sha;
            let id = if q.shape == 3 {
                format!("{sha}:find")
            } else {
                let mut ps: Vec<&str> = q.seeds.iter().map(|&f| g.paths[f as usize].as_str()).collect();
                ps.sort_unstable();
                format!("{sha}:{}:{}", R3_SHAPES[q.shape], ps.join("+"))
            };
            transfer_ids[q.shape].push(id);
        }
        let mut tinfo = serde_json::Map::new();
        for (si, (outs, tc_of)) in transfer.iter_mut().enumerate() {
            if outs.is_empty() {
                continue;
            }
            if si == 3 {
                let f2: Vec<String> = R3_SELF2.iter().map(|s| s.to_string()).collect();
                let mut f3 = f2.clone();
                f3.extend(R3_SELF3.iter().map(|s| s.to_string()));
                let mut f2t = f2.clone();
                f2t.extend(["T1s", "PXT"].iter().map(|s| s.to_string()));
                tinfo.insert(R3_SHAPES[si].into(), r3_select(outs, tc_of, r1s.len(), &[("SELf2".to_string(), f2), ("SELf3".to_string(), f3), ("SELf2t".to_string(), f2t)]));
            } else {
                let gi = r3_gates(outs, tc_of, &r3_conf_tc, &tc_sha, &R3_GATE_PAIRS, &r3_cands_b);
                let mut sets: Vec<(String, Vec<String>)> = R1_SEL.iter().map(|(n, c)| (n.to_string(), c.iter().map(|x| x.to_string()).collect())).collect();
                sets.extend(r3_sets(&args));
                let si_ = r3_select(outs, tc_of, r1s.len(), &sets);
                tinfo.insert(R3_SHAPES[si].into(), json!({"gates": gi, "select": si_}));
            }
        }
        r3_info.insert("transfer".into(), serde_json::Value::Object(tinfo));
        eprintln!("[round3] gates and {} transfer queries in {:.1}s", tqs.len(), t3.elapsed().as_secs_f64());
    }
    // ---- engine parity (EA.9), on the queries as replayed (before any warm-up commit is dropped) ----
    let parity = parity_on.then(|| engine_parity(&ctx, &queries, &tqs, &tc_sha, &args.label));
    if ctx.ans {
        let n_transfer: usize = transfer.iter().take(3).map(|(o, _)| o.len()).sum();
        let n_find = transfer.get(3).map_or(0, |(o, _)| o.len());
        eprintln!(
            "[rank_eval] ans rankers=3 single={} transfer={n_transfer} find={n_find} ms={}",
            results.len(),
            ctx.ans_ns.load(AtomicOrdering::Relaxed) / 1_000_000
        );
    }
    let r1_sel = if r1_on { round1_select(&mut results, &queries, tcs.len()) } else { serde_json::Value::Null };
    if r3_on {
        r3_info.insert("select".into(), r3_select(&mut results, &tc_of_main, tcs.len(), &r3_sets(&args)));
        // facts (test commits that are scored, i.e. not ancestor warm-ups)
        let live: Vec<usize> = (0..r1s.len()).filter(|&o| !r1s[o].r3.as_ref().is_some_and(|x| x.warm)).collect();
        let mut f = r3stats.clone();
        let cnt = |p: &dyn Fn(usize) -> bool| live.iter().filter(|&&o| p(o)).count() as f64;
        let x3 = |o: usize| r1s[o].r3.clone().unwrap_or_default();
        let hit = |v: &[(u32, f64)], o: usize, top: usize| -> bool {
            let mut s: Vec<(u32, f64)> = v.iter().copied().filter(|x| x.1 > 0.0).collect();
            s.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            s.iter().take(top).any(|x| tc_files[o].contains(&x.0))
        };
        let idr_id = |o: usize| r1s[o].r2.as_ref().is_some_and(|x| x.has_id);
        let idr_reach = |o: usize| r1s[o].r2.as_ref().is_some_and(|x| !x.idr.is_empty());
        let idh_reach = |o: usize| !x3(o).idh.is_empty();
        f.insert("r3_test_commits".into(), live.len() as f64);
        f.insert("r3_test_commits_warmup_only".into(), (r1s.len() - live.len()) as f64);
        f.insert("r3_test_commits_with_keys".into(), cnt(&|o| x3(o).n_keys > 0));
        f.insert("r3_test_commits_with_key_history".into(), cnt(&|o| x3(o).n_hist > 0));
        f.insert("r3_test_commits_idh_reach".into(), cnt(&|o| idh_reach(o)));
        f.insert("r3_idh_any_changed_file".into(), cnt(&|o| hit(&x3(o).idh, o, usize::MAX)));
        f.insert("r3_idh_top10_changed_file".into(), cnt(&|o| hit(&x3(o).idh, o, 10)));
        f.insert("r3_idh~_any_changed_file".into(), cnt(&|o| hit(&x3(o).idh_c, o, usize::MAX)));
        f.insert("r3_idh~_top10_changed_file".into(), cnt(&|o| hit(&x3(o).idh_c, o, 10)));
        f.insert("r3_idh~u_top10_changed_file".into(), cnt(&|o| hit(&x3(o).idh_u, o, 10)));
        f.insert("r3_idr_id".into(), cnt(&|o| idr_id(o)));
        f.insert("r3_idr_id_idr_reach".into(), cnt(&|o| idr_id(o) && idr_reach(o)));
        f.insert("r3_idr_id_idh_reach".into(), cnt(&|o| idr_id(o) && idh_reach(o)));
        f.insert("r3_idr_id_either_reach".into(), cnt(&|o| idr_id(o) && (idr_reach(o) || idh_reach(o))));
        f.insert("r3_idr_id_idh_only".into(), cnt(&|o| idr_id(o) && !idr_reach(o) && idh_reach(o)));
        f.insert("r3_idr_id_idh_only_top10_changed_file".into(), cnt(&|o| idr_id(o) && !idr_reach(o) && hit(&x3(o).idh, o, 10)));
        for (i, nm) in R3_CONF.iter().enumerate() {
            f.insert(format!("r3_conf_mean_{nm}"), live.iter().map(|&o| x3(o).conf[i]).sum::<f64>() / live.len().max(1) as f64);
        }
        f.insert("r3_bank_mismatches".into(), results.iter().map(|r| r.r3_bad).sum());
        for (si, (outs, _)) in transfer.iter().enumerate() {
            f.insert(format!("r3_transfer_queries_{}", R3_SHAPES[si]), outs.len() as f64);
        }
        if let Some(a) = &r3anc {
            f.extend(a.facts.clone());
            r3_info.insert("ancestor_verify".into(), json!(a.verify));
        }
        r3_info.insert("facts".into(), json!(f));
        // GLIA_R3_DUMP=<file>: per scored test commit, the subject's IDH keys and what IDH / IDR reach (inspection only)
        if let Ok(path) = std::env::var("GLIA_R3_DUMP") {
            let mut s = String::new();
            for &o in &live {
                let k = test_idx[o];
                let x = x3(o);
                let mut top = x.idh.clone();
                top.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                let tops: Vec<String> = top.iter().take(8).map(|(fi, w)| format!("{}{}:{w:.2}", if tc_files[o].contains(fi) { "*" } else { "" }, g.paths[*fi as usize])).collect();
                let keys: Vec<&String> = r3keys[k].iter().collect();
                let idr_n = r1s[o].r2.as_ref().map_or(0, |y| y.idr.len());
                s.push_str(&format!("{}\t{}\t{:?}\thist={}\tidr_files={}\t{}\n", &commits[k].sha[..8], subject(&commits[k].msg), keys, x.n_hist, idr_n, tops.join(" ")));
            }
            std::fs::write(path, s).expect("r3 dump");
        }
    }
    // ---- round 3 (C): the ancestor warm-up commits are dropped before anything is scored ----
    if r3anc.is_some() {
        let keep: Vec<bool> = r1s.iter().map(|r| !r.r3.as_ref().is_some_and(|x| x.warm)).collect();
        let mut newo = vec![usize::MAX; keep.len()];
        let mut nk = 0;
        for (o, &k) in keep.iter().enumerate() {
            if k {
                newo[o] = nk;
                nk += 1;
            }
        }
        let kept: Vec<(Out, Query)> = results.into_iter().zip(queries).filter(|(_, q)| keep[q.tc]).map(|(r, q)| (r, Query { tc: newo[q.tc], ..q })).collect();
        (results, queries) = kept.into_iter().unzip();
        tcs = tcs.into_iter().enumerate().filter(|(o, _)| keep[*o]).map(|x| x.1).collect();
        r1s = r1s.into_iter().enumerate().filter(|(o, _)| keep[*o]).map(|x| x.1).collect();
        tc_sha = tc_sha.into_iter().enumerate().filter(|(o, _)| keep[*o]).map(|x| x.1).collect();
        tc_files = tc_files.into_iter().enumerate().filter(|(o, _)| keep[*o]).map(|x| x.1).collect();
        for (si, (outs, tc_of)) in transfer.iter_mut().enumerate() {
            let (mut no, mut nt) = (Vec::new(), Vec::new());
            let mut ni = Vec::new();
            for (j, (o, &t)) in outs.drain(..).zip(tc_of.iter()).enumerate() {
                if keep[t] {
                    no.push(o);
                    nt.push(newo[t]);
                    if let Some(id) = transfer_ids.get(si).and_then(|v| v.get(j)) {
                        ni.push(id.clone());
                    }
                }
            }
            *outs = no;
            *tc_of = nt;
            if let Some(v) = transfer_ids.get_mut(si) {
                *v = ni;
            }
        }
    }
    let _ = &tc_files;
    // ---- `--engine on` (EB.9): the ENG-* rows go last, after every selection and gate ----
    eng_append(&mut results);
    for (outs, _) in transfer.iter_mut() {
        eng_append(outs);
    }
    let names = results[0].names.clone();
    for r in &results {
        assert_eq!(r.names, names, "ranker order differs between queries");
    }
    let nq = results.len();
    let nr = names.len();
    let mean = |ri: usize, mi: usize| results.iter().map(|r| r.m[ri][mi]).sum::<f64>() / nq as f64;
    let metrics: Vec<[f64; 5]> = (0..nr).map(|ri| [0, 1, 2, 3, 4].map(|mi| mean(ri, mi))).collect();
    let col = |ri: usize, mi: usize| -> Vec<f64> { results.iter().map(|r| r.m[ri][mi]).collect() };
    let commit_of: Vec<usize> = queries.iter().map(|q| q.tc).collect();
    let lf = ln_fact_table(nq + 1);
    let pos = |n: &str| names.iter().position(|x| x == n).unwrap_or_else(|| panic!("no ranker {n}"));
    let baselines = ["B0", "P1", "C0"];
    // (ranker, baseline, metric idx) -> Paired
    let mut paired_all: BTreeMap<(String, String, usize), Paired> = BTreeMap::new();
    for (ri, name) in names.iter().enumerate() {
        for bl in baselines {
            for mi in [1usize, 3] {
                let p = paired(&col(ri, mi), &col(pos(bl), mi), &commit_of, tcs.len(), args.boot, &lf);
                paired_all.insert((name.clone(), bl.to_string(), mi), p);
            }
        }
    }

    // ---- inefficiency map (full history to the snapshot) ----
    let mut cochanged: HashSet<(u32, u32)> = HashSet::new();
    for (a, m) in co.iter().enumerate() {
        for (&b, &n) in m {
            if (a as u32) < b && n > 0 {
                cochanged.insert((a as u32, b));
            }
        }
    }
    let all_pairs = (nf as f64) * (nf as f64 - 1.0) / 2.0;
    let base_rate = cochanged.len() as f64 / all_pairs;
    let mut ineff: Vec<serde_json::Value> = Vec::new();
    for (cat, ps) in &cat_pairs {
        let cid = EdgeCategoryId(*cat);
        if cid == ec::CONTAINS || cid == ec::DEFINES {
            continue;
        }
        let hit = ps.iter().filter(|&&p| cochanged.contains(&pair_list[p as usize])).count();
        let rate = hit as f64 / ps.len() as f64;
        ineff.push(json!({
            "category": ec::name(cid),
            "pairs": ps.len(),
            "cochanged_pairs": hit,
            "p_cochange": rate,
            "lift": if base_rate > 0.0 { rate / base_rate } else { 0.0 },
            "coverage": if cochanged.is_empty() { 0.0 } else { hit as f64 / cochanged.len() as f64 },
            "activation_weight": base_cfg.edge_weights.get(&cid).copied().unwrap_or(1.0),
            "carry": CODE_TABLES.carries(cid),
        }));
    }
    ineff.sort_by(|a, b| b["lift"].as_f64().unwrap().total_cmp(&a["lift"].as_f64().unwrap()));
    let mut hub: BTreeMap<String, f64> = BTreeMap::new();
    for r in &results {
        for (n, v) in &r.hub_share {
            *hub.entry(n.clone()).or_default() += v / nq as f64;
        }
    }

    // ---- stats ----
    let mean_targets = queries.iter().map(|q| q.targets.len()).sum::<usize>() as f64 / nq as f64;
    let mean_seed_nodes = queries.iter().map(|q| g.file_nodes[q.seed as usize].len()).sum::<usize>() as f64 / nq as f64;
    let chance_r10 = 10.0 / (nf as f64 - 1.0);
    let cold = queries.iter().filter(|q| q.c0.is_empty()).count();
    let t1_empty = tcs.iter().filter(|t| t.t1.is_empty()).count();
    let stats = json!({
        "label": args.label,
        "rev": args.rev,
        "nodes": g.ids.len(),
        "edges": edges.len(),
        "candidate_files": nf,
        "candidate_ext": ext_hist,
        "excluded_module_files_by_ext": g.excluded_modules,
        "generated_module_files": g.generated_modules,
        "inter_file_pairs_with_edge": pair_list.len(),
        "git_nonmerge_commits": raw_commits,
        "train_commits": n_train,
        "test_eligible_commits": eligible.len(),
        "test_commits": tcs.len(),
        "queries": nq,
        "mean_targets": mean_targets,
        "mean_seed_nodes": mean_seed_nodes,
        "chance_recall10": chance_r10,
        "queries_without_cochange_history": cold,
        "test_commits_without_past_task_hit": t1_empty,
        "leiden_communities_used": n_leiden_used,
        "services": svc_m,
        "service_keying": keying.name(),
        "hub_files": hub_files,
        "build_s": build_s,
        "leiden_s": leiden_s,
        "eval_s": eval_s,
        "task_replay_ms_per_test_commit": 1000.0 * t_task / tcs.len().max(1) as f64,
        "total_s": t_start.elapsed().as_secs_f64(),
        "cochanged_pairs": cochanged.len(),
        "random_pair_cochange_rate": base_rate,
        "engine_parity_files": files_parity,
    });
    let metrics_json: BTreeMap<String, serde_json::Value> = names
        .iter()
        .enumerate()
        .map(|(ri, n)| {
            let m = metrics[ri];
            (n.clone(), json!({"r5": m[0], "r10": m[1], "r20": m[2], "mrr": m[3], "ndcg20": m[4]}))
        })
        .collect();
    let paired_json: Vec<serde_json::Value> = paired_all
        .iter()
        .map(|((r, b, mi), p)| {
            json!({"ranker": r, "vs": b, "metric": if *mi == 1 { "r10" } else { "mrr" },
                "diff": p.mean, "lo": p.lo, "hi": p.hi, "p_sign": p.p, "wins": p.wins, "losses": p.losses})
        })
        .collect();
    let mut doc = json!({
        "stats": stats,
        "rankers": names,
        "metrics": metrics_json,
        "paired": paired_json,
        "inefficiency": ineff,
        "hub_share_top20_mass_on_top5_degree_files": hub,
        "timing_ms_per_query": timing,
    });
    if let Some(m) = vmode {
        let bi = BootIn { commit_of: &commit_of, boot: args.boot, lf: &lf };
        let (vj, vmd) = vertical_report(&args.label, m, &names, &results, &queries, &layers, &bi);
        doc["vertical"] = vj;
        std::fs::write(&args.vmd, vmd).expect("write vertical md");
    }
    if let Some(h) = &hist {
        let bi = BootIn { commit_of: &commit_of, boot: args.boot, lf: &lf };
        let (rj, mut rmd) = round1_report(&args.label, &names, &results, &queries, &r1s, &h.stats, &bi);
        doc["round1"] = rj;
        doc["round1"]["select"] = r1_sel;
        if args.engine == "on" {
            let (ej, emd) = eng_report(&args.label, &results, &r1s, &transfer, &bi);
            doc["engine_intent_parity"] = ej;
            rmd.push_str(&emd);
        }
        std::fs::write(&args.r1md, rmd).expect("write round1 md");
    }
    if r3_on {
        let bi = BootIn { commit_of: &commit_of, boot: args.boot, lf: &lf };
        let facts: BTreeMap<String, f64> = r3_info
            .get("facts")
            .and_then(|v| v.as_object())
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_f64().unwrap_or(f64::NAN))).collect())
            .unwrap_or_default();
        let (rj, rmd) = round3_report(&args.label, &names, &results, &bi, &transfer, &facts, serde_json::Value::Object(r3_info));
        doc["round3"] = rj;
        std::fs::write(&args.r3md, rmd).expect("write round3 md");
    }
    if args.r3_perq == "on" {
        let ids: Vec<String> = queries.iter().map(|q| format!("{}:{}", tc_sha[q.tc], g.paths[q.seed as usize])).collect();
        let mut r10 = serde_json::Map::new();
        let mut mrr = serde_json::Map::new();
        for (ri, n) in names.iter().enumerate() {
            r10.insert(n.clone(), json!(col(ri, 1)));
            mrr.insert(n.clone(), json!(col(ri, 3)));
        }
        doc["perq"] = json!({"ids": ids, "commit_of": commit_of, "r10": r10, "mrr": mrr});
        let mut shapes = serde_json::Map::new();
        for (si, (outs, tc_of)) in transfer.iter().enumerate() {
            if outs.is_empty() {
                continue;
            }
            let mut r10 = serde_json::Map::new();
            let mut mrr = serde_json::Map::new();
            for (ri, n) in outs[0].names.iter().enumerate() {
                r10.insert(n.clone(), json!(outs.iter().map(|o| o.m[ri][1]).collect::<Vec<f64>>()));
                mrr.insert(n.clone(), json!(outs.iter().map(|o| o.m[ri][3]).collect::<Vec<f64>>()));
            }
            shapes.insert(R3_SHAPES[si].into(), json!({"ids": transfer_ids[si], "commit_of": tc_of, "r10": r10, "mrr": mrr}));
        }
        doc["perq_shapes"] = serde_json::Value::Object(shapes);
    }
    if let Some(p) = &parity {
        doc["engine_parity"] = p.json.clone();
    }
    std::fs::write(&args.out, serde_json::to_string_pretty(&doc).unwrap()).expect("write json");
    let mut md = render_md(&args.label, &doc, &names, &metrics, &paired_all);
    if let Some(p) = &parity {
        md.push_str(&p.md);
    }
    std::fs::write(&args.md, md).expect("write md");
    eprintln!("[rank_eval] {}: done in {:.1}s -> {} {}", args.label, t_start.elapsed().as_secs_f64(), args.out, args.md);
    if let Some(first) = parity.and_then(|p| p.first_single) {
        panic!("[rank-parity] single-seed key mismatch (the report is written): {first}");
    }
}

// ============================================================================
// Timing pass
// ============================================================================

fn timing_pass(ctx: &Ctx, queries: &[Query], n: usize, cfg: &ActivationConfig) -> serde_json::Value {
    let g = ctx.g;
    let wf = |c: EdgeCategoryId| ctx.push_w.get(&c).copied().unwrap_or(1.0);
    let mut push = PushPpr::new(&ctx.adj_push, &wf);
    let mut acc: BTreeMap<&'static str, f64> = BTreeMap::new();
    let mut max_diff = 0.0f64;
    let mut bit_identical = true;
    let mut taken = 0usize;
    let s0 = vec![0.0f32; ctx.n_pairs];
    for q in queries.iter().step_by((queries.len() / n.max(1)).max(1)).take(n) {
        taken += 1;
        let seed_nodes = &g.file_nodes[q.seed as usize];
        let seed_ids: Vec<NodeId> = seed_nodes.iter().map(|&u| g.ids[u as usize]).collect();
        let seed_ppr: Vec<usize> = seed_nodes.iter().map(|&u| g.node_ppr[u as usize]).collect();
        let pers = Ppr::uniform(&seed_ppr);

        let t = Instant::now();
        let _ = bfs_key(ctx, &ctx.adj_carry, &seed_ids);
        *acc.entry("B0 (carry BFS + file depth)").or_default() += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let v = ctx.ppr[0].1.run(&pers);
        let fs = file_pool(g, |u| v[g.node_ppr[u]], false);
        *acc.entry("P1 (power iteration over a prebuilt index)").or_default() += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let view = ActivationPlan::<MergedGraph>::new(cfg.clone()).run(ctx.merged, &seed_ids);
        let prod: HashMap<NodeId, f64> = view.scores.into_iter().collect();
        let fs_prod = file_pool(g, |u| prod.get(&g.ids[u]).copied().unwrap_or(0.0), false);
        *acc.entry("P1 production (ActivationPlan::run, as answers call it)").or_default() += t.elapsed().as_secs_f64();
        for (u, id) in g.ids.iter().enumerate() {
            let a = v[g.node_ppr[u]];
            let b = prod.get(id).copied().unwrap_or(0.0);
            let b = if b > 0.0 { b } else { 0.0 };
            let a = if a > 0.0 { a } else { 0.0 };
            if a.to_bits() != b.to_bits() {
                bit_identical = false;
            }
            max_diff = max_diff.max((a - b).abs());
        }
        let _ = (fs, fs_prod);

        let t = Instant::now();
        let _ = push_scores(ctx, &mut push, &seed_ids);
        *acc.entry("P3 (push PPR, per seed, averaged)").or_default() += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let (inc, ow) = ctx.upr.reweight(&s0, 5.0);
        let v = ctx.upr.power(&pers, &inc, &ow);
        let _ = file_pool(g, |u| v[g.node_ppr[u]], false);
        *acc.entry("U1 (reweight edges + power iteration)").or_default() += t.elapsed().as_secs_f64();

        let t = Instant::now();
        let k = pooled(&fs_or_zero(&v, g), &ctx.clusters[0].1, ctx.clusters[0].2);
        let _ = evaluate(&k, q.seed as usize, &q.targets);
        *acc.entry("K pooling given P1 + one metric evaluation").or_default() += t.elapsed().as_secs_f64();
    }
    let mut out = serde_json::Map::new();
    for (k, v) in acc {
        out.insert(k.into(), json!(1000.0 * v / taken.max(1) as f64));
    }
    out.insert("queries_timed".into(), json!(taken));
    out.insert("replica_vs_production_max_abs_diff".into(), json!(max_diff));
    out.insert("replica_vs_production_bit_identical".into(), json!(bit_identical));
    serde_json::Value::Object(out)
}

fn fs_or_zero(v: &[f64], g: &Graph) -> Vec<f64> {
    file_pool(g, |u| v[g.node_ppr[u]], false)
}

// ============================================================================
// Markdown
// ============================================================================

fn render_md(
    label: &str,
    doc: &serde_json::Value,
    names: &[String],
    metrics: &[[f64; 5]],
    paired_all: &BTreeMap<(String, String, usize), Paired>,
) -> String {
    let pos = |n: &str| names.iter().position(|x| x == n);
    let r10 = |n: &str| pos(n).map_or(f64::NEG_INFINITY, |i| metrics[i][1]);
    let structural = [
        "B0", "B0i", "B1", "B3", "P1", "P1sum", "P1f", "P1b", "P1repair", "P1review", "P1onboard", "P2", "P2i", "P3",
        "K1dir", "K1top2", "K1leiden", "K1svc", "K2dir", "K2top2", "K2leiden", "K2svc",
    ];
    let best_struct = structural.iter().copied().max_by(|a, b| r10(a).total_cmp(&r10(b))).unwrap();
    let best_k = structural.iter().copied().filter(|n| n.starts_with('K')).max_by(|a, b| r10(a).total_cmp(&r10(b))).unwrap();
    let mut rows: Vec<String> = [
        "B0", "B0i", "B1", "B2", "B3", "C0", "P1", "P1sum", "P1f", "P1b", "P1repair", "P1review", "P1onboard", "P2",
        "P2i", "P3", "K1dir", "K1top2", "K1leiden", "K1svc", "K2dir", "K2top2", "K2leiden", "K2svc", "H1a25",
        "H1a50", "H1a75", "H1r25", "H1r50", "H1r75", "H2a25", "H2a50", "H2a75", "H3", "H4",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    rows.push(format!("H5[{best_k}]"));
    rows.extend(["H10", "T0", "T1", "T2", "H6", "H7", "H8"].iter().map(|s| s.to_string()));
    for base in ["P1", "P1sum", "P2"] {
        for a in [25, 50, 75] {
            rows.push(format!("H9a{a}[{base}]"));
        }
    }
    let best_h9 = names
        .iter()
        .filter(|n| n.starts_with("H9a"))
        .max_by(|a, b| r10(a).total_cmp(&r10(b)))
        .cloned()
        .unwrap_or_default();
    if !rows.contains(&best_h9) {
        rows.push(best_h9.clone());
    }
    rows.extend(
        ["H18", "H11", "H12", "H13", "H14", "H15", "H16", "H17", "U1b1", "U1b5", "U2b1", "U2b5", "U2fb1", "U2fb5", "U3b5", "U3sb5"]
            .iter()
            .map(|s| s.to_string()),
    );
    let rows: Vec<String> = rows.into_iter().filter(|r| pos(r).is_some()).collect();
    let mut best = [f64::NEG_INFINITY; 5];
    for r in &rows {
        let m = metrics[pos(r).unwrap()];
        for i in 0..5 {
            best[i] = best[i].max(m[i]);
        }
    }
    let s = &doc["stats"];
    let mut md = String::new();
    md.push_str(&format!("### {label}\n\n"));
    md.push_str(&format!(
        "Graph: {} nodes, {} edges, {} candidate code files. History: {} non-merge commits, {} training-eligible (1..=30 code files), {} test-eligible (2..=30), {} test commits, {} queries (mean {:.2} targets, mean {:.1} seed nodes). Chance recall@10 = {:.4}. Queries with no co-change history for the seed: {}. Test commits whose message matched no past task: {}.\n\n",
        s["nodes"], s["edges"], s["candidate_files"], s["git_nonmerge_commits"], s["train_commits"],
        s["test_eligible_commits"], s["test_commits"], s["queries"], s["mean_targets"].as_f64().unwrap(),
        s["mean_seed_nodes"].as_f64().unwrap(), s["chance_recall10"].as_f64().unwrap(),
        s["queries_without_cochange_history"], s["test_commits_without_past_task_hit"]
    ));
    md.push_str(&format!(
        "Best structural ranker by recall@10: {best_struct}. Best K* (H5's base): {best_k}. Best H9 over every structural base (selected on this test set): {best_h9}.\n\n"
    ));
    md.push_str("| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |\n|---|---|---|---|---|---|\n");
    for r in &rows {
        let m = metrics[pos(r).unwrap()];
        let cells: Vec<String> = (0..5)
            .map(|i| if (m[i] - best[i]).abs() < 1e-12 { format!("**{:.3}**", m[i]) } else { format!("{:.3}", m[i]) })
            .collect();
        md.push_str(&format!("| {r} | {} |\n", cells.join(" | ")));
    }
    for (mi, mname) in [(1usize, "recall@10"), (3, "MRR")] {
        md.push_str(&format!(
            "\nPaired {mname} differences (ranker minus baseline; mean, 95% commit-cluster bootstrap CI, sign-test p over queries, wins/losses):\n\n| ranker | vs B0 | vs P1 | vs C0 |\n|---|---|---|---|\n"
        ));
        let key_rows = [
            "B0", "B0i", "B1", "B3", "C0", "P1", "P1sum", "P1f", "P1b", "P1onboard", "P2", "P3", "K1svc", "K2svc",
            "H1a25", "H2a50", "H3", "H4", "H10", "T0", "T1", "T2", "H6", "H7", "H8", "H11", "H12", "H13", "H14", "H15",
            "H16", "H17", "U1b1", "U1b5", "U2b5", "U2fb5", "U3b5", "U3sb5",
        ];
        let mut prow: Vec<String> = key_rows.iter().map(|s| s.to_string()).collect();
        prow.push(format!("H5[{best_k}]"));
        for extra in ["H9a25[P1]", "H9a50[P1]", "H9a25[P2]", "H9a50[P2]", "H9a50[P1sum]", "H18"] {
            prow.push(extra.to_string());
        }
        if !prow.contains(&best_h9) {
            prow.push(best_h9.clone());
        }
        for r in prow.iter().filter(|r| pos(r).is_some()) {
            let cell = |bl: &str| {
                paired_all.get(&(r.clone(), bl.to_string(), mi)).map_or("-".to_string(), |p| {
                    if p.wins + p.losses == 0 {
                        "=".to_string()
                    } else {
                        format!("{:+.3} [{:+.3}, {:+.3}] p={:.2e} {}/{}", p.mean, p.lo, p.hi, p.p, p.wins, p.losses)
                    }
                })
            };
            md.push_str(&format!("| {r} | {} | {} | {} |\n", cell("B0"), cell("P1"), cell("C0")));
        }
    }
    md.push_str("\nInefficiency map (file pairs joined by an edge of the category; lift = P(pair co-changed in >= 1 commit) / P(random pair co-changed)):\n\n| category | weight | carry | pairs | co-changed | P(co-change) | lift | coverage of co-changed pairs |\n|---|---|---|---|---|---|---|---|\n");
    for row in doc["inefficiency"].as_array().unwrap() {
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {:.3} | {:.1} | {:.3} |\n",
            row["category"].as_str().unwrap(),
            row["activation_weight"],
            row["carry"],
            row["pairs"],
            row["cochanged_pairs"],
            row["p_cochange"].as_f64().unwrap(),
            row["lift"].as_f64().unwrap(),
            row["coverage"].as_f64().unwrap()
        ));
    }
    md.push_str(&format!(
        "\nRandom-pair co-change rate: {:.4} ({} co-changed pairs). Hub flooding (mean share of the top-20 files' score mass on the 5 highest-degree files {}): {}\n",
        s["random_pair_cochange_rate"].as_f64().unwrap(),
        s["cochanged_pairs"],
        s["hub_files"],
        doc["hub_share_top20_mass_on_top5_degree_files"]
    ));
    md.push_str(&format!("\nTiming (ms per query, sequential): {}\n", doc["timing_ms_per_query"]));
    md
}

// ============================================================================
// Vertical layers (second run, 2026-10-18)
//
// The question: do files that change together hang under the same upper-layer
// node (a requirement, a spec, a law, a packet, a ruling) even when no code edge
// joins them? Every upper layer is read from the TEST COMMIT'S PARENT revision
// (code with `git grep <parent>`, docs from the same revision), so nothing the
// commit itself writes is visible. Kina's specs moved into its wiki (its own git
// repository) at c643306; after that the doc layer is the wiki commit current at
// the parent's committer time. The `w` variants read the wiki at its HEAD
// instead (a snapshot, later than every test commit: flagged).
// ============================================================================

#[derive(Clone, Copy, PartialEq, Debug)]
enum VMode {
    Kina,
    Glia,
    Quokka,
}

impl VMode {
    fn parse(s: &str) -> Option<VMode> {
        match s {
            "" => None,
            "kina" => Some(VMode::Kina),
            "glia" => Some(VMode::Glia),
            "quokka" => Some(VMode::Quokka),
            other => panic!("--vertical {other}: expected kina, glia or quokka"),
        }
    }
}

/// One set-valued upper-layer signal at one revision: candidate file -> the
/// upper-layer nodes it links to; score(f) = sum over nodes shared with the seed
/// of idf(node) = ln(1 + files / files linking the node).
#[derive(Clone, Default)]
struct Sig {
    sets: Vec<Vec<u32>>,
    post: Vec<Vec<u32>>,
    idf: Vec<f64>,
    names: Vec<String>,
    /// Round 2: a weight per (file, node) link, aligned with `sets` (empty = every link 1.0)
    wts: Vec<Vec<f32>>,
    /// Round 2: a text weight per (node, file) posting, aligned with `post` (empty = none); the
    /// `+trank` ranker orders the typed-link candidates by it
    tw: Vec<Vec<f32>>,
}

impl Sig {
    fn build(keyed: &[BTreeSet<String>]) -> Sig {
        let nf = keyed.len();
        let mut all: BTreeSet<&String> = BTreeSet::new();
        for s in keyed {
            all.extend(s.iter());
        }
        let names: Vec<String> = all.into_iter().cloned().collect();
        let ix: HashMap<&str, u32> = names.iter().enumerate().map(|(i, n)| (n.as_str(), i as u32)).collect();
        let sets: Vec<Vec<u32>> = keyed.iter().map(|s| s.iter().map(|k| ix[k.as_str()]).collect()).collect();
        Sig::from_sets(nf, sets, names)
    }

    fn from_sets(nf: usize, sets: Vec<Vec<u32>>, names: Vec<String>) -> Sig {
        let mut post = vec![Vec::new(); names.len()];
        for (f, s) in sets.iter().enumerate() {
            for &x in s {
                post[x as usize].push(f as u32);
            }
        }
        let idf = post.iter().map(|p| if p.is_empty() { 0.0 } else { (1.0 + nf as f64 / p.len() as f64).ln() }).collect();
        Sig { sets, post, idf, names, wts: Vec::new(), tw: Vec::new() }
    }

    /// Round 2: a set signal whose links carry weights (a names-file link from a done unit 0.5).
    fn build_w(keyed: &[BTreeMap<String, f64>]) -> Sig {
        let sets: Vec<BTreeSet<String>> = keyed.iter().map(|m| m.keys().cloned().collect()).collect();
        let mut sig = Sig::build(&sets);
        sig.wts = sig.sets.iter().enumerate().map(|(f, s)| s.iter().map(|&x| keyed[f][&sig.names[x as usize]] as f32).collect()).collect();
        sig
    }

    fn weight(&self, f: usize, x: u32) -> f64 {
        if self.wts.is_empty() {
            return 1.0;
        }
        let i = self.sets[f].binary_search(&x).unwrap_or(0);
        f64::from(self.wts[f].get(i).copied().unwrap_or(1.0))
    }

    fn score(&self, seed: usize) -> Vec<f64> {
        let mut s = vec![0.0; self.sets.len()];
        for (i, &x) in self.sets[seed].iter().enumerate() {
            let ws = if self.wts.is_empty() { 1.0 } else { f64::from(self.wts[seed][i]) };
            for &f in &self.post[x as usize] {
                s[f as usize] += self.idf[x as usize] * ws * if self.wts.is_empty() { 1.0 } else { self.weight(f as usize, x) };
            }
        }
        s[seed] = 0.0;
        s
    }

    /// Round 2 (`+trank`): the typed-link candidates (score > 0) ordered by the text weight of the
    /// nodes they share with the seed (min-max over the candidates), ties by the link score; a
    /// file with no typed link scores 0, so text adds no candidate.
    fn score_trank(&self, seed: usize) -> Vec<f64> {
        let base = self.score(seed);
        let mut t = vec![0.0f64; self.sets.len()];
        if !self.tw.is_empty() {
            for &x in &self.sets[seed] {
                for (j, &f) in self.post[x as usize].iter().enumerate() {
                    t[f as usize] += f64::from(self.tw[x as usize][j]);
                }
            }
        }
        let tmax = (0..base.len()).filter(|&f| f != seed && base[f] > 0.0).map(|f| t[f]).fold(0.0f64, f64::max);
        let bmax = base.iter().copied().fold(0.0f64, f64::max);
        (0..base.len())
            .map(|f| {
                if f == seed || base[f] <= 0.0 {
                    0.0
                } else {
                    1.0 + if tmax > 0.0 { t[f] / tmax } else { 0.0 } + 1e-3 * base[f] / bmax.max(1e-12)
                }
            })
            .collect()
    }

    /// The control: each file keeps its NUMBER of upper-layer links, but they are
    /// drawn at random (deterministically) from the nodes in use at this revision,
    /// uniformly or in proportion to how many files link each node.
    fn shuffled(&self, key: &str, weighted: bool) -> Sig {
        let m = self.names.len();
        let w: Vec<f64> = (0..m).map(|x| if weighted { self.post[x].len() as f64 } else { 1.0 }).collect();
        let total: f64 = w.iter().sum();
        let mut cum = Vec::with_capacity(m);
        let mut acc = 0.0;
        for &x in &w {
            acc += x;
            cum.push(acc);
        }
        let sets: Vec<Vec<u32>> = self
            .sets
            .iter()
            .enumerate()
            .map(|(f, s)| {
                if s.is_empty() || m == 0 {
                    return Vec::new();
                }
                let want = s.len().min(m);
                let mut rng = Rng(h64(&format!("{key}:{f}")));
                let mut got: BTreeSet<u32> = BTreeSet::new();
                let mut tries = 0;
                while got.len() < want && tries < 1000 {
                    tries += 1;
                    let r = (rng.next() >> 11) as f64 / (1u64 << 53) as f64 * total;
                    let x = cum.partition_point(|&c| c <= r).min(m - 1);
                    got.insert(x as u32);
                }
                got.into_iter().collect()
            })
            .collect();
        Sig::from_sets(self.sets.len(), sets, self.names.clone())
    }
}

/// Round 2: a signal scored through `score_trank` in the vertical rankers (`<name>+trank`).
fn is_trank(name: &str) -> bool {
    name.ends_with("+trank")
}

struct VLayer {
    /// The signals, in display order (V1, V2, ...).
    sigs: Vec<(String, Sig)>,
    /// Controls: V0 (uniform random ids, as specified), V0df[X] (popularity-preserving
    /// random nodes for each signal), V4perm (glia: the ruling -> packet map permuted).
    ctrls: Vec<(String, Sig)>,
    /// The commit message's own ids routed through the layer (seed-independent).
    vm: Option<Vec<f64>>,
    vm_cov: bool,
    info: BTreeMap<String, f64>,
    vocab: Vec<String>,
    /// Fourth run: resolved links with their text (the newest test commit only), for the audit.
    kaudit: Vec<KAudit>,
    /// Fourth run: (resolved, raw) signal pairs whose co-change links are compared.
    gains: Vec<(String, String)>,
}

// ---- git ----

fn git_text(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git grep -n -I` at `rev`, as (path, line) in file order; `pat` None = every line.
fn git_grep(dir: &str, rev: &str, pat: Option<&str>, specs: &[&str]) -> Vec<(String, String)> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(["grep", "-n", "-I", "--no-color"]);
    match pat {
        Some(p) => {
            cmd.args(["-E", "-e", p]);
        }
        None => {
            cmd.args(["-e", ""]);
        }
    }
    cmd.arg(rev);
    if !specs.is_empty() {
        cmd.arg("--");
        cmd.args(specs);
    }
    let out = cmd.output().expect("git grep");
    let text = String::from_utf8_lossy(&out.stdout);
    let pre = format!("{rev}:");
    let mut v = Vec::new();
    for l in text.lines() {
        let Some(rest) = l.strip_prefix(&pre) else { continue };
        let Some((path, rest)) = rest.split_once(':') else { continue };
        let Some((_, txt)) = rest.split_once(':') else { continue };
        v.push((path.to_string(), txt.to_string()));
    }
    v
}

/// sha -> (first parent, committer time) for every commit reachable from `rev`.
fn git_parents(dir: &str, rev: &str) -> HashMap<String, (Option<String>, i64)> {
    let text = git_text(dir, &["log", "--format=%H%x1f%P%x1f%ct", rev]).expect("git log parents");
    text.lines()
        .filter_map(|l| {
            let mut p = l.split('\x1f');
            let sha = p.next()?.to_string();
            let parent = p.next()?.split_whitespace().next().map(str::to_string);
            let ct = p.next()?.trim().parse().ok()?;
            Some((sha, (parent, ct)))
        })
        .collect()
}

// ---- scanners (no regex crate in glia-engine's dependencies) ----

fn wb(b: &[u8], i: usize) -> bool {
    i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')
}

fn we(b: &[u8], j: usize) -> bool {
    j >= b.len() || !(b[j].is_ascii_alphanumeric() || b[j] == b'_')
}

fn digits_at(b: &[u8], k: usize, max: usize) -> usize {
    let mut e = k;
    while e < b.len() && e - k < max && b[e].is_ascii_digit() {
        e += 1;
    }
    e
}

/// Hyphenated ids `[A-Z][A-Z0-9]{1,6}-[0-9]{1,4}[a-z]?` as (prefix, id), with the
/// `P-01/02` and `P-01..06` shorthands expanded.
fn hyphen_ids(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if !(b[i].is_ascii_uppercase() && wb(b, i) && (i == 0 || b[i - 1] != b'-')) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n && j - i < 8 && (b[j].is_ascii_uppercase() || b[j].is_ascii_digit()) {
            j += 1;
        }
        if (2..=7).contains(&(j - i)) && j < n && b[j] == b'-' {
            let k0 = j + 1;
            let k = digits_at(b, k0, 4);
            if k > k0 {
                let mut e = k;
                if e < n && b[e].is_ascii_lowercase() && we(b, e + 1) {
                    e += 1;
                }
                if we(b, e) && !(e + 1 < n && b[e] == b'-' && b[e + 1].is_ascii_digit()) {
                    let prefix = &s[i..j];
                    out.push((prefix.to_string(), s[i..e].to_string()));
                    let width = k - k0;
                    let lo: u32 = s[k0..k].parse().unwrap_or(0);
                    let mut m = e;
                    loop {
                        if m + 1 < n && b[m] == b'/' && b[m + 1].is_ascii_digit() {
                            let d = digits_at(b, m + 1, 4);
                            if we(b, d) {
                                out.push((prefix.to_string(), format!("{prefix}-{}", &s[m + 1..d])));
                                m = d;
                                continue;
                            }
                        }
                        if m + 2 < n && b[m] == b'.' && b[m + 1] == b'.' && b[m + 2].is_ascii_digit() {
                            let d = digits_at(b, m + 2, 4);
                            if we(b, d) {
                                let hi: u32 = s[m + 2..d].parse().unwrap_or(0);
                                if hi > lo && hi - lo <= 30 {
                                    for x in lo + 1..=hi {
                                        out.push((prefix.to_string(), format!("{prefix}-{x:0width$}")));
                                    }
                                }
                                m = d;
                                continue;
                            }
                        }
                        break;
                    }
                    i = m.max(i + 1);
                    continue;
                }
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Spec-kit's hyphen-less ids: `T###` (tasks), `R#` / `C#` / `D#` (research,
/// analysis and decision rows), for the kinds in `kinds`.
fn bare_ids(s: &str, kinds: &BTreeSet<u8>) -> Vec<(u8, String)> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    for i in 0..n {
        if !kinds.contains(&b[i]) || !wb(b, i) {
            continue;
        }
        let e = digits_at(b, i + 1, 4);
        let len = e - (i + 1);
        let ok = if b[i] == b'T' { len == 3 } else { (1..=2).contains(&len) };
        if !ok || !we(b, e) {
            continue;
        }
        if e + 1 < n && (b[e] == b'.' || b[e] == b'-') && b[e + 1].is_ascii_digit() {
            continue;
        }
        out.push((b[i], s[i..e].to_string()));
    }
    out
}

/// Packet ids `[A-Z][A-Z0-9]{0,4}.[0-9]{1,3}[a-z]{0,2}` as (prefix, id), with the
/// letter shorthands `LA.25a/b`, `LA.30a-c` and `DT.5h-DT.5l` expanded.
fn dot_ids(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < n {
        if !(b[i].is_ascii_uppercase() && wb(b, i) && (i == 0 || b[i - 1] != b'.')) {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < n && j - i < 6 && (b[j].is_ascii_uppercase() || b[j].is_ascii_digit()) {
            j += 1;
        }
        if (1..=5).contains(&(j - i)) && j + 1 < n && b[j] == b'.' && b[j + 1].is_ascii_digit() {
            let k = digits_at(b, j + 1, 3);
            let mut e = k;
            while e < n && e - k < 2 && b[e].is_ascii_lowercase() {
                e += 1;
            }
            if we(b, e) && !(e + 1 < n && b[e] == b'.' && b[e + 1].is_ascii_digit()) {
                let prefix = s[i..j].to_string();
                let base = s[i..k].to_string();
                out.push((prefix.clone(), s[i..e].to_string()));
                let last = if e > k { b[e - 1] } else { 0 };
                let mut m = e;
                if last != 0 {
                    // a/b
                    while m + 1 < n && b[m] == b'/' && b[m + 1].is_ascii_lowercase() && we(b, m + 2) {
                        out.push((prefix.clone(), format!("{base}{}", b[m + 1] as char)));
                        m += 2;
                    }
                    // a-c / a–c / a-DT.5l
                    let (dash, dl) = if m < n && b[m] == b'-' {
                        (true, 1)
                    } else if s[m..].starts_with('\u{2013}') {
                        (true, 3)
                    } else {
                        (false, 0)
                    };
                    if dash {
                        let r = m + dl;
                        let to = if r < n && b[r].is_ascii_lowercase() && we(b, r + 1) {
                            Some(b[r])
                        } else if s[r..].starts_with(&base) {
                            let q = r + base.len();
                            (q < n && b[q].is_ascii_lowercase() && we(b, q + 1)).then(|| b[q])
                        } else {
                            None
                        };
                        if let Some(to) = to
                            && to > last
                            && to - last <= 20
                        {
                            for c in last + 1..=to {
                                out.push((prefix.clone(), format!("{base}{}", c as char)));
                            }
                        }
                    }
                }
                i = m.max(i + 1);
                continue;
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// Spec-kit spec numbers a line names: `spec 012`, `Spec 012`, `specs/012-..`,
/// `012 T014`, `012 FR-057`, `012-US2`.
fn spec_refs(s: &str) -> Vec<u32> {
    let b = s.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    for i in 0..n {
        if !b[i].is_ascii_digit() || !wb(b, i) {
            continue;
        }
        let e = digits_at(b, i, 4);
        if e - i != 3 || (e < n && b[e].is_ascii_digit()) {
            continue;
        }
        let before = s[..i].to_ascii_lowercase();
        let after = &s[e..];
        let pre = before.ends_with("spec ") || before.ends_with("specs/") || before.ends_with("spec-") || before.ends_with("specs ");
        let ab = after.as_bytes();
        let post = after.starts_with(" FR-")
            || after.starts_with(" SC-")
            || after.starts_with("-US")
            || after.starts_with(" US")
            || (ab.len() >= 5 && ab[0] == b' ' && ab[1] == b'T' && ab[2..5].iter().all(u8::is_ascii_digit))
            || (ab.len() >= 3 && ab[0] == b' ' && matches!(ab[1], b'R' | b'C' | b'D') && ab[2].is_ascii_digit());
        if pre || post {
            out.push(s[i..e].parse().unwrap_or(0));
        }
    }
    out
}

const LAW_HEADS: &[&str] = &["Act", "Acts", "Rules", "Regulations", "Regulation", "Directives", "Directive"];
const LAW_CONNECT: &[&str] = &["and", "of", "the", "for", "on", "to", "&", "in"];
/// Words dropped from a law name (the private config's `law_drop` adds the jurisdictions').
const LAW_DROP: &[&str] = &["the", "us"];
/// Sentence fillers a capitalised run can start with ("Nothing in this Act", "Reach of the Privacy Act").
const LAW_LEAD: &[&str] = &[
    "neither", "nothing", "none", "no", "add", "adds", "amend", "amends", "amending", "enacted", "assent", "principal",
    "statute", "explanatory", "commentary", "reach", "division", "part", "under", "see", "per", "with", "from", "by",
    "as", "at", "or", "if", "is", "every", "when", "where", "why", "how", "not", "both", "either", "only", "also",
    "s", "section", "schedule", "item", "items", "chapter", "reg", "rule", "subsection", "paragraph",
];
const LAW_GENERIC: &[&str] = &[
    "this", "that", "each", "any", "an", "a", "our", "its", "their", "these", "those", "which", "under", "see", "per",
    "new", "same", "other", "relevant", "both", "all",
];

/// Law keys a line names: a run of capitalised words, acronyms and connectors
/// ending in Act / Rules / Regulations / Directive(s). Normalised: lower-case,
/// alphanumerics only, jurisdiction words dropped, the regime instrument's spellings
/// unified (the private config); the year is not part of the key (so two
/// jurisdictions' same-named Acts of different years are one key). File names
/// (`XX_Some_Act_2006_..pdf`) read the same.
fn law_keys(s: &str) -> Vec<String> {
    let p = prv();
    let dropped = |l: &str| LAW_DROP.contains(&l) || any_eq(&p.law_drop, l);
    let mut words: Vec<String> = Vec::new();
    for raw in s.split_whitespace() {
        let t = raw.trim_matches(|c: char| c == '`' || c == '"' || c == '\'' || c == '*');
        let lower = t.to_ascii_lowercase();
        if lower.ends_with(".pdf") || lower.ends_with(".txt") || lower.ends_with(".pdf)") {
            let stem = &t[..t.to_ascii_lowercase().rfind('.').unwrap_or(t.len())];
            words.extend(stem.split(['_', '-', '/']).filter(|w| !w.is_empty()).map(str::to_string));
            words.push(",".into());
        } else {
            for w in t.split('_') {
                if !w.is_empty() {
                    words.push(w.to_string());
                }
            }
        }
    }
    let mut out = Vec::new();
    for k in 0..words.len() {
        let core = words[k].trim_matches(|c: char| !c.is_alphanumeric());
        if !LAW_HEADS.contains(&core) || !words[k].starts_with(core.chars().next().unwrap_or(' ')) && !words[k].starts_with('(') {
            continue;
        }
        let mut name: Vec<String> = Vec::new();
        let mut m = k;
        while m > 0 && name.len() < 12 {
            let prev = &words[m - 1];
            if prev.ends_with(',') || prev.ends_with(';') || prev.ends_with(':') || prev.ends_with('.') || prev == "," {
                break;
            }
            let c = prev.trim_matches(|c: char| c == '(' || c == ')' || c == '[' || c == ']');
            let ok = match c.chars().next() {
                Some(ch) if ch.is_uppercase() => true,
                _ => LAW_CONNECT.contains(&c),
            };
            if !ok || c.is_empty() {
                break;
            }
            name.push(c.to_string());
            m -= 1;
        }
        name.reverse();
        for w in name.iter_mut() {
            for suf in ["'s", "\u{2019}s"] {
                if let Some(x) = w.strip_suffix(suf) {
                    *w = x.to_string();
                }
            }
        }
        while name.first().is_some_and(|w| {
            let l = w.to_ascii_lowercase();
            LAW_CONNECT.contains(&l.as_str()) || LAW_LEAD.contains(&l.as_str()) || dropped(&l)
        }) {
            name.remove(0);
        }
        let kept: Vec<&String> = name.iter().filter(|w| !dropped(&w.to_ascii_lowercase())).collect();
        if kept.is_empty() {
            continue;
        }
        if kept.len() == 1 && LAW_GENERIC.contains(&kept[0].to_ascii_lowercase().as_str()) {
            continue;
        }
        let acronym = kept.iter().any(|w| w.chars().filter(|c| c.is_ascii_uppercase()).count() >= 2);
        if core != "Act" && core != "Acts" && !acronym && kept.len() < 2 {
            continue;
        }
        let mut compact: String =
            kept.iter().flat_map(|w| w.chars()).filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
        for pat in &p.law_unify {
            if compact.contains(pat.as_str()) {
                compact = compact.replace(pat.as_str(), &p.law_unify_to);
                break;
            }
        }
        for x in &p.law_strip {
            compact = compact.replace(x.as_str(), "");
        }
        if compact.len() < 3 {
            continue;
        }
        let head = match core {
            "Acts" => "act".to_string(),
            "Regulation" => "regulations".to_string(),
            "Directive" => "directives".to_string(),
            h => h.to_ascii_lowercase(),
        };
        out.push(format!("{compact}_{head}"));
    }
    out
}

/// Candidate file paths resolved from path-like tokens in a line: an exact path,
/// or a path suffix (or a bare file name) that names exactly one candidate.
struct PathRes {
    exts: HashSet<String>,
    suffix: HashMap<String, Vec<u32>>,
}

impl PathRes {
    fn new(paths: &[String]) -> PathRes {
        let mut suffix: HashMap<String, Vec<u32>> = HashMap::new();
        let mut exts = HashSet::new();
        for (f, p) in paths.iter().enumerate() {
            exts.insert(ext_of(p));
            let segs: Vec<&str> = p.split('/').collect();
            for k in 0..segs.len() {
                suffix.entry(segs[k..].join("/")).or_default().push(f as u32);
            }
        }
        PathRes { exts, suffix }
    }

    fn files(&self, s: &str) -> Vec<u32> {
        let mut out = Vec::new();
        for raw in s.split(|c: char| {
            c.is_whitespace() || matches!(c, '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '<' | '>' | ',' | ';' | '|' | '*' | '{' | '}' | '=')
        }) {
            let mut t = raw.trim_end_matches(['.', ':', '!', '?']);
            t = t.trim_start_matches("./").trim_start_matches('/');
            if let Some((a, _)) = t.split_once(':') {
                t = a;
            }
            if t.len() < 4 || !t.contains('.') {
                continue;
            }
            if !self.exts.contains(&ext_of(t)) {
                continue;
            }
            if let Some(v) = self.suffix.get(t)
                && v.len() == 1
            {
                out.push(v[0]);
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The ids a markdown line DEFINES: the first token of a heading, of a table
/// row's first cell, or of a (checkbox) list item, possibly bold or quoted.
fn line_head(line: &str) -> &str {
    let t = line.trim_start();
    let t = if let Some(h) = t.strip_prefix('#') {
        h.trim_start_matches('#').trim_start()
    } else if let Some(r) = t.strip_prefix('|') {
        r.split('|').next().unwrap_or("").trim()
    } else {
        let t = t
            .strip_prefix("- ")
            .or_else(|| t.strip_prefix("* "))
            .or_else(|| t.strip_prefix("+ "))
            .unwrap_or_else(|| {
                let d = t.bytes().take_while(u8::is_ascii_digit).count();
                if d > 0 && t[d..].starts_with(". ") { &t[d + 2..] } else { t }
            });
        let t = t.trim_start();
        t.strip_prefix("[ ] ").or_else(|| t.strip_prefix("[x] ")).or_else(|| t.strip_prefix("[X] ")).unwrap_or(t)
    };
    t.trim_start_matches(['*', '`', '_', '~', ' '])
}

// ---- Kina (its id patterns are in the private config: `id_standards`, `id_bare`, `code_pat`) ----

#[derive(Default)]
struct DocSet {
    units: Vec<String>,
    is_spec: Vec<bool>,
    spec_unit: BTreeMap<u32, u32>,
    defs: BTreeMap<String, BTreeSet<u32>>,
    laws: Vec<BTreeSet<String>>,
    law_vocab: BTreeSet<String>,
    /// (unit, ids the line defines, files the line names, the line is an unchecked `- [ ]` task)
    mentions: Vec<(u32, Vec<String>, Vec<u32>, bool)>,
    vocab_hyphen: BTreeMap<String, usize>,
    vocab_bare: BTreeMap<u8, usize>,
}

fn kina_unit(path: &str) -> String {
    let segs: Vec<&str> = path.split('/').collect();
    if segs.len() >= 3 && segs[0] == "specs" { format!("specs/{}", segs[1]) } else { path.to_string() }
}

fn build_docset(lines: &[(String, String)], pr: &PathRes) -> DocSet {
    let mut d = DocSet::default();
    let mut uix: HashMap<String, u32> = HashMap::new();
    let bare: BTreeSet<u8> = bare_kinds();
    for (path, line) in lines {
        let un = kina_unit(path);
        let u = *uix.entry(un.clone()).or_insert_with(|| {
            d.units.push(un.clone());
            let spec = un.starts_with("specs/");
            d.is_spec.push(spec);
            d.laws.push(BTreeSet::new());
            if spec {
                let num: String = un["specs/".len()..].chars().take_while(char::is_ascii_digit).collect();
                if num.len() == 3 {
                    d.spec_unit.insert(num.parse().unwrap_or(0), (d.units.len() - 1) as u32);
                }
            }
            (d.units.len() - 1) as u32
        });
        // definitions
        let head = line_head(line);
        let mut defined: Vec<String> = Vec::new();
        if let Some((p, id)) = hyphen_ids(head).into_iter().next()
            && head.starts_with(&id)
        {
            if !any_eq(&prv().id_standards, &p) {
                *d.vocab_hyphen.entry(p).or_default() += 1;
                defined.push(id);
            }
            // `| ABC-01/02 |`: the shorthand siblings are defined too
            for (p2, id2) in hyphen_ids(head).into_iter().skip(1) {
                if head[..head.find([' ', '*', '|', ':']).unwrap_or(head.len())].contains(&id2[p2.len() + 1..]) {
                    defined.push(id2);
                }
            }
        } else if let Some((k, id)) = bare_ids(head, &bare).into_iter().next()
            && head.starts_with(&id)
        {
            *d.vocab_bare.entry(k).or_default() += 1;
            defined.push(id);
        }
        for id in &defined {
            d.defs.entry(id.clone()).or_default().insert(u);
        }
        // laws
        for k in law_keys(line) {
            d.laws[u as usize].insert(k);
        }
        // files named
        let files = pr.files(line);
        if !files.is_empty() {
            let t = line.trim_start();
            let open = t.starts_with("- [ ]") || t.starts_with("* [ ]");
            d.mentions.push((u, defined, files, open));
        }
    }
    // A law key counts only when at least two doc units name it (drops sentence
    // fragments a single page happens to capitalise).
    let mut units_per: BTreeMap<String, usize> = BTreeMap::new();
    for ls in &d.laws {
        for k in ls {
            *units_per.entry(k.clone()).or_default() += 1;
        }
    }
    d.law_vocab = units_per.into_iter().filter(|(_, n)| *n >= 2).map(|(k, _)| k).collect();
    let vocab = d.law_vocab.clone();
    for ls in d.laws.iter_mut() {
        ls.retain(|k| vocab.contains(k));
    }
    d
}

/// One cited id resolved to its upper-layer token and the units that define it.
/// Spec-kit ids (FR-002, T014, ...) are numbered per spec, so a spec-scoped id is
/// qualified `specs/<dir>|<id>` by a spec number on the same line, its only
/// defining spec, or the one spec the file names; global ids (ABC-01, XY-13, ...)
/// keep their bare form.
fn kina_resolve(
    d: &DocSet,
    id: &str,
    is_bare: bool,
    line_specs: &[u32],
    file_specs: &[u32],
    stats: &mut BTreeMap<String, f64>,
) -> Option<(String, Vec<u32>)> {
    let units = |nums: &[u32]| -> Vec<u32> {
        let mut v: Vec<u32> = nums.iter().filter_map(|n| d.spec_unit.get(n).copied()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let tok = |u: u32| format!("{}|{id}", d.units[u as usize]);
    let mut bump = |k: &str| *stats.entry(format!("resolve_{k}")).or_default() += 1.0;
    let defs: Vec<u32> = d.defs.get(id).map(|s| s.iter().copied().collect()).unwrap_or_default();
    let lu = units(line_specs);
    if defs.is_empty() {
        if lu.len() == 1 {
            bump("undefined_line_spec");
            return Some((tok(lu[0]), lu));
        }
        if is_bare {
            bump("dropped_bare_undefined");
            return None;
        }
        bump("undefined_global");
        return Some((id.to_string(), Vec::new()));
    }
    let spec_defs: Vec<u32> = defs.iter().copied().filter(|&u| d.is_spec[u as usize]).collect();
    let glob_defs: Vec<u32> = defs.iter().copied().filter(|&u| !d.is_spec[u as usize]).collect();
    if spec_defs.is_empty() {
        bump("global");
        return Some((id.to_string(), defs));
    }
    let pick = |c: &[u32]| -> Vec<u32> { c.iter().copied().filter(|u| spec_defs.contains(u)).collect() };
    let q = pick(&lu);
    if q.len() == 1 {
        bump("line_spec");
        return Some((tok(q[0]), q));
    }
    // An id a global doc also defines (a plan's D6 vs a spec's D6) is scoped to
    // a spec only by a qualifier on its own line.
    if !glob_defs.is_empty() {
        bump("global_over_spec");
        return Some((id.to_string(), glob_defs));
    }
    if spec_defs.len() == 1 {
        bump("unique_spec");
        return Some((tok(spec_defs[0]), spec_defs));
    }
    let q = pick(&units(file_specs));
    if q.len() == 1 {
        bump("file_spec");
        return Some((tok(q[0]), q));
    }
    if is_bare {
        bump("dropped_bare_ambiguous");
        return None;
    }
    bump("ambiguous");
    Some((format!("*|{id}"), spec_defs))
}

/// Per candidate file: (tokens, units, direct law keys), from the code lines.
type Cites = (Vec<BTreeSet<String>>, Vec<BTreeSet<u32>>, Vec<BTreeSet<String>>);

fn kina_cites(d: &DocSet, code: &BTreeMap<u32, Vec<String>>, nf: usize, stats: &mut BTreeMap<String, f64>) -> Cites {
    let kinds: BTreeSet<u8> = d.vocab_bare.keys().copied().collect();
    let mut toks = vec![BTreeSet::new(); nf];
    let mut units = vec![BTreeSet::new(); nf];
    let mut laws = vec![BTreeSet::new(); nf];
    for (&f, lines) in code {
        let file_specs: Vec<u32> = lines.iter().flat_map(|l| spec_refs(l)).collect();
        for l in lines {
            let ls = spec_refs(l);
            for n in &ls {
                if let Some(&u) = d.spec_unit.get(n) {
                    units[f as usize].insert(u);
                }
            }
            for (p, id) in hyphen_ids(l) {
                if !d.vocab_hyphen.contains_key(&p) {
                    continue;
                }
                if let Some((t, us)) = kina_resolve(d, &id, false, &ls, &file_specs, stats) {
                    toks[f as usize].insert(t);
                    units[f as usize].extend(us);
                }
            }
            for (_, id) in bare_ids(l, &kinds) {
                if let Some((t, us)) = kina_resolve(d, &id, true, &ls, &file_specs, stats) {
                    toks[f as usize].insert(t);
                    units[f as usize].extend(us);
                }
            }
            for k in law_keys(l) {
                if d.law_vocab.contains(&k) {
                    laws[f as usize].insert(k);
                }
            }
        }
    }
    (toks, units, laws)
}

struct KinaSignals {
    i: Vec<BTreeSet<String>>,
    s: Vec<BTreeSet<String>>,
    l: Vec<BTreeSet<String>>,
    ld: Vec<BTreeSet<String>>,
    m: Vec<BTreeSet<String>>,
    m_open: Vec<BTreeSet<String>>,
    m_rest: Vec<BTreeSet<String>>,
}

fn kina_signals(d: &DocSet, code: &BTreeMap<u32, Vec<String>>, nf: usize, stats: &mut BTreeMap<String, f64>) -> KinaSignals {
    let (i, units, direct) = kina_cites(d, code, nf, stats);
    let s: Vec<BTreeSet<String>> = units.iter().map(|us| us.iter().map(|&u| d.units[u as usize].clone()).collect()).collect();
    let l: Vec<BTreeSet<String>> =
        units.iter().map(|us| us.iter().flat_map(|&u| d.laws[u as usize].iter().cloned()).collect()).collect();
    let ld: Vec<BTreeSet<String>> = l.iter().zip(&direct).map(|(a, b)| a.union(b).cloned().collect()).collect();
    let mut m = vec![BTreeSet::new(); nf];
    let mut m_open = vec![BTreeSet::new(); nf];
    let mut m_rest = vec![BTreeSet::new(); nf];
    for (u, ids, files, open) in &d.mentions {
        let un = &d.units[*u as usize];
        let toks: Vec<String> = if ids.is_empty() {
            vec![un.clone()]
        } else {
            ids.iter().map(|id| if d.is_spec[*u as usize] { format!("{un}|{id}") } else { id.clone() }).collect()
        };
        for &f in files {
            m[f as usize].extend(toks.iter().cloned());
            if *open {
                m_open[f as usize].extend(toks.iter().cloned());
            } else {
                m_rest[f as usize].extend(toks.iter().cloned());
            }
        }
    }
    KinaSignals { i, s, l, ld, m, m_open, m_rest }
}

fn group_code(lines: Vec<(String, String)>, pidx: &HashMap<&str, u32>) -> BTreeMap<u32, Vec<String>> {
    let mut code: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    for (p, l) in lines {
        if let Some(&f) = pidx.get(p.as_str()) {
            code.entry(f).or_default().push(l);
        }
    }
    code
}

fn kina_doc_lines(git: &str, rev: &str) -> Vec<(String, String)> {
    git_grep(git, rev, None, &["*.md", ":!.specify", ":!.claude", ":!**/node_modules/**"])
}

// ---- glia ----

const GLIA_PACKETS: &[&str] = &[
    "dev-notes/wave-packets.json",
    "dev-notes/leap-packets.json",
    "dev-notes/leap-051-packets.json",
    "dev-notes/leap-052-packets.json",
];
const GLIA_SCHEDULES: &[&str] = &[
    "dev-notes/wave-runner/schedule.py",
    "dev-notes/wave-runner/leap_schedule.py",
    "dev-notes/wave-runner/schedule_051.py",
    "dev-notes/wave-runner/schedule_052.py",
];
const GLIA_PLANS: &[&str] =
    &["dev-notes/next-leap-0.5.0.md", "dev-notes/next-leap-0.5.1.md", "dev-notes/next-leap-0.5.2.md", "dev-notes/tweaks-0.5.2.md"];
const GLIA_CORRECTIONS: &[&str] = &[
    "dev-notes/packet-corrections.json",
    "dev-notes/leap-corrections.json",
    "dev-notes/leap-051-corrections.json",
    "dev-notes/leap-052-corrections.json",
];

#[derive(Clone)]
struct Packet {
    id: String,
    batch: String,
    files: Vec<String>,
    /// Q-numbers (next-leap-0.5.2.md §0.1) the packet's text cites; 0.5.2 packets only.
    qs: Vec<u32>,
}

struct BlobCache {
    text: std::sync::Mutex<HashMap<String, std::sync::Arc<String>>>,
    packets: std::sync::Mutex<HashMap<String, std::sync::Arc<Vec<Packet>>>>,
}

impl BlobCache {
    fn get(&self, git: &str, blob: &str) -> std::sync::Arc<String> {
        if let Some(t) = self.text.lock().unwrap().get(blob) {
            return t.clone();
        }
        let t = std::sync::Arc::new(git_text(git, &["cat-file", "-p", blob]).unwrap_or_default());
        self.text.lock().unwrap().insert(blob.to_string(), t.clone());
        t
    }

    fn packets(&self, git: &str, blob: &str, with_q: bool) -> std::sync::Arc<Vec<Packet>> {
        if let Some(p) = self.packets.lock().unwrap().get(blob) {
            return p.clone();
        }
        let text = self.get(git, blob);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        let mut out = Vec::new();
        if let Some(ps) = v.get("packets").and_then(|p| p.as_array()) {
            for p in ps {
                let Some(id) = p.get("id").and_then(|x| x.as_str()) else { continue };
                let batch = p.get("batch").and_then(|x| x.as_str()).unwrap_or("").to_string();
                let files = p
                    .get("files_touched")
                    .and_then(|x| x.as_array())
                    .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                let mut qs = Vec::new();
                if with_q {
                    let t = p.to_string();
                    let b = t.as_bytes();
                    for i in 0..b.len() {
                        if b[i] == b'Q' && wb(b, i) {
                            let e = digits_at(b, i + 1, 2);
                            if e > i + 1 && we(b, e) {
                                let q: u32 = t[i + 1..e].parse().unwrap_or(0);
                                if (1..=25).contains(&q) {
                                    qs.push(q);
                                }
                            }
                        }
                    }
                    qs.sort_unstable();
                    qs.dedup();
                }
                out.push(Packet { id: id.to_string(), batch, files, qs });
            }
        }
        let a = std::sync::Arc::new(out);
        self.packets.lock().unwrap().insert(blob.to_string(), a.clone());
        a
    }
}

fn ls_tree(git: &str, rev: &str, paths: &[&str]) -> BTreeMap<String, String> {
    let mut args = vec!["ls-tree", rev, "--"];
    args.extend_from_slice(paths);
    let text = git_text(git, &args).unwrap_or_default();
    text.lines()
        .filter_map(|l| {
            let (meta, path) = l.split_once('\t')?;
            let blob = meta.split_whitespace().nth(2)?;
            Some((path.to_string(), blob.to_string()))
        })
        .collect()
}

fn family(id: &str) -> String {
    id.trim_end_matches(|c: char| c.is_ascii_lowercase()).to_string()
}

struct GliaDocs {
    packets: BTreeMap<String, Packet>,
    prefixes: BTreeSet<String>,
    wave: BTreeMap<String, String>,
    /// ruling -> packets
    rulings: BTreeMap<String, BTreeSet<String>>,
}

fn landed_waves(name: &str, text: &str, into: &mut BTreeMap<String, String>) {
    let mut on = false;
    for l in text.lines() {
        if l.starts_with("LANDED = {") {
            on = true;
            continue;
        }
        if on {
            if l.trim_start().starts_with('}') {
                break;
            }
            let Some((n, rest)) = l.trim().split_once(':') else { continue };
            let Ok(n) = n.trim().parse::<u32>() else { continue };
            let ids = rest.trim().trim_end_matches(',').trim_matches('"');
            for id in ids.split_whitespace() {
                into.entry(id.to_string()).or_insert_with(|| format!("{name}:W{n}"));
            }
        }
    }
}

/// Ruling items in a plan doc: inside any section whose heading says "decision"
/// or "ruling", each numbered item and each table row is one ruling; the packets
/// it names (ids, `P.n` families written `P-n`, and, in tweaks-0.5.2.md, `T<n>`
/// for the DT.<n> family and the T1-T13 gap rows) are its packets.
fn plan_rulings(
    doc: &str,
    text: &str,
    prefixes: &BTreeSet<String>,
    fams: &BTreeMap<String, BTreeSet<String>>,
    out: &mut BTreeMap<String, BTreeSet<String>>,
) {
    let tweaks = doc.ends_with("tweaks-0.5.2.md");
    let mut heads: Vec<(usize, String)> = Vec::new();
    let mut cur: Option<(String, String)> = None;
    let close = |cur: &mut Option<(String, String)>, out: &mut BTreeMap<String, BTreeSet<String>>| {
        let Some((key, t)) = cur.take() else { return };
        let mut ps: BTreeSet<String> = BTreeSet::new();
        for (p, id) in dot_ids(&t) {
            if prefixes.contains(&p) {
                ps.insert(id);
            }
        }
        for (p, id) in hyphen_ids(&t) {
            if prefixes.contains(&p) {
                let fam = format!("{p}.{}", &id[p.len() + 1..]);
                ps.extend(fams.get(&fam).into_iter().flatten().cloned());
            }
        }
        if tweaks {
            let b = t.as_bytes();
            for i in 0..b.len() {
                if b[i] == b'T' && wb(b, i) {
                    let e = digits_at(b, i + 1, 2);
                    if e > i + 1 && we(b, e) {
                        ps.extend(fams.get(&format!("DT.{}", &t[i + 1..e])).into_iter().flatten().cloned());
                    }
                }
            }
        }
        if !ps.is_empty() {
            out.entry(format!("{doc}#{key}")).or_default().extend(ps);
        }
    };
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with('#') {
            close(&mut cur, out);
            let level = t.bytes().take_while(|&c| c == b'#').count();
            heads.retain(|(l, _)| *l < level);
            heads.push((level, t.trim_start_matches('#').trim().to_string()));
            continue;
        }
        let in_rulings = heads.iter().any(|(_, h)| {
            let h = h.to_ascii_lowercase();
            h.contains("decision") || h.contains("ruling")
        });
        let section = heads.last().map_or(String::new(), |(_, h)| h.chars().take(40).collect());
        let gap_row = tweaks && t.starts_with("| T") && t[3..].starts_with(|c: char| c.is_ascii_digit());
        let gap_para = tweaks && t.starts_with('T') && t[1..].starts_with(|c: char| c.is_ascii_digit()) && t.contains(" (James");
        if gap_row || gap_para {
            close(&mut cur, out);
            let n: String = t.trim_start_matches(['|', ' ', 'T']).chars().take_while(char::is_ascii_digit).collect();
            cur = Some((format!("T{n}"), t.to_string()));
            close(&mut cur, out);
            continue;
        }
        if !in_rulings {
            close(&mut cur, out);
            continue;
        }
        let d = t.bytes().take_while(u8::is_ascii_digit).count();
        if d > 0 && t[d..].starts_with(". ") {
            close(&mut cur, out);
            cur = Some((format!("{section}/{}", &t[..d]), t.to_string()));
        } else if t.starts_with('|') {
            close(&mut cur, out);
            let cell = t.trim_start_matches('|').split('|').next().unwrap_or("").trim().to_string();
            if !cell.is_empty() && !cell.starts_with("---") && cell != "#" && cell != "decision" {
                cur = Some((format!("{section}/{cell}"), t.to_string()));
                close(&mut cur, out);
            }
        } else if t.is_empty() {
            close(&mut cur, out);
        } else if let Some((_, buf)) = cur.as_mut() {
            buf.push(' ');
            buf.push_str(t);
        }
    }
    close(&mut cur, out);
}

fn glia_docs(git: &str, rev: &str, cache: &BlobCache) -> GliaDocs {
    let mut all: Vec<&str> = Vec::new();
    for g in [GLIA_PACKETS, GLIA_SCHEDULES, GLIA_PLANS, GLIA_CORRECTIONS] {
        all.extend_from_slice(g);
    }
    let blobs = ls_tree(git, rev, &all);
    let mut packets: BTreeMap<String, Packet> = BTreeMap::new();
    for &p in GLIA_PACKETS {
        if let Some(b) = blobs.get(p) {
            for pk in cache.packets(git, b, p.ends_with("leap-052-packets.json")).iter() {
                packets.entry(pk.id.clone()).or_insert_with(|| pk.clone());
            }
        }
    }
    let prefixes: BTreeSet<String> = packets.keys().map(|id| id.split('.').next().unwrap_or("").to_string()).collect();
    let mut fams: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for id in packets.keys() {
        fams.entry(family(id)).or_default().insert(id.clone());
    }
    let mut wave = BTreeMap::new();
    for &s in GLIA_SCHEDULES {
        if let Some(b) = blobs.get(s) {
            let name = s.rsplit('/').next().unwrap_or(s).trim_end_matches(".py");
            landed_waves(name, &cache.get(git, b), &mut wave);
        }
    }
    let mut rulings: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for &d in GLIA_PLANS {
        if let Some(b) = blobs.get(d) {
            let name = d.rsplit('/').next().unwrap_or(d);
            plan_rulings(name, &cache.get(git, b), &prefixes, &fams, &mut rulings);
        }
    }
    for pk in packets.values() {
        for q in &pk.qs {
            rulings.entry(format!("next-leap-0.5.2.md#Q{q}")).or_default().insert(pk.id.clone());
        }
    }
    for &c in GLIA_CORRECTIONS {
        let Some(b) = blobs.get(c) else { continue };
        let v: serde_json::Value = serde_json::from_str(&cache.get(git, b)).unwrap_or(serde_json::Value::Null);
        let Some(obj) = v.get("corrections").and_then(|x| x.as_object()) else { continue };
        let name = c.rsplit('/').next().unwrap_or(c);
        for (k, val) in obj {
            let text = val.as_str().map_or_else(|| val.to_string(), str::to_string);
            if !text.contains("James") {
                continue;
            }
            let mut ps: BTreeSet<String> = BTreeSet::new();
            if packets.contains_key(k) {
                ps.insert(k.clone());
            }
            for (p, id) in dot_ids(&text) {
                if prefixes.contains(&p) {
                    ps.insert(id);
                }
            }
            if !ps.is_empty() {
                rulings.entry(format!("{name}#{k}")).or_default().extend(ps);
            }
        }
    }
    GliaDocs { packets, prefixes, wave, rulings }
}

// ---- layer assembly ----

/// (path, line) of every doc line at one revision, shared between test commits.
type DocLines = std::sync::Arc<Vec<(String, String)>>;

struct VEnv<'a> {
    mode: VMode,
    git: &'a str,
    wiki: &'a str,
    paths: &'a [String],
    pidx: &'a HashMap<&'a str, u32>,
    pr: PathRes,
    parents: HashMap<String, (Option<String>, i64)>,
    /// wiki commits (committer time, sha), oldest first
    wiki_revs: Vec<(i64, String)>,
    wiki_head_docs: Vec<(String, String)>,
    wiki_lines: std::sync::Mutex<HashMap<String, DocLines>>,
    cache: BlobCache,
    /// Fourth run: the resolvers' shared state (None without `--keys`).
    keys: Option<KEnv>,
    /// Round 2: where the keyed run writes its audits
    keys_out: String,
}

/// The keyed run's state shared by every test commit's layer.
struct KEnv {
    kp: KPaths,
    known: Option<LawKnown>,
    blobs: KBlobs,
    /// the test commit whose layer is written for the hand audit (the newest)
    last_sha: String,
    pkc: std::sync::Mutex<HashMap<String, std::sync::Arc<GPkFile>>>,
}

fn controls(name: &str, sig: &Sig, sha: &str, ctrls: &mut Vec<(String, Sig)>) {
    ctrls.push((format!("V0df[{name}]"), sig.shuffled(&format!("{sha}:{name}:df"), true)));
}

fn msg_route(r: &Sig, msg_toks: &BTreeSet<String>, nf: usize) -> (Vec<f64>, bool) {
    let mut v = vec![0.0; nf];
    let mut cov = false;
    for (x, name) in r.names.iter().enumerate() {
        if msg_toks.contains(name) {
            cov = true;
            for &f in &r.post[x] {
                v[f as usize] += r.idf[x];
            }
        }
    }
    (v, cov)
}

fn build_layer(env: &VEnv, sha: &str, msg: &str) -> VLayer {
    let nf = env.paths.len();
    let mut info: BTreeMap<String, f64> = BTreeMap::new();
    let parent = env.parents.get(sha).and_then(|p| p.0.clone());
    let empty = vec![BTreeSet::<String>::new(); nf];
    let Some(p) = parent else {
        info.insert("no_parent".into(), 1.0);
        let sigs: Vec<(String, Sig)> = vertical_names(env.mode).iter().map(|n| (n.to_string(), Sig::build(&empty))).collect();
        let mut ctrls = Vec::new();
        if env.mode != VMode::Quokka {
            ctrls.push(("V0".to_string(), Sig::build(&empty)));
        }
        for (n, s) in &sigs {
            controls(n, s, sha, &mut ctrls);
        }
        if env.mode == VMode::Glia {
            for n in ["V4perm", "V4pperm", "V14perm"] {
                ctrls.push((n.into(), Sig::build(&empty)));
            }
        }
        let mut sigs = sigs;
        if env.keys.is_some() {
            match env.mode {
                VMode::Kina => key_sigs_kina(None, None, None, nf, sha, &mut sigs, &mut ctrls),
                VMode::Glia => key_sigs_glia(None, nf, sha, &mut sigs, &mut ctrls),
                VMode::Quokka => {}
            }
        }
        let vm = (env.mode != VMode::Quokka).then(|| vec![0.0; nf]);
        let gains = if env.keys.is_some() { key_gain_pairs(env.mode) } else { Vec::new() };
        return VLayer { sigs, ctrls, vm, vm_cov: false, info, vocab: Vec::new(), kaudit: Vec::new(), gains };
    };
    match env.mode {
        VMode::Kina => {
            let pct = env.parents.get(&p).map_or(0, |x| x.1);
            let mut docs = kina_doc_lines(env.git, &p);
            let wrev = env.wiki_revs.iter().rev().find(|(ct, _)| *ct <= pct).map(|(_, s)| s.clone());
            if let Some(w) = &wrev {
                let cached = env.wiki_lines.lock().unwrap().get(w).cloned();
                let lines = cached.unwrap_or_else(|| {
                    let l = std::sync::Arc::new(git_grep(env.wiki, w, None, &["*.md"]));
                    env.wiki_lines.lock().unwrap().insert(w.clone(), l.clone());
                    l
                });
                docs.extend(lines.iter().cloned());
                info.insert("wiki_aligned".into(), 1.0);
            }
            let d = build_docset(&docs, &env.pr);
            let code = group_code(git_grep(env.git, &p, Some(&prv().code_pat), &[]), env.pidx);
            let ks = kina_signals(&d, &code, nf, &mut info);
            let dw = build_docset(&env.wiki_head_docs, &env.pr);
            let mut junk = BTreeMap::new();
            let kw = kina_signals(&dw, &code, nf, &mut junk);
            let r: Vec<BTreeSet<String>> = ks.i.iter().zip(&ks.m).map(|(a, b)| a.union(b).cloned().collect()).collect();
            let mut sigs = vec![
                ("V1".to_string(), Sig::build(&ks.i)),
                ("V2".to_string(), Sig::build(&ks.s)),
                ("V3".to_string(), Sig::build(&ks.l)),
                ("V3d".to_string(), Sig::build(&ks.ld)),
                ("V2m".to_string(), Sig::build(&ks.m)),
                ("V1m".to_string(), Sig::build(&r)),
                ("V2w".to_string(), Sig::build(&kw.s)),
                ("V3w".to_string(), Sig::build(&kw.l)),
                ("V2mw".to_string(), Sig::build(&kw.m)),
                ("V2mP".to_string(), Sig::build(&ks.m_open)),
                ("V2mD".to_string(), Sig::build(&ks.m_rest)),
            ];
            let mut ctrls = vec![("V0".to_string(), sigs[0].1.shuffled(&format!("{sha}:V1:uniform"), false))];
            for (n, s) in &sigs {
                controls(n, s, sha, &mut ctrls);
            }
            // the message's own ids
            let mut msg_toks = BTreeSet::new();
            let kinds: BTreeSet<u8> = d.vocab_bare.keys().copied().collect();
            let file_specs: Vec<u32> = msg.lines().flat_map(spec_refs).collect();
            let mut ms = BTreeMap::new();
            for l in msg.lines() {
                let ls = spec_refs(l);
                for (pfx, id) in hyphen_ids(l) {
                    if d.vocab_hyphen.contains_key(&pfx)
                        && let Some((t, _)) = kina_resolve(&d, &id, false, &ls, &file_specs, &mut ms)
                    {
                        msg_toks.insert(t);
                    }
                }
                for (_, id) in bare_ids(l, &kinds) {
                    if let Some((t, _)) = kina_resolve(&d, &id, true, &ls, &file_specs, &mut ms) {
                        msg_toks.insert(t);
                    }
                }
            }
            let (vm, vm_cov) = msg_route(&sigs[5].1, &msg_toks, nf);
            info.insert("code_files_citing".into(), ks.i.iter().filter(|s| !s.is_empty()).count() as f64);
            info.insert("doc_units".into(), d.units.len() as f64);
            info.insert("spec_units".into(), d.spec_unit.len() as f64);
            info.insert("law_keys".into(), d.law_vocab.len() as f64);
            info.insert("defined_ids".into(), d.defs.len() as f64);
            let mut vocab: Vec<String> = d.vocab_hyphen.iter().map(|(k, v)| format!("{k}-* ({v} defs)")).collect();
            vocab.extend(d.vocab_bare.iter().map(|(k, v)| format!("{}### ({v} defs)", *k as char)));
            vocab.push(format!("laws: {}", d.law_vocab.iter().cloned().collect::<Vec<_>>().join(", ")));
            let mut kaudit = Vec::new();
            if let Some(ke) = &env.keys {
                let code_lines = git_grep(env.git, &p, Some(&prv().keys_code_pat), &[]);
                let bm = ke.blobs.index(&p, nf);
                let known = ke.known.as_ref().expect("--keys kina needs the law");
                let km = kina_model(&docs, &code_lines, &ke.kp, &env.pr, known, Some(&bm), nf, sha == ke.last_sha, false);
                // round 2: the same revision with namespaced bare ids
                let km_ns = kina_model(&docs, &code_lines, &ke.kp, &env.pr, known, Some(&bm), nf, sha == ke.last_sha, true);
                for (k, v) in &km.stats {
                    *info.entry(k.clone()).or_default() += v;
                }
                key_sigs_kina(Some(&km), Some(&km_ns), Some(&bm), nf, sha, &mut sigs, &mut ctrls);
                kaudit = km.audit;
                if sha == ke.last_sha {
                    write_ns_audit(&ke_dir(env), &km_ns.ns_audit);
                }
            }
            let gains = if env.keys.is_some() { key_gain_pairs(env.mode) } else { Vec::new() };
            VLayer { sigs, ctrls, vm: Some(vm), vm_cov, info, vocab, kaudit, gains }
        }
        VMode::Glia => {
            let gd = glia_docs(env.git, &p, &env.cache);
            let code = group_code(git_grep(env.git, &p, Some("[A-Z][A-Z0-9]{0,4}[.][0-9]"), &[]), env.pidx);
            let mut i = vec![BTreeSet::new(); nf];
            for (&f, lines) in &code {
                for l in lines {
                    for (pfx, id) in dot_ids(l) {
                        if gd.prefixes.contains(&pfx) {
                            i[f as usize].insert(id);
                        }
                    }
                }
            }
            let fam: Vec<BTreeSet<String>> = i.iter().map(|s| s.iter().map(|x| family(x)).collect()).collect();
            let bat: Vec<BTreeSet<String>> = i
                .iter()
                .map(|s| {
                    s.iter()
                        .map(|x| {
                            gd.packets.get(x).filter(|p| !p.batch.is_empty()).map_or_else(
                                || x.split('.').next().unwrap_or("").to_string(),
                                |p| p.batch.clone(),
                            )
                        })
                        .collect()
                })
                .collect();
            let wav: Vec<BTreeSet<String>> = i.iter().map(|s| s.iter().filter_map(|x| gd.wave.get(x).cloned()).collect()).collect();
            let mut m = vec![BTreeSet::new(); nf];
            // split by the schedule at the parent: a packet with no LANDED wave yet is pending
            let mut m_pend = vec![BTreeSet::new(); nf];
            let mut m_done = vec![BTreeSet::new(); nf];
            for pk in gd.packets.values() {
                let pending = !gd.wave.contains_key(&pk.id);
                for entry in &pk.files {
                    for f in env.pr.files(entry) {
                        m[f as usize].insert(pk.id.clone());
                        if pending {
                            m_pend[f as usize].insert(pk.id.clone());
                        } else {
                            m_done[f as usize].insert(pk.id.clone());
                        }
                    }
                }
            }
            let r: Vec<BTreeSet<String>> = i.iter().zip(&m).map(|(a, b)| a.union(b).cloned().collect()).collect();
            let mut by_packet: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
            for (ru, ps) in &gd.rulings {
                for pk in ps {
                    by_packet.entry(pk.as_str()).or_default().insert(ru.as_str());
                }
            }
            let rul: Vec<BTreeSet<String>> = i
                .iter()
                .map(|s| s.iter().flat_map(|x| by_packet.get(x.as_str()).into_iter().flatten().map(|r| r.to_string())).collect())
                .collect();
            // V4p: the plan docs' rulings only (decision sections, Q-citations, the T1-T13 rows), no corrections.
            let plan_only = |set: &BTreeSet<String>| -> BTreeSet<String> {
                set.iter().filter(|r| !r.contains("corrections.json#")).cloned().collect()
            };
            let rul_p: Vec<BTreeSet<String>> = rul.iter().map(plan_only).collect();
            // control: the ruling -> packet map under a random permutation of the defined packets
            let defined: Vec<&str> = gd.packets.keys().map(String::as_str).collect();
            let mut perm: Vec<usize> = (0..defined.len()).collect();
            let mut rng = Rng(h64(&format!("{sha}:V4perm")));
            for k in (1..perm.len()).rev() {
                let j = (rng.next() % (k as u64 + 1)) as usize;
                perm.swap(k, j);
            }
            let pos: HashMap<&str, usize> = defined.iter().enumerate().map(|(k, d)| (*d, k)).collect();
            let rul_perm: Vec<BTreeSet<String>> = i
                .iter()
                .map(|s| {
                    s.iter()
                        .filter_map(|x| pos.get(x.as_str()).map(|&k| defined[perm[k]]))
                        .flat_map(|x| by_packet.get(x).into_iter().flatten().map(|r| r.to_string()))
                        .collect()
                })
                .collect();
            let rul_p_perm: Vec<BTreeSet<String>> = rul_perm.iter().map(plan_only).collect();
            // V14: the cited packets AND their rulings (packet ids and ruling keys are disjoint names).
            let union = |a: &[BTreeSet<String>], b: &[BTreeSet<String>]| -> Vec<BTreeSet<String>> {
                a.iter().zip(b).map(|(x, y)| x.union(y).cloned().collect()).collect()
            };
            let mut sigs = vec![
                ("V1".to_string(), Sig::build(&i)),
                ("V2fam".to_string(), Sig::build(&fam)),
                ("V2bat".to_string(), Sig::build(&bat)),
                ("V2wav".to_string(), Sig::build(&wav)),
                ("V2m".to_string(), Sig::build(&m)),
                ("V1m".to_string(), Sig::build(&r)),
                ("V4".to_string(), Sig::build(&rul)),
                ("V4p".to_string(), Sig::build(&rul_p)),
                ("V14".to_string(), Sig::build(&union(&i, &rul))),
                ("V2mP".to_string(), Sig::build(&m_pend)),
                ("V2mD".to_string(), Sig::build(&m_done)),
            ];
            let mut ctrls = vec![("V0".to_string(), sigs[0].1.shuffled(&format!("{sha}:V1:uniform"), false))];
            for (n, s) in &sigs {
                controls(n, s, sha, &mut ctrls);
            }
            ctrls.push(("V4perm".into(), Sig::build(&rul_perm)));
            ctrls.push(("V4pperm".into(), Sig::build(&rul_p_perm)));
            ctrls.push(("V14perm".into(), Sig::build(&union(&i, &rul_perm))));
            let msg_toks: BTreeSet<String> =
                dot_ids(msg).into_iter().filter(|(pfx, _)| gd.prefixes.contains(pfx)).map(|(_, id)| id).collect();
            let (vm, vm_cov) = msg_route(&sigs[5].1, &msg_toks, nf);
            info.insert("code_files_citing".into(), i.iter().filter(|s| !s.is_empty()).count() as f64);
            info.insert("packets_defined".into(), gd.packets.len() as f64);
            info.insert("rulings".into(), gd.rulings.len() as f64);
            info.insert("rulings_packets".into(), gd.rulings.values().map(BTreeSet::len).sum::<usize>() as f64);
            info.insert("packets_with_wave".into(), gd.wave.len() as f64);
            let mut vocab: Vec<String> = gd.prefixes.iter().cloned().collect();
            let mut rk: Vec<String> = gd.rulings.iter().map(|(k, v)| format!("{k} -> {}", v.len())).collect();
            rk.truncate(400);
            vocab.push(format!("rulings: {}", rk.join("; ")));
            let mut kaudit = Vec::new();
            if let Some(ke) = &env.keys {
                let code_lines = git_grep(env.git, &p, Some("[A-Z][A-Z0-9]{0,4}[.][0-9]|Q[0-9]"), &[]);
                let bm = ke.blobs.index(&p, nf);
                let gm = glia_model(env.git, &p, &env.cache, &ke.pkc, &code_lines, &ke.kp, &env.pr, Some(&bm), nf, sha == ke.last_sha);
                for (k, v) in &gm.stats {
                    *info.entry(k.clone()).or_default() += v;
                }
                key_sigs_glia(Some(&gm), nf, sha, &mut sigs, &mut ctrls);
                kaudit = gm.audit;
            }
            let gains = if env.keys.is_some() { key_gain_pairs(env.mode) } else { Vec::new() };
            VLayer { sigs, ctrls, vm: Some(vm), vm_cov, info, vocab, kaudit, gains }
        }
        VMode::Quokka => {
            let lines = git_grep(env.git, &p, None, &["features", "docs", ".ai", "*.md", ":!**/node_modules/**"]);
            let mut m = vec![BTreeSet::new(); nf];
            let mut units: BTreeSet<String> = BTreeSet::new();
            for (path, l) in &lines {
                let segs: Vec<&str> = path.split('/').collect();
                let unit = if segs.len() >= 3 && segs[0] == "features" { format!("features/{}", segs[1]) } else { path.clone() };
                for f in env.pr.files(l) {
                    m[f as usize].insert(unit.clone());
                    units.insert(unit.clone());
                }
            }
            let sigs = vec![("V2m".to_string(), Sig::build(&m))];
            let mut ctrls = Vec::new();
            for (n, s) in &sigs {
                controls(n, s, sha, &mut ctrls);
            }
            info.insert("doc_units_naming_files".into(), units.len() as f64);
            VLayer { sigs, ctrls, vm: None, vm_cov: false, info, vocab: units.into_iter().collect(), kaudit: Vec::new(), gains: Vec::new() }
        }
    }
}

fn vertical_names(mode: VMode) -> &'static [&'static str] {
    match mode {
        VMode::Kina => &["V1", "V2", "V3", "V3d", "V2m", "V1m", "V2w", "V3w", "V2mw", "V2mP", "V2mD"],
        VMode::Glia => &["V1", "V2fam", "V2bat", "V2wav", "V2m", "V1m", "V4", "V4p", "V14", "V2mP", "V2mD"],
        VMode::Quokka => &["V2m"],
    }
}

fn build_layers(mode: VMode, args: &Args, paths: &[String], pidx: &HashMap<&str, u32>, tests: &[(String, String)]) -> Vec<VLayer> {
    let mut wiki_revs = Vec::new();
    let mut wiki_head_docs = Vec::new();
    if mode == VMode::Kina {
        assert!(!args.wiki.is_empty(), "--vertical kina needs --wiki");
        let text = git_text(&args.wiki, &["log", "--format=%H%x1f%ct", "HEAD"]).expect("wiki log");
        for l in text.lines() {
            if let Some((s, ct)) = l.split_once('\x1f') {
                wiki_revs.push((ct.trim().parse().unwrap_or(0), s.to_string()));
            }
        }
        wiki_revs.sort();
        let head = git_text(&args.wiki, &["rev-parse", "HEAD"]).expect("wiki head").trim().to_string();
        wiki_head_docs = git_grep(&args.wiki, &head, None, &["*.md"]);
        eprintln!("[rank_eval] vertical: wiki {} commits, HEAD {head}, {} doc lines", wiki_revs.len(), wiki_head_docs.len());
    }
    let parents = git_parents(&args.git, &args.rev);
    let keys = (!args.keys.is_empty() && mode != VMode::Quokka).then(|| {
        let t = Instant::now();
        let renames = git_renames(&args.git, &args.rev);
        let kp = KPaths::new(paths, &renames);
        let known = (mode == VMode::Kina).then(|| law_known(args));
        let revs: Vec<String> =
            tests.iter().filter_map(|(s, _)| parents.get(s).and_then(|p| p.0.clone())).collect::<BTreeSet<_>>().into_iter().collect();
        let blobs = KBlobs::load(&args.git, &revs, &kp);
        eprintln!(
            "[keys] {} renames ({} historical paths now candidates), {} revisions, {} blobs read in {:.1}s",
            kp.renames,
            kp.hist.len(),
            revs.len(),
            blobs.tf.len(),
            t.elapsed().as_secs_f64()
        );
        KEnv { kp, known, blobs, last_sha: tests.last().map(|x| x.0.clone()).unwrap_or_default(), pkc: std::sync::Mutex::new(HashMap::new()) }
    });
    let env = VEnv {
        mode,
        git: &args.git,
        wiki: &args.wiki,
        paths,
        pidx,
        pr: PathRes::new(paths),
        parents,
        wiki_revs,
        wiki_head_docs,
        wiki_lines: std::sync::Mutex::new(HashMap::new()),
        cache: BlobCache { text: std::sync::Mutex::new(HashMap::new()), packets: std::sync::Mutex::new(HashMap::new()) },
        keys,
        keys_out: args.keys_out.clone(),
    };
    tests.par_iter().map(|(sha, msg)| build_layer(&env, sha, msg)).collect()
}

// ---- rankers per query ----

/// HT grid: (a T1, b V, c P1).
const HT_GRID: [(&str, f64, f64, f64); 4] =
    [("a", 0.5, 0.25, 0.25), ("b", 0.25, 0.5, 0.25), ("c", 0.25, 0.25, 0.5), ("d", 1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0)];

fn lex2(a: &[f64], b: &[f64]) -> Vec<Key> {
    a.iter().zip(b).map(|(&x, &y)| [x, y, 0.0]).collect()
}

fn vertical_rankers(out: &mut Out, vl: &VLayer, seed: usize, t: &[u32], p1: &[f64], t1: &[f64], direct: &HashSet<(u32, u32)>) {
    let nf = p1.len();
    let mm_p1 = minmax(p1, seed);
    let mm_t1 = minmax(t1, seed);
    let h9 = blend(0.5, &mm_t1, &mm_p1);
    let mm_h9 = minmax(&h9, seed);
    for (name, sig) in &vl.ctrls {
        out.push(name.clone(), &scalar(&sig.score(seed)), seed, t);
    }
    let is_t: Vec<bool> = {
        let mut v = vec![false; nf];
        for &x in t {
            v[x as usize] = true;
        }
        v
    };
    for (name, sig) in &vl.sigs {
        let v = if is_trank(name) { sig.score_trank(seed) } else { sig.score(seed) };
        out.push(name.clone(), &scalar(&v), seed, t);
        let mm_v = minmax(&v, seed);
        // an ablated keyed signal (`VR1h-path`, `VR1h+text`) gets the two fixed blends only
        let lite = name.contains('-') || name.contains('+');
        if lite {
            out.push(format!("HP25[{name}]"), &scalar(&blend(0.25, &mm_v, &mm_p1)), seed, t);
            out.push(format!("HR25[{name}]"), &scalar(&blend(0.25, &mm_v, &mm_h9)), seed, t);
        }
        for a in [0.25, 0.5].into_iter().filter(|_| !lite) {
            out.push(format!("HP{}[{name}]", (a * 100.0) as u32), &scalar(&blend(a, &mm_v, &mm_p1)), seed, t);
        }
        if !lite {
            out.push(format!("HPlex[{name}]"), &lex2(&v, p1), seed, t);
            for b in [0.25, 0.5] {
                out.push(format!("HR{}[{name}]", (b * 100.0) as u32), &scalar(&blend(b, &mm_v, &mm_h9)), seed, t);
            }
            out.push(format!("HRlex[{name}]"), &lex2(&v, &h9), seed, t);
        }
        for (tag, a, b, c) in HT_GRID.into_iter().filter(|_| !lite) {
            let s: Vec<f64> = (0..nf).map(|f| a * mm_t1[f] + b * mm_v[f] + c * mm_p1[f]).collect();
            out.push(format!("HT{tag}[{name}]"), &scalar(&s), seed, t);
        }
        let mut d = [0.0; 6];
        for f in 0..nf {
            if f == seed {
                continue;
            }
            let linked = v[f] > 0.0;
            let key = ((seed as u32).min(f as u32), (seed as u32).max(f as u32));
            if is_t[f] {
                d[1] += 1.0;
                if linked {
                    d[0] += 1.0;
                }
                if !direct.contains(&key) {
                    d[3] += 1.0;
                    if linked {
                        d[2] += 1.0;
                    }
                }
            } else {
                d[5] += 1.0;
                if linked {
                    d[4] += 1.0;
                }
            }
        }
        out.vdiag.push((name.clone(), d));
    }
    for (a, b) in &vl.gains {
        let (Some(sa), Some(sb)) = (vl.sigs.iter().find(|x| &x.0 == a), vl.sigs.iter().find(|x| &x.0 == b)) else { continue };
        let (va, vb) = (sa.1.score(seed), sb.1.score(seed));
        let mut g = [0.0; 5];
        for &x in t {
            let (la, lb) = (va[x as usize] > 0.0, vb[x as usize] > 0.0);
            g[0] += f64::from(u8::from(la));
            g[1] += f64::from(u8::from(lb));
            g[2] += f64::from(u8::from(la && !lb));
            g[3] += f64::from(u8::from(lb && !la));
            g[4] += 1.0;
        }
        out.vgain.push((format!("{a}|{b}"), g));
    }
    if let Some(vm) = &vl.vm {
        out.push("VM", &scalar(vm), seed, t);
        let mm_vm = minmax(vm, seed);
        for b in [0.25, 0.5] {
            out.push(format!("HM{}", (b * 100.0) as u32), &scalar(&blend(b, &mm_vm, &mm_h9)), seed, t);
        }
        out.push("HMlex", &lex2(vm, &h9), seed, t);
    }
}

// ---- the vertical report ----

fn paired_sub(a: &[f64], b: &[f64], idx: &[usize], commit_of: &[usize], boot: usize, lf: &[f64]) -> Option<Paired> {
    if idx.is_empty() {
        return None;
    }
    let mut map: HashMap<usize, usize> = HashMap::new();
    let mut co = Vec::with_capacity(idx.len());
    let (mut aa, mut bb) = (Vec::with_capacity(idx.len()), Vec::with_capacity(idx.len()));
    for &q in idx {
        let n = map.len();
        co.push(*map.entry(commit_of[q]).or_insert(n));
        aa.push(a[q]);
        bb.push(b[q]);
    }
    Some(paired(&aa, &bb, &co, map.len(), boot, lf))
}

/// The signal whose covered subset a vertical ranker is judged on.
fn family_of(name: &str) -> String {
    if name == "V0" {
        return "V1".into();
    }
    if let Some(x) = name.strip_suffix("perm") {
        return x.into();
    }
    if name == "VM" || name.starts_with("HM") {
        return "VM".into();
    }
    if let Some(i) = name.find('[') {
        return name[i + 1..name.len() - 1].to_string();
    }
    name.to_string()
}

/// The paired-bootstrap inputs: each query's test commit, the resample count, ln n! table.
struct BootIn<'a> {
    commit_of: &'a [usize],
    boot: usize,
    lf: &'a [f64],
}

fn vertical_report(
    label: &str,
    mode: VMode,
    names: &[String],
    results: &[Out],
    queries: &[Query],
    layers: &[VLayer],
    bi: &BootIn,
) -> (serde_json::Value, String) {
    let (commit_of, boot, lf) = (bi.commit_of, bi.boot, bi.lf);
    let nq = queries.len();
    let pos = |n: &str| names.iter().position(|x| x == n);
    let col = |ri: usize, mi: usize| -> Vec<f64> { results.iter().map(|r| r.m[ri][mi]).collect() };
    // every layer carries the same signals in the same order (the keyed run appends its own)
    let sig_names: Vec<String> = match layers.first() {
        Some(l) => l.sigs.iter().map(|(n, _)| n.clone()).collect(),
        None => vertical_names(mode).iter().map(|s| s.to_string()).collect(),
    };
    // coverage per family
    let mut cov: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (k, n) in sig_names.iter().enumerate() {
        let idx: Vec<usize> =
            (0..nq).filter(|&q| !layers[queries[q].tc].sigs[k].1.sets[queries[q].seed as usize].is_empty()).collect();
        cov.insert(n.clone(), idx);
    }
    if mode != VMode::Quokka {
        cov.insert("VM".into(), (0..nq).filter(|&q| layers[queries[q].tc].vm_cov).collect());
    }
    let all: Vec<usize> = (0..nq).collect();
    let baselines = ["B0", "P1", "P1sum", "C0", "H10", "T1", "H9a25[P1]", "H9a50[P1]"];
    let vnames: Vec<String> = names
        .iter()
        .filter(|n| {
            n.starts_with('V') || n.starts_with("HP") || n.starts_with("HR") || n.starts_with("HT") || n.starts_with("HM")
        })
        .cloned()
        .collect();
    let mean_on = |ri: usize, idx: &[usize]| -> [f64; 5] {
        let mut m = [0.0; 5];
        for &q in idx {
            for (acc, x) in m.iter_mut().zip(results[q].m[ri]) {
                *acc += x;
            }
        }
        m.map(|x| if idx.is_empty() { f64::NAN } else { x / idx.len() as f64 })
    };
    let fmt5 = |m: [f64; 5]| m.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(" | ");
    let mut md = String::new();
    let mut js = serde_json::Map::new();
    md.push_str(&format!("### {label}: vertical layers\n\n"));

    // ---- coverage and pair links ----
    md.push_str("Coverage and pair links. A query is covered when its seed links to at least one upper-layer node at the parent revision. Pair columns: share of (seed, co-changed file) pairs that share a node; the same for pairs with no direct code edge; for (seed, other candidate) pairs; lift = first / third.\n\n");
    md.push_str("| signal | covered queries | share | test commits with a covered seed | P(link \\| co-change) | P(link \\| co-change, no edge) | P(link \\| other) | lift | co-change pairs with no edge but a link (share of all co-change pairs) |\n|---|---|---|---|---|---|---|---|---|\n");
    let mut cov_js = serde_json::Map::new();
    for n in &sig_names {
        let idx = &cov[n];
        let commits: BTreeSet<usize> = idx.iter().map(|&q| queries[q].tc).collect();
        let mut d = [0.0; 6];
        for r in results {
            if let Some((_, x)) = r.vdiag.iter().find(|(m, _)| m == n) {
                for i in 0..6 {
                    d[i] += x[i];
                }
            }
        }
        let p_co = d[0] / d[1].max(1.0);
        let p_ne = d[2] / d[3].max(1.0);
        let p_ot = d[4] / d[5].max(1.0);
        let lift = if p_ot > 0.0 { p_co / p_ot } else { f64::NAN };
        md.push_str(&format!(
            "| {n} | {} | {:.3} | {} / {} | {p_co:.3} | {p_ne:.3} | {p_ot:.4} | {lift:.1} | {:.3} |\n",
            idx.len(),
            idx.len() as f64 / nq as f64,
            commits.len(),
            layers.len(),
            d[2] / d[1].max(1.0)
        ));
        cov_js.insert(
            n.clone(),
            json!({"covered_queries": idx.len(), "share": idx.len() as f64 / nq as f64, "commits": commits.len(),
                "p_link_cochange": p_co, "p_link_cochange_no_edge": p_ne, "p_link_other": p_ot, "lift": lift,
                "cochange_pairs": d[1], "cochange_pairs_no_edge": d[3]}),
        );
    }
    if let Some(idx) = cov.get("VM") {
        let commits: BTreeSet<usize> = idx.iter().map(|&q| queries[q].tc).collect();
        let cm = layers.iter().filter(|l| l.vm_cov).count();
        md.push_str(&format!(
            "| VM (message ids) | {} | {:.3} | {} / {} (messages citing a known id: {cm}) | - | - | - | - | - |\n",
            idx.len(),
            idx.len() as f64 / nq as f64,
            commits.len(),
            layers.len()
        ));
        cov_js.insert("VM".into(), json!({"covered_queries": idx.len(), "commits": commits.len(), "messages_citing": cm}));
    }
    let direct_pairs: f64 = results.iter().filter_map(|r| r.vdiag.first()).map(|(_, x)| x[1] - x[3]).sum();
    let all_pairs: f64 = results.iter().filter_map(|r| r.vdiag.first()).map(|(_, x)| x[1]).sum();
    md.push_str(&format!(
        "\n(seed, co-changed file) pairs: {all_pairs}; joined by a direct code edge: {direct_pairs} ({:.3}).\n\n",
        direct_pairs / all_pairs.max(1.0)
    ));
    js.insert("coverage".into(), serde_json::Value::Object(cov_js));

    // ---- fourth run: co-change pairs that gain a resolved upper link ----
    let mut gsum: BTreeMap<String, [f64; 5]> = BTreeMap::new();
    for r in results {
        for (k, g) in &r.vgain {
            let e = gsum.entry(k.clone()).or_default();
            for i in 0..5 {
                e[i] += g[i];
            }
        }
    }
    if !gsum.is_empty() {
        md.push_str("Co-change pairs (seed, co-changed file) linked by a resolved signal vs the earlier raw one, summed over queries:\n\n| resolved | raw | pairs | linked resolved | linked raw | resolved only (gained) | raw only (lost) |\n|---|---|---|---|---|---|---|\n");
        let mut gj = serde_json::Map::new();
        for (k, g) in &gsum {
            let (a, b) = k.split_once('|').unwrap_or((k, ""));
            md.push_str(&format!("| {a} | {b} | {} | {} ({:.3}) | {} ({:.3}) | {} | {} |\n", g[4], g[0], g[0] / g[4].max(1.0), g[1], g[1] / g[4].max(1.0), g[2], g[3]));
            gj.insert(k.clone(), json!({"pairs": g[4], "linked_resolved": g[0], "linked_raw": g[1], "gained": g[2], "lost": g[3]}));
        }
        md.push('\n');
        js.insert("gains".into(), serde_json::Value::Object(gj));
    }

    // ---- layer facts ----
    let mut info: BTreeMap<String, f64> = BTreeMap::new();
    for l in layers {
        for (k, v) in &l.info {
            *info.entry(k.clone()).or_default() += v;
        }
    }
    let nl = layers.len().max(1) as f64;
    md.push_str("Layer facts (summed over test commits; `resolve_*` count id citations by how they were scoped; per-commit means in brackets):\n\n");
    for (k, v) in &info {
        md.push_str(&format!("- {k}: {v} [{:.1}]\n", v / nl));
    }
    if let Some(l) = layers.last() {
        md.push_str(&format!("\nVocabulary at the last test commit's parent: {}\n\n", l.vocab.join("; ")));
    }
    js.insert("info".into(), json!(info));
    js.insert("vocab_last".into(), json!(layers.last().map(|l| l.vocab.clone()).unwrap_or_default()));

    // ---- metric tables ----
    let mut rows: Vec<String> = baselines.iter().filter(|b| pos(b).is_some()).map(|s| s.to_string()).collect();
    rows.extend(vnames.iter().cloned());
    md.push_str("ALL queries:\n\n| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |\n|---|---|---|---|---|---|\n");
    let mut all_js = serde_json::Map::new();
    for r in &rows {
        let m = mean_on(pos(r).unwrap(), &all);
        md.push_str(&format!("| {r} | {} |\n", fmt5(m)));
        all_js.insert(r.clone(), json!({"r5": m[0], "r10": m[1], "r20": m[2], "mrr": m[3], "ndcg20": m[4]}));
    }
    js.insert("metrics_all".into(), serde_json::Value::Object(all_js));
    let mut covj = serde_json::Map::new();
    for (fam, idx) in &cov {
        md.push_str(&format!(
            "\nCOVERED by {fam} ({} queries, {:.1}%):\n\n| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |\n|---|---|---|---|---|---|\n",
            idx.len(),
            100.0 * idx.len() as f64 / nq as f64
        ));
        let mut fj = serde_json::Map::new();
        let fam_rows: Vec<String> = baselines
            .iter()
            .filter(|b| pos(b).is_some())
            .map(|s| s.to_string())
            .chain(vnames.iter().filter(|n| &family_of(n) == fam || (fam == "V1" && *n == "V0")).cloned())
            .collect();
        for r in &fam_rows {
            let m = mean_on(pos(r).unwrap(), idx);
            md.push_str(&format!("| {r} | {} |\n", fmt5(m)));
            fj.insert(r.clone(), json!({"r5": m[0], "r10": m[1], "r20": m[2], "mrr": m[3], "ndcg20": m[4]}));
        }
        covj.insert(fam.clone(), serde_json::Value::Object(fj));
    }
    js.insert("metrics_covered".into(), serde_json::Value::Object(covj));

    // ---- paired ----
    let mut pj: Vec<serde_json::Value> = Vec::new();
    for (mi, mname) in [(1usize, "recall@10"), (3, "MRR")] {
        for (subset, sname) in [(false, "ALL queries"), (true, "COVERED queries (each row on its own signal's covered subset)")] {
            md.push_str(&format!(
                "\nPaired {mname}, {sname} (ranker minus baseline; mean, 95% commit-cluster bootstrap CI, sign-test p, wins/losses):\n\n| ranker | n | vs P1 | vs H9a50[P1] | vs V0 | vs own control V0df[X] | vs H10 |\n|---|---|---|---|---|---|---|\n"
            ));
            for r in &vnames {
                let fam = family_of(r);
                let idx: &[usize] = if subset { cov.get(&fam).map_or(&[][..], |v| &v[..]) } else { &all };
                let a = col(pos(r).unwrap(), mi);
                let own = format!("V0df[{fam}]");
                let mut cells = Vec::new();
                for bl in ["P1", "H9a50[P1]", "V0", own.as_str(), "H10"] {
                    let Some(bi) = pos(bl) else {
                        cells.push("-".to_string());
                        continue;
                    };
                    if bi == pos(r).unwrap() {
                        cells.push("=".into());
                        continue;
                    }
                    match paired_sub(&a, &col(bi, mi), idx, commit_of, boot, lf) {
                        Some(p) => {
                            cells.push(format!("{:+.3} [{:+.3}, {:+.3}] p={:.1e} {}/{}", p.mean, p.lo, p.hi, p.p, p.wins, p.losses));
                            pj.push(json!({"ranker": r, "vs": bl, "metric": if mi == 1 { "r10" } else { "mrr" },
                                "subset": if subset { fam.as_str() } else { "all" }, "n": idx.len(),
                                "diff": p.mean, "lo": p.lo, "hi": p.hi, "p_sign": p.p, "wins": p.wins, "losses": p.losses}));
                        }
                        None => cells.push("-".into()),
                    }
                }
                md.push_str(&format!("| {r} | {} | {} |\n", idx.len(), cells.join(" | ")));
            }
        }
    }
    js.insert("paired".into(), json!(pj));

    // ---- contrasts: does an upper layer add to the layer below it? ----
    // (a, b, subset family): a minus b on ALL and on the family's covered subset.
    let mut contrasts: Vec<(String, String, String)> = Vec::new();
    let shapes = ["", "HP25", "HP50", "HPlex", "HR25", "HR50", "HTd"];
    let mut layer_pair = |hi: &str, lo: &str, fam: &str| {
        for sh in shapes {
            let (a, b) = if sh.is_empty() { (hi.to_string(), lo.to_string()) } else { (format!("{sh}[{hi}]"), format!("{sh}[{lo}]")) };
            contrasts.push((a, b, fam.to_string()));
        }
    };
    match mode {
        VMode::Kina => {
            layer_pair("V2", "V1", "V1");
            layer_pair("V3", "V2", "V3");
            layer_pair("V3d", "V2", "V3d");
            layer_pair("V1m", "V1", "V1");
            layer_pair("V2m", "V1", "V2m");
            layer_pair("V2w", "V2", "V2");
            layer_pair("V2mP", "V2mD", "V2m");
        }
        VMode::Glia => {
            layer_pair("V4", "V2fam", "V4");
            layer_pair("V4", "V1", "V4");
            layer_pair("V4p", "V2fam", "V4p");
            layer_pair("V14", "V1", "V1");
            layer_pair("V2fam", "V1", "V1");
            layer_pair("V2wav", "V1", "V2wav");
            layer_pair("V2m", "V1", "V1");
            layer_pair("V2mP", "V2mD", "V2m");
            for (a, b, f) in [("V4", "V4perm", "V4"), ("V4p", "V4pperm", "V4p"), ("V14", "V14perm", "V1")] {
                contrasts.push((a.into(), b.into(), f.into()));
            }
        }
        VMode::Quokka => {}
    }
    // fourth run: resolved against raw, against the layer below, against the permuted map,
    // and each ablation against the full keyed signal
    if sig_names.iter().any(|n| n.starts_with("VR")) {
        // A comparison: (higher signal, lower signal, family).
        type Triple = (&'static str, &'static str, &'static str);
        let (pairs, plain, abl_of): (&[Triple], &[Triple], &[&str]) = match mode {
            VMode::Kina => (
                &[
                    ("VR1h", "V1m", "VR1h"),
                    ("VR1", "V1", "VR1"),
                    ("VR2u", "V2", "VR2u"),
                    ("VR2d", "V2", "VR2d"),
                    ("VR3h", "V3", "VR3h"),
                    ("VR3a", "V3", "VR3a"),
                    ("VR3b", "V3", "VR3b"),
                    ("VR2u", "VR1h", "VR2u"),
                    ("VR3h", "VR2u", "VR3h"),
                    ("VR3h", "VR3b", "VR3h"),
                ],
                &[("VR3h", "VR3hperm", "VR3h")],
                &["VR1h", "VR2u", "VR3h"],
            ),
            VMode::Glia => (
                &[
                    ("VR1", "V1m", "VR1"),
                    ("VR1c", "V1", "VR1c"),
                    ("VR4c", "V4", "VR4c"),
                    ("VR4a", "V4", "VR4a"),
                    ("VR14", "V14", "VR14"),
                    ("VR4", "VR1", "VR4"),
                    ("VR4a", "VR1", "VR4a"),
                    ("VR4c", "VR1c", "VR4c"),
                    ("VR14", "VR1", "VR1"),
                ],
                &[("VR4c", "VR4cperm", "VR4c"), ("VR4", "VR4perm", "VR4")],
                &["VR1", "VR4a"],
            ),
            VMode::Quokka => (&[], &[], &[]),
        };
        for (hi, lo, fam) in pairs {
            for sh in shapes {
                let (a, b) = if sh.is_empty() { (hi.to_string(), lo.to_string()) } else { (format!("{sh}[{hi}]"), format!("{sh}[{lo}]")) };
                contrasts.push((a, b, fam.to_string()));
            }
        }
        for (a, b, f) in plain {
            contrasts.push((a.to_string(), b.to_string(), f.to_string()));
        }
        for base in abl_of {
            for n in sig_names.iter().filter(|n| n.starts_with(base) && n.len() > base.len() && matches!(n.as_bytes()[base.len()], b'-' | b'+')) {
                for sh in ["", "HP25", "HR25"] {
                    let (x, y) = if sh.is_empty() { (base.to_string(), n.clone()) } else { (format!("{sh}[{base}]"), format!("{sh}[{n}]")) };
                    contrasts.push((x, y, base.to_string()));
                }
            }
        }
    }
    if !contrasts.is_empty() {
        md.push_str("\nContrasts (a minus b; recall@10 and MRR; 95% commit-cluster bootstrap CI; on ALL queries and on the named covered subset):\n\n| a | b | subset | n | recall@10 ALL | recall@10 COVERED | MRR ALL | MRR COVERED |\n|---|---|---|---|---|---|---|---|\n");
        let mut cj = Vec::new();
        for (a, b, fam) in &contrasts {
            let (Some(ia), Some(ib)) = (pos(a), pos(b)) else { continue };
            let sub: &[usize] = cov.get(fam).map_or(&[][..], |v| &v[..]);
            let mut cells = Vec::new();
            for mi in [1usize, 3] {
                for idx in [&all[..], sub] {
                    match paired_sub(&col(ia, mi), &col(ib, mi), idx, commit_of, boot, lf) {
                        Some(p) => {
                            cells.push(format!("{:+.3} [{:+.3}, {:+.3}]", p.mean, p.lo, p.hi));
                            cj.push(json!({"a": a, "b": b, "metric": if mi == 1 { "r10" } else { "mrr" },
                                "subset": if idx.len() == all.len() { "all" } else { fam.as_str() }, "n": idx.len(),
                                "diff": p.mean, "lo": p.lo, "hi": p.hi, "p_sign": p.p, "wins": p.wins, "losses": p.losses}));
                        }
                        None => cells.push("-".into()),
                    }
                }
            }
            md.push_str(&format!("| {a} | {b} | {fam} | {} | {} |\n", sub.len(), cells.join(" | ")));
        }
        js.insert("contrasts".into(), json!(cj));
    }
    (serde_json::Value::Object(js), md)
}

// ============================================================================
// Third run (2026-10-18): law impact. Does a change in the LAW predict which
// CODE must change, traced down through the goal layer (specs / requirements)?
//
//   1. law diff: two versions of an instrument (pdftotext), split by section number
//      and classified changed / added / repealed / unchanged, whitespace and page
//      furniture normalised; the amending instrument as made is the cross-check;
//   2. law -> goal: every doc unit (heading section) citing a section of that
//      instrument, the instrument resolved from context, and the requirement ids the
//      unit defines;
//   3. goal -> code: the code files citing those ids;
//   all read at the start of the implementing work (the code at --rev, the wiki
//   commit current then); 4-5. scored against the code files the implementing
//   commits changed, beside text (BM25), structure (PPR), random and permuted-map
//   controls; 6. the reverse view: does each implementing commit's code trace up to
//   a changed section? The instruments, their file stems and the regime's vocabulary
//   are the private config's.
// ============================================================================

const LAW_DRAWS: usize = 1000;
/// Words an amending item heading starts with (an inserted section heading starts with anything else).
const LAW_ITEM_KW: &[&str] = &[
    "Section", "Sections", "Subsection", "Subsections", "Paragraph", "Paragraphs", "Subparagraph", "Subparagraphs",
    "After", "Before", "At", "In", "Part", "Parts", "Division", "Divisions", "Subdivision", "Subdivisions", "Chapter",
    "Item", "Items", "Table", "The", "Note", "Notes", "Clause", "Schedule", "Definition", "Heading", "Insert", "Add",
    "Repeal", "Omit", "Further", "Amendments", "Amendment",
];
fn law_pdf(args: &Args, stem: &str) -> String {
    let pdf = format!("{}/{stem}.pdf", args.law_sources);
    let out = format!("{}/{stem}.txt", args.law_cache);
    if !std::path::Path::new(&out).exists() {
        std::fs::create_dir_all(&args.law_cache).expect("law cache dir");
        let st = Command::new("pdftotext").args(["-layout", &pdf, &out]).status().expect("pdftotext");
        assert!(st.success(), "pdftotext failed on {pdf}");
    }
    std::fs::read_to_string(&out).expect("read pdftotext text")
}

fn law_id_tok(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 5
        && t.chars().next().is_some_and(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        && t.chars().all(|c| c.is_ascii_alphanumeric())
}

/// A running page header: `Section 26F`, `Sections 26F and 26G`, `Part 1A Some programs`,
/// `Some programs Part 1A`, `Division 3 ...`, `... Schedule 1`, `Endnotes`.
fn law_running_header(t: &str) -> bool {
    if t.contains('\u{2014}') {
        return false;
    }
    let w: Vec<&str> = t.split_whitespace().collect();
    if w.is_empty() {
        return false;
    }
    if w == ["Endnotes"] {
        return true;
    }
    if matches!(w[0], "Section" | "Sections") {
        return w.len() >= 2 && w.len() <= 6 && w[1..].iter().all(|x| matches!(*x, "to" | "and") || law_id_tok(x));
    }
    const KW: [&str; 5] = ["Part", "Division", "Subdivision", "Schedule", "Chapter"];
    (w.len() >= 2 && KW.contains(&w[0]) && law_id_tok(w[1]))
        || (w.len() >= 3 && KW.contains(&w[w.len() - 2]) && law_id_tok(w[w.len() - 1]))
}

/// The text's lines with page furniture removed: each page's running headers
/// (its top lines up to the body) and the lines `furniture` names.
fn law_lines(text: &str, furniture: fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    for page in text.split('\x0c') {
        let lines: Vec<&str> = page.lines().collect();
        let mut start = 0;
        for (k, l) in lines.iter().enumerate() {
            let t = l.trim();
            if t.is_empty() {
                continue;
            }
            if law_running_header(t) {
                start = k + 1;
            } else {
                break;
            }
        }
        for l in &lines[start..] {
            if !furniture(l.trim()) {
                out.push(l.trim_end().to_string());
            }
        }
    }
    out
}

fn act_furniture(t: &str) -> bool {
    let p = prv();
    if p.furn_prefix.iter().any(|x| t.starts_with(x.as_str())) || p.furn_act_contains.iter().any(|x| t.contains(x.as_str())) {
        return true;
    }
    let w: Vec<&str> = t.split_whitespace().collect();
    let title: Vec<&str> = p.act_title.split_whitespace().collect();
    let num = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit() || matches!(c, 'i' | 'v' | 'x'));
    w.len() == title.len() + 1 && ((num(w[0]) && w[1..] == title[..]) || (num(w[w.len() - 1]) && w[..w.len() - 1] == title[..]))
}

fn amend_furniture(t: &str) -> bool {
    let p = prv();
    p.furn_act_contains.iter().any(|x| t.contains(x.as_str()))
        || any_eq(&p.furn_amend_equals, t)
        || p.furn_amend_all.iter().any(|g| g.iter().all(|x| t.contains(x.as_str())))
}

#[derive(Clone, Default)]
struct LawSec {
    num: String,
    title: String,
    part: String,
    div: String,
    sub: String,
    ctx: String,
    text: String,
}

/// A section heading at the left margin: `26F Reporting entities must ...`.
fn law_sec_heading(l: &str) -> Option<(String, String)> {
    let b = l.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() {
        return None;
    }
    let d = digits_at(b, 0, 4);
    if d > 3 {
        return None;
    }
    let mut e = d;
    while e < b.len() && e - d < 3 && b[e].is_ascii_uppercase() {
        e += 1;
    }
    if e >= b.len() || b[e] != b' ' {
        return None;
    }
    let rest = l[e..].trim();
    // a title starts upper-case, or with a number word (`177 2 or more proceedings may be heard together`)
    let rb = rest.as_bytes();
    let numeric_title = rb.len() > 2 && rb[0].is_ascii_digit() && rb[1] == b' ' && rb[2].is_ascii_lowercase();
    if !rest.chars().next().is_some_and(char::is_uppercase) && !numeric_title {
        return None;
    }
    Some((l[..e].to_string(), rest.to_string()))
}

/// A Part / Division / Subdivision heading at the left margin (`Part 1A—Some programs`).
fn law_container(l: &str) -> Option<(char, String, String)> {
    for (kw, c) in [("Part ", 'P'), ("Division ", 'D'), ("Subdivision ", 'S')] {
        if let Some(r) = l.strip_prefix(kw)
            && let Some((id, title)) = r.split_once('\u{2014}')
            && law_id_tok(id.trim())
        {
            return Some((c, id.trim().to_string(), title.trim().to_string()));
        }
    }
    None
}

fn law_norm(s: &str) -> String {
    let m: String = s
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' => '"',
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2009}' | '\u{202F}' => ' ',
            c => c,
        })
        .collect();
    m.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn law_key(n: &str) -> (u32, String) {
    let d: String = n.chars().take_while(char::is_ascii_digit).collect();
    (d.parse().unwrap_or(0), n[d.len()..].to_string())
}

/// The body's sections (from the long title, whose first words are the private config's
/// `act_body_start`, to the endnotes), each with its Part / Division context and its
/// normalised text; repeated numbers are reported.
fn act_sections(lines: &[String]) -> (Vec<LawSec>, Vec<String>) {
    let start = lines.iter().position(|l| l.starts_with(prv().act_body_start.as_str())).unwrap_or(0);
    // the body ends at the Act's own schedules (clauses, not sections) or the endnotes
    let end = lines
        .iter()
        .enumerate()
        .skip(start)
        .find(|(_, l)| l.trim_end() == "Endnotes" || (l.starts_with("Schedule ") && l.contains('\u{2014}')))
        .map_or(lines.len(), |(k, _)| k);
    let (mut part, mut div, mut sub) = (String::new(), String::new(), String::new());
    let (mut pt, mut dt) = (String::new(), String::new());
    let mut secs: Vec<LawSec> = Vec::new();
    let mut dups = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut cur: Option<LawSec> = None;
    let mut raw: Vec<String> = Vec::new();
    let mut flush = |cur: &mut Option<LawSec>, raw: &mut Vec<String>| {
        if let Some(mut s) = cur.take() {
            s.text = law_norm(&raw.join("\n"));
            raw.clear();
            if seen.insert(s.num.clone()) {
                secs.push(s);
            } else {
                dups.push(s.num.clone());
            }
        }
    };
    for l in &lines[start..end] {
        if let Some((c, id, title)) = law_container(l) {
            flush(&mut cur, &mut raw);
            match c {
                'P' => {
                    part = id;
                    pt = title;
                    div.clear();
                    dt.clear();
                    sub.clear();
                }
                'D' => {
                    div = id;
                    dt = title;
                    sub.clear();
                }
                _ => sub = id,
            }
            continue;
        }
        if let Some((num, title)) = law_sec_heading(l) {
            flush(&mut cur, &mut raw);
            let mut ctx = format!("Part {part} {pt}");
            if !div.is_empty() {
                ctx.push_str(&format!(" / Division {div} {dt}"));
            }
            raw.push(title.clone());
            cur = Some(LawSec { num, title, part: part.clone(), div: div.clone(), sub: sub.clone(), ctx, text: String::new() });
            continue;
        }
        if cur.is_some() {
            raw.push(l.clone());
        }
    }
    flush(&mut cur, &mut raw);
    (secs, dups)
}

fn law_words(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

/// Dice over word multisets.
fn law_dice(a: &str, b: &str) -> f64 {
    let mut ca: HashMap<String, i64> = HashMap::new();
    let wa = law_words(a);
    let wb2 = law_words(b);
    for w in &wa {
        *ca.entry(w.clone()).or_default() += 1;
    }
    let mut inter = 0i64;
    for w in &wb2 {
        if let Some(c) = ca.get_mut(w)
            && *c > 0
        {
            *c -= 1;
            inter += 1;
        }
    }
    let tot = (wa.len() + wb2.len()) as f64;
    if tot == 0.0 { 1.0 } else { 2.0 * inter as f64 / tot }
}

struct LawDiff {
    old: BTreeMap<String, LawSec>,
    new: BTreeMap<String, LawSec>,
    /// Every section number in either version, in section order.
    order: Vec<String>,
    status: BTreeMap<String, &'static str>,
    dice: BTreeMap<String, f64>,
    /// (repealed old number, added new number, Dice) for added sections whose text matches a repealed one.
    moved: Vec<(String, String, f64)>,
    dups_old: Vec<String>,
    dups_new: Vec<String>,
}

fn law_diff(old: Vec<LawSec>, new: Vec<LawSec>, dups_old: Vec<String>, dups_new: Vec<String>) -> LawDiff {
    let old: BTreeMap<String, LawSec> = old.into_iter().map(|s| (s.num.clone(), s)).collect();
    let new: BTreeMap<String, LawSec> = new.into_iter().map(|s| (s.num.clone(), s)).collect();
    let mut order: Vec<String> = old.keys().chain(new.keys()).cloned().collect::<BTreeSet<_>>().into_iter().collect();
    order.sort_by_key(|n| law_key(n));
    let mut status = BTreeMap::new();
    let mut dice = BTreeMap::new();
    for n in &order {
        let st = match (old.get(n), new.get(n)) {
            (Some(a), Some(b)) => {
                let dc = law_dice(&a.text, &b.text);
                dice.insert(n.clone(), dc);
                if a.text == b.text { "unchanged" } else { "changed" }
            }
            (None, Some(_)) => "added",
            (Some(_), None) => "repealed",
            (None, None) => unreachable!(),
        };
        status.insert(n.clone(), st);
    }
    let repealed: Vec<&String> = order.iter().filter(|n| status[*n] == "repealed").collect();
    let mut moved = Vec::new();
    for n in order.iter().filter(|n| status[*n] == "added") {
        let best = repealed
            .iter()
            .map(|r| (r, law_dice(&old[*r].text, &new[n].text)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        if let Some((r, dc)) = best
            && dc >= 0.6
        {
            moved.push(((*r).clone(), n.clone(), dc));
        }
    }
    LawDiff { old, new, order, status, dice, moved, dups_old, dups_new }
}

/// Section numbers from `key`'s order between `a` and `b` inclusive (a range), or just a and b.
fn law_range(order: &[String], a: &str, b: &str) -> Vec<String> {
    let (ka, kb) = (law_key(a), law_key(b));
    if ka > kb {
        return vec![a.to_string(), b.to_string()];
    }
    let v: Vec<String> = order.iter().filter(|n| law_key(n) >= ka && law_key(n) <= kb).cloned().collect();
    if v.is_empty() { vec![a.to_string(), b.to_string()] } else { v }
}

/// Leading `[0-9]{1,3}[A-Z]{0,3}` of a token.
fn law_num_prefix(t: &str) -> Option<String> {
    let b = t.as_bytes();
    if b.is_empty() || !b[0].is_ascii_digit() {
        return None;
    }
    let d = digits_at(b, 0, 4);
    if d > 3 {
        return None;
    }
    let mut e = d;
    while e < b.len() && e - d < 3 && b[e].is_ascii_uppercase() {
        e += 1;
    }
    if e < b.len() && b[e].is_ascii_alphanumeric() {
        return None;
    }
    Some(t[..e].to_string())
}

struct AmendInfo {
    /// section -> how the amending Act names it (item heading or an inserted section)
    named: BTreeMap<String, BTreeSet<String>>,
    items: usize,
    items_target: usize,
    inserted: usize,
    inserted_unmatched: Vec<String>,
}

/// Sections the amending Act names: item headings (`22 Section 15`, `7 Divisions 1 to 5 of Part 2`,
/// `27 Subsection 39(5)`) and the section headings it inserts, within its blocks amending the
/// primary instrument only.
fn amend_parse(lines: &[String], d: &LawDiff) -> AmendInfo {
    let start = lines
        .iter()
        .position(|l| l.starts_with("Schedule 1\u{2014}") && !l.trim_end().ends_with(|c: char| c.is_ascii_digit()))
        .unwrap_or(0);
    let mut info = AmendInfo { named: BTreeMap::new(), items: 0, items_target: 0, inserted: 0, inserted_unmatched: Vec::new() };
    let mut target_act = false;
    let mut sched = String::new();
    let mut k = start;
    let all_secs: Vec<&LawSec> = d.old.values().chain(d.new.values()).collect();
    let is_act_line = |t: &str| -> bool {
        let w: Vec<&str> = t.split_whitespace().collect();
        w.len() >= 3
            && w.len() <= 14
            && w[w.len() - 2] == "Act"
            && w[w.len() - 1].len() == 4
            && w[w.len() - 1].chars().all(|c| c.is_ascii_digit())
            && !t.contains('(')
            && w[..w.len() - 2].iter().all(|x| x.chars().next().is_some_and(char::is_uppercase) || LAW_CONNECT.contains(x))
    };
    while k < lines.len() {
        let l = &lines[k];
        k += 1;
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        if let Some(r) = l.strip_prefix("Schedule ")
            && let Some((id, _)) = r.split_once('\u{2014}')
        {
            sched = id.trim().to_string();
            target_act = false;
            continue;
        }
        if let Some((c, _, title)) = law_container(l) {
            if c == 'P' && ["Transitional", "Application", "Saving"].iter().any(|w| title.contains(w)) {
                target_act = false;
            }
            continue;
        }
        let indent = l.len() - l.trim_start().len();
        // the amended Act's name, possibly wrapped before its "Act 2006"
        let next = lines.get(k).map_or("", |x| x.trim());
        let year_act = next.len() == 8 && next.starts_with("Act ") && next[4..].chars().all(|c| c.is_ascii_digit());
        let joined = if year_act { format!("{t} {next}") } else { t.to_string() };
        if indent <= 12 && is_act_line(&joined) {
            target_act = joined == prv().act_title;
            if year_act {
                k += 1;
            }
            continue;
        }
        // item heading at the margin: "<n> <Section ...>"
        let b = l.as_bytes();
        if !b.is_empty() && b[0].is_ascii_digit() {
            let d0 = digits_at(b, 0, 5);
            let mut e = d0;
            while e < b.len() && e - d0 < 2 && b[e].is_ascii_uppercase() {
                e += 1;
            }
            if e < b.len() && b[e] == b' ' {
                let rest = l[e..].trim().to_string();
                let first = rest.split_whitespace().next().unwrap_or("");
                if LAW_ITEM_KW.contains(&first) {
                    info.items += 1;
                    let mut head = rest.clone();
                    let mut extra = 0;
                    while head.matches('(').count() > head.matches(')').count() && k < lines.len() && extra < 3 {
                        head.push(' ');
                        head.push_str(lines[k].trim());
                        k += 1;
                        extra += 1;
                    }
                    if target_act {
                        info.items_target += 1;
                        let how = format!("Sch {sched} item {}", &l[..e]);
                        for s in amend_targets(&head, &d.order, &all_secs) {
                            info.named.entry(s).or_default().insert(how.clone());
                        }
                    }
                    continue;
                }
                // an inserted (or substituted) section heading
                if target_act && let Some((num, title)) = law_sec_heading(l) {
                    info.inserted += 1;
                    let tw = |s: &str| law_words(s).into_iter().take(2).collect::<Vec<_>>();
                    let matched = [d.new.get(&num), d.old.get(&num)].into_iter().flatten().any(|s| tw(&s.title) == tw(&title));
                    if matched {
                        info.named.entry(num).or_default().insert(format!("Sch {sched} inserts"));
                    } else {
                        info.inserted_unmatched.push(format!("{num} {title}"));
                    }
                }
            }
        }
    }
    info
}

/// The sections an amending item heading names. Positional headings (`After section 5`) name a
/// section only when they insert into it (`After subsection 6(6)`); container headings
/// (`Divisions 1 to 5 of Part 2`) name every section the container holds in either version.
fn amend_targets(h: &str, order: &[String], secs: &[&LawSec]) -> Vec<String> {
    let w: Vec<&str> = h.split_whitespace().collect();
    let mut out = Vec::new();
    if w.is_empty() {
        return out;
    }
    // `After section 5` / `Before section 26` insert a new section (named by its own heading);
    // `At the end of section 76Q` and `In the appropriate position in section 5` amend the section
    let positional = matches!(w[0], "After" | "Before");
    let clean = |x: &str| x.trim_matches(|c: char| c == ',' || c == ';' || c == ':').to_string();
    // container headings
    if !positional && matches!(w[0], "Part" | "Parts" | "Division" | "Divisions" | "Subdivision" | "Subdivisions") {
        let ids_after = |i: usize| -> (Vec<String>, usize) {
            let mut ids = Vec::new();
            let mut j = i;
            let mut range_from: Option<String> = None;
            while j < w.len() {
                let t = clean(w[j]);
                if law_id_tok(&t) {
                    if let Some(a) = range_from.take() {
                        let (ka, kb) = (law_key(&a), law_key(&t));
                        let mut cand: Vec<String> = secs.iter().flat_map(|s| [s.part.clone(), s.div.clone(), s.sub.clone()]).collect();
                        cand.sort();
                        cand.dedup();
                        for c in cand {
                            if law_key(&c) > ka && law_key(&c) < kb {
                                ids.push(c);
                            }
                        }
                    }
                    ids.push(t);
                    j += 1;
                    if j < w.len() && w[j] == "to" {
                        range_from = ids.last().cloned();
                        j += 1;
                        continue;
                    }
                    if j < w.len() && (w[j] == "and" || w[j - 1].ends_with(',')) {
                        if w[j] == "and" {
                            j += 1;
                        }
                        continue;
                    }
                    break;
                }
                break;
            }
            (ids, j)
        };
        let (ids0, mut j) = ids_after(1);
        let (mut parts, mut divs, mut subs): (Vec<String>, Vec<String>, Vec<String>) = (Vec::new(), Vec::new(), Vec::new());
        match w[0] {
            "Part" | "Parts" => parts = ids0,
            "Division" | "Divisions" => divs = ids0,
            _ => subs = ids0,
        }
        while j + 1 < w.len() && w[j] == "of" {
            let (ids, j2) = ids_after(j + 2);
            match w[j + 1] {
                "Division" => divs = ids,
                "Part" => parts = ids,
                _ => {}
            }
            j = j2;
        }
        for s in secs {
            let ok = (parts.is_empty() || parts.contains(&s.part))
                && (divs.is_empty() || divs.contains(&s.div))
                && (subs.is_empty() || subs.contains(&s.sub))
                && !(parts.is_empty() && divs.is_empty() && subs.is_empty());
            if ok && !out.contains(&s.num) {
                out.push(s.num.clone());
            }
        }
        // `Part 2 (heading)` replaces the heading only
        return if h.contains("(heading)") { Vec::new() } else { out };
    }
    let mut i = 0;
    while i < w.len() {
        let kw = clean(w[i]).to_ascii_lowercase();
        if !matches!(
            kw.as_str(),
            "section" | "sections" | "subsection" | "subsections" | "paragraph" | "paragraphs" | "subparagraph" | "subparagraphs"
        ) {
            i += 1;
            continue;
        }
        let whole = kw == "section" || kw == "sections";
        let mut j = i + 1;
        let mut range_from: Option<String> = None;
        let mut got = Vec::new();
        while j < w.len() {
            let Some(n) = law_num_prefix(w[j]) else { break };
            if let Some(a) = range_from.take() {
                got.extend(law_range(order, &a, &n));
            } else {
                got.push(n.clone());
            }
            j += 1;
            if j < w.len() && w[j] == "to" {
                range_from = Some(n);
                j += 1;
                continue;
            }
            if j < w.len() && (w[j] == "and" || w[j] == "or") {
                j += 1;
                continue;
            }
            if w[j - 1].ends_with(',') {
                continue;
            }
            break;
        }
        if !(positional && whole) {
            for g in got {
                if !out.contains(&g) {
                    out.push(g);
                }
            }
        }
        i = j.max(i + 1);
    }
    out
}

// ---- the goal layer: doc units, definition blocks and their Act citations ----

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LInst {
    /// the regime instrument of the home jurisdiction / of the other jurisdiction / of one not yet known
    HomeAct,
    OtherAct,
    ActUnknown,
    /// the rules / the second instrument
    Rules,
    TRules,
    OtherLaw,
    TheAct,
}

struct LMention {
    start: usize,
    end: usize,
    inst: LInst,
}

/// Regime words (the private config's `regime_line_words`): a heading or line using them is about
/// the regime of the primary instrument.
fn law_regime_words(s: &str) -> bool {
    prv().regime_line_words.iter().map(String::as_str).any(|w| {
        // short acronyms only as whole words
        if w.len() <= 5 && w.chars().all(|c| c.is_ascii_uppercase()) {
            let b = s.as_bytes();
            let mut from = 0;
            while let Some(p) = s[from..].find(w) {
                let st = from + p;
                let en = st + w.len();
                // a trailing plural `s` still counts (ABCs)
                let en2 = if en < b.len() && b[en] == b's' { en + 1 } else { en };
                if wb(b, st) && we(b, en2) {
                    return true;
                }
                from = en;
            }
            false
        } else {
            s.contains(w)
        }
    })
}

/// Instrument mentions on a line, with byte spans.
fn law_mentions(line: &str) -> Vec<LMention> {
    // file names (`XX_Some_Rules_2026_..pdf`) read as words; '_' -> ' ' keeps byte offsets
    let spaced = line.replace('_', " ");
    let line = spaced.as_str();
    let mut v: Vec<LMention> = Vec::new();
    let lower = line.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let covered = |v: &Vec<LMention>, s: usize| v.iter().any(|m| s >= m.start && s < m.end);
    let pv = prv();
    for pat in pv.act_names.iter().map(String::as_str) {
        let mut from = 0;
        while let Some(p) = lower[from..].find(pat) {
            let s = from + p;
            let e = s + pat.len();
            from = e;
            if !wb(lb, s) || !we(lb, e) || covered(&v, s) {
                continue;
            }
            let tail = lower[e..].trim_start_matches([' ', '(']);
            let skip = lower[e..].len() - tail.len();
            let home_tail = pv.act_home_tails.iter().find(|x| tail.starts_with(x.as_str()));
            let other_tail = pv.act_other_tails.iter().find(|x| tail.starts_with(x.as_str()));
            let (inst, e2) = if let Some(x) = home_tail {
                (LInst::HomeAct, e + skip + x.len())
            } else if let Some(x) = other_tail {
                (LInst::OtherAct, e + skip + x.len())
            } else {
                (LInst::ActUnknown, e)
            };
            v.push(LMention { start: s, end: e2, inst });
        }
    }
    for (pat, class) in &pv.rules_names {
        let pat = pat.as_str();
        let inst = if class == "TRULES" { LInst::TRules } else { LInst::Rules };
        let mut from = 0;
        while let Some(p) = lower[from..].find(pat) {
            let s = from + p;
            from = s + pat.len();
            if wb(lb, s) && !covered(&v, s) {
                v.push(LMention { start: s, end: s + pat.len(), inst });
            }
        }
    }
    for pat in ["U.S.C.", "Criminal Code"] {
        let mut from = 0;
        while let Some(p) = line[from..].find(pat) {
            let s = from + p;
            from = s + pat.len();
            if !covered(&v, s) {
                v.push(LMention { start: s, end: s + pat.len(), inst: LInst::OtherLaw });
            }
        }
    }
    let b = line.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if !b[i].is_ascii_alphabetic() || !wb(b, i) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < b.len() && b[j].is_ascii_alphanumeric() {
            j += 1;
        }
        let word = &line[i..j];
        if !covered(&v, i) {
            let inst = match word {
                "Rules" => Some(LInst::Rules),
                "Rule" if line[j..].starts_with(' ') && line[j + 1..].starts_with(|c: char| c.is_ascii_digit()) => Some(LInst::Rules),
                "Act" | "Acts" => {
                    let before = line[..i].trim_end();
                    let pw = before.rsplit(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
                    // "the Act", "this Act", and a bare "Act s66A" are the page's Act; "Corporations Act" is another
                    // "the Principal Act" is an amending instrument's name for the Act it amends
                    if pw.chars().next().is_some_and(char::is_uppercase) && !matches!(pw, "The" | "This" | "Principal") {
                        Some(LInst::OtherLaw)
                    } else {
                        Some(LInst::TheAct)
                    }
                }
                w if any_eq(&pv.other_abbr, w) => Some(LInst::OtherLaw),
                _ => None,
            };
            if let Some(inst) = inst {
                v.push(LMention { start: i, end: j, inst });
            }
        }
        i = j;
    }
    v
}

/// (home-jurisdiction marks, other-jurisdiction marks) on a text.
fn law_marks(s: &str) -> (usize, usize) {
    let p = prv();
    let home = p.marks_home.iter().map(|m| s.matches(m.as_str()).count()).sum();
    let other = p.marks_other.iter().map(|m| s.matches(m.as_str()).count()).sum();
    (home, other)
}

/// The instrument class a mention resolves to: the home jurisdiction's label (its regime
/// instrument), the other jurisdiction's, RULES / TRULES (the rules / the second instrument),
/// OTHER, or "" (unresolved).
fn law_class(inst: LInst, line: &str, page_jur: &'static str, page_def: &'static str) -> &'static str {
    match inst {
        LInst::HomeAct => home(),
        LInst::OtherAct => oth(),
        LInst::Rules => "RULES",
        LInst::TRules => "TRULES",
        LInst::OtherLaw => "OTHER",
        LInst::ActUnknown => {
            let (a, p) = law_marks(line);
            if a > p {
                home()
            } else if p > a {
                oth()
            } else if !page_jur.is_empty() {
                page_jur
            } else {
                home()
            }
        }
        LInst::TheAct => {
            if page_def == home() || page_def == oth() {
                page_def
            } else if law_regime_words(line) {
                let (a, p) = law_marks(line);
                if p > a { oth() } else { home() }
            } else {
                ""
            }
        }
    }
}

/// Up to `len` bytes from `i`, ASCII-lowercased (other bytes kept as-is), for prefix tests.
fn law_lower_at(b: &[u8], i: usize, len: usize) -> String {
    b[i.min(b.len())..(i + len).min(b.len())].iter().map(|c| c.to_ascii_lowercase() as char).collect()
}

struct LScan {
    start: usize,
    end: usize,
    secs: Vec<String>,
}

/// Act-section citations on a line: `s.123`, `s123`, `s 26F(4)(d)`, `ss 80-95`, `s.76A/76B`,
/// `section 41`, `sections 26F and 26J`, `s5, s12`. Ranges expand over `order` (every section
/// number either version has). Rules numbers (`4-2`), dotted numbers (`3.2`) and years are not
/// sections.
fn law_cites(line: &str, order: &[String]) -> Vec<LScan> {
    let b = line.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = b[i];
        if !(c == b's' || c == b'S') || !wb(b, i) || (i > 0 && (b[i - 1] == b'\'' || b[i - 1] == b'$')) || line[..i].ends_with('\u{2019}') {
            i += 1;
            continue;
        }
        let rl = law_lower_at(b, i, 14);
        let mut start_num = None;
        for kw in ["subsections", "subsection", "sections", "section", "ss", "s"] {
            if !rl.starts_with(kw) {
                continue;
            }
            if (kw == "s" || kw == "ss") && c != b's' {
                break;
            }
            let mut p = i + kw.len();
            let dot = p < n && b[p] == b'.';
            if dot {
                p += 1;
            }
            let mut sp = 0;
            while p < n && b[p] == b' ' && sp < 2 {
                p += 1;
                sp += 1;
            }
            if kw.len() > 2 && sp == 0 {
                break;
            }
            if p < n && b[p].is_ascii_digit() {
                start_num = Some(p);
            }
            break;
        }
        let Some(mut p) = start_num else {
            i += 1;
            continue;
        };
        let mut secs: Vec<String> = Vec::new();
        let mut range_from: Option<String> = None;
        let mut end = p;
        loop {
            let e = digits_at(b, p, 4);
            if e == p || e - p > 3 {
                break;
            }
            let mut q = e;
            while q < n && q - e < 3 && b[q].is_ascii_uppercase() {
                q += 1;
            }
            if q < n && b[q].is_ascii_alphanumeric() {
                break;
            }
            if q + 1 < n && matches!(b[q], b'-' | b'.' | b':') && b[q + 1].is_ascii_digit() {
                break;
            }
            let num = line[p..q].to_string();
            match range_from.take() {
                Some(a) => secs.extend(law_range(order, &a, &num)),
                None => secs.push(num.clone()),
            }
            let mut r = q;
            while r < n && b[r] == b'(' {
                match line[r..].find(')') {
                    Some(x) if x <= 8 => r += x + 1,
                    _ => break,
                }
            }
            end = r;
            let tail = &line[r..];
            let mut sep = None;
            for (s, rg) in [
                (" \u{2013} ", true),
                (" - ", true),
                (" to ", true),
                (" and ", false),
                (" or ", false),
                (" & ", false),
                (", ", false),
                ("\u{2013}", true),
                ("-", true),
                ("/", false),
                (",", false),
            ] {
                if tail.starts_with(s) {
                    sep = Some((s.len(), rg));
                    break;
                }
            }
            let Some((sl, rg)) = sep else { break };
            let mut p2 = r + sl;
            let t2 = law_lower_at(b, p2, 10);
            for kw in ["sections ", "section ", "ss. ", "ss ", "s. ", "s.", "s "] {
                if t2.starts_with(kw) {
                    p2 += kw.len();
                    break;
                }
            }
            if p2 + 1 < n && b[p2] == b's' && b[p2 + 1].is_ascii_digit() {
                p2 += 1;
            }
            if !(p2 < n && b[p2].is_ascii_digit()) {
                break;
            }
            if rg {
                range_from = Some(num);
            }
            p = p2;
        }
        if !secs.is_empty() {
            out.push(LScan { start: i, end, secs });
        }
        i = end.max(i + 1);
    }
    out
}

/// The ids a markdown line defines (build_docset's rule), spec-scoped (`specs/<dir>|<id>`) in a spec.
fn law_line_defs(line: &str, scope: Option<&str>) -> Vec<String> {
    let bare: BTreeSet<u8> = bare_kinds();
    let head = line_head(line);
    let mut defined = Vec::new();
    if let Some((p, id)) = hyphen_ids(head).into_iter().next()
        && head.starts_with(&id)
    {
        if !any_eq(&prv().id_standards, &p) {
            defined.push(id);
        }
        for (p2, id2) in hyphen_ids(head).into_iter().skip(1) {
            if head[..head.find([' ', '*', '|', ':']).unwrap_or(head.len())].contains(&id2[p2.len() + 1..]) {
                defined.push(id2);
            }
        }
    } else if let Some((_, id)) = bare_ids(head, &bare).into_iter().next()
        && head.starts_with(&id)
    {
        defined.push(id);
    }
    defined.into_iter().map(|id| match scope { Some(u) => format!("{u}|{id}"), None => id }).collect()
}

struct LBlock {
    unit: usize,
    /// a numbered decision row (`| 14 | ...` under a heading naming decisions): `decision|14`
    drow: Option<String>,
    /// ids the block's definition line defines, plus those its enclosing headings define
    ids: BTreeSet<String>,
    own: Vec<String>,
    mentions: BTreeSet<String>,
    files: BTreeSet<u32>,
    cites: Vec<usize>,
}

struct LUnit {
    path: String,
    name: String,
    ids: BTreeSet<String>,
    drows: BTreeSet<String>,
    mentions: BTreeSet<String>,
    blocks: Vec<usize>,
    cites: Vec<usize>,
}

struct LCite {
    unit: usize,
    block: usize,
    secs: Vec<String>,
    class: &'static str,
    route: &'static str,
    line: String,
}

#[derive(Default)]
struct GoalLayer {
    units: Vec<LUnit>,
    blocks: Vec<LBlock>,
    cites: Vec<LCite>,
    def_blocks: HashMap<String, Vec<usize>>,
    drow_blocks: HashMap<String, Vec<usize>>,
    pages: usize,
    page_defaults: BTreeMap<String, usize>,
}

/// A page's jurisdiction reading: the jurisdiction its marks (and its path) name most, or "";
/// whether it is a regime page (by its path or its title); its lower-cased `# ` title and path.
struct PageJur {
    jur: &'static str,
    regime: bool,
    title: String,
    pl: String,
}

fn page_jur_of(path: &str, lines: &[&str]) -> PageJur {
    let p = prv();
    let (mut hm, mut ot) = (0usize, 0usize);
    for l in lines {
        let (a, b) = law_marks(l);
        hm += a;
        ot += b;
    }
    let pl = path.to_ascii_lowercase();
    if p.path_other.iter().any(|w| pl.contains(w.as_str())) {
        ot += 5;
    }
    if p.path_home.iter().any(|w| pl.contains(w.as_str())) {
        hm += 5;
    }
    let jur: &'static str = if hm >= ot && hm > 0 {
        home()
    } else if ot > hm {
        oth()
    } else {
        ""
    };
    let title = lines.iter().find(|l| l.starts_with("# ")).map_or(String::new(), |l| l.to_lowercase());
    let regime = p.regime_page_path.iter().any(|w| pl.contains(w.as_str()))
        || (!title.contains("excluding") && p.regime_page_title.iter().any(|w| title.contains(w.as_str())));
    PageJur { jur, regime, title, pl }
}

/// Every doc page split into units (heading sections, with their enclosing headings' ids) and
/// definition blocks (a definition line to the next one or the next heading), with the ids each
/// defines and mentions, the candidate files it names, and its Act citations resolved by context:
/// the nearest instrument on the line, else the innermost heading naming one, else the page's
/// dominant instrument ("the Act" counts only on a regime page).
fn goal_layer(docs: &[(String, String)], d: &DocSet, pr: &PathRes, order: &[String]) -> GoalLayer {
    let mut gl = GoalLayer::default();
    let kinds: BTreeSet<u8> = d.vocab_bare.keys().copied().collect();
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for (p, l) in docs {
        if groups.last().is_none_or(|(q, _)| *q != p.as_str()) {
            groups.push((p.as_str(), Vec::new()));
        }
        groups.last_mut().unwrap().1.push(l.as_str());
    }
    gl.pages = groups.len();
    let known: HashSet<&str> = order.iter().map(String::as_str).collect();
    for (path, lines) in &groups {
        let page_first_cite = gl.cites.len();
        let un = kina_unit(path);
        let scope: Option<String> = un.starts_with("specs/").then(|| un.clone());
        let own_spec: Vec<u32> = scope
            .as_ref()
            .and_then(|u| u["specs/".len()..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok())
            .into_iter()
            .collect();
        // The page's own Act: a regime page (by its path or its title) means the regime instrument of
        // its jurisdiction; any other page, the Act its explicit mentions name most (Rules never: their
        // provisions are numbered 4-2, and the second instrument's are named on the line).
        let PageJur { jur: page_jur, regime: regime_page, .. } = page_jur_of(path, lines);
        let mut cnt: BTreeMap<&'static str, usize> = BTreeMap::new();
        for l in lines {
            for m in law_mentions(l) {
                let c = law_class(m.inst, l, page_jur, "");
                if c == home() || c == oth() || c == "OTHER" {
                    *cnt.entry(c).or_default() += 1;
                }
            }
        }
        let mut page_def: &'static str = "";
        if regime_page {
            page_def = if page_jur == oth() { oth() } else { home() };
        } else {
            let mut best = 0;
            for c in [home(), oth(), "OTHER"] {
                let n = cnt.get(c).copied().unwrap_or(0);
                if n > best {
                    best = n;
                    page_def = c;
                }
            }
        }
        *gl.page_defaults.entry(if page_def.is_empty() { "(none)".into() } else { page_def.to_string() }).or_default() += 1;

        let mut stack: Vec<(usize, Vec<String>, String)> = Vec::new();
        let mut fence = false;
        // (line number, class) of the last explicit instrument mention in this unit
        let mut recent: Option<(usize, &'static str)> = None;
        let mut unit = gl.units.len();
        gl.units.push(LUnit {
            path: path.to_string(),
            name: format!("{path} \u{a7} (top)"),
            ids: BTreeSet::new(),
            drows: BTreeSet::new(),
            mentions: BTreeSet::new(),
            blocks: Vec::new(),
            cites: Vec::new(),
        });
        let mut block = gl.blocks.len();
        gl.blocks.push(LBlock {
            unit,
            drow: None,
            ids: BTreeSet::new(),
            own: Vec::new(),
            mentions: BTreeSet::new(),
            files: BTreeSet::new(),
            cites: Vec::new(),
        });
        gl.units[unit].blocks.push(block);
        for (ln, l) in lines.iter().enumerate() {
            let t = l.trim_start();
            if t.starts_with("```") || t.starts_with("~~~") {
                fence = !fence;
            }
            let lvl = if !fence && t.starts_with('#') {
                let h = t.bytes().take_while(|&c| c == b'#').count();
                (h <= 6 && t[h..].starts_with(' ')).then_some(h)
            } else {
                None
            };
            let defs = if fence { Vec::new() } else { law_line_defs(l, scope.as_deref()) };
            if let Some(h) = lvl {
                while stack.last().is_some_and(|s| s.0 >= h) {
                    stack.pop();
                }
                stack.push((h, defs.clone(), l.to_string()));
                recent = None;
                let ids: BTreeSet<String> = stack.iter().flat_map(|s| s.1.iter().cloned()).collect();
                unit = gl.units.len();
                gl.units.push(LUnit {
                    path: path.to_string(),
                    name: format!("{path} \u{a7} {}", t.trim_start_matches('#').trim()),
                    ids: ids.clone(),
                    drows: BTreeSet::new(),
                    mentions: BTreeSet::new(),
                    blocks: Vec::new(),
                    cites: Vec::new(),
                });
                block = gl.blocks.len();
                gl.blocks.push(LBlock { unit, drow: None, ids, own: defs, mentions: BTreeSet::new(), files: BTreeSet::new(), cites: Vec::new() });
                gl.units[unit].blocks.push(block);
            } else if !fence
                && t.starts_with('|')
                && stack.last().is_some_and(|h| h.2.to_lowercase().contains("decision"))
                && let Some(n) = t[1..].split('|').next().map(str::trim).filter(|c| !c.is_empty() && c.len() <= 3 && c.chars().all(|x| x.is_ascii_digit()))
            {
                // a numbered decision row (two pages' numbered decision tables share the numbers)
                let tok = format!("decision|{n}");
                let ids: BTreeSet<String> = stack.iter().flat_map(|s| s.1.iter().cloned()).collect();
                gl.units[unit].drows.insert(tok.clone());
                block = gl.blocks.len();
                gl.blocks.push(LBlock { unit, drow: Some(tok), ids, own: Vec::new(), mentions: BTreeSet::new(), files: BTreeSet::new(), cites: Vec::new() });
                gl.units[unit].blocks.push(block);
            } else if !defs.is_empty() {
                let mut ids: BTreeSet<String> = stack.iter().flat_map(|s| s.1.iter().cloned()).collect();
                ids.extend(defs.iter().cloned());
                gl.units[unit].ids.extend(defs.iter().cloned());
                block = gl.blocks.len();
                gl.blocks.push(LBlock { unit, drow: None, ids, own: defs, mentions: BTreeSet::new(), files: BTreeSet::new(), cites: Vec::new() });
                gl.units[unit].blocks.push(block);
            }
            // ids mentioned (resolved like a code citation, with the page's own spec in scope)
            let ls: Vec<u32> = spec_refs(l).into_iter().chain(own_spec.iter().copied()).collect();
            let mut junk = BTreeMap::new();
            let mut ment: Vec<String> = Vec::new();
            for (pfx, id) in hyphen_ids(l) {
                if d.vocab_hyphen.contains_key(&pfx)
                    && let Some((tk, _)) = kina_resolve(d, &id, false, &ls, &own_spec, &mut junk)
                {
                    ment.push(tk);
                }
            }
            for (_, id) in bare_ids(l, &kinds) {
                if let Some((tk, _)) = kina_resolve(d, &id, true, &ls, &own_spec, &mut junk) {
                    ment.push(tk);
                }
            }
            gl.blocks[block].mentions.extend(ment.iter().cloned());
            gl.units[unit].mentions.extend(ment);
            gl.blocks[block].files.extend(pr.files(l));
            // Act citations
            let ments = law_mentions(l);
            for sc in law_cites(l, order) {
                let mut best: Option<(usize, LInst)> = None;
                // the rules number their provisions 4-2, 8-3: a plain-number citation is never theirs
                for m in ments.iter().filter(|m| m.inst != LInst::Rules) {
                    let dist = if m.end <= sc.start {
                        sc.start - m.end
                    } else if m.start >= sc.end && m.start - sc.end <= 30 {
                        m.start - sc.end + 20
                    } else {
                        continue;
                    };
                    if best.is_none_or(|(bd, _)| dist < bd) {
                        best = Some((dist, m.inst));
                    }
                }
                let (class, route) = match best {
                    Some((_, inst)) => (law_class(inst, l, page_jur, page_def), "line"),
                    None if law_regime_words(l) => {
                        // the other jurisdiction only when the line carries one of its narrow marks
                        let other = prv().other_line_marks.iter().any(|w| l.contains(w.as_str()));
                        (if other { oth() } else { home() }, "line-words")
                    }
                    None => {
                        // the innermost heading naming an instrument, or using regime words
                        let mut hc = None;
                        for (hlv, _, hl) in stack.iter().rev() {
                            if let Some(m) = law_mentions(hl).iter().rfind(|m| m.inst != LInst::TheAct) {
                                hc = Some(law_class(m.inst, hl, page_jur, page_def));
                                break;
                            }
                            if *hlv >= 2 && !hl.contains("excluding") && law_regime_words(hl) {
                                hc = Some(home());
                                break;
                            }
                        }
                        // on a regime page the page's Act outranks a mention a few lines up
                        match (hc, recent) {
                            (Some(c), _) => (c, "heading"),
                            (None, Some((rl, c))) if ln - rl <= 12 && !regime_page => (c, "context"),
                            _ => (page_def, "page"),
                        }
                    }
                };
                let class = if class.is_empty() { "UNRESOLVED" } else { class };
                let ci = gl.cites.len();
                let lt: String = l.trim().chars().take(240).collect();
                gl.cites.push(LCite { unit, block, secs: sc.secs, class, route, line: lt });
                gl.blocks[block].cites.push(ci);
                gl.units[unit].cites.push(ci);
            }
            // (the page validity check follows the line loop)
            // local context carries Act-level mentions only (the Rules are named on their own lines)
            if let Some(m) =
                ments.iter().filter(|m| !matches!(m.inst, LInst::Rules | LInst::TRules | LInst::TheAct)).max_by_key(|m| m.start)
            {
                recent = Some((ln, law_class(m.inst, l, page_jur, page_def)));
            }
        }
        // Page validity check: when most of a page's home-jurisdiction citations resolved from
        // context (not on their own line) are numbers the primary instrument does not have, the page
        // is about another instrument, and those citations are re-read as OTHER.
        let ctx_home: Vec<usize> = (page_first_cite..gl.cites.len()).filter(|&c| gl.cites[c].class == home() && gl.cites[c].route != "line").collect();
        let valid = ctx_home.iter().filter(|&&c| gl.cites[c].secs.iter().all(|x| known.contains(x.as_str()))).count();
        if ctx_home.len() >= 3 && (valid as f64) < 0.5 * ctx_home.len() as f64 {
            for c in ctx_home {
                gl.cites[c].class = "OTHER";
                gl.cites[c].route = "page-check";
            }
        }
    }
    for (bi, b) in gl.blocks.iter().enumerate() {
        for id in &b.ids {
            gl.def_blocks.entry(id.clone()).or_default().push(bi);
        }
        if let Some(d) = &b.drow {
            gl.drow_blocks.entry(d.clone()).or_default().push(bi);
        }
    }
    gl
}

// ---- truth: the implementing commits ----

struct LCommit {
    sha: String,
    date: String,
    subject: String,
    tids: Vec<String>,
    spec: String,
    spec_score: f64,
    /// the task text or the message names a regime word (the private config's)
    regime: bool,
    paths: usize,
    code: Vec<u32>,
    /// code-looking paths created after the start (no candidate can hold them)
    code_new: Vec<String>,
    /// code-looking paths present at the start that the graph holds no MODULE for
    code_noncand: Vec<String>,
}

/// A truth set: (candidate code files changed, commits, code files created after the start).
type LTruth = (HashSet<u32>, usize, BTreeSet<String>);

fn law_is_regime(text: &str) -> bool {
    let p = prv();
    let lower = text.to_lowercase();
    let words: HashSet<&str> = lower.split(|c: char| !c.is_alphanumeric()).collect();
    p.regime_words.iter().any(|w| words.contains(w.as_str())) || p.regime_phrases.iter().any(|x| lower.contains(x.as_str()))
}

/// Whether an implementing commit belongs to the truth named `tname` (the target spec's commits,
/// their regime-work subset, or every commit).
fn law_in_truth(tname: &str, c: &LCommit, tspec: &str) -> bool {
    if tname == prv().truth_spec {
        c.spec == tspec
    } else if tname == prv().truth_regime {
        c.spec == tspec && c.regime
    } else {
        true
    }
}

/// (spec unit, T-id) -> the task item's text (its line and indented continuation), from every
/// spec's tasks.md at the wiki's HEAD (labels the truth commits only; never a predictor).
fn law_tasks(wiki: &str) -> BTreeMap<(String, String), String> {
    let head = git_text(wiki, &["rev-parse", "HEAD"]).expect("wiki head").trim().to_string();
    let lines = git_grep(wiki, &head, None, &["specs/*/tasks.md"]);
    let tk: BTreeSet<u8> = [b'T'].into_iter().collect();
    let mut out: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut cur: Option<(String, String)> = None;
    let mut last = String::new();
    for (p, l) in &lines {
        if *p != last {
            cur = None;
            last = p.clone();
        }
        let t = l.trim_start();
        if t.starts_with('#') {
            cur = None;
            continue;
        }
        let item = t.starts_with("- ") || t.starts_with("* ");
        if item {
            let head = line_head(l);
            if let Some((_, id)) = bare_ids(head, &tk).into_iter().next()
                && head.starts_with(&id)
            {
                let key = (kina_unit(p), id);
                let e = out.entry(key.clone()).or_default();
                e.push_str(l);
                e.push('\n');
                cur = Some(key);
                continue;
            }
            if l.len() == t.len() {
                cur = None;
            }
        }
        if let Some(k) = &cur
            && let Some(e) = out.get_mut(k)
        {
            e.push_str(l);
            e.push('\n');
        }
    }
    out
}

/// Every non-merge commit reachable from the tips but not from the start, oldest first, with
/// its T-ids, the spec whose task text its subject best matches, and its candidate code files.
/// sha -> spec unit for commits reachable only from a `wave/NNN-*` tip (not from the first tip,
/// the mainline): the branch names its spec.
fn law_branch_specs(git: &str, start: &str, tip_names: &[String], tasks: &BTreeMap<(String, String), String>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let specs: BTreeSet<&String> = tasks.keys().map(|k| &k.0).collect();
    for name in tip_names.iter().skip(1) {
        let Some(pos) = name.find("wave/") else { continue };
        let num: String = name[pos + 5..].chars().take_while(char::is_ascii_digit).collect();
        if num.len() != 3 {
            continue;
        }
        let Some(spec) = specs.iter().find(|s| s["specs/".len()..].starts_with(&num)) else { continue };
        let neg_start = format!("^{start}");
        let neg_main = format!("^{}", tip_names[0]);
        let text = git_text(git, &["rev-list", &neg_start, &neg_main, name.as_str()]).unwrap_or_default();
        for sha in text.lines() {
            out.insert(sha.trim().to_string(), (*spec).clone());
        }
    }
    out
}

/// T-id -> the specs whose tasks.md ticked it (`- [ ]` -> `- [x]`) in a wiki commit after the start.
fn law_ticks(wiki: &str, start_ct: i64) -> HashMap<String, BTreeSet<String>> {
    let text = git_text(wiki, &["log", "-p", "--format=%x1e%H%x1f%ct", "--", "specs/*/tasks.md"]).unwrap_or_default();
    let tk: BTreeSet<u8> = [b'T'].into_iter().collect();
    let mut out: HashMap<String, BTreeSet<String>> = HashMap::new();
    for rec in text.split('\x1e').skip(1) {
        let mut lines = rec.lines();
        let head = lines.next().unwrap_or("");
        let ct: i64 = head.split('\x1f').nth(1).and_then(|x| x.trim().parse().ok()).unwrap_or(0);
        if ct <= start_ct {
            continue;
        }
        let mut spec = String::new();
        for l in lines {
            if let Some(r) = l.strip_prefix("+++ b/") {
                spec = kina_unit(r.trim());
            } else if let Some(r) = l.strip_prefix('+')
                && (r.starts_with("- [x] ") || r.starts_with("- [X] "))
                && let Some((_, id)) = bare_ids(&r[6..], &tk).into_iter().next()
                && r[6..].starts_with(&id)
            {
                out.entry(id).or_default().insert(spec.clone());
            }
        }
    }
    out
}

fn law_commits(
    git: &str,
    start: &str,
    tips: &[String],
    pidx: &HashMap<&str, u32>,
    tasks: &BTreeMap<(String, String), String>,
    branch_spec: &HashMap<String, String>,
    ticks: &HashMap<String, BTreeSet<String>>,
) -> Vec<LCommit> {
    let at_start: HashSet<String> =
        git_text(git, &["ls-tree", "-r", "--name-only", start]).unwrap_or_default().lines().map(str::to_string).collect();
    let neg = format!("^{start}");
    let mut a: Vec<&str> = vec!["log", "--no-merges", "--topo-order", "--reverse", "--name-only", "--format=%x1e%H%x1f%ci%x1f%B%x1f", &neg];
    for t in tips {
        a.push(t.as_str());
    }
    let text = git_text(git, &a).expect("git log implementing commits");
    let tk: BTreeSet<u8> = [b'T'].into_iter().collect();
    let mut out = Vec::new();
    for rec in text.split('\x1e').skip(1) {
        let mut parts = rec.splitn(4, '\x1f');
        let sha = parts.next().unwrap_or("").trim().to_string();
        let date = parts.next().unwrap_or("").trim().to_string();
        let msg = parts.next().unwrap_or("").to_string();
        let names = parts.next().unwrap_or("");
        let subject = msg.lines().next().unwrap_or("").to_string();
        // the subject's T-ids (a body often names neighbouring tasks)
        let mut tids: Vec<String> = bare_ids(&subject, &tk).into_iter().map(|x| x.1).collect();
        tids.sort();
        tids.dedup();
        let is_tid = |t: &str| t.len() == 4 && t.starts_with('t') && t[1..].chars().all(|c| c.is_ascii_digit());
        let stoks: BTreeSet<String> = tokens(&subject).into_iter().filter(|t| !is_tid(t)).collect();
        let msg_specs: BTreeSet<u32> = spec_refs(&msg).into_iter().collect();
        let mut scores: BTreeMap<String, f64> = BTreeMap::new();
        for ((spec, tid), txt) in tasks {
            if tids.contains(tid) {
                let tt: BTreeSet<String> = tokens(txt).into_iter().collect();
                let ov = stoks.intersection(&tt).count() as f64 / stoks.len().max(1) as f64;
                *scores.entry(spec.clone()).or_default() += ov;
            }
        }
        for (spec, s) in scores.iter_mut() {
            let num: u32 = spec["specs/".len()..].chars().take_while(char::is_ascii_digit).collect::<String>().parse().unwrap_or(0);
            if msg_specs.contains(&num) {
                *s += 1.0;
            }
        }
        let (mut spec, mut best) = (String::new(), 0.0f64);
        for (s, v) in &scores {
            if *v > best {
                best = *v;
                spec = s.clone();
            }
        }
        // the spec that ticked the subject's tasks after the start, when exactly one did
        let ticked: BTreeSet<&String> = tids.iter().flat_map(|t| ticks.get(t).into_iter().flatten()).collect();
        if ticked.len() == 1 {
            spec = (*ticked.iter().next().unwrap()).clone();
            best = -1.0;
        }
        if let Some(bs) = branch_spec.get(&sha) {
            spec = bs.clone();
            best = f64::INFINITY;
        }
        let task_text: String = tids.iter().filter_map(|t| tasks.get(&(spec.clone(), t.clone()))).cloned().collect::<Vec<_>>().join("\n");
        let regime = law_is_regime(&msg) || law_is_regime(&task_text);
        let mut code = Vec::new();
        let mut code_new = Vec::new();
        let mut code_noncand = Vec::new();
        let mut paths = 0;
        for l in names.lines().map(str::trim).filter(|l| !l.is_empty()) {
            paths += 1;
            match pidx.get(l) {
                Some(&f) => code.push(f),
                None => {
                    let e = ext_of(l);
                    if !e.is_empty() && !NOT_CODE.contains(&e.as_str()) && e != "sql" {
                        if at_start.contains(l) {
                            code_noncand.push(l.to_string());
                        } else {
                            code_new.push(l.to_string());
                        }
                    }
                }
            }
        }
        code.sort_unstable();
        code.dedup();
        out.push(LCommit { sha, date, subject, tids, spec, spec_score: best, regime, paths, code, code_new, code_noncand });
    }
    out
}

// ---- the chain ----

#[derive(Clone, Copy, PartialEq, Debug)]
enum LChain {
    Unit,
    Block,
    Mention,
    UnitNamed,
    UnitHop,
    Decision,
}

const LAW_CHAINS: [(LChain, &str, &str); 6] = [
    (LChain::Unit, "C1", "units citing the section -> the ids they define -> code files citing an id (the specified chain)"),
    (LChain::Block, "C2", "definition blocks citing the section -> their ids -> code files (finer than a unit)"),
    (LChain::Mention, "C1m", "C1, plus every id the citing units mention (looser)"),
    (LChain::UnitNamed, "C3", "C1, plus the files the ids' definition blocks name (doc -> file)"),
    (LChain::UnitHop, "C4", "C3 after one goal-internal hop: blocks that mention a reached id add their ids"),
    (LChain::Decision, "C5", "C3, plus the numbered decision rows whose own row cites the section (`decision|N`, a number the report's decisions and the code check's rows share) and the files every row of that number names"),
];

fn law_chain_ids(kind: LChain, gl: &GoalLayer, units: &BTreeSet<usize>, blocks: &BTreeSet<usize>) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    if kind == LChain::Block {
        for &b in blocks {
            ids.extend(gl.blocks[b].ids.iter().cloned());
        }
    } else {
        for &u in units {
            ids.extend(gl.units[u].ids.iter().cloned());
            if kind == LChain::Mention {
                ids.extend(gl.units[u].mentions.iter().cloned());
            }
        }
    }
    if kind == LChain::Decision {
        // only the decision rows whose own row cites the section
        for &b in blocks {
            if let Some(d) = &gl.blocks[b].drow {
                ids.insert(d.clone());
            }
        }
    }
    if kind == LChain::UnitHop {
        let extra: Vec<String> =
            gl.blocks.iter().filter(|b| b.mentions.iter().any(|m| ids.contains(m))).flat_map(|b| b.ids.iter().cloned()).collect();
        ids.extend(extra);
    }
    ids
}

fn law_chain_files(kind: LChain, gl: &GoalLayer, ids: &BTreeSet<String>, files_of: &HashMap<String, Vec<u32>>) -> BTreeSet<u32> {
    let mut f: BTreeSet<u32> = ids.iter().flat_map(|i| files_of.get(i).into_iter().flatten().copied()).collect();
    if matches!(kind, LChain::UnitNamed | LChain::UnitHop | LChain::Decision) {
        for i in ids {
            for &b in gl.def_blocks.get(i).into_iter().flatten() {
                f.extend(gl.blocks[b].files.iter().copied());
            }
            if kind == LChain::Decision {
                for &b in gl.drow_blocks.get(i).into_iter().flatten() {
                    f.extend(gl.blocks[b].files.iter().copied());
                }
            }
        }
    }
    f
}

fn law_files_of(toks: &[BTreeSet<String>]) -> HashMap<String, Vec<u32>> {
    let mut m: HashMap<String, Vec<u32>> = HashMap::new();
    for (f, ts) in toks.iter().enumerate() {
        for t in ts {
            m.entry(t.clone()).or_default().push(f as u32);
        }
    }
    m
}

/// score(f) = sum over input sections reaching f of idf = ln(1 + files / files the section reaches).
fn law_score(sets: &[&BTreeSet<u32>], nf: usize) -> Vec<f64> {
    let mut s = vec![0.0; nf];
    for fs in sets {
        if fs.is_empty() {
            continue;
        }
        let idf = (1.0 + nf as f64 / fs.len() as f64).ln();
        for &f in fs.iter() {
            s[f as usize] += idf;
        }
    }
    s
}

/// Every file, best first (an exact tie goes to the smaller path).
fn law_rank(score: &[f64]) -> Vec<u32> {
    let mut ix: Vec<u32> = (0..score.len() as u32).collect();
    ix.sort_by(|&a, &b| score[b as usize].total_cmp(&score[a as usize]).then(a.cmp(&b)));
    ix
}

// ---- metrics ----

/// [set size, hits, precision, recall, F1, hits@10, hits@20, recall@10, recall@20]
type LMet = [f64; 9];

fn law_met(set: &[u32], ranked: &[u32], truth: &HashSet<u32>) -> LMet {
    let nt = truth.len().max(1) as f64;
    let hits = set.iter().filter(|f| truth.contains(f)).count() as f64;
    let n = set.len() as f64;
    let p = if n > 0.0 { hits / n } else { 0.0 };
    let r = hits / nt;
    let f1 = if p + r > 0.0 { 2.0 * p * r / (p + r) } else { 0.0 };
    let hk = |k: usize| ranked.iter().take(k).filter(|f| truth.contains(f)).count() as f64;
    [n, hits, p, r, f1, hk(10), hk(20), hk(10) / nt, hk(20) / nt]
}

/// A ranked prediction: the positive-score files are the set.
fn law_met_scores(score: &[f64], truth: &HashSet<u32>) -> LMet {
    let ranked = law_rank(score);
    let set: Vec<u32> = ranked.iter().copied().filter(|&f| score[f as usize] > 0.0).collect();
    law_met(&set, &set, truth)
}

/// A full ranking cut at k.
fn law_met_topk(score: &[f64], k: usize, truth: &HashSet<u32>) -> LMet {
    let ranked = law_rank(score);
    law_met(&ranked[..k.min(ranked.len())], &ranked, truth)
}

struct LDist {
    mean: LMet,
    lo: LMet,
    hi: LMet,
    /// share of draws with F1 at or above the real chain's
    ge: f64,
}

fn law_dist(ms: &[LMet], actual_f1: f64) -> LDist {
    let mut mean = [0.0; 9];
    let mut lo = [0.0; 9];
    let mut hi = [0.0; 9];
    for k in 0..9 {
        let mut v: Vec<f64> = ms.iter().map(|m| m[k]).collect();
        v.sort_by(f64::total_cmp);
        mean[k] = v.iter().sum::<f64>() / v.len().max(1) as f64;
        lo[k] = v[((v.len() as f64) * 0.025) as usize];
        hi[k] = v[(((v.len() as f64) * 0.975) as usize).min(v.len() - 1)];
    }
    let ge = ms.iter().filter(|m| m[4] >= actual_f1 - 1e-12).count() as f64 / ms.len().max(1) as f64;
    LDist { mean, lo, hi, ge }
}

/// F1 of `a` minus F1 of `b` under a commit-cluster bootstrap of the truth (resample the truth
/// commits with replacement; the truth is the union of their files): (mean, 2.5%, 97.5%).
fn law_boot(commits: &[&Vec<u32>], a: &[u32], b: &[u32], key: &str) -> (f64, f64, f64) {
    let mut v: Vec<f64> = (0..LAW_DRAWS)
        .map(|dr| {
            let mut rng = Rng(h64(&format!("{key}:{dr}")));
            let mut t: HashSet<u32> = HashSet::new();
            for _ in 0..commits.len() {
                t.extend(commits[(rng.next() % commits.len() as u64) as usize].iter().copied());
            }
            law_met(a, a, &t)[4] - law_met(b, b, &t)[4]
        })
        .collect();
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    (mean, v[(v.len() as f64 * 0.025) as usize], v[((v.len() as f64 * 0.975) as usize).min(v.len() - 1)])
}

fn law_random(nf: usize, k: usize, truth: &HashSet<u32>, key: &str, f1: f64) -> LDist {
    let ms: Vec<LMet> = (0..LAW_DRAWS)
        .map(|dr| {
            let mut rng = Rng(h64(&format!("{key}:{dr}")));
            let mut ix: Vec<u32> = (0..nf as u32).collect();
            let kk = k.min(nf);
            for i in 0..kk {
                let j = i + (rng.next() % (nf - i) as u64) as usize;
                ix.swap(i, j);
            }
            ix.truncate(kk);
            law_met(&ix, &ix, truth)
        })
        .collect();
    law_dist(&ms, f1)
}

/// The chain with the section -> unit map permuted over every home-cited section: each changed cited
/// section takes the reached files of a random cited section.
fn law_permuted(cited: &[String], changed: &BTreeSet<String>, files: &BTreeMap<String, BTreeSet<u32>>, nf: usize, truth: &HashSet<u32>, key: &str, f1: f64) -> LDist {
    let ms: Vec<LMet> = (0..LAW_DRAWS)
        .map(|dr| {
            let mut rng = Rng(h64(&format!("{key}:{dr}")));
            let mut perm: Vec<usize> = (0..cited.len()).collect();
            for k in (1..perm.len()).rev() {
                let j = (rng.next() % (k as u64 + 1)) as usize;
                perm.swap(k, j);
            }
            let sets: Vec<&BTreeSet<u32>> =
                cited.iter().enumerate().filter(|(_, s)| changed.contains(*s)).map(|(i, _)| &files[&cited[perm[i]]]).collect();
            law_met_scores(&law_score(&sets, nf), truth)
        })
        .collect();
    law_dist(&ms, f1)
}

/// Code tokens: words split at non-alphanumerics and camelCase / PascalCase boundaries, lower-cased,
/// at least 3 characters, no stop words or numbers.
fn law_code_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let push = |w: &str, out: &mut Vec<String>| {
        let l = w.to_lowercase();
        if l.chars().count() >= 3 && !STOP.contains(&l.as_str()) && !l.chars().all(|c| c.is_ascii_digit()) {
            out.push(l);
        }
    };
    for raw in s.split(|c: char| !c.is_alphanumeric()) {
        let mut cur = String::new();
        let mut prev_lower = false;
        for c in raw.chars() {
            if c.is_uppercase() && prev_lower && !cur.is_empty() {
                push(&cur, &mut out);
                cur.clear();
            }
            prev_lower = c.is_lowercase() || c.is_ascii_digit();
            cur.push(c);
        }
        if !cur.is_empty() {
            push(&cur, &mut out);
        }
    }
    out
}

/// PPR (the production P1 config) restarted at the seed files, each file 1/|seeds| spread over its nodes.
fn law_ppr(g: &Graph, ppr: &Ppr, seeds: &[u32]) -> Vec<f64> {
    let sf: Vec<u32> = seeds.iter().copied().filter(|&f| !g.file_nodes[f as usize].is_empty()).collect();
    let mut pers = Vec::new();
    for &f in &sf {
        let ns = &g.file_nodes[f as usize];
        for &u in ns {
            pers.push((g.node_ppr[u as usize], 1.0 / sf.len() as f64 / ns.len() as f64));
        }
    }
    if pers.is_empty() {
        return vec![0.0; g.paths.len()];
    }
    let v = ppr.run(&pers);
    file_pool(g, |u| v[g.node_ppr[u]], false)
}

fn law_fmt(m: &LMet) -> String {
    format!(
        "{:.0} | {:.0} | {:.3} | {:.3} | {:.3} | {:.0} / {:.3} | {:.0} / {:.3}",
        m[0], m[1], m[2], m[3], m[4], m[5], m[7], m[6], m[8]
    )
}

fn law_fmt_dist(d: &LDist) -> String {
    format!(
        "{:.0} | {:.1} | {:.3} [{:.3}, {:.3}] | {:.3} [{:.3}, {:.3}] | {:.3} [{:.3}, {:.3}] | {:.1} / {:.3} | {:.1} / {:.3}",
        d.mean[0], d.mean[1], d.mean[2], d.lo[2], d.hi[2], d.mean[3], d.lo[3], d.hi[3], d.mean[4], d.lo[4], d.hi[4], d.mean[5], d.mean[7], d.mean[6], d.mean[8]
    )
}

fn law_json(m: &LMet) -> serde_json::Value {
    json!({"n": m[0], "hits": m[1], "precision": m[2], "recall": m[3], "f1": m[4], "hits10": m[5], "hits20": m[6], "recall10": m[7], "recall20": m[8]})
}

fn law_json_dist(d: &LDist) -> serde_json::Value {
    json!({"mean": law_json(&d.mean), "lo": law_json(&d.lo), "hi": law_json(&d.hi), "share_f1_ge_real": d.ge})
}

// ---- the run ----

fn law_impact_main(args: &Args) {
    assert_eq!(args.law_impact, "kina", "--law-impact: only `kina` is wired");
    for (v, n) in [(&args.wiki, "--wiki"), (&args.law_sources, "--law-sources"), (&args.law_cache, "--law-cache"), (&args.law_out, "--law-out"), (&args.law_tips, "--law-tips")] {
        assert!(!v.is_empty(), "--law-impact needs {n}");
    }
    std::fs::create_dir_all(&args.law_out).expect("law out dir");
    let outp = |name: &str| format!("{}/{name}", args.law_out);
    let t0 = Instant::now();
    let mut js = serde_json::Map::new();

    // ---- 1. law diff ----
    let pv = prv();
    let (old, dups_old) = act_sections(&law_lines(&law_pdf(args, &pv.pdf_act_old), act_furniture));
    let (new, dups_new) = act_sections(&law_lines(&law_pdf(args, &pv.pdf_act_new), act_furniture));
    let (n_old, n_new) = (old.len(), new.len());
    let diff = law_diff(old, new, dups_old, dups_new);
    let amend = amend_parse(&law_lines(&law_pdf(args, &pv.pdf_amend), amend_furniture), &diff);
    let changed: BTreeSet<String> = diff.order.iter().filter(|n| diff.status[*n] != "unchanged").cloned().collect();
    let count = |st: &str| diff.status.values().filter(|s| **s == st).count();
    let (n_unch, n_ch, n_add, n_rep) = (count("unchanged"), count("changed"), count("added"), count("repealed"));
    let minor = diff.order.iter().filter(|n| diff.status[*n] == "changed" && diff.dice[*n] >= 0.97).count();
    keprintln!(
        "[law] sections: «old» {n_old}, «new» {n_new}; unchanged {n_unch}, changed {n_ch} ({minor} with Dice >= 0.97), added {n_add}, repealed {n_rep}; dup numbers {} / {}",
        diff.dups_old.len(),
        diff.dups_new.len()
    );
    let named = |n: &str| amend.named.contains_key(n);
    let mut xc = [[0usize; 2]; 2];
    for n in &diff.order {
        xc[usize::from(changed.contains(n))][usize::from(named(n))] += 1;
    }
    keprintln!(
        "[law] amending Act: {} items ({} on the «act»), {} inserted headings ({} unmatched); named {} sections; cross-check differ&named {} differ&not {} same&named {} same&not {}",
        amend.items,
        amend.items_target,
        amend.inserted,
        amend.inserted_unmatched.len(),
        amend.named.len(),
        xc[1][1],
        xc[1][0],
        xc[0][1],
        xc[0][0]
    );
    {
        let mut tsv = String::from("section\tstatus\tdice\tnamed_by_amending_act\ttitle_«old»\ttitle_«new»\tcontext\n");
        for n in &diff.order {
            let o = diff.old.get(n);
            let w = diff.new.get(n);
            tsv.push_str(&format!(
                "{n}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                diff.status[n],
                diff.dice.get(n).map_or("-".into(), |x| format!("{x:.4}")),
                amend.named.get(n).map_or(String::new(), |s| s.iter().cloned().collect::<Vec<_>>().join("; ")),
                o.map_or("", |s| s.title.as_str()),
                w.map_or("", |s| s.title.as_str()),
                w.or(o).map_or("", |s| s.ctx.as_str())
            ));
        }
        kwrite(outp("law_sections.tsv"), &tsv).expect("write sections");
    }

    // ---- the code graph at the start ----
    let start = git_text(&args.git, &["rev-parse", &args.rev]).expect("rev-parse start").trim().to_string();
    let res = generate_one(&args.repo).expect("build");
    let merged = &res.merged;
    let all_ids = merged.node_ids();
    let id_to_idx: HashMap<NodeId, usize> = all_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let edges: Vec<&Edge> = merged.all_edges().collect();
    let g = index_graph(merged, &all_ids, &id_to_idx, &edges);
    let nf = g.paths.len();
    let pidx: HashMap<&str, u32> = g.paths.iter().enumerate().map(|(i, p)| (p.as_str(), i as u32)).collect();
    let mut cfg = ref_config(None);
    cfg.direction = Direction::Undirected;
    cfg.top_k = usize::MAX;
    let ppr = Ppr::build(all_ids.len(), &id_to_idx, &edges, &vec![NONE; edges.len()], &cfg);
    eprintln!("[law] graph at {start}: nodes={} edges={} candidate_files={nf}", g.ids.len(), edges.len());

    // ---- 2. the goal layer at the start ----
    let start_ct: i64 = git_text(&args.git, &["log", "-1", "--format=%ct", &start]).expect("start ct").trim().parse().expect("ct");
    let wlog = git_text(&args.wiki, &["log", "--format=%H%x1f%ct", "HEAD"]).expect("wiki log");
    let wrev = wlog
        .lines()
        .filter_map(|l| l.split_once('\x1f'))
        .filter_map(|(s, ct)| ct.trim().parse::<i64>().ok().map(|c| (c, s.to_string())))
        .filter(|(c, _)| *c <= start_ct)
        .max()
        .map(|x| x.1)
        .expect("a wiki commit at or before the start");
    let mut docs = kina_doc_lines(&args.git, &start);
    let n_kina_doc_lines = docs.len();
    docs.extend(git_grep(&args.wiki, &wrev, None, &["*.md"]));
    let pr = PathRes::new(&g.paths);
    let d = build_docset(&docs, &pr);
    let gl = goal_layer(&docs, &d, &pr, &diff.order);
    eprintln!(
        "[law] goal layer at wiki {wrev}: {} doc lines ({n_kina_doc_lines} from the code repo), {} pages, {} units, {} blocks, {} Act-section citations",
        docs.len(),
        gl.pages,
        gl.units.len(),
        gl.blocks.len(),
        gl.cites.len()
    );

    // ---- 3. code citations at the start (and the leaky end) ----
    let mut cstats = BTreeMap::new();
    let code = group_code(git_grep(&args.git, &start, Some(&prv().code_pat), &[]), &pidx);
    let (toks, _, _) = kina_cites(&d, &code, nf, &mut cstats);
    let files_of = law_files_of(&toks);
    let tips: Vec<String> = args
        .law_tips
        .split(',')
        .map(|t| git_text(&args.git, &["rev-parse", t.trim()]).expect("rev-parse tip").trim().to_string())
        .collect();
    let end = if args.law_end.is_empty() {
        tips[0].clone()
    } else {
        git_text(&args.git, &["rev-parse", &args.law_end]).expect("rev-parse end").trim().to_string()
    };
    let code_end = group_code(git_grep(&args.git, &end, Some(&prv().code_pat), &[]), &pidx);
    let (toks_end, _, _) = kina_cites(&d, &code_end, nf, &mut BTreeMap::new());
    let files_of_end = law_files_of(&toks_end);
    let compliance: Vec<u32> = (0..nf as u32).filter(|&f| !toks[f as usize].is_empty()).collect();
    let compliance_end = (0..nf).filter(|&f| !toks_end[f].is_empty()).count();

    // ---- 4. truth ----
    let tasks = law_tasks(&args.wiki);
    let tip_names: Vec<String> = args.law_tips.split(',').map(|t| t.trim().to_string()).collect();
    let branch_spec = law_branch_specs(&args.git, &start, &tip_names, &tasks);
    let ticks = law_ticks(&args.wiki, start_ct);
    let commits = law_commits(&args.git, &start, &tips, &pidx, &tasks, &branch_spec, &ticks);
    let tspec = pv.target_spec.as_str();
    let pick = |f: &dyn Fn(&LCommit) -> bool| -> LTruth {
        let cs: Vec<&LCommit> = commits.iter().filter(|c| f(c)).collect();
        (cs.iter().flat_map(|c| c.code.iter().copied()).collect(), cs.len(), cs.iter().flat_map(|c| c.code_new.iter().cloned()).collect())
    };
    let noncand: BTreeMap<&str, BTreeSet<String>> = [(pv.truth_spec.as_str(), true), ("ALL", false)]
        .into_iter()
        .map(|(n, only)| (n, commits.iter().filter(|c| !only || c.spec == tspec).flat_map(|c| c.code_noncand.iter().cloned()).collect()))
        .collect();
    let truths: Vec<(&str, &str, LTruth)> = vec![
        (pv.truth_spec.as_str(), t("truth_spec_desc"), pick(&|c: &LCommit| c.spec == tspec)),
        (pv.truth_regime.as_str(), t("truth_regime_desc"), pick(&|c: &LCommit| c.spec == tspec && c.regime)),
        ("ALL", t("truth_all_desc"), pick(&|_: &LCommit| true)),
    ];
    for (n, _, (t, c, nw)) in &truths {
        eprintln!("[law] truth {n}: {c} commits, {} candidate code files, {} code files created after the start", t.len(), nw.len());
    }

    // ---- the chain: home-cited sections -> units / blocks -> ids -> files ----
    let mut sec_units: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut sec_blocks: BTreeMap<String, BTreeSet<usize>> = BTreeMap::new();
    let mut sec_ncite: BTreeMap<String, usize> = BTreeMap::new();
    for c in &gl.cites {
        if c.class != home() {
            continue;
        }
        for s in &c.secs {
            sec_units.entry(s.clone()).or_default().insert(c.unit);
            sec_blocks.entry(s.clone()).or_default().insert(c.block);
            *sec_ncite.entry(s.clone()).or_default() += 1;
        }
    }
    let mut cited: Vec<String> = sec_units.keys().filter(|s| diff.status.contains_key(*s)).cloned().collect();
    cited.sort_by_key(|n| law_key(n));
    let cited_unknown: Vec<String> = sec_units.keys().filter(|s| !diff.status.contains_key(*s)).cloned().collect();
    let cited_changed: Vec<String> = cited.iter().filter(|s| changed.contains(*s)).cloned().collect();
    let cited_unchanged: BTreeSet<String> = cited.iter().filter(|s| !changed.contains(*s)).cloned().collect();
    let cited_all: BTreeSet<String> = cited.iter().cloned().collect();
    keprintln!(
        "[law] «home»-cited sections: {} known ({} changed, {} unchanged), {} numbers in neither «ver»",
        cited.len(),
        cited_changed.len(),
        cited_unchanged.len(),
        cited_unknown.len()
    );
    struct KindSets {
        ids: BTreeMap<String, BTreeSet<String>>,
        files: BTreeMap<String, BTreeSet<u32>>,
        files_end: BTreeMap<String, BTreeSet<u32>>,
    }
    let kinds: Vec<KindSets> = LAW_CHAINS
        .iter()
        .map(|(k, _, _)| {
            let mut ks = KindSets { ids: BTreeMap::new(), files: BTreeMap::new(), files_end: BTreeMap::new() };
            for s in &cited {
                let ids = law_chain_ids(*k, &gl, &sec_units[s], &sec_blocks[s]);
                ks.files.insert(s.clone(), law_chain_files(*k, &gl, &ids, &files_of));
                ks.files_end.insert(s.clone(), law_chain_files(*k, &gl, &ids, &files_of_end));
                ks.ids.insert(s.clone(), ids);
            }
            ks
        })
        .collect();
    let chain_score = |ki: usize, inputs: &BTreeSet<String>, end: bool| -> Vec<f64> {
        let ks = &kinds[ki];
        let sets: Vec<&BTreeSet<u32>> =
            cited.iter().filter(|s| inputs.contains(*s)).map(|s| if end { &ks.files_end[s] } else { &ks.files[s] }).collect();
        law_score(&sets, nf)
    };

    // ---- BM25 over the code files' text ----
    let mut bm = Bm25::default();
    for p in &g.paths {
        let txt = std::fs::read(format!("{}/{p}", args.repo)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        bm.add(law_code_tokens(&txt), Vec::new());
    }
    let sec_text = |n: &str| -> String { diff.new.get(n).or_else(|| diff.old.get(n)).map_or(String::new(), |s| s.text.clone()) };
    let q_all: Vec<String> = changed.iter().flat_map(|n| law_code_tokens(&sec_text(n))).collect();
    let q_cited: Vec<String> = cited_changed.iter().flat_map(|n| law_code_tokens(&sec_text(n))).collect();
    let bm_all = bm.scores(&q_all);
    let bm_cited = bm.scores(&q_cited);
    // compliance code ranked by its cited ids' idf
    let mut comp_score = vec![0.0; nf];
    for &f in &compliance {
        for t in &toks[f as usize] {
            comp_score[f as usize] += (1.0 + nf as f64 / files_of[t].len() as f64).ln();
        }
    }
    let ppr_comp = law_ppr(&g, &ppr, &compliance);

    // ---- 5. scores ----
    let mut md = String::new();
    let mut js_scores = serde_json::Map::new();
    let c1 = chain_score(0, &changed, false);
    let c3 = chain_score(3, &changed, false);
    let c1_set: Vec<u32> = law_rank(&c1).into_iter().filter(|&f| c1[f as usize] > 0.0).collect();
    // baselines and controls at the size of these chains: (name, kind index, score, PPR from its files)
    let focus: Vec<(&str, usize, Vec<f64>, Vec<f64>)> = [("C1", 0usize), ("C3", 3), ("C5", 5)]
        .into_iter()
        .map(|(n, ki)| {
            let sc = chain_score(ki, &changed, false);
            let set: Vec<u32> = law_rank(&sc).into_iter().filter(|&f| sc[f as usize] > 0.0).collect();
            let pp = law_ppr(&g, &ppr, &set);
            (n, ki, sc, pp)
        })
        .collect();
    for (tname, tdesc, (truth, ncommits, tnew)) in &truths {
        let mut rows: Vec<(String, String, serde_json::Value)> = Vec::new();
        let row = |name: &str, m: LMet, rows: &mut Vec<(String, String, serde_json::Value)>| {
            rows.push((name.to_string(), law_fmt(&m), law_json(&m)));
        };
        for (ki, (_, cname, _)) in LAW_CHAINS.iter().enumerate() {
            let m = law_met_scores(&chain_score(ki, &changed, false), truth);
            row(&format!("{cname} (changed sections)"), m, &mut rows);
        }

        let mut c1_paths = c1_set.clone();
        c1_paths.sort_unstable();
        row("C1 unranked (same set, path order)", law_met(&c1_paths, &c1_paths, truth), &mut rows);
        row("C1 from UNCHANGED cited sections (control)", law_met_scores(&chain_score(0, &cited_unchanged, false), truth), &mut rows);
        row("C3 from UNCHANGED cited sections (control)", law_met_scores(&chain_score(3, &cited_unchanged, false), truth), &mut rows);
        row("C5 from UNCHANGED cited sections (control)", law_met_scores(&chain_score(5, &cited_unchanged, false), truth), &mut rows);
        row("C1 from ALL cited sections", law_met_scores(&chain_score(0, &cited_all, false), truth), &mut rows);
        row("C5 from ALL cited sections", law_met_scores(&chain_score(5, &cited_all, false), truth), &mut rows);
        row("C1 with END code citations (LEAKY)", law_met_scores(&chain_score(0, &changed, true), truth), &mut rows);
        row("C3 with END code citations (LEAKY)", law_met_scores(&chain_score(3, &changed, true), truth), &mut rows);
        row("compliance code: every file citing an id (set; ranked by id idf)", law_met_scores(&comp_score, truth), &mut rows);
        let mut dists: Vec<(String, LDist)> = Vec::new();
        let tcommits: Vec<&Vec<u32>> = commits
            .iter()
            .filter(|c| law_in_truth(tname, c, tspec))
            .map(|c| &c.code)
            .collect();
        let mut boots: Vec<(String, (f64, f64, f64))> = Vec::new();
        for (fname, ki, sc, pp) in &focus {
            let set: Vec<u32> = law_rank(sc).into_iter().filter(|&f| sc[f as usize] > 0.0).collect();
            let k = set.len();
            let top = |v: &[f64]| -> Vec<u32> { law_rank(v).into_iter().take(k).collect() };
            for (bn, bset) in [
                ("BM25 (all changed text)", top(&bm_all)),
                ("PPR from compliance code", top(&ppr_comp)),
                ("compliance code by id idf", top(&comp_score)),
                (&format!("PPR from {fname}'s files") as &str, top(pp)),
            ] {
                boots.push((format!("{fname} - {bn}, both at {k} files"), law_boot(&tcommits, &set, &bset, &format!("{tname}:{fname}:{bn}"))));
            }
            let _ = ki;
        }
        for (fname, ki, sc, pp) in &focus {
            let m = law_met_scores(sc, truth);
            let k = m[0] as usize;
            row(&format!("BM25 all changed sections' text, top |{fname}|={k}"), law_met_topk(&bm_all, k, truth), &mut rows);
            row(&format!("BM25 cited changed sections' text, top |{fname}|={k}"), law_met_topk(&bm_cited, k, truth), &mut rows);
            row(&format!("PPR from compliance code, top |{fname}|={k}"), law_met_topk(&ppr_comp, k, truth), &mut rows);
            row(&format!("compliance code by id idf, top |{fname}|={k}"), law_met_topk(&comp_score, k, truth), &mut rows);
            row(&format!("PPR from {fname}'s files, top |{fname}|={k}"), law_met_topk(pp, k, truth), &mut rows);
            dists.push((format!("random files, size |{fname}|"), law_random(nf, k, truth, &format!("{tname}:rand:{fname}"), m[4])));
            dists.push((
                format!("{fname} with the section -> unit map permuted"),
                law_permuted(&cited, &changed, &kinds[*ki].files, nf, truth, &format!("{tname}:perm:{fname}"), m[4]),
            ));
        }
        md.push_str(&format!(
            "\n## Truth {tname}: {tdesc}\n\n{ncommits} commits; {} candidate code files changed (the targets); {} code files created after the start (no ranker over the start's files can name them; not scored); {nf} candidate files.\n\n| ranker | set size | hits | precision | recall | F1 | hits@10 / recall@10 | hits@20 / recall@20 |\n|---|---|---|---|---|---|---|---|\n",
            truth.len(),
            tnew.len()
        ));
        let mut jr = serde_json::Map::new();
        for (n, cells, j) in &rows {
            md.push_str(&format!("| {n} | {cells} |\n"));
            jr.insert(n.clone(), j.clone());
        }
        md.push_str("\nControls, mean [95% interval] over 1000 draws (share of draws with F1 >= the real chain's in the last column):\n\n| control | set size | hits | precision | recall | F1 | hits@10 / recall@10 | hits@20 / recall@20 | share F1 >= real |\n|---|---|---|---|---|---|---|---|---|\n");
        for (n, dd) in &dists {
            md.push_str(&format!("| {n} | {} | {:.3} |\n", law_fmt_dist(dd), dd.ge));
            jr.insert(n.to_string(), law_json_dist(dd));
        }
        md.push_str("\nF1 differences, paired commit-cluster bootstrap of the truth (1000 resamples of its commits), mean [95% interval]:\n\n| comparison | F1 difference |\n|---|---|\n");
        for (n, (m, lo, hi)) in &boots {
            md.push_str(&format!("| {n} | {m:+.3} [{lo:+.3}, {hi:+.3}] |\n"));
            jr.insert(format!("boot: {n}"), json!({"mean": m, "lo": lo, "hi": hi}));
        }
        js_scores.insert(tname.to_string(), serde_json::Value::Object(jr));
    }

    // ---- hops ----
    let mut hop = String::new();
    let units_changed: BTreeSet<usize> = cited_changed.iter().flat_map(|s| sec_units[s].iter().copied()).collect();
    let blocks_changed: BTreeSet<usize> = cited_changed.iter().flat_map(|s| sec_blocks[s].iter().copied()).collect();
    let pages_changed: BTreeSet<&str> = units_changed.iter().map(|&u| gl.units[u].path.as_str()).collect();
    hop.push_str(&format!(
        "| hop | count |\n|---|---|\n| Act sections, «old_sp» / «new_sp» | {n_old} / {n_new} |\n| sections changed / added / repealed (the law change) | {n_ch} / {n_add} / {n_rep} = {} |\n| ... of which cited by the goal layer as the «act_full» | {} |\n| cited «home» sections, changed or not | {} (+{} numbers in neither «ver») |\n| units (heading sections) citing a changed section | {} on {} pages |\n| definition blocks citing a changed section | {} |\n",
        changed.len(),
        cited_changed.len(),
        cited.len(),
        cited_unknown.len(),
        units_changed.len(),
        pages_changed.len(),
        blocks_changed.len()
    ));
    let units_with_ids = units_changed.iter().filter(|&&u| !gl.units[u].ids.is_empty()).count();
    let blocks_own = blocks_changed.iter().filter(|&&b| !gl.blocks[b].own.is_empty()).count();
    let blocks_ids = blocks_changed.iter().filter(|&&b| !gl.blocks[b].ids.is_empty()).count();
    hop.push_str(&format!("| ... of those units, defining at least one id | {units_with_ids} |\n| ... of those blocks, with an id (own definition line / incl. enclosing headings) | {blocks_own} / {blocks_ids} |\n"));
    for (ki, (_, cname, _)) in LAW_CHAINS.iter().enumerate() {
        let ks = &kinds[ki];
        let ids: BTreeSet<&String> = cited_changed.iter().flat_map(|s| ks.ids[s].iter()).collect();
        let ids_in_code = ids.iter().filter(|i| files_of.contains_key(i.as_str())).count();
        let ids_in_code_end = ids.iter().filter(|i| files_of_end.contains_key(i.as_str())).count();
        let files: BTreeSet<u32> = cited_changed.iter().flat_map(|s| ks.files[s].iter().copied()).collect();
        let secs_reach = cited_changed.iter().filter(|s| !ks.files[*s].is_empty()).count();
        hop.push_str(&format!(
            "| {cname}: ids reached / cited in code at start (at end, leaky) / files reached / changed sections reaching a file | {} / {ids_in_code} ({ids_in_code_end}) / {} / {secs_reach} |\n",
            ids.len(),
            files.len()
        ));
    }
    hop.push_str(&format!(
        "| code files citing any id at the start (compliance code) / at the end | {} / {compliance_end} |\n| candidate code files | {nf} |\n",
        compliance.len()
    ));
    hop.push_str("\nPer changed section cited by the goal layer (C1 and C3 reach):\n\n| section | status | Dice | «home» cites | units | blocks | C1 ids | C1 ids in code | C1 files | C3 files | C1 files in «ts» truth | title |\n|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    let tspec_truth = &truths[0].2.0;
    for s in &cited_changed {
        let ids = &kinds[0].ids[s];
        let in_code = ids.iter().filter(|i| files_of.contains_key(i.as_str())).count();
        let f1 = &kinds[0].files[s];
        let hit = f1.iter().filter(|f| tspec_truth.contains(f)).count();
        hop.push_str(&format!(
            "| {s} | {} | {} | {} | {} | {} | {} | {in_code} | {} | {} | {hit} | {} |\n",
            diff.status[s],
            diff.dice.get(s).map_or("-".into(), |x| format!("{x:.3}")),
            sec_ncite[s],
            sec_units[s].len(),
            sec_blocks[s].len(),
            ids.len(),
            f1.len(),
            kinds[3].files[s].len(),
            diff.new.get(s).or_else(|| diff.old.get(s)).map_or("", |x| x.title.as_str())
        ));
    }

    {
        let mut det = String::from("section\tstatus\tchain\tids_reached\tids_cited_in_code\tfiles\tfiles_in_«ts»\tfile_paths\n");
        for s in &cited_changed {
            for (ki, (_, cname, _)) in LAW_CHAINS.iter().enumerate() {
                let ks = &kinds[ki];
                let inc: Vec<&String> = ks.ids[s].iter().filter(|i| files_of.contains_key(i.as_str())).collect();
                let fs = &ks.files[s];
                det.push_str(&format!(
                    "{s}\t{}\t{cname}\t{}\t{}\t{}\t{}\t{}\n",
                    diff.status[s],
                    ks.ids[s].len(),
                    inc.iter().map(|x| x.as_str()).collect::<Vec<_>>().join(" "),
                    fs.len(),
                    fs.iter().filter(|f| tspec_truth.contains(f)).count(),
                    fs.iter().map(|&f| g.paths[f as usize].as_str()).collect::<Vec<_>>().join(" ")
                ));
            }
        }
        kwrite(outp("law_chain_detail.tsv"), &det).expect("write chain detail");
    }
    // GLIA_LAW_DEBUG=<section>: each C3 id of that section with the definition blocks that name files (inspection only)
    if let Ok(sec) = std::env::var("GLIA_LAW_DEBUG")
        && let Some(ids) = kinds[3].ids.get(&sec)
    {
        for &u in sec_units.get(&sec).into_iter().flatten() {
            eprintln!("[law-debug] {sec} unit={} ids={:?}", gl.units[u].name, gl.units[u].ids);
        }
        for id in ids {
            for &b in gl.def_blocks.get(id).into_iter().flatten() {
                let bl = &gl.blocks[b];
                if !bl.files.is_empty() {
                    eprintln!(
                        "[law-debug] {sec} id={id} block_own={:?} unit={} files={}",
                        bl.own,
                        gl.units[bl.unit].name,
                        bl.files.iter().map(|&f| g.paths[f as usize].as_str()).collect::<Vec<_>>().join(" ")
                    );
                }
            }
        }
    }
    for (n, v) in &noncand {
        hop.push_str(&format!("\nTruth {n}: {} changed code-looking files existed at the start without a graph MODULE (not candidates): {}\n", v.len(), v.iter().take(12).cloned().collect::<Vec<_>>().join(", ")));
    }

    // ---- citations ----
    let mut cls: BTreeMap<(&str, &str), usize> = BTreeMap::new();
    let mut cit_tsv = String::from("class\troute\tsections\tunit\tline\n");
    for c in &gl.cites {
        *cls.entry((c.class, c.route)).or_default() += 1;
        cit_tsv.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", c.class, c.route, c.secs.join(","), gl.units[c.unit].name, c.line.replace('\t', " ")));
    }
    kwrite(outp("law_citations.tsv"), &cit_tsv).expect("write citations");
    let mut cit_md = String::from("| class | route | citations |\n|---|---|---|\n");
    for ((c, r), n) in &cls {
        cit_md.push_str(&format!("| {c} | {r} | {n} |\n"));
    }
    cit_md.push_str(&format!("\nPage default instruments (pages): {:?}\n", gl.page_defaults));
    cit_md.push_str("\nMost-cited «home» sections (citations, status):\n\n| section | «home» citations | status | units |\n|---|---|---|---|\n");
    let mut by_n: Vec<(&String, &usize)> = sec_ncite.iter().collect();
    by_n.sort_by(|a, b| b.1.cmp(a.1).then(law_key(a.0).cmp(&law_key(b.0))));
    for (s, n) in by_n.iter().take(40) {
        cit_md.push_str(&format!("| {s} | {n} | {} | {} |\n", diff.status.get(*s).copied().unwrap_or("(not a section)"), sec_units[*s].len()));
    }
    cit_md.push_str(&format!("\n«home»-resolved numbers in neither «ver»: {}\n", cited_unknown.join(", ")));
    // a deterministic audit sample: every 25th citation
    cit_md.push_str("\nAudit sample (every 25th citation in doc order):\n\n| class | route | sections | unit | line |\n|---|---|---|---|---|\n");
    for c in gl.cites.iter().step_by(25) {
        let line: String = c.line.chars().take(160).collect::<String>().replace('|', "/");
        cit_md.push_str(&format!("| {} | {} | {} | {} | {line} |\n", c.class, c.route, c.secs.join(","), gl.units[c.unit].name.replace('|', "/")));
    }

    // ---- truth table ----
    let mut tr = String::from("| # | commit | date | T-ids | spec (score) | «tr» | files | candidate code | new code | subject |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for (i, c) in commits.iter().enumerate() {
        tr.push_str(&format!(
            "| {} | {} | {} | {} | {} ({}) | {} | {} | {} | {} | {} |\n",
            i + 1,
            &c.sha[..8],
            &c.date[..10.min(c.date.len())],
            c.tids.join(" "),
            c.spec.trim_start_matches("specs/"),
            if c.spec_score.is_infinite() {
                "branch".to_string()
            } else if c.spec_score < 0.0 {
                "ticked".to_string()
            } else {
                format!("overlap {:.2}", c.spec_score)
            },
            if c.regime { "y" } else { "" },
            c.paths,
            c.code.len(),
            c.code_new.len(),
            c.subject.replace('|', "/")
        ));
    }

    // ---- 6. reverse view: does each implementing commit's code trace up to a changed section? ----
    let unit_secs = |u: usize| -> BTreeSet<String> {
        gl.units[u].cites.iter().filter(|&&ci| gl.cites[ci].class == home()).flat_map(|&ci| gl.cites[ci].secs.iter().cloned()).filter(|s| diff.status.contains_key(s)).collect()
    };
    let block_secs = |b: usize| -> BTreeSet<String> {
        gl.blocks[b].cites.iter().filter(|&&ci| gl.cites[ci].class == home()).flat_map(|&ci| gl.cites[ci].secs.iter().cloned()).filter(|s| diff.status.contains_key(s)).collect()
    };
    let id_secs = |id: &str| -> BTreeSet<String> {
        let mut s = BTreeSet::new();
        for &b in gl.def_blocks.get(id).into_iter().flatten() {
            s.extend(unit_secs(gl.blocks[b].unit));
        }
        s
    };
    let mut rv = String::from("| commit | T-ids | candidate code files | citing an id | reaching a cited «home» section | reaching a changed section | changed sections reached (files) | task block: changed sections (direct / +1 hop) | subject |\n|---|---|---|---|---|---|---|---|---|\n");
    let (mut tot_files, mut tot_cite, mut tot_any, mut tot_ch, mut tot_task_direct, mut tot_task_hop, mut n_tspec) = (0, 0, 0, 0, 0, 0, 0);
    for c in commits.iter().filter(|c| c.spec == tspec) {
        n_tspec += 1;
        let mut n_cite = 0;
        let mut n_any = 0;
        let mut n_ch = 0;
        let mut secs_hit: BTreeSet<String> = BTreeSet::new();
        for &f in &c.code {
            let ts = &toks[f as usize];
            if ts.is_empty() {
                continue;
            }
            n_cite += 1;
            let ss: BTreeSet<String> = ts.iter().flat_map(|t| id_secs(t)).collect();
            if !ss.is_empty() {
                n_any += 1;
            }
            let ch: Vec<String> = ss.into_iter().filter(|s| changed.contains(s)).collect();
            if !ch.is_empty() {
                n_ch += 1;
                secs_hit.extend(ch);
            }
        }
        // the commit's own task blocks
        let mut direct: BTreeSet<String> = BTreeSet::new();
        let mut hop1: BTreeSet<String> = BTreeSet::new();
        for t in &c.tids {
            let tok = format!("{tspec}|{t}");
            for &b in gl.def_blocks.get(&tok).into_iter().flatten() {
                direct.extend(block_secs(b));
                for m in &gl.blocks[b].mentions {
                    for &b2 in gl.def_blocks.get(m).into_iter().flatten() {
                        hop1.extend(block_secs(b2));
                    }
                }
            }
        }
        let dch: Vec<&String> = direct.iter().filter(|s| changed.contains(*s)).collect();
        let hch: BTreeSet<&String> = hop1.iter().chain(direct.iter()).filter(|s| changed.contains(*s)).collect();
        tot_files += c.code.len();
        tot_cite += n_cite;
        tot_any += n_any;
        tot_ch += n_ch;
        if !dch.is_empty() {
            tot_task_direct += 1;
        }
        if !hch.is_empty() {
            tot_task_hop += 1;
        }
        let show = |v: Vec<&String>| -> String {
            let n = v.len();
            let mut s: Vec<String> = v.into_iter().take(8).cloned().collect();
            if n > 8 {
                s.push(format!("+{}", n - 8));
            }
            s.join(" ")
        };
        rv.push_str(&format!(
            "| {} | {} | {} | {n_cite} | {n_any} | {n_ch} | {} | {} / {} | {} |\n",
            &c.sha[..8],
            c.tids.join(" "),
            c.code.len(),
            show(secs_hit.iter().collect()),
            show(dch),
            show(hch.into_iter().collect()),
            c.subject.replace('|', "/")
        ));
    }
    let rv_head = format!(
        "«ts» commits: {n_tspec}. Over their candidate code files (counted per commit): {tot_files} files, {tot_cite} cite an id at the start, {tot_any} trace (id -> defining unit -> «home» citation) to a cited Act section, {tot_ch} to a changed one. Commits whose own task block cites a changed section directly: {tot_task_direct}; with one hop through the ids the task mentions: {tot_task_hop}.\n\n"
    );

    // ---- predictions ----
    let mut ptsv = String::from("rank\tfile\tC1_score\tC3_score\tin_«ts»\tin_«tr»\tin_ALL\tids_cited\n");
    let c3_rank = law_rank(&c3);
    for (r, &f) in c3_rank.iter().enumerate() {
        if c3[f as usize] <= 0.0 && c1[f as usize] <= 0.0 {
            break;
        }
        ptsv.push_str(&format!(
            "{}\t{}\t{:.4}\t{:.4}\t{}\t{}\t{}\t{}\n",
            r + 1,
            g.paths[f as usize],
            c1[f as usize],
            c3[f as usize],
            u8::from(truths[0].2.0.contains(&f)),
            u8::from(truths[1].2.0.contains(&f)),
            u8::from(truths[2].2.0.contains(&f)),
            toks[f as usize].iter().take(12).cloned().collect::<Vec<_>>().join(" ")
        ));
    }
    kwrite(outp("law_predictions.tsv"), &ptsv).expect("write predictions");

    // ---- law diff tables ----
    let mut ld = format!(
        "«old_desc»: {n_old} sections. «new_desc»: {n_new} sections. Repeated section numbers dropped as extraction noise: {:?} / {:?}.\n\n| status | sections |\n|---|---|\n| unchanged | {n_unch} |\n| changed | {n_ch} (of which {minor} with word-multiset Dice >= 0.97, minor edits) |\n| added | {n_add} |\n| repealed | {n_rep} |\n\n",
        diff.dups_old, diff.dups_new
    );
    ld.push_str(&format!(
        "Cross-check with «amend_desc» ({} item headings, {} on the «act»; {} inserted section headings, {} not matched to a «ver» title; {} sections named):\n\n| | named by the amending Act | not named |\n|---|---|---|\n| differs (changed / added / repealed) | {} | {} |\n| unchanged | {} | {} |\n\n",
        amend.items,
        amend.items_target,
        amend.inserted,
        amend.inserted_unmatched.len(),
        amend.named.len(),
        xc[1][1],
        xc[1][0],
        xc[0][1],
        xc[0][0]
    ));
    let list = |pred: &dyn Fn(&String) -> bool| -> String {
        diff.order.iter().filter(|n| pred(n)).cloned().collect::<Vec<_>>().join(", ")
    };
    ld.push_str(&format!("Differs but not named: {}\n\nNamed but unchanged: {}\n\nUnmatched inserted headings: {}\n\n", list(&|n| changed.contains(n) && !named(n)), list(&|n| !changed.contains(n) && named(n)), amend.inserted_unmatched.join("; ")));
    ld.push_str(&format!("Added sections whose text matches a repealed one (Dice >= 0.6, likely moved): {}\n\n", diff.moved.iter().map(|(a, b, x)| format!("{a} -> {b} ({x:.2})")).collect::<Vec<_>>().join(", ")));
    for st in ["changed", "added", "repealed"] {
        ld.push_str(&format!("**{st}** ({}): {}\n\n", count(st), list(&|n| diff.status[n] == st)));
    }

    // ---- write ----
    let chains_md: String = LAW_CHAINS.iter().map(|(_, n, d)| format!("- **{n}**: {d}\n")).collect();
    let mut all = format!(
        "# Law impact tables (Kina, «act_full» «old_sp» -> «new_sp»)\n\nStart {start} (code), wiki {wrev}; END (leaky) {end}. Generated by `rank_eval --law-impact kina` in {:.1}s.\n\nChains:\n{chains_md}\n## 1. Law diff\n\n{ld}\n## 2-3. Hops\n\n{hop}\n## Citations\n\n{cit_md}\n## 4. Truth commits (after the start; «new_pdf_note»)\n\n{tr}\n## 5. Scores\n{md}\n## 6. Reverse view («ts» commits)\n\n{rv_head}{rv}",
        t0.elapsed().as_secs_f64()
    );
    all.push('\n');
    kwrite(outp("law_tables.md"), &all).expect("write tables");
    js.insert("start".into(), json!(start));
    js.insert("wiki_rev".into(), json!(wrev));
    js.insert("end_rev".into(), json!(end));
    js.insert(
        "law".into(),
        json!({t("old"): n_old, t("new"): n_new, "unchanged": n_unch, "changed": n_ch, "changed_minor": minor, "added": n_add, "repealed": n_rep,
               "amend_items": amend.items, t("amend_items_target_key"): amend.items_target, "amend_named": amend.named.len(),
               "cross": {"differ_named": xc[1][1], "differ_not": xc[1][0], "same_named": xc[0][1], "same_not": xc[0][0]}}),
    );
    js.insert(
        "goal".into(),
        json!({"pages": gl.pages, "units": gl.units.len(), "blocks": gl.blocks.len(), "citations": gl.cites.len(),
               "classes": cls.iter().map(|((c, r), n)| (format!("{c}/{r}"), json!(n))).collect::<serde_json::Map<_, _>>(),
               "cited_known": cited.len(), "cited_changed": cited_changed.len(), "cited_unknown": cited_unknown.len(),
               "compliance_files_start": compliance.len(), "compliance_files_end": compliance_end, "candidate_files": nf}),
    );
    js.insert("scores".into(), serde_json::Value::Object(js_scores));
    js.insert("code_cite_stats".into(), json!(cstats));
    if !args.keys.is_empty() {
        let set = |ki: usize| -> Vec<u32> {
            let sc = chain_score(ki, &changed, false);
            law_rank(&sc).into_iter().filter(|&f| sc[f as usize] > 0.0).collect()
        };
        keys_law(&KLawIn {
            args,
            g: &g,
            ppr: &ppr,
            docs: &docs,
            start: &start,
            truths: &truths,
            commits: &commits,
            tspec,
            changed: &changed,
            status: &diff.status,
            bm_all: &bm_all,
            ppr_comp: &ppr_comp,
            comp_score: &comp_score,
            raw_sets: vec![("C1", set(0)), ("C1m", set(2)), ("C3", set(3)), ("C5", set(5))],
            raw_cited: (cited.len(), cited_changed.len()),
            prefix: pv.key_act.as_str(),
            inst: t("act_full"),
            extra_secs: Vec::new(),
        });
    }
    kwrite(outp("law_impact.json"), &serde_json::to_string_pretty(&serde_json::Value::Object(js)).unwrap()).expect("write json");
    eprintln!("[law] done in {:.1}s -> {}", t0.elapsed().as_secs_f64(), outp("law_tables.md"));
}

// ============================================================================
// Fourth run (2026-10-18): resolved cross-layer keys (`--keys`).
//
// The earlier runs joined the layers by raw text matches: an id string, a
// capitalised Act name, a path that happens to be a unique suffix. Here every
// layer emits KEY MENTIONS ("unit U cites / defines key K of type T"); one
// resolver per KEY TYPE normalises, scopes and versions them; one generic
// joiner links the mentions of one resolved key, or of one lineage class of
// keys, tiered FACT / DERIVED / HEURISTIC like glia's identity pass. No
// resolver knows which layer a mention came from, and no pair of layers has a
// resolver of its own.
//
// Key types:
//   path     files, directories and globs a unit names; relative and short
//            paths normalised; git renames followed to the file's name now;
//   scoped   spec-kit ids (FR-002, T014, R4, C#, D#, US#) scoped to the spec or
//            doc that defines them; ids numbered once (ABC-03) stay global;
//   lineage  old id -> new id (an id column of a numbered table, "was",
//            "formerly", "supersedes", "split from"): one identity class,
//            capped, so a citation of the old id reaches the new row;
//   cite     instrument + jurisdiction + section, resolved from context and
//            validated against each instrument's own section list;
//   row      numbered table rows as units ("decision 13", "rows 5, 6");
//   text     BM25 between a unit's text and code files' identifiers and
//            comments: the HEURISTIC fallback, top 3, only for units no other
//            key links to code.
// ============================================================================

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum KT {
    Path,
    Scoped,
    Lineage,
    Cite,
    Row,
    Text,
}

const KTS: [KT; 6] = [KT::Path, KT::Scoped, KT::Lineage, KT::Cite, KT::Row, KT::Text];

impl KT {
    fn name(self) -> &'static str {
        match self {
            KT::Path => "path",
            KT::Scoped => "scoped",
            KT::Lineage => "lineage",
            KT::Cite => "cite",
            KT::Row => "row",
            KT::Text => "text",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
enum Tier {
    Fact,
    Derived,
    Heuristic,
}

impl Tier {
    fn name(self) -> &'static str {
        match self {
            Tier::Fact => "FACT",
            Tier::Derived => "DERIVED",
            Tier::Heuristic => "HEURISTIC",
        }
    }
}

/// A resolution: the key, its tier and the route that resolved it.
type KRes = (String, Tier, &'static str);

/// One resolved key reference.
#[derive(Clone, Debug)]
struct KRef {
    kt: KT,
    key: String,
    tier: Tier,
    route: &'static str,
}

/// One resolved link with the text it came from (written for the hand audit only).
#[derive(Clone)]
struct KAudit {
    kt: KT,
    tier: Tier,
    route: &'static str,
    at: String,
    mention: String,
    target: String,
}

/// A joiner variant: key types dropped, or a type read through its raw (pre-resolution) form.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct KMask {
    drop: [bool; 6],
    raw_path: bool,
    raw_scoped: bool,
    raw_cite: bool,
    /// the weakest tier a link may have (HEURISTIC keeps every link)
    max_tier: Tier,
    /// Round 2: role-aware joining (hops only along `implements` mentions, names-file links weighted
    /// by the naming unit's state)
    roles: bool,
}

impl KMask {
    const FULL: KMask = KMask { drop: [false; 6], raw_path: false, raw_scoped: false, raw_cite: false, max_tier: Tier::Heuristic, roles: false };
    fn on(&self, kt: KT) -> bool {
        !self.drop[kt as usize]
    }
    fn keeps(&self, r: &KRef) -> bool {
        self.on(r.kt) && r.tier <= self.max_tier
    }
}

/// The base joiner: every key type except the text fallback.
const KBASE: KMask = KMask { drop: [false, false, false, false, false, true], raw_path: false, raw_scoped: false, raw_cite: false, max_tier: Tier::Heuristic, roles: false };
/// Round 2: the base joiner with roles on.
const KROLES: KMask = KMask { roles: true, ..KBASE };

/// The ablations against the base: (name suffix, joiner variant). `-<type>` drops the type's
/// links; `-raw<type>` reads the type through the earlier runs' raw matching instead;
/// `-heuristic` keeps FACT and DERIVED links only; `+text` adds the BM25 fallback.
fn kvariants() -> Vec<(&'static str, KMask)> {
    let drop = |kt: KT| {
        let mut m = KBASE;
        m.drop[kt as usize] = true;
        m
    };
    vec![
        ("-path", drop(KT::Path)),
        ("-rawpath", KMask { raw_path: true, ..KBASE }),
        ("-scoped", drop(KT::Scoped)),
        ("-rawscoped", KMask { raw_scoped: true, ..KBASE }),
        ("-lineage", drop(KT::Lineage)),
        ("-rawcite", KMask { raw_cite: true, ..KBASE }),
        ("-row", drop(KT::Row)),
        ("-heuristic", KMask { max_tier: Tier::Derived, ..KBASE }),
        ("+text", KMask::FULL),
    ]
}

/// The links a model resolved, counted by key type, tier and route.
fn ktally<'a>(refs: impl Iterator<Item = &'a KRef>, stats: &mut BTreeMap<String, f64>) {
    for r in refs {
        kbump(stats, format!("k_link_{}_{}", r.kt.name(), r.tier.name()));
        kbump(stats, format!("k_link_{}_{}_{}", r.kt.name(), r.tier.name(), r.route));
    }
}

fn kbump(stats: &mut BTreeMap<String, f64>, k: impl Into<String>) {
    *stats.entry(k.into()).or_default() += 1.0;
}

/// Identity classes over keys from lineage edges: union-find, FACT edges first, and a
/// merge that would make a class larger than `cap` is refused (a class that large is a
/// grouping, not one thing). A class is named by its least key.
struct KClasses {
    rep: HashMap<String, String>,
}

impl KClasses {
    fn build(edges: &[(String, String, Tier)], cap: usize, stats: &mut BTreeMap<String, f64>) -> KClasses {
        let mut ix: HashMap<String, usize> = HashMap::new();
        let mut names: Vec<String> = Vec::new();
        let mut parent: Vec<usize> = Vec::new();
        let mut size: Vec<usize> = Vec::new();
        let mut order: Vec<&(String, String, Tier)> = edges.iter().collect();
        order.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)).then_with(|| a.1.cmp(&b.1)));
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for (a, b, _) in order {
            if a == b {
                continue;
            }
            let mut id = |k: &String| -> usize {
                if let Some(&i) = ix.get(k) {
                    return i;
                }
                let i = names.len();
                ix.insert(k.clone(), i);
                names.push(k.clone());
                parent.push(i);
                size.push(1);
                i
            };
            let (ia, ib) = (id(a), id(b));
            let (ra, rb) = (find(&mut parent, ia), find(&mut parent, ib));
            if ra == rb {
                continue;
            }
            if size[ra] + size[rb] > cap {
                kbump(stats, "k_lineage_merge_refused_cap");
                continue;
            }
            let (big, small) = if size[ra] >= size[rb] { (ra, rb) } else { (rb, ra) };
            parent[small] = big;
            size[big] += size[small];
            kbump(stats, "k_lineage_merges");
        }
        let mut least: HashMap<usize, String> = HashMap::new();
        for (i, name) in names.iter().enumerate() {
            let r = find(&mut parent, i);
            let e = least.entry(r).or_insert_with(|| name.clone());
            if name < e {
                *e = name.clone();
            }
        }
        let mut rep = HashMap::new();
        for (i, name) in names.iter().enumerate() {
            let r = find(&mut parent, i);
            if least[&r] != *name {
                rep.insert(name.clone(), least[&r].clone());
            }
        }
        KClasses { rep }
    }

    fn of<'a>(&'a self, k: &'a str) -> &'a str {
        self.rep.get(k).map_or(k, String::as_str)
    }
}

/// Id lineage over oriented edges (new, old): an edge whose ends have no other lineage
/// partner is a rename and merges into one identity class; an old id several new ids
/// absorb, or a new row that lists several old ids, redirects each old id to its new id(s)
/// without making the siblings one thing.
struct KLin {
    cl: KClasses,
    redirect: HashMap<String, Vec<String>>,
}

impl KLin {
    /// `edges`: (new, old, tier, always merge). A restated row or a moved section is one thing
    /// whatever its other partners; any other edge merges only when it is one-to-one.
    fn build(edges: &[(String, String, Tier, bool)], cap: usize, stats: &mut BTreeMap<String, f64>) -> KLin {
        let always: HashSet<(&String, &String)> = edges.iter().filter(|e| e.3).map(|e| (&e.0, &e.1)).collect();
        let pairs: BTreeSet<(&String, &String, Tier)> = edges.iter().filter(|e| e.0 != e.1).map(|e| (&e.0, &e.1, e.2)).collect();
        let mut part: HashMap<&str, BTreeSet<&str>> = HashMap::new();
        for (n, o, _) in &pairs {
            part.entry(n.as_str()).or_default().insert(o.as_str());
            part.entry(o.as_str()).or_default().insert(n.as_str());
        }
        let mut merge = Vec::new();
        let mut redirect: HashMap<String, Vec<String>> = HashMap::new();
        for (n, o, t) in pairs {
            if always.contains(&(n, o)) || part[n.as_str()].len() == 1 && part[o.as_str()].len() == 1 {
                merge.push((n.clone(), o.clone(), t));
                kbump(stats, "k_lineage_merge_one_to_one");
            } else {
                redirect.entry(o.clone()).or_default().push(n.clone());
                kbump(stats, "k_lineage_redirect");
            }
        }
        KLin { cl: KClasses::build(&merge, cap, stats), redirect }
    }

    /// A key's class, plus the classes of the new ids an old one redirects to.
    fn exp(&self, k: &str) -> Vec<String> {
        let mut v = vec![self.cl.of(k).to_string()];
        for n in self.redirect.get(k).into_iter().flatten() {
            let c = self.cl.of(n).to_string();
            if !v.contains(&c) {
                v.push(c);
            }
        }
        v
    }
}

// ---- the path resolver ----

/// `(old, new)` for every rename git detects (-M) in non-merge history to `rev`, oldest first.
fn git_renames(git: &str, rev: &str) -> Vec<(String, String)> {
    let text = git_text(git, &["log", "--no-merges", "-M", "--diff-filter=R", "--name-status", "--format=%x1e", rev]).unwrap_or_default();
    let mut out = Vec::new();
    for rec in text.split('\x1e').skip(1) {
        for l in rec.lines() {
            let mut p = l.split('\t');
            if p.next().is_some_and(|st| st.starts_with('R'))
                && let (Some(a), Some(b)) = (p.next(), p.next())
            {
                out.push((a.to_string(), b.to_string()));
            }
        }
    }
    out.reverse();
    out
}

/// Candidate files (the snapshot's), every historical path git renamed into one, and the
/// suffix / directory indexes over both.
struct KPaths {
    cand: Vec<String>,
    exact: HashMap<String, u32>,
    hist: HashMap<String, u32>,
    suffix: HashMap<String, Vec<u32>>,
    hsuffix: HashMap<String, Vec<u32>>,
    dir_files: HashMap<String, Vec<u32>>,
    dir_suffix: HashMap<String, Vec<String>>,
    exts: HashSet<String>,
    renames: usize,
}

const KDIR_CAP: usize = 30;

impl KPaths {
    fn new(cand: &[String], renames: &[(String, String)]) -> KPaths {
        let exact: HashMap<String, u32> = cand.iter().enumerate().map(|(i, p)| (p.clone(), i as u32)).collect();
        let mut fwd: HashMap<&str, &str> = HashMap::new();
        for (a, b) in renames {
            fwd.insert(a.as_str(), b.as_str());
        }
        let mut hist: HashMap<String, u32> = HashMap::new();
        for &old in fwd.keys() {
            if exact.contains_key(old) {
                continue;
            }
            let mut p = old;
            for _ in 0..64 {
                match fwd.get(p) {
                    Some(&n) => {
                        p = n;
                        if exact.contains_key(p) {
                            break;
                        }
                    }
                    None => break,
                }
            }
            if let Some(&f) = exact.get(p) {
                hist.insert(old.to_string(), f);
            }
        }
        let mut suffix: HashMap<String, Vec<u32>> = HashMap::new();
        let mut dir_files: HashMap<String, Vec<u32>> = HashMap::new();
        let add_dirs = |segs: &[&str], f: u32, dir_files: &mut HashMap<String, Vec<u32>>| {
            for end in 1..segs.len() {
                dir_files.entry(segs[..end].join("/")).or_default().push(f);
            }
        };
        for (f, p) in cand.iter().enumerate() {
            let segs: Vec<&str> = p.split('/').collect();
            for k in 0..segs.len() {
                suffix.entry(segs[k..].join("/")).or_default().push(f as u32);
            }
            add_dirs(&segs, f as u32, &mut dir_files);
        }
        let mut hsuffix: HashMap<String, Vec<u32>> = HashMap::new();
        for (old, &f) in &hist {
            let segs: Vec<&str> = old.split('/').collect();
            for k in 0..segs.len() {
                hsuffix.entry(segs[k..].join("/")).or_default().push(f);
            }
            add_dirs(&segs, f, &mut dir_files);
        }
        for v in suffix.values_mut().chain(hsuffix.values_mut()).chain(dir_files.values_mut()) {
            v.sort_unstable();
            v.dedup();
        }
        let mut dir_suffix: HashMap<String, Vec<String>> = HashMap::new();
        for d in dir_files.keys() {
            let segs: Vec<&str> = d.split('/').collect();
            for k in 0..segs.len() {
                dir_suffix.entry(segs[k..].join("/")).or_default().push(d.clone());
            }
        }
        for v in dir_suffix.values_mut() {
            v.sort();
            v.dedup();
        }
        let exts = cand.iter().map(|p| ext_of(p)).filter(|e| !e.is_empty()).collect();
        KPaths { cand: cand.to_vec(), exact, hist, suffix, hsuffix, dir_files, dir_suffix, exts, renames: renames.len() }
    }

    /// The candidate a path at some revision is now: itself, or the file git renamed it into.
    fn map(&self, p: &str) -> Option<u32> {
        self.exact.get(p).or_else(|| self.hist.get(p)).copied()
    }
}

#[derive(Debug)]
enum PTok {
    File(String),
    Dir(String),
    Glob(String),
}

/// Path-like tokens on a line: split at whitespace and at quoting, bracket and separator
/// characters outside `{..}`, so a brace glob keeps its commas.
fn kpath_tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0usize;
    for c in line.chars() {
        let sep = c.is_whitespace()
            || matches!(c, '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '<' | '>' | ';' | '|' | '=')
            || (c == ',' && depth == 0);
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth = depth.saturating_sub(1);
        }
        if sep && depth == 0 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        if depth > 0 {
            out.extend(cur.split_whitespace().map(str::to_string));
        } else {
            out.push(cur);
        }
    }
    out
}

/// A token normalised to a path mention: emphasis, line ranges (`:12-40`, `#L12`), `./`,
/// `../` and a leading `/` removed; URLs dropped.
fn kpath_norm(raw: &str, exts: &HashSet<String>) -> Option<PTok> {
    let mut t: &str = raw;
    for _ in 0..2 {
        t = t.trim_end_matches(['.', ',', ':', ';', '!', '?']);
        if let Some(x) = t.strip_prefix("**").filter(|x| !x.starts_with('/')) {
            t = x;
        }
        if let Some(x) = t.strip_suffix("**").filter(|x| !x.ends_with('/')) {
            t = x;
        }
        t = t.trim_matches(|c: char| c == '_' || c == '~');
    }
    if t.contains("://") || t.starts_with("http") || t.starts_with("www.") || t.contains('@') && !t.contains('/') {
        return None;
    }
    let mut s = t.to_string();
    if let Some(i) = s.find("#L") {
        s.truncate(i);
    }
    if let Some(i) = s.char_indices().find(|&(i, c)| c == ':' && s[i + 1..].starts_with(|d: char| d.is_ascii_digit())).map(|x| x.0) {
        s.truncate(i);
    }
    let mut s = s.as_str();
    loop {
        if let Some(x) = s.strip_prefix("./") {
            s = x;
        } else if let Some(x) = s.strip_prefix("../") {
            s = x;
        } else if let Some(x) = s.strip_prefix('/') {
            s = x;
        } else {
            break;
        }
    }
    let s = s.trim_end_matches(['.', ',', ':']);
    if s.len() < 3 || !s.chars().any(|c| c.is_ascii_alphabetic()) || s.contains("..") {
        return None;
    }
    if s.contains('{') || s.contains('*') {
        return (s.contains('/') || s.contains('.')).then(|| PTok::Glob(s.to_string()));
    }
    if let Some(d) = s.strip_suffix('/') {
        return (!d.is_empty()).then(|| PTok::Dir(d.to_string()));
    }
    let e = ext_of(s);
    if !e.is_empty() && exts.contains(&e) {
        return Some(PTok::File(s.to_string()));
    }
    if s.contains('/') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.' | '@' | '+')) {
        return Some(PTok::Dir(s.to_string()));
    }
    None
}

/// `{a,b}` alternatives expanded (cartesian, capped at 64 patterns).
fn kbraces(p: &str) -> Vec<String> {
    let Some(o) = p.find('{') else { return vec![p.to_string()] };
    let Some(c) = p[o..].find('}').map(|x| o + x) else { return vec![p.replace('{', "")] };
    let (head, alts, tail) = (&p[..o], &p[o + 1..c], &p[c + 1..]);
    let mut out = Vec::new();
    for t in kbraces(tail) {
        for a in alts.split(',') {
            out.push(format!("{head}{}{t}", a.trim()));
            if out.len() >= 64 {
                return out;
            }
        }
    }
    out
}

/// Glob match: `*` and `?` stay inside a segment, `**` crosses segments.
fn kglob(p: &[u8], s: &[u8]) -> bool {
    if p.is_empty() {
        return s.is_empty();
    }
    if p.starts_with(b"**") {
        let rest = if p.len() > 2 && p[2] == b'/' { &p[3..] } else { &p[2..] };
        return (0..=s.len()).any(|k| (k == 0 || s[k - 1] == b'/') && kglob(rest, &s[k..])) || kglob(rest, s);
    }
    match p[0] {
        b'*' => (0..=s.len()).take_while(|&k| k == 0 || s[k - 1] != b'/').any(|k| kglob(&p[1..], &s[k..])),
        b'?' => !s.is_empty() && s[0] != b'/' && kglob(&p[1..], &s[1..]),
        c => !s.is_empty() && s[0] == c && kglob(&p[1..], &s[1..]),
    }
}

impl KPaths {
    /// The candidates under one directory (a full directory or a suffix naming exactly one).
    fn dir(&self, d: &str, stats: &mut BTreeMap<String, f64>) -> Option<(Vec<u32>, &'static str)> {
        let full: Vec<&String> = match self.dir_files.get_key_value(d) {
            Some((k, _)) => vec![k],
            None => self.dir_suffix.get(d).map(|v| v.iter().collect()).unwrap_or_default(),
        };
        if full.len() != 1 {
            if full.len() > 1 {
                kbump(stats, "k_path_dir_ambiguous");
            }
            return None;
        }
        let files = &self.dir_files[full[0]];
        if files.len() > KDIR_CAP {
            kbump(stats, "k_path_dir_over_cap");
            return None;
        }
        Some((files.clone(), if full[0] == d { "dir" } else { "dir-suffix" }))
    }

    /// One file mention resolved: exact, renamed, a unique suffix or bare name, a suffix the
    /// block's other paths disambiguate, or a unique suffix of a renamed path.
    fn file(&self, t: &str, ctx: &[String], stats: &mut BTreeMap<String, f64>) -> Option<(u32, Tier, &'static str)> {
        if let Some(&f) = self.exact.get(t) {
            return Some((f, Tier::Fact, "exact"));
        }
        if let Some(&f) = self.hist.get(t) {
            return Some((f, Tier::Derived, "rename"));
        }
        if let Some(v) = self.suffix.get(t) {
            if v.len() == 1 {
                return Some((v[0], Tier::Derived, if t.contains('/') { "suffix" } else { "name" }));
            }
            // the block's other paths pick the candidate in their directory tree
            let mut best = (0usize, Vec::new());
            for &f in v {
                let fd: Vec<&str> = dir_of(&self.cand[f as usize]).split('/').collect();
                let mut s = 0;
                for c in ctx {
                    let cd: Vec<&str> = c.split('/').collect();
                    s = s.max(fd.iter().zip(&cd).take_while(|(a, b)| a == b).count());
                }
                if s > best.0 {
                    best = (s, vec![f]);
                } else if s == best.0 && s > 0 {
                    best.1.push(f);
                }
            }
            if best.1.len() == 1 {
                return Some((best.1[0], Tier::Derived, "context"));
            }
            kbump(stats, "k_path_ambiguous");
            return None;
        }
        if let Some(v) = self.hsuffix.get(t)
            && v.len() == 1
        {
            return Some((v[0], Tier::Derived, "rename-suffix"));
        }
        None
    }

    /// Every path mention on the lines of one block, resolved (tokens in block order; the
    /// block's own unique paths are the context that disambiguates a short suffix).
    fn block(&self, toks: &[String], stats: &mut BTreeMap<String, f64>) -> Vec<(u32, Tier, &'static str, String)> {
        let cand = &self.cand;
        let mut out: Vec<(u32, Tier, &'static str, String)> = Vec::new();
        let mut pending: Vec<&String> = Vec::new();
        let mut ctx: Vec<String> = Vec::new();
        for raw in toks {
            let Some(pt) = kpath_norm(raw, &self.exts) else { continue };
            match pt {
                PTok::File(t) => match self.file(&t, &[], stats) {
                    Some((f, tier, route)) => {
                        ctx.push(dir_of(&cand[f as usize]).to_string());
                        out.push((f, tier, route, raw.clone()));
                    }
                    None if self.suffix.get(&t).is_some_and(|v| v.len() > 1) => pending.push(raw),
                    None => kbump(stats, "k_path_unresolved_file"),
                },
                PTok::Dir(d) => {
                    if let Some((fs, route)) = self.dir(&d, stats) {
                        for f in fs {
                            out.push((f, Tier::Derived, route, raw.clone()));
                        }
                    }
                }
                PTok::Glob(g) => {
                    let mut got: BTreeSet<u32> = BTreeSet::new();
                    for p in kbraces(&g) {
                        if !p.contains('*') {
                            match kpath_norm(&p, &self.exts) {
                                Some(PTok::File(t)) => got.extend(self.file(&t, &[], stats).map(|x| x.0)),
                                Some(PTok::Dir(d)) => got.extend(self.dir(&d, stats).map(|x| x.0).unwrap_or_default()),
                                _ => {}
                            }
                            continue;
                        }
                        // a glob with no directory part (`client.*`) names no particular place
                        if !p.contains('/') {
                            kbump(stats, "k_path_glob_no_dir");
                            continue;
                        }
                        let anchored = cand.iter().any(|c| c.split('/').next() == p.split('/').next());
                        let pat = if anchored { p.clone() } else { format!("**/{p}") };
                        for (f, c) in cand.iter().enumerate() {
                            if kglob(pat.as_bytes(), c.as_bytes()) {
                                got.insert(f as u32);
                            }
                        }
                    }
                    if got.len() > KDIR_CAP {
                        kbump(stats, "k_path_glob_over_cap");
                    } else {
                        for f in got {
                            out.push((f, Tier::Derived, "glob", raw.clone()));
                        }
                    }
                }
            }
        }
        for raw in pending {
            if let Some(PTok::File(t)) = kpath_norm(raw, &self.exts)
                && let Some((f, tier, route)) = self.file(&t, &ctx, stats)
            {
                out.push((f, tier, route, raw.clone()));
            }
        }
        out
    }
}

// ---- the code text index (BM25 over each candidate's text at a revision) ----

/// One blob's hashed terms with their counts, and its length in tokens.
type KTf = std::sync::Arc<(Vec<(u64, u32)>, u32)>;

/// Term-frequency vectors per blob (terms hashed), read once per blob for every revision.
#[derive(Default)]
struct KBlobs {
    tf: HashMap<String, KTf>,
    /// revision -> (candidate, blob) for the candidates present at it
    trees: HashMap<String, Vec<(u32, String)>>,
}

fn ktf(text: &str) -> (Vec<(u64, u32)>, u32) {
    let toks = law_code_tokens(text);
    let mut m: HashMap<u64, u32> = HashMap::new();
    for t in &toks {
        *m.entry(h64(t)).or_default() += 1;
    }
    let mut v: Vec<(u64, u32)> = m.into_iter().collect();
    v.sort_unstable();
    (v, toks.len() as u32)
}

impl KBlobs {
    /// `git ls-tree` at every revision, then one `git cat-file --batch` over the blobs not yet read.
    fn load(git: &str, revs: &[String], kp: &KPaths) -> KBlobs {
        use std::io::{BufRead, BufReader, Read, Write};
        let mut kb = KBlobs::default();
        let trees: Vec<(String, Vec<(u32, String)>)> = revs
            .par_iter()
            .map(|r| {
                let text = git_text(git, &["ls-tree", "-r", r]).unwrap_or_default();
                let mut v: Vec<(u32, String)> = Vec::new();
                let mut seen: HashSet<u32> = HashSet::new();
                for l in text.lines() {
                    let Some((meta, path)) = l.split_once('\t') else { continue };
                    let Some(blob) = meta.split_whitespace().nth(2) else { continue };
                    if let Some(f) = kp.map(path)
                        && seen.insert(f)
                    {
                        v.push((f, blob.to_string()));
                    }
                }
                (r.clone(), v)
            })
            .collect();
        let mut need: BTreeSet<String> = BTreeSet::new();
        for (_, v) in &trees {
            need.extend(v.iter().map(|x| x.1.clone()));
        }
        kb.trees = trees.into_iter().collect();
        let need: Vec<String> = need.into_iter().collect();
        let mut child = Command::new("git")
            .arg("-C")
            .arg(git)
            .args(["cat-file", "--batch"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("git cat-file --batch");
        let mut stdin = child.stdin.take().expect("stdin");
        let input: String = need.iter().map(|s| format!("{s}\n")).collect();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(input.as_bytes());
        });
        let mut r = BufReader::new(child.stdout.take().expect("stdout"));
        let mut raw: Vec<(String, Vec<u8>)> = Vec::with_capacity(need.len());
        for _ in &need {
            let mut head = String::new();
            if r.read_line(&mut head).unwrap_or(0) == 0 {
                break;
            }
            let parts: Vec<&str> = head.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }
            let size: usize = parts[2].parse().unwrap_or(0);
            let mut buf = vec![0u8; size];
            if r.read_exact(&mut buf).is_err() {
                break;
            }
            let mut nl = [0u8; 1];
            let _ = r.read_exact(&mut nl);
            raw.push((parts[0].to_string(), buf));
        }
        let _ = writer.join();
        let _ = child.wait();
        kb.tf = raw
            .into_par_iter()
            .map(|(s, b)| (s, std::sync::Arc::new(ktf(&String::from_utf8_lossy(&b)))))
            .collect();
        kb
    }

    fn index(&self, rev: &str, nf: usize) -> KBm {
        let mut post: HashMap<u64, Vec<(u32, u32)>> = HashMap::new();
        let mut dl = vec![0u32; nf];
        for (f, blob) in self.trees.get(rev).into_iter().flatten() {
            if let Some(t) = self.tf.get(blob) {
                dl[*f as usize] = t.1;
                for &(term, c) in &t.0 {
                    post.entry(term).or_default().push((*f, c));
                }
            }
        }
        let n = dl.iter().filter(|&&d| d > 0).count();
        let avg = dl.iter().map(|&d| f64::from(d)).sum::<f64>() / n.max(1) as f64;
        KBm { post, dl, avg: avg.max(1e-9), n }
    }
}

struct KBm {
    post: HashMap<u64, Vec<(u32, u32)>>,
    dl: Vec<u32>,
    avg: f64,
    n: usize,
}

impl KBm {
    /// Round 2: BM25 of a unit's text against each of the given files (0 when nothing matches).
    fn score_files(&self, text: &str, files: &[u32]) -> Vec<f64> {
        let mut q: Vec<u64> = law_code_tokens(text).iter().map(|t| h64(t)).collect();
        q.sort_unstable();
        q.dedup();
        let want: HashMap<u32, usize> = files.iter().enumerate().map(|(i, &f)| (f, i)).collect();
        let mut out = vec![0.0; files.len()];
        if self.n == 0 {
            return out;
        }
        for t in q {
            let Some(pl) = self.post.get(&t) else { continue };
            let df = pl.len() as f64;
            let idf = (1.0 + (self.n as f64 - df + 0.5) / (df + 0.5)).ln();
            for &(d, tf) in pl {
                if let Some(&i) = want.get(&d) {
                    let tf = f64::from(tf);
                    let dl = f64::from(self.dl[d as usize]);
                    out[i] += idf * tf * (BM25_K1 + 1.0) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / self.avg));
                }
            }
        }
        out
    }

    /// The top `k` files by BM25 of a unit's text (score > 0), best first.
    fn top(&self, text: &str, k: usize) -> Vec<(u32, f64)> {
        let mut q: Vec<u64> = law_code_tokens(text).iter().map(|t| h64(t)).collect();
        q.sort_unstable();
        q.dedup();
        if q.len() < 8 || self.n == 0 {
            return Vec::new();
        }
        let mut s: HashMap<u32, f64> = HashMap::new();
        for t in q {
            let Some(pl) = self.post.get(&t) else { continue };
            let df = pl.len() as f64;
            let idf = (1.0 + (self.n as f64 - df + 0.5) / (df + 0.5)).ln();
            for &(d, tf) in pl {
                let tf = f64::from(tf);
                let dl = f64::from(self.dl[d as usize]);
                *s.entry(d).or_default() += idf * tf * (BM25_K1 + 1.0) / (tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / self.avg));
            }
        }
        let mut v: Vec<(u32, f64)> = s.into_iter().filter(|x| x.1 > 0.0).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(k);
        v
    }
}

// ---- law knowledge: each instrument's own section list ----

/// Each instrument's section list (file stems in the private config): the primary instrument
/// (either version, and each), the rules' provisions, the second instrument's sections.
struct LawKnown {
    act: HashSet<String>,
    act_old: HashSet<String>,
    act_new: HashSet<String>,
    rules: HashSet<String>,
    trules: HashSet<String>,
    moved: Vec<(String, String)>,
    order: Vec<String>,
}

/// A rules provision heading at the margin: `6-12 Some obligation`.
fn rules_head(l: &str) -> Option<String> {
    let b = l.as_bytes();
    let d = digits_at(b, 0, 3);
    if d == 0 || d > 2 || d >= b.len() || b[d] != b'-' {
        return None;
    }
    let d2 = digits_at(b, d + 1, 4);
    if d2 == d + 1 || d2 - d - 1 > 3 {
        return None;
    }
    let mut q = d2;
    if q < b.len() && b[q].is_ascii_uppercase() {
        q += 1;
    }
    (q + 1 < b.len() && b[q] == b' ' && b[q + 1].is_ascii_uppercase()).then(|| l[..q].to_string())
}

fn law_known(args: &Args) -> LawKnown {
    let pv = prv();
    let (old, dold) = act_sections(&law_lines(&law_pdf(args, &pv.pdf_act_old), act_furniture));
    let (new, dnew) = act_sections(&law_lines(&law_pdf(args, &pv.pdf_act_new), act_furniture));
    let act_old: HashSet<String> = old.iter().map(|s| s.num.clone()).collect();
    let act_new: HashSet<String> = new.iter().map(|s| s.num.clone()).collect();
    let diff = law_diff(old, new, dold, dnew);
    let rules: HashSet<String> = law_pdf(args, &pv.pdf_rules).lines().filter_map(rules_head).collect();
    let trules: HashSet<String> = law_lines(&law_pdf(args, &pv.pdf_trules), |_| false).iter().filter_map(|l| law_sec_heading(l).map(|x| x.0)).collect();
    keprintln!(
        "[keys] law: «home» Act {} sections («old_sp» {}, «new_sp» {}), Rules {} provisions, «trules_name» {} sections, {} moved",
        diff.order.len(),
        act_old.len(),
        act_new.len(),
        rules.len(),
        trules.len(),
        diff.moved.len()
    );
    LawKnown {
        act: diff.order.iter().cloned().collect(),
        act_old,
        act_new,
        rules,
        trules,
        moved: diff.moved.iter().map(|(a, b, _)| (a.clone(), b.clone())).collect(),
        order: diff.order.clone(),
    }
}

/// Rules provisions a line cites: `Rule 6-12(4)`, `Rules 5-17(6)(c), 5-18(3)(c)`.
fn rules_cites(line: &str) -> Vec<String> {
    let b = line.as_bytes();
    let lower = line.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = lower[from..].find("rule") {
        let s = from + p;
        from = s + 4;
        if !wb(b, s) {
            continue;
        }
        let mut e = s + 4;
        if e < b.len() && b[e] == b's' {
            e += 1;
        }
        if !(e < b.len() && b[e] == b' ') {
            continue;
        }
        e += 1;
        loop {
            let d = digits_at(b, e, 3);
            if d == e || d - e > 2 || d >= b.len() || b[d] != b'-' {
                break;
            }
            let d2 = digits_at(b, d + 1, 4);
            if d2 == d + 1 || d2 - d - 1 > 3 {
                break;
            }
            let mut q = d2;
            if q < b.len() && b[q].is_ascii_uppercase() {
                q += 1;
            }
            out.push(line[e..q].to_string());
            let mut r = q;
            while r < b.len() && b[r] == b'(' {
                match line[r..].find(')') {
                    Some(x) if x <= 8 => r += x + 1,
                    _ => break,
                }
            }
            match [", ", " and ", " or "].iter().find(|s| line[r..].starts_with(**s)) {
                Some(sp) => e = r + sp.len(),
                None => break,
            }
        }
    }
    out
}

/// The page-level instrument context of a doc page (or a code file): the earlier run's
/// jurisdiction, regime-page and page-default reading, plus a second-instrument title.
struct KCiteCtx {
    page_jur: &'static str,
    page_def: &'static str,
    regime_page: bool,
    tr_page: bool,
    /// the other instrument the page names most (for a bare number on a page about it)
    page_other: String,
}

fn kcite_ctx(path: &str, lines: &[&str]) -> KCiteCtx {
    let PageJur { jur: page_jur, regime: regime_page, title, pl } = page_jur_of(path, lines);
    let mut cnt: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut others: BTreeMap<String, usize> = BTreeMap::new();
    for l in lines {
        for m in law_mentions(l) {
            let c = law_class(m.inst, l, page_jur, "");
            if c == home() || c == oth() || c == "OTHER" {
                *cnt.entry(c).or_default() += 1;
            }
            if c == "OTHER" {
                let n = kother_name(l, &m);
                if n.len() > 3 {
                    *others.entry(n).or_default() += 1;
                }
            }
        }
    }
    // a page's bare numbers belong to its other instrument only when one instrument dominates it
    let total: usize = others.values().sum();
    let page_other = others
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
        .filter(|x| x.1 * 10 >= total * 7)
        .map(|x| x.0)
        .unwrap_or_default();
    let mut page_def: &'static str = "";
    if regime_page {
        page_def = if page_jur == oth() { oth() } else { home() };
    } else {
        let mut best = 0;
        for c in [home(), oth(), "OTHER"] {
            let n = cnt.get(c).copied().unwrap_or(0);
            if n > best {
                best = n;
                page_def = c;
            }
        }
    }
    let tw = prv().trules_word.as_str();
    KCiteCtx { page_jur, page_def, regime_page, tr_page: title.contains(tw) || pl.contains(tw), page_other }
}

/// The name an "other instrument" mention gives (`Corporations Act`, `CBA`), normalised.
fn kother_name(l: &str, m: &LMention) -> String {
    let spaced = l.replace('_', " ");
    let word = &spaced[m.start..m.end];
    let mut name = word.to_string();
    if matches!(word, "Act" | "Acts") {
        // the capitalised run just before "Act", stopping at punctuation (`PDF; Corporations Act`)
        let before: Vec<&str> = spaced[..m.start]
            .split_whitespace()
            .rev()
            .take_while(|w| w.chars().next().is_some_and(char::is_uppercase) && !w.ends_with([',', ';', ':', '.', ')', '|']))
            .take(5)
            .collect();
        name = before.into_iter().rev().chain(std::iter::once("Act")).collect::<Vec<_>>().join(" ");
    }
    name.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_lowercase()
}

/// One line's Act-section citations, read twice: the earlier run's way (`raw`: the home
/// jurisdiction's class, numbers unchecked) and this run's (every number checked against the
/// instrument's own section list; the second instrument from the line, a heading or the
/// page title; another instrument keyed by its name; no instrument read across a `;`;
/// a code file's bare number defaults to the primary instrument, HEURISTIC).
/// Where on its page a line sits, for the citation resolver: the page context, the heading
/// stack above it, and the last instrument named a few lines up.
struct KCiteAt<'a> {
    cx: &'a KCiteCtx,
    stack: &'a [(usize, String)],
    recent: Option<(usize, &'static str)>,
}

fn kcite_line(l: &str, ln: usize, at: &KCiteAt, known: &LawKnown, code: bool, stats: &mut BTreeMap<String, f64>) -> (Vec<KRes>, Vec<String>) {
    let (cx, stack, recent) = (at.cx, at.stack, at.recent);
    let ments = law_mentions(l);
    let mut res: Vec<(String, Tier, &'static str)> = Vec::new();
    let mut raw: Vec<String> = Vec::new();
    let pv = prv();
    let tw = pv.trules_word.as_str();
    let tr_ctx = l.to_ascii_lowercase().contains(tw) || stack.iter().any(|(_, h)| h.to_ascii_lowercase().contains(tw)) || cx.tr_page;
    for sc in law_cites(l, &known.order) {
        let pick = |clause: bool| -> Option<(usize, &LMention)> {
            let mut best: Option<(usize, &LMention)> = None;
            for m in ments.iter().filter(|m| m.inst != LInst::Rules) {
                let dist = if m.end <= sc.start {
                    sc.start - m.end
                } else if m.start >= sc.end && m.start - sc.end <= 30 {
                    m.start - sc.end + 20
                } else {
                    continue;
                };
                if clause {
                    let (a, b) = if m.end <= sc.start { (m.end, sc.start) } else { (sc.end, m.start) };
                    if l.get(a..b).is_some_and(|x| x.contains(';')) {
                        continue;
                    }
                }
                if best.is_none_or(|(bd, _)| dist < bd) {
                    best = Some((dist, m));
                }
            }
            best
        };
        let fallback = || -> (&'static str, &'static str, Option<String>) {
            if law_regime_words(l) {
                let other = pv.other_line_marks.iter().any(|w| l.contains(w.as_str()));
                return (if other { oth() } else { home() }, "line-words", None);
            }
            let mut hc = None;
            for (hlv, hl) in stack.iter().rev() {
                if let Some(m) = law_mentions(hl).iter().rfind(|m| m.inst != LInst::TheAct) {
                    let c = law_class(m.inst, hl, cx.page_jur, cx.page_def);
                    hc = Some((c, (c == "OTHER").then(|| kother_name(hl, m))));
                    break;
                }
                if *hlv >= 2 && !hl.contains("excluding") && law_regime_words(hl) {
                    hc = Some((home(), None));
                    break;
                }
            }
            match (hc, recent) {
                (Some((c, n)), _) => (c, "heading", n),
                (None, Some((rl, c))) if ln - rl <= 12 && !cx.regime_page => (c, "context", None),
                _ => (cx.page_def, "page", (cx.page_def == "OTHER" && !cx.page_other.is_empty()).then(|| cx.page_other.clone())),
            }
        };
        let c_old = match pick(false) {
            Some((_, m)) => law_class(m.inst, l, cx.page_jur, cx.page_def),
            None => fallback().0,
        };
        if c_old == home() {
            raw.extend(sc.secs.iter().map(|s| format!("{}{s}", pv.key_act)));
        }
        // an instrument inside the citation's clause first, then one across a `;`, then context
        let (class, route, other) = match pick(true).or_else(|| pick(false)) {
            Some((_, m)) => {
                let c = law_class(m.inst, l, cx.page_jur, cx.page_def);
                (c, "line", (c == "OTHER").then(|| kother_name(l, m)))
            }
            None => fallback(),
        };
        // the other jurisdiction's instrument has no readable section list (no text layer): a number
        // the primary instrument has is the primary instrument's unless the line itself marks the
        // other jurisdiction
        let other_line = pv.other_line_marks_wide.iter().any(|w| l.contains(w.as_str()));
        let tier = if route == "line" { Tier::Fact } else { Tier::Derived };
        let (is_home, is_oth) = (class == home(), class == oth());
        for s in &sc.secs {
            if is_home && known.act.contains(s) {
                res.push((format!("{}{s}", pv.key_act), tier, route));
            } else if (is_home || class == "TRULES") && tr_ctx && known.trules.contains(s) {
                res.push((format!("{}{s}", pv.key_trules), Tier::Derived, "tr-context"));
            } else if class == "TRULES" && known.trules.contains(s) {
                res.push((format!("{}{s}", pv.key_trules), tier, route));
            } else if is_oth && !other_line && known.act.contains(s) {
                res.push((format!("{}{s}", pv.key_act), Tier::Derived, t("route_home_valid_over_other")));
            } else if is_oth {
                res.push((format!("{}{s}", pv.key_other_act), Tier::Derived, if route == "line" { t("route_other_line_unvalidated") } else { t("route_other_unvalidated") }));
            } else if class == "OTHER" && other.as_ref().is_some_and(|n| pv.regime_name_frags.iter().any(|f| n.contains(f.as_str()))) {
                // a regime-instrument spelling the earlier pattern list lacks is the home or the
                // other jurisdiction's instrument by the line's jurisdiction marks
                let (a, p) = law_marks(l);
                if p > a || (p == a && cx.page_jur == oth()) || pv.other_line_marks.iter().any(|w| l.contains(w.as_str())) {
                    res.push((format!("{}{s}", pv.key_other_act), Tier::Derived, t("route_name_other")));
                } else if known.act.contains(s) {
                    res.push((format!("{}{s}", pv.key_act), Tier::Derived, t("route_name_home")));
                }
            } else if class == "OTHER" {
                match &other {
                    Some(n) if !n.is_empty() => res.push((format!("law:OTHER:{n}:{s}"), tier, "other-named")),
                    _ => kbump(stats, "k_cite_other_unnamed_dropped"),
                }
            } else if (class.is_empty() || class == "UNRESOLVED") && code && known.act.contains(s) && !l[sc.start..].to_ascii_lowercase().starts_with("section") {
                // in a code comment a bare `s123` defaults to the primary instrument; a bare
                // "section 6" is a section of some doc (`contracts/README.md section 6`), never read
                // as the instrument's
                res.push((format!("{}{s}", pv.key_act), Tier::Heuristic, "code-default"))
            } else if is_home {
                kbump(stats, t("stat_home_not_a_section"));
            } else {
                kbump(stats, format!("k_cite_{}_dropped", if class.is_empty() { "unresolved" } else { class }));
            }
        }
    }
    for p in rules_cites(l) {
        if known.rules.contains(&p) {
            res.push((format!("{}{p}", pv.key_rules), Tier::Fact, "rules-line"));
        } else {
            kbump(stats, "k_cite_rules_unknown_dropped");
        }
    }
    (res, raw)
}

// ---- the Kina goal layer as key mentions ----

/// Spec-kit user stories a line names: `US1`, `[US2]`, `User Story 3`.
fn us_ids(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len() {
        if b[i] == b'U' && i + 2 < b.len() && b[i + 1] == b'S' && wb(b, i) {
            let e = digits_at(b, i + 2, 3);
            if e > i + 2 && e - i - 2 <= 2 && we(b, e) {
                out.push(format!("US{}", &s[i + 2..e]));
            }
        }
    }
    let mut from = 0;
    while let Some(p) = s[from..].find("User Story ") {
        let st = from + p + "User Story ".len();
        let e = digits_at(b, st, 3);
        if e > st && we(b, e) {
            out.push(format!("US{}", &s[st..e]));
        }
        from = st;
    }
    out
}

/// Id mentions on a line with byte positions: (start, end, id, bare).
fn kid_mentions(l: &str, vocab_free: bool) -> Vec<(usize, usize, String, bool)> {
    let bare: BTreeSet<u8> = bare_kinds();
    let mut v: Vec<(String, bool)> = hyphen_ids(l).into_iter().filter(|(p, _)| vocab_free || !any_eq(&prv().id_standards, p)).map(|(_, id)| (id, false)).collect();
    v.extend(bare_ids(l, &bare).into_iter().map(|(_, id)| (id, true)));
    v.extend(us_ids(l).into_iter().map(|id| (id, true)));
    let mut out = Vec::new();
    let mut used: HashSet<usize> = HashSet::new();
    for (id, b) in v {
        let mut from = 0;
        while let Some(p) = l[from..].find(&id) {
            let s = from + p;
            from = s + id.len();
            if used.insert(s) {
                out.push((s, s + id.len(), id.clone(), b));
                break;
            }
        }
    }
    out.sort_by_key(|x| x.0);
    out
}

/// Identity lineage phrases (an absorption such as "folded into" is not one): the phrase,
/// its name, and whether the id it names is the NEW one (`superseded by X`) rather than the
/// old one (`supersedes X`).
const KLIN_PHRASES: &[(&str, &str, bool)] = &[
    ("(was ", "was", false),
    ("formerly ", "formerly", false),
    ("previously ", "previously", false),
    ("renamed from ", "renamed", false),
    ("renumbered from ", "renumbered", false),
    ("supersedes ", "supersedes", false),
    ("replaces ", "replaces", false),
    ("split from ", "split-from", false),
    ("superseded by ", "superseded-by", true),
    ("replaced by ", "replaced-by", true),
    ("(now ", "now", true),
    (" is now ", "now", true),
    ("split into ", "split-into", true),
];

/// Id lineage phrases on a line: (the id just before the phrase, or None for the block's
/// own definition; the id the phrase names; the phrase).
/// A raw lineage mention: (the id before the phrase, or None for the block's own definition;
/// the id the phrase names; the phrase). Ids are (id, bare).
type KLinRaw = (Option<(String, bool)>, (String, bool), &'static str);

fn klin_phrases(l: &str, ids: &[(usize, usize, String, bool)]) -> Vec<KLinRaw> {
    let lower = l.to_ascii_lowercase();
    let mut out = Vec::new();
    for (ph, name, _) in KLIN_PHRASES {
        let mut from = 0;
        while let Some(p) = lower[from..].find(ph) {
            let s = from + p;
            let e = s + ph.len();
            from = e;
            let Some(obj) = ids.iter().find(|x| x.0 >= e && x.0 <= e + 3) else { continue };
            let subj = ids.iter().rev().find(|x| x.1 <= s && s - x.1 <= 40).map(|x| (x.2.clone(), x.3));
            out.push((subj, (obj.2.clone(), obj.3), *name));
        }
    }
    out
}

/// The section tag a numbered table's rows are keyed under: its heading's kind word.
fn row_tag(head: &str) -> &'static str {
    let h = head.to_lowercase();
    // whole words, singular or plural (`screening` is not `screen`)
    let words: Vec<&str> = h.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    for (w, t) in [
        ("decision", "decision"),
        ("question", "question"),
        ("finding", "finding"),
        ("gap", "gap"),
        ("claim", "claim"),
        ("check", "check"),
        ("risk", "risk"),
        ("requirement", "requirement"),
        ("screen", "screen"),
        ("step", "step"),
    ] {
        if words.iter().any(|x| *x == w || x.strip_suffix('s') == Some(w)) {
            return t;
        }
    }
    "table"
}

/// Row references in running text: `decision 13`, `decisions 3 and 4`, `rows 5, 6, 16`,
/// `row #14`, ranges `1 to 33` (tag `*` = any numbered table).
fn row_refs(line: &str) -> Vec<(&'static str, Vec<u32>)> {
    let lower = line.to_ascii_lowercase();
    let b = lower.as_bytes();
    let mut out = Vec::new();
    for (w, tag) in [("decisions", "decision"), ("decision", "decision"), ("rows", "*"), ("row", "*")] {
        let mut from = 0;
        while let Some(p) = lower[from..].find(w) {
            let s = from + p;
            let mut e = s + w.len();
            from = e;
            if !wb(b, s) || e >= b.len() || b[e] != b' ' {
                continue;
            }
            e += 1;
            if e < b.len() && b[e] == b'#' {
                e += 1;
            }
            let ns = knum_list(&lower, e);
            if !ns.is_empty() {
                out.push((tag, ns));
            }
        }
    }
    out
}

/// A list of small numbers from byte `e`: `5, 6, 16 and 18`, `1 to 33`, `3-4`, `#8 and #9`.
fn knum_list(s: &str, mut e: usize) -> Vec<u32> {
    let b = s.as_bytes();
    let mut nums = Vec::new();
    let mut range_from: Option<u32> = None;
    loop {
        let d = digits_at(b, e, 3);
        if d == e || d - e > 2 || (d < b.len() && (b[d].is_ascii_alphanumeric() || b[d] == b'.' && d + 1 < b.len() && b[d + 1].is_ascii_digit())) {
            break;
        }
        let n: u32 = s[e..d].parse().unwrap_or(0);
        match range_from.take() {
            Some(a) if n > a && n - a <= 40 => nums.extend(a + 1..=n),
            _ => nums.push(n),
        }
        let rest = &s[d..];
        let Some((len, rg)) = [(", ", false), (" and ", false), (" & ", false), ("/", false), (" to ", true), ("\u{2013}", true), ("-", true), (",", false)]
            .iter()
            .find(|(x, _)| rest.starts_with(x))
            .map(|(x, r)| (x.len(), *r))
        else {
            break;
        };
        if rg {
            range_from = Some(n);
        }
        e = d + len;
        if e < b.len() && b[e] == b'#' {
            e += 1;
        }
    }
    nums
}

/// A numbered table's id letter, from the text between its heading and the table:
/// `Q1–19 were put to ...` makes rows `| 1 |` .. define Q1 ..; None when nothing names one.
fn row_letter(intro: &str, rows: &[u32]) -> Option<char> {
    let b = intro.as_bytes();
    let mut hits: BTreeMap<char, usize> = BTreeMap::new();
    for i in 0..b.len() {
        if !b[i].is_ascii_uppercase() || !wb(b, i) {
            continue;
        }
        let d = digits_at(b, i + 1, 3);
        if d == i + 1 {
            continue;
        }
        let lo: u32 = intro[i + 1..d].parse().unwrap_or(0);
        let mut hi = lo;
        let rest = &intro[d..];
        for sep in ["\u{2013}", "-", " to "] {
            if let Some(r) = rest.strip_prefix(sep) {
                let rb = r.as_bytes();
                let mut k = 0;
                if !rb.is_empty() && rb[0] == b[i] {
                    k = 1;
                }
                let e = digits_at(rb, k, 3);
                if e > k {
                    hi = r[k..e].parse().unwrap_or(lo);
                }
                break;
            }
        }
        if d < b.len() && b[d].is_ascii_alphanumeric() {
            continue;
        }
        let n = rows.iter().filter(|&&r| r >= lo && r <= hi.max(lo)).count();
        if n > 0 {
            *hits.entry(b[i] as char).or_default() += n;
        }
    }
    hits.into_iter().max_by_key(|x| x.1).map(|x| x.0)
}

/// Doc references a line makes that whitespace splitting breaks: backtick spans and markdown
/// link targets (`%20` read as a space), so `reports/A B.md` is one reference.
fn kdoc_refs(l: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut it = l.split('`');
    it.next();
    while let Some(span) = it.next() {
        out.push(span.to_string());
        it.next();
    }
    let mut from = 0;
    while let Some(p) = l[from..].find("](") {
        let st = from + p + 2;
        let Some(e) = l[st..].find(')') else { break };
        out.push(l[st..st + e].split('#').next().unwrap_or("").replace("%20", " "));
        from = st + e;
    }
    out
}

/// The spec numbers that qualify the id at bytes `s..e` of a line: only those written next to it
/// (`012 FR-060`, `spec 013 T011`), not any spec the line happens to name.
fn kqualifier(l: &str, s: usize, e: usize) -> Vec<u32> {
    let mut a = s.saturating_sub(16);
    while !l.is_char_boundary(a) {
        a -= 1;
    }
    let mut b = (e + 5).min(l.len());
    while !l.is_char_boundary(b) {
        b += 1;
    }
    spec_refs(&l[a..b])
}

/// Whether a line's leading id is a definition. A list item defines one only when the id is
/// bold, a checkbox task or followed by a colon (spec-kit's `- **FR-001**:`, `- [ ] T001`); a
/// plain item that starts with an id (`- D4 / L-A4 "..." Verdict: ...`) reviews it.
fn kdef_list_ok(l: &str) -> bool {
    let t = l.trim_start();
    let body = if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")).or_else(|| t.strip_prefix("+ ")) {
        r
    } else {
        let d = t.bytes().take_while(u8::is_ascii_digit).count();
        if d > 0 && t[d..].starts_with(". ") {
            &t[d + 2..]
        } else {
            return true;
        }
    };
    let body = body.trim_start();
    if body.starts_with("[ ]") || body.starts_with("[x]") || body.starts_with("[X]") || body.starts_with("**") || body.starts_with('`') {
        return true;
    }
    let head: String = body.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let rest = &body[head.len()..];
    rest.starts_with(':') || rest.starts_with(" \u{2014}")
}

/// Raw (unresolved) id mention.
#[derive(Clone)]
struct KRawId {
    id: String,
    bare: bool,
    specs: Vec<u32>,
    line: u32,
}

/// One block of a doc page: a heading, a definition line to the next one, a table row, or
/// the text that follows a table. Raw mentions first (pass A), resolved keys after (pass C).
#[derive(Default)]
struct KBlock {
    page: u32,
    unit: u32,
    kind: u8,
    line: u32,
    raw_defs: Vec<(String, bool)>,
    row: Option<String>,
    raw_ids: Vec<KRawId>,
    raw_rows: Vec<(&'static str, Vec<u32>, u32)>,
    /// raw lineage mentions and the line each is on
    raw_lin: Vec<(KLinRaw, u32)>,
    ptoks: Vec<(String, u32)>,
    files_raw: Vec<u32>,
    cites: Vec<(String, Tier, &'static str, u32)>,
    cites_raw: Vec<String>,
    text: String,
    defs: Vec<(KT, String)>,
    refs: Vec<KRef>,
    raw_id_keys: Vec<String>,
    files: Vec<(u32, Tier, &'static str)>,
    text_files: Vec<(u32, f64)>,
    /// Round 2: the resolved keys this block mentions in the `implements` role
    impl_keys: Vec<String>,
}

/// Round 2: a doc mention's role is `implements` when its block is a checkbox task, or an
/// implementing verb stands just before it (`implements FR-002`, `per D4`); otherwise `cites`.
fn krole_implements(b: &KBlock, line: &str, id: &str) -> bool {
    if kblock_state(b).is_some() {
        return true;
    }
    let Some(p) = line.find(id) else { return false };
    let mut a = p.saturating_sub(24);
    while !line.is_char_boundary(a) {
        a -= 1;
    }
    let before = line[a..p].to_ascii_lowercase();
    let before = before.trim_end_matches(['(', '[', ' ', '*', '`']);
    ["implements", "implementing", "implement", "satisfies", "covers", "addresses", "fixes", "closes", "resolves", "per", "delivers"]
        .iter()
        .any(|v| before.ends_with(v) && before[..before.len() - v.len()].chars().last().is_none_or(|c| !c.is_alphanumeric()))
}

struct KPage {
    path: String,
    scope: String,
    family: String,
    start: usize,
}

struct KinaModel {
    pages: Vec<KPage>,
    heads: Vec<(u32, String)>,
    head_parent: Vec<Option<u32>>,
    blocks: Vec<KBlock>,
    code_refs: Vec<Vec<KRef>>,
    code_raw_ids: Vec<Vec<String>>,
    code_cites_raw: Vec<Vec<String>>,
    code_specs: Vec<Vec<String>>,
    lineage: Vec<(String, String, Tier, &'static str)>,
    /// ids a doc's title heading defines (`# ABC-03 ...`): they name a document, so a table
    /// cell or phrase naming one cites the document, never an id the row supersedes
    doc_ids: HashSet<String>,
    stats: BTreeMap<String, f64>,
    audit: Vec<KAudit>,
    /// Round 2: the id scopes pass B built (the subject's ids resolve against them).
    kids: Option<KIds>,
    /// Round 2 (`ns`): every bare-id mention's (at, mention, base key, ns key, ns route), for the audit
    ns_audit: Vec<(String, String, String, String, &'static str)>,
}

/// Round 2 (`ns`): a bare-id series is the ids one page defines under one letter; its namespace
/// is the page (in a spec, `<spec>/<file>`), so "series X's D6" is a key, never a bare D6. A
/// findings series (the page or the headings over it name an analysis / review, none of them a
/// decision) is local to its page; a series restated on another page (3+ shared ids whose rows
/// share words) is one namespace with it.
struct KNs {
    /// (page, id) -> the namespace of that page's definition
    def: HashMap<(usize, String), String>,
    /// id -> its series: (namespace, page-local, spec scope, defining page)
    by_id: HashMap<String, Vec<KSer>>,
    /// namespace -> the page scopes it spans
    scopes: HashMap<String, BTreeSet<String>>,
    /// (namespace, id) -> the defining row's text (the text-rank step reads it)
    rows: HashMap<(String, String), String>,
    stats: BTreeMap<String, f64>,
}

/// One bare-id series: (namespace, page-local findings series, spec scope, defining page).
type KSer = (String, bool, Option<String>, usize);

/// A mention's (base, namespaced) resolutions.
type KResPair = (Result<KRes, &'static str>, Result<KRes, &'static str>);

/// The context of one bare mention, strongest evidence first.
struct KNsCtx<'a> {
    own_page: Option<usize>,
    /// specs written next to the id
    line: Vec<String>,
    /// the page's own spec (a doc page inside a spec)
    own_spec: Option<String>,
    /// the doc pages this page names
    named: Vec<String>,
    /// specs the page (or the code file) names
    file: Vec<String>,
    /// code: the specs whose docs name the file
    path: Vec<String>,
    text: &'a str,
}

const KNS_LOCAL: &[&str] = &["analysis", "finding", "remediation", "review", "audit"];

impl KNs {
    fn build(m: &KinaModel) -> KNs {
        let mut stats0: BTreeMap<String, f64> = BTreeMap::new();
        let mut ser: BTreeMap<(usize, u8), BTreeMap<String, String>> = BTreeMap::new();
        let mut local: HashMap<(usize, u8), bool> = HashMap::new();
        for b in &m.blocks {
            // a table row defines an id only when its key cell is the id alone (`| D7 |`); a cell
            // such as `D7 rollout` labels an item about D7
            let first = b.text.lines().next().unwrap_or("");
            let key_cell = first.trim_start().starts_with('|').then(|| kcells(first).into_iter().next().unwrap_or_default());
            for (id, bare) in &b.raw_defs {
                if !*bare || id.is_empty() {
                    continue;
                }
                if let Some(c) = &key_cell
                    && c.trim_matches(|x: char| x == '*' || x == '`' || x == ' ') != id
                {
                    kbump(&mut stats0, "k_ns_label_row_not_a_definition");
                    continue;
                }
                let (pi, letter) = (b.page as usize, id.as_bytes()[0]);
                let row = b.text.lines().next().unwrap_or("").replace(id.as_str(), " ");
                ser.entry((pi, letter)).or_default().entry(id.clone()).or_insert(row);
                let mut chain = String::new();
                let mut nearest: Option<String> = None;
                let mut u = Some(b.unit);
                while let Some(x) = u {
                    let h = m.heads[x as usize].1.rsplit(" \u{a7} ").next().unwrap_or("").to_ascii_lowercase();
                    if nearest.is_none() && h != "(top)" {
                        nearest = Some(h.clone());
                    }
                    chain.push_str(&h);
                    chain.push(' ');
                    u = m.head_parent[x as usize];
                }
                let file = m.pages[pi].path.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
                let words = format!("{file} {chain}");
                let is_local = KNS_LOCAL.iter().any(|w| words.contains(w)) && !nearest.as_deref().unwrap_or("").contains("decision");
                let e = local.entry((pi, letter)).or_insert(is_local);
                *e = *e && is_local;
            }
        }
        let keys: Vec<(usize, u8)> = ser.keys().copied().collect();
        let name0 = |pi: usize| -> String {
            let p = &m.pages[pi];
            if p.scope.starts_with("specs/") { format!("{}/{}", p.scope, p.path.rsplit('/').next().unwrap_or("")) } else { p.path.clone() }
        };
        let mut parent: Vec<usize> = (0..keys.len()).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        let mut stats = stats0;
        for i in 0..keys.len() {
            for j in i + 1..keys.len() {
                let (a, b) = (keys[i], keys[j]);
                if a.1 != b.1 || a.0 == b.0 || local[&a] || local[&b] {
                    continue;
                }
                let (sa, sb) = (&ser[&a], &ser[&b]);
                let shared: Vec<&String> = sa.keys().filter(|k| sb.contains_key(*k)).collect();
                if shared.len() < 3 {
                    continue;
                }
                let dice: f64 = shared.iter().map(|k| law_dice(&sa[*k], &sb[*k])).sum::<f64>() / shared.len() as f64;
                if dice >= 0.3 {
                    let (ra, rb) = (find(&mut parent, i), find(&mut parent, j));
                    if ra != rb {
                        parent[rb] = ra;
                        kbump(&mut stats, "k_ns_restated_merges");
                    }
                }
            }
        }
        // a component's namespace: its spec member's (least), else its least name
        let mut comp: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for i in 0..keys.len() {
            let r = find(&mut parent, i);
            comp.entry(r).or_default().push(i);
        }
        let mut def = HashMap::new();
        let mut rows: HashMap<(String, String), String> = HashMap::new();
        let mut by_id: HashMap<String, Vec<KSer>> = HashMap::new();
        let mut scopes: HashMap<String, BTreeSet<String>> = HashMap::new();
        for members in comp.values() {
            let spec_m: Option<&usize> = members.iter().filter(|&&i| m.pages[keys[i].0].scope.starts_with("specs/")).min_by_key(|&&i| name0(keys[i].0));
            let name = match spec_m {
                Some(&i) => name0(keys[i].0),
                None => members.iter().map(|&i| name0(keys[i].0)).min().unwrap_or_default(),
            };
            let spec = spec_m.map(|&i| m.pages[keys[i].0].scope.clone());
            for &i in members {
                let (pi, _) = keys[i];
                scopes.entry(name.clone()).or_default().insert(m.pages[pi].scope.clone());
                for (id, row) in &ser[&keys[i]] {
                    rows.entry((name.clone(), id.clone())).or_insert_with(|| row.clone());
                    def.insert((pi, id.clone()), name.clone());
                    let v = by_id.entry(id.clone()).or_default();
                    if !v.iter().any(|x| x.0 == name) {
                        v.push((name.clone(), local[&keys[i]], spec.clone(), pi));
                    }
                }
            }
            kbump(&mut stats, if members.iter().all(|&i| local[&keys[i]]) { "k_ns_series_local" } else { "k_ns_series_exported" });
        }
        if let Ok(want) = std::env::var("GLIA_NS_DEBUG") {
            eprintln!("[ns-debug] {want}: {:?}", by_id.get(&want));
        }
        KNs { def, by_id, scopes, rows, stats }
    }

    /// Same-line siblings: `D2/D5` names one series. A mention resolved by a weak route takes the
    /// namespace a strongly resolved sibling (same letter, same line) got, when it defines the id.
    fn siblings(&self, ments: &[(u32, &str)], res: &mut [Result<KRes, &'static str>]) {
        let weak = |r: &str| matches!(r, "ns-path-scope" | "ns-text-rank");
        let ns_of = |k: &str| k.strip_prefix("id:").and_then(|x| x.rsplit_once('|')).map(|x| x.0.to_string());
        for i in 0..ments.len() {
            let Ok((_, _, r)) = &res[i] else { continue };
            if !weak(r) {
                continue;
            }
            let (line, id) = ments[i];
            let mut got: Option<String> = None;
            for j in 0..ments.len() {
                if j == i || ments[j].0 != line || ments[j].1.as_bytes().first() != id.as_bytes().first() {
                    continue;
                }
                if let Ok((k, _, rj)) = &res[j]
                    && !weak(rj)
                    && rj.starts_with("ns-")
                    && let Some(n) = ns_of(k)
                    && self.by_id.get(id).is_some_and(|v| v.iter().any(|x| x.0 == n))
                {
                    got = Some(n);
                    break;
                }
            }
            if let Some(n) = got {
                res[i] = Ok((format!("id:{n}|{id}"), Tier::Derived, "ns-sibling"));
            }
        }
    }

    /// A bare mention under the namespace rules, strongest evidence first: its own page's series;
    /// else among the series visible here (a local findings series only on its page): one in a spec
    /// written next to it, the base resolution when it came from the same line (`cocite*`), the
    /// page's own spec, a doc the page names, a spec the page or file names, the one global
    /// (non-spec) series, a spec whose docs name the file; else (HEURISTIC) the candidate whose
    /// defining row shares the most words with the mention's line.
    fn resolve(&self, id: &str, cx: &KNsCtx, base: Result<KRes, &'static str>) -> Result<KRes, &'static str> {
        let key = |n: &str| format!("id:{n}|{id}");
        if let Some(pi) = cx.own_page
            && let Some(n) = self.def.get(&(pi, id.to_string()))
        {
            return Ok((key(n), Tier::Fact, "ns-own-page"));
        }
        let Some(all) = self.by_id.get(id) else { return base };
        // a findings series is visible on its page and the other pages of its spec
        let vis: Vec<&KSer> =
            all.iter().filter(|x| !x.1 || Some(x.3) == cx.own_page || (x.2.is_some() && x.2 == cx.own_spec)).collect();
        if vis.is_empty() {
            return Err("ns-local-only");
        }
        if vis.len() == 1 {
            return Ok((key(&vis[0].0), Tier::Derived, "ns-unique"));
        }
        let in_scope = |x: &KSer, c: &str| x.2.as_deref() == Some(c) || self.scopes.get(&x.0).is_some_and(|s| s.contains(c));
        // one series in the first context scope that has any; a decision series before a findings one
        let pick = |list: &[String]| -> Option<String> {
            for c in list {
                let hit: Vec<&&KSer> = vis.iter().filter(|x| in_scope(x, c)).collect();
                if hit.len() == 1 {
                    return Some(hit[0].0.clone());
                }
                let exported: Vec<&&&KSer> = hit.iter().filter(|x| !x.1).collect();
                if exported.len() == 1 {
                    return Some(exported[0].0.clone());
                }
            }
            None
        };
        if let Some(n) = pick(&cx.line) {
            return Ok((key(&n), Tier::Derived, "ns-line-spec"));
        }
        if let Ok((k, t, r)) = &base
            && (*r == "cocite" || *r == "cocite-dir")
            && let Some((sc, _)) = k.strip_prefix("id:").and_then(|x| x.rsplit_once('|'))
            && let Some(n) = pick(&[sc.to_string()])
        {
            return Ok((key(&n), *t, "ns-cocite"));
        }
        if let Some(n) = cx.own_spec.as_ref().and_then(|s| pick(std::slice::from_ref(s))) {
            return Ok((key(&n), Tier::Derived, "ns-own-spec"));
        }
        if let Some(n) = pick(&cx.named) {
            return Ok((key(&n), Tier::Derived, "ns-named-doc"));
        }
        if let Some(n) = pick(&cx.file) {
            return Ok((key(&n), Tier::Derived, "ns-file-spec"));
        }
        let global: Vec<&&KSer> = vis.iter().filter(|x| x.2.is_none()).collect();
        if global.len() == 1 {
            return Ok((key(&global[0].0), Tier::Derived, "ns-global"));
        }
        if let Some(n) = pick(&cx.path) {
            return Ok((key(&n), Tier::Derived, "ns-path-scope"));
        }
        // text as a ranker among the typed candidates (never a new candidate)
        let mut best: Option<(f64, String)> = None;
        for x in &vis {
            let row = self.rows.get(&(x.0.clone(), id.to_string())).map_or("", String::as_str);
            let d = law_dice(row, cx.text);
            if best.as_ref().is_none_or(|b| d > b.0 || (d == b.0 && x.0 < b.1)) {
                best = Some((d, x.0.clone()));
            }
        }
        let (_, n) = best.expect("two or more visible series");
        Ok((key(&n), Tier::Heuristic, "ns-text-rank"))
    }
}

/// The comment part of a code line (`//`, `/*`, `*`, `#`, `--`, `<!--`), where a citation
/// can sit; a code line's own text (`s1 := ..`) is never read as one.
fn kcomment(l: &str) -> Option<&str> {
    let t = l.trim_start();
    if t.starts_with("* ") || t.starts_with('#') || t.starts_with("--") || t.starts_with("*/") {
        return Some(t);
    }
    ["//", "/*", "<!--", " -- ", " # "].iter().filter_map(|m| l.find(m)).min().map(|i| &l[i..])
}

fn kexcerpt(s: &str) -> String {
    let t: String = s.trim().chars().take(220).collect();
    t.replace(['\t', '\n'], " ")
}

/// Id scopes: where each id is defined, which hyphen prefixes are numbered per spec or doc
/// (an id of the prefix is defined in two scopes), and the spec scopes by number.
struct KIds {
    defs: HashMap<String, BTreeSet<String>>,
    scoped: HashSet<String>,
    vocab: HashSet<String>,
    spec_scope: BTreeMap<u32, String>,
}

impl KIds {
    fn prefix(id: &str) -> &str {
        id.split('-').next().unwrap_or(id)
    }

    fn accepts(&self, id: &str, bare: bool) -> bool {
        bare || self.vocab.contains(KIds::prefix(id))
    }

    fn is_scoped(&self, id: &str, bare: bool) -> bool {
        bare || self.scoped.contains(KIds::prefix(id))
    }

    /// The scope a definition on a page lives in: its spec, or outside specs its doc (a plan's
    /// D4 and another review's D4 are two things).
    fn def_scope(page: &str, _bare: bool) -> &str {
        page
    }

    /// The rules that need nothing beyond the mention: a global id is itself; a scoped one is
    /// fixed by a spec written next to it, its own scope, or its only defining scope.
    /// `Ok(None)`: scoped and still ambiguous.
    fn resolve_direct(&self, id: &str, bare: bool, own: Option<&str>, qual: &[u32]) -> Result<Option<(String, Tier, &'static str)>, &'static str> {
        let own = own.map(|p| KIds::def_scope(p, bare));
        let defs = self.defs.get(id);
        if !self.is_scoped(id, bare) {
            return Ok(Some(match defs {
                Some(_) => (format!("id:{id}"), Tier::Fact, "global"),
                None => (format!("id:{id}"), Tier::Derived, "global-undefined"),
            }));
        }
        let key = |s: &str| format!("id:{s}|{id}");
        let empty = BTreeSet::new();
        let defs = defs.unwrap_or(&empty);
        let ls: BTreeSet<&String> = qual.iter().filter_map(|n| self.spec_scope.get(n)).collect();
        if ls.len() == 1 {
            let s = *ls.iter().next().unwrap();
            return Ok(Some(if defs.contains(s) { (key(s), Tier::Fact, "line-spec") } else { (key(s), Tier::Derived, "line-spec-undefined") }));
        }
        if defs.is_empty() {
            return match own {
                Some(o) if !bare => Ok(Some((key(o), Tier::Derived, "own-undefined"))),
                _ => Err("undefined"),
            };
        }
        if let Some(o) = own
            && defs.contains(o)
        {
            return Ok(Some((key(o), Tier::Fact, "own-scope")));
        }
        if defs.len() == 1 {
            return Ok(Some((key(defs.iter().next().unwrap()), Tier::Derived, "unique")));
        }
        Ok(None)
    }

    /// The context cascade for a scoped id still ambiguous: each step's hint scopes in order
    /// (a defining scope that is a hint; else the one defining doc in a hint doc's directory),
    /// then a spec the file names, then (HEURISTIC) the newest defining spec.
    fn resolve_hints(&self, id: &str, steps: &[(&[String], &'static str, &'static str)], file_specs: &[u32]) -> Result<(String, Tier, &'static str), &'static str> {
        let Some(defs) = self.defs.get(id) else { return Err("undefined") };
        let key = |s: &str| format!("id:{s}|{id}");
        for (hints, route, near_route) in steps {
            if let Some(h) = hints.iter().find(|h| defs.contains(*h)) {
                return Ok((key(h), Tier::Derived, route));
            }
            let dirs: BTreeSet<&str> = hints.iter().filter(|h| !h.starts_with("specs/")).map(|h| dir_of(h)).collect();
            let near: Vec<&String> = defs.iter().filter(|d| !d.starts_with("specs/") && dirs.contains(dir_of(d))).collect();
            if near.len() == 1 {
                return Ok((key(near[0]), Tier::Derived, near_route));
            }
        }
        let fs: Vec<&String> = file_specs.iter().filter_map(|n| self.spec_scope.get(n)).filter(|s| defs.contains(*s)).collect::<BTreeSet<_>>().into_iter().collect();
        if fs.len() == 1 {
            return Ok((key(fs[0]), Tier::Derived, "file-spec"));
        }
        match defs.iter().filter(|s| s.starts_with("specs/")).max() {
            Some(s) => Ok((key(s), Tier::Heuristic, "newest-spec")),
            None => Err("ambiguous"),
        }
    }

    /// Every id mention of one context (a doc page or a code file): the direct rules first; the
    /// direct resolutions (and global ids' defining docs) are then the hints for the rest, in
    /// order: the same line's, the specs whose tasks name the file (`extra`), the file's.
    fn resolve_all(&self, ments: &[&KRawId], own: Option<&str>, file_specs: &[u32], extra: &[String], extra_route: &'static str) -> Vec<Result<(String, Tier, &'static str), &'static str>> {
        let first: Vec<Result<Option<KRes>, &'static str>> = ments.iter().map(|m| self.resolve_direct(&m.id, m.bare, own, &m.specs)).collect();
        let mut line_h: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        let mut file_h: BTreeMap<String, usize> = BTreeMap::new();
        for (m, r) in ments.iter().zip(&first) {
            if let Ok(Some((k, _, rt))) = r {
                let scopes: Vec<String> = if rt.starts_with("global") {
                    self.defs.get(&m.id).map(|s| s.iter().cloned().collect()).unwrap_or_default()
                } else {
                    k.strip_prefix("id:").and_then(|x| x.rsplit_once('|')).map(|x| vec![x.0.to_string()]).unwrap_or_default()
                };
                for s in scopes {
                    line_h.entry(m.line).or_default().push(s.clone());
                    *file_h.entry(s).or_default() += 1;
                }
            }
        }
        let mut fh: Vec<(String, usize)> = file_h.into_iter().collect();
        fh.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        let fh: Vec<String> = fh.into_iter().map(|x| x.0).collect();
        let none: Vec<String> = Vec::new();
        ments
            .iter()
            .zip(first)
            .map(|(m, r)| match r {
                Ok(Some(x)) => Ok(x),
                Err(e) => Err(e),
                Ok(None) => {
                    let lh = line_h.get(&m.line).unwrap_or(&none);
                    self.resolve_hints(&m.id, &[(lh, "cocite", "cocite-dir"), (extra, extra_route, extra_route), (&fh, "cocite-file", "cocite-file-dir")], file_specs)
                }
            })
            .collect()
    }
}

fn kraw_id(k: &str) -> String {
    match k.strip_prefix("id:").and_then(|x| x.rsplit_once('|')) {
        Some((_, id)) => format!("id:{id}"),
        None => k.to_string(),
    }
}

/// The family a doc's numbered tables belong to: a doc in a directory named after another
/// doc (`reports/X.md` and `notes/X/a.md`) shares that doc's family; otherwise the doc itself.
fn kfamily(path: &str, stems: &HashSet<String>, dirs: &HashSet<String>) -> String {
    let parent = dir_of(path).rsplit('/').next().unwrap_or("");
    let stem = path.rsplit('/').next().unwrap_or("").trim_end_matches(".md");
    if !parent.is_empty() && stems.contains(parent) {
        format!("fam:{parent}")
    } else if dirs.contains(stem) {
        format!("fam:{stem}")
    } else {
        path.to_string()
    }
}

struct KTable {
    header: Vec<String>,
    tag: &'static str,
    intro: String,
    rows: Vec<(usize, Vec<String>, Option<u32>)>,
}

fn kcells(t: &str) -> Vec<String> {
    let t = t.trim().trim_start_matches('|').trim_end_matches('|');
    t.split('|').map(|c| c.trim().to_string()).collect()
}

/// Pass A over one page: blocks, tables and raw key mentions.
fn kina_page(
    m: &mut KinaModel,
    pi: usize,
    lines: &[&str],
    known: &LawKnown,
    pr: &PathRes,
) {
    let path = m.pages[pi].path.clone();
    let family = m.pages[pi].family.clone();
    let cx = kcite_ctx(&path, lines);
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut fence = false;
    let mut recent: Option<(usize, &'static str)> = None;
    let new_block = |m: &mut KinaModel, unit: u32, kind: u8, ln: usize| -> usize {
        m.blocks.push(KBlock { page: pi as u32, unit, kind, line: ln as u32, ..Default::default() });
        m.blocks.len() - 1
    };
    m.heads.push((pi as u32, format!("{path} \u{a7} (top)")));
    m.head_parent.push(None);
    let mut unit = (m.heads.len() - 1) as u32;
    let top = unit;
    let mut ustack: Vec<(usize, u32)> = Vec::new();
    let mut cur = new_block(m, unit, b'h', 0);
    let mut table: Option<KTable> = None;
    let mut group: Option<String> = None;
    let mut intro = String::new();
    let close = |m: &mut KinaModel, table: &mut Option<KTable>| {
        let Some(t) = table.take() else { return };
        let numbered: Vec<u32> = t.rows.iter().filter_map(|r| r.2).collect();
        if numbered.len() >= 2 {
            let letter = row_letter(&t.intro, &numbered);
            for (b, _, n) in &t.rows {
                if let Some(n) = n {
                    m.blocks[*b].row = Some(format!("row:{family}|{}|{n}", t.tag));
                    if let Some(c) = letter {
                        m.blocks[*b].raw_defs.push((format!("{c}{n}"), true));
                    }
                }
            }
        }
        // lineage columns: a header naming an earlier id, or (numbered rows) a column whose
        // cells mostly hold ids; at most 3 ids per cell (more is a list, not an identity)
        let ncol = t.header.len();
        for j in 1..ncol {
            let h = t.header[j].to_ascii_lowercase();
            let named = ["old", "was", "previous", "formerly", "former", "legacy", "replaces", "supersedes"].iter().any(|w| h.split(|c: char| !c.is_alphanumeric()).any(|x| x == *w));
            let filled: Vec<&(usize, Vec<String>, Option<u32>)> = t.rows.iter().filter(|r| r.1.get(j).is_some_and(|c| !c.is_empty())).collect();
            let with_ids = filled.iter().filter(|r| !kid_mentions(&r.1[j], false).iter().all(|x| x.3)).count();
            let id_col = numbered.len() >= 2 && filled.len() >= 2 && with_ids * 2 >= filled.len();
            if !(named || id_col) {
                continue;
            }
            for (b, cells, _) in &t.rows {
                let Some(cell) = cells.get(j) else { continue };
                // an id cell, not prose that cites ids (a review row quoting the doc it reviews)
                if cell.chars().count() > 60 {
                    continue;
                }
                let ids: Vec<(String, bool)> = kid_mentions(cell, false).into_iter().filter(|x| named || !x.3).map(|x| (x.2, x.3)).collect();
                if ids.is_empty() || ids.len() > 3 {
                    continue;
                }
                let ln = m.blocks[*b].line;
                for id in ids {
                    m.blocks[*b].raw_lin.push(((None, id, if named { "lineage-column" } else { "id-column" }), ln));
                }
            }
        }
        // a `Rows` / `Decisions` first column references rows of another table
        if t.header.first().is_some_and(|h| matches!(h.to_ascii_lowercase().as_str(), "rows" | "row" | "decisions" | "decision")) {
            for (b, cells, _) in &t.rows {
                let c0 = cells.first().map_or("", |c| c.split(';').next().unwrap_or(""));
                let cb = c0.as_bytes();
                let ln = m.blocks[*b].line;
                let mut ns = Vec::new();
                for i in 0..cb.len() {
                    if cb[i].is_ascii_digit() && wb(cb, i) && (i == 0 || cb[i - 1] != b'#') {
                        let e = digits_at(cb, i, 3);
                        if e - i <= 2 && we(cb, e) {
                            ns.push(c0[i..e].parse().unwrap_or(0));
                        }
                    }
                }
                if !ns.is_empty() {
                    m.blocks[*b].raw_rows.push(("*", ns, ln));
                }
            }
        }
    };
    for (ln, l) in lines.iter().enumerate() {
        let t = l.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        }
        let lvl = if !fence && t.starts_with('#') {
            let h = t.bytes().take_while(|&c| c == b'#').count();
            (h <= 6 && t[h..].starts_with(' ')).then_some(h)
        } else {
            None
        };
        let is_sep = |x: &str| x.trim().starts_with('|') && x.trim().chars().all(|c| matches!(c, '|' | '-' | ':' | ' '));
        if let Some(h) = lvl {
            close(m, &mut table);
            while stack.last().is_some_and(|s| s.0 >= h) {
                stack.pop();
            }
            stack.push((h, l.to_string()));
            recent = None;
            while ustack.last().is_some_and(|s| s.0 >= h) {
                ustack.pop();
            }
            m.heads.push((pi as u32, format!("{path} \u{a7} {}", t.trim_start_matches('#').trim())));
            m.head_parent.push(Some(ustack.last().map_or(top, |s| s.1)));
            unit = (m.heads.len() - 1) as u32;
            ustack.push((h, unit));
            cur = new_block(m, unit, b'h', ln);
            let mut defs: Vec<(String, bool)> = law_line_defs(l, None).into_iter().map(|id| {
                let bare = !id.contains('-');
                (id, bare)
            }).collect();
            if let Some(r) = t.trim_start_matches('#').trim().strip_prefix("User Story ") {
                let n: String = r.chars().take_while(char::is_ascii_digit).collect();
                if !n.is_empty() {
                    defs.push((format!("US{n}"), true));
                }
            }
            if h == 1 {
                m.doc_ids.extend(defs.iter().map(|d| d.0.clone()));
            }
            m.blocks[cur].raw_defs = defs;
            // `### 12. <title>` under `## Decisions` is decision row 12
            let ht = t.trim_start_matches('#').trim();
            let d = ht.bytes().take_while(u8::is_ascii_digit).count();
            if (1..=2).contains(&d) && ht[d..].starts_with(". ") && stack.len() >= 2 {
                let tag = row_tag(&stack[stack.len() - 2].1);
                if tag != "table" {
                    m.blocks[cur].row = Some(format!("row:{family}|{tag}|{}", &ht[..d]));
                }
            }
            intro.clear();
        } else if !fence && t.starts_with('|') {
            if is_sep(t) {
                continue;
            }
            let next_sep = lines.get(ln + 1).is_some_and(|x| is_sep(x));
            if next_sep {
                close(m, &mut table);
                let tag = stack.last().map_or("table", |s| row_tag(&s.1));
                table = Some(KTable { header: kcells(t), tag, intro: intro.clone(), rows: Vec::new() });
                continue;
            }
            if let Some(tb) = table.as_mut() {
                cur = new_block(m, unit, b'r', ln);
                let cells = kcells(t);
                // a group row (bold first cell, the rest empty) heads the rows after it
                if cells.first().is_some_and(|c| c.starts_with("**")) && cells.iter().skip(1).all(|c| c.is_empty()) {
                    group = Some(cells[0].trim_matches('*').to_string());
                }
                let n = cells.first().filter(|c| !c.is_empty() && c.len() <= 3 && c.chars().all(|x| x.is_ascii_digit())).and_then(|c| c.parse().ok());
                tb.rows.push((cur, cells, n));
                let defs = law_line_defs(l, None);
                m.blocks[cur].raw_defs = defs.into_iter().map(|id| {
                    let bare = !id.contains('-');
                    (id, bare)
                }).collect();
            }
        } else {
            close(m, &mut table);
            let defs = if fence || !kdef_list_ok(l) { Vec::new() } else { law_line_defs(l, None) };
            // a numbered item at the margin under a decisions-type heading is a row
            let d = l.bytes().take_while(u8::is_ascii_digit).count();
            let item_tag = stack.last().map_or("table", |s| row_tag(&s.1));
            if !fence && (1..=2).contains(&d) && l[d..].starts_with(". ") && item_tag != "table" {
                cur = new_block(m, unit, b'r', ln);
                m.blocks[cur].row = Some(format!("row:{family}|{item_tag}|{}", &l[..d]));
                m.blocks[cur].raw_defs = defs.iter().map(|id| (id.clone(), !id.contains('-'))).collect();
            } else if !defs.is_empty() {
                cur = new_block(m, unit, b'd', ln);
                m.blocks[cur].raw_defs = defs.into_iter().map(|id| {
                    let bare = !id.contains('-');
                    (id, bare)
                }).collect();
            } else if m.blocks[cur].kind == b'r' {
                cur = new_block(m, unit, b't', ln);
            }
            if intro.len() < 4000 {
                intro.push_str(l);
                intro.push('\n');
            }
        }
        // raw mentions on this line, into the current block
        let b = &mut m.blocks[cur];
        let ids = kid_mentions(l, false);
        for (st, en, id, bare) in &ids {
            b.raw_ids.push(KRawId { id: id.clone(), bare: *bare, specs: kqualifier(l, *st, *en), line: ln as u32 });
        }
        for x in klin_phrases(l, &ids) {
            b.raw_lin.push((x, ln as u32));
        }
        for (tag, ns) in row_refs(l) {
            b.raw_rows.push((tag, ns, ln as u32));
        }
        for tok in kpath_tokens(l) {
            b.ptoks.push((tok, ln as u32));
        }
        b.files_raw.extend(pr.files(l));
        if !t.starts_with('|') {
            group = None;
        }
        let ctx_stack: Vec<(usize, String)> = match &group {
            Some(g) => stack.iter().cloned().chain(std::iter::once((7, format!("####### {g}")))).collect(),
            None => stack.clone(),
        };
        let (res, raw) = kcite_line(l, ln, &KCiteAt { cx: &cx, stack: &ctx_stack, recent }, known, false, &mut m.stats);
        for (k, tier, route) in res {
            m.blocks[cur].cites.push((k, tier, route, ln as u32));
        }
        m.blocks[cur].cites_raw.extend(raw);
        if m.blocks[cur].text.len() < 4000 {
            m.blocks[cur].text.push_str(l);
            m.blocks[cur].text.push('\n');
        }
        let ments = law_mentions(l);
        if let Some(mm) = ments.iter().filter(|x| !matches!(x.inst, LInst::Rules | LInst::TRules | LInst::TheAct)).max_by_key(|x| x.start) {
            recent = Some((ln, law_class(mm.inst, l, cx.page_jur, cx.page_def)));
        }
    }
    close(m, &mut table);
}

/// Builds the key model of one revision: pass A over every doc page and code file, pass B
/// the id scopes, pass C every mention resolved, pass D the text fallback.
#[allow(clippy::too_many_arguments)]
fn kina_model(
    docs: &[(String, String)],
    code_lines: &[(String, String)],
    kp: &KPaths,
    pr: &PathRes,
    known: &LawKnown,
    bm: Option<&KBm>,
    nf: usize,
    audit: bool,
    ns: bool,
) -> KinaModel {
    let mut m = KinaModel {
        pages: Vec::new(),
        heads: Vec::new(),
        head_parent: Vec::new(),
        blocks: Vec::new(),
        code_refs: vec![Vec::new(); nf],
        code_raw_ids: vec![Vec::new(); nf],
        code_cites_raw: vec![Vec::new(); nf],
        code_specs: vec![Vec::new(); nf],
        lineage: Vec::new(),
        doc_ids: HashSet::new(),
        stats: BTreeMap::new(),
        audit: Vec::new(),
        kids: None,
        ns_audit: Vec::new(),
    };
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for (i, (p, _)) in docs.iter().enumerate() {
        if groups.last().is_none_or(|g| docs[g.0].0 != *p) {
            groups.push((i, i));
        }
        groups.last_mut().unwrap().1 = i + 1;
    }
    let stems: HashSet<String> = groups.iter().map(|g| docs[g.0].0.rsplit('/').next().unwrap_or("").trim_end_matches(".md").to_string()).collect();
    let dirs: HashSet<String> = groups.iter().map(|g| dir_of(&docs[g.0].0).rsplit('/').next().unwrap_or("").to_string()).filter(|d| !d.is_empty()).collect();
    for &(s, e) in &groups {
        let path = docs[s].0.clone();
        m.pages.push(KPage { scope: kina_unit(&path), family: kfamily(&path, &stems, &dirs), path, start: s });
        let lines: Vec<&str> = docs[s..e].iter().map(|x| x.1.as_str()).collect();
        let pi = m.pages.len() - 1;
        kina_page(&mut m, pi, &lines, known, pr);
    }
    // ---- pass B: id scopes ----
    let mut ids = KIds { defs: HashMap::new(), scoped: HashSet::new(), vocab: HashSet::new(), spec_scope: BTreeMap::new() };
    for b in &m.blocks {
        let sc = &m.pages[b.page as usize].scope;
        for (id, bare) in &b.raw_defs {
            ids.defs.entry(id.clone()).or_default().insert(KIds::def_scope(sc, *bare).to_string());
            if !bare {
                ids.vocab.insert(KIds::prefix(id).to_string());
            }
        }
    }
    for p in &m.pages {
        if let Some(r) = p.scope.strip_prefix("specs/") {
            let num: String = r.chars().take_while(char::is_ascii_digit).collect();
            if num.len() == 3 {
                ids.spec_scope.insert(num.parse().unwrap_or(0), p.scope.clone());
            }
        }
    }
    // a hyphen prefix is numbered per spec (spec-kit's FR-002) when one id of it is defined in
    // two specs; restated in several plan docs, it stays one global id
    for (id, scopes) in &ids.defs {
        if id.contains('-') && scopes.iter().filter(|x| x.starts_with("specs/")).count() >= 2 {
            ids.scoped.insert(KIds::prefix(id).to_string());
        }
    }
    for p in ["FR", "SC"] {
        ids.scoped.insert(p.to_string());
    }
    // ---- pass B2 (round 2, `ns`): bare-id series namespaces ----
    let kns: Option<KNs> = ns.then(|| KNs::build(&m));
    if let Some(k) = &kns {
        for (a, b) in &k.stats {
            *m.stats.entry(a.clone()).or_default() += b;
        }
    }
    // ---- pass C: resolve ----
    let line_of = |m: &KinaModel, b: &KBlock, ln: u32| -> String {
        let p = &m.pages[b.page as usize];
        docs.get(p.start + ln as usize).map_or(String::new(), |x| kexcerpt(&x.1))
    };
    let at_of = |m: &KinaModel, b: &KBlock, ln: u32| format!("{}:{}", m.pages[b.page as usize].path, ln + 1);
    // rows index: (family, tag) -> numbers; and row key -> defining block
    let mut rows: HashMap<(String, String), BTreeSet<u32>> = HashMap::new();
    let mut row_pages: HashMap<(String, String), BTreeSet<usize>> = HashMap::new();
    let mut row_def: HashMap<String, usize> = HashMap::new();
    for (bi, b) in m.blocks.iter().enumerate() {
        if let Some(r) = &b.row
            && let Some(rest) = r.strip_prefix("row:")
            && let Some((ft, n)) = rest.rsplit_once('|')
            && let Some((f, t)) = ft.rsplit_once('|')
        {
            rows.entry((f.to_string(), t.to_string())).or_default().insert(n.parse().unwrap_or(0));
            row_pages.entry((f.to_string(), t.to_string())).or_default().insert(b.page as usize);
            row_def.entry(r.clone()).or_insert(bi);
        }
    }
    // the families of the docs each page names (`see reports/X.md`): an untyped "rows 5, 6"
    // means the rows of a doc the page points at before its own
    let mut page_suffix: HashMap<String, Vec<usize>> = HashMap::new();
    for (pi, pg) in m.pages.iter().enumerate() {
        let segs: Vec<&str> = pg.path.split('/').collect();
        for k in 0..segs.len() {
            page_suffix.entry(segs[k..].join("/")).or_default().push(pi);
        }
    }
    let mut named_fams: Vec<BTreeSet<String>> = vec![BTreeSet::new(); m.pages.len()];
    let mut named_pages: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); m.pages.len()];
    for b in &m.blocks {
        for tok in b.ptoks.iter().map(|x| x.0.clone()).chain(b.text.lines().flat_map(kdoc_refs)) {
            let t = tok.trim_matches(|c: char| matches!(c, '*' | '_' | ',' | ':')).trim_start_matches("./").trim_start_matches(".wiki/").trim_start_matches('/');
            let t = if t.ends_with(".md") { t.to_string() } else { format!("{t}.md") };
            let t = t.as_str();
            if t.ends_with(".md")
                && let Some(v) = page_suffix.get(t)
                && v.len() == 1
                && v[0] != b.page as usize
            {
                named_fams[b.page as usize].insert(m.pages[v[0]].family.clone());
                named_pages[b.page as usize].insert(v[0]);
            }
        }
    }
    // one reference list (`rows 5, 6, 16`, `decisions 1-36`) belongs to one table: the one
    // holding the most of its numbers, among the docs the page names, then its own family,
    // then any; a tie is no resolution
    let resolve_rows = |tag: &str, nums: &[u32], fam: &str, named: &BTreeSet<String>, npages: &BTreeSet<usize>| -> Option<((String, String), Tier, &'static str)> {
        let best = |f: &dyn Fn(&(String, String)) -> bool| -> Option<(String, String)> {
            let mut sc: Vec<(usize, &(String, String))> = rows
                .iter()
                .filter(|(k, _)| f(k) && (tag == "*" || k.1 == tag))
                .map(|(k, ns)| (nums.iter().filter(|n| ns.contains(n)).count(), k))
                .filter(|x| x.0 > 0)
                .collect();
            sc.sort_by_key(|x| std::cmp::Reverse(x.0));
            match sc.as_slice() {
                [] => None,
                [a] => Some(a.1.clone()),
                [a, b, ..] if a.0 > b.0 => Some(a.1.clone()),
                _ => None,
            }
        };
        if tag != "*"
            && let Some(k) = best(&|k| k.0 == fam)
        {
            return Some((k, Tier::Fact, "own-family"));
        }
        // the tables on a page this page names, then the rest of its family
        if let Some(k) = best(&|k| row_pages.get(k).is_some_and(|ps| ps.iter().any(|p| npages.contains(p)))) {
            return Some((k, Tier::Derived, "named-page"));
        }
        if let Some(k) = best(&|k| named.contains(&k.0)) {
            return Some((k, Tier::Derived, "named-family"));
        }
        if tag == "*"
            && let Some(k) = best(&|k| k.0 == fam)
        {
            return Some((k, Tier::Derived, "own-family"));
        }
        best(&|_| true).map(|k| (k, Tier::Derived, "unique-family"))
    };
    let mut audit_v: Vec<KAudit> = Vec::new();
    let pages_n = m.pages.len();
    let mut by_page: Vec<Vec<usize>> = vec![Vec::new(); pages_n];
    for (bi, b) in m.blocks.iter().enumerate() {
        by_page[b.page as usize].push(bi);
    }
    for pi in 0..pages_n {
        let scope = m.pages[pi].scope.clone();
        let family = m.pages[pi].family.clone();
        let own: Option<&str> = Some(scope.as_str());
        let page_specs: Vec<u32> = by_page[pi].iter().flat_map(|&bi| m.blocks[bi].text.lines().flat_map(spec_refs).collect::<Vec<_>>()).collect();
        // ids: definitions are keys in the page scope (global prefixes stay global)
        let ments: Vec<(usize, KRawId)> = by_page[pi]
            .iter()
            .flat_map(|&bi| m.blocks[bi].raw_ids.iter().filter(|r| ids.accepts(&r.id, r.bare)).map(move |r| (bi, r.clone())))
            .collect();
        let refs: Vec<&KRawId> = ments.iter().map(|x| &x.1).collect();
        // the docs the page names (`[DECISIONS](plans/decisions)`) scope the ids it reviews
        let named_scopes: Vec<String> = named_pages[pi].iter().map(|&q| m.pages[q].scope.clone()).collect();
        let res = ids.resolve_all(&refs, own, &page_specs, &named_scopes, "named-doc");
        let res: Vec<Result<KRes, &'static str>> = match &kns {
            None => res,
            Some(kn) => {
                let pairs: Vec<KResPair> = ments
                    .iter()
                    .zip(res)
                    .map(|((bi, rm), r)| {
                        if !rm.bare {
                            return (r.clone(), r);
                        }
                        let text = line_of(&m, &m.blocks[*bi], rm.line);
                        let cx = KNsCtx {
                            own_page: Some(pi),
                            line: rm.specs.iter().filter_map(|n| ids.spec_scope.get(n).cloned()).collect(),
                            own_spec: scope.starts_with("specs/").then(|| scope.clone()),
                            named: named_scopes.clone(),
                            file: page_specs.iter().filter_map(|n| ids.spec_scope.get(n).cloned()).collect(),
                            path: Vec::new(),
                            text: &text,
                        };
                        (r.clone(), kn.resolve(&rm.id, &cx, r))
                    })
                    .collect();
                let (base_v, mut ns_v): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                let keys: Vec<(u32, &str)> = ments.iter().map(|(_, rm)| (rm.line, rm.id.as_str())).collect();
                kn.siblings(&keys, &mut ns_v);
                if audit {
                    let show = |x: &Result<KRes, &'static str>| x.as_ref().map_or_else(|e| format!("ERR:{e}"), |v| format!("{} [{}]", v.0, v.2));
                    for (k, (bi, rm)) in ments.iter().enumerate() {
                        if rm.bare {
                            let b = &m.blocks[*bi];
                            let row = (at_of(&m, b, rm.line), format!("{} :: {}", rm.id, line_of(&m, b, rm.line)), show(&base_v[k]), show(&ns_v[k]), ns_v[k].as_ref().map_or("unresolved", |v| v.2));
                            m.ns_audit.push(row);
                        }
                    }
                }
                ns_v
            }
        };
        for ((bi, rm), r) in ments.iter().zip(res) {
            m.blocks[*bi].raw_id_keys.push(format!("id:{}", rm.id));
            match r {
                Ok((k, tier, route)) => {
                    if audit {
                        let b = &m.blocks[*bi];
                        audit_v.push(KAudit { kt: KT::Scoped, tier, route, at: at_of(&m, b, rm.line), mention: format!("{} :: {}", rm.id, line_of(&m, b, rm.line)), target: k.clone() });
                    }
                    kbump(&mut m.stats, format!("k_scoped_doc_{route}"));
                    let line = docs.get(m.pages[pi].start + rm.line as usize).map_or("", |x| x.1.as_str());
                    if krole_implements(&m.blocks[*bi], line, &rm.id) {
                        kbump(&mut m.stats, "k_role_implements");
                        m.blocks[*bi].impl_keys.push(k.clone());
                    } else {
                        kbump(&mut m.stats, "k_role_cites");
                    }
                    m.blocks[*bi].refs.push(KRef { kt: KT::Scoped, key: k, tier, route });
                }
                Err(e) => kbump(&mut m.stats, format!("k_scoped_doc_{e}")),
            }
        }
        for &bi in &by_page[pi] {
            let defs: Vec<(String, bool)> = m.blocks[bi].raw_defs.clone();
            for (id, bare) in defs {
                let k = match kns.as_ref().and_then(|kn| bare.then(|| kn.def.get(&(pi, id.clone()))).flatten()) {
                    Some(n) => format!("id:{n}|{id}"),
                    None if ids.is_scoped(&id, bare) => format!("id:{}|{id}", KIds::def_scope(&scope, bare)),
                    None => format!("id:{id}"),
                };
                m.blocks[bi].defs.push((KT::Scoped, k));
            }
            if let Some(r) = m.blocks[bi].row.clone() {
                m.blocks[bi].defs.push((KT::Row, r));
            }
            // rows referenced
            let rr = m.blocks[bi].raw_rows.clone();
            for (tag, nums, ln) in rr {
                let Some(((fa, tg), tier, route)) = resolve_rows(tag, &nums, &family, &named_fams[pi], &named_pages[pi]) else {
                    kbump(&mut m.stats, "k_row_ref_unresolved");
                    continue;
                };
                let have = &rows[&(fa.clone(), tg.clone())];
                for n in nums.iter().filter(|n| have.contains(n)) {
                    let k = format!("row:{fa}|{tg}|{n}");
                    {
                        if Some(&k) == m.blocks[bi].row.as_ref() {
                            continue;
                        }
                        if audit {
                            let b = &m.blocks[bi];
                            let def = row_def.get(&k).map(|&d| line_of(&m, &m.blocks[d], m.blocks[d].line)).unwrap_or_default();
                            audit_v.push(KAudit { kt: KT::Row, tier, route, at: at_of(&m, b, ln), mention: line_of(&m, b, ln), target: format!("{k} :: {def}") });
                        }
                        kbump(&mut m.stats, format!("k_row_ref_{route}"));
                        if kblock_state(&m.blocks[bi]).is_some() {
                            m.blocks[bi].impl_keys.push(k.clone());
                        }
                        m.blocks[bi].refs.push(KRef { kt: KT::Row, key: k, tier, route });
                    }
                }
            }
            // citations (resolved during pass A, where the heading context was)
            let cites = m.blocks[bi].cites.clone();
            if audit && let Ok(want) = std::env::var("GLIA_KEYS_CITE") {
                for (k, _, route, ln) in &cites {
                    if *k == want {
                        eprintln!("[keys-cite] {k} {route} {} :: {}", at_of(&m, &m.blocks[bi], *ln), line_of(&m, &m.blocks[bi], *ln));
                    }
                }
            }
            for (k, tier, route, ln) in cites {
                if audit {
                    let b = &m.blocks[bi];
                    let sec = k.rsplit(':').next().unwrap_or("");
                    let ver = if !k.starts_with(prv().key_act.as_str()) {
                        ""
                    } else if known.act_old.contains(sec) && known.act_new.contains(sec) {
                        t("ver_both")
                    } else if known.act_new.contains(sec) {
                        t("ver_new_only")
                    } else {
                        t("ver_old_only")
                    };
                    audit_v.push(KAudit { kt: KT::Cite, tier, route, at: at_of(&m, b, ln), mention: line_of(&m, b, ln), target: format!("{k}{ver}") });
                }
                kbump(&mut m.stats, format!("k_cite_{route}"));
                m.blocks[bi].refs.push(KRef { kt: KT::Cite, key: k, tier, route });
            }
            // paths
            let toks: Vec<String> = m.blocks[bi].ptoks.iter().map(|x| x.0.clone()).collect();
            let got = kp.block(&toks, &mut m.stats);
            let mut seen: HashMap<u32, usize> = HashMap::new();
            for (f, tier, route, raw) in got {
                kbump(&mut m.stats, format!("k_path_{route}"));
                if audit {
                    let b = &m.blocks[bi];
                    let ln = b.ptoks.iter().find(|x| x.0 == raw).map_or(b.line, |x| x.1);
                    audit_v.push(KAudit { kt: KT::Path, tier, route, at: at_of(&m, b, ln), mention: format!("{raw} :: {}", line_of(&m, b, ln)), target: kp.cand[f as usize].clone() });
                }
                match seen.get(&f) {
                    Some(&i) if m.blocks[bi].files[i].1 <= tier => {}
                    Some(&i) => m.blocks[bi].files[i] = (f, tier, route),
                    None => {
                        seen.insert(f, m.blocks[bi].files.len());
                        m.blocks[bi].files.push((f, tier, route));
                    }
                }
            }
            let fr = &mut m.blocks[bi].files_raw;
            fr.sort_unstable();
            fr.dedup();
        }
        // lineage (needs the page's resolved ids)
        for &bi in &by_page[pi] {
            let lin = m.blocks[bi].raw_lin.clone();
            for ((subj, (oid, obare), route), ln) in lin {
                if !ids.accepts(&oid, obare) {
                    continue;
                }
                if m.doc_ids.contains(&oid) {
                    kbump(&mut m.stats, "k_lineage_doc_id_skipped");
                    continue;
                }
                let lspecs = spec_refs(&line_of(&m, &m.blocks[bi], ln));
                let r = |id: &str, bare: bool| match ids.resolve_direct(id, bare, own, &lspecs) {
                    Ok(Some(x)) => Some(x.0),
                    Ok(None) => ids.resolve_hints(id, &[], &page_specs).ok().map(|x| x.0),
                    Err(_) => None,
                };
                let old = r(&oid, obare);
                let news: Vec<String> = match &subj {
                    Some((sid, sb)) if ids.accepts(sid, *sb) => r(sid, *sb).into_iter().collect(),
                    Some(_) => Vec::new(),
                    None => m.blocks[bi].defs.iter().map(|d| d.1.clone()).collect(),
                };
                let Some(old) = old else {
                    kbump(&mut m.stats, "k_lineage_unresolved");
                    continue;
                };
                // the phrase names the new id (`superseded by X`): the subject is the old one
                let names_new = KLIN_PHRASES.iter().any(|p| p.1 == route && p.2);
                for nk in news {
                    if nk == old {
                        continue;
                    }
                    let (new, old) = if names_new { (old.clone(), nk) } else { (nk, old.clone()) };
                    let tier = if route == "lineage-column" { Tier::Fact } else { Tier::Derived };
                    if audit {
                        let b = &m.blocks[bi];
                        audit_v.push(KAudit { kt: KT::Lineage, tier, route, at: at_of(&m, b, ln), mention: line_of(&m, b, ln), target: format!("new {new} <- old {old}") });
                    }
                    kbump(&mut m.stats, format!("k_lineage_{route}"));
                    m.lineage.push((new, old, tier, route));
                }
            }
        }
    }
    // a restated id: the same id defined in two docs (outside specs, whose numbering is per spec
    // by design) where one definition quotes the other (a review note quoting a plan's row):
    // a quoted or bold span of four or more words of one, its words all in the other
    {
        let spans = |t: &str| -> Vec<BTreeSet<String>> {
            let mut out = Vec::new();
            for sep in ["**", "\"", "\u{201c}"] {
                let close = if sep == "\u{201c}" { "\u{201d}" } else { sep };
                let mut rest = t;
                while let Some(a) = rest.find(sep) {
                    let after = &rest[a + sep.len()..];
                    let Some(b) = after.find(close) else { break };
                    let w: BTreeSet<String> = law_words(&after[..b]).into_iter().collect();
                    if w.len() >= 4 {
                        out.push(w);
                    }
                    rest = &after[b + close.len()..];
                }
            }
            out
        };
        let mut by_id: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for b in &m.blocks {
            let p = &m.pages[b.page as usize];
            if p.scope.starts_with("specs/") {
                continue;
            }
            let line = docs.get(p.start + b.line as usize).map_or("", |x| x.1.as_str());
            for (kt, k) in &b.defs {
                if *kt != KT::Scoped || !k.contains('|') {
                    continue;
                }
                let id = k.rsplit('|').next().unwrap_or("");
                by_id.entry(id.to_string()).or_default().entry(k.clone()).or_insert_with(|| line.replace(id, " "));
            }
        }
        for (_, defs) in by_id {
            let v: Vec<(&String, &String)> = defs.iter().collect();
            for i in 0..v.len() {
                for j in i + 1..v.len() {
                    let (wi, wj): (BTreeSet<String>, BTreeSet<String>) = (law_words(v[i].1).into_iter().collect(), law_words(v[j].1).into_iter().collect());
                    let quotes = spans(v[i].1).iter().any(|sp| sp.is_subset(&wj)) || spans(v[j].1).iter().any(|sp| sp.is_subset(&wi));
                    if quotes {
                        if audit {
                            audit_v.push(KAudit { kt: KT::Lineage, tier: Tier::Derived, route: "restated", at: v[j].0.clone(), mention: kexcerpt(v[j].1), target: format!("{} == {} :: {}", v[j].0, v[i].0, kexcerpt(v[i].1)) });
                        }
                        kbump(&mut m.stats, "k_lineage_restated");
                        m.lineage.push((v[j].0.clone(), v[i].0.clone(), Tier::Derived, "restated"));
                    }
                }
            }
        }
    }
    for (a, b) in &known.moved {
        let ka = &prv().key_act;
        m.lineage.push((format!("{ka}{b}"), format!("{ka}{a}"), Tier::Derived, "moved-section"));
        kbump(&mut m.stats, "k_lineage_moved-section");
    }
    // ---- code files ----
    let mut by_file: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
    let mut code_path: HashMap<u32, String> = HashMap::new();
    for (p, l) in code_lines {
        if let Some(f) = kp.map(p) {
            if !kp.exact.contains_key(p) {
                kbump(&mut m.stats, "k_code_lines_via_rename");
            }
            by_file.entry(f).or_default().push(l.as_str());
            code_path.entry(f).or_insert_with(|| p.clone());
        }
    }
    let spec_of_num = ids.spec_scope.clone();
    // per code file, the spec scopes of the blocks that name it (most blocks first)
    let mut path_scope: HashMap<u32, BTreeMap<String, usize>> = HashMap::new();
    for b in &m.blocks {
        let sc = &m.pages[b.page as usize].scope;
        if sc.starts_with("specs/") {
            for &(f, tier, _) in &b.files {
                if tier <= Tier::Derived {
                    *path_scope.entry(f).or_default().entry(sc.clone()).or_default() += 1;
                }
            }
        }
    }
    for (&f, lines) in &by_file {
        let path = &code_path[&f];
        let cx = kcite_ctx(path, lines);
        let mut raw: Vec<KRawId> = Vec::new();
        for (ln, l) in lines.iter().enumerate() {
            let specs = spec_refs(l);
            for n in &specs {
                if let Some(s) = spec_of_num.get(n) {
                    m.code_specs[f as usize].push(s.clone());
                }
            }
            for (st, en, id, bare) in kid_mentions(l, false) {
                if ids.accepts(&id, bare) {
                    raw.push(KRawId { id, bare, specs: kqualifier(l, st, en), line: ln as u32 });
                }
            }
            let Some(cm) = kcomment(l) else { continue };
            let (res, rawc) = kcite_line(cm, ln, &KCiteAt { cx: &cx, stack: &[], recent: None }, known, true, &mut m.stats);
            for (k, tier, route) in res {
                if audit {
                    audit_v.push(KAudit { kt: KT::Cite, tier, route, at: format!("{path}:{}", ln + 1), mention: kexcerpt(l), target: k.clone() });
                }
                kbump(&mut m.stats, format!("k_cite_code_{route}"));
                m.code_refs[f as usize].push(KRef { kt: KT::Cite, key: k, tier, route });
            }
            m.code_cites_raw[f as usize].extend(rawc);
        }
        let file_specs: Vec<u32> = lines.iter().flat_map(|l| spec_refs(l)).collect();
        let refs: Vec<&KRawId> = raw.iter().collect();
        // the file's own documented scope: the specs naming it, then its non-test sibling
        // (`x_test.go` -> `x.go`, `x.spec.ts` -> `x.ts`), then its directory
        let cp = &kp.cand[f as usize];
        let sib: Option<u32> = [("_test.go", ".go"), (".spec.ts", ".ts"), (".test.ts", ".ts"), (".test.js", ".js"), ("_test.py", ".py")]
            .iter()
            .find_map(|(a, b)| cp.strip_suffix(a).and_then(|st| kp.exact.get(&format!("{st}{b}")).copied()));
        let mut extra: Vec<String> = Vec::new();
        let push = |sc: Option<&BTreeMap<String, usize>>, extra: &mut Vec<String>| {
            let mut v: Vec<(&String, &usize)> = sc.map(|x| x.iter().collect()).unwrap_or_default();
            v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            for (k, _) in v {
                if !extra.contains(k) {
                    extra.push(k.clone());
                }
            }
        };
        push(path_scope.get(&f), &mut extra);
        if let Some(sf) = sib {
            push(path_scope.get(&sf), &mut extra);
        }
        let mut dir_sc: BTreeMap<String, usize> = BTreeMap::new();
        for (&g, sc) in &path_scope {
            if dir_of(&kp.cand[g as usize]) == dir_of(cp) {
                for (k, v) in sc {
                    *dir_sc.entry(k.clone()).or_default() += v;
                }
            }
        }
        push(Some(&dir_sc), &mut extra);
        let res = ids.resolve_all(&refs, None, &file_specs, &extra, "path-scope");
        let res: Vec<Result<KRes, &'static str>> = match &kns {
            None => res,
            Some(kn) => {
                let pairs: Vec<KResPair> = raw
                    .iter()
                    .zip(res)
                    .map(|(rm, r)| {
                        if !rm.bare {
                            return (r.clone(), r);
                        }
                        let cx = KNsCtx {
                            own_page: None,
                            line: rm.specs.iter().filter_map(|n| ids.spec_scope.get(n).cloned()).collect(),
                            own_spec: None,
                            named: Vec::new(),
                            file: file_specs.iter().filter_map(|n| ids.spec_scope.get(n).cloned()).collect(),
                            path: extra.clone(),
                            text: lines[rm.line as usize],
                        };
                        (r.clone(), kn.resolve(&rm.id, &cx, r))
                    })
                    .collect();
                let (base_v, mut ns_v): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                let keys: Vec<(u32, &str)> = raw.iter().map(|rm| (rm.line, rm.id.as_str())).collect();
                kn.siblings(&keys, &mut ns_v);
                if audit {
                    let show = |x: &Result<KRes, &'static str>| x.as_ref().map_or_else(|e| format!("ERR:{e}"), |v| format!("{} [{}]", v.0, v.2));
                    for (k, rm) in raw.iter().enumerate() {
                        if rm.bare {
                            m.ns_audit.push((format!("{path}:{}", rm.line + 1), format!("{} :: {}", rm.id, kexcerpt(lines[rm.line as usize])), show(&base_v[k]), show(&ns_v[k]), ns_v[k].as_ref().map_or("unresolved", |v| v.2)));
                        }
                    }
                }
                ns_v
            }
        };
        if audit && std::env::var("GLIA_KEYS_DEBUG").is_ok() {
            for (rm, r) in raw.iter().zip(&res) {
                if let Err(e) = r {
                    eprintln!("[keys-debug] code {e} {} defs={:?} at {path}:{}", rm.id, ids.defs.get(&rm.id), rm.line + 1);
                }
            }
        }
        for (rm, r) in raw.iter().zip(res) {
            m.code_raw_ids[f as usize].push(format!("id:{}", rm.id));
            match r {
                Ok((k, tier, route)) => {
                    if audit {
                        audit_v.push(KAudit { kt: KT::Scoped, tier, route, at: format!("{path}:{}", rm.line + 1), mention: format!("{} :: {}", rm.id, kexcerpt(lines[rm.line as usize])), target: k.clone() });
                    }
                    kbump(&mut m.stats, format!("k_scoped_code_{route}"));
                    m.code_refs[f as usize].push(KRef { kt: KT::Scoped, key: k, tier, route });
                }
                Err(e) => kbump(&mut m.stats, format!("k_scoped_code_{e}")),
            }
        }
        m.code_specs[f as usize].sort();
        m.code_specs[f as usize].dedup();
    }
    // ---- pass D: text fallback for definition and row blocks no other key links to code ----
    if let Some(bm) = bm {
        let cited: HashSet<&str> = m.code_refs.iter().flatten().map(|r| r.key.as_str()).collect();
        let want: Vec<usize> = (0..m.blocks.len())
            .filter(|&bi| {
                let b = &m.blocks[bi];
                !b.defs.is_empty() && b.files.is_empty() && !b.defs.iter().any(|d| cited.contains(d.1.as_str()))
            })
            .collect();
        let tops: Vec<(usize, Vec<(u32, f64)>)> = want.par_iter().map(|&bi| (bi, bm.top(&m.blocks[bi].text, 3))).collect();
        for (bi, top) in tops {
            for &(f, s) in &top {
                kbump(&mut m.stats, "k_text_links");
                if audit {
                    let b = &m.blocks[bi];
                    audit_v.push(KAudit { kt: KT::Text, tier: Tier::Heuristic, route: "bm25-top3", at: at_of(&m, b, b.line), mention: line_of(&m, b, b.line), target: format!("{} (bm25 {s:.1})", kp.cand[f as usize]) });
                }
            }
            m.blocks[bi].text_files = top;
        }
    }
    let mut tally = BTreeMap::new();
    ktally(m.blocks.iter().flat_map(|b| b.refs.iter()).chain(m.code_refs.iter().flatten()), &mut tally);
    m.stats.extend(tally);
    m.stats.insert("k_blocks".into(), m.blocks.len() as f64);
    m.stats.insert("k_pages".into(), m.pages.len() as f64);
    m.stats.insert("k_lineage_edges".into(), m.lineage.len() as f64);
    m.stats.insert("k_scoped_prefixes".into(), ids.scoped.len() as f64);
    m.audit = audit_v;
    m.kids = Some(ids);
    m
}

// ---- the joiner (Kina) ----

/// Per candidate file, the upper-layer nodes it reaches through resolved keys.
#[derive(Default)]
struct KSets {
    /// requirement level: the key classes it cites or the blocks that name it define
    r1: Vec<BTreeSet<String>>,
    /// r1 plus the classes those naming blocks mention (one hop)
    r1h: Vec<BTreeSet<String>>,
    /// spec level: the heading units of its related blocks
    r2u: Vec<BTreeSet<String>>,
    /// spec level, coarse: the spec dir or doc of its related blocks, plus `spec NNN` in code
    r2d: Vec<BTreeSet<String>>,
    /// law: the sections its related blocks (or its own comments) cite
    r3b: Vec<BTreeSet<String>>,
    /// law: r3b after one goal-internal hop
    r3h: Vec<BTreeSet<String>>,
    /// law at the instrument level (r3h's instruments)
    r3a: Vec<BTreeSet<String>>,
    /// Round 2 (roles on): r1h and r2u with each link's weight (a names-file link from a done unit 0.5)
    r1hw: Vec<BTreeMap<String, f64>>,
    r2uw: Vec<BTreeMap<String, f64>>,
}

const KHUB: usize = 40;

/// Per-block resolved keys under one joiner variant.
struct KBk {
    defs: Vec<(KT, String)>,
    refs: Vec<String>,
    cites: Vec<String>,
    /// Round 2: the classes of the refs in the `implements` role
    impl_refs: Vec<String>,
}

/// One joiner variant's keys: the lineage classes, each block's keys, each code file's id and
/// citation classes, and the blocks that name each code file.
type KKeys = (KLin, Vec<KBk>, Vec<Vec<String>>, Vec<Vec<String>>, Vec<Vec<u32>>);

fn kina_keys(m: &KinaModel, mask: KMask) -> KKeys {
    let idk = |k: &str| -> String { if mask.raw_scoped { kraw_id(k) } else { k.to_string() } };
    let mut edges: Vec<(String, String, Tier, bool)> = Vec::new();
    if mask.on(KT::Lineage) {
        for (a, b, t, r) in &m.lineage {
            if a.starts_with("law:") && !mask.on(KT::Cite) {
                continue;
            }
            let both_rows = a.starts_with("row:") && b.starts_with("row:");
            if !a.starts_with("law:") && !mask.on(KT::Scoped) && !both_rows {
                continue;
            }
            edges.push((idk(a), idk(b), *t, matches!(*r, "restated" | "moved-section")));
        }
    }
    let mut st = BTreeMap::new();
    let cl = KLin::build(&edges, 8, &mut st);
    let c = |k: &str| cl.exp(k);
    let mut bks = Vec::with_capacity(m.blocks.len());
    let mut names: Vec<Vec<u32>> = vec![Vec::new(); m.code_refs.len()];
    for (bi, b) in m.blocks.iter().enumerate() {
        let defs: Vec<(KT, String)> = b.defs.iter().filter(|d| mask.on(d.0)).flat_map(|d| c(&idk(&d.1)).into_iter().map(move |x| (d.0, x))).collect();
        let refs: Vec<String> = b.refs.iter().filter(|r| matches!(r.kt, KT::Scoped | KT::Row) && mask.keeps(r)).flat_map(|r| c(&idk(&r.key))).collect();
        let impl_refs: Vec<String> = b
            .refs
            .iter()
            .filter(|r| matches!(r.kt, KT::Scoped | KT::Row) && mask.keeps(r) && b.impl_keys.contains(&r.key))
            .flat_map(|r| c(&idk(&r.key)))
            .collect();
        let cites: Vec<String> = if !mask.on(KT::Cite) {
            Vec::new()
        } else if mask.raw_cite {
            b.cites_raw.iter().flat_map(|k| c(k)).collect()
        } else {
            b.refs.iter().filter(|r| r.kt == KT::Cite && mask.keeps(r)).flat_map(|r| c(&r.key)).collect()
        };
        if mask.on(KT::Path) {
            if mask.raw_path {
                for &f in &b.files_raw {
                    names[f as usize].push(bi as u32);
                }
            } else {
                for &(f, tier, _) in &b.files {
                    if tier <= mask.max_tier {
                        names[f as usize].push(bi as u32);
                    }
                }
            }
        }
        if mask.on(KT::Text) && mask.max_tier >= Tier::Heuristic {
            for &(f, _) in &b.text_files {
                names[f as usize].push(bi as u32);
            }
        }
        bks.push(KBk { defs, refs, cites, impl_refs });
    }
    for v in names.iter_mut() {
        v.sort_unstable();
        v.dedup();
    }
    let code_ids: Vec<Vec<String>> = (0..m.code_refs.len())
        .map(|f| {
            if !mask.on(KT::Scoped) {
                return m.code_refs[f].iter().filter(|r| r.kt == KT::Row && mask.keeps(r)).flat_map(|r| c(&r.key)).collect();
            }
            if mask.raw_scoped {
                let mut v: Vec<String> = m.code_raw_ids[f].iter().flat_map(|k| c(k)).collect();
                v.extend(m.code_refs[f].iter().filter(|r| r.kt == KT::Row && mask.keeps(r)).flat_map(|r| c(&r.key)));
                return v;
            }
            m.code_refs[f].iter().filter(|r| matches!(r.kt, KT::Scoped | KT::Row) && mask.keeps(r)).flat_map(|r| c(&r.key)).collect()
        })
        .collect();
    let code_cites: Vec<Vec<String>> = (0..m.code_refs.len())
        .map(|f| {
            if !mask.on(KT::Cite) {
                Vec::new()
            } else if mask.raw_cite {
                m.code_cites_raw[f].iter().flat_map(|k| c(k)).collect()
            } else {
                m.code_refs[f].iter().filter(|r| r.kt == KT::Cite && mask.keeps(r)).flat_map(|r| c(&r.key)).collect()
            }
        })
        .collect();
    (cl, bks, code_ids, code_cites, names)
}

fn kina_join(m: &KinaModel, mask: KMask, perm: Option<&HashMap<usize, usize>>) -> KSets {
    let nf = m.code_refs.len();
    let (_, mut bks, code_ids, code_cites, names) = kina_keys(m, mask);
    if let Some(p) = perm {
        let orig: Vec<Vec<String>> = bks.iter().map(|b| b.cites.clone()).collect();
        for (&a, &b) in p {
            bks[a].cites = orig[b].clone();
        }
    }
    let mut def_blocks: HashMap<&str, Vec<u32>> = HashMap::new();
    let mut ref_blocks: HashMap<&str, Vec<u32>> = HashMap::new();
    for (bi, b) in bks.iter().enumerate() {
        for d in &b.defs {
            def_blocks.entry(d.1.as_str()).or_default().push(bi as u32);
        }
        // roles on: only an `implements` mention links a block to a key it does not define
        for r in if mask.roles { &b.impl_refs } else { &b.refs } {
            ref_blocks.entry(r.as_str()).or_default().push(bi as u32);
        }
    }
    let w_of = |b: u32| if mask.roles && kblock_state(&m.blocks[b as usize]) == Some(false) { R2_W_DONE } else { 1.0 };
    let inst = |k: &str| -> String {
        let parts: Vec<&str> = k.split(':').collect();
        if parts.len() >= 4 && parts[1] == "OTHER" { format!("law:OTHER:{}", parts[2]) } else { parts[..parts.len().min(2)].join(":") }
    };
    let mut out = KSets::default();
    for f in 0..nf {
        let named = &names[f];
        let mut r1: BTreeSet<String> = code_ids[f].iter().cloned().collect();
        for &b in named {
            let bk = &bks[b as usize];
            if bk.defs.is_empty() {
                r1.insert(format!("blk:{b}"));
            } else {
                r1.extend(bk.defs.iter().map(|d| d.1.clone()));
            }
        }
        let mut r1h = r1.clone();
        for &b in named {
            r1h.extend(if mask.roles { bks[b as usize].impl_refs.iter() } else { bks[b as usize].refs.iter() }.cloned());
        }
        if mask.roles {
            let mut w: BTreeMap<String, f64> = code_ids[f].iter().map(|k| (k.clone(), 1.0)).collect();
            for &b in named {
                let bk = &bks[b as usize];
                let keys: Vec<String> = if bk.defs.is_empty() { vec![format!("blk:{b}")] } else { bk.defs.iter().map(|d| d.1.clone()).collect() };
                for k in keys.into_iter().chain(bk.impl_refs.iter().cloned()) {
                    let e = w.entry(k).or_insert(0.0);
                    *e = e.max(w_of(b));
                }
            }
            out.r1hw.push(w);
        }
        let mut related: BTreeSet<u32> = named.iter().copied().collect();
        for k in &code_ids[f] {
            related.extend(def_blocks.get(k.as_str()).into_iter().flatten().copied());
        }
        let r2u: BTreeSet<String> = related.iter().map(|&b| format!("u:{}", m.blocks[b as usize].unit)).collect();
        if mask.roles {
            let mut w: BTreeMap<String, f64> = BTreeMap::new();
            for &b in &related {
                let wb = if named.contains(&b) { w_of(b) } else { 1.0 };
                let e = w.entry(format!("u:{}", m.blocks[b as usize].unit)).or_insert(0.0);
                *e = e.max(wb);
            }
            out.r2uw.push(w);
        }
        let mut r2d: BTreeSet<String> = related.iter().map(|&b| kina_unit(&m.pages[m.blocks[b as usize].page as usize].path)).collect();
        r2d.extend(m.code_specs[f].iter().cloned());
        let mut r3b: BTreeSet<String> = code_cites[f].iter().cloned().collect();
        for &b in &related {
            r3b.extend(bks[b as usize].cites.iter().cloned());
        }
        let mut related1 = related.clone();
        for &b in &related {
            let bk = &bks[b as usize];
            for k in bk.defs.iter().map(|d| &d.1).chain(if mask.roles { &bk.impl_refs } else { &bk.refs }) {
                let d = def_blocks.get(k.as_str()).map_or(0, Vec::len);
                let r = ref_blocks.get(k.as_str()).map_or(0, Vec::len);
                if d + r > KHUB {
                    continue;
                }
                related1.extend(def_blocks.get(k.as_str()).into_iter().flatten().copied());
                related1.extend(ref_blocks.get(k.as_str()).into_iter().flatten().copied());
            }
        }
        let mut r3h = r3b.clone();
        for &b in &related1 {
            r3h.extend(bks[b as usize].cites.iter().cloned());
        }
        let r3a: BTreeSet<String> = r3h.iter().map(|k| inst(k)).collect();
        out.r1.push(r1);
        out.r1h.push(r1h);
        out.r2u.push(r2u);
        out.r2d.push(r2d);
        out.r3b.push(r3b);
        out.r3h.push(r3h);
        out.r3a.push(r3a);
    }
    out
}

// ---- the glia goal layer as key mentions ----

/// A packet with its typed path column and its prose.
#[derive(Clone)]
struct GPk {
    id: String,
    files: Vec<String>,
    text: String,
}

/// The packets of one packets file, and its split remaps (packet -> files a wave-0 split
/// moved the packet's claim to).
type GPkFile = (Vec<GPk>, Vec<(String, Vec<String>)>);

fn glia_pk_full(text: &str) -> GPkFile {
    let v: serde_json::Value = serde_json::from_str(text).unwrap_or(serde_json::Value::Null);
    let mut out = Vec::new();
    for p in v.get("packets").and_then(|p| p.as_array()).into_iter().flatten() {
        let Some(id) = p.get("id").and_then(|x| x.as_str()) else { continue };
        let files = p
            .get("files_touched")
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let mut t = String::new();
        for k in ["title", "rationale", "implementation", "acceptance", "risks", "research", "fixtures"] {
            match p.get(k) {
                Some(serde_json::Value::String(s)) => t.push_str(s),
                Some(x) if !x.is_null() => t.push_str(&x.to_string()),
                _ => {}
            }
            t.push('\n');
        }
        out.push(GPk { id: id.to_string(), files, text: t });
    }
    let mut remap = Vec::new();
    for (_, arr) in v.get("splits").and_then(|x| x.as_object()).into_iter().flatten() {
        let items: Vec<&serde_json::Value> = match arr {
            serde_json::Value::Array(a) => a.iter().collect(),
            x => vec![x],
        };
        for e in items {
            for (pk, fs) in e.get("remap").and_then(|x| x.as_object()).into_iter().flatten() {
                let files: Vec<String> = fs.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default();
                if !files.is_empty() {
                    remap.push((pk.clone(), files));
                }
            }
        }
    }
    (out, remap)
}

/// Ruling units of a plan doc: inside a heading naming decisions or rulings, each numbered
/// item and table row (the earlier run's sections), and tweaks-0.5.2.md's T rows. A numbered
/// table whose intro names `Q1–19` keys its rows `Q<n>`, the key the packets and code cite.
fn glia_ruling_units(doc: &str, text: &str) -> Vec<(String, String, &'static str)> {
    let tweaks = doc.ends_with("tweaks-0.5.2.md");
    let lines: Vec<&str> = text.lines().collect();
    let mut heads: Vec<(usize, String)> = Vec::new();
    let mut out: Vec<(String, String, &'static str)> = Vec::new();
    let mut cur: Option<(String, String, &'static str)> = None;
    let mut intro = String::new();
    let mut letter: Option<char> = None;
    let flush = |cur: &mut Option<(String, String, &'static str)>, out: &mut Vec<(String, String, &'static str)>| {
        if let Some((k, t, r)) = cur.take() {
            out.push((format!("{doc}#{k}"), t, r));
        }
    };
    let is_sep = |x: &str| x.trim().starts_with('|') && x.trim().chars().all(|c| matches!(c, '|' | '-' | ':' | ' '));
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with('#') {
            flush(&mut cur, &mut out);
            let level = t.bytes().take_while(|&c| c == b'#').count();
            heads.retain(|(l, _)| *l < level);
            heads.push((level, t.trim_start_matches('#').trim().to_string()));
            intro.clear();
            letter = None;
            continue;
        }
        let in_rulings = heads.iter().any(|(_, h)| {
            let h = h.to_ascii_lowercase();
            h.contains("decision") || h.contains("ruling")
        });
        let section: String = heads.last().map_or(String::new(), |(_, h)| h.chars().take(40).collect());
        let gap_row = tweaks && t.starts_with("| T") && t[3..].starts_with(|c: char| c.is_ascii_digit());
        let gap_para = tweaks && t.starts_with('T') && t[1..].starts_with(|c: char| c.is_ascii_digit()) && t.contains(" (James");
        if gap_row || gap_para {
            flush(&mut cur, &mut out);
            let n: String = t.trim_start_matches(['|', ' ', 'T']).chars().take_while(char::is_ascii_digit).collect();
            cur = Some((format!("T{n}"), t.to_string(), "t-row"));
            flush(&mut cur, &mut out);
            continue;
        }
        if !t.starts_with('|') && intro.len() < 4000 {
            intro.push_str(t);
            intro.push('\n');
        }
        if !in_rulings {
            flush(&mut cur, &mut out);
            continue;
        }
        let d = t.bytes().take_while(u8::is_ascii_digit).count();
        if d > 0 && t[d..].starts_with(". ") {
            flush(&mut cur, &mut out);
            cur = Some((format!("{section}/{}", &t[..d]), t.to_string(), "item"));
        } else if t.starts_with('|') {
            flush(&mut cur, &mut out);
            if is_sep(t) {
                continue;
            }
            if lines.get(i + 1).is_some_and(|x| is_sep(x)) {
                // a header: read ahead the table's numbered first cells for its id letter
                let nums: Vec<u32> = lines[i + 2..]
                    .iter()
                    .take_while(|x| x.trim_start().starts_with('|'))
                    .filter_map(|x| kcells(x).first().and_then(|c| c.parse().ok()))
                    .collect();
                letter = row_letter(&intro, &nums);
                continue;
            }
            let cell = kcells(t).first().cloned().unwrap_or_default();
            if !cell.is_empty() && !cell.starts_with("---") && cell != "#" && cell != "decision" {
                match (letter, cell.parse::<u32>()) {
                    (Some(c), Ok(n)) => cur = Some((format!("{c}{n}"), t.to_string(), "lettered-row")),
                    _ => cur = Some((format!("{section}/{cell}"), t.to_string(), "row")),
                }
                flush(&mut cur, &mut out);
            }
        } else if t.is_empty() {
            flush(&mut cur, &mut out);
        } else if let Some((_, buf, _)) = cur.as_mut() {
            buf.push(' ');
            buf.push_str(t);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// Packet lineage phrases (identity only: a claim "moved to" another packet, or work "folded
/// into" one, is not), and whether the id the phrase names is the NEW packet.
const GLIN_PHRASES: &[(&str, &str, bool)] = &[
    ("split from ", "split-from", false),
    ("supersedes ", "supersedes", false),
    ("renamed from ", "renamed", false),
    ("formerly ", "formerly", false),
    ("(was ", "was", false),
    ("replaces ", "replaces", false),
    ("split into ", "split-into", true),
    ("superseded by ", "superseded-by", true),
];

/// Packet lineage phrases in a packet's prose: the packet id a phrase names (`Split from the
/// first DT.10c`, `moved to CZ.1`), skipping a `the first` / `the original` / `the`.
fn glia_lin(text: &str) -> Vec<(String, &'static str, bool)> {
    let lower = text.to_ascii_lowercase();
    let mut out = Vec::new();
    for (ph, name, names_new) in GLIN_PHRASES {
        let mut from = 0;
        while let Some(p) = lower[from..].find(ph) {
            let mut e = from + p + ph.len();
            from = e;
            for skip in ["the first ", "the original ", "the "] {
                if lower[e..].starts_with(skip) {
                    e += skip.len();
                    break;
                }
            }
            let w: String = text[e..].chars().take(14).collect();
            if let Some((_, id)) = dot_ids(&w).into_iter().next()
                && w.starts_with(&id)
            {
                out.push((id, *name, *names_new));
            }
        }
    }
    out
}

/// `Q<n>` mentions (Q1..Q99), word-bounded.
fn q_ids(s: &str) -> Vec<String> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    for i in 0..b.len() {
        if b[i] == b'Q' && wb(b, i) {
            let e = digits_at(b, i + 1, 3);
            if e > i + 1 && e - i - 1 <= 2 && we(b, e) {
                out.push(s[i..e].to_string());
            }
        }
    }
    out
}

struct GliaModel {
    pk_ids: Vec<String>,
    pk_files: Vec<Vec<(u32, Tier, &'static str)>>,
    pk_files_raw: Vec<Vec<u32>>,
    pk_text: Vec<Vec<(u32, f64)>>,
    pk_q: Vec<Vec<KRef>>,
    rul: Vec<String>,
    rul_pk: Vec<Vec<KRef>>,
    rul_files: Vec<Vec<(u32, Tier, &'static str)>>,
    rul_files_raw: Vec<Vec<u32>>,
    rul_text: Vec<Vec<(u32, f64)>>,
    code_pk: Vec<Vec<KRef>>,
    code_pk_raw: Vec<Vec<String>>,
    code_q: Vec<Vec<KRef>>,
    lineage: Vec<(String, String, Tier, &'static str)>,
    stats: BTreeMap<String, f64>,
    audit: Vec<KAudit>,
    /// Round 2: the defined packets each packet's prose names (a mention, not a definition).
    pk_ment: Vec<Vec<String>>,
}

#[allow(clippy::too_many_arguments)]
fn glia_model(
    git: &str,
    rev: &str,
    cache: &BlobCache,
    pkc: &std::sync::Mutex<HashMap<String, std::sync::Arc<GPkFile>>>,
    code_lines: &[(String, String)],
    kp: &KPaths,
    pr: &PathRes,
    bm: Option<&KBm>,
    nf: usize,
    audit: bool,
) -> GliaModel {
    let mut stats: BTreeMap<String, f64> = BTreeMap::new();
    let mut aud: Vec<KAudit> = Vec::new();
    let mut all: Vec<&str> = Vec::new();
    for g in [GLIA_PACKETS, GLIA_PLANS, GLIA_CORRECTIONS] {
        all.extend_from_slice(g);
    }
    let blobs = ls_tree(git, rev, &all);
    // packets (first definition wins, as in the earlier run), with their file's release plan
    let mut pks: BTreeMap<String, (GPk, &'static str)> = BTreeMap::new();
    let mut remaps: Vec<(String, Vec<String>)> = Vec::new();
    for &p in GLIA_PACKETS {
        let Some(b) = blobs.get(p) else { continue };
        let got = {
            let hit = pkc.lock().unwrap().get(b).cloned();
            hit.unwrap_or_else(|| {
                let a = std::sync::Arc::new(glia_pk_full(&cache.get(git, b)));
                pkc.lock().unwrap().insert(b.clone(), a.clone());
                a
            })
        };
        let plan: &'static str = match p {
            "dev-notes/leap-052-packets.json" => "next-leap-0.5.2.md",
            "dev-notes/leap-051-packets.json" => "next-leap-0.5.1.md",
            "dev-notes/leap-packets.json" => "next-leap-0.5.0.md",
            _ => "",
        };
        for pk in &got.0 {
            pks.entry(pk.id.clone()).or_insert_with(|| (pk.clone(), plan));
        }
        remaps.extend(got.1.iter().cloned());
    }
    let pk_ids: Vec<String> = pks.keys().cloned().collect();
    let pix: HashMap<&str, usize> = pk_ids.iter().enumerate().map(|(i, p)| (p.as_str(), i)).collect();
    let prefixes: BTreeSet<String> = pk_ids.iter().map(|id| id.split('.').next().unwrap_or("").to_string()).collect();
    let mut fams: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for id in &pk_ids {
        fams.entry(family(id)).or_default().push(id.clone());
    }
    // rulings
    let mut rul: Vec<String> = Vec::new();
    let mut rul_text: Vec<String> = Vec::new();
    let mut rul_route: Vec<&'static str> = Vec::new();
    for &d in GLIA_PLANS {
        if let Some(b) = blobs.get(d) {
            let name = d.rsplit('/').next().unwrap_or(d);
            for (k, t, r) in glia_ruling_units(name, &cache.get(git, b)) {
                rul.push(k);
                rul_text.push(t);
                rul_route.push(r);
            }
        }
    }
    let mut corr_key: Vec<Option<String>> = vec![None; rul.len()];
    for &c in GLIA_CORRECTIONS {
        let Some(b) = blobs.get(c) else { continue };
        let v: serde_json::Value = serde_json::from_str(&cache.get(git, b)).unwrap_or(serde_json::Value::Null);
        let name = c.rsplit('/').next().unwrap_or(c);
        for (k, val) in v.get("corrections").and_then(|x| x.as_object()).into_iter().flatten() {
            let text = val.as_str().map_or_else(|| val.to_string(), str::to_string);
            if !text.contains("James") {
                continue;
            }
            rul.push(format!("{name}#{k}"));
            rul_text.push(text);
            rul_route.push("correction");
            corr_key.push(Some(k.clone()));
        }
    }
    let rix: HashMap<&str, usize> = rul.iter().enumerate().map(|(i, r)| (r.as_str(), i)).collect();
    // where each Q<n> is defined (a lettered row of a plan doc)
    let mut q_def: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (i, k) in rul.iter().enumerate() {
        if rul_route[i] == "lettered-row"
            && let Some((doc, q)) = k.split_once('#')
        {
            q_def.entry(q.to_string()).or_default().push(doc.to_string());
            if audit {
                aud.push(KAudit { kt: KT::Row, tier: Tier::Derived, route: "lettered-row", at: doc.to_string(), mention: kexcerpt(&rul_text[i]), target: format!("rul:{k}") });
            }
            kbump(&mut stats, "k_row_lettered");
        }
    }
    let resolve_q = |q: &str, own_plan: &str, line_release: bool| -> Option<(String, Tier, &'static str)> {
        let docs = q_def.get(q)?;
        if docs.iter().any(|d| d == own_plan) {
            return Some((format!("rul:{own_plan}#{q}"), Tier::Fact, "own-release"));
        }
        if docs.len() == 1 {
            return Some((format!("rul:{}#{q}", docs[0]), if line_release { Tier::Fact } else { Tier::Derived }, if line_release { "line-release" } else { "unique-doc" }));
        }
        None
    };
    // a packet mention (an id, a family, a `P-n` shorthand) -> packet keys
    let pk_mentions = |t: &str| -> Vec<(String, Tier, &'static str, String)> {
        let mut v = Vec::new();
        for (p, id) in dot_ids(t) {
            if !prefixes.contains(&p) {
                continue;
            }
            if pix.contains_key(id.as_str()) {
                v.push((format!("pk:{id}"), Tier::Fact, "packet-id", id));
            } else if let Some(ms) = fams.get(&id) {
                v.extend(ms.iter().map(|x| (format!("pk:{x}"), Tier::Derived, "family", id.clone())));
            }
        }
        for (p, id) in hyphen_ids(t) {
            if prefixes.contains(&p) {
                let fam = format!("{p}.{}", &id[p.len() + 1..]);
                if let Some(ms) = fams.get(&fam) {
                    v.extend(ms.iter().map(|x| (format!("pk:{x}"), Tier::Derived, "family-shorthand", id.clone())));
                }
            }
        }
        v
    };
    // packets: typed paths, Q citations, lineage phrases
    let n = pk_ids.len();
    let mut pk_files = vec![Vec::new(); n];
    let mut pk_files_raw = vec![Vec::new(); n];
    let mut pk_q: Vec<Vec<KRef>> = vec![Vec::new(); n];
    let mut pk_ment: Vec<Vec<String>> = vec![Vec::new(); n];
    let mut lineage = Vec::new();
    for (i, id) in pk_ids.iter().enumerate() {
        let (pk, plan) = &pks[id];
        let mut mset: BTreeSet<String> = BTreeSet::new();
        for (p, other) in dot_ids(&pk.text) {
            if prefixes.contains(&p) && other != *id && pix.contains_key(other.as_str()) {
                mset.insert(other);
            }
        }
        pk_ment[i] = mset.into_iter().collect();
        for entry in &pk.files {
            pk_files_raw[i].extend(pr.files(entry));
            for (f, tier, route, _) in kp.block(&kpath_tokens(entry), &mut stats) {
                kbump(&mut stats, format!("k_path_{route}"));
                if audit {
                    aud.push(KAudit { kt: KT::Path, tier, route, at: format!("{id} files_touched"), mention: kexcerpt(entry), target: kp.cand[f as usize].clone() });
                }
                pk_files[i].push((f, tier, route));
            }
        }
        for q in q_ids(&pk.text) {
            if let Some((k, tier, route)) = resolve_q(&q, plan, false) {
                kbump(&mut stats, format!("k_scoped_pkq_{route}"));
                pk_q[i].push(KRef { kt: KT::Scoped, key: k, tier, route });
            }
        }
        for (other, verb, names_new) in glia_lin(&pk.text) {
            if other != *id && pix.contains_key(other.as_str()) {
                let (new, old) = if names_new { (other.clone(), id.clone()) } else { (id.clone(), other.clone()) };
                if audit {
                    let pos = pk.text.find(&other).unwrap_or(0);
                    let st = pk.text[..pos].char_indices().rev().nth(120).map_or(0, |x| x.0);
                    let ctx: String = pk.text[st..].chars().take(220).collect();
                    aud.push(KAudit { kt: KT::Lineage, tier: Tier::Derived, route: verb, at: id.clone(), mention: kexcerpt(&ctx), target: format!("new pk:{new} <- old pk:{old}") });
                }
                kbump(&mut stats, format!("k_lineage_{verb}"));
                lineage.push((format!("pk:{new}"), format!("pk:{old}"), Tier::Derived, verb));
            }
        }
    }
    for (pk, files) in &remaps {
        let Some(&i) = pix.get(pk.as_str()) else { continue };
        for fs in files {
            for (f, _, route, _) in kp.block(&kpath_tokens(fs), &mut stats) {
                kbump(&mut stats, "k_path_split-remap");
                if audit {
                    aud.push(KAudit { kt: KT::Path, tier: Tier::Derived, route: "split-remap", at: format!("{pk} split remap ({route})"), mention: kexcerpt(fs), target: kp.cand[f as usize].clone() });
                }
                pk_files[i].push((f, Tier::Derived, "split-remap"));
            }
        }
    }
    // rulings: packets, T-row families, paths
    let nr = rul.len();
    let mut rul_pk: Vec<Vec<KRef>> = vec![Vec::new(); nr];
    let mut rul_files = vec![Vec::new(); nr];
    let mut rul_files_raw = vec![Vec::new(); nr];
    for r in 0..nr {
        let t = &rul_text[r];
        let mut got = pk_mentions(t);
        if let Some(k) = &corr_key[r]
            && pix.contains_key(k.as_str())
        {
            got.push((format!("pk:{k}"), Tier::Fact, "correction-key", k.clone()));
        }
        for (k, tier, route, raw) in got {
            kbump(&mut stats, format!("k_scoped_ruling_{route}"));
            if audit {
                aud.push(KAudit { kt: KT::Scoped, tier, route, at: format!("rul:{}", rul[r]), mention: format!("{raw} :: {}", kexcerpt(t)), target: k.clone() });
            }
            rul_pk[r].push(KRef { kt: KT::Scoped, key: k, tier, route });
        }
        if rul_route[r] == "t-row"
            && let Some(tn) = rul[r].rsplit('#').next()
            && let Some(ms) = fams.get(&format!("DT.{}", &tn[1..]))
        {
            for x in ms {
                kbump(&mut stats, "k_row_t-row-family");
                if audit {
                    aud.push(KAudit { kt: KT::Row, tier: Tier::Derived, route: "t-row-family", at: format!("rul:{}", rul[r]), mention: kexcerpt(t), target: format!("pk:{x}") });
                }
                rul_pk[r].push(KRef { kt: KT::Row, key: format!("pk:{x}"), tier: Tier::Derived, route: "t-row-family" });
            }
        }
        rul_files_raw[r] = t.lines().flat_map(|l| pr.files(l)).collect();
        for (f, tier, route, raw) in kp.block(&kpath_tokens(t), &mut stats) {
            kbump(&mut stats, format!("k_path_ruling_{route}"));
            if audit {
                aud.push(KAudit { kt: KT::Path, tier, route, at: format!("rul:{}", rul[r]), mention: format!("{raw} :: {}", kexcerpt(t)), target: kp.cand[f as usize].clone() });
            }
            rul_files[r].push((f, tier, route));
        }
    }
    // code: packet citations and Q citations
    let mut code_pk: Vec<Vec<KRef>> = vec![Vec::new(); nf];
    let mut code_pk_raw: Vec<Vec<String>> = vec![Vec::new(); nf];
    let mut code_q: Vec<Vec<KRef>> = vec![Vec::new(); nf];
    for (p, l) in code_lines {
        let Some(f) = kp.map(p) else { continue };
        if !kp.exact.contains_key(p) {
            kbump(&mut stats, "k_code_lines_via_rename");
        }
        let f = f as usize;
        for (pfx, id) in dot_ids(l) {
            if prefixes.contains(&pfx) {
                code_pk_raw[f].push(format!("pk:{id}"));
            }
        }
        for (k, tier, route, raw) in pk_mentions(l) {
            kbump(&mut stats, format!("k_scoped_code_{route}"));
            if audit {
                aud.push(KAudit { kt: KT::Scoped, tier, route, at: p.clone(), mention: format!("{raw} :: {}", kexcerpt(l)), target: k.clone() });
            }
            code_pk[f].push(KRef { kt: KT::Scoped, key: k, tier, route });
        }
        let release = l.contains("0.5.2");
        for q in q_ids(l) {
            match resolve_q(&q, "", release) {
                Some((k, tier, route)) => {
                    kbump(&mut stats, format!("k_scoped_codeq_{route}"));
                    if audit {
                        aud.push(KAudit { kt: KT::Scoped, tier, route, at: p.clone(), mention: format!("{q} :: {}", kexcerpt(l)), target: k.clone() });
                    }
                    code_q[f].push(KRef { kt: KT::Scoped, key: k, tier, route });
                }
                None => kbump(&mut stats, "k_scoped_codeq_unresolved"),
            }
        }
    }
    // text fallback: rulings linked to no packet and no file; packets with no file and no citing code
    let mut rul_tl = vec![Vec::new(); nr];
    let mut pk_tl = vec![Vec::new(); n];
    if let Some(bm) = bm {
        let cited: HashSet<&str> = code_pk.iter().flatten().map(|r| r.key.as_str()).collect();
        let rq: HashSet<usize> = pk_q.iter().flatten().filter_map(|q| rix.get(q.key.trim_start_matches("rul:")).copied()).collect();
        let rw: Vec<usize> = (0..nr).filter(|&r| rul_pk[r].is_empty() && rul_files[r].is_empty() && !rq.contains(&r)).collect();
        let rt: Vec<(usize, Vec<(u32, f64)>)> = rw.par_iter().map(|&r| (r, bm.top(&rul_text[r], 3))).collect();
        for (r, top) in rt {
            for &(f, s) in &top {
                kbump(&mut stats, "k_text_ruling_links");
                if audit {
                    aud.push(KAudit { kt: KT::Text, tier: Tier::Heuristic, route: "bm25-top3", at: format!("rul:{}", rul[r]), mention: kexcerpt(&rul_text[r]), target: format!("{} (bm25 {s:.1})", kp.cand[f as usize]) });
                }
            }
            rul_tl[r] = top;
        }
        let pw: Vec<usize> = (0..n).filter(|&i| pk_files[i].is_empty() && !cited.contains(format!("pk:{}", pk_ids[i]).as_str())).collect();
        let pt: Vec<(usize, Vec<(u32, f64)>)> = pw.par_iter().map(|&i| (i, bm.top(&pks[&pk_ids[i]].0.text, 3))).collect();
        for (i, top) in pt {
            for &(f, s) in &top {
                kbump(&mut stats, "k_text_packet_links");
                if audit {
                    aud.push(KAudit { kt: KT::Text, tier: Tier::Heuristic, route: "bm25-top3", at: pk_ids[i].clone(), mention: kexcerpt(&pks[&pk_ids[i]].0.text), target: format!("{} (bm25 {s:.1})", kp.cand[f as usize]) });
                }
            }
            pk_tl[i] = top;
        }
    }
    ktally(code_pk.iter().flatten().chain(code_q.iter().flatten()).chain(rul_pk.iter().flatten()).chain(pk_q.iter().flatten()), &mut stats);
    stats.insert("k_packets".into(), n as f64);
    stats.insert("k_rulings".into(), nr as f64);
    stats.insert("k_lineage_edges".into(), lineage.len() as f64);
    GliaModel {
        pk_ids,
        pk_files,
        pk_files_raw,
        pk_text: pk_tl,
        pk_q,
        rul,
        rul_pk,
        rul_files,
        rul_files_raw,
        rul_text: rul_tl,
        code_pk,
        code_pk_raw,
        code_q,
        lineage,
        stats,
        audit: aud,
        pk_ment,
    }
}

#[derive(Default)]
struct GSets {
    /// packet classes the file cites
    r1c: Vec<BTreeSet<String>>,
    /// r1c plus the packet classes whose typed path column names it
    r1: Vec<BTreeSet<String>>,
    /// rulings of r1c's packets (the earlier V4's route)
    r4c: Vec<BTreeSet<String>>,
    /// rulings of r1's packets
    r4: Vec<BTreeSet<String>>,
    /// rulings linked directly: their text names the file, or the file cites them (Q<n>)
    r4d: Vec<BTreeSet<String>>,
    r4a: Vec<BTreeSet<String>>,
    r14: Vec<BTreeSet<String>>,
}

/// The joiner over the glia model. `perm` (the control): packet classes, in sorted order,
/// take the rulings of the class at the permuted position, as the earlier V4perm did.
fn glia_join(m: &GliaModel, mask: KMask, perm_seed: Option<&str>) -> GSets {
    let nf = m.code_pk.len();
    let mut edges = Vec::new();
    if mask.on(KT::Lineage) {
        for (a, b, t, _) in &m.lineage {
            edges.push((a.clone(), b.clone(), *t, false));
        }
    }
    let mut st = BTreeMap::new();
    let cl = KLin::build(&edges, 6, &mut st);
    let c = |k: &str| cl.exp(k);
    let mut names: Vec<BTreeSet<String>> = vec![BTreeSet::new(); nf];
    for (i, id) in m.pk_ids.iter().enumerate() {
        let k = c(&format!("pk:{id}"));
        if mask.on(KT::Path) {
            if mask.raw_path {
                for &f in &m.pk_files_raw[i] {
                    names[f as usize].extend(k.iter().cloned());
                }
            } else {
                for &(f, tier, _) in &m.pk_files[i] {
                    if tier <= mask.max_tier {
                        names[f as usize].extend(k.iter().cloned());
                    }
                }
            }
        }
        if mask.on(KT::Text) && mask.max_tier >= Tier::Heuristic {
            for &(f, _) in &m.pk_text[i] {
                names[f as usize].extend(k.iter().cloned());
            }
        }
    }
    let rix: HashMap<&str, usize> = m.rul.iter().enumerate().map(|(i, r)| (r.as_str(), i)).collect();
    let mut rul_of: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (r, refs) in m.rul_pk.iter().enumerate() {
        for x in refs {
            if mask.keeps(x) {
                for k in c(&x.key) {
                    rul_of.entry(k).or_default().insert(format!("rul:{}", m.rul[r]));
                }
            }
        }
    }
    if mask.on(KT::Scoped) {
        for (i, qs) in m.pk_q.iter().enumerate() {
            for q in qs {
                if mask.keeps(q) && rix.contains_key(q.key.trim_start_matches("rul:")) {
                    for k in c(&format!("pk:{}", m.pk_ids[i])) {
                        rul_of.entry(k).or_default().insert(q.key.clone());
                    }
                }
            }
        }
    }
    if let Some(seed) = perm_seed {
        let classes: Vec<String> = m.pk_ids.iter().map(|id| cl.cl.of(&format!("pk:{id}")).to_string()).collect::<BTreeSet<_>>().into_iter().collect();
        let mut perm: Vec<usize> = (0..classes.len()).collect();
        let mut rng = Rng(h64(&format!("{seed}:VR4perm")));
        for k in (1..perm.len()).rev() {
            let j = (rng.next() % (k as u64 + 1)) as usize;
            perm.swap(k, j);
        }
        let old = rul_of.clone();
        rul_of.clear();
        for (k, cls) in classes.iter().enumerate() {
            if let Some(rs) = old.get(&classes[perm[k]]) {
                rul_of.insert(cls.clone(), rs.clone());
            }
        }
    }
    let mut direct: Vec<BTreeSet<String>> = vec![BTreeSet::new(); nf];
    for r in 0..m.rul.len() {
        let rk = format!("rul:{}", m.rul[r]);
        if mask.on(KT::Path) {
            if mask.raw_path {
                for &f in &m.rul_files_raw[r] {
                    direct[f as usize].insert(rk.clone());
                }
            } else {
                for &(f, tier, _) in &m.rul_files[r] {
                    if tier <= mask.max_tier {
                        direct[f as usize].insert(rk.clone());
                    }
                }
            }
        }
        if mask.on(KT::Text) && mask.max_tier >= Tier::Heuristic {
            for &(f, _) in &m.rul_text[r] {
                direct[f as usize].insert(rk.clone());
            }
        }
    }
    let mut out = GSets::default();
    for f in 0..nf {
        let r1c: BTreeSet<String> = if !mask.on(KT::Scoped) {
            BTreeSet::new()
        } else if mask.raw_scoped {
            m.code_pk_raw[f].iter().flat_map(|k| c(k)).collect()
        } else {
            m.code_pk[f].iter().filter(|r| mask.keeps(r)).flat_map(|r| c(&r.key)).collect()
        };
        let mut r1 = r1c.clone();
        r1.extend(names[f].iter().cloned());
        let via = |s: &BTreeSet<String>| -> BTreeSet<String> { s.iter().flat_map(|k| rul_of.get(k).into_iter().flatten().cloned()).collect() };
        let r4c = via(&r1c);
        let r4 = via(&r1);
        let mut r4d = direct[f].clone();
        if mask.on(KT::Scoped) {
            r4d.extend(m.code_q[f].iter().filter(|q| mask.keeps(q)).map(|q| q.key.clone()));
        }
        let mut r4a = r4.clone();
        r4a.extend(r4d.iter().cloned());
        let mut r14 = r1.clone();
        r14.extend(r4a.iter().cloned());
        out.r1c.push(r1c);
        out.r1.push(r1);
        out.r4c.push(r4c);
        out.r4.push(r4);
        out.r4d.push(r4d);
        out.r4a.push(r4a);
        out.r14.push(r14);
    }
    out
}

// ---- the keyed signals of one revision (appended after the earlier run's) ----

/// The keyed signals and controls a mode adds, in order: (full signals, ablated signals,
/// permutation controls). Every test commit's layer carries exactly these names.
fn key_sig_names(mode: VMode) -> (Vec<String>, Vec<String>, Vec<String>) {
    let vars: Vec<&str> = kvariants().iter().map(|v| v.0).collect();
    match mode {
        VMode::Kina => {
            let full: Vec<String> = ["VR1", "VR1h", "VR2u", "VR2d", "VR3b", "VR3h", "VR3a"].iter().map(|s| s.to_string()).collect();
            let mut abl = Vec::new();
            for v in &vars {
                if *v != "-rawcite" {
                    abl.push(format!("VR1h{v}"));
                    abl.push(format!("VR2u{v}"));
                }
                abl.push(format!("VR3h{v}"));
            }
            // round 2: namespaced bare ids, role-aware joining, text as a ranker behind the typed links
            for v in ["+ns", "+roles"] {
                for b in ["VR1h", "VR2u", "VR3h"] {
                    abl.push(format!("{b}{v}"));
                }
            }
            abl.push("VR1h+trank".into());
            abl.push("VR2u+trank".into());
            (full, abl, vec!["VR3hperm".into()])
        }
        VMode::Glia => {
            let full: Vec<String> = ["VR1c", "VR1", "VR4c", "VR4", "VR4d", "VR4a", "VR14"].iter().map(|s| s.to_string()).collect();
            let mut abl = Vec::new();
            for v in &vars {
                if *v != "-rawcite" {
                    abl.push(format!("VR1{v}"));
                    abl.push(format!("VR4a{v}"));
                }
            }
            (full, abl, vec!["VR4cperm".into(), "VR4perm".into()])
        }
        VMode::Quokka => (Vec::new(), Vec::new(), Vec::new()),
    }
}

/// (resolved signal, the earlier run's raw signal) pairs whose co-change links are compared.
fn key_gain_pairs(mode: VMode) -> Vec<(String, String)> {
    let p: &[(&str, &str)] = match mode {
        VMode::Kina => &[("VR1h", "V1m"), ("VR1", "V1"), ("VR2u", "V2"), ("VR2d", "V2"), ("VR3h", "V3"), ("VR3a", "V3"), ("VR3b", "V3")],
        VMode::Glia => &[("VR1", "V1m"), ("VR1c", "V1"), ("VR4c", "V4"), ("VR4a", "V4"), ("VR14", "V14")],
        VMode::Quokka => &[],
    };
    p.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// Appends the keyed signals (and their V0df and permutation controls) to a layer. With no
/// model (a commit with no parent) every set is empty, so the ranker list stays the same.
/// Round 2 (`+trank`): each node's text (the blocks defining a key class, the block itself for
/// `blk:N`, the unit's blocks for `u:N`) scored by BM25 against each file linking the node.
fn trank_weights(sig: &mut Sig, m: &KinaModel, bks: &[KBk], bm: &KBm) {
    let mut def_text: HashMap<&str, Vec<usize>> = HashMap::new();
    for (bi, b) in bks.iter().enumerate() {
        for d in &b.defs {
            def_text.entry(d.1.as_str()).or_default().push(bi);
        }
    }
    let mut unit_blocks: HashMap<u32, Vec<usize>> = HashMap::new();
    for (bi, b) in m.blocks.iter().enumerate() {
        unit_blocks.entry(b.unit).or_default().push(bi);
    }
    let tw: Vec<Vec<f32>> = (0..sig.names.len())
        .into_par_iter()
        .map(|x| {
            let post = &sig.post[x];
            if post.len() < 2 {
                return vec![0.0; post.len()];
            }
            let n = &sig.names[x];
            let blocks: Vec<usize> = if let Some(r) = n.strip_prefix("blk:") {
                r.parse().ok().into_iter().collect()
            } else if let Some(r) = n.strip_prefix("u:") {
                r.parse::<u32>().ok().and_then(|u| unit_blocks.get(&u).cloned()).unwrap_or_default()
            } else {
                def_text.get(n.as_str()).cloned().unwrap_or_default()
            };
            let mut text = String::new();
            for b in blocks {
                if text.len() > 8000 {
                    break;
                }
                text.push_str(&m.blocks[b].text);
                text.push('\n');
            }
            bm.score_files(&text, post).into_iter().map(|v| v as f32).collect()
        })
        .collect();
    sig.tw = tw;
}

#[allow(clippy::too_many_arguments)]
fn key_sigs_kina(m: Option<&KinaModel>, m_ns: Option<&KinaModel>, bm: Option<&KBm>, nf: usize, sha: &str, sigs: &mut Vec<(String, Sig)>, ctrls: &mut Vec<(String, Sig)>) {
    let (full_n, abl_n, perm_n) = key_sig_names(VMode::Kina);
    let empty = vec![BTreeSet::<String>::new(); nf];
    let mut full: Vec<(String, Sig)> = Vec::new();
    let mut abl: Vec<(String, Sig)> = Vec::new();
    let mut perm: Vec<(String, Sig)> = Vec::new();
    match m {
        None => {
            full = full_n.iter().map(|n| (n.clone(), Sig::build(&empty))).collect();
            abl = abl_n.iter().map(|n| (n.clone(), Sig::build(&empty))).collect();
            perm = perm_n.iter().map(|n| (n.clone(), Sig::build(&empty))).collect();
        }
        Some(m) => {
            let base = KBASE;
            let j = kina_join(m, base, None);
            for (n, s) in [("VR1", &j.r1), ("VR1h", &j.r1h), ("VR2u", &j.r2u), ("VR2d", &j.r2d), ("VR3b", &j.r3b), ("VR3h", &j.r3h), ("VR3a", &j.r3a)] {
                full.push((n.to_string(), Sig::build(s)));
            }
            // control: the block -> citations map permuted among the blocks that cite
            let (_, bks, _, _, _) = kina_keys(m, base);
            let citing: Vec<usize> = (0..bks.len()).filter(|&b| !bks[b].cites.is_empty()).collect();
            let mut order: Vec<usize> = (0..citing.len()).collect();
            let mut rng = Rng(h64(&format!("{sha}:VR3hperm")));
            for k in (1..order.len()).rev() {
                let x = (rng.next() % (k as u64 + 1)) as usize;
                order.swap(k, x);
            }
            let pm: HashMap<usize, usize> = citing.iter().enumerate().map(|(i, &b)| (b, citing[order[i]])).collect();
            perm.push(("VR3hperm".into(), Sig::build(&kina_join(m, base, Some(&pm)).r3h)));
            for (v, mask) in kvariants() {
                let jv = kina_join(m, mask, None);
                if v != "-rawcite" {
                    abl.push((format!("VR1h{v}"), Sig::build(&jv.r1h)));
                    abl.push((format!("VR2u{v}"), Sig::build(&jv.r2u)));
                }
                abl.push((format!("VR3h{v}"), Sig::build(&jv.r3h)));
            }
            // round 2
            let jn = kina_join(m_ns.expect("the namespaced model"), base, None);
            abl.push(("VR1h+ns".into(), Sig::build(&jn.r1h)));
            abl.push(("VR2u+ns".into(), Sig::build(&jn.r2u)));
            abl.push(("VR3h+ns".into(), Sig::build(&jn.r3h)));
            let jr = kina_join(m, KROLES, None);
            abl.push(("VR1h+roles".into(), Sig::build_w(&jr.r1hw)));
            abl.push(("VR2u+roles".into(), Sig::build_w(&jr.r2uw)));
            abl.push(("VR3h+roles".into(), Sig::build(&jr.r3h)));
            let mut t1 = Sig::build(&j.r1h);
            let mut t2 = Sig::build(&j.r2u);
            if let Some(bm) = bm {
                trank_weights(&mut t1, m, &bks, bm);
                trank_weights(&mut t2, m, &bks, bm);
            }
            abl.push(("VR1h+trank".into(), t1));
            abl.push(("VR2u+trank".into(), t2));
            // abl was pushed per variant; reorder to key_sig_names' order
            let pos: HashMap<&String, usize> = abl_n.iter().enumerate().map(|(i, n)| (n, i)).collect();
            abl.sort_by_key(|(n, _)| pos[n]);
        }
    }
    for (n, s) in &full {
        controls(n, s, sha, ctrls);
    }
    sigs.extend(full);
    sigs.extend(abl);
    ctrls.extend(perm);
}

fn key_sigs_glia(m: Option<&GliaModel>, nf: usize, sha: &str, sigs: &mut Vec<(String, Sig)>, ctrls: &mut Vec<(String, Sig)>) {
    let (full_n, abl_n, perm_n) = key_sig_names(VMode::Glia);
    let empty = vec![BTreeSet::<String>::new(); nf];
    let mut full: Vec<(String, Sig)> = Vec::new();
    let mut abl: Vec<(String, Sig)> = Vec::new();
    let mut perm: Vec<(String, Sig)> = Vec::new();
    match m {
        None => {
            full = full_n.iter().map(|n| (n.clone(), Sig::build(&empty))).collect();
            abl = abl_n.iter().map(|n| (n.clone(), Sig::build(&empty))).collect();
            perm = perm_n.iter().map(|n| (n.clone(), Sig::build(&empty))).collect();
        }
        Some(m) => {
            let base = KBASE;
            let j = glia_join(m, base, None);
            for (n, s) in [("VR1c", &j.r1c), ("VR1", &j.r1), ("VR4c", &j.r4c), ("VR4", &j.r4), ("VR4d", &j.r4d), ("VR4a", &j.r4a), ("VR14", &j.r14)] {
                full.push((n.to_string(), Sig::build(s)));
            }
            let jp = glia_join(m, base, Some(sha));
            perm.push(("VR4cperm".into(), Sig::build(&jp.r4c)));
            perm.push(("VR4perm".into(), Sig::build(&jp.r4)));
            for (v, mask) in kvariants() {
                if v == "-rawcite" {
                    continue;
                }
                let jv = glia_join(m, mask, None);
                abl.push((format!("VR1{v}"), Sig::build(&jv.r1)));
                abl.push((format!("VR4a{v}"), Sig::build(&jv.r4a)));
            }
        }
    }
    for (n, s) in &full {
        controls(n, s, sha, ctrls);
    }
    sigs.extend(full);
    sigs.extend(abl);
    ctrls.extend(perm);
}

fn ke_dir(env: &VEnv) -> String {
    env.keys_out.clone()
}

/// Round 2: every bare-id mention at the newest test commit's parent, its base and namespaced
/// resolution (`ns_audit.tsv`, D-ids first; judged by hand).
fn write_ns_audit(dir: &str, rows: &[(String, String, String, String, &'static str)]) {
    std::fs::create_dir_all(dir).expect("keys out dir");
    let mut v: Vec<&(String, String, String, String, &'static str)> = rows.iter().collect();
    v.sort_by_key(|a| (!a.1.starts_with('D'), h64(&a.0)));
    let mut out = String::from("n\tat\tmention\tbase\tns\tns_route\tchanged\tverdict_ns\tverdict_base\n");
    for (i, (at, men, b, n, r)) in v.iter().enumerate() {
        let changed = b.split(' ').next() != n.split(' ').next();
        out.push_str(&format!("{}\t{at}\t{}\t{b}\t{n}\t{r}\t{}\t\t\n", i + 1, men.replace('\t', " "), if changed { "yes" } else { "no" }));
    }
    std::fs::write(format!("{dir}/ns_audit.tsv"), out).expect("write ns audit");
}

/// Writes one audit file per key type: up to 80 resolved links in a deterministic hash order.
/// Writes one audit file per key type: up to 80 resolved links, drawn round-robin over the
/// routes (each route in a deterministic hash order), with each route's share of all links so
/// a per-route precision can be weighted back to the whole.
fn write_audit(dir: &str, tag: &str, rows: &[KAudit]) {
    std::fs::create_dir_all(dir).expect("keys out dir");
    for kt in KTS {
        let all: Vec<&KAudit> = rows.iter().filter(|a| a.kt == kt).collect();
        let mut by: BTreeMap<&str, Vec<&KAudit>> = BTreeMap::new();
        for a in &all {
            by.entry(a.route).or_default().push(a);
        }
        for v in by.values_mut() {
            v.sort_by_key(|a| h64(&format!("{}|{}|{}", a.at, a.mention, a.target)));
            v.dedup_by(|a, b| a.at == b.at && a.mention == b.mention && a.target == b.target);
        }
        let mut pick: Vec<&KAudit> = Vec::new();
        let mut k = 0;
        while pick.len() < 80 && by.values().any(|v| v.len() > k) {
            for v in by.values() {
                if let Some(a) = v.get(k)
                    && pick.len() < 80
                {
                    pick.push(a);
                }
            }
            k += 1;
        }
        let mut s = String::from("n\ttier\troute\troute_share\tat\tmention\ttarget\tverdict\n");
        for (i, a) in pick.iter().enumerate() {
            let share = all.iter().filter(|x| x.route == a.route).count() as f64 / all.len().max(1) as f64;
            s.push_str(&format!("{}\t{}\t{}\t{share:.4}\t{}\t{}\t{}\t\n", i + 1, a.tier.name(), a.route, a.at, a.mention, a.target));
        }
        std::fs::write(format!("{dir}/audit_{tag}_{}.tsv", kt.name()), s).expect("write audit");
    }
}

// ---- (b) the law-impact chains on resolved keys ----

/// The resolved chains, in order: R1 (units citing the section -> the ids they and their
/// enclosing headings define -> code citing one of the ids' classes), R3 (R1 plus the files
/// the ids' defining blocks name), R5 (R3 plus the rows whose own row cites the section, their
/// lineage classes, the code citing them and the files every row of the class names), R5h (R5
/// after one goal-internal hop: blocks that mention a reached class add the classes they
/// define), R5u (R5h plus the files the citing units name themselves).
const KLAW_CHAINS: [&str; 6] = ["R1", "R1m", "R3", "R5", "R5h", "R5u"];
const KR1: usize = 0;
const KR1M: usize = 1;
const KR3: usize = 2;
const KR5: usize = 3;
const KR5H: usize = 4;

/// Per resolved section of the event's instrument, the files each resolved chain reaches.
type KReach = BTreeMap<String, [BTreeSet<u32>; 6]>;

fn klaw_reach(m: &KinaModel, mask: KMask, prefix: &str) -> KReach {
    let (cl, bks, code_ids, _, names) = kina_keys(m, mask);
    let mut members: HashMap<String, BTreeSet<String>> = HashMap::new();
    for b in &m.blocks {
        let keys: Vec<&str> = if mask.raw_cite {
            b.cites_raw.iter().map(String::as_str).collect()
        } else {
            b.refs.iter().filter(|r| r.kt == KT::Cite).map(|r| r.key.as_str()).collect()
        };
        for k in keys {
            if let Some(s) = k.strip_prefix(prefix) {
                for c in cl.exp(k) {
                    members.entry(c).or_default().insert(s.to_string());
                }
            }
        }
    }
    let nb = bks.len();
    let mut bfiles: Vec<Vec<u32>> = vec![Vec::new(); nb];
    for (f, bs) in names.iter().enumerate() {
        for &b in bs {
            bfiles[b as usize].push(f as u32);
        }
    }
    let mut citing: HashMap<&str, Vec<u32>> = HashMap::new();
    for (f, ks) in code_ids.iter().enumerate() {
        for k in ks {
            citing.entry(k.as_str()).or_default().push(f as u32);
        }
    }
    let mut def_b: HashMap<&str, Vec<u32>> = HashMap::new();
    let mut ref_b: HashMap<&str, Vec<u32>> = HashMap::new();
    let mut unit_b: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut cite_b: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    for (bi, b) in bks.iter().enumerate() {
        for d in &b.defs {
            def_b.entry(d.1.as_str()).or_default().push(bi as u32);
        }
        for r in if mask.roles { &b.impl_refs } else { &b.refs } {
            ref_b.entry(r.as_str()).or_default().push(bi as u32);
        }
        unit_b.entry(m.blocks[bi].unit).or_default().push(bi as u32);
        for c in &b.cites {
            for s in members.get(c).into_iter().flatten() {
                cite_b.entry(s.clone()).or_default().insert(bi as u32);
            }
        }
    }
    let files_of = |ks: &BTreeSet<String>, named: bool| -> BTreeSet<u32> {
        let mut out = BTreeSet::new();
        for k in ks {
            out.extend(citing.get(k.as_str()).into_iter().flatten().copied());
            if named {
                for &b in def_b.get(k.as_str()).into_iter().flatten() {
                    out.extend(bfiles[b as usize].iter().copied());
                }
            }
        }
        out
    };
    let mut out = BTreeMap::new();
    for (s, cb) in &cite_b {
        let units: BTreeSet<u32> = cb.iter().map(|&b| m.blocks[b as usize].unit).collect();
        let mut ids1: BTreeSet<String> = BTreeSet::new();
        for &u in &units {
            for &b in unit_b.get(&u).into_iter().flatten() {
                ids1.extend(bks[b as usize].defs.iter().filter(|d| d.0 == KT::Scoped).map(|d| d.1.clone()));
            }
            // the enclosing headings' ids (a unit under `## ABC-03` carries ABC-03)
            let mut a = m.head_parent[u as usize];
            while let Some(p) = a {
                for &b in unit_b.get(&p).into_iter().flatten() {
                    if m.blocks[b as usize].kind == b'h' {
                        ids1.extend(bks[b as usize].defs.iter().filter(|d| d.0 == KT::Scoped).map(|d| d.1.clone()));
                    }
                }
                a = m.head_parent[p as usize];
            }
        }
        let k1 = files_of(&ids1, false);
        // R1m: plus the classes the citing units mention (C1m's resolved form)
        let mut ment: BTreeSet<String> = ids1.clone();
        for &u in &units {
            for &b in unit_b.get(&u).into_iter().flatten() {
                ment.extend(if mask.roles { bks[b as usize].impl_refs.iter() } else { bks[b as usize].refs.iter() }.cloned());
            }
        }
        let k1m = files_of(&ment, false);
        let k3 = files_of(&ids1, true);
        let mut rows: BTreeSet<String> = BTreeSet::new();
        for &b in cb {
            rows.extend(bks[b as usize].defs.iter().filter(|d| d.0 == KT::Row).map(|d| d.1.clone()));
        }
        let mut k5 = k3.clone();
        k5.extend(files_of(&rows, true));
        let ids5: BTreeSet<String> = ids1.union(&rows).cloned().collect();
        let mut hop: BTreeSet<String> = BTreeSet::new();
        for k in &ids5 {
            let rb: &[u32] = ref_b.get(k.as_str()).map_or(&[], |v| &v[..]);
            let nd = def_b.get(k.as_str()).map_or(0, Vec::len);
            if rb.len() + nd > KHUB {
                continue;
            }
            for &b in rb {
                hop.extend(bks[b as usize].defs.iter().map(|d| d.1.clone()));
            }
        }
        let mut k5h = k5.clone();
        k5h.extend(files_of(&hop, true));
        let mut k5u = k5h.clone();
        for &u in &units {
            for &b in unit_b.get(&u).into_iter().flatten() {
                k5u.extend(bfiles[b as usize].iter().copied());
            }
        }
        out.insert(s.clone(), [k1, k1m, k3, k5, k5h, k5u]);
    }
    out
}

/// The law-impact run's inputs the keyed chains are scored with.
struct KLawIn<'a> {
    args: &'a Args,
    g: &'a Graph,
    ppr: &'a Ppr,
    docs: &'a [(String, String)],
    start: &'a str,
    truths: &'a [(&'a str, &'a str, LTruth)],
    commits: &'a [LCommit],
    /// the target spec (the truths' spec unit)
    tspec: &'a str,
    changed: &'a BTreeSet<String>,
    status: &'a BTreeMap<String, &'static str>,
    bm_all: &'a [f64],
    ppr_comp: &'a [f64],
    comp_score: &'a [f64],
    /// the earlier chains (C1, C1m, C3, C5) from the changed sections, as file sets
    raw_sets: Vec<(&'static str, Vec<u32>)>,
    /// the earlier run's home-cited sections (known to either version) and how many changed
    raw_cited: (usize, usize),
    /// Round 2: the instrument the event is about: its key prefix and name, and the section numbers
    /// of its other version the citation resolver should also accept
    prefix: &'static str,
    inst: &'static str,
    extra_secs: Vec<String>,
}

fn keys_law(x: &KLawIn) {
    let t0 = Instant::now();
    let dir = &x.args.keys_out;
    std::fs::create_dir_all(dir).expect("keys out dir");
    let nf = x.g.paths.len();
    let mut known = law_known(x.args);
    if x.prefix == prv().key_trules {
        known.trules.extend(x.extra_secs.iter().cloned());
    }
    let renames = git_renames(&x.args.git, x.start);
    let kp = KPaths::new(&x.g.paths, &renames);
    let pr = PathRes::new(&x.g.paths);
    let code_lines = git_grep(&x.args.git, x.start, Some(&prv().keys_code_pat), &[]);
    let blobs = KBlobs::load(&x.args.git, &[x.start.to_string()], &kp);
    let bm = blobs.index(x.start, nf);
    let km = kina_model(x.docs, &code_lines, &kp, &pr, &known, Some(&bm), nf, true, false);
    let km_ns = kina_model(x.docs, &code_lines, &kp, &pr, &known, Some(&bm), nf, true, true);
    write_audit(dir, "law", &km.audit);
    let base = KBASE;
    let mut variants: Vec<(String, KMask)> = vec![("R (resolved, no text)".to_string(), base)];
    variants.extend(kvariants().into_iter().map(|(n, m)| (n.to_string(), m)));
    let mut reaches: Vec<(String, KReach)> = variants.par_iter().map(|(n, m)| (n.clone(), klaw_reach(&km, *m, x.prefix))).collect();
    // round 2: namespaced bare ids, role-aware joining, both
    reaches.push(("+ns".into(), klaw_reach(&km_ns, KBASE, x.prefix)));
    reaches.push(("+roles".into(), klaw_reach(&km, KROLES, x.prefix)));
    reaches.push(("+ns+roles".into(), klaw_reach(&km_ns, KROLES, x.prefix)));
    write_ns_audit(&format!("{dir}/law_ns"), &km_ns.ns_audit);
    let reach = &reaches[0].1;
    let cited: Vec<String> = {
        let mut v: Vec<String> = reach.keys().filter(|s| x.status.contains_key(*s)).cloned().collect();
        v.sort_by_key(|n| law_key(n));
        v
    };
    let cited_changed: BTreeSet<String> = cited.iter().filter(|s| x.changed.contains(*s)).cloned().collect();
    let cited_unchanged: BTreeSet<String> = cited.iter().filter(|s| !x.changed.contains(*s)).cloned().collect();
    let cited_all: BTreeSet<String> = cited.iter().cloned().collect();
    let kind_files = |r: &KReach, ki: usize| -> BTreeMap<String, BTreeSet<u32>> {
        cited.iter().map(|s| (s.clone(), r.get(s).map(|a| a[ki].clone()).unwrap_or_default())).collect()
    };
    let score_of = |r: &KReach, ki: usize, inputs: &BTreeSet<String>| -> Vec<f64> {
        let sets: Vec<&BTreeSet<u32>> = inputs.iter().filter_map(|s| r.get(s).map(|a| &a[ki])).collect();
        law_score(&sets, nf)
    };
    let set_of = |sc: &[f64]| -> Vec<u32> { law_rank(sc).into_iter().filter(|&f| sc[f as usize] > 0.0).collect() };
    let mut md = String::new();
    let mut js = serde_json::Map::new();
    // ---- coverage of the hops ----
    let cite_stats: BTreeMap<&String, &f64> = km.stats.iter().filter(|(k, _)| k.starts_with("k_cite") || k.starts_with("k_lineage") || k.starts_with("k_row") || k.starts_with("k_text") || k.starts_with("k_path") || k.starts_with("k_scoped")).collect();
    md.push_str(&format!(
        "# Law impact on resolved keys\n\nStart {}; generated by `rank_eval --law-impact kina --keys on` in {:.1}s. Blocks {}, pages {}, lineage edges {}, code files with a resolved key {}.\n\n",
        x.start,
        t0.elapsed().as_secs_f64(),
        km.blocks.len(),
        km.pages.len(),
        km.lineage.len(),
        km.code_refs.iter().filter(|v| !v.is_empty()).count()
    ));
    md.push_str(&format!(
        "{} sections cited by the goal layer: resolved {} ({} changed, {} unchanged); the earlier resolver {} ({} changed).\n\n",
        x.inst,
        cited.len(),
        cited_changed.len(),
        cited_unchanged.len(),
        x.raw_cited.0,
        x.raw_cited.1
    ));
    md.push_str("Key-mention counts at the start (by route):\n\n| counter | n |\n|---|---|\n");
    for (k, v) in &cite_stats {
        md.push_str(&format!("| {k} | {v} |\n"));
    }
    js.insert("stats".into(), json!(km.stats));
    js.insert("cited".into(), json!({"resolved": cited.len(), "changed": cited_changed.len(), "unchanged": cited_unchanged.len(), "raw": x.raw_cited.0, "raw_changed": x.raw_cited.1}));
    md.push_str("\nReach per chain (changed cited sections reaching a file / files reached):\n\n| chain | sections reaching | files |\n|---|---|---|\n");
    for (ki, cn) in KLAW_CHAINS.iter().enumerate() {
        let secs = cited_changed.iter().filter(|s| reach.get(*s).is_some_and(|a| !a[ki].is_empty())).count();
        let files: BTreeSet<u32> = cited_changed.iter().flat_map(|s| reach.get(s).map(|a| a[ki].clone()).unwrap_or_default()).collect();
        md.push_str(&format!("| {cn} | {secs} / {} | {} |\n", cited_changed.len(), files.len()));
    }
    // ---- scores per truth ----
    let mut jt = serde_json::Map::new();
    for (tname, tdesc, (truth, ncommits, _)) in x.truths {
        let tcommits: Vec<&Vec<u32>> = x
            .commits
            .iter()
            .filter(|c| law_in_truth(tname, c, x.tspec))
            .map(|c| &c.code)
            .collect();
        md.push_str(&format!("\n## Truth {tname}: {tdesc}\n\n{ncommits} commits, {} targets, {nf} candidates.\n\n| ranker | set size | hits | precision | recall | F1 | hits@10 / recall@10 | hits@20 / recall@20 |\n|---|---|---|---|---|---|---|---|\n", truth.len()));
        let mut rows: Vec<(String, LMet)> = Vec::new();
        for (n, s) in &x.raw_sets {
            rows.push((format!("{n} (earlier, raw keys)"), law_met(s, s, truth)));
        }
        let mut sets: Vec<(usize, Vec<u32>, Vec<f64>)> = Vec::new();
        for (ki, cn) in KLAW_CHAINS.iter().enumerate() {
            let sc = score_of(reach, ki, &cited_changed);
            rows.push((format!("{cn} (resolved, changed sections)"), law_met_scores(&sc, truth)));
            sets.push((ki, set_of(&sc), sc));
        }
        for (ki, cn) in KLAW_CHAINS.iter().enumerate() {
            rows.push((format!("{cn} from UNCHANGED cited sections (control)"), law_met_scores(&score_of(reach, ki, &cited_unchanged), truth)));
            rows.push((format!("{cn} from ALL cited sections"), law_met_scores(&score_of(reach, ki, &cited_all), truth)));
        }
        let plus_text = reaches.iter().find(|r| r.0 == "+text").map(|r| &r.1);
        if let Some(rt) = plus_text {
            for (ki, cn) in KLAW_CHAINS.iter().enumerate().skip(KR5H) {
                rows.push((format!("{cn} +text (HEURISTIC BM25 fallback)"), law_met_scores(&score_of(rt, ki, &cited_changed), truth)));
            }
        }
        let mut dists: Vec<(String, LDist)> = Vec::new();
        let mut boots: Vec<(String, (f64, f64, f64))> = Vec::new();
        for (ki, set, sc) in &sets {
            let cn = KLAW_CHAINS[*ki];
            let k = set.len();
            let f1 = law_met_scores(sc, truth)[4];
            let top = |v: &[f64]| -> Vec<u32> { law_rank(v).into_iter().take(k).collect() };
            let pp = law_ppr(x.g, x.ppr, set);
            for (bn, bv) in [("BM25 all changed text", x.bm_all), ("PPR from compliance code", x.ppr_comp), ("compliance code by id idf", x.comp_score), ("PPR from the chain's own files", &pp[..])] {
                rows.push((format!("{bn}, top |{cn}|={k}"), law_met_topk(bv, k, truth)));
                boots.push((format!("{cn} - {bn}, both at {k}"), law_boot(&tcommits, set, &top(bv), &format!("{tname}:k:{cn}:{bn}"))));
            }
            dists.push((format!("random files, size |{cn}|"), law_random(nf, k, truth, &format!("{tname}:krand:{cn}"), f1)));
            dists.push((format!("{cn}, section -> reach map permuted over the resolved cited sections"), law_permuted(&cited, &cited_changed, &kind_files(reach, *ki), nf, truth, &format!("{tname}:kperm:{cn}"), f1)));
        }
        for (rn, rset) in &x.raw_sets {
            let ki = match *rn {
                "C1" => KR1,
                "C1m" => KR1M,
                "C3" => KR3,
                _ => KR5,
            };
            boots.push((format!("{} - {rn} (resolved minus raw)", KLAW_CHAINS[ki]), law_boot(&tcommits, &sets[ki].1, rset, &format!("{tname}:kr:{rn}"))));
            if *rn == "C5" {
                boots.push((format!("R5h - {rn} (resolved minus raw)"), law_boot(&tcommits, &sets[KR5H].1, rset, &format!("{tname}:kr:R5h"))));
            }
        }
        let mut jr = serde_json::Map::new();
        for (n, m) in &rows {
            md.push_str(&format!("| {n} | {} |\n", law_fmt(m)));
            jr.insert(n.clone(), law_json(m));
        }
        md.push_str("\nControls, mean [95% interval] over 1000 draws (last column: share of draws with F1 >= the real chain's):\n\n| control | set size | hits | precision | recall | F1 | hits@10 / recall@10 | hits@20 / recall@20 | share F1 >= real |\n|---|---|---|---|---|---|---|---|---|\n");
        for (n, d) in &dists {
            md.push_str(&format!("| {n} | {} | {:.3} |\n", law_fmt_dist(d), d.ge));
            jr.insert(n.clone(), law_json_dist(d));
        }
        md.push_str("\nF1 differences, paired commit-cluster bootstrap (1000 resamples), mean [95% interval]:\n\n| comparison | F1 difference |\n|---|---|\n");
        for (n, (mm, lo, hi)) in &boots {
            md.push_str(&format!("| {n} | {mm:+.3} [{lo:+.3}, {hi:+.3}] |\n"));
            jr.insert(format!("boot: {n}"), json!({"mean": mm, "lo": lo, "hi": hi}));
        }
        // ablation: which resolvers matter (R5h, and R1 / R3)
        md.push_str("\nAblation (from the changed sections; each row drops one key type, or reads it raw):\n\n| variant | cited sections | R1 F1 (size) | R1m F1 (size) | R3 F1 (size) | R5 F1 (size) | R5h P / R / F1 (size) |\n|---|---|---|---|---|---|---|\n");
        let mut ja = serde_json::Map::new();
        for (vn, r) in &reaches {
            let cc: BTreeSet<String> = r.keys().filter(|s| x.changed.contains(*s)).cloned().collect();
            let m1 = law_met_scores(&score_of(r, KR1, &cc), truth);
            let m1m = law_met_scores(&score_of(r, KR1M, &cc), truth);
            let m3 = law_met_scores(&score_of(r, KR3, &cc), truth);
            let m5r = law_met_scores(&score_of(r, KR5, &cc), truth);
            let m5 = law_met_scores(&score_of(r, KR5H, &cc), truth);
            md.push_str(&format!(
                "| {vn} | {} | {:.3} ({:.0}) | {:.3} ({:.0}) | {:.3} ({:.0}) | {:.3} ({:.0}) | {:.3} / {:.3} / {:.3} ({:.0}) |\n",
                r.keys().filter(|s| x.status.contains_key(*s)).count(),
                m1[4],
                m1[0],
                m1m[4],
                m1m[0],
                m3[4],
                m3[0],
                m5r[4],
                m5r[0],
                m5[2],
                m5[3],
                m5[4],
                m5[0]
            ));
            ja.insert(vn.clone(), json!({"R1": law_json(&m1), "R1m": law_json(&m1m), "R3": law_json(&m3), "R5": law_json(&m5r), "R5h": law_json(&m5)}));
        }
        jr.insert("ablation".into(), serde_json::Value::Object(ja));
        // round 2: each variant against the base chains (paired commit bootstrap) and its permuted map
        md.push_str("\nRound 2 variants against the base chains (from the changed sections; F1, variant minus base with a paired commit-cluster bootstrap, and the variant's own permuted-map control: mean F1, share of draws >= the real chain):\n\n| variant | chain | files | F1 | minus base | permuted mean F1 | share >= real |\n|---|---|---|---|---|---|---|\n");
        let mut jv = serde_json::Map::new();
        for (vn, r) in reaches.iter().filter(|(n, _)| n.starts_with("+ns") || n == "+roles") {
            let cited_v: Vec<String> = {
                let mut v: Vec<String> = r.keys().filter(|s| x.status.contains_key(*s)).cloned().collect();
                v.sort_by_key(|n| law_key(n));
                v
            };
            let cc: BTreeSet<String> = cited_v.iter().filter(|s| x.changed.contains(*s)).cloned().collect();
            for (ki, cn) in KLAW_CHAINS.iter().enumerate().take(KR5H + 1) {
                let sc = score_of(r, ki, &cc);
                let set = set_of(&sc);
                let mt = law_met_scores(&sc, truth);
                let b = law_boot(&tcommits, &set, &sets[ki].1, &format!("{tname}:r2:{vn}:{cn}"));
                let kf: BTreeMap<String, BTreeSet<u32>> = cited_v.iter().map(|s| (s.clone(), r.get(s).map(|a| a[ki].clone()).unwrap_or_default())).collect();
                let d = law_permuted(&cited_v, &cc, &kf, nf, truth, &format!("{tname}:r2perm:{vn}:{cn}"), mt[4]);
                md.push_str(&format!("| {vn} | {cn} | {} | {:.3} | {:+.3} [{:+.3}, {:+.3}] | {:.3} | {:.3} |\n", set.len(), mt[4], b.0, b.1, b.2, d.mean[4], d.ge));
                jv.insert(format!("{vn} {cn}"), json!({"files": set.len(), "f1": mt[4], "minus_base": [b.0, b.1, b.2], "perm_mean_f1": d.mean[4], "perm_share_ge": d.ge}));
            }
        }
        jr.insert("round2_variants".into(), serde_json::Value::Object(jv));
        jt.insert(tname.to_string(), serde_json::Value::Object(jr));
    }
    js.insert("scores".into(), serde_json::Value::Object(jt));
    // ---- specificity: are the files changed sections reach different from the unchanged ones'? ----
    let s3 = &x.truths[0].2.0;
    md.push_str("\n## Section specificity («ts» truth)\n\n| chain | files reached only by changed sections: n / precision | only by unchanged: n / precision | by both: n / precision |\n|---|---|---|---|\n");
    for (ki, cn) in KLAW_CHAINS.iter().enumerate() {
        let a: BTreeSet<u32> = cited_changed.iter().flat_map(|s| reach[s][ki].clone()).collect();
        let b: BTreeSet<u32> = cited_unchanged.iter().flat_map(|s| reach[s][ki].clone()).collect();
        let p = |v: Vec<&u32>| -> String { format!("{} / {:.3}", v.len(), v.iter().filter(|f| s3.contains(f)).count() as f64 / v.len().max(1) as f64) };
        md.push_str(&format!("| {cn} | {} | {} | {} |\n", p(a.difference(&b).collect()), p(b.difference(&a).collect()), p(a.intersection(&b).collect())));
    }
    kwrite(format!("{dir}/keys_law_tables.md"), &md).expect("write keys law md");
    kwrite(format!("{dir}/keys_law.json"), &serde_json::to_string_pretty(&serde_json::Value::Object(js)).unwrap()).expect("write keys law json");
    eprintln!("[keys] law: {} resolved cited sections ({} changed) in {:.1}s -> {dir}/keys_law_tables.md", cited.len(), cited_changed.len(), t0.elapsed().as_secs_f64());
}

// ============================================================================
// Round 1 (2026-10-18): realistic intent, the work neighbourhood, milestones,
// and PPR tuning with a hold-out (`--round1 on [--pre glia|kina] [--tune on]`).
//
// James: "intent comes from people's purpose to create ... sometimes it's a
// general idea made up of little milestones and ideas". The first run's T1 read
// each test commit's own message, written AFTER the change. Every signal here is
// something that exists BEFORE the change:
// - T1s: BM25 over older commits (full messages) with only the subject line as
//   the query; T1ss: the same over older commits' subject lines.
// - T1pre: the plan text the commit names, read at its parent: glia, the named
//   packets' title + rationale (leap-*-packets.json); Kina, the task / requirement
//   rows the commit's ids name (Kina's docs + the wiki current at the parent).
// - W_k: the files the last k training commits touched, recency-decayed.
// - M: the files earlier commits of the same milestone touched; the milestone is
//   a label known before the change (glia: the named packet's family; a merged
//   branch), else a time-gap + file-overlap segmentation of the history.
// Each has a control: W with the window moved to a random earlier point, M with
// the milestone swapped for one of the recently active other milestones.
// ============================================================================

const R1_KS: [usize; 4] = [1, 3, 5, 10];
/// W_k: the j-th most recent training commit (j = 1 the newest) weighs R1_DECAY^(j-1).
const R1_DECAY: f64 = 0.75;
/// Mgap: a commit continues the current milestone only within this gap (author time, seconds) ...
const R1_GAP_S: i64 = 24 * 3600;
/// ... and only when it shares a file with the milestone's last this-many commits.
const R1_OVERLAP_LAST: usize = 3;
/// The milestone controls draw one of this many most recently active other milestones.
const R1_SHUF_RECENT: usize = 5;
/// The PPR tuning grid's edge-weight sets, from the first run's inefficiency map (overrides on the base table).
const R1_WSETS: [(&str, &[(EdgeCategoryId, f64)]); 4] = [
    ("base", &[]),
    ("WA", &[(ec::HTTP_CALLS, 2.0), (ec::HANDLED_BY, 2.0), (ec::TESTS, 4.0), (ec::IMPLEMENTS, 3.0), (ec::INHERITS_FROM, 3.0)]),
    ("WB", &[(ec::HTTP_CALLS, 1.0), (ec::HANDLED_BY, 1.0), (ec::TESTS, 6.0), (ec::IMPLEMENTS, 5.0), (ec::INHERITS_FROM, 5.0)]),
    (
        "WC",
        &[
            (ec::HTTP_CALLS, 1.0),
            (ec::HANDLED_BY, 1.0),
            (ec::TESTS, 6.0),
            (ec::IMPLEMENTS, 5.0),
            (ec::INHERITS_FROM, 5.0),
            (ec::INJECTS, 1.0),
            (ec::NAVIGATES_TO, 1.0),
        ],
    ),
];
const R1_DAMP: [f64; 4] = [0.3, 0.5, 0.7, 0.85];

/// A query without its digit-bearing tokens (reused bare ids such as T019, migration and version numbers).
fn no_digits(toks: &[String]) -> Vec<String> {
    toks.iter().filter(|t| !t.bytes().any(|b| b.is_ascii_digit())).cloned().collect()
}

fn subject(msg: &str) -> &str {
    msg.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("")
}

/// The id-bearing part of a message: its subject when the subject names an id, else the whole message.
fn id_text(msg: &str, has: impl Fn(&str) -> bool) -> &str {
    let s = subject(msg);
    if has(s) { s } else { msg }
}

fn git_author_times(git: &str, rev: &str) -> HashMap<String, i64> {
    let text = git_text(git, &["log", "--format=%H %at", rev]).expect("git log author times");
    text.lines()
        .filter_map(|l| {
            let (s, t) = l.split_once(' ')?;
            Some((s.to_string(), t.trim().parse().ok()?))
        })
        .collect()
}

/// The branch a merge commit's subject names (None for a subtree import of another repository).
fn merge_label(subj: &str) -> Option<String> {
    if subj.starts_with("Merge commit '") && subj.contains("' as '") {
        return None;
    }
    if let Some(r) = subj.strip_prefix("Merge pull request #") {
        let num: String = r.chars().take_while(char::is_ascii_digit).collect();
        return Some(format!("pr#{num}"));
    }
    for pre in ["Merge branch '", "Merge remote-tracking branch '"] {
        if let Some(r) = subj.strip_prefix(pre) {
            return r.split('\'').next().map(str::to_string);
        }
    }
    let r = subj.strip_prefix("Merge ")?;
    let (head, into) = match r.rsplit_once(" into ") {
        Some((h, i)) => (h, Some(i.trim().trim_matches('\''))),
        None => (r, None),
    };
    let head = head.split(" (").next().unwrap_or(head).trim().trim_matches('\'');
    if head.is_empty() {
        return None;
    }
    Some(match into {
        Some(i) if !matches!(i, "main" | "master") => format!("{head}@{i}"),
        _ => head.to_string(),
    })
}

/// sha -> the branch it came in on: the smallest second-parent side of a merge whose
/// subject names a branch (so a lane merged into a wave branch keeps the lane).
fn branch_labels(git: &str, rev: &str) -> HashMap<String, String> {
    let text = git_text(git, &["log", "--merges", "--format=%H%x1f%P%x1f%s", rev]).unwrap_or_default();
    let mut best: HashMap<String, (usize, String)> = HashMap::new();
    for l in text.lines() {
        let mut p = l.split('\x1f');
        let (Some(_), Some(ps), Some(subj)) = (p.next(), p.next(), p.next()) else { continue };
        let ps: Vec<&str> = ps.split_whitespace().collect();
        if ps.len() < 2 {
            continue;
        }
        let Some(lab) = merge_label(subj) else { continue };
        let side = git_text(git, &["rev-list", "--no-merges", ps[1], &format!("^{}", ps[0])]).unwrap_or_default();
        let shas: Vec<&str> = side.lines().collect();
        let n = shas.len();
        for s in shas {
            let e = best.entry(s.to_string()).or_insert((usize::MAX, String::new()));
            if n < e.0 {
                *e = (n, lab.clone());
            }
        }
    }
    best.into_iter().map(|(k, (_, l))| (k, l)).collect()
}

/// A glia wave-runner landing commit: "wave W12 landed", "leap wave W3 landed", "wave 7 landed".
fn wave_landing(subj: &str) -> bool {
    let b = subj.as_bytes();
    let mut i = 0;
    while let Some(off) = subj[i..].find("wave ") {
        let j = i + off + 5;
        let k = if subj[j..].starts_with('W') { j + 1 } else { j };
        let e = digits_at(b, k, 4);
        if e > k && subj[e..].starts_with(" landed") {
            return true;
        }
        i = j;
    }
    false
}

/// glia: packet id -> "title\nrationale", per packets-JSON blob.
#[derive(Default)]
struct PreCache {
    m: std::sync::Mutex<HashMap<String, std::sync::Arc<HashMap<String, String>>>>,
}

impl PreCache {
    fn get(&self, git: &str, blob: &str) -> std::sync::Arc<HashMap<String, String>> {
        if let Some(x) = self.m.lock().unwrap().get(blob) {
            return x.clone();
        }
        let text = git_text(git, &["cat-file", "-p", blob]).unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        let mut out = HashMap::new();
        for p in v.get("packets").and_then(|p| p.as_array()).into_iter().flatten() {
            let Some(id) = p.get("id").and_then(|x| x.as_str()) else { continue };
            let f = |k: &str| p.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            out.insert(id.to_string(), format!("{}\n{}", f("title"), f("rationale")));
        }
        let a = std::sync::Arc::new(out);
        self.m.lock().unwrap().insert(blob.to_string(), a.clone());
        a
    }
}

/// A test commit's pre-change text, and how many ids its message named and
/// how many of those the parent held.
type PreText = (Option<String>, usize, usize);

/// glia: the title + rationale, at `parent`, of the packets the message names (its subject's ids, else the
/// whole message's); returns (text, ids named, ids found at the parent).
fn glia_pre_text(git: &str, parent: &str, msg: &str, cache: &PreCache) -> PreText {
    let blobs = ls_tree(git, parent, GLIA_PACKETS);
    let maps: Vec<_> = GLIA_PACKETS.iter().filter_map(|p| blobs.get(*p)).map(|b| cache.get(git, b)).collect();
    let part = id_text(msg, |s| !dot_ids(s).is_empty());
    let mut seen = BTreeSet::new();
    let (mut text, mut found) = (String::new(), 0);
    for (_, id) in dot_ids(part) {
        if !seen.insert(id.clone()) {
            continue;
        }
        if let Some(t) = maps.iter().find_map(|m| m.get(&id)) {
            found += 1;
            text.push_str(t);
            text.push('\n');
        }
    }
    ((!text.trim().is_empty()).then_some(text), seen.len(), found)
}

/// Kina: the definition rows, in `docs`, of the ids `text` names, and the units (spec dir or doc) defining
/// them. A row is the line whose head defines the id (as in the vertical run). An id defined in several
/// units is narrowed by a spec number on its own message line, then to the rows still open (`- [ ]`, the
/// task about to be done), then to the newest spec (highest number) when only spec units remain.
fn kina_rows(docs: &[(String, String)], text: &str, stats: &mut BTreeMap<String, f64>) -> (Option<String>, BTreeSet<String>) {
    let bare: BTreeSet<u8> = bare_kinds();
    let mut defs: HashMap<String, Vec<(String, String, bool)>> = HashMap::new();
    let mut vocab_h: BTreeSet<String> = BTreeSet::new();
    let mut kinds: BTreeSet<u8> = BTreeSet::new();
    for (path, line) in docs {
        let head = line_head(line);
        let mut defined: Vec<String> = Vec::new();
        if let Some((p, id)) = hyphen_ids(head).into_iter().next()
            && head.starts_with(&id)
        {
            if !any_eq(&prv().id_standards, &p) {
                vocab_h.insert(p);
                defined.push(id);
            }
        } else if let Some((k, id)) = bare_ids(head, &bare).into_iter().next()
            && head.starts_with(&id)
        {
            kinds.insert(k);
            defined.push(id);
        }
        if defined.is_empty() {
            continue;
        }
        let t = line.trim_start();
        let open = t.starts_with("- [ ]") || t.starts_with("* [ ]");
        let unit = kina_unit(path);
        for id in defined {
            defs.entry(id).or_default().push((unit.clone(), line.clone(), open));
        }
    }
    let unit_spec = |u: &str| -> Option<u32> {
        let r = u.strip_prefix("specs/")?;
        let d: String = r.chars().take_while(char::is_ascii_digit).collect();
        (d.len() == 3).then(|| d.parse().ok()).flatten()
    };
    let mut rows: BTreeSet<String> = BTreeSet::new();
    let mut units: BTreeSet<String> = BTreeSet::new();
    let mut bump = |k: &str| *stats.entry(format!("pre_{k}")).or_default() += 1.0;
    for l in text.lines() {
        let lt = l.trim_start().to_ascii_lowercase();
        if lt.starts_with("co-authored-by:") || lt.starts_with("claude-session:") || lt.starts_with("signed-off-by:") {
            continue;
        }
        let ls = spec_refs(l);
        let mut found: Vec<String> = hyphen_ids(l).into_iter().filter(|(p, _)| vocab_h.contains(p)).map(|x| x.1).collect();
        found.extend(bare_ids(l, &kinds).into_iter().map(|x| x.1));
        for id in found {
            let Some(cands) = defs.get(&id) else {
                bump("id_undefined");
                continue;
            };
            let mut c: Vec<&(String, String, bool)> = cands.iter().collect();
            let multi = c.iter().map(|x| &x.0).collect::<BTreeSet<_>>().len() > 1;
            if !ls.is_empty() {
                let r: Vec<_> = c.iter().copied().filter(|x| unit_spec(&x.0).is_some_and(|n| ls.contains(&n))).collect();
                if !r.is_empty() {
                    c = r;
                    bump("by_line_spec");
                }
            }
            if c.iter().any(|x| x.2) && c.iter().any(|x| !x.2) {
                c.retain(|x| x.2);
                bump("by_open_row");
            }
            let us: BTreeSet<&String> = c.iter().map(|x| &x.0).collect();
            if us.len() > 1 && us.iter().all(|u| unit_spec(u).is_some()) {
                let newest = us.iter().map(|u| unit_spec(u).unwrap_or(0)).max().unwrap_or(0);
                c.retain(|x| unit_spec(&x.0) == Some(newest));
                bump("by_newest_spec");
            }
            bump(if multi { "id_multi_unit" } else { "id_one_unit" });
            for (u, line, _) in c {
                rows.insert(line.clone());
                units.insert(u.clone());
            }
        }
    }
    let text: String = rows.into_iter().collect::<Vec<_>>().join("\n");
    ((!text.trim().is_empty()).then_some(text), units)
}

/// Per commit, what is known before it: author time, merged branch, milestone ids, wave landings, and
/// (test commits only) the pre-change intent text.
struct R1Hist {
    at: Vec<i64>,
    br: Vec<Option<String>>,
    /// Mid labels: glia, the families of the packets the commit names; Kina, the units of its rows.
    ids: Vec<BTreeSet<String>>,
    /// glia: the packet prefixes the commit names (DT, DA, ...).
    pfx: Vec<BTreeSet<String>>,
    wave_end: Vec<bool>,
    pre: Vec<Option<String>>,
    glia: bool,
    stats: BTreeMap<String, f64>,
}

fn r1_hist(args: &Args, commits: &[Commit], tests: &BTreeSet<usize>, extra_at: Option<&HashMap<String, i64>>) -> R1Hist {
    let t = Instant::now();
    let at_map = git_author_times(&args.git, &args.rev);
    let br_map = branch_labels(&args.git, &args.rev);
    let n = commits.len();
    let at: Vec<i64> =
        commits.iter().map(|c| at_map.get(&c.sha).or_else(|| extra_at.and_then(|m| m.get(&c.sha))).copied().unwrap_or(0)).collect();
    let br: Vec<Option<String>> = commits.iter().map(|c| br_map.get(&c.sha).cloned()).collect();
    let mut ids = vec![BTreeSet::new(); n];
    let mut pfx = vec![BTreeSet::new(); n];
    let mut wave_end = vec![false; n];
    let mut pre: Vec<Option<String>> = vec![None; n];
    let mut stats: BTreeMap<String, f64> = BTreeMap::new();
    stats.insert("commits_on_a_merged_branch".into(), br.iter().filter(|b| b.is_some()).count() as f64);
    stats.insert("merged_branches".into(), br.iter().flatten().collect::<BTreeSet<_>>().len() as f64);
    let parents = git_parents(&args.git, &args.rev);
    match args.pre.as_str() {
        "" => {}
        "glia" => {
            // the packet prefixes defined at the snapshot (A1-A16, L0, LA-LG, C0, CA-CL, D0, DA-DH, DT, DZ)
            let cache = PreCache::default();
            let blobs = ls_tree(&args.git, &args.rev, GLIA_PACKETS);
            let prefixes: BTreeSet<String> = GLIA_PACKETS
                .iter()
                .filter_map(|p| blobs.get(*p))
                .flat_map(|b| cache.get(&args.git, b).keys().map(|id| id.split('.').next().unwrap_or("").to_string()).collect::<Vec<_>>())
                .collect();
            for (k, c) in commits.iter().enumerate() {
                let part = id_text(&c.msg, |s| dot_ids(s).iter().any(|(p, _)| prefixes.contains(p)));
                for (p, id) in dot_ids(part) {
                    if prefixes.contains(&p) {
                        ids[k].insert(family(&id));
                        pfx[k].insert(p);
                    }
                }
                wave_end[k] = wave_landing(subject(&c.msg));
            }
            let got: Vec<(usize, PreText)> = tests
                .par_iter()
                .map(|&k| {
                    let p = parents.get(&commits[k].sha).and_then(|x| x.0.clone());
                    (k, p.map_or((None, 0, 0), |p| glia_pre_text(&args.git, &p, &commits[k].msg, &cache)))
                })
                .collect();
            for (k, (text, named, found)) in got {
                *stats.entry("pre_ids_named".into()).or_default() += named as f64;
                *stats.entry("pre_ids_found_at_parent".into()).or_default() += found as f64;
                pre[k] = text;
            }
            stats.insert("wave_landings".into(), wave_end.iter().filter(|&&w| w).count() as f64);
        }
        "kina" => {
            assert!(!args.wiki.is_empty(), "--pre kina needs --wiki");
            let wtext = git_text(&args.wiki, &["log", "--format=%H%x1f%ct", "HEAD"]).expect("wiki log");
            let mut wiki_revs: Vec<(i64, String)> = wtext
                .lines()
                .filter_map(|l| l.split_once('\x1f').map(|(s, ct)| (ct.trim().parse().unwrap_or(0), s.to_string())))
                .collect();
            wiki_revs.sort();
            let wcache: std::sync::Mutex<HashMap<String, DocLines>> = std::sync::Mutex::new(HashMap::new());
            let maybe = |m: &str| !hyphen_ids(m).is_empty() || !bare_ids(m, &bare_kinds()).is_empty();
            // (commit, pre-change text, its units, per-commit stats).
            type KinaPre = (usize, Option<String>, BTreeSet<String>, BTreeMap<String, f64>);
            let got: Vec<KinaPre> = (0..n)
                .into_par_iter()
                .filter(|&k| maybe(&commits[k].msg))
                .map(|k| {
                    let c = &commits[k];
                    let mut st = BTreeMap::new();
                    let Some(p) = parents.get(&c.sha).and_then(|x| x.0.clone()) else { return (k, None, BTreeSet::new(), st) };
                    let pct = parents.get(&p).map_or(0, |x| x.1);
                    let mut docs = kina_doc_lines(&args.git, &p);
                    if let Some((_, w)) = wiki_revs.iter().rev().find(|(ct, _)| *ct <= pct) {
                        let cached = wcache.lock().unwrap().get(w).cloned();
                        let lines = cached.unwrap_or_else(|| {
                            let l = std::sync::Arc::new(git_grep(&args.wiki, w, None, &["*.md"]));
                            wcache.lock().unwrap().insert(w.clone(), l.clone());
                            l
                        });
                        docs.extend(lines.iter().cloned());
                    }
                    let part = id_text(&c.msg, maybe);
                    let (text, units) = kina_rows(&docs, part, &mut st);
                    (k, text, units, st)
                })
                .collect();
            for (k, text, units, st) in got {
                ids[k] = units;
                if tests.contains(&k) {
                    pre[k] = text;
                    for (a, b) in st {
                        *stats.entry(a).or_default() += b;
                    }
                }
            }
        }
        other => panic!("--pre {other}: expected glia or kina"),
    }
    stats.insert("commits_with_milestone_ids".into(), ids.iter().filter(|s| !s.is_empty()).count() as f64);
    stats.insert("test_commits_with_pre_text".into(), tests.iter().filter(|&&k| pre[k].is_some()).count() as f64);
    eprintln!("[round1] history labels and pre-change text in {:.1}s: {stats:?}", t.elapsed().as_secs_f64());
    R1Hist { at, br, ids, pfx, wave_end, pre, glia: args.pre == "glia", stats }
}

/// Sparse file scores: `(file, score)`.
type FileScores = Vec<(u32, f64)>;

/// One test commit's round-1 signals (sparse file scores; the seed is excluded per query).
#[derive(Default)]
struct R1Commit {
    t1s: Vec<(u32, f64)>,
    t1ss: Vec<(u32, f64)>,
    t1pre: Vec<(u32, f64)>,
    t1s_nd: Vec<(u32, f64)>,
    t1pre_nd: Vec<(u32, f64)>,
    pre_cov: bool,
    w: Vec<Vec<(u32, f64)>>,
    wr: Vec<Vec<(u32, f64)>>,
    /// (milestone variant, signal, its control)
    ms: Vec<(&'static str, FileScores, FileScores)>,
    /// which variant M took: Mid, Mbr, Mgap or none
    m_src: &'static str,
    /// the milestone keys M read (for C0m)
    m_keys: Vec<String>,
    /// per seed: recency-decayed co-change (C0r) and co-change within the current milestone (C0m)
    c0r: Vec<(u32, Vec<(u32, f64)>)>,
    c0m: Vec<(u32, Vec<(u32, f64)>)>,
    /// the gate at this commit: mean (W5 - Wr5) and (M - M~) pseudo recall@10 over the last R1_GATE_N commits
    gate_w: f64,
    gate_m: f64,
    /// T1sp: the subject and the pre-change plan text as one query
    t1sp: Vec<(u32, f64)>,
    /// Round 2: the subject's ids routed through the typed links
    r2: Option<R2Idr>,
    /// Round 3: id history, text confidence, ancestor warm-up flag
    r3: Option<R3Commit>,
    /// `--engine on` (EB.9): the engine's intent signals for this test commit
    eng: Option<EngCommit>,
}

/// C0r's decay per training commit: a co-change k training commits ago weighs R1_GAMMA^k.
const R1_GAMMA: f64 = 0.98;
/// The gate (post-hoc, parameters fixed a priori): W5 / M enter a blend only when, over the last
/// R1_GATE_N training commits replayed as pseudo-queries (each against the state before it), their
/// recall@10 beat their own control's (Wr5 / M~) on average.
const R1_GATE_N: usize = 30;

/// recall@10 of a sparse score (score desc, then file index) over `targets`.
fn r1_pseudo_r10(v: &[(u32, f64)], targets: &[u32]) -> f64 {
    let mut s: Vec<(u32, f64)> = v.iter().copied().filter(|x| x.1 > 0.0).collect();
    s.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let top: BTreeSet<u32> = s.iter().take(10).map(|x| x.0).collect();
    targets.iter().filter(|f| top.contains(f)).count() as f64 / targets.len().max(1) as f64
}

/// The replay's round-1 state: training commits so far, the Mgap segments, label milestones.
#[derive(Default)]
struct R1State {
    /// C0r: co-change and per-file counts, each commit weighted R1_GAMMA^-i (i = its training index), so a
    /// ratio read at any later time equals the decayed one
    co_r: HashMap<u32, HashMap<u32, f64>>,
    cnt_r: HashMap<u32, f64>,
    /// C0m: co-change and counts per milestone key ("seg:N", "id:..", "br:..")
    co_key: HashMap<String, HashMap<u32, HashMap<u32, f64>>>,
    cnt_key: HashMap<String, HashMap<u32, f64>>,
    /// the gate's pseudo-query history: (W5, Wr5, M, M~) recall@10 per recent training commit
    gate: std::collections::VecDeque<[f64; 4]>,
    /// (commit index, files) of every training commit so far, oldest first
    recent: Vec<(usize, Vec<u32>)>,
    seg_pos: Vec<Vec<usize>>,
    seg_files: Vec<BTreeMap<u32, f64>>,
    lab_files: HashMap<String, BTreeMap<u32, f64>>,
    lab_last: HashMap<String, usize>,
    wave: usize,
}

fn r1_window(recent: &[(usize, Vec<u32>)], a: usize, b: usize) -> Vec<(u32, f64)> {
    let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
    for (j, p) in (a..b).rev().enumerate() {
        let w = R1_DECAY.powi(j as i32);
        for &f in &recent[p].1 {
            *acc.entry(f).or_default() += w;
        }
    }
    acc.into_iter().collect()
}

impl R1State {
    fn labels(&self, h: &R1Hist, k: usize) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(b) = &h.br[k] {
            v.push(format!("br:{b}"));
        }
        v.extend(h.ids[k].iter().map(|u| format!("id:{u}")));
        v.extend(h.pfx[k].iter().map(|p| format!("pfx:{p}")));
        if h.glia {
            v.push(format!("wave:{}", self.wave));
        }
        v
    }

    fn train(&mut self, h: &R1Hist, k: usize, c: &Commit) {
        // the gate: this commit as a pseudo-query against the state before it
        {
            let r = self.test(h, k, &c.sha);
            let i5 = R1_KS.iter().position(|&x| x == 5).unwrap();
            let m = &r.ms[0];
            let row = [
                r1_pseudo_r10(&r.w[i5], &c.files),
                r1_pseudo_r10(&r.wr[i5], &c.files),
                r1_pseudo_r10(&m.1, &c.files),
                r1_pseudo_r10(&m.2, &c.files),
            ];
            self.gate.push_back(row);
            if self.gate.len() > R1_GATE_N {
                self.gate.pop_front();
            }
        }
        let pos = self.recent.len();
        let cont = match self.recent.last() {
            None => false,
            Some((prev, _)) => {
                let s = self.seg_pos.len() - 1;
                let last: BTreeSet<u32> =
                    self.seg_pos[s].iter().rev().take(R1_OVERLAP_LAST).flat_map(|&p| self.recent[p].1.iter().copied()).collect();
                h.at[k] - h.at[*prev] <= R1_GAP_S && c.files.iter().any(|f| last.contains(f))
            }
        };
        if !cont {
            self.seg_pos.push(Vec::new());
            self.seg_files.push(BTreeMap::new());
        }
        let s = self.seg_pos.len() - 1;
        self.seg_pos[s].push(pos);
        for &f in &c.files {
            *self.seg_files[s].entry(f).or_default() += 1.0;
        }
        let wgt = R1_GAMMA.powi(-(pos as i32));
        let mut ckeys: Vec<String> = self.labels(h, k).into_iter().filter(|x| x.starts_with("id:") || x.starts_with("br:")).collect();
        ckeys.push(format!("seg:{s}"));
        for &a in &c.files {
            *self.cnt_r.entry(a).or_default() += wgt;
            for key in &ckeys {
                *self.cnt_key.entry(key.clone()).or_default().entry(a).or_default() += 1.0;
            }
            for &b in &c.files {
                if a != b {
                    *self.co_r.entry(a).or_default().entry(b).or_default() += wgt;
                    for key in &ckeys {
                        *self.co_key.entry(key.clone()).or_default().entry(a).or_default().entry(b).or_default() += 1.0;
                    }
                }
            }
        }
        for key in self.labels(h, k) {
            let e = self.lab_files.entry(key.clone()).or_default();
            for &f in &c.files {
                *e.entry(f).or_default() += 1.0;
            }
            self.lab_last.insert(key, pos);
        }
        self.recent.push((k, c.files.clone()));
    }

    /// After every commit (training or not): a landing commit closes its wave.
    fn step(&mut self, h: &R1Hist, k: usize) {
        if h.wave_end[k] {
            self.wave += 1;
        }
    }

    fn lab_sig(&self, keys: &[String], kind: &str, sha: &str) -> (FileScores, FileScores) {
        let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
        let mut hit = false;
        for key in keys.iter().filter(|x| x.starts_with(kind)) {
            if let Some(m) = self.lab_files.get(key) {
                hit = true;
                for (f, v) in m {
                    *acc.entry(*f).or_default() += v;
                }
            }
        }
        if !hit {
            return (Vec::new(), Vec::new());
        }
        let mut others: Vec<(&String, usize)> =
            self.lab_last.iter().filter(|(l, _)| l.starts_with(kind) && !keys.contains(l)).map(|(l, &p)| (l, p)).collect();
        others.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        others.truncate(R1_SHUF_RECENT);
        let sh = if others.is_empty() {
            Vec::new()
        } else {
            let pick = others[(h64(&format!("{sha}:{kind}~")) % others.len() as u64) as usize].0;
            self.lab_files[pick].iter().map(|(&f, &v)| (f, v)).collect()
        };
        (acc.into_iter().collect(), sh)
    }

    /// C0r and C0m for one seed (as C0: c(seed, f) / (c(seed) + 1), the 1 weighted as a commit made now).
    fn seed_sigs(&self, seed: u32, m_keys: &[String]) -> (FileScores, FileScores) {
        let now = R1_GAMMA.powi(-(self.recent.len() as i32));
        let denom = self.cnt_r.get(&seed).copied().unwrap_or(0.0) + now;
        let mut r: Vec<(u32, f64)> = self.co_r.get(&seed).map(|m| m.iter().map(|(&f, &v)| (f, v / denom)).collect()).unwrap_or_default();
        r.sort_unstable_by_key(|x| x.0);
        let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
        for key in m_keys {
            let n = self.cnt_key.get(key).and_then(|m| m.get(&seed)).copied().unwrap_or(0.0) + 1.0;
            if let Some(m) = self.co_key.get(key).and_then(|m| m.get(&seed)) {
                for (&f, &v) in m {
                    *acc.entry(f).or_default() += v / n;
                }
            }
        }
        (r, acc.into_iter().collect())
    }

    fn test(&self, h: &R1Hist, k: usize, sha: &str) -> R1Commit {
        let n = self.recent.len();
        let mut r = R1Commit::default();
        for &kk in &R1_KS {
            r.w.push(r1_window(&self.recent, n.saturating_sub(kk), n));
            r.wr.push(if n >= 2 * kk {
                let span = (n - 2 * kk + 1) as u64;
                let e = kk + (h64(&format!("{sha}:wr{kk}")) % span) as usize;
                r1_window(&self.recent, e - kk, e)
            } else {
                Vec::new()
            });
        }
        // Mgap: the test commit's files are unknown, so it joins the current milestone on time alone.
        let joined = self.recent.last().is_some_and(|(prev, _)| h.at[k] - h.at[*prev] <= R1_GAP_S);
        let (mgap, mgap_sh) = if joined && !self.seg_pos.is_empty() {
            let s = self.seg_pos.len() - 1;
            let others: Vec<usize> = (s.saturating_sub(R1_SHUF_RECENT)..s).collect();
            let sh = if others.is_empty() {
                Vec::new()
            } else {
                let o = others[(h64(&format!("{sha}:mgap~")) % others.len() as u64) as usize];
                self.seg_files[o].iter().map(|(&f, &v)| (f, v)).collect()
            };
            (self.seg_files[s].iter().map(|(&f, &v)| (f, v)).collect(), sh)
        } else {
            (Vec::new(), Vec::new())
        };
        let keys = self.labels(h, k);
        let (mbr, mbr_sh) = self.lab_sig(&keys, "br:", sha);
        let (mid, mid_sh) = self.lab_sig(&keys, "id:", sha);
        let (m, m_sh, src) = if !mid.is_empty() {
            (mid.clone(), mid_sh.clone(), "Mid")
        } else if !mbr.is_empty() {
            (mbr.clone(), mbr_sh.clone(), "Mbr")
        } else if !mgap.is_empty() {
            (mgap.clone(), mgap_sh.clone(), "Mgap")
        } else {
            (Vec::new(), Vec::new(), "none")
        };
        r.m_keys = match src {
            "Mid" => keys.iter().filter(|x| x.starts_with("id:")).cloned().collect(),
            "Mbr" => keys.iter().filter(|x| x.starts_with("br:")).cloned().collect(),
            "Mgap" => vec![format!("seg:{}", self.seg_pos.len() - 1)],
            _ => Vec::new(),
        };
        r.ms.push(("M", m, m_sh));
        r.ms.push(("Mgap", mgap, mgap_sh));
        r.ms.push(("Mbr", mbr, mbr_sh));
        r.ms.push(("Mid", mid, mid_sh));
        if h.glia {
            let (mw, mw_sh) = self.lab_sig(&keys, "wave:", sha);
            let (mp, mp_sh) = self.lab_sig(&keys, "pfx:", sha);
            r.ms.push(("Mwave", mw, mw_sh));
            r.ms.push(("Mpfx", mp, mp_sh));
        }
        r.m_src = src;
        let n = self.gate.len().max(1) as f64;
        r.gate_w = self.gate.iter().map(|x| x[0] - x[1]).sum::<f64>() / n;
        r.gate_m = self.gate.iter().map(|x| x[2] - x[3]).sum::<f64>() / n;
        r
    }
}

fn r1_cov(v: &[f64], seed: usize) -> bool {
    v.iter().enumerate().any(|(f, &s)| f != seed && s > 0.0)
}

/// The round-1 rankers for one query (appended after every earlier ranker, so those are unchanged).
#[allow(clippy::too_many_arguments)]
fn round1_rankers(out: &mut Out, ctx: &Ctx, r: &R1Commit, q: &Query, t: &[u32], p1: &[f64], p1sum: &[f64], c0: &[f64], t1: &[f64], pers: &[(usize, f64)], b0: &[Key]) {
    let g = ctx.g;
    let seed = q.seed as usize;
    let nf = p1.len();
    let d = |v: &[(u32, f64)]| dense(nf, v, seed);
    let mm_p1 = minmax(p1, seed);
    let mm_c0 = minmax(c0, seed);
    let mm_t1 = minmax(t1, seed);
    let (t1s, t1ss, t1pre) = (d(&r.t1s), d(&r.t1ss), d(&r.t1pre));
    let (mm_t1s, mm_t1ss, mm_t1pre) = (minmax(&t1s, seed), minmax(&t1ss, seed), minmax(&t1pre, seed));
    out.push("T1s", &scalar(&t1s), seed, t);
    out.push("T1ss", &scalar(&t1ss), seed, t);
    out.push("T1pre", &scalar(&t1pre), seed, t);
    let (t1s_nd, t1pre_nd) = (d(&r.t1s_nd), d(&r.t1pre_nd));
    out.push("T1s-nd", &scalar(&t1s_nd), seed, t);
    out.push("T1pre-nd", &scalar(&t1pre_nd), seed, t);
    out.r1cov.push(("pre".into(), r.pre_cov));
    for a in [0.25, 0.5] {
        let tag = (a * 100.0) as u32;
        out.push(format!("H9s{tag}"), &scalar(&blend(a, &mm_t1s, &mm_p1)), seed, t);
        out.push(format!("H9ss{tag}"), &scalar(&blend(a, &mm_t1ss, &mm_p1)), seed, t);
        out.push(format!("H9pre{tag}"), &scalar(&blend(a, &mm_t1pre, &mm_p1)), seed, t);
    }
    out.push("H9s50-nd", &scalar(&blend(0.5, &minmax(&t1s_nd, seed), &mm_p1)), seed, t);
    out.push("H9pre50-nd", &scalar(&blend(0.5, &minmax(&t1pre_nd, seed), &mm_p1)), seed, t);
    // the first run's H18 (equal min-max blend of T1, C0 and P1) with the realistic intent
    let eq3 = |x: &[f64]| -> Vec<f64> { (0..nf).map(|f| (x[f] + mm_c0[f] + mm_p1[f]) / 3.0).collect() };
    out.push("H18s", &scalar(&eq3(&mm_t1s)), seed, t);
    out.push("H18pre", &scalar(&eq3(&mm_t1pre)), seed, t);
    // ---- the work neighbourhood ----
    let ws: Vec<Vec<f64>> = r.w.iter().map(|v| d(v)).collect();
    let wrs: Vec<Vec<f64>> = r.wr.iter().map(|v| d(v)).collect();
    let mut mm_w5 = vec![0.0; nf];
    for (i, &k) in R1_KS.iter().enumerate() {
        out.push(format!("W{k}"), &scalar(&ws[i]), seed, t);
        out.push(format!("Wr{k}"), &scalar(&wrs[i]), seed, t);
        out.r1cov.push((format!("W{k}"), r1_cov(&ws[i], seed)));
        let mm_w = minmax(&ws[i], seed);
        let mm_wr = minmax(&wrs[i], seed);
        for a in [0.25, 0.5] {
            let tag = (a * 100.0) as u32;
            out.push(format!("HWa{tag}[W{k}]"), &scalar(&blend(a, &mm_w, &mm_p1)), seed, t);
        }
        out.push(format!("HWa25[Wr{k}]"), &scalar(&blend(0.25, &mm_wr, &mm_p1)), seed, t);
        for (tag, a, b, c) in HT_GRID {
            let s: Vec<f64> = (0..nf).map(|f| a * mm_t1s[f] + b * mm_w[f] + c * mm_p1[f]).collect();
            out.push(format!("HTs{tag}[W{k}]"), &scalar(&s), seed, t);
            let s: Vec<f64> = (0..nf).map(|f| a * mm_t1pre[f] + b * mm_w[f] + c * mm_p1[f]).collect();
            out.push(format!("HTpre{tag}[W{k}]"), &scalar(&s), seed, t);
        }
        out.push(format!("LX[W{k},P1]"), &lex2(&ws[i], p1), seed, t);
        let lx: Vec<Key> = (0..nf).map(|f| [ws[i][f], c0[f], p1[f]]).collect();
        out.push(format!("LX[W{k},C0,P1]"), &lx, seed, t);
        // co-change first (H10's order), the work neighbourhood second
        let lx: Vec<Key> = (0..nf).map(|f| [c0[f], ws[i][f], p1[f]]).collect();
        out.push(format!("LX[C0,W{k},P1]"), &lx, seed, t);
        if k == 5 {
            mm_w5 = mm_w;
        }
    }
    // ---- milestones ----
    let mut mm_m = vec![0.0; nf];
    for (name, sig, ctl) in &r.ms {
        let (v, vc) = (d(sig), d(ctl));
        out.push(*name, &scalar(&v), seed, t);
        out.push(format!("{name}~"), &scalar(&vc), seed, t);
        out.r1cov.push(((*name).to_string(), r1_cov(&v, seed)));
        let mm_v = minmax(&v, seed);
        let mm_vc = minmax(&vc, seed);
        for a in [0.25, 0.5] {
            let tag = (a * 100.0) as u32;
            out.push(format!("HMa{tag}[{name}]"), &scalar(&blend(a, &mm_v, &mm_p1)), seed, t);
        }
        out.push(format!("HMa25[{name}~]"), &scalar(&blend(0.25, &mm_vc, &mm_p1)), seed, t);
        out.push(format!("LX[{name},P1]"), &lex2(&v, p1), seed, t);
        if *name == "M" {
            for (tag, a, b, c) in HT_GRID {
                let s: Vec<f64> = (0..nf).map(|f| a * mm_t1s[f] + b * mm_v[f] + c * mm_p1[f]).collect();
                out.push(format!("HTs{tag}[M]"), &scalar(&s), seed, t);
                let s: Vec<f64> = (0..nf).map(|f| a * mm_t1pre[f] + b * mm_v[f] + c * mm_p1[f]).collect();
                out.push(format!("HTpre{tag}[M]"), &scalar(&s), seed, t);
            }
            let lx: Vec<Key> = (0..nf).map(|f| [v[f], c0[f], p1[f]]).collect();
            out.push("LX[M,C0,P1]", &lx, seed, t);
            let lx: Vec<Key> = (0..nf).map(|f| [c0[f], v[f], p1[f]]).collect();
            out.push("LX[C0,M,P1]", &lx, seed, t);
            mm_m = mm_v;
        }
    }
    // ---- seed-conditional work signals: recency-decayed co-change, co-change inside the milestone ----
    let find = |v: &[(u32, Vec<(u32, f64)>)]| v.iter().find(|x| x.0 == q.seed).map(|x| d(&x.1)).unwrap_or_else(|| vec![0.0; nf]);
    let (c0r, c0m) = (find(&r.c0r), find(&r.c0m));
    out.push("C0r", &scalar(&c0r), seed, t);
    out.push("C0m", &scalar(&c0m), seed, t);
    out.r1cov.push(("C0m".into(), r1_cov(&c0m, seed)));
    out.r1cov.push(("C0".into(), r1_cov(c0, seed)));
    out.push("H10r", &lex2(&c0r, p1), seed, t);
    let lx: Vec<Key> = (0..nf).map(|f| [c0m[f], c0[f], p1[f]]).collect();
    out.push("H10m", &lx, seed, t);
    let mm_c0r = minmax(&c0r, seed);
    out.push("H1a25r", &scalar(&blend(0.25, &mm_p1, &mm_c0r)), seed, t);
    out.push("H18sr", &scalar(&(0..nf).map(|f| (mm_t1s[f] + mm_c0r[f] + mm_p1[f]) / 3.0).collect::<Vec<_>>()), seed, t);
    // ---- the realistic hybrids on other structural bases: P1sum, and the PPR config picked on the tuning repos ----
    let mut bases: Vec<(&str, Vec<f64>)> = vec![("P1sum", p1sum.to_vec())];
    if let Some(i) = ctx.pick {
        let v = ctx.tune[i].1.run(pers);
        let pt = file_pool(g, |u| v[g.node_ppr[u]], true);
        out.push("PT*", &scalar(&pt), seed, t);
        out.hub_share.push(("PT*".into(), hub_share(&pt, &ctx.hubs, seed)));
        out.hub_share.push(("P1sum".into(), hub_share(p1sum, &ctx.hubs, seed)));
        bases.push(("PT*", pt));
    }
    for (bname, b) in &bases {
        let mm_b = minmax(b, seed);
        out.push(format!("H9s50[{bname}]"), &scalar(&blend(0.5, &mm_t1s, &mm_b)), seed, t);
        out.push(format!("H9s25[{bname}]"), &scalar(&blend(0.25, &mm_t1s, &mm_b)), seed, t);
        out.push(format!("H9pre50[{bname}]"), &scalar(&blend(0.5, &mm_t1pre, &mm_b)), seed, t);
        out.push(format!("H18s[{bname}]"), &scalar(&(0..nf).map(|f| (mm_t1s[f] + mm_c0[f] + mm_b[f]) / 3.0).collect::<Vec<_>>()), seed, t);
        out.push(format!("H10[{bname}]"), &lex2(c0, b), seed, t);
        out.push(format!("H10r[{bname}]"), &lex2(&c0r, b), seed, t);
        out.push(format!("H9a50[{bname}]"), &scalar(&blend(0.5, &mm_t1, &mm_b)), seed, t);
    }
    // ---- the gated blends (post-hoc): W5 / M only when they beat their own control on the repo's recent past ----
    let (gw, gm) = (if r.gate_w > 0.0 { 1.0 } else { 0.0 }, if r.gate_m > 0.0 { 1.0 } else { 0.0 });
    out.r1cov.push(("gateW".into(), gw > 0.0));
    out.r1cov.push(("gateM".into(), gm > 0.0));
    let t1sp = d(&r.t1sp);
    let mm_t1sp = minmax(&t1sp, seed);
    out.push("T1sp", &scalar(&t1sp), seed, t);
    out.push("H9sp50", &scalar(&blend(0.5, &mm_t1sp, &mm_p1)), seed, t);
    out.push("HWg", &scalar(&blend(0.25 * gw, &mm_w5, &mm_p1)), seed, t);
    out.push("HMg", &scalar(&blend(0.25 * gm, &mm_m, &mm_p1)), seed, t);
    for (bname, b) in &bases {
        let mm_b = minmax(b, seed);
        let (aw, am) = (0.25 * gw, 0.25 * gm);
        let s: Vec<f64> = (0..nf).map(|f| 0.25 * mm_t1s[f] + aw * mm_w5[f] + am * mm_m[f] + (0.75 - aw - am) * mm_b[f]).collect();
        out.push(format!("HG[{bname}]"), &scalar(&s), seed, t);
        let s: Vec<f64> = (0..nf).map(|f| aw * mm_w5[f] + am * mm_m[f] + (1.0 - aw - am) * mm_b[f]).collect();
        out.push(format!("HG0[{bname}]"), &scalar(&s), seed, t);
        let lx: Vec<Key> = (0..nf).map(|f| [c0[f], aw * mm_w5[f] + am * mm_m[f], b[f]]).collect();
        out.push(format!("H10g[{bname}]"), &lx, seed, t);
        out.push(format!("H9sp25[{bname}]"), &scalar(&blend(0.25, &mm_t1sp, &mm_b)), seed, t);
    }
    // ---- equal-weight blends of the realistic signals (W5 and M fixed a priori) ----
    let eq = |parts: &[&[f64]]| -> Vec<f64> { (0..nf).map(|f| parts.iter().map(|p| p[f]).sum::<f64>() / parts.len() as f64).collect() };
    out.push("ALL4", &scalar(&eq(&[&mm_w5, &mm_m, &mm_c0, &mm_p1])), seed, t);
    out.push("ALL5", &scalar(&eq(&[&mm_t1s, &mm_w5, &mm_m, &mm_c0, &mm_p1])), seed, t);
    out.push("ALL5pre", &scalar(&eq(&[&mm_t1pre, &mm_w5, &mm_m, &mm_c0, &mm_p1])), seed, t);
    out.push("ALL5T1", &scalar(&eq(&[&mm_t1, &mm_w5, &mm_m, &mm_c0, &mm_p1])), seed, t);
    // ---- the PPR tuning grid ----
    for (name, idx) in &ctx.tune {
        let v = idx.run(pers);
        out.push(format!("PT[{name},max]"), &scalar(&file_pool(g, |u| v[g.node_ppr[u]], false)), seed, t);
        out.push(format!("PT[{name},sum]"), &scalar(&file_pool(g, |u| v[g.node_ppr[u]], true)), seed, t);
    }
    // ---- round 2 (appended, so every round-1 ranker above is unchanged) ----
    if let Some(x) = &r.r2 {
        let pt = bases.iter().find(|b| b.0 == "PT*").map(|b| b.1.as_slice());
        round2_rankers(out, x, seed, t, p1, c0, &t1s, pt);
    }
    // ---- round 3 (appended, so every round-2 ranker above is unchanged) ----
    if let Some(x3) = &r.r3 {
        let pt = bases.iter().find(|b| b.0 == "PT*").map_or_else(|| p1sum.to_vec(), |b| b.1.clone());
        round3_rankers(out, r, x3, seed, t, p1, &pt, c0, &t1s, &ws[2], b0);
    }
}

/// Online selection (leak-free, prequential): at each test commit, the candidate with the best mean recall@10
/// over every EARLIER test commit's queries (their changed files are known by then); the first candidate
/// before any. The candidate sets are fixed; the `0` sets leave out the work-neighbourhood / milestone ones.
const R1_SEL: [(&str, &[&str]); 5] = [
    ("SELnt0", &["H10[PT*]", "H10", "PT*"]),
    ("SELnt", &["H10[PT*]", "H10", "PT*", "HWa25[W5]", "HWa25[W10]", "HMa25[M]", "LX[C0,W5,P1]"]),
    ("SELt0", &["H18s[PT*]", "H9s25[PT*]", "H9s50", "H10[PT*]"]),
    (
        "SELt",
        &["H18s[PT*]", "H9s25[PT*]", "H9s50", "H10[PT*]", "HTsa[W5]", "HTsc[W5]", "HTsd[W5]", "HTsc[M]", "HTsd[M]", "ALL5"],
    ),
    // Round 2: SELt plus the id-routed hybrids (role-aware IDR), fixed a priori
    (
        "SELt2",
        &[
            "H18s[PT*]",
            "H9s25[PT*]",
            "H9s50",
            "H10[PT*]",
            "HTsa[W5]",
            "HTsc[W5]",
            "HTsd[W5]",
            "HTsc[M]",
            "HTsd[M]",
            "ALL5",
            "LX[IDR,H18s[PT*]]",
            "LX[IDR,H9s25[PT*]]",
            "H18si[PT*]",
            "LX[IDR,C0,PT*]",
        ],
    ),
];

/// Appends the SEL* rankers to every query's results; returns, per set, how often each candidate was picked.
fn round1_select(results: &mut [Out], queries: &[Query], ntc: usize) -> serde_json::Value {
    let names = results[0].names.clone();
    let pos = |n: &str| names.iter().position(|x| x == n);
    let mut by_tc: Vec<Vec<usize>> = vec![Vec::new(); ntc];
    for (q, x) in queries.iter().enumerate() {
        by_tc[x.tc].push(q);
    }
    let mut info = serde_json::Map::new();
    for (sname, cands) in R1_SEL {
        let ix: Vec<usize> = cands.iter().filter_map(|c| pos(c)).collect();
        if ix.len() != cands.len() {
            continue;
        }
        let mut sum: Vec<f64> = vec![0.0; ix.len()];
        let mut seen = 0usize;
        let mut picks: BTreeMap<String, usize> = BTreeMap::new();
        let mut per_q: Vec<[f64; 5]> = vec![[0.0; 5]; queries.len()];
        let mut last = String::new();
        for qs in &by_tc {
            let best = if seen == 0 {
                0
            } else {
                (0..ix.len()).max_by(|&a, &b| sum[a].total_cmp(&sum[b]).then(b.cmp(&a))).unwrap_or(0)
            };
            last = names[ix[best]].clone();
            *picks.entry(last.clone()).or_default() += 1;
            for &q in qs {
                per_q[q] = results[q].m[ix[best]];
            }
            for &q in qs {
                for (j, &ri) in ix.iter().enumerate() {
                    sum[j] += results[q].m[ri][1];
                }
                seen += 1;
            }
        }
        for (q, r) in results.iter_mut().enumerate() {
            r.names.push(sname.to_string());
            r.m.push(per_q[q]);
        }
        info.insert(sname.into(), json!({"picks_per_test_commit": picks, "pick_at_the_last_test_commit": last}));
    }
    serde_json::Value::Object(info)
}

/// The PPR tuning grid's indexes: weight set x damping (pooling is applied per query).
fn r1_tune_indexes(n: usize, id_to_idx: &HashMap<NodeId, usize>, edges: &[&Edge], edge_pair: &[u32]) -> Vec<(String, Ppr)> {
    let mut out = Vec::new();
    for (wname, over) in R1_WSETS {
        for dmp in R1_DAMP {
            let mut c = ref_config(None);
            c.direction = Direction::Undirected;
            c.top_k = usize::MAX;
            c.damping = dmp;
            c.edge_weights.extend(over.iter().copied());
            out.push((format!("{wname},d{}", (dmp * 100.0).round() as u32), Ppr::build(n, id_to_idx, edges, edge_pair, &c)));
        }
    }
    out
}

/// The round-1 report: facts, metrics (ALL and covered subsets), contrasts with commit-cluster CIs.
fn round1_report(label: &str, names: &[String], results: &[Out], queries: &[Query], r1s: &[R1Commit], hist_stats: &BTreeMap<String, f64>, bi: &BootIn) -> (serde_json::Value, String) {
    let (commit_of, boot, lf) = (bi.commit_of, bi.boot, bi.lf);
    let nq = queries.len();
    let pos = |n: &str| names.iter().position(|x| x == n);
    let col = |ri: usize, mi: usize| -> Vec<f64> { results.iter().map(|r| r.m[ri][mi]).collect() };
    let all: Vec<usize> = (0..nq).collect();
    let mut cov: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (q, r) in results.iter().enumerate() {
        for (k, on) in &r.r1cov {
            let e = cov.entry(k.clone()).or_default();
            if *on {
                e.push(q);
            }
        }
    }
    let mean_on = |ri: usize, idx: &[usize]| -> [f64; 5] {
        let mut m = [0.0; 5];
        for &q in idx {
            for (acc, x) in m.iter_mut().zip(results[q].m[ri]) {
                *acc += x;
            }
        }
        m.map(|x| if idx.is_empty() { f64::NAN } else { x / idx.len() as f64 })
    };
    let fmt5 = |m: [f64; 5]| m.iter().map(|x| format!("{x:.3}")).collect::<Vec<_>>().join(" | ");
    let mut md = String::new();
    let mut js = serde_json::Map::new();
    md.push_str(&format!("### {label}: round 1\n\n"));

    // ---- facts ----
    let ntc = r1s.len();
    let mut src: BTreeMap<&str, usize> = BTreeMap::new();
    for r in r1s {
        *src.entry(r.m_src).or_default() += 1;
    }
    let mut facts: BTreeMap<String, f64> = hist_stats.clone();
    facts.insert("test_commits".into(), ntc as f64);
    facts.insert("test_commits_pre_covered".into(), r1s.iter().filter(|r| r.pre_cov).count() as f64);
    facts.insert("test_commits_gate_w_on".into(), r1s.iter().filter(|r| r.gate_w > 0.0).count() as f64);
    facts.insert("test_commits_gate_m_on".into(), r1s.iter().filter(|r| r.gate_m > 0.0).count() as f64);
    facts.insert("mean_gate_w".into(), r1s.iter().map(|r| r.gate_w).sum::<f64>() / r1s.len().max(1) as f64);
    facts.insert("mean_gate_m".into(), r1s.iter().map(|r| r.gate_m).sum::<f64>() / r1s.len().max(1) as f64);
    for (s, n) in &src {
        facts.insert(format!("test_commits_M_from_{s}"), *n as f64);
    }
    for (k, idx) in &cov {
        facts.insert(format!("queries_covered_{k}"), idx.len() as f64);
    }
    md.push_str("Facts (test commits, queries covered per signal, history labels):\n\n");
    for (k, v) in &facts {
        md.push_str(&format!("- {k}: {v}\n"));
    }
    md.push('\n');
    js.insert("facts".into(), json!(facts));

    // ---- metric tables ----
    let r1_rows: Vec<String> = names
        .iter()
        .skip_while(|n| *n != "T1s")
        .filter(|n| !n.starts_with("PT["))
        .cloned()
        .collect();
    let mut rows: Vec<String> = ["B0", "B1", "C0", "P1", "P1sum", "H10", "T0", "T1", "H9a25[P1]", "H9a50[P1]", "H18"]
        .iter()
        .filter(|n| pos(n).is_some())
        .map(|s| s.to_string())
        .collect();
    rows.extend(r1_rows.iter().cloned());
    let mut mj = serde_json::Map::new();
    let mut subsets: Vec<(String, Vec<usize>)> = vec![("all".into(), all.clone())];
    for k in ["pre", "M", "Mid", "Mbr", "Mgap", "W1", "W5", "C0m", "idr_id", "idr"] {
        if let Some(idx) = cov.get(k)
            && !idx.is_empty()
        {
            subsets.push((k.to_string(), idx.clone()));
        }
    }
    // cold starts: the seed has no earlier co-change
    if let Some(warm) = cov.get("C0") {
        let warm: HashSet<usize> = warm.iter().copied().collect();
        let cold: Vec<usize> = (0..nq).filter(|q| !warm.contains(q)).collect();
        if !cold.is_empty() {
            cov.insert("cold".into(), cold.clone());
            subsets.push(("cold".into(), cold));
        }
    }
    for (sname, idx) in &subsets {
        md.push_str(&format!(
            "{} ({} queries):\n\n| ranker | recall@5 | recall@10 | recall@20 | MRR | nDCG@20 |\n|---|---|---|---|---|---|\n",
            if sname == "all" { "ALL queries".to_string() } else { format!("COVERED by {sname}") },
            idx.len()
        ));
        let mut sj = serde_json::Map::new();
        let show: Vec<&String> = if sname == "all" {
            rows.iter().collect()
        } else {
            rows.iter().filter(|r| (!r.starts_with("HTs") && !r.starts_with("HTpre")) || r.ends_with("[M]") || r.ends_with("[W5]")).collect()
        };
        for r in show {
            let m = mean_on(pos(r).unwrap(), idx);
            md.push_str(&format!("| {r} | {} |\n", fmt5(m)));
            sj.insert(r.clone(), json!({"r5": m[0], "r10": m[1], "r20": m[2], "mrr": m[3], "ndcg20": m[4]}));
        }
        // every ranker's mean on this subset goes to the JSON (the PT grid included)
        for (ri, n) in names.iter().enumerate() {
            if !sj.contains_key(n) {
                let m = mean_on(ri, idx);
                sj.insert(n.clone(), json!({"r5": m[0], "r10": m[1], "r20": m[2], "mrr": m[3], "ndcg20": m[4]}));
            }
        }
        md.push('\n');
        mj.insert(sname.clone(), serde_json::Value::Object(sj));
    }
    js.insert("metrics".into(), serde_json::Value::Object(mj));
    js.insert("subset_sizes".into(), json!(subsets.iter().map(|(s, i)| (s.clone(), i.len())).collect::<BTreeMap<_, _>>()));

    // ---- contrasts ----
    let mut cs: Vec<(String, String, &str)> = Vec::new();
    let mut add = |a: &str, b: &str, s: &'static str| cs.push((a.to_string(), b.to_string(), s));
    // realistic intent vs the optimistic T1
    for (a, b) in [
        ("T1", "P1"),
        ("T1s", "P1"),
        ("T1ss", "P1"),
        ("T1pre", "P1"),
        ("T1s", "T1"),
        ("T1ss", "T1"),
        ("T1pre", "T1"),
        ("T1s", "T0"),
        ("T1pre", "T1s"),
        ("H9a50[P1]", "P1"),
        ("H9s50", "P1"),
        ("H9s25", "P1"),
        ("H9ss50", "P1"),
        ("H9pre50", "P1"),
        ("H9pre25", "P1"),
        ("H9s50", "H9a50[P1]"),
        ("H9pre50", "H9a50[P1]"),
        ("H9pre50", "H9s50"),
        ("T1s-nd", "T1s"),
        ("T1pre-nd", "T1pre"),
        ("H9s50-nd", "H9s50"),
        ("H9pre50-nd", "H9pre50"),
        ("H18", "P1"),
        ("H18s", "P1"),
        ("H18pre", "P1"),
        ("H18s", "H18"),
        ("H18s", "H10"),
    ] {
        add(a, b, "pre");
    }
    // the work neighbourhood and its random-window control
    for k in R1_KS {
        let (w, wr) = (format!("W{k}"), format!("Wr{k}"));
        cs.push((w.clone(), wr.clone(), "all"));
        cs.push((w.clone(), "P1".into(), "all"));
        cs.push((w.clone(), "C0".into(), "all"));
        cs.push((format!("HWa25[W{k}]"), "P1".into(), "all"));
        cs.push((format!("HWa50[W{k}]"), "P1".into(), "all"));
        cs.push((format!("HWa25[W{k}]"), format!("HWa25[Wr{k}]"), "all"));
        cs.push((format!("HWa25[W{k}]"), "H10".into(), "all"));
        cs.push((format!("LX[W{k},C0,P1]"), "H10".into(), "all"));
        cs.push((format!("LX[C0,W{k},P1]"), "H10".into(), "all"));
        cs.push((format!("HTsd[W{k}]"), "H9s50".into(), "all"));
        cs.push((format!("HTsd[W{k}]"), "H9a50[P1]".into(), "all"));
        cs.push((format!("HTpred[W{k}]"), "H9pre50".into(), "all"));
    }
    // milestones and their shuffled controls
    for m in ["M", "Mgap", "Mbr", "Mid", "Mwave", "Mpfx"] {
        let sub: &'static str = match m {
            "M" => "M",
            "Mgap" => "Mgap",
            "Mbr" => "Mbr",
            "Mid" => "Mid",
            _ => "all",
        };
        cs.push((m.into(), format!("{m}~"), sub));
        cs.push((m.into(), "P1".into(), sub));
        cs.push((format!("HMa25[{m}]"), "P1".into(), sub));
        cs.push((format!("HMa50[{m}]"), "P1".into(), sub));
        cs.push((format!("HMa25[{m}]"), format!("HMa25[{m}~]"), sub));
        cs.push((format!("HMa25[{m}]"), "H10".into(), sub));
        cs.push((format!("LX[{m},P1]"), "P1".into(), sub));
    }
    for (a, b, s) in [
        ("SELnt0", "H10[PT*]", "all"),
        ("SELnt0", "P1", "all"),
        ("SELnt0", "H10", "all"),
        ("SELnt", "SELnt0", "all"),
        ("SELnt", "H10[PT*]", "all"),
        ("SELnt", "P1", "all"),
        ("SELt0", "H18s[PT*]", "all"),
        ("SELt0", "P1", "all"),
        ("SELt", "SELt0", "all"),
        ("SELt", "H18s[PT*]", "all"),
        ("SELt", "P1", "all"),
        ("SELt", "H9a50[P1]", "all"),
        ("SELt0", "H9a50[P1]", "all"),
        ("H9s25[PT*]", "H9a50[P1]", "all"),
        ("H18s[PT*]", "H9a50[P1]", "all"),
        ("H18s[PT*]", "B0", "all"),
        ("SELt", "B0", "all"),
        ("SELnt", "B0", "all"),
        ("HWg", "P1", "all"),
        ("HMg", "P1", "all"),
        ("HG[PT*]", "H9s25[PT*]", "all"),
        ("HG[PT*]", "P1", "all"),
        ("HG0[PT*]", "PT*", "all"),
        ("H10g[PT*]", "H10[PT*]", "all"),
        ("T1sp", "T1s", "pre"),
        ("H9sp50", "H9s50", "pre"),
        ("H9sp25[PT*]", "H9s25[PT*]", "pre"),
        ("H10", "P1", "cold"),
        ("H9s50", "P1", "cold"),
        ("HWa25[W5]", "P1", "cold"),
        ("HMa25[M]", "P1", "cold"),
        ("PT*", "P1", "cold"),
        ("C0r", "C0", "C0"),
        ("C0m", "C0", "C0m"),
        ("C0r", "C0", "all"),
        ("H10r", "H10", "all"),
        ("H10m", "H10", "all"),
        ("H10m", "H10", "C0m"),
        ("H1a25r", "H1a25", "all"),
        ("H18sr", "H18s", "all"),
        ("H9s50[P1sum]", "H9s50", "all"),
        ("H9s50[PT*]", "H9s50", "all"),
        ("H9s50[PT*]", "P1", "all"),
        ("H9s50[PT*]", "H9a50[P1]", "all"),
        ("H18s[PT*]", "P1", "all"),
        ("H10[PT*]", "H10", "all"),
        ("H10r[PT*]", "H10", "all"),
        ("PT*", "P1", "all"),
        ("PT*", "B0", "all"),
        ("PT*", "P1sum", "all"),
        ("M", "W5", "M"),
        ("HMa25[M]", "HWa25[W5]", "M"),
        ("LX[M,C0,P1]", "H10", "M"),
        ("LX[C0,M,P1]", "H10", "M"),
        ("HTsd[M]", "H9s50", "all"),
        ("HTsd[M]", "H9a50[P1]", "all"),
        ("HTpred[M]", "H9pre50", "pre"),
        ("HTpred[M]", "P1", "pre"),
        ("HTpred[M]", "H9a50[P1]", "pre"),
        ("ALL4", "P1", "all"),
        ("ALL4", "H10", "all"),
        ("ALL5", "P1", "all"),
        ("ALL5", "H9s50", "all"),
        ("ALL5", "H9a50[P1]", "all"),
        ("ALL5pre", "P1", "pre"),
        ("ALL5pre", "H9a50[P1]", "pre"),
        ("ALL5T1", "H9a50[P1]", "all"),
        ("ALL5", "ALL5T1", "all"),
    ] {
        cs.push((a.into(), b.into(), s));
    }
    // round 2: the subject's ids as typed links
    for (a, b) in [
        ("IDR", "P1"),
        ("IDR", "T1s"),
        ("LX[IDR,P1]", "P1"),
        ("HI50[P1]", "P1"),
        ("HI25[P1]", "P1"),
        ("LX[IDR,H9s50]", "H9s50"),
        ("LX[IDR,H9s50]", "H9a50[P1]"),
        ("LX[IDR,H9s50]", "P1"),
        ("H9si", "H9s50"),
        ("H9si", "H9a50[P1]"),
        ("H9si", "P1"),
        ("LX[IDR,C0,P1]", "H10"),
        ("LX[IDR,C0,P1]", "P1"),
        ("HCi", "P1"),
        ("HCi", "H1a50"),
        ("IDRt", "IDR"),
        ("IDRt", "LX[IDR,P1]"),
        ("LX[IDRt,H9s50]", "LX[IDR,H9s50]"),
        ("IDRh", "IDR"),
        ("LX[IDRh,H9s50]", "LX[IDR,H9s50]"),
        ("IDR~roles", "IDR"),
        ("LX[IDR~roles,H9s50]", "LX[IDR,H9s50]"),
        ("H9si~roles", "H9si"),
        ("H9sih", "H9si"),
        ("IDRn", "IDRc"),
        ("LX[IDR,H18s[PT*]]", "H18s[PT*]"),
        ("LX[IDR,H18s[PT*]]", "H9a50[P1]"),
        ("LX[IDR,H18s[PT*]]", "P1"),
        ("LX[IDR,H9s25[PT*]]", "H9s25[PT*]"),
        ("H18si[PT*]", "H18s[PT*]"),
        ("H18si[PT*]", "H9a50[P1]"),
        ("LX[IDR,C0,PT*]", "H10[PT*]"),
        ("LX[IDRh,H18s[PT*]]", "LX[IDR,H18s[PT*]]"),
        ("LX[IDR~roles,H18s[PT*]]", "LX[IDR,H18s[PT*]]"),
        ("H9a50[P1]", "P1"),
        ("H9s50", "P1"),
        ("H18s[PT*]", "P1"),
    ] {
        cs.push((a.into(), b.into(), "idr_id"));
    }
    for (a, b) in [("SELt2", "SELt"), ("SELt2", "P1"), ("SELt2", "H9a50[P1]"), ("SELt2", "H9s50"), ("SELt2", "B0"), ("SELt2", "H18s[PT*]")] {
        cs.push((a.into(), b.into(), "all"));
    }
    md.push_str("Contrasts (a minus b; recall@10 and MRR; mean, 95% commit-cluster bootstrap CI; on ALL queries and on the named covered subset):\n\n| a | b | subset | n | recall@10 ALL | recall@10 COVERED | MRR ALL | MRR COVERED |\n|---|---|---|---|---|---|---|---|\n");
    let mut cj = Vec::new();
    for (a, b, s) in &cs {
        let (Some(ia), Some(ib)) = (pos(a), pos(b)) else { continue };
        let sub: &[usize] = if *s == "all" { &all } else { cov.get(*s).map_or(&[][..], |v| &v[..]) };
        let mut cells = Vec::new();
        for mi in [1usize, 3] {
            for (idx, sname) in [(&all[..], "all"), (sub, *s)] {
                if sname == "all" && idx.len() == sub.len() && *s == "all" && cells.len() % 2 == 1 {
                    cells.push("=".into());
                    continue;
                }
                match paired_sub(&col(ia, mi), &col(ib, mi), idx, commit_of, boot, lf) {
                    Some(p) => {
                        cells.push(format!("{:+.3} [{:+.3}, {:+.3}]", p.mean, p.lo, p.hi));
                        cj.push(json!({"a": a, "b": b, "metric": if mi == 1 { "r10" } else { "mrr" }, "subset": sname, "n": idx.len(),
                            "diff": p.mean, "lo": p.lo, "hi": p.hi, "p_sign": p.p, "wins": p.wins, "losses": p.losses}));
                    }
                    None => cells.push("-".into()),
                }
            }
        }
        md.push_str(&format!("| {a} | {b} | {s} | {} | {} |\n", sub.len(), cells.join(" | ")));
    }
    js.insert("contrasts".into(), json!(cj));

    // ---- the PPR tuning grid ----
    let pt: Vec<&String> = names.iter().filter(|n| n.starts_with("PT[")).collect();
    if !pt.is_empty() {
        md.push_str("\nPPR tuning grid (ALL queries; each config minus P1 and minus B0, recall@10 and MRR, 95% commit-cluster bootstrap CI):\n\n| config | recall@10 | MRR | r10 - P1 | r10 - B0 | MRR - P1 | MRR - B0 |\n|---|---|---|---|---|---|---|\n");
        let mut gj = Vec::new();
        for n in pt {
            let ri = pos(n).unwrap();
            let m = mean_on(ri, &all);
            let mut cells = Vec::new();
            let mut row = json!({"config": n, "r10": m[1], "mrr": m[3]});
            for (mi, mname) in [(1usize, "r10"), (3, "mrr")] {
                for bl in ["P1", "B0"] {
                    let Some(bi) = pos(bl) else { continue };
                    let p = paired(&col(ri, mi), &col(bi, mi), commit_of, r1s.len(), boot, lf);
                    cells.push(format!("{:+.3} [{:+.3}, {:+.3}]", p.mean, p.lo, p.hi));
                    row[format!("{mname}_vs_{bl}")] = json!({"diff": p.mean, "lo": p.lo, "hi": p.hi});
                }
            }
            md.push_str(&format!("| {n} | {:.3} | {:.3} | {} | {} | {} | {} |\n", m[1], m[3], cells[0], cells[1], cells[2], cells[3]));
            gj.push(row);
        }
        js.insert("tune".into(), json!(gj));
    }
    (serde_json::Value::Object(js), md)
}

// ============================================================================
// Round 2 (2026-10-18), C: a second law event, a second instrument as made -> its first
// consolidated version (both file stems in the private config), traced through the resolved goal
// layer (the keyed chains, the private config's `key_trules` prefix) to the code
// (`--law-impact kina --law-event tr --keys on`). Truths as the first event's (the first event's
// implementing commits; no commit implements this amendment itself, see the report).
// ============================================================================

/// Page furniture of the second instrument's PDFs: the version line, the drafter's code, the
/// consolidation line, the running title (with its page number) and the contents' dot leaders.
fn tr_furniture(t: &str) -> bool {
    let p = prv();
    let tn = t.replace(['\u{2011}', '\u{2010}'], "-");
    let t = tn.as_str();
    if p.furn_trules_contains.iter().any(|x| t.contains(x.as_str())) || p.furn_prefix.iter().any(|x| t.starts_with(x.as_str())) || t.contains(".....") {
        return true;
    }
    if t.starts_with(p.furn_drafter_prefix.as_str()) && t.len() <= 16 && t.contains(" - ") {
        return true;
    }
    let ws: Vec<&str> = t.split_whitespace().collect();
    let title: Vec<&str> = p.trules_title.split_whitespace().collect();
    let page = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_digit() || matches!(c, 'i' | 'v' | 'x'));
    if ws == title
        || (ws.len() == title.len() + 1 && ((page(ws[ws.len() - 1]) && ws[..ws.len() - 1] == title[..]) || (page(ws[0]) && ws[1..] == title[..])))
    {
        return true;
    }
    // running headers anywhere on the page (a wrapped Part title ends the top-of-page run), bare
    // page numbers, and the one- or two-word lower-case tail of a wrapped header
    law_running_header(t)
        || (ws.len() >= 3 && matches!(ws[ws.len() - 1], "Part" | "Division") && !t.contains('\u{2014}'))
        || (!ws.is_empty() && ws.iter().all(|x| x.chars().all(|c| c.is_ascii_digit())))
        || (!ws.is_empty() && ws.len() <= 2 && ws.iter().all(|x| x.chars().all(|c| c.is_ascii_lowercase())) && !t.ends_with('.') && !t.ends_with(';'))
}

fn law_event_tr(args: &Args) {
    assert_eq!(args.law_impact, "kina", "--law-impact: only `kina` is wired");
    for (v, n) in [(&args.wiki, "--wiki"), (&args.law_sources, "--law-sources"), (&args.law_cache, "--law-cache"), (&args.law_out, "--law-out"), (&args.law_tips, "--law-tips"), (&args.keys_out, "--keys-out")] {
        assert!(!v.is_empty(), "--law-event tr needs {n}");
    }
    std::fs::create_dir_all(&args.law_out).expect("law out dir");
    let t0 = Instant::now();
    // ---- the diff ----
    let pv = prv();
    let (old, dups_old) = act_sections(&law_lines(&law_pdf(args, &pv.pdf_trules_made), tr_furniture));
    let (new, dups_new) = act_sections(&law_lines(&law_pdf(args, &pv.pdf_trules), tr_furniture));
    let (n_old, n_new) = (old.len(), new.len());
    let made: Vec<String> = old.iter().map(|s| s.num.clone()).collect();
    if let Ok(want) = std::env::var("GLIA_TR_DEBUG") {
        for (tag, v) in [("as made", &old), ("«tr_new_sp»", &new)] {
            if let Some(x) = v.iter().find(|x| x.num == want) {
                keprintln!("[tr-debug] {tag} {want}: {}", x.text);
            }
        }
    }
    let diff = law_diff(old, new, dups_old, dups_new);
    let changed: BTreeSet<String> = diff.order.iter().filter(|n| diff.status[*n] != "unchanged").cloned().collect();
    let count = |st: &str| diff.status.values().filter(|s| **s == st).count();
    keprintln!(
        "[law-tr] sections: as made {n_old}, «tr_new» {n_new}; unchanged {}, changed {}, added {}, repealed {}; dup numbers {} / {}",
        count("unchanged"),
        count("changed"),
        count("added"),
        count("repealed"),
        diff.dups_old.len(),
        diff.dups_new.len()
    );
    let mut tsv = String::from("section\tstatus\tdice\ttitle_as_made\ttitle_«tr_new_key»\tcontext\n");
    for n in &diff.order {
        let (o, w) = (diff.old.get(n), diff.new.get(n));
        tsv.push_str(&format!(
            "{n}\t{}\t{}\t{}\t{}\t{}\n",
            diff.status[n],
            diff.dice.get(n).map_or("-".into(), |x| format!("{x:.4}")),
            o.map_or("", |s| s.title.as_str()),
            w.map_or("", |s| s.title.as_str()),
            w.or(o).map_or("", |s| s.ctx.as_str())
        ));
    }
    kwrite(format!("{}/law_sections_tr.tsv", args.law_out), &tsv).expect("write tr sections");
    // ---- the code graph and the goal layer at the start ----
    let start = git_text(&args.git, &["rev-parse", &args.rev]).expect("rev-parse start").trim().to_string();
    let res = generate_one(&args.repo).expect("build");
    let merged = &res.merged;
    let all_ids = merged.node_ids();
    let id_to_idx: HashMap<NodeId, usize> = all_ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let edges: Vec<&Edge> = merged.all_edges().collect();
    let g = index_graph(merged, &all_ids, &id_to_idx, &edges);
    let nf = g.paths.len();
    let pidx: HashMap<&str, u32> = g.paths.iter().enumerate().map(|(i, p)| (p.as_str(), i as u32)).collect();
    let mut cfg = ref_config(None);
    cfg.direction = Direction::Undirected;
    cfg.top_k = usize::MAX;
    let ppr = Ppr::build(all_ids.len(), &id_to_idx, &edges, &vec![NONE; edges.len()], &cfg);
    let start_ct: i64 = git_text(&args.git, &["log", "-1", "--format=%ct", &start]).expect("start ct").trim().parse().expect("ct");
    let wlog = git_text(&args.wiki, &["log", "--format=%H%x1f%ct", "HEAD"]).expect("wiki log");
    let wrev = wlog
        .lines()
        .filter_map(|l| l.split_once('\x1f'))
        .filter_map(|(s, ct)| ct.trim().parse::<i64>().ok().map(|c| (c, s.to_string())))
        .filter(|(c, _)| *c <= start_ct)
        .max()
        .map(|x| x.1)
        .expect("a wiki commit at or before the start");
    let mut docs = kina_doc_lines(&args.git, &start);
    docs.extend(git_grep(&args.wiki, &wrev, None, &["*.md"]));
    let pr = PathRes::new(&g.paths);
    let d = build_docset(&docs, &pr);
    let mut cstats = BTreeMap::new();
    let code = group_code(git_grep(&args.git, &start, Some(&prv().code_pat), &[]), &pidx);
    let (toks, _, _) = kina_cites(&d, &code, nf, &mut cstats);
    let files_of = law_files_of(&toks);
    let compliance: Vec<u32> = (0..nf as u32).filter(|&f| !toks[f as usize].is_empty()).collect();
    let mut comp_score = vec![0.0; nf];
    for &f in &compliance {
        for t in &toks[f as usize] {
            comp_score[f as usize] += (1.0 + nf as f64 / files_of[t].len() as f64).ln();
        }
    }
    let ppr_comp = law_ppr(&g, &ppr, &compliance);
    let mut bm = Bm25::default();
    for p in &g.paths {
        let txt = std::fs::read(format!("{}/{p}", args.repo)).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        bm.add(law_code_tokens(&txt), Vec::new());
    }
    let sec_text = |n: &str| -> String { diff.new.get(n).or_else(|| diff.old.get(n)).map_or(String::new(), |s| s.text.clone()) };
    let q_all: Vec<String> = changed.iter().flat_map(|n| law_code_tokens(&sec_text(n))).collect();
    let bm_all = bm.scores(&q_all);
    // ---- truth (as the first event's) ----
    let tasks = law_tasks(&args.wiki);
    let tip_names: Vec<String> = args.law_tips.split(',').map(|t| t.trim().to_string()).collect();
    let tips: Vec<String> = tip_names.iter().map(|t| git_text(&args.git, &["rev-parse", t]).expect("rev-parse tip").trim().to_string()).collect();
    let branch_spec = law_branch_specs(&args.git, &start, &tip_names, &tasks);
    let ticks = law_ticks(&args.wiki, start_ct);
    let commits = law_commits(&args.git, &start, &tips, &pidx, &tasks, &branch_spec, &ticks);
    let tspec = pv.target_spec.as_str();
    let pick = |f: &dyn Fn(&LCommit) -> bool| -> LTruth {
        let cs: Vec<&LCommit> = commits.iter().filter(|c| f(c)).collect();
        (cs.iter().flat_map(|c| c.code.iter().copied()).collect(), cs.len(), cs.iter().flat_map(|c| c.code_new.iter().cloned()).collect())
    };
    let truths: Vec<(&str, &str, LTruth)> = vec![
        (pv.truth_spec.as_str(), t("tr_truth_spec_desc"), pick(&|c: &LCommit| c.spec == tspec)),
        (pv.truth_regime.as_str(), t("tr_truth_regime_desc"), pick(&|c: &LCommit| c.spec == tspec && c.regime)),
        ("ALL", "PROXY: every commit after the start", pick(&|_: &LCommit| true)),
    ];
    keprintln!("[law-tr] goal layer at wiki {wrev}; {} «impl» commits; built in {:.1}s", commits.len(), t0.elapsed().as_secs_f64());
    keys_law(&KLawIn {
        args,
        g: &g,
        ppr: &ppr,
        docs: &docs,
        start: &start,
        truths: &truths,
        commits: &commits,
        tspec,
        changed: &changed,
        status: &diff.status,
        bm_all: &bm_all,
        ppr_comp: &ppr_comp,
        comp_score: &comp_score,
        raw_sets: Vec::new(),
        raw_cited: (0, 0),
        prefix: pv.key_trules.as_str(),
        inst: t("trules_full"),
        extra_secs: made,
    });
    eprintln!("[law-tr] done in {:.1}s -> {}/keys_law_tables.md", t0.elapsed().as_secs_f64(), args.keys_out);
}

// ============================================================================
// Round 2 (2026-10-18): the subject's ids as typed links (IDR)
// ============================================================================
//
// `--round2 on` (with `--round1 on`): each test commit's SUBJECT ids are resolved, at the
// parent revision, with the keyed resolvers (scoped id + lineage) to the units that define
// them; the files those units name (a "names-file" edge, pending / done by its checkbox) and
// the code files citing the id are the IDR signal. glia: packet ids -> files_touched (pending
// = no LANDED wave yet) and code citing the packet. Other repos: a conventional-commit scope
// that names a doc directory (`feat(billing): ..` -> `features/billing/*.md`) -> the files those
// docs name. The rankers are appended after round 1's, so every earlier ranker is unchanged.

/// IDR weights, fixed a priori: a file a defining unit names (pending or no checkbox), one a done
/// unit names, a code file citing the id, and the one-hop factor.
const R2_W_NAMED: f64 = 1.0;
const R2_W_DONE: f64 = 0.5;
const R2_W_CITED: f64 = 0.5;
const R2_W_HOP: f64 = 0.5;
/// How many earlier test commits' resolved scopes the "recent-scope" rule reads.
const R2_RECENT: usize = 5;

/// One test commit's id routing (sparse file scores; the seed is excluded per query).
#[derive(Default, Clone)]
struct R2Idr {
    /// the subject names an id of the repo's vocabulary (glia: a defined packet prefix; other
    /// repos: a scope naming a doc directory)
    has_id: bool,
    /// IDR = named + cited, summed over the subject's resolved ids
    idr: Vec<(u32, f64)>,
    named: Vec<(u32, f64)>,
    cited: Vec<(u32, f64)>,
    /// IDR plus one hop (the ids the defining unit mentions; glia: the packets its prose names)
    hop: Vec<(u32, f64)>,
    /// the control without roles: files named by ANY unit mentioning the id, no state
    noroles: Vec<(u32, f64)>,
}

/// What one resolved key reaches.
#[derive(Default, Clone)]
struct R2Reach {
    named: BTreeMap<u32, f64>,
    cited: BTreeMap<u32, f64>,
    hop: BTreeMap<u32, f64>,
    noroles: BTreeMap<u32, f64>,
}

fn r2_max(m: &mut BTreeMap<u32, f64>, f: u32, w: f64) {
    let e = m.entry(f).or_default();
    *e = e.max(w);
}

fn r2_finish(reach: &[&R2Reach], has_id: bool) -> R2Idr {
    let mut acc: [BTreeMap<u32, f64>; 5] = Default::default();
    for r in reach {
        for (f, w) in &r.named {
            *acc[0].entry(*f).or_default() += w;
            *acc[1].entry(*f).or_default() += w;
            *acc[3].entry(*f).or_default() += w;
        }
        for (f, w) in &r.cited {
            *acc[0].entry(*f).or_default() += w;
            *acc[2].entry(*f).or_default() += w;
            *acc[3].entry(*f).or_default() += w;
        }
        for (f, w) in &r.hop {
            *acc[3].entry(*f).or_default() += w;
        }
        for (f, w) in &r.noroles {
            *acc[4].entry(*f).or_default() += w;
        }
    }
    let v = |m: &BTreeMap<u32, f64>| m.iter().map(|(&f, &w)| (f, w)).collect::<Vec<_>>();
    R2Idr { has_id, idr: v(&acc[0]), named: v(&acc[1]), cited: v(&acc[2]), hop: v(&acc[3]), noroles: v(&acc[4]) }
}

/// A doc block's checkbox state: Some(true) pending (`- [ ]`), Some(false) done (`- [x]`), None none.
fn kblock_state(b: &KBlock) -> Option<bool> {
    let first = b.text.lines().next().unwrap_or("").trim_start();
    let body = first.strip_prefix("- ").or_else(|| first.strip_prefix("* "))?.trim_start();
    if body.starts_with("[ ]") {
        Some(true)
    } else if body.starts_with("[x]") || body.starts_with("[X]") {
        Some(false)
    } else {
        None
    }
}

/// The Kina key model's lookups IDR reads: lineage classes, blocks defining / mentioning each
/// class, code files citing each class.
struct R2KIdx {
    cl: KLin,
    bks: Vec<KBk>,
    def_b: HashMap<String, Vec<u32>>,
    ref_b: HashMap<String, Vec<u32>>,
    citing: HashMap<String, Vec<u32>>,
}

fn r2_kidx(m: &KinaModel) -> R2KIdx {
    let (cl, bks, code_ids, _, _) = kina_keys(m, KBASE);
    let mut def_b: HashMap<String, Vec<u32>> = HashMap::new();
    let mut ref_b: HashMap<String, Vec<u32>> = HashMap::new();
    for (bi, b) in bks.iter().enumerate() {
        for d in &b.defs {
            def_b.entry(d.1.clone()).or_default().push(bi as u32);
        }
        for r in &b.refs {
            ref_b.entry(r.clone()).or_default().push(bi as u32);
        }
    }
    let mut citing: HashMap<String, Vec<u32>> = HashMap::new();
    for (f, ks) in code_ids.iter().enumerate() {
        for k in ks {
            citing.entry(k.clone()).or_default().push(f as u32);
        }
    }
    for v in def_b.values_mut().chain(ref_b.values_mut()).chain(citing.values_mut()) {
        v.sort_unstable();
        v.dedup();
    }
    R2KIdx { cl, bks, def_b, ref_b, citing }
}

fn r2_kina_reach(m: &KinaModel, ix: &R2KIdx, key: &str) -> R2Reach {
    let classes = ix.cl.exp(key);
    let mut r = R2Reach::default();
    let none: Vec<u32> = Vec::new();
    let w_of = |b: u32| if kblock_state(&m.blocks[b as usize]) == Some(false) { R2_W_DONE } else { R2_W_NAMED };
    for c in &classes {
        for &b in ix.def_b.get(c).unwrap_or(&none) {
            let w = w_of(b);
            for &(f, _, _) in &m.blocks[b as usize].files {
                r2_max(&mut r.named, f, w);
            }
            // one hop: the ids the defining unit mentions (not hubs)
            for k2 in &ix.bks[b as usize].refs {
                if classes.contains(k2) {
                    continue;
                }
                let (nd, nr) = (ix.def_b.get(k2).map_or(0, Vec::len), ix.ref_b.get(k2).map_or(0, Vec::len));
                if nd + nr > KHUB {
                    continue;
                }
                for &b2 in ix.def_b.get(k2).unwrap_or(&none) {
                    let w2 = w_of(b2);
                    for &(f, _, _) in &m.blocks[b2 as usize].files {
                        r2_max(&mut r.hop, f, R2_W_HOP * w2);
                    }
                }
                for &f in ix.citing.get(k2).unwrap_or(&none) {
                    r2_max(&mut r.hop, f, R2_W_HOP * R2_W_CITED);
                }
            }
        }
        for &f in ix.citing.get(c).unwrap_or(&none) {
            r2_max(&mut r.cited, f, R2_W_CITED);
            r2_max(&mut r.noroles, f, R2_W_CITED);
        }
        // the control without roles: any unit that defines OR merely mentions the id names files
        for &b in ix.def_b.get(c).unwrap_or(&none).iter().chain(ix.ref_b.get(c).unwrap_or(&none)) {
            for &(f, _, _) in &m.blocks[b as usize].files {
                r2_max(&mut r.noroles, f, R2_W_NAMED);
            }
        }
    }
    r
}

/// One subject id mention at the parent: resolved by the direct rules, or the scopes still
/// defining it (with whether that scope's defining row is still pending), or unresolvable.
enum R2Cand {
    Done(String, Tier, &'static str),
    Amb(String, Vec<(String, bool)>),
    Fail(&'static str),
}

/// Phase 1 of one Kina test commit: candidates, their reach, whether the subject names an id, stats.
type R2Phase1 = (Vec<R2Cand>, HashMap<String, R2Reach>, bool, BTreeMap<String, f64>);

/// Phase 1 for one Kina test commit: the subject's id mentions and what each candidate key reaches.
fn r2_kina_commit(m: &KinaModel, subj: &str) -> (Vec<R2Cand>, HashMap<String, R2Reach>, bool) {
    let Some(ids) = m.kids.as_ref() else { return (Vec::new(), HashMap::new(), false) };
    let ix = r2_kidx(m);
    let qual = spec_refs(subj);
    let ments: Vec<(String, bool)> =
        kid_mentions(subj, false).into_iter().filter(|x| ids.accepts(&x.2, x.3)).map(|x| (x.2, x.3)).collect::<BTreeSet<_>>().into_iter().collect();
    let has_id = !ments.is_empty();
    let mut cands = Vec::new();
    let mut keys: BTreeSet<String> = BTreeSet::new();
    for (id, bare) in &ments {
        match ids.resolve_direct(id, *bare, None, &qual) {
            Ok(Some((k, tier, route))) => {
                keys.insert(k.clone());
                cands.push(R2Cand::Done(k, tier, route));
            }
            Ok(None) => {
                let scopes: Vec<(String, bool)> = ids
                    .defs
                    .get(id)
                    .into_iter()
                    .flatten()
                    .map(|s| {
                        let k = format!("id:{s}|{id}");
                        let pending = ix.def_b.get(&k).is_some_and(|bs| bs.iter().any(|&b| kblock_state(&m.blocks[b as usize]) == Some(true)));
                        keys.insert(k);
                        (s.clone(), pending)
                    })
                    .collect();
                cands.push(R2Cand::Amb(id.clone(), scopes));
            }
            Err(e) => cands.push(R2Cand::Fail(e)),
        }
    }
    let reach: HashMap<String, R2Reach> = keys.into_iter().map(|k| (k.clone(), r2_kina_reach(m, &ix, &k))).collect();
    (cands, reach, has_id)
}

/// Phase 2 (sequential, oldest test commit first): an ambiguous id takes the one scope whose row is
/// still pending, else a scope an earlier test commit resolved to (newest first), else the newest
/// spec (HEURISTIC). Returns the keys picked, and the scopes for the recent list.
fn r2_pick(cands: &[R2Cand], recent: &std::collections::VecDeque<String>, stats: &mut BTreeMap<String, f64>) -> (Vec<String>, Vec<String>) {
    let mut keys = Vec::new();
    let mut scopes = Vec::new();
    for c in cands {
        match c {
            R2Cand::Done(k, tier, route) => {
                kbump(stats, format!("r2_route_{route}_{}", tier.name()));
                kbump(stats, format!("r2_route_last={route}"));
                keys.push(k.clone());
                if let Some((s, _)) = k.strip_prefix("id:").and_then(|x| x.rsplit_once('|')) {
                    scopes.push(s.to_string());
                }
            }
            R2Cand::Fail(e) => kbump(stats, format!("r2_unresolved_{e}")),
            R2Cand::Amb(id, sc) => {
                let pend: Vec<&(String, bool)> = sc.iter().filter(|x| x.1).collect();
                let pool: Vec<&(String, bool)> = if pend.is_empty() { sc.iter().collect() } else { pend };
                let (s, route) = if pool.len() == 1 {
                    (pool[0].0.clone(), if sc.len() > 1 && sc.iter().filter(|x| x.1).count() == 1 { "open-row" } else { "one-left" })
                } else if let Some(r) = recent.iter().find(|r| pool.iter().any(|p| &p.0 == *r)) {
                    (r.clone(), "recent-scope")
                } else {
                    let best = pool.iter().filter(|p| p.0.starts_with("specs/")).map(|p| p.0.clone()).max().unwrap_or_else(|| pool[0].0.clone());
                    (best, "newest-spec")
                };
                kbump(stats, format!("r2_route_{route}"));
                kbump(stats, format!("r2_route_last={route}"));
                keys.push(format!("id:{s}|{id}"));
                scopes.push(s);
            }
        }
    }
    (keys, scopes)
}

/// glia: the subject's packets (a defined id, or a family of defined ids) -> their lineage class ->
/// files_touched (pending = no LANDED wave at the parent) and the code citing them.
fn r2_glia_commit(gm: &GliaModel, wave: &BTreeMap<String, String>, subj: &str, stats: &mut BTreeMap<String, f64>) -> R2Idr {
    let pix: HashMap<&str, usize> = gm.pk_ids.iter().enumerate().map(|(i, p)| (p.as_str(), i)).collect();
    let prefixes: BTreeSet<&str> = gm.pk_ids.iter().map(|id| id.split('.').next().unwrap_or("")).collect();
    let mut fams: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, id) in gm.pk_ids.iter().enumerate() {
        fams.entry(family(id)).or_default().push(i);
    }
    let mut edges = Vec::new();
    for (a, b, t, _) in &gm.lineage {
        edges.push((a.clone(), b.clone(), *t, false));
    }
    let mut st = BTreeMap::new();
    let cl = KLin::build(&edges, 6, &mut st);
    let mut members: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, id) in gm.pk_ids.iter().enumerate() {
        members.entry(cl.cl.of(&format!("pk:{id}")).to_string()).or_default().push(i);
    }
    let mut has_id = false;
    let mut start: BTreeSet<usize> = BTreeSet::new();
    for (p, id) in dot_ids(subj) {
        if !prefixes.contains(p.as_str()) {
            continue;
        }
        has_id = true;
        if let Some(&i) = pix.get(id.as_str()) {
            start.insert(i);
            kbump(stats, "r2_route_packet-id");
        } else if let Some(ms) = fams.get(&id) {
            start.extend(ms.iter().copied());
            kbump(stats, "r2_route_family");
        } else {
            kbump(stats, "r2_unresolved_undefined");
        }
    }
    let mut reach = Vec::new();
    for &i in &start {
        let mut r = R2Reach::default();
        let classes = cl.exp(&format!("pk:{}", gm.pk_ids[i]));
        let mem: BTreeSet<usize> = classes.iter().flat_map(|c| members.get(c).into_iter().flatten().copied()).collect();
        let w_of = |j: usize| if wave.contains_key(&gm.pk_ids[j]) { R2_W_DONE } else { R2_W_NAMED };
        for &j in &mem {
            for &(f, _, _) in &gm.pk_files[j] {
                r2_max(&mut r.named, f, w_of(j));
                r2_max(&mut r.noroles, f, R2_W_NAMED);
            }
            for other in &gm.pk_ment[j] {
                if let Some(&k) = pix.get(other.as_str())
                    && !mem.contains(&k)
                {
                    for &(f, _, _) in &gm.pk_files[k] {
                        r2_max(&mut r.hop, f, R2_W_HOP * w_of(k));
                    }
                }
            }
        }
        // the control without roles: every packet whose prose merely names one of the class's packets
        for (k, ms) in gm.pk_ment.iter().enumerate() {
            if ms.iter().any(|x| pix.get(x.as_str()).is_some_and(|j| mem.contains(j))) {
                for &(f, _, _) in &gm.pk_files[k] {
                    r2_max(&mut r.noroles, f, R2_W_NAMED);
                }
            }
        }
        for (f, refs) in gm.code_pk.iter().enumerate() {
            if refs.iter().any(|x| cl.exp(&x.key).iter().any(|c| classes.contains(c))) {
                r2_max(&mut r.cited, f as u32, R2_W_CITED);
                r2_max(&mut r.noroles, f as u32, R2_W_CITED);
            }
        }
        reach.push(r);
    }
    r2_finish(&reach.iter().collect::<Vec<_>>(), has_id)
}

/// The conventional-commit scopes of a subject: `feat(a,b): ..` -> [a, b].
fn r2_scopes(subj: &str) -> Vec<String> {
    let Some(open) = subj.find('(') else { return Vec::new() };
    let head = &subj[..open];
    if head.is_empty() || !head.chars().all(|c| c.is_ascii_alphabetic()) {
        return Vec::new();
    }
    let Some(close) = subj[open..].find(')') else { return Vec::new() };
    let after = &subj[open + close + 1..];
    if !(after.starts_with(':') || after.starts_with("!:")) {
        return Vec::new();
    }
    subj[open + 1..open + close]
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s.len() >= 3 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .collect()
}

/// Other repos: a scope that names a doc directory (`features/<scope>/`, `specs/NNN-<scope>/`) ->
/// the files those docs name (path resolver), pending / done by the line's checkbox.
fn r2_scope_commit(git: &str, parent: &str, subj: &str, kp: &KPaths, stats: &mut BTreeMap<String, f64>) -> R2Idr {
    let scopes = r2_scopes(subj);
    if scopes.is_empty() {
        return R2Idr::default();
    }
    let mut specs: Vec<String> = Vec::new();
    for s in &scopes {
        specs.push(format!(":(glob)**/{s}/**/*.md"));
        specs.push(format!(":(glob)**/[0-9][0-9][0-9]-{s}/**/*.md"));
    }
    specs.push(":!**/node_modules/**".into());
    let spec_refs: Vec<&str> = specs.iter().map(String::as_str).collect();
    let lines = git_grep(git, parent, None, &spec_refs);
    let mut reach = Vec::new();
    let mut has_id = false;
    for s in &scopes {
        let mut r = R2Reach::default();
        let mine = |p: &str| p.split('/').any(|seg| seg == s || seg.len() > 4 && seg[..3].bytes().all(|b| b.is_ascii_digit()) && &seg[4..] == s.as_str());
        let mut any = false;
        for (p, l) in lines.iter().filter(|(p, _)| mine(dir_of(p))) {
            let _ = p;
            any = true;
            let t = l.trim_start();
            let done = t.starts_with("- [x]") || t.starts_with("- [X]") || t.starts_with("* [x]");
            for (f, _, _, _) in kp.block(&kpath_tokens(l), stats) {
                r2_max(&mut r.named, f, if done { R2_W_DONE } else { R2_W_NAMED });
                r2_max(&mut r.noroles, f, R2_W_NAMED);
            }
        }
        if any {
            has_id = true;
            kbump(stats, "r2_route_scope-dir");
            reach.push(r);
        }
    }
    r2_finish(&reach.iter().collect::<Vec<_>>(), has_id)
}

/// IDR for every test commit (in test order), read at each parent revision.
fn r2_build(args: &Args, commits: &[Commit], test_idx: &[usize], paths: &[String], pidx: &HashMap<&str, u32>) -> (Vec<R2Idr>, BTreeMap<String, f64>) {
    let t = Instant::now();
    let nf = paths.len();
    let parents = git_parents(&args.git, &args.rev);
    let renames = git_renames(&args.git, &args.rev);
    let kp = KPaths::new(paths, &renames);
    let pr = PathRes::new(paths);
    let _ = pidx;
    let mut stats: BTreeMap<String, f64> = BTreeMap::new();
    let parent_of = |k: usize| parents.get(&commits[k].sha).and_then(|x| x.0.clone());
    let out: Vec<R2Idr> = match args.pre.as_str() {
        "kina" => {
            assert!(!args.wiki.is_empty() && !args.law_sources.is_empty() && !args.law_cache.is_empty(), "--round2 with --pre kina needs --wiki, --law-sources and --law-cache");
            let known = law_known(args);
            let wtext = git_text(&args.wiki, &["log", "--format=%H%x1f%ct", "HEAD"]).expect("wiki log");
            let mut wiki_revs: Vec<(i64, String)> = wtext.lines().filter_map(|l| l.split_once('\x1f').map(|(s, ct)| (ct.trim().parse().unwrap_or(0), s.to_string()))).collect();
            wiki_revs.sort();
            let wcache: std::sync::Mutex<HashMap<String, DocLines>> = std::sync::Mutex::new(HashMap::new());
            let phase1: Vec<R2Phase1> = test_idx
                .par_iter()
                .map(|&k| {
                    let mut st = BTreeMap::new();
                    let Some(p) = parent_of(k) else { return (Vec::new(), HashMap::new(), false, st) };
                    let pct = parents.get(&p).map_or(0, |x| x.1);
                    let mut docs = kina_doc_lines(&args.git, &p);
                    if let Some((_, w)) = wiki_revs.iter().rev().find(|(ct, _)| *ct <= pct) {
                        let cached = wcache.lock().unwrap().get(w).cloned();
                        let lines = cached.unwrap_or_else(|| {
                            let l = std::sync::Arc::new(git_grep(&args.wiki, w, None, &["*.md"]));
                            wcache.lock().unwrap().insert(w.clone(), l.clone());
                            l
                        });
                        docs.extend(lines.iter().cloned());
                    }
                    let code_lines = git_grep(&args.git, &p, Some(&prv().keys_code_pat), &[]);
                    let m = kina_model(&docs, &code_lines, &kp, &pr, &known, None, nf, false, false);
                    let (c, r, h) = r2_kina_commit(&m, subject(&commits[k].msg));
                    kbump(&mut st, if h { "r2_test_commits_with_subject_id" } else { "r2_test_commits_without_subject_id" });
                    (c, r, h, st)
                })
                .collect();
            let mut recent: std::collections::VecDeque<String> = std::collections::VecDeque::new();
            let mut out = Vec::new();
            // GLIA_R2_DUMP=<file>: per test commit, the subject's keys, routes and reach (inspection only)
            let mut dump = String::new();
            for (o, (cands, reach, has_id, st)) in phase1.into_iter().enumerate() {
                for (a, b) in st {
                    *stats.entry(a).or_default() += b;
                }
                let before = stats.clone();
                let (keys, scopes) = r2_pick(&cands, &recent, &mut stats);
                if std::env::var("GLIA_R2_DUMP").is_ok() {
                    let routes: Vec<String> = stats.iter().filter(|(k, v)| k.starts_with("r2_route_last=") && **v > before.get(*k).copied().unwrap_or(0.0)).map(|(k, _)| k[14..].to_string()).collect();
                    let c = &commits[test_idx[o]];
                    let rs: Vec<&R2Reach> = keys.iter().filter_map(|k| reach.get(k)).collect();
                    let x = r2_finish(&rs, has_id);
                    let truth: BTreeSet<u32> = c.files.iter().copied().collect();
                    let mut top = x.idr.clone();
                    top.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                    let hits = x.idr.iter().filter(|(f, _)| truth.contains(f)).count();
                    let tops: Vec<String> = top.iter().take(12).map(|(f, w)| format!("{}{}:{w}", if truth.contains(f) { "*" } else { "" }, paths[*f as usize])).collect();
                    dump.push_str(&format!("{}\t{}\t{}\t{}\t{}\t{}/{}\t{}\n", &c.sha[..8], subject(&c.msg), keys.join(" "), routes.join(" "), x.idr.len(), hits, c.files.len(), tops.join(" ")));
                }
                for s in scopes {
                    recent.retain(|x| *x != s);
                    recent.push_front(s);
                    recent.truncate(R2_RECENT);
                }
                let rs: Vec<&R2Reach> = keys.iter().filter_map(|k| reach.get(k)).collect();
                let x = r2_finish(&rs, has_id);
                if !x.idr.is_empty() {
                    kbump(&mut stats, "r2_test_commits_reaching_files");
                }
                out.push(x);
            }
            if let Ok(f) = std::env::var("GLIA_R2_DUMP") {
                std::fs::write(f, dump).expect("r2 dump");
            }
            stats.retain(|k, _| !k.starts_with("r2_route_last="));
            out
        }
        "glia" => {
            let cache = BlobCache { text: std::sync::Mutex::new(HashMap::new()), packets: std::sync::Mutex::new(HashMap::new()) };
            let pkc: std::sync::Mutex<HashMap<String, std::sync::Arc<GPkFile>>> = std::sync::Mutex::new(HashMap::new());
            let got: Vec<(R2Idr, BTreeMap<String, f64>)> = test_idx
                .par_iter()
                .map(|&k| {
                    let mut st = BTreeMap::new();
                    let Some(p) = parent_of(k) else { return (R2Idr::default(), st) };
                    let gd = glia_docs(&args.git, &p, &cache);
                    let code_lines = git_grep(&args.git, &p, Some("[A-Z][A-Z0-9]{0,4}[.][0-9]|Q[0-9]"), &[]);
                    let gm = glia_model(&args.git, &p, &cache, &pkc, &code_lines, &kp, &pr, None, nf, false);
                    let x = r2_glia_commit(&gm, &gd.wave, subject(&commits[k].msg), &mut st);
                    kbump(&mut st, if x.has_id { "r2_test_commits_with_subject_id" } else { "r2_test_commits_without_subject_id" });
                    if !x.idr.is_empty() {
                        kbump(&mut st, "r2_test_commits_reaching_files");
                    }
                    (x, st)
                })
                .collect();
            got.into_iter()
                .map(|(x, st)| {
                    for (a, b) in st {
                        *stats.entry(a).or_default() += b;
                    }
                    x
                })
                .collect()
        }
        _ => {
            let got: Vec<(R2Idr, BTreeMap<String, f64>)> = test_idx
                .par_iter()
                .map(|&k| {
                    let mut st = BTreeMap::new();
                    let Some(p) = parent_of(k) else { return (R2Idr::default(), st) };
                    let x = r2_scope_commit(&args.git, &p, subject(&commits[k].msg), &kp, &mut st);
                    kbump(&mut st, if x.has_id { "r2_test_commits_with_subject_id" } else { "r2_test_commits_without_subject_id" });
                    if !x.idr.is_empty() {
                        kbump(&mut st, "r2_test_commits_reaching_files");
                    }
                    (x, st)
                })
                .collect();
            got.into_iter()
                .map(|(x, st)| {
                    for (a, b) in st {
                        *stats.entry(a).or_default() += b;
                    }
                    x
                })
                .collect()
        }
    };
    eprintln!("[round2] id routing for {} test commits in {:.1}s: {stats:?}", out.len(), t.elapsed().as_secs_f64());
    (out, stats)
}

/// The round-2 rankers for one query (appended after every round-1 ranker).
#[allow(clippy::too_many_arguments)]
fn round2_rankers(out: &mut Out, x: &R2Idr, seed: usize, t: &[u32], p1: &[f64], c0: &[f64], t1s: &[f64], pt: Option<&[f64]>) {
    let nf = p1.len();
    let d = |v: &[(u32, f64)]| dense(nf, v, seed);
    let (idr, named, cited, hop, nor) = (d(&x.idr), d(&x.named), d(&x.cited), d(&x.hop), d(&x.noroles));
    out.r1cov.push(("idr_id".into(), x.has_id));
    out.r1cov.push(("idr".into(), r1_cov(&idr, seed)));
    let mm = |v: &[f64]| minmax(v, seed);
    let (mm_idr, mm_p1, mm_c0, mm_t1s, mm_hop, mm_nor) = (mm(&idr), mm(p1), mm(c0), mm(t1s), mm(&hop), mm(&nor));
    out.push("IDR", &scalar(&idr), seed, t);
    out.push("IDRn", &scalar(&named), seed, t);
    out.push("IDRc", &scalar(&cited), seed, t);
    out.push("IDRh", &scalar(&hop), seed, t);
    out.push("IDR~roles", &scalar(&nor), seed, t);
    // typed links first, structure second; and the min-max blends
    out.push("LX[IDR,P1]", &lex2(&idr, p1), seed, t);
    out.push("HI50[P1]", &scalar(&blend(0.5, &mm_idr, &mm_p1)), seed, t);
    out.push("HI25[P1]", &scalar(&blend(0.25, &mm_idr, &mm_p1)), seed, t);
    // with the subject's text (H9s50 = 0.5 mm(T1s) + 0.5 mm(P1))
    let h9s50 = blend(0.5, &mm_t1s, &mm_p1);
    let eq = |parts: &[&[f64]]| -> Vec<f64> { (0..nf).map(|f| parts.iter().map(|p| p[f]).sum::<f64>() / parts.len() as f64).collect() };
    out.push("LX[IDR,H9s50]", &lex2(&idr, &h9s50), seed, t);
    out.push("H9si", &scalar(&eq(&[&mm_idr, &mm_t1s, &mm_p1])), seed, t);
    // with co-change
    let lx: Vec<Key> = (0..nf).map(|f| [idr[f], c0[f], p1[f]]).collect();
    out.push("LX[IDR,C0,P1]", &lx, seed, t);
    out.push("HCi", &scalar(&eq(&[&mm_idr, &mm_c0, &mm_p1])), seed, t);
    // text as a ranker behind the typed links: IDR's files ordered by the subject's BM25 (then IDR),
    // every other file by P1 after them; text adds no candidate
    let it: Vec<Key> = (0..nf)
        .map(|f| if idr[f] > 0.0 { [1.0, mm_t1s[f] + 1e-3 * mm_idr[f], p1[f]] } else { [0.0, 0.0, p1[f]] })
        .collect();
    out.push("IDRt", &it, seed, t);
    let it: Vec<Key> = (0..nf)
        .map(|f| if idr[f] > 0.0 { [1.0, mm_t1s[f] + 1e-3 * mm_idr[f], h9s50[f]] } else { [0.0, 0.0, h9s50[f]] })
        .collect();
    out.push("LX[IDRt,H9s50]", &it, seed, t);
    // roles off / one hop, in the main text hybrid
    out.push("LX[IDRh,H9s50]", &lex2(&hop, &h9s50), seed, t);
    out.push("LX[IDR~roles,H9s50]", &lex2(&nor, &h9s50), seed, t);
    out.push("H9si~roles", &scalar(&eq(&[&mm_nor, &mm_t1s, &mm_p1])), seed, t);
    out.push("H9sih", &scalar(&eq(&[&mm_hop, &mm_t1s, &mm_p1])), seed, t);
    if let Some(pt) = pt {
        let mm_pt = mm(pt);
        let h18 = eq(&[&mm_t1s, &mm_c0, &mm_pt]);
        let h9s25 = blend(0.25, &mm_t1s, &mm_pt);
        out.push("LX[IDR,H18s[PT*]]", &lex2(&idr, &h18), seed, t);
        out.push("LX[IDR,H9s25[PT*]]", &lex2(&idr, &h9s25), seed, t);
        out.push("H18si[PT*]", &scalar(&eq(&[&mm_idr, &mm_t1s, &mm_c0, &mm_pt])), seed, t);
        let lx: Vec<Key> = (0..nf).map(|f| [idr[f], c0[f], pt[f]]).collect();
        out.push("LX[IDR,C0,PT*]", &lx, seed, t);
        out.push("LX[IDRh,H18s[PT*]]", &lex2(&hop, &h18), seed, t);
        out.push("LX[IDR~roles,H18s[PT*]]", &lex2(&nor, &h18), seed, t);
    }
}

// ============================================================================
// Round 3 (2026-10-19): `--round3 on` (with `--round1 on --round2 on`).
//
// A. IDH, id history: the subject's ids -> the earlier training commits whose subjects named the
//    same id -> the files they touched, weighted by recency and count. Ids: the scoped ids round 2
//    resolves (Kina: the resolved spec-scoped keys; glia: packet ids) plus ticket ids
//    `[A-Z][A-Z0-9]+-N` and `#N` references (a `#N` carries only once another commit names the
//    same number, so a squash-merge PR number is a no-op). Controls: each key with history
//    swapped for another key seen by then (one of the 5 most recently active, IDH~; any, IDH~u).
// B. Per-query text gating: the subject's BM25 match enters only when its confidence (top-score
//    margin, informative matched terms, top-file peakedness, top score) clears a threshold picked
//    online from earlier test commits; control: a random gate at the same rate. Per-bucket online
//    selection over confidence tertiles (SELb), with a random-bucket control.
// C. History across a monorepo import (`--r3-anc`): the pre-merge repos' histories, their paths
//    mapped onto the import's subdirectories (verified by blob match at the import), merged by
//    author time before the repo's own; their eligible commits warm online selection up and are
//    never scored. Controls: `--r3-anc-map perm:<i,j,..>` (ancestor i lands on ancestor j's
//    subdirectory, files permuted) and `within` (files permuted inside the right subdirectory).
// D. SELt3: SELt2's candidates plus B0, H10[B0] and H18s[B0] (SELt3a adds IDH's fusions; SELt3x
//    adds `--r3-selx`, chosen after the first run and so post-hoc).
// T. Transfer shapes: review (seed 2, 3 or half of the changed files, score the rest) and find
//    (no seed: the subject's BM25 over history, PPR seeded from its top files, IDR / IDH).
// Every ranker is appended after round 2's, so every earlier ranker is unchanged.
// ============================================================================

/// IDH: the j-th newest earlier commit naming a key weighs R3_IDH_DECAY^j (recency and count).
const R3_IDH_DECAY: f64 = 0.7;
/// A hyphenated id with one of these prefixes names an encoding, digest or standard, not a ticket.
const R3_TICKET_STOP: &[&str] = &[
    "UTF", "UCS", "SHA", "MD", "ISO", "IEC", "CRC", "AES", "RSA", "DES", "TLS", "SSL", "HTTP", "IPV", "ES", "EC",
    "ECMA", "HMAC", "PKCS", "JSR", "JEP", "RFC", "PEP", "ANSI", "IEEE", "CP", "KOI", "GB", "WIN", "IBM", "JDK", "JRE",
    "JLS", "API", "CSS", "HTML",
];
/// B: a matched query term is informative when its BM25 idf is at least this (df under about 5%).
const R3_IDF_MIN: f64 = 3.0;
/// The find shape: PPR and co-change are seeded from the text ranking's top this-many files.
const R3_FIND_TOP: usize = 10;
/// SELb: buckets open after this many earlier test commits; a bucket needs this many to pick.
const R3_BUCKET_WARM: usize = 30;
const R3_BUCKET_MIN: usize = 10;
const R3_CONF: [&str; 4] = ["mg", "tm", "pk", "s1"];
const R3_SHAPES: [&str; 4] = ["k2", "k3", "half", "find"];
/// B: (text ranker, its text-free fallback).
const R3_GATE_PAIRS: [(&str, &str); 4] = [("H9s50", "P1"), ("H9s25[PT*]", "PT*"), ("H18s[PT*]", "H10[PT*]"), ("LX[IDR,H9s50]", "LX[IDR,P1]")];
const R3_SELT2: [&str; 14] = [
    "H18s[PT*]",
    "H9s25[PT*]",
    "H9s50",
    "H10[PT*]",
    "HTsa[W5]",
    "HTsc[W5]",
    "HTsd[W5]",
    "HTsc[M]",
    "HTsd[M]",
    "ALL5",
    "LX[IDR,H18s[PT*]]",
    "LX[IDR,H9s25[PT*]]",
    "H18si[PT*]",
    "LX[IDR,C0,PT*]",
];
const R3_BFS: [&str; 3] = ["B0", "H10[B0]", "H18s[B0]"];
const R3_IDH_FUSIONS: [&str; 2] = ["LX[IDRH,H18s[PT*]]", "LX[IDRH,C0,PT*]"];
/// The find shape's SELt2 (the same fusions with the text-seeded PPR and co-change) and SELt3a.
const R3_SELF2: [&str; 9] = ["H18x", "H9x25T", "H9x", "H10x", "ALL5x", "LX[IDR,H18x]", "LX[IDR,H9x25T]", "H18xi", "LX[IDR,C0x,PXT]"];
const R3_SELF3: [&str; 2] = ["LX[IDRH,H18x]", "LX[IDRH,C0x,PXT]"];

/// Ticket ids in a subject: `[A-Z][A-Z0-9]+-N` (not an encoding or standard) and `#N`.
fn r3_tickets(subj: &str) -> Vec<String> {
    let mut v: Vec<String> = hyphen_ids(subj).into_iter().filter(|(p, _)| !R3_TICKET_STOP.contains(&p.as_str())).map(|x| x.1).collect();
    let b = subj.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'#' && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let e = digits_at(b, i + 1, 6);
            if e > i + 1 && we(b, e) {
                v.push(subj[i..e].to_string());
            }
        }
    }
    v.sort();
    v.dedup();
    v
}

/// Every commit's IDH keys, read from its subject, in replay order. Kina: the scoped keys round 2's
/// resolvers give at the commit's parent (the ambiguity cascade replayed over every commit);
/// glia: packet ids of a defined prefix, plus tickets; other repos: tickets.
fn r3_keys(args: &Args, commits: &[Commit], paths: &[String], stats: &mut BTreeMap<String, f64>) -> Vec<BTreeSet<String>> {
    let t = Instant::now();
    let n = commits.len();
    let mut keys: Vec<BTreeSet<String>> = vec![BTreeSet::new(); n];
    match args.pre.as_str() {
        "kina" => {
            assert!(!args.wiki.is_empty() && !args.law_sources.is_empty() && !args.law_cache.is_empty(), "--round3 with --pre kina needs --wiki, --law-sources and --law-cache");
            let parents = git_parents(&args.git, &args.rev);
            let renames = git_renames(&args.git, &args.rev);
            let kp = KPaths::new(paths, &renames);
            let pr = PathRes::new(paths);
            let known = law_known(args);
            let wtext = git_text(&args.wiki, &["log", "--format=%H%x1f%ct", "HEAD"]).expect("wiki log");
            let mut wiki_revs: Vec<(i64, String)> =
                wtext.lines().filter_map(|l| l.split_once('\x1f').map(|(s, ct)| (ct.trim().parse().unwrap_or(0), s.to_string()))).collect();
            wiki_revs.sort();
            let wcache: std::sync::Mutex<HashMap<String, DocLines>> = std::sync::Mutex::new(HashMap::new());
            let bare: BTreeSet<u8> = bare_kinds();
            let idx: Vec<usize> = (0..n)
                .filter(|&k| {
                    let s = subject(&commits[k].msg);
                    !hyphen_ids(s).is_empty() || !bare_ids(s, &bare).is_empty()
                })
                .collect();
            let got: Vec<Vec<R2Cand>> = idx
                .par_iter()
                .map(|&k| {
                    let Some(p) = parents.get(&commits[k].sha).and_then(|x| x.0.clone()) else { return Vec::new() };
                    let pct = parents.get(&p).map_or(0, |x| x.1);
                    let mut docs = kina_doc_lines(&args.git, &p);
                    if let Some((_, w)) = wiki_revs.iter().rev().find(|(ct, _)| *ct <= pct) {
                        let cached = wcache.lock().unwrap().get(w).cloned();
                        let lines = cached.unwrap_or_else(|| {
                            let l = std::sync::Arc::new(git_grep(&args.wiki, w, None, &["*.md"]));
                            wcache.lock().unwrap().insert(w.clone(), l.clone());
                            l
                        });
                        docs.extend(lines.iter().cloned());
                    }
                    let code_lines = git_grep(&args.git, &p, Some(&prv().keys_code_pat), &[]);
                    let m = kina_model(&docs, &code_lines, &kp, &pr, &known, None, paths.len(), false, false);
                    r2_kina_commit(&m, subject(&commits[k].msg)).0
                })
                .collect();
            let mut recent: std::collections::VecDeque<String> = std::collections::VecDeque::new();
            let mut junk = BTreeMap::new();
            for (&k, c) in idx.iter().zip(got) {
                let (ks, scopes) = r2_pick(&c, &recent, &mut junk);
                for s in scopes {
                    recent.retain(|x| *x != s);
                    recent.push_front(s);
                    recent.truncate(R2_RECENT);
                }
                keys[k].extend(ks);
            }
            stats.insert("r3_commits_scanned_for_scoped_ids".into(), idx.len() as f64);
        }
        "glia" => {
            let cache = PreCache::default();
            let blobs = ls_tree(&args.git, &args.rev, GLIA_PACKETS);
            let prefixes: BTreeSet<String> = GLIA_PACKETS
                .iter()
                .filter_map(|p| blobs.get(*p))
                .flat_map(|b| cache.get(&args.git, b).keys().map(|id| id.split('.').next().unwrap_or("").to_string()).collect::<Vec<_>>())
                .collect();
            for (k, c) in commits.iter().enumerate() {
                let s = subject(&c.msg);
                for (p, id) in dot_ids(s) {
                    if prefixes.contains(&p) {
                        keys[k].insert(format!("pk:{id}"));
                    }
                }
                keys[k].extend(r3_tickets(s));
            }
        }
        _ => {
            for (k, c) in commits.iter().enumerate() {
                keys[k].extend(r3_tickets(subject(&c.msg)));
            }
        }
    }
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for ks in &keys {
        for key in ks {
            let kind = if key.starts_with("id:") {
                "scoped"
            } else if key.starts_with("pk:") {
                "packet"
            } else if key.starts_with('#') {
                "issue"
            } else {
                "ticket"
            };
            *kinds.entry(kind.to_string()).or_default() += 1;
        }
    }
    for (kd, c) in kinds {
        stats.insert(format!("r3_key_mentions_{kd}"), c as f64);
    }
    stats.insert("r3_commits_with_keys".into(), keys.iter().filter(|k| !k.is_empty()).count() as f64);
    eprintln!("[round3] subject keys for {n} commits in {:.1}s", t.elapsed().as_secs_f64());
    keys
}

/// IDH's replay state: per key, the files of every earlier training commit naming it.
#[derive(Default)]
struct R3Idh {
    hist: HashMap<String, Vec<Vec<u32>>>,
    last: HashMap<String, usize>,
    pos: usize,
}

impl R3Idh {
    /// One key's signal, normalised to max 1: the j-th newest commit's files weigh R3_IDH_DECAY^j.
    fn sig(&self, key: &str, acc: &mut BTreeMap<u32, f64>) {
        let Some(l) = self.hist.get(key) else { return };
        let mut one: BTreeMap<u32, f64> = BTreeMap::new();
        for (j, fs) in l.iter().rev().enumerate() {
            let w = R3_IDH_DECAY.powi(j as i32);
            for &f in fs {
                *one.entry(f).or_default() += w;
            }
        }
        let mx = one.values().fold(0.0f64, |a, &b| a.max(b));
        if mx > 0.0 {
            for (f, v) in one {
                *acc.entry(f).or_default() += v / mx;
            }
        }
    }

    /// (IDH, IDH~ recent-5 control, IDH~u uniform control, keys with history).
    fn test(&self, keys: &BTreeSet<String>, sha: &str) -> (FileScores, FileScores, FileScores, usize) {
        let (mut a, mut c, mut u) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
        let mut recent: Vec<(&String, usize)> = self.last.iter().filter(|(k, _)| !keys.contains(*k)).map(|(k, &p)| (k, p)).collect();
        recent.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(y.0)));
        let mut all: Vec<&String> = recent.iter().map(|x| x.0).collect();
        all.sort();
        recent.truncate(R1_SHUF_RECENT);
        let mut hit = 0;
        for key in keys {
            if !self.hist.contains_key(key) {
                continue;
            }
            hit += 1;
            self.sig(key, &mut a);
            if !recent.is_empty() {
                let pick = recent[(h64(&format!("{sha}:{key}:idh~")) % recent.len() as u64) as usize].0;
                self.sig(pick, &mut c);
            }
            if !all.is_empty() {
                let pick = all[(h64(&format!("{sha}:{key}:idh~u")) % all.len() as u64) as usize];
                self.sig(pick, &mut u);
            }
        }
        let v = |m: BTreeMap<u32, f64>| m.into_iter().collect::<Vec<_>>();
        (v(a), v(c), v(u), hit)
    }

    fn train(&mut self, keys: &BTreeSet<String>, files: &[u32]) {
        for k in keys {
            self.hist.entry(k.clone()).or_default().push(files.to_vec());
            self.last.insert(k.clone(), self.pos);
        }
        self.pos += 1;
    }
}

/// One test commit's round-3 signals.
#[derive(Default, Clone)]
struct R3Commit {
    n_keys: usize,
    n_hist: usize,
    idh: Vec<(u32, f64)>,
    idh_c: Vec<(u32, f64)>,
    idh_u: Vec<(u32, f64)>,
    /// B: [margin, informative matched terms, top-file peakedness, top score] of the subject's BM25
    conf: [f64; 4],
    /// C: an ancestor commit that only warms online selection up (never scored)
    warm: bool,
}

/// B's confidence of the subject's BM25 match over the history (`bm`, every earlier message).
fn r3_conf(bm: &Bm25, stoks: &[String], t1s: &[(u32, f64)]) -> [f64; 4] {
    let sc = bm.scores(stoks);
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in &sc {
        if v > s1 {
            s2 = s1;
            s1 = v;
        } else if v > s2 {
            s2 = v;
        }
    }
    let mg = if s1 > 0.0 { (s1 - s2) / s1 } else { 0.0 };
    let n = bm.dl.len() as f64;
    let uniq: BTreeSet<&String> = stoks.iter().collect();
    let tm = uniq
        .iter()
        .filter(|t| {
            bm.post.get(**t).is_some_and(|pl| {
                let df = pl.len() as f64;
                (1.0 + (n - df + 0.5) / (df + 0.5)).ln() >= R3_IDF_MIN
            })
        })
        .count() as f64;
    let mut v: Vec<f64> = t1s.iter().map(|x| x.1).filter(|&x| x > 0.0).collect();
    v.sort_by(|a, b| b.total_cmp(a));
    let top: f64 = v.iter().take(10).sum();
    let pk = if top > 0.0 { v[0] / top } else { 0.0 };
    [mg, tm, pk, s1]
}

/// A transfer-shape query: review (several changed files seed, the rest are targets) or find (no
/// seed file; `seeds` are the text ranking's top files with their weights, every changed file a target).
struct TQuery {
    tc: usize,
    shape: usize,
    seeds: Vec<u32>,
    w: Vec<f64>,
    targets: Vec<u32>,
    c0: Vec<(u32, f64)>,
}

/// The transfer queries of one test commit, with co-change read before it.
fn r3_tqueries(o: usize, c: &Commit, paths: &[String], co: &[HashMap<u32, u32>], cnt: &[u32], t1s: &[(u32, f64)], out: &mut Vec<TQuery>) {
    let mut ord = c.files.clone();
    ord.sort_by_key(|&f| h64(&format!("{}:{}", c.sha, paths[f as usize])));
    let n = ord.len();
    for (shape, k) in [(0usize, 2usize), (1, 3), (2, n / 2)] {
        let need = if shape == 2 { 4 } else { k + 1 };
        if n < need {
            continue;
        }
        let seeds = ord[..k].to_vec();
        let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
        for &s in &seeds {
            let d = f64::from(cnt[s as usize]) + 1.0;
            for (&f, &x) in &co[s as usize] {
                *acc.entry(f).or_default() += f64::from(x) / d;
            }
        }
        out.push(TQuery { tc: o, shape, seeds, w: vec![1.0; k], targets: ord[k..].to_vec(), c0: acc.into_iter().collect() });
    }
    let mut top: Vec<(u32, f64)> = t1s.iter().copied().filter(|x| x.1 > 0.0).collect();
    top.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    top.truncate(R3_FIND_TOP);
    let tot: f64 = top.iter().map(|x| x.1).sum();
    let mut acc: BTreeMap<u32, f64> = BTreeMap::new();
    for &(g, wg) in &top {
        let d = f64::from(cnt[g as usize]) + 1.0;
        for (&f, &x) in &co[g as usize] {
            *acc.entry(f).or_default() += wg / tot * f64::from(x) / d;
        }
    }
    out.push(TQuery {
        tc: o,
        shape: 3,
        seeds: top.iter().map(|x| x.0).collect(),
        w: top.iter().map(|x| x.1 / tot).collect(),
        targets: c.files.clone(),
        c0: acc.into_iter().collect(),
    });
}

/// recall@5/10/20, MRR, nDCG@20 with every excluded file (the seeds) left out of the ranking.
fn eval_ex(key: &[Key], excl: &[bool], targets: &[u32]) -> [f64; 5] {
    let mut ranks: Vec<usize> = targets
        .iter()
        .map(|&t| {
            let t = t as usize;
            let kt = key[t];
            let mut r = 1;
            for (f, kf) in key.iter().enumerate() {
                if !excl[f] && f != t && better(kf, f, &kt, t) {
                    r += 1;
                }
            }
            r
        })
        .collect();
    ranks.sort_unstable();
    let nt = targets.len() as f64;
    let rec = |k: usize| ranks.iter().filter(|&&r| r <= k).count() as f64 / nt;
    let mrr = 1.0 / ranks[0] as f64;
    let dcg: f64 = ranks.iter().filter(|&&r| r <= 20).map(|&r| 1.0 / ((r + 1) as f64).log2()).sum();
    let idcg: f64 = (1..=targets.len().min(20)).map(|i| 1.0 / ((i + 1) as f64).log2()).sum();
    [rec(5), rec(10), rec(20), mrr, dcg / idcg]
}

/// Min-max over the non-excluded files (as `minmax` with one seed); a flat vector is 0.
fn mm_ex(v: &[f64], excl: &[bool]) -> Vec<f64> {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (f, &s) in v.iter().enumerate() {
        if !excl[f] {
            lo = lo.min(s);
            hi = hi.max(s);
        }
    }
    if hi.partial_cmp(&lo) != Some(std::cmp::Ordering::Greater) {
        return vec![0.0; v.len()];
    }
    v.iter().map(|&s| (s - lo) / (hi - lo)).collect()
}

/// Rank normalisation over the non-excluded files (as `ranknorm` with one seed).
fn rn_ex(key: &[Key], excl: &[bool]) -> Vec<f64> {
    let mut ix: Vec<usize> = (0..key.len()).filter(|&f| !excl[f]).collect();
    ix.sort_by(|&a, &b| {
        if better(&key[a], a, &key[b], b) {
            std::cmp::Ordering::Less
        } else if a == b {
            std::cmp::Ordering::Equal
        } else {
            std::cmp::Ordering::Greater
        }
    });
    let m = ix.len().max(2) as f64 - 1.0;
    let mut out = vec![0.0; key.len()];
    for (pos, &f) in ix.iter().enumerate() {
        out[f] = (m - pos as f64) / m;
    }
    out
}

fn r3_eq(parts: &[&[f64]]) -> Vec<f64> {
    let nf = parts[0].len();
    (0..nf).map(|f| parts.iter().map(|p| p[f]).sum::<f64>() / parts.len() as f64).collect()
}

/// The inputs of the seeded bank (dense, per file; the seeds excluded).
struct R3In<'a> {
    excl: &'a [bool],
    p1: &'a [f64],
    pt: &'a [f64],
    c0: &'a [f64],
    t1s: &'a [f64],
    w5: &'a [f64],
    m: &'a [f64],
    idr: &'a [f64],
    idh: &'a [f64],
    idh_c: &'a [f64],
    idh_u: &'a [f64],
    b0: Option<&'a [Key]>,
}

/// The seeded-shape rankers. `full` adds the earlier rounds' rankers the transfer shapes need (the
/// single-seed shape already has them; there they are recomputed only to check this code).
fn r3_bank_seed(x: &R3In, full: bool) -> Vec<(String, Vec<Key>)> {
    let nf = x.p1.len();
    let mm = |v: &[f64]| mm_ex(v, x.excl);
    let (mm_p1, mm_pt, mm_c0, mm_t1s, mm_w5, mm_m, mm_idr) = (mm(x.p1), mm(x.pt), mm(x.c0), mm(x.t1s), mm(x.w5), mm(x.m), mm(x.idr));
    let h9s50 = blend(0.5, &mm_t1s, &mm_p1);
    let h9s25 = blend(0.25, &mm_t1s, &mm_pt);
    let h18 = r3_eq(&[&mm_t1s, &mm_c0, &mm_pt]);
    let lx3 = |a: &[f64], b: &[f64], c: &[f64]| -> Vec<Key> { (0..nf).map(|f| [a[f], b[f], c[f]]).collect() };
    let mut out: Vec<(String, Vec<Key>)> = Vec::new();
    let mut push = |n: &str, k: Vec<Key>| out.push((n.to_string(), k));
    if full {
        push("P1", scalar(x.p1));
        push("PT*", scalar(x.pt));
        push("C0", scalar(x.c0));
        push("T1s", scalar(x.t1s));
        push("H10", lx3(x.c0, x.p1, &vec![0.0; nf]));
        push("H10[PT*]", lex2(x.c0, x.pt));
        push("H9s50", scalar(&h9s50));
        push("H9s25[PT*]", scalar(&h9s25));
        push("H18s[PT*]", scalar(&h18));
        for (tag, a, b, c) in HT_GRID {
            let s: Vec<f64> = (0..nf).map(|f| a * mm_t1s[f] + b * mm_w5[f] + c * mm_p1[f]).collect();
            push(&format!("HTs{tag}[W5]"), scalar(&s));
            let s: Vec<f64> = (0..nf).map(|f| a * mm_t1s[f] + b * mm_m[f] + c * mm_p1[f]).collect();
            push(&format!("HTs{tag}[M]"), scalar(&s));
        }
        push("ALL5", scalar(&r3_eq(&[&mm_t1s, &mm_w5, &mm_m, &mm_c0, &mm_p1])));
        push("IDR", scalar(x.idr));
        push("LX[IDR,P1]", lex2(x.idr, x.p1));
        push("LX[IDR,H9s50]", lex2(x.idr, &h9s50));
        push("H9si", scalar(&r3_eq(&[&mm_idr, &mm_t1s, &mm_p1])));
        push("LX[IDR,C0,P1]", lx3(x.idr, x.c0, x.p1));
        push("LX[IDR,H18s[PT*]]", lex2(x.idr, &h18));
        push("LX[IDR,H9s25[PT*]]", lex2(x.idr, &h9s25));
        push("H18si[PT*]", scalar(&r3_eq(&[&mm_idr, &mm_t1s, &mm_c0, &mm_pt])));
        push("LX[IDR,C0,PT*]", lx3(x.idr, x.c0, x.pt));
        if let Some(b0) = x.b0 {
            push("B0", b0.to_vec());
        }
    }
    // ---- A: id history ----
    let idrh: Vec<f64> = (0..nf).map(|f| x.idr[f] + x.idh[f]).collect();
    let idrh_c: Vec<f64> = (0..nf).map(|f| x.idr[f] + x.idh_c[f]).collect();
    push("IDH", scalar(x.idh));
    push("IDH~", scalar(x.idh_c));
    push("IDH~u", scalar(x.idh_u));
    push("IDRH", scalar(&idrh));
    push("LX[IDH,P1]", lex2(x.idh, x.p1));
    push("LX[IDH,H9s50]", lex2(x.idh, &h9s50));
    push("LX[IDH~,H9s50]", lex2(x.idh_c, &h9s50));
    push("LX[IDH~u,H9s50]", lex2(x.idh_u, &h9s50));
    push("LX[IDH,H18s[PT*]]", lex2(x.idh, &h18));
    push("LX[IDRH,P1]", lex2(&idrh, x.p1));
    push("LX[IDRH,H9s50]", lex2(&idrh, &h9s50));
    push("LX[IDRH~,H9s50]", lex2(&idrh_c, &h9s50));
    push("LX[IDRH,H18s[PT*]]", lex2(&idrh, &h18));
    push("LX[IDRH~,H18s[PT*]]", lex2(&idrh_c, &h18));
    push("LX[IDRH,H9s25[PT*]]", lex2(&idrh, &h9s25));
    push("LX[IDRH,C0,PT*]", lx3(&idrh, x.c0, x.pt));
    push("LX[IDRH~,C0,PT*]", lx3(&idrh_c, x.c0, x.pt));
    push("LX[IDR,IDH,H9s50]", lx3(x.idr, x.idh, &h9s50));
    push("H18sih[PT*]", scalar(&r3_eq(&[&mm(&idrh), &mm_t1s, &mm_c0, &mm_pt])));
    // ---- D: BFS bases ----
    if let Some(b0) = x.b0 {
        let rn_b0 = rn_ex(b0, x.excl);
        push("H10[B0]", (0..nf).map(|f| [x.c0[f], b0[f][0], b0[f][1]]).collect());
        let h18b = r3_eq(&[&mm_t1s, &mm_c0, &rn_b0]);
        push("H18s[B0]", scalar(&h18b));
        push("H9s25[B0]", scalar(&blend(0.25, &mm_t1s, &rn_b0)));
        push("LX[IDRH,H18s[B0]]", lex2(&idrh, &h18b));
    }
    out
}

/// The find shape's inputs (no seed file).
struct R3Find<'a> {
    t1s: &'a [f64],
    t1ss: &'a [f64],
    px1: &'a [f64],
    pxt: &'a [f64],
    c0x: &'a [f64],
    w5: &'a [f64],
    m: &'a [f64],
    idr: &'a [f64],
    idh: &'a [f64],
    idh_c: &'a [f64],
    deg: &'a [f64],
}

/// The find-shape rankers: the subject's text ranking, PPR seeded from its top files (PX1 the
/// base config max-pooled, PXT the picked config sum-pooled), co-change from them, IDR / IDH.
fn r3_bank_find(x: &R3Find) -> Vec<(String, Vec<Key>)> {
    let nf = x.t1s.len();
    let none = vec![false; nf];
    let mm = |v: &[f64]| mm_ex(v, &none);
    let (mm_t1s, mm_px1, mm_pxt, mm_c0x, mm_w5, mm_m, mm_idr) = (mm(x.t1s), mm(x.px1), mm(x.pxt), mm(x.c0x), mm(x.w5), mm(x.m), mm(x.idr));
    let idrh: Vec<f64> = (0..nf).map(|f| x.idr[f] + x.idh[f]).collect();
    let idrh_c: Vec<f64> = (0..nf).map(|f| x.idr[f] + x.idh_c[f]).collect();
    let h9x = blend(0.5, &mm_t1s, &mm_px1);
    let h9x25 = blend(0.25, &mm_t1s, &mm_pxt);
    let h18x = r3_eq(&[&mm_t1s, &mm_c0x, &mm_pxt]);
    let lx3 = |a: &[f64], b: &[f64], c: &[f64]| -> Vec<Key> { (0..nf).map(|f| [a[f], b[f], c[f]]).collect() };
    let mut out: Vec<(String, Vec<Key>)> = Vec::new();
    let mut push = |n: &str, k: Vec<Key>| out.push((n.to_string(), k));
    push("B1", scalar(x.deg));
    push("T1s", scalar(x.t1s));
    push("T1ss", scalar(x.t1ss));
    push("PX1", scalar(x.px1));
    push("PXT", scalar(x.pxt));
    push("C0x", scalar(x.c0x));
    push("IDR", scalar(x.idr));
    push("IDH", scalar(x.idh));
    push("IDRH", scalar(&idrh));
    push("H9x", scalar(&h9x));
    push("H9x25T", scalar(&h9x25));
    push("H18x", scalar(&h18x));
    push("H10x", lex2(x.c0x, x.pxt));
    push("H10x1", lex2(x.c0x, x.px1));
    push("ALL5x", scalar(&r3_eq(&[&mm_t1s, &mm_w5, &mm_m, &mm_c0x, &mm_pxt])));
    push("LX[IDR,T1s]", lex2(x.idr, x.t1s));
    push("LX[IDR,H9x]", lex2(x.idr, &h9x));
    push("H9xi", scalar(&r3_eq(&[&mm_idr, &mm_t1s, &mm_px1])));
    push("LX[IDR,H18x]", lex2(x.idr, &h18x));
    push("LX[IDR,H9x25T]", lex2(x.idr, &h9x25));
    push("H18xi", scalar(&r3_eq(&[&mm_idr, &mm_t1s, &mm_c0x, &mm_pxt])));
    push("LX[IDR,C0x,PXT]", lx3(x.idr, x.c0x, x.pxt));
    push("LX[IDH,H9x]", lex2(x.idh, &h9x));
    push("LX[IDRH,T1s]", lex2(&idrh, x.t1s));
    push("LX[IDRH,H9x]", lex2(&idrh, &h9x));
    push("LX[IDRH,H18x]", lex2(&idrh, &h18x));
    push("LX[IDRH~,H18x]", lex2(&idrh_c, &h18x));
    push("LX[IDRH,C0x,PXT]", lx3(&idrh, x.c0x, x.pxt));
    // ties in the first keys go to the file's degree (most files score 0 without a seed)
    for (_, k) in out.iter_mut() {
        if k.iter().all(|v| v[2] == 0.0) {
            for (f, v) in k.iter_mut().enumerate() {
                v[2] = x.deg[f];
            }
        }
    }
    out
}

/// The single-seed shape's round-3 rankers (appended after round 2's). The earlier rankers the
/// bank recomputes are checked against the ones already in `out` (they must be identical).
#[allow(clippy::too_many_arguments)]
fn round3_rankers(out: &mut Out, r: &R1Commit, x3: &R3Commit, seed: usize, t: &[u32], p1: &[f64], pt: &[f64], c0: &[f64], t1s: &[f64], w5: &[f64], b0: &[Key]) {
    let nf = p1.len();
    let d = |v: &[(u32, f64)]| dense(nf, v, seed);
    let mut excl = vec![false; nf];
    excl[seed] = true;
    let m = d(&r.ms[0].1);
    let idr = r.r2.as_ref().map_or_else(|| vec![0.0; nf], |x| d(&x.idr));
    let (idh, idh_c, idh_u) = (d(&x3.idh), d(&x3.idh_c), d(&x3.idh_u));
    out.r1cov.push(("idh_id".into(), x3.n_keys > 0));
    out.r1cov.push(("idh".into(), r1_cov(&idh, seed)));
    let x = R3In { excl: &excl, p1, pt, c0, t1s, w5, m: &m, idr: &idr, idh: &idh, idh_c: &idh_c, idh_u: &idh_u, b0: Some(b0) };
    let mut bad = 0.0;
    for (name, key) in r3_bank_seed(&x, true) {
        let mv = evaluate(&key, seed, t);
        if let Some(i) = out.names.iter().position(|n| *n == name) {
            if out.m[i] != mv {
                bad += 1.0;
            }
            continue;
        }
        out.names.push(name);
        out.m.push(mv);
    }
    out.r3_bad += bad;
    // attribution: IDH first over the subject's BM25 with its digit-bearing tokens (ticket numbers) removed
    let mm_p1 = minmax(p1, seed);
    let h9nd = blend(0.5, &minmax(&d(&r.t1s_nd), seed), &mm_p1);
    out.push("LX[IDH,H9s50-nd]", &lex2(&idh, &h9nd), seed, t);
    out.push("LX[IDH~,H9s50-nd]", &lex2(&idh_c, &h9nd), seed, t);
}

/// Runs the transfer queries; per shape, the queries' results (in test order) and their test commits.
/// With `ctx.ans`, each query's bank gains the ANS rankers last (ANS:impact / ANS:pack on the
/// seed shapes, ANS:find on the find shape, over `subjects[q.tc]`, the test commit's subject).
fn r3_transfer(ctx: &Ctx, tqs: &[TQuery], r1s: &[R1Commit], subjects: &[String]) -> Vec<(Vec<Out>, Vec<usize>)> {
    let g = ctx.g;
    let nf = g.paths.len();
    let res: Vec<Out> = tqs
        .par_iter()
        .map(|q| {
            let r = &r1s[q.tc];
            let find = q.shape == 3;
            let mut excl = vec![false; nf];
            let mut pers: Vec<(usize, f64)> = Vec::new();
            if find {
                for (i, &f) in q.seeds.iter().enumerate() {
                    let ns = &g.file_nodes[f as usize];
                    for &u in ns {
                        pers.push((g.node_ppr[u as usize], q.w[i] / ns.len() as f64));
                    }
                }
            } else {
                for &s in &q.seeds {
                    excl[s as usize] = true;
                }
                let nodes: Vec<usize> = q.seeds.iter().flat_map(|&f| g.file_nodes[f as usize].iter().map(|&u| g.node_ppr[u as usize])).collect();
                pers = Ppr::uniform(&nodes);
            }
            let run = |idx: &Ppr, sum: bool| -> Vec<f64> {
                if pers.is_empty() {
                    return vec![0.0; nf];
                }
                let v = idx.run(&pers);
                file_pool(g, |u| v[g.node_ppr[u]], sum)
            };
            let p1 = run(&ctx.ppr[0].1, false);
            let pt = match ctx.pick {
                Some(i) => run(&ctx.tune[i].1, true),
                None => run(&ctx.ppr[0].1, true),
            };
            let d = |v: &[(u32, f64)]| {
                let mut x = vec![0.0; nf];
                for &(f, s) in v {
                    x[f as usize] = s;
                }
                x
            };
            let x3 = r.r3.clone().unwrap_or_default();
            let (c0, t1s, t1ss, w5, m) = (d(&q.c0), d(&r.t1s), d(&r.t1ss), d(&r.w[2]), d(&r.ms[0].1));
            let idr = r.r2.as_ref().map_or_else(|| vec![0.0; nf], |x| d(&x.idr));
            let (idh, idh_c, idh_u) = (d(&x3.idh), d(&x3.idh_c), d(&x3.idh_u));
            let mut bank = if find {
                r3_bank_find(&R3Find { t1s: &t1s, t1ss: &t1ss, px1: &p1, pxt: &pt, c0x: &c0, w5: &w5, m: &m, idr: &idr, idh: &idh, idh_c: &idh_c, deg: &g.file_deg })
            } else {
                let seed_ids: Vec<NodeId> = q.seeds.iter().flat_map(|&f| g.file_nodes[f as usize].iter().map(|&u| g.ids[u as usize])).collect();
                let (b0, _) = bfs_key(ctx, &ctx.adj_carry, &seed_ids);
                r3_bank_seed(&R3In { excl: &excl, p1: &p1, pt: &pt, c0: &c0, t1s: &t1s, w5: &w5, m: &m, idr: &idr, idh: &idh, idh_c: &idh_c, idh_u: &idh_u, b0: Some(&b0) }, true)
            };
            if ctx.ans {
                bank.extend(if find { ans_keys_find(ctx, &subjects[q.tc]) } else { ans_keys_seed(ctx, q.tc, &q.seeds) });
            }
            let eng = r.eng.as_ref().map(|e| eng_bank_transfer(e, &excl, &p1, &t1s, &g.file_deg, find)).unwrap_or_default();
            let mut out = Out::default();
            for (name, key) in bank {
                out.names.push(name);
                out.m.push(eval_ex(&key, &excl, &q.targets));
            }
            out.eng = eng.into_iter().map(|(name, key)| (name, eval_ex(&key, &excl, &q.targets))).collect();
            out
        })
        .collect();
    let mut shapes: Vec<(Vec<Out>, Vec<usize>)> = (0..R3_SHAPES.len()).map(|_| (Vec::new(), Vec::new())).collect();
    for (q, o) in tqs.iter().zip(res) {
        shapes[q.shape].0.push(o);
        shapes[q.shape].1.push(q.tc);
    }
    shapes
}

/// Online selection over dynamic candidate sets (round 1's rule: at each test commit, the candidate
/// with the best summed recall@10 over every earlier test commit's queries; the first before any).
fn r3_select(results: &mut [Out], tc_of: &[usize], ntc: usize, sets: &[(String, Vec<String>)]) -> serde_json::Value {
    let names = results[0].names.clone();
    let pos = |n: &str| names.iter().position(|x| x == n);
    let mut by_tc: Vec<Vec<usize>> = vec![Vec::new(); ntc];
    for (q, &t) in tc_of.iter().enumerate() {
        by_tc[t].push(q);
    }
    let mut info = serde_json::Map::new();
    for (sname, cands) in sets {
        let ix: Vec<usize> = cands.iter().filter_map(|c| pos(c)).collect();
        if ix.len() != cands.len() {
            let miss: Vec<&String> = cands.iter().filter(|c| pos(c).is_none()).collect();
            info.insert(sname.clone(), json!({ "skipped_missing": miss }));
            continue;
        }
        let mut sum: Vec<f64> = vec![0.0; ix.len()];
        let mut seen = 0usize;
        let mut picks: BTreeMap<String, usize> = BTreeMap::new();
        let mut per_q: Vec<[f64; 5]> = vec![[0.0; 5]; tc_of.len()];
        let mut last = String::new();
        for qs in &by_tc {
            if qs.is_empty() {
                continue;
            }
            let best = if seen == 0 { 0 } else { (0..ix.len()).max_by(|&a, &b| sum[a].total_cmp(&sum[b]).then(b.cmp(&a))).unwrap_or(0) };
            last = names[ix[best]].clone();
            *picks.entry(last.clone()).or_default() += 1;
            for &q in qs {
                per_q[q] = results[q].m[ix[best]];
            }
            for &q in qs {
                for (j, &ri) in ix.iter().enumerate() {
                    sum[j] += results[q].m[ri][1];
                }
                seen += 1;
            }
        }
        for (q, r) in results.iter_mut().enumerate() {
            r.names.push(sname.clone());
            r.m.push(per_q[q]);
        }
        info.insert(sname.clone(), json!({"picks_per_test_commit": picks, "pick_at_the_last_test_commit": last}));
    }
    serde_json::Value::Object(info)
}

/// B: the gated rankers G<conf>[X|Y] (X while the commit's confidence clears the threshold picked
/// online, else Y), their random-gate controls G<conf>r[X|Y] at the same on-rate, and the
/// confidence-bucket selections SELb<conf> (with the random-bucket control SELb<conf>r).
fn r3_gates(results: &mut [Out], tc_of: &[usize], conf: &[[f64; 4]], shas: &[String], pairs: &[(&str, &str)], cands_b: &[String]) -> serde_json::Value {
    let ntc = conf.len();
    let names = results[0].names.clone();
    let pos = |n: &str| names.iter().position(|x| x == n);
    let mut by_tc: Vec<Vec<usize>> = vec![Vec::new(); ntc];
    for (q, &t) in tc_of.iter().enumerate() {
        by_tc[t].push(q);
    }
    let unif = |s: &str| (h64(s) >> 11) as f64 / (1u64 << 53) as f64;
    let mut info = serde_json::Map::new();
    let mut cols: Vec<(String, Vec<[f64; 5]>)> = Vec::new();
    let live: Vec<usize> = (0..ntc).filter(|&t| !by_tc[t].is_empty()).collect();
    for (fi, fname) in R3_CONF.iter().enumerate() {
        for &(xn, yn) in pairs {
            let (Some(ix), Some(iy)) = (pos(xn), pos(yn)) else { continue };
            let dsum: Vec<f64> = (0..ntc).map(|t| by_tc[t].iter().map(|&q| results[q].m[ix][1] - results[q].m[iy][1]).sum()).collect();
            let mut on = vec![false; ntc];
            let mut thr = f64::NEG_INFINITY;
            for (j, &t) in live.iter().enumerate() {
                let earlier = &live[..j];
                let mut cands: Vec<f64> = earlier.iter().map(|&c| conf[c][fi]).collect();
                cands.sort_by(f64::total_cmp);
                cands.dedup();
                let mut best = (f64::NEG_INFINITY, earlier.iter().map(|&c| dsum[c]).sum::<f64>());
                for &th in &cands {
                    let s: f64 = earlier.iter().filter(|&&c| conf[c][fi] >= th).map(|&c| dsum[c]).sum();
                    if s > best.1 {
                        best = (th, s);
                    }
                }
                if 0.0 > best.1 {
                    best = (f64::INFINITY, 0.0);
                }
                thr = best.0;
                on[t] = conf[t][fi] >= best.0;
            }
            let rate = live.iter().filter(|&&t| on[t]).count() as f64 / live.len().max(1) as f64;
            let on_r: Vec<bool> = (0..ntc).map(|t| unif(&format!("{}:gate:{fname}:{xn}", shas[t])) < rate).collect();
            let rate_r = live.iter().filter(|&&t| on_r[t]).count() as f64 / live.len().max(1) as f64;
            let col = |gate: &[bool]| -> Vec<[f64; 5]> { tc_of.iter().enumerate().map(|(q, &t)| if gate[t] { results[q].m[ix] } else { results[q].m[iy] }).collect() };
            cols.push((format!("G{fname}[{xn}|{yn}]"), col(&on)));
            cols.push((format!("G{fname}r[{xn}|{yn}]"), col(&on_r)));
            info.insert(format!("G{fname}[{xn}|{yn}]"), json!({"on_rate": rate, "control_on_rate": rate_r, "last_threshold": if thr.is_finite() { json!(thr) } else { json!(format!("{thr}")) }}));
        }
        let cix: Vec<usize> = cands_b.iter().filter_map(|c| pos(c)).collect();
        if cix.len() != cands_b.len() {
            continue;
        }
        let csum: Vec<Vec<f64>> = (0..ntc).map(|t| cix.iter().map(|&i| by_tc[t].iter().map(|&q| results[q].m[i][1]).sum()).collect()).collect();
        for variant in ["", "r"] {
            let vals: Vec<f64> = (0..ntc).map(|t| if variant.is_empty() { conf[t][fi] } else { unif(&format!("{}:bucket:{fname}", shas[t])) }).collect();
            let mut pick = vec![0usize; ntc];
            let mut bucketed = 0usize;
            for (j, &t) in live.iter().enumerate() {
                let earlier = &live[..j];
                let mut members: Vec<usize> = earlier.to_vec();
                if earlier.len() >= R3_BUCKET_WARM {
                    let mut e: Vec<f64> = earlier.iter().map(|&c| vals[c]).collect();
                    e.sort_by(f64::total_cmp);
                    let (c1, c2) = (e[e.len() / 3], e[2 * e.len() / 3]);
                    let b = |x: f64| if x < c1 { 0 } else if x < c2 { 1 } else { 2 };
                    let bt = b(vals[t]);
                    let mem: Vec<usize> = earlier.iter().copied().filter(|&c| b(vals[c]) == bt).collect();
                    if mem.len() >= R3_BUCKET_MIN {
                        members = mem;
                        bucketed += 1;
                    }
                }
                if members.is_empty() {
                    continue;
                }
                let s: Vec<f64> = (0..cix.len()).map(|a| members.iter().map(|&c| csum[c][a]).sum()).collect();
                pick[t] = (0..cix.len()).max_by(|&a, &b2| s[a].total_cmp(&s[b2]).then(b2.cmp(&a))).unwrap_or(0);
            }
            let name = format!("SELb{fname}{variant}");
            cols.push((name.clone(), tc_of.iter().enumerate().map(|(q, &t)| results[q].m[cix[pick[t]]]).collect()));
            let mut picks: BTreeMap<String, usize> = BTreeMap::new();
            for &t in &live {
                *picks.entry(names[cix[pick[t]]].clone()).or_default() += 1;
            }
            info.insert(name, json!({"bucketed_commits": bucketed, "picks_per_test_commit": picks}));
        }
    }
    for (n, col) in cols {
        for (q, r) in results.iter_mut().enumerate() {
            r.names.push(n.clone());
            r.m.push(col[q]);
        }
    }
    serde_json::Value::Object(info)
}

/// The selection sets of round 3 (main and review shapes; find has its own).
fn r3_sets(args: &Args) -> Vec<(String, Vec<String>)> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
    let t3: Vec<String> = R3_SELT2.iter().chain(R3_BFS.iter()).map(|x| x.to_string()).collect();
    let mut t3a = t3.clone();
    t3a.extend(s(&R3_IDH_FUSIONS));
    let mut out = vec![("SELt3".to_string(), t3.clone()), ("SELt3a".to_string(), t3a)];
    // the text-free answers (impact, blast radius): structure alone, and co-change first then structure
    out.push(("SELs0".to_string(), s(&["P1", "PT*", "B0"])));
    out.push(("SELh0".to_string(), s(&["H10", "H10[PT*]", "H10[B0]"])));
    if !args.r3_selx.is_empty() {
        let mut x = t3;
        x.extend(args.r3_selx.split(',').map(|v| v.trim().to_string()).filter(|v| !v.is_empty()));
        out.push(("SELt3x".to_string(), x));
    }
    out
}

/// The main shape's round-3 contrasts: (a, b, subset).
fn r3_contrasts() -> Vec<(String, String, &'static str)> {
    let mut v: Vec<(String, String, &'static str)> = Vec::new();
    let mut add = |a: &str, b: &str, s: &'static str| v.push((a.to_string(), b.to_string(), s));
    // A
    for s in ["all", "idh", "idh_id"] {
        add("IDH", "IDH~", s);
        add("IDH", "IDH~u", s);
        add("IDH", "P1", s);
        add("LX[IDH,H9s50]", "H9s50", s);
        add("LX[IDH,H9s50]", "LX[IDH~,H9s50]", s);
        add("LX[IDH,H9s50]", "LX[IDH~u,H9s50]", s);
        add("LX[IDH,H18s[PT*]]", "H18s[PT*]", s);
        add("LX[IDRH,H9s50]", "LX[IDR,H9s50]", s);
        add("LX[IDRH,H9s50]", "LX[IDRH~,H9s50]", s);
        add("LX[IDRH,H18s[PT*]]", "LX[IDR,H18s[PT*]]", s);
        add("LX[IDRH,H18s[PT*]]", "LX[IDRH~,H18s[PT*]]", s);
        add("LX[IDRH,C0,PT*]", "LX[IDR,C0,PT*]", s);
        add("LX[IDRH,C0,PT*]", "LX[IDRH~,C0,PT*]", s);
        add("LX[IDR,IDH,H9s50]", "LX[IDR,H9s50]", s);
        add("H18sih[PT*]", "H18si[PT*]", s);
        add("LX[IDRH,P1]", "LX[IDR,P1]", s);
        add("LX[IDH,H9s50-nd]", "H9s50-nd", s);
        add("LX[IDH,H9s50-nd]", "LX[IDH~,H9s50-nd]", s);
        add("H9s50", "H9s50-nd", s);
    }
    for (x, y) in R3_GATE_PAIRS {
        add(x, y, "all");
    }
    for s in ["all", "idr_id", "idr_miss", "idh"] {
        add("LX[IDRH,H9s50]", "H9a50[P1]", s);
        add("LX[IDR,H9s50]", "H9a50[P1]", s);
        add("LX[IDRH,H9s50]", "P1", s);
        add("LX[IDRH,H18s[PT*]]", "H9a50[P1]", s);
        add("LX[IDRH,H9s50]", "LX[IDR,H9s50]", s);
        add("LX[IDRH,H18s[PT*]]", "LX[IDR,H18s[PT*]]", s);
    }
    // B
    for (fname, _) in R3_CONF.iter().zip(0..) {
        for (x, y) in R3_GATE_PAIRS {
            let g = format!("G{fname}[{x}|{y}]");
            let gr = format!("G{fname}r[{x}|{y}]");
            v.push((g.clone(), x.to_string(), "all"));
            v.push((g.clone(), y.to_string(), "all"));
            v.push((g, gr, "all"));
        }
        v.push((format!("SELb{fname}"), "SELt2".into(), "all"));
        v.push((format!("SELb{fname}"), format!("SELb{fname}r"), "all"));
        v.push((format!("SELb{fname}"), "P1".into(), "all"));
    }
    // D
    let mut add = |a: &str, b: &str, s: &'static str| v.push((a.to_string(), b.to_string(), s));
    for (a, b) in [
        ("SELt3", "SELt2"),
        ("SELt3", "P1"),
        ("SELt3", "B0"),
        ("SELt3", "H9a50[P1]"),
        ("SELt3", "PT*"),
        ("SELt2", "B0"),
        ("SELt2", "P1"),
        ("PT*", "B0"),
        ("H10[B0]", "H10[PT*]"),
        ("H18s[B0]", "H18s[PT*]"),
        ("H18s[B0]", "B0"),
        ("SELt3a", "SELt3"),
        ("SELt3a", "SELt2"),
        ("SELt3a", "P1"),
        ("SELt3x", "SELt3"),
        ("SELt3x", "SELt2"),
        ("SELt3x", "P1"),
        ("SELt3x", "B0"),
        ("SELt3x", "H9a50[P1]"),
        ("SELs0", "P1"),
        ("SELs0", "B0"),
        ("SELs0", "PT*"),
        ("SELh0", "H10"),
        ("SELh0", "H10[PT*]"),
        ("SELh0", "H10[B0]"),
        ("H10[B0]", "H10"),
        // the 0.5.3 mechanisms in the single-seed shape (for the transfer table)
        ("H10", "P1"),
        ("H10[PT*]", "PT*"),
        ("H9s50", "P1"),
        ("H18s[PT*]", "P1"),
        ("LX[IDR,H9s50]", "H9s50"),
        ("LX[IDR,H9s50]", "H9si"),
        ("LX[IDR,H9s50]", "P1"),
        ("LX[IDR,H18s[PT*]]", "P1"),
        ("SELt2", "H18s[PT*]"),
        ("B0", "P1"),
        ("T1s", "P1"),
    ] {
        add(a, b, "all");
    }
    v
}

/// The transfer shapes' contrasts.
fn r3_tcontrasts(find: bool) -> Vec<(&'static str, &'static str)> {
    if find {
        vec![
            ("PXT", "PX1"),
            ("PX1", "B1"),
            ("T1s", "B1"),
            ("H10x", "PXT"),
            ("H10x1", "PX1"),
            ("H9x", "PX1"),
            ("H9x", "T1s"),
            ("H18x", "PXT"),
            ("H18x", "T1s"),
            ("H18x", "H9x"),
            ("LX[IDR,H9x]", "H9x"),
            ("LX[IDR,H9x]", "H9xi"),
            ("LX[IDR,T1s]", "T1s"),
            ("LX[IDR,H18x]", "H18x"),
            ("SELf2", "H9x"),
            ("SELf2", "T1s"),
            ("SELf2", "H18x"),
            ("ALL5x", "H18x"),
            ("ALL5x", "T1s"),
            ("PX1", "T1s"),
            ("PXT", "T1s"),
            ("SELf2t", "T1s"),
            ("SELf2t", "SELf2"),
            ("SELf2t", "ALL5x"),
            ("SELf3", "SELf2"),
            ("LX[IDRH,H18x]", "LX[IDR,H18x]"),
            ("LX[IDRH,H18x]", "LX[IDRH~,H18x]"),
            ("LX[IDRH,H9x]", "LX[IDR,H9x]"),
            ("LX[IDH,H9x]", "H9x"),
        ]
    } else {
        vec![
            ("PT*", "P1"),
            ("H10", "P1"),
            ("H10[PT*]", "PT*"),
            ("H9s50", "P1"),
            ("H18s[PT*]", "P1"),
            ("LX[IDR,H9s50]", "H9s50"),
            ("LX[IDR,H9s50]", "H9si"),
            ("LX[IDR,H9s50]", "P1"),
            ("LX[IDR,H18s[PT*]]", "P1"),
            ("SELt2", "P1"),
            ("SELt2", "H18s[PT*]"),
            ("SELt3", "SELt2"),
            ("SELt3", "P1"),
            ("SELt3", "B0"),
            ("SELt3a", "SELt3"),
            ("SELt3x", "SELt3"),
            ("SELt3x", "P1"),
            ("B0", "P1"),
            ("PT*", "B0"),
            ("SELs0", "P1"),
            ("SELs0", "B0"),
            ("SELs0", "PT*"),
            ("SELh0", "H10"),
            ("SELh0", "H10[B0]"),
            ("H10[B0]", "H10"),
            ("LX[IDRH,H18s[PT*]]", "LX[IDR,H18s[PT*]]"),
            ("LX[IDRH,H18s[PT*]]", "LX[IDRH~,H18s[PT*]]"),
            ("LX[IDRH,H9s50]", "LX[IDR,H9s50]"),
            ("Gmg[H18s[PT*]|H10[PT*]]", "H18s[PT*]"),
            ("Gtm[H18s[PT*]|H10[PT*]]", "H18s[PT*]"),
            ("Gpk[H18s[PT*]|H10[PT*]]", "H18s[PT*]"),
            ("Gs1[H18s[PT*]|H10[PT*]]", "H18s[PT*]"),
            ("Gmg[H9s50|P1]", "H9s50"),
            ("Gtm[H9s50|P1]", "H9s50"),
            ("Gpk[H9s50|P1]", "H9s50"),
            ("Gs1[H9s50|P1]", "H9s50"),
        ]
    }
}

/// Mean metrics of every ranker over `idx` (NaN when empty).
fn r3_means(results: &[Out], names: &[String], idx: &[usize]) -> serde_json::Map<String, serde_json::Value> {
    let mut sj = serde_json::Map::new();
    for (ri, n) in names.iter().enumerate() {
        let mut m = [0.0; 5];
        for &q in idx {
            for (acc, x) in m.iter_mut().zip(results[q].m[ri]) {
                *acc += x;
            }
        }
        let m = m.map(|x| if idx.is_empty() { f64::NAN } else { x / idx.len() as f64 });
        sj.insert(n.clone(), json!({"r5": m[0], "r10": m[1], "r20": m[2], "mrr": m[3], "ndcg20": m[4]}));
    }
    sj
}

#[allow(clippy::too_many_arguments)]
fn r3_contrast_rows(results: &[Out], names: &[String], cs: &[(String, String, String)], subsets: &BTreeMap<String, Vec<usize>>, commit_of: &[usize], boot: usize, lf: &[f64], md: &mut String) -> Vec<serde_json::Value> {
    let pos = |n: &str| names.iter().position(|x| x == n);
    let col = |ri: usize, mi: usize| -> Vec<f64> { results.iter().map(|r| r.m[ri][mi]).collect() };
    let mut cj = Vec::new();
    for (a, b, s) in cs {
        let (Some(ia), Some(ib)) = (pos(a), pos(b)) else { continue };
        let Some(idx) = subsets.get(s) else { continue };
        let mut cells = Vec::new();
        for mi in [1usize, 3] {
            match paired_sub(&col(ia, mi), &col(ib, mi), idx, commit_of, boot, lf) {
                Some(p) => {
                    cells.push(format!("{:+.3} [{:+.3}, {:+.3}]", p.mean, p.lo, p.hi));
                    cj.push(json!({"a": a, "b": b, "metric": if mi == 1 { "r10" } else { "mrr" }, "subset": s, "n": idx.len(),
                        "diff": p.mean, "lo": p.lo, "hi": p.hi, "p_sign": p.p, "wins": p.wins, "losses": p.losses}));
                }
                None => cells.push("-".into()),
            }
        }
        md.push_str(&format!("| {a} | {b} | {s} | {} | {} |\n", idx.len(), cells.join(" | ")));
    }
    cj
}

/// The round-3 report: facts, metrics on the round-3 subsets, contrasts, and the transfer shapes.
#[allow(clippy::too_many_arguments)]
fn round3_report(
    label: &str,
    names: &[String],
    results: &[Out],
    bi: &BootIn,
    transfer: &[(Vec<Out>, Vec<usize>)],
    facts: &BTreeMap<String, f64>,
    info: serde_json::Value,
) -> (serde_json::Value, String) {
    let (commit_of, boot, lf) = (bi.commit_of, bi.boot, bi.lf);
    let nq = results.len();
    let mut cov: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    cov.insert("all".into(), (0..nq).collect());
    let mut flag: BTreeMap<String, Vec<bool>> = BTreeMap::new();
    for (q, r) in results.iter().enumerate() {
        for (k, on) in &r.r1cov {
            flag.entry(k.clone()).or_insert_with(|| vec![false; nq])[q] = *on;
        }
    }
    let get = |k: &str| flag.get(k).cloned().unwrap_or_else(|| vec![false; nq]);
    let (idr_id, idr, idh_id, idh) = (get("idr_id"), get("idr"), get("idh_id"), get("idh"));
    let sel = |f: &dyn Fn(usize) -> bool| -> Vec<usize> { (0..nq).filter(|&q| f(q)).collect() };
    cov.insert("idr_id".into(), sel(&|q| idr_id[q]));
    cov.insert("idr".into(), sel(&|q| idr[q]));
    cov.insert("idh_id".into(), sel(&|q| idh_id[q]));
    cov.insert("idh".into(), sel(&|q| idh[q]));
    cov.insert("idr_miss".into(), sel(&|q| idr_id[q] && !idr[q]));
    cov.insert("idh_new".into(), sel(&|q| idh[q] && !idr[q]));
    let mut md = format!("### {label}: round 3\n\nFacts:\n\n");
    for (k, v) in facts {
        md.push_str(&format!("- {k}: {v}\n"));
    }
    let mut js = serde_json::Map::new();
    js.insert("facts".into(), json!(facts));
    js.insert("info".into(), info);
    js.insert("subset_sizes".into(), json!(cov.iter().map(|(k, v)| (k.clone(), v.len())).collect::<BTreeMap<_, _>>()));
    let mut mj = serde_json::Map::new();
    for (s, idx) in &cov {
        mj.insert(s.clone(), serde_json::Value::Object(r3_means(results, names, idx)));
    }
    js.insert("metrics".into(), serde_json::Value::Object(mj));
    md.push_str("\nContrasts, single-seed shape (a - b; recall@10 and MRR; 95% commit-cluster CI):\n\n| a | b | subset | n | recall@10 | MRR |\n|---|---|---|---|---|---|\n");
    let cs: Vec<(String, String, String)> = r3_contrasts().into_iter().map(|(a, b, s)| (a, b, s.to_string())).collect();
    let cj = r3_contrast_rows(results, names, &cs, &cov, commit_of, boot, lf, &mut md);
    js.insert("contrasts".into(), json!(cj));
    // ---- transfer shapes ----
    let mut tj = serde_json::Map::new();
    for (si, (outs, tc_of)) in transfer.iter().enumerate() {
        let shape = R3_SHAPES[si];
        if outs.is_empty() {
            continue;
        }
        let tnames = outs[0].names.clone();
        let mut sub = BTreeMap::new();
        sub.insert("all".to_string(), (0..outs.len()).collect::<Vec<usize>>());
        md.push_str(&format!("\nTransfer shape {shape} ({} queries):\n\n| a | b | subset | n | recall@10 | MRR |\n|---|---|---|---|---|---|\n", outs.len()));
        let cs: Vec<(String, String, String)> = r3_tcontrasts(si == 3).into_iter().map(|(a, b)| (a.to_string(), b.to_string(), "all".to_string())).collect();
        let cj = r3_contrast_rows(outs, &tnames, &cs, &sub, tc_of, boot, lf, &mut md);
        tj.insert(shape.to_string(), json!({"queries": outs.len(), "metrics": r3_means(outs, &tnames, &sub["all"]), "contrasts": cj}));
    }
    js.insert("transfer".into(), serde_json::Value::Object(tj));
    (serde_json::Value::Object(js), md)
}

/// C: the merged history (ancestors first by author time, then interleaved with the repo's own
/// commits by author time) and, per commit, whether it is the repo's own.
struct R3Anc {
    own: Vec<bool>,
    at: HashMap<String, i64>,
    facts: BTreeMap<String, f64>,
    verify: Vec<serde_json::Value>,
}

/// `--r3-anc dir@rev=prefix@import,..`: each pre-merge repo's history at `rev`, its paths mapped onto
/// `prefix/` (the true map, or a control: `--r3-anc-map perm:<j0,j1,..>` sends ancestor i onto ancestor
/// j_i's subdirectory with its files permuted; `within` permutes files inside the right one). The map
/// is verified by matching the ancestor's tree at `rev` against the import commit's `prefix/` tree.
fn r3_ancestors(args: &Args, own: Vec<Commit>, paths: &[String], pidx: &HashMap<&str, u32>) -> (Vec<Commit>, R3Anc) {
    let specs: Vec<(String, String, String, String)> = args
        .r3_anc
        .split(',')
        .map(|s| {
            let (a, b) = s.split_once('=').expect("--r3-anc dir@rev=prefix@import");
            let (dir, rev) = a.rsplit_once('@').expect("--r3-anc dir@rev");
            let (prefix, imp) = b.rsplit_once('@').expect("--r3-anc prefix@import");
            (dir.to_string(), rev.to_string(), prefix.trim_end_matches('/').to_string(), imp.to_string())
        })
        .collect();
    let mode = args.r3_anc_map.as_str();
    let perm: Vec<usize> = match mode.strip_prefix("perm:") {
        Some(p) => p.split(',').map(|x| x.trim().parse().expect("--r3-anc-map perm:<indices>")).collect(),
        None => (0..specs.len()).collect(),
    };
    assert_eq!(perm.len(), specs.len(), "--r3-anc-map perm needs one index per ancestor");
    let shuffle = !(mode.is_empty() || mode == "true");
    let under = |p: &str| -> Vec<u32> {
        let pre = format!("{p}/");
        (0..paths.len() as u32).filter(|&f| paths[f as usize].starts_with(&pre)).collect()
    };
    let mut facts: BTreeMap<String, f64> = BTreeMap::new();
    let mut verify = Vec::new();
    let mut anc: Vec<(i64, usize, Commit)> = Vec::new();
    let mut at: HashMap<String, i64> = HashMap::new();
    for (i, (dir, rev, prefix, imp)) in specs.iter().enumerate() {
        let parse = |t: &str, strip: &str| -> HashMap<String, String> {
            t.lines()
                .filter_map(|l| {
                    let (meta, p) = l.split_once('\t')?;
                    let blob = meta.split_whitespace().nth(2)?;
                    Some((p.strip_prefix(strip).unwrap_or(p).to_string(), blob.to_string()))
                })
                .collect()
        };
        let ta = git_text(dir, &["ls-tree", "-r", rev]).unwrap_or_default();
        let tb = git_text(&args.git, &["ls-tree", "-r", imp, "--", &format!("{prefix}/")]).unwrap_or_default();
        let (ma, mb) = (parse(&ta, ""), parse(&tb, &format!("{prefix}/")));
        let same_path = ma.keys().filter(|k| mb.contains_key(*k)).count();
        let same_blob = ma.iter().filter(|(k, v)| mb.get(*k) == Some(*v)).count();
        verify.push(json!({"ancestor": i, "prefix": prefix, "ancestor_files": ma.len(), "import_files": mb.len(),
            "same_path": same_path, "same_path_and_blob": same_blob}));
        let dom = under(prefix);
        let mut range = under(&specs[perm[i]].2);
        let mut rng = Rng(h64(&format!("r3anc:{mode}:{i}")));
        for j in (1..range.len()).rev() {
            let k = (rng.next() % (j as u64 + 1)) as usize;
            range.swap(j, k);
        }
        let fmap: HashMap<u32, u32> =
            if shuffle && !range.is_empty() { dom.iter().enumerate().map(|(j, &f)| (f, range[j % range.len()])).collect() } else { HashMap::new() };
        let times = git_author_times(dir, rev);
        let out = Command::new("git")
            .args(["-C", dir, "log", "--topo-order", "--no-merges", "--no-renames", "--name-only", "--format=%x1e%H%x1f%B%x1f", rev])
            .output()
            .expect("git log (ancestor)");
        let text = String::from_utf8_lossy(&out.stdout);
        let (mut mentions, mut mapped) = (0usize, 0usize);
        let mut list = Vec::new();
        for rec in text.split('\x1e').skip(1) {
            let mut parts = rec.splitn(3, '\x1f');
            let sha = parts.next().unwrap_or("").trim().to_string();
            let msg = parts.next().unwrap_or("").to_string();
            let names = parts.next().unwrap_or("");
            let mut files: Vec<u32> = Vec::new();
            for l in names.lines().map(str::trim).filter(|l| !l.is_empty()) {
                mentions += 1;
                if let Some(&f) = pidx.get(format!("{prefix}/{l}").as_str()) {
                    mapped += 1;
                    files.push(if shuffle { fmap.get(&f).copied().unwrap_or(f) } else { f });
                }
            }
            files.sort_unstable();
            files.dedup();
            list.push(Commit { sha, msg, files });
        }
        list.reverse();
        facts.insert(format!("r3_anc{i}_commits"), list.len() as f64);
        facts.insert(format!("r3_anc{i}_file_mentions"), mentions as f64);
        facts.insert(format!("r3_anc{i}_file_mentions_mapped"), mapped as f64);
        for c in list {
            let t = times.get(&c.sha).copied().unwrap_or(0);
            at.insert(c.sha.clone(), t);
            anc.push((t, i, c));
        }
    }
    anc.sort_by_key(|x| (x.0, x.1));
    let own_at = git_author_times(&args.git, &args.rev);
    let mut merged = Vec::new();
    let mut flag = Vec::new();
    let mut ai = anc.into_iter().peekable();
    for c in own {
        let t = own_at.get(&c.sha).copied().unwrap_or(0);
        while ai.peek().is_some_and(|x| x.0 <= t) {
            merged.push(ai.next().expect("peeked").2);
            flag.push(false);
        }
        merged.push(c);
        flag.push(true);
    }
    for x in ai {
        merged.push(x.2);
        flag.push(false);
    }
    facts.insert("r3_anc_commits_before_first_own".into(), flag.iter().take_while(|&&o| !o).count() as f64);
    eprintln!("[round3] ancestors ({mode}): {facts:?} verify={verify:?}");
    (merged, R3Anc { own: flag, at, facts, verify })
}

/// `--r3-compare <a.json>,<b.json>`: paired b - a for every ranker the two runs' per-query dumps
/// (`--r3-perq on`) share; the runs must have scored the same queries.
fn r3_compare_main(args: &Args) {
    let p: Vec<&str> = args.r3_compare.split(',').collect();
    assert_eq!(p.len(), 2, "--r3-compare a.json,b.json");
    let load = |f: &str| -> serde_json::Value { serde_json::from_str(&std::fs::read_to_string(f).expect("read json")).expect("parse json") };
    let (a, b) = (load(p[0]), load(p[1]));
    let (pa, pb) = (&a["perq"], &b["perq"]);
    assert_eq!(pa["ids"], pb["ids"], "the two runs scored different queries");
    let commit_of: Vec<usize> = pa["commit_of"].as_array().expect("commit_of").iter().map(|x| x.as_u64().unwrap_or(0) as usize).collect();
    let nc = commit_of.iter().max().map_or(0, |m| m + 1);
    let lf = ln_fact_table(commit_of.len() + 1);
    let col = json_col;
    let mut rows = Vec::new();
    let mut md = "| ranker | a r@10 | b r@10 | b - a recall@10 | b - a MRR |\n|---|---|---|---|---|\n".to_string();
    if let Some(ra) = pa["r10"].as_object() {
        for (name, va) in ra {
            let (Some(vb), Some(ma), Some(mb)) = (pb["r10"].get(name), pa["mrr"].get(name), pb["mrr"].get(name)) else { continue };
            let (xa, xb, ya, yb) = (col(va), col(vb), col(ma), col(mb));
            let r = paired(&xb, &xa, &commit_of, nc, args.boot, &lf);
            let m = paired(&yb, &ya, &commit_of, nc, args.boot, &lf);
            let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
            md.push_str(&format!("| {name} | {:.3} | {:.3} | {:+.3} [{:+.3}, {:+.3}] | {:+.3} [{:+.3}, {:+.3}] |\n", mean(&xa), mean(&xb), r.mean, r.lo, r.hi, m.mean, m.lo, m.hi));
            rows.push(json!({"ranker": name, "a_r10": mean(&xa), "b_r10": mean(&xb), "r10": {"diff": r.mean, "lo": r.lo, "hi": r.hi},
                "a_mrr": mean(&ya), "b_mrr": mean(&yb), "mrr": {"diff": m.mean, "lo": m.lo, "hi": m.hi}}));
        }
    }
    let doc = json!({"a": p[0], "b": p[1], "queries": commit_of.len(), "test_commits": nc,
        "a_facts": a["round3"]["facts"], "b_facts": b["round3"]["facts"], "rows": rows});
    std::fs::write(&args.out, serde_json::to_string_pretty(&doc).unwrap()).expect("write compare json");
    std::fs::write(&args.md, md).expect("write compare md");
    eprintln!("[rank_eval] compare {} -> {} rows -> {} {}", args.r3_compare, rows.len(), args.out, args.md);
}

/// One per-query JSON column (`perq`'s r10 / mrr arrays) as f64s; a null (a NaN written by
/// serde_json) reads back as NaN.
fn json_col(v: &serde_json::Value) -> Vec<f64> {
    v.as_array().map(|x| x.iter().map(|y| y.as_f64().unwrap_or(f64::NAN)).collect()).unwrap_or_default()
}

// ============================================================================
// The rank gate (0.5.3, EG.2)
// ============================================================================

/// The shapes a gate may name, in gate order: `single` is the single-seed `perq`, the rest the
/// round-3 `perq_shapes`.
const GATE_SHAPES: [&str; 5] = ["single", "k2", "k3", "half", "find"];
/// The metrics a gate may name (`perq`'s two per-query columns).
const GATE_METRICS: [&str; 2] = ["r10", "mrr"];
/// The family-wise error rate of one check, per direction (James 2026-10-23, ruling EG-2:
/// Holm over the changed rows of one check, one-sided, family alpha 0.05).
const GATE_ALPHA: f64 = 0.05;

/// A parsed gate.json: its metrics and, in gate order, each shape's gated rankers.
struct Gate {
    metrics: Vec<String>,
    shapes: Vec<(String, Vec<String>)>,
}

fn gate_load(path: &str) -> Result<serde_json::Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("[rank-gate] read {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("[rank-gate] parse {path}: {e}"))
}

/// Reads gate.json: `{"gate":1, "metrics":[..], "rankers":{<shape>:[..]}, "items":{..}}`. Every
/// ranker an `items` array names must be gated in some shape (an `{"exempt": ..}` item names none).
fn gate_read(path: &str) -> Result<Gate, String> {
    let g = gate_load(path)?;
    if g["gate"].as_u64() != Some(1) {
        return Err(format!("[rank-gate] gate {path}: expected \"gate\":1"));
    }
    let mut metrics = Vec::new();
    for m in g["metrics"].as_array().ok_or_else(|| format!("[rank-gate] gate {path}: no metrics list"))? {
        match m.as_str() {
            Some(m) if GATE_METRICS.contains(&m) => metrics.push(m.to_string()),
            _ => return Err(format!("[rank-gate] gate {path}: unknown metric={m}")),
        }
    }
    let rankers = g["rankers"].as_object().ok_or_else(|| format!("[rank-gate] gate {path}: no rankers object"))?;
    if let Some(s) = rankers.keys().find(|k| !GATE_SHAPES.contains(&k.as_str())) {
        return Err(format!("[rank-gate] gate {path}: unknown shape={s}"));
    }
    let mut shapes = Vec::new();
    let mut gated = BTreeSet::new();
    for s in GATE_SHAPES {
        let Some(list) = rankers.get(s) else { continue };
        let list = list.as_array().ok_or_else(|| format!("[rank-gate] gate {path}: shape={s} is not a list"))?;
        let mut names = Vec::new();
        for r in list {
            let r = r.as_str().ok_or_else(|| format!("[rank-gate] gate {path}: shape={s} names a non-string ranker"))?;
            if !names.iter().any(|x: &String| x == r) {
                names.push(r.to_string());
            }
            gated.insert(r.to_string());
        }
        shapes.push((s.to_string(), names));
    }
    let mut ungated = Vec::new();
    if let Some(items) = g["items"].as_object() {
        for (item, v) in items {
            for r in v.as_array().map(Vec::as_slice).unwrap_or_default() {
                let r = r.as_str().unwrap_or_default();
                if !gated.contains(r) {
                    ungated.push(format!("[rank-gate] items name ungated ranker={r} item={item}"));
                }
            }
        }
    }
    if !ungated.is_empty() {
        return Err(ungated.join("\n"));
    }
    Ok(Gate { metrics, shapes })
}

/// A run's (or a baseline's) per-query block for one gate shape.
fn gate_run_shape<'a>(run: &'a serde_json::Value, shape: &str) -> Option<&'a serde_json::Value> {
    let v = if shape == "single" { &run["perq"] } else { &run["perq_shapes"][shape] };
    v.as_object().map(|_| v)
}

/// `--gate-trim <gate.json> --gate-new <run.json> --out <baseline.json>`: the run reduced to the
/// gate's rankers, per shape `ids`, `commit_of` and each metric's arrays (the run's own f64s,
/// never rounded), plus `label` / `rev` from `stats` and `glia_head` (env `GLIA_RANK_GATE_HEAD`).
/// Compact JSON over BTreeMaps, so one run always trims to the same bytes.
fn gate_trim_main(args: &Args) -> i32 {
    let gate = match gate_read(&args.gate_trim) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    if args.gate_new.is_empty() {
        eprintln!("[rank-gate] --gate-trim needs --gate-new <run.json>");
        return 2;
    }
    let run = match gate_load(&args.gate_new) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let label = run["stats"]["label"].as_str().unwrap_or_default().to_string();
    let mut code = 0;
    let mut shapes: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    let mut kept = 0usize;
    for (shape, rankers) in &gate.shapes {
        let Some(s) = gate_run_shape(&run, shape) else {
            eprintln!("[rank-gate] missing shape={shape} label={label}");
            code = 2;
            continue;
        };
        let mut out: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        out.insert("ids".into(), s["ids"].clone());
        out.insert("commit_of".into(), s["commit_of"].clone());
        for m in &gate.metrics {
            let mut cols: BTreeMap<String, serde_json::Value> = BTreeMap::new();
            for r in rankers {
                match s[m.as_str()].get(r) {
                    Some(v) if v.is_array() => {
                        cols.insert(r.clone(), v.clone());
                    }
                    _ => {
                        if m == &gate.metrics[0] {
                            eprintln!("[rank-gate] missing ranker={r} shape={shape} label={label}");
                        }
                        code = 2;
                    }
                }
            }
            out.insert(m.clone(), json!(cols));
        }
        kept += rankers.len();
        shapes.insert(shape.clone(), json!(out));
    }
    if code != 0 {
        return code;
    }
    let mut doc: BTreeMap<String, serde_json::Value> = BTreeMap::new();
    doc.insert("label".into(), json!(label));
    doc.insert("rev".into(), run["stats"]["rev"].clone());
    doc.insert("glia_head".into(), json!(std::env::var("GLIA_RANK_GATE_HEAD").unwrap_or_default()));
    doc.insert("shapes".into(), json!(shapes));
    let text = match serde_json::to_string(&doc) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[rank-gate] serialise {}: {e}", args.out);
            return 2;
        }
    };
    if let Err(e) = std::fs::write(&args.out, text) {
        eprintln!("[rank-gate] write {}: {e}", args.out);
        return 2;
    }
    eprintln!("[rank-gate] trim label={label} shapes={} rankers={kept} -> {}", shapes.len(), args.out);
    0
}

/// One judged row of a check.
struct GateRow {
    shape: String,
    ranker: String,
    metric: String,
    base: f64,
    new: f64,
    p: Paired,
    /// Some row value differs (bit for bit, NaN equal to NaN): only these rows form Holm's family.
    changed: bool,
    /// One-sided bootstrap p-values: the share of bootstrap means >= 0 (drop) / <= 0 (gain).
    p_drop: f64,
    p_gain: f64,
    verdict: &'static str,
}

/// Holm's step-down over `(row, p)`: the rows rejected at family-wise `alpha` (ties by row index).
fn holm(mut ps: Vec<(usize, f64)>, alpha: f64) -> BTreeSet<usize> {
    ps.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    let m = ps.len();
    let mut out = BTreeSet::new();
    for (i, (row, p)) in ps.into_iter().enumerate() {
        if p > alpha / (m - i) as f64 {
            break;
        }
        out.insert(row);
    }
    out
}

/// `--gate-check <gate.json> --gate-base <baseline.json> --gate-new <run.json> --out <v.json>
/// --md <v.md>`: per gated (shape, ranker, metric), `paired(new, base)` over the shape's queries
/// with the commit-cluster bootstrap (B = `--boot`), so diff = new - base and the CI its 2.5 /
/// 97.5 percentiles. The verdict follows ruling EG-2 (James 2026-10-23): the rows whose values
/// changed form one family per direction, each row's one-sided p is the share of its bootstrap
/// means on the wrong side of 0 (>= 0 for a drop, <= 0 for a gain), and Holm at family alpha
/// 0.05 decides `drop` / `gain`; every other row is `same` (an unchanged row diffs exactly 0).
/// A shape whose ids or commit clusters moved is drift (exit 2, only a declared re-baseline fixes
/// it); a gated ranker missing from the run exits 2; one in the run but not the baseline is
/// `new`, unjudged. Exit 1 on any drop when nothing else failed, else 0.
fn gate_check_main(args: &Args) -> i32 {
    let loaded = gate_read(&args.gate_check).and_then(|g| {
        if args.gate_base.is_empty() || args.gate_new.is_empty() {
            return Err("[rank-gate] --gate-check needs --gate-base <baseline.json> and --gate-new <run.json>".to_string());
        }
        Ok((g, gate_load(&args.gate_base)?, gate_load(&args.gate_new)?))
    });
    let (gate, base, run) = match loaded {
        Ok(x) => x,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let label = base["label"].as_str().or_else(|| run["stats"]["label"].as_str()).unwrap_or_default().to_string();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
    let mut rows: Vec<GateRow> = Vec::new();
    let (mut missing, mut drift, mut fresh) = (Vec::new(), Vec::new(), Vec::new());
    for (shape, rankers) in &gate.shapes {
        let Some(n) = gate_run_shape(&run, shape) else {
            eprintln!("[rank-gate] missing shape={shape} label={label}");
            missing.push(format!("{shape}:*"));
            continue;
        };
        let present: Vec<&String> = rankers
            .iter()
            .filter(|r| {
                let ok = gate.metrics.iter().all(|m| n[m.as_str()].get(r.as_str()).is_some_and(serde_json::Value::is_array));
                if !ok {
                    eprintln!("[rank-gate] missing ranker={r} shape={shape} label={label}");
                    missing.push(format!("{shape}:{r}"));
                }
                ok
            })
            .collect();
        let b = &base["shapes"][shape.as_str()];
        let (ids_b, ids_n) = (b["ids"].as_array(), n["ids"].as_array());
        if ids_b.is_none() || ids_b != ids_n || b["commit_of"] != n["commit_of"] {
            let nb: Vec<&serde_json::Value> = ids_b.map(|v| v.iter().collect()).unwrap_or_default();
            let nn: BTreeSet<String> = ids_n.map(|v| v.iter().map(|x| x.to_string()).collect()).unwrap_or_default();
            let common = nb.iter().filter(|x| nn.contains(&x.to_string())).count();
            eprintln!(
                "[rank-gate] drift label={label} shape={shape} base_queries={} new_queries={} common={common}",
                nb.len(),
                ids_n.map_or(0, Vec::len)
            );
            drift.push(shape.clone());
            continue;
        }
        let commit_of: Vec<usize> = b["commit_of"].as_array().map(|v| v.iter().map(|x| x.as_u64().unwrap_or(0) as usize).collect()).unwrap_or_default();
        let nq = commit_of.len();
        let nc = commit_of.iter().max().map_or(0, |m| m + 1);
        let lf = ln_fact_table(nq + 1);
        for r in present {
            let in_base = gate.metrics.iter().all(|m| b[m.as_str()].get(r.as_str()).is_some_and(serde_json::Value::is_array));
            if !in_base {
                let nm = |m: &str| n[m].get(r.as_str()).map(json_col).map_or(f64::NAN, |v| mean(&v));
                eprintln!("[rank-gate] new label={label} shape={shape} ranker={r} new_r10={:.4} new_mrr={:.4}", nm("r10"), nm("mrr"));
                fresh.push(format!("{shape}:{r}"));
                continue;
            }
            for m in &gate.metrics {
                let (xa, xb) = (json_col(&b[m.as_str()][r.as_str()]), json_col(&n[m.as_str()][r.as_str()]));
                if xa.len() != nq || xb.len() != nq || nq == 0 {
                    eprintln!("[rank-gate] drift label={label} shape={shape} ranker={r} metric={m} base_values={} new_values={} queries={nq}", xa.len(), xb.len());
                    drift.push(format!("{shape}:{r}:{m}"));
                    continue;
                }
                let (p, means) = paired_boot(&xb, &xa, &commit_of, nc, args.boot, &lf);
                let share = |f: &dyn Fn(f64) -> bool| means.iter().filter(|&&x| f(x)).count() as f64 / means.len() as f64;
                let changed = xa.iter().zip(&xb).any(|(x, y)| !(x == y || (x.is_nan() && y.is_nan())));
                rows.push(GateRow {
                    shape: shape.clone(),
                    ranker: r.clone(),
                    metric: m.clone(),
                    base: mean(&xa),
                    new: mean(&xb),
                    changed,
                    p_drop: share(&|x| x >= 0.0),
                    p_gain: share(&|x| x <= 0.0),
                    p,
                    verdict: "same",
                });
            }
        }
    }
    let changed: Vec<usize> = (0..rows.len()).filter(|&i| rows[i].changed).collect();
    let drops = holm(changed.iter().map(|&i| (i, rows[i].p_drop)).collect(), GATE_ALPHA);
    let gains = holm(changed.iter().map(|&i| (i, rows[i].p_gain)).collect(), GATE_ALPHA);
    for (i, row) in rows.iter_mut().enumerate() {
        row.verdict = if drops.contains(&i) {
            "drop"
        } else if gains.contains(&i) {
            "gain"
        } else {
            "same"
        };
    }
    let mut json_rows: BTreeMap<String, BTreeMap<String, BTreeMap<String, serde_json::Value>>> = BTreeMap::new();
    let mut md = format!("## rank gate: {label}\n\nnew - base, commit-cluster bootstrap 95% CI (B = {}); Holm over the {} changed rows, one-sided, family alpha {GATE_ALPHA}.\n", args.boot, changed.len());
    let mut md_shape = String::new();
    for row in &rows {
        eprintln!(
            "[rank-gate] label={label} shape={} ranker={} metric={} base={:.4} new={:.4} diff={:+.4} ci=[{:+.4}, {:+.4}] verdict={} p_drop={:.4} p_gain={:.4} changed={}",
            row.shape, row.ranker, row.metric, row.base, row.new, row.p.mean, row.p.lo, row.p.hi, row.verdict, row.p_drop, row.p_gain, u8::from(row.changed)
        );
        if md_shape != row.shape {
            md_shape = row.shape.clone();
            md.push_str(&format!("\n### {}\n\n| ranker | metric | base | new | diff [CI] | verdict |\n|---|---|---|---|---|---|\n", row.shape));
        }
        md.push_str(&format!(
            "| {} | {} | {:.4} | {:.4} | {:+.4} [{:+.4}, {:+.4}] | {} |\n",
            row.ranker, row.metric, row.base, row.new, row.p.mean, row.p.lo, row.p.hi, row.verdict
        ));
        json_rows.entry(row.shape.clone()).or_default().entry(row.ranker.clone()).or_default().insert(
            row.metric.clone(),
            json!({"base": row.base, "new": row.new, "diff": row.p.mean, "lo": row.p.lo, "hi": row.p.hi, "changed": row.changed,
                "p_drop": row.p_drop, "p_gain": row.p_gain, "wins": row.p.wins, "losses": row.p.losses, "verdict": row.verdict}),
        );
    }
    let n_drop = rows.iter().filter(|r| r.verdict == "drop").count();
    let n_gain = rows.iter().filter(|r| r.verdict == "gain").count();
    let doc = json!({"label": label, "base_head": base["glia_head"], "base_rev": base["rev"], "new_rev": run["stats"]["rev"],
        "boot": args.boot, "alpha": GATE_ALPHA, "correction": "holm", "rows": json_rows, "changed": changed.len(),
        "drops": n_drop, "gains": n_gain, "new": fresh, "drift": drift, "missing": missing});
    let wrote = serde_json::to_string_pretty(&doc)
        .map_err(|e| e.to_string())
        .and_then(|t| std::fs::write(&args.out, t).map_err(|e| e.to_string()))
        .and_then(|()| std::fs::write(&args.md, &md).map_err(|e| e.to_string()));
    if let Err(e) = wrote {
        eprintln!("[rank-gate] write {} / {}: {e}", args.out, args.md);
        return 2;
    }
    eprintln!(
        "[rank-gate] verdict label={label} rows={} drops={n_drop} gains={n_gain} new={} drift={} missing={} changed={} correction=holm alpha={GATE_ALPHA}",
        rows.len(),
        fresh.len(),
        drift.len(),
        missing.len(),
        changed.len()
    );
    if !missing.is_empty() || !drift.is_empty() {
        2
    } else if n_drop > 0 {
        1
    } else {
        0
    }
}
