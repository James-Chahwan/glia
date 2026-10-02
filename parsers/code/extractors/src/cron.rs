//! Cron extraction (v0.4.x — task #7; framework schedulers LA.19a).
//!
//! Emits `CRON_JOB` nodes for every scheduled invocation we can see in the
//! repo. In-repo sources (path/content gated):
//!
//!   1. GitHub Actions — `.github/workflows/*.yml` with `on.schedule.cron`
//!   2. k8s CronJob YAML — file containing `kind: CronJob` + `schedule:`
//!   3. node-cron / cron — `cron.schedule('* * * * *', handler)`
//!   4. Celery beat — entries inside an `app.conf.beat_schedule = { ... }`
//!      dict, looking for `'schedule': crontab(...)` or `'schedule': N.0`
//!   5. Spring — `@Scheduled(cron = "...")`
//!   6. Quartz — `cronSchedule("...")` / `new CronExpression("...")` (LA.19a)
//!   7. Hangfire — `RecurringJob.AddOrUpdate(.., () => X.M(), "..." | Cron.X)`
//!   8. Go — robfig `c.AddFunc("..", fn)` / `c.AddJob`, gocron `.Cron("..").Do(fn)`
//!   9. APScheduler — `@s.scheduled_job("cron", **kw)`, `s.add_job(f, ..)`
//!  10. whenever — `every 1.day, at: '4:30 am' do runner "X.y" end` (LA.19b)
//!  11. sidekiq-cron / sidekiq-scheduler — a schedule YAML's `cron:` + `class:`
//!      entries, and `Sidekiq::Cron::Job.create(..)` in Ruby (LA.19b)
//!  12. Laravel — `$schedule->job(new X)->everyFiveMinutes()` (LA.19b)
//!  13. Oban — `{Oban.Plugins.Cron, crontab: [{"@daily", MyApp.Worker}]}` (LA.19b)
//!  14. NestJS — `@Cron('0 3 * * *')` / `@Cron(CronExpression.X)` /
//!      `@Interval(ms)` from `@nestjs/schedule` on a provider method (CL.9)
//!
//! Sources 5–14 are CODE sources: a job whose handler is nameable also
//! carries a `CRON_JOB --HANDLED_BY--> handler` [`UnresolvedRef`] (bound by the
//! graph builder's `resolve_refs`), so trace / blast-radius walk from a job
//! into the code it runs. YAML jobs (sidekiq's included) carry none: they live
//! in the synthetic yaml graph, where a handler in another language's graph
//! can never resolve. Nor does a job named only by a command string (Laravel
//! `->command('emails:send')`, whenever `rake` / `command`): no resolver
//! indexes CLI_COMMANDs by that string.
//!
//! Out of scope:
//!   - Server-side `crontab -e` entries that aren't committed
//!   - UI-configured cloud schedulers (GCP Scheduler / EventBridge)
//!   - systemd `*.timer`
//!   - Dockerfile CMD bridging to external scheduler — IaC resolver (#9) closes
//!     this gap by linking image → k8s CronJob via Resource nodes.
//!
//! Qname shape: `cron:<schedule>:<target_id>`. `<schedule>` is the verbatim
//! cron expression (or a normalised rate marker). `<target_id>` is the script
//! basename / handler symbol when extractable, else `anon`. The full qname is
//! the join key for `CronResolver` — drift detection rather than schedule
//! overlap. The code sources run their schedule through [`normalise_schedule`]
//! (descriptors such as `@daily` become their 5-field expansion) so a robfig
//! `@hourly` job and a manifest's `0 * * * *` share one identity.

use glia_code_domain::{
    CallQualifier, CodeNav, GRAPH_TYPE, UnresolvedRef, edge_category, line_of, node_kind,
};
use glia_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

use crate::code_guard::LazyGuard;

pub struct CronNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
    /// LA.19a: `CRON_JOB --HANDLED_BY--> handler` refs for code-sourced jobs.
    pub refs: Vec<UnresolvedRef>,
}

#[derive(Debug, Clone)]
struct CronJob {
    schedule: String,
    target: String,
    /// workflow / k8s / node-cron / celery / scheduled-annot / quartz /
    /// hangfire / robfig / gocron / apscheduler / whenever / sidekiq_cron /
    /// laravel / oban / nestjs
    source: &'static str,
    /// The code the job runs, `Bare(fn)` or `Attribute { base, name }`, with
    /// the 0-based row of the construct that registers it (the scheduling
    /// call, annotation or decorator; the job line of a whenever block): the
    /// HANDLED_BY ref's site line (LC.3b).
    handler: Option<(CallQualifier, u32)>,
}

/// `handler` placed at the row of byte `at` of `source` (LC.3b).
fn at_row(handler: Option<CallQualifier>, source: &str, at: usize) -> Option<(CallQualifier, u32)> {
    handler.map(|h| (h, line_of(source, at)))
}

/// The byte offset of `part` in `source` when `part` is a sub-slice of it,
/// else `fallback`. Scanners hand back sub-slices; the offset gives their row.
fn offset_in(source: &str, part: &str, fallback: usize) -> usize {
    (part.as_ptr() as usize)
        .checked_sub(source.as_ptr() as usize)
        .filter(|o| *o <= source.len())
        .unwrap_or(fallback)
}

/// Per-file tally of code-sourced jobs, for the LA.19a `[cron] code` and the
/// LA.19b `[cron] script` fired_on markers.
#[derive(Debug, Default, PartialEq)]
struct CodeCounts {
    quartz: usize,
    hangfire: usize,
    go: usize,
    apscheduler: usize,
    spring: usize,
    nestjs: usize,
    whenever: usize,
    sidekiq: usize,
    laravel: usize,
    oban: usize,
}

impl CodeCounts {
    fn bump(&mut self, source: &str) {
        match source {
            "quartz" => self.quartz += 1,
            "hangfire" => self.hangfire += 1,
            "robfig" | "gocron" => self.go += 1,
            "apscheduler" => self.apscheduler += 1,
            "scheduled_annot" => self.spring += 1,
            "nestjs" => self.nestjs += 1,
            "whenever" => self.whenever += 1,
            "sidekiq_cron" => self.sidekiq += 1,
            "laravel" => self.laravel += 1,
            "oban" => self.oban += 1,
            _ => {}
        }
    }

    fn total(&self) -> usize {
        self.quartz + self.hangfire + self.go + self.apscheduler + self.spring + self.nestjs
    }

    fn script_total(&self) -> usize {
        self.whenever + self.sidekiq + self.laravel + self.oban
    }
}

/// The LA.19b sources, which the `[cron] script` marker counts.
fn is_script_source(source: &str) -> bool {
    matches!(source, "whenever" | "sidekiq_cron" | "laravel" | "oban")
}

/// LA.19a fired_on: `[cron] code jobs=1 quartz=1 hangfire=0 go=0 apscheduler=0
/// spring=0 nestjs=0 handler_refs=1 path=Jobs.java`, or `None` when the file
/// declared no code-sourced job. The prefix and field order are stable;
/// `nestjs=` (CL.9) follows `spring=`.
fn code_marker(c: &CodeCounts, handler_refs: usize, path: &str) -> Option<String> {
    (c.total() > 0).then(|| {
        format!(
            "[cron] code jobs={} quartz={} hangfire={} go={} apscheduler={} spring={} nestjs={} handler_refs={handler_refs} path={path}",
            c.total(),
            c.quartz,
            c.hangfire,
            c.go,
            c.apscheduler,
            c.spring,
            c.nestjs,
        )
    })
}

/// LA.19b fired_on: `[cron] script jobs=3 whenever=0 sidekiq=0 laravel=3 oban=0
/// handler_refs=1 path=app/Console/Kernel.php`, or `None` when the file
/// declared no whenever / sidekiq-cron / Laravel / Oban job. `handler_refs`
/// counts only the refs of those jobs. The prefix and field order are stable.
fn script_marker(c: &CodeCounts, handler_refs: usize, path: &str) -> Option<String> {
    (c.script_total() > 0).then(|| {
        format!(
            "[cron] script jobs={} whenever={} sidekiq={} laravel={} oban={} handler_refs={handler_refs} path={path}",
            c.script_total(),
            c.whenever,
            c.sidekiq,
            c.laravel,
            c.oban,
        )
    })
}

pub fn extract_cron_nodes(
    source: &str,
    path: &str,
    module_id: NodeId,
    repo: RepoId,
) -> CronNodes {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut nav = CodeNav::default();
    let mut refs: Vec<UnresolvedRef> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut jobs: Vec<CronJob> = Vec::new();

    if is_github_actions_workflow(path) {
        jobs.extend(extract_github_actions(source, path));
    }
    if looks_like_k8s_cronjob(source) {
        let scan = extract_k8s_cronjob(source);
        // fired_on marker — only when at least one document really declared
        // `kind: CronJob`, so it stays quiet on unrelated YAML that merely
        // mentions the word.
        if scan.cronjob_docs > 0 {
            eprintln!("[cron] k8s docs={} jobs={}", scan.docs, scan.jobs.len());
        }
        jobs.extend(scan.jobs);
    }
    // CJ.1c: in a Rust or Python file a node-cron / `@Scheduled(` needle in
    // a string literal or comment is no job, and a Celery beat key counts
    // only when it opens its own literal (code_guard.rs).
    let mut guard = LazyGuard::new(path, source);
    jobs.extend(extract_node_cron(source, &mut guard));
    jobs.extend(extract_celery_beat(source, &mut guard));
    jobs.extend(extract_scheduled_annotation(source, &mut guard));
    guard.report("cron");
    // LA.19a framework schedulers. Each is gated on the file's language AND
    // its library's import / namespace, so every other file costs one
    // extension test.
    let ext = path.rsplit_once('.').map_or("", |(_, e)| e);
    if matches!(ext, "java" | "kt" | "kts" | "groovy" | "scala") && source.contains("org.quartz") {
        jobs.extend(extract_quartz(source));
    }
    if matches!(ext, "cs" | "vb" | "fs") && source.contains("Hangfire") {
        jobs.extend(extract_hangfire(source));
    }
    if ext == "go" {
        let robfig = source.contains("robfig/cron");
        let gocron = source.contains("go-co-op/gocron");
        if robfig || gocron {
            jobs.extend(extract_go_cron(source, robfig, gocron));
        }
    }
    if matches!(ext, "py" | "pyw") && source.contains("apscheduler") {
        jobs.extend(extract_apscheduler(source));
    }
    if matches!(ext, "ts" | "tsx" | "js" | "mts" | "cts") && source.contains("@nestjs/schedule") {
        jobs.extend(extract_nest_schedule(source));
    }
    // LA.19b script-language schedulers, gated the same way: extension first,
    // then the library's own spelling.
    jobs.extend(extract_script_schedulers(source, path, ext));

    let mut counts = CodeCounts::default();
    let (mut code_refs, mut script_refs) = (0usize, 0usize);
    for job in jobs {
        let qname = format!("cron:{}:{}", job.schedule, job.target);
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CRON_JOB, &qname);
        if seen.insert(qname.clone()) {
            let payload = format!(
                r#"{{"schedule":"{}","target":"{}","source":"{}"}}"#,
                escape_json(&job.schedule),
                escape_json(&job.target),
                job.source,
            );
            nodes.push(Node {
                id,
                repo,
                confidence: Confidence::Medium,
                cells: vec![Cell {
                    kind: glia_code_domain::cell_type::CODE,
                    payload: CellPayload::Json(payload),
                }],
            });
            nav.record(
                id,
                &job.schedule,
                &qname,
                node_kind::CRON_JOB,
                Some(module_id),
            );
            // Edge from the file's module → the cron job — lets a graph query
            // find every job a service registers without a separate index.
            edges.push(Edge {
                from: module_id,
                to: id,
                category: edge_category::SCHEDULES,
                confidence: Confidence::Medium,
                cells: Vec::new(),
            });
            counts.bump(job.source);
        }
        // A duplicate qname is one node, but a second handler spelling for it
        // is still a distinct ref.
        if let Some((handler, line)) = job.handler
            && !refs.iter().any(|r| r.from == id && r.qualifier == handler)
        {
            refs.push(UnresolvedRef {
                from: id,
                from_module: module_id,
                qualifier: handler,
                category: edge_category::HANDLED_BY,
                line,
            });
            if is_script_source(job.source) {
                script_refs += 1;
            } else {
                code_refs += 1;
            }
        }
    }
    if let Some(marker) = code_marker(&counts, code_refs, path) {
        eprintln!("{marker}");
    }
    if let Some(marker) = script_marker(&counts, script_refs, path) {
        eprintln!("{marker}");
    }

    CronNodes {
        nodes,
        edges,
        nav,
        refs,
    }
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ----------------------------------------------------------------------------
// GitHub Actions: `.github/workflows/*.yml` with `on: schedule: - cron: '...'`
// ----------------------------------------------------------------------------

fn is_github_actions_workflow(path: &str) -> bool {
    let norm = path.replace('\\', "/");
    norm.contains(".github/workflows/") && (norm.ends_with(".yml") || norm.ends_with(".yaml"))
}

fn extract_github_actions(source: &str, path: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    // Workflow target = file basename (stripped of extension), since GHA jobs
    // are named per workflow file.
    let target = workflow_target_from_path(path);
    for line in source.lines() {
        let t = line.trim();
        // Match `cron: '* * * * *'` and `cron: "..."`. Indented under `schedule:`
        // — we don't validate the parent key (cheap; false-positives elsewhere
        // require the literal `cron:` key which is rare outside this context).
        if let Some(rest) = t.strip_prefix("- cron:").or_else(|| t.strip_prefix("cron:")) {
            if let Some(schedule) = first_yaml_string(rest) {
                if looks_like_cron_expr(&schedule) {
                    out.push(CronJob {
                        schedule,
                        target: target.clone(),
                        source: "github_actions",
                        handler: None,
                    });
                }
            }
        }
    }
    out
}

fn workflow_target_from_path(path: &str) -> String {
    let norm = path.replace('\\', "/");
    let base = norm.rsplit('/').next().unwrap_or(&norm);
    let stripped = base
        .strip_suffix(".yml")
        .or_else(|| base.strip_suffix(".yaml"))
        .unwrap_or(base);
    stripped.to_string()
}

// ----------------------------------------------------------------------------
// k8s CronJob. A manifest file is a stream of `---`-separated documents, so
// every document is scanned on its own: a whole-file line walk would let a
// later document's `schedule:` / `command:` overwrite an earlier CronJob's
// and emit one node pairing fields that never belonged together. Mirrors the
// document split `iac::extract_k8s_documents` already does on the same files.
// Pull `schedule:` and a target hint from the first container's `command:` /
// `args:` / `image:` if present.
// ----------------------------------------------------------------------------

/// Cheap whole-file pre-gate, deliberately wider than the per-document check:
/// case-insensitive so `kind: "CronJob"`, `kind:  CronJob` and the like still
/// reach `doc_is_cronjob`. A false hit costs one `split` and is then filtered
/// out document by document.
fn looks_like_k8s_cronjob(source: &str) -> bool {
    source
        .as_bytes()
        .windows(b"cronjob".len())
        .any(|w| w.eq_ignore_ascii_case(b"cronjob"))
}

/// Per-document kind gate, mirroring `iac::read_k8s_kind`: the first `kind:`
/// line in the document decides, tolerating extra whitespace and quoting.
/// Deciding on the FIRST `kind:` (rather than the first one that says
/// `CronJob`) is what keeps a nested `kind:` inside a Deployment from
/// promoting that document to a CronJob.
fn doc_is_cronjob(doc: &str) -> bool {
    for line in doc.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('-') {
            continue;
        }
        if let Some(rest) = t.strip_prefix("kind:") {
            return rest.trim().trim_matches(|c| c == '"' || c == '\'') == "CronJob";
        }
    }
    false
}

/// What one file's k8s scan found. `docs` / `cronjob_docs` exist for the
/// fired_on marker; `jobs` is the extraction result.
struct K8sScan {
    docs: usize,
    cronjob_docs: usize,
    jobs: Vec<CronJob>,
}

fn extract_k8s_cronjob(source: &str) -> K8sScan {
    let mut scan = K8sScan {
        docs: 0,
        cronjob_docs: 0,
        jobs: Vec::new(),
    };
    for doc in source.split("\n---") {
        let doc = doc.trim_start_matches('\n');
        if doc.is_empty() {
            continue;
        }
        scan.docs += 1;
        if !doc_is_cronjob(doc) {
            continue;
        }
        scan.cronjob_docs += 1;
        if let Some(job) = extract_k8s_cronjob_doc(doc) {
            scan.jobs.push(job);
        }
    }
    scan
}

/// One document in, at most one `CronJob` out.
fn extract_k8s_cronjob_doc(doc: &str) -> Option<CronJob> {
    let mut current_schedule: Option<String> = None;
    let mut current_image: Option<String> = None;
    let mut current_command: Option<String> = None;
    for line in doc.lines() {
        // Strip both leading whitespace and a YAML list marker (`- `). k8s
        // container blocks live inside a list, so `- image: foo` and `- name:
        // bar` both arrive trimmed-but-prefixed.
        let t = line.trim();
        let t = t.strip_prefix("- ").unwrap_or(t);
        if let Some(rest) = t.strip_prefix("schedule:") {
            if let Some(s) = first_yaml_string(rest) {
                if looks_like_cron_expr(&s) {
                    current_schedule = Some(s);
                }
            }
        }
        if let Some(rest) = t.strip_prefix("image:") {
            if let Some(img) = first_yaml_string(rest) {
                current_image = Some(image_basename(&img));
            }
        }
        // `command: ['/usr/bin/x']` and `command: [/usr/bin/x]` and
        // `args: ["--once"]` — pick the first list element as a hint.
        if let Some(rest) = t.strip_prefix("command:") {
            if let Some(cmd) = first_list_element(rest) {
                current_command = Some(basename(&cmd));
            }
        }
    }
    let schedule = current_schedule?;
    let target = current_command
        .or(current_image)
        .unwrap_or_else(|| "anon".to_string());
    Some(CronJob {
        schedule,
        target,
        source: "k8s_cronjob",
        handler: None,
    })
}

fn image_basename(image: &str) -> String {
    // `repo/path/img:tag` → `img`.
    let no_tag = image.split(':').next().unwrap_or(image);
    no_tag.rsplit('/').next().unwrap_or(no_tag).to_string()
}

fn basename(path: &str) -> String {
    let p = path.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    p.rsplit('/').next().unwrap_or(p).to_string()
}

// ----------------------------------------------------------------------------
// node-cron / `cron` lib: `cron.schedule('* * * * *', handler)` and the class
// form `new CronJob({ cronTime: '...', onTick: handler })`.
// ----------------------------------------------------------------------------

fn extract_node_cron(source: &str, guard: &mut LazyGuard<'_>) -> Vec<CronJob> {
    let mut out = Vec::new();
    // Method form: `cron.schedule('...', handler)`. Handler may be an
    // identifier, an arrow function, or a method reference.
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("cron.schedule(") {
        let pos = search_from + rel;
        search_from = pos + "cron.schedule(".len();
        if !guard.admits(pos) {
            continue;
        }
        let after = &source[pos + "cron.schedule(".len()..];
        if let Some(schedule) = first_quoted(after) {
            if looks_like_cron_expr(&schedule) {
                let target = handler_after_first_arg(after).unwrap_or_else(|| "anon".to_string());
                out.push(CronJob {
                    schedule,
                    target,
                    source: "node_cron",
                    handler: None,
                });
            }
        }
    }
    // Class form: `new CronJob({ cronTime: '...', onTick: handler })`
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("cronTime:") {
        let pos = search_from + rel;
        search_from = pos + "cronTime:".len();
        if !guard.admits(pos) {
            continue;
        }
        let after = &source[pos + "cronTime:".len()..];
        if let Some(schedule) = first_quoted(after) {
            if looks_like_cron_expr(&schedule) {
                // Look for `onTick:` within the next ~256 bytes for the target,
                // snapped DOWN so a multibyte char on the cut can't panic.
                let win_end = source.floor_char_boundary((pos + 256).min(source.len()));
                let win = &source[pos..win_end];
                let target = if let Some(tick) = win.find("onTick:") {
                    let after_tick = &win[tick + "onTick:".len()..];
                    handler_identifier(after_tick).unwrap_or_else(|| "anon".to_string())
                } else {
                    "anon".to_string()
                };
                out.push(CronJob {
                    schedule,
                    target,
                    source: "node_cron",
                    handler: None,
                });
            }
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Celery beat: `'schedule': crontab(minute=..., hour=...)` or
// `'schedule': 30.0` / `'schedule': timedelta(...)`. We capture the schedule
// expression verbatim from the source (post-colon, pre-comma) and try to
// pull a sibling `'task':` for the target.
//
// CJ.1c: the needle is itself a quoted dict key, so in a Rust or Python file
// it counts only when the literal holding it opens AT it
// ([`LazyGuard::admits_key`]): a key inside a larger string or a comment (a
// scanner's own test literal, a doc line) is no entry.
// ----------------------------------------------------------------------------

fn extract_celery_beat(source: &str, guard: &mut LazyGuard<'_>) -> Vec<CronJob> {
    let mut out = Vec::new();
    // Single-quoted keys first, then the double-quoted variant.
    for needle in ["'schedule':", "\"schedule\":"] {
        let mut search_from = 0;
        while let Some(rel) = source[search_from..].find(needle) {
            let pos = search_from + rel;
            search_from = pos + needle.len();
            if !guard.admits_key(pos) {
                continue;
            }
            let after = &source[pos + needle.len()..];
            let schedule = celery_schedule_expr(after);
            if !schedule.is_empty() && schedule.len() < 256 {
                // Walk back ~256 bytes to find the sibling `'task':` value;
                // the start snaps UP so a multibyte char on the cut can't
                // panic.
                let look_back_start = source.ceil_char_boundary(pos.saturating_sub(256));
                let context = &source[look_back_start..pos];
                let target = celery_task_in(context).unwrap_or_else(|| "anon".to_string());
                out.push(CronJob {
                    schedule,
                    target,
                    source: "celery_beat",
                    handler: None,
                });
            }
        }
    }
    out
}

/// Read a Celery beat schedule expression — everything between the colon and
/// the next top-level comma or closing `}`, trimmed.
fn celery_schedule_expr(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let start = bytes
        .iter()
        .position(|&b| !b.is_ascii_whitespace())
        .unwrap_or(0);
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            b',' | b'\n' if depth == 0 => break,
            _ => {}
        }
        i += 1;
    }
    s[start..i].trim().to_string()
}

fn celery_task_in(context: &str) -> Option<String> {
    for needle in ["'task':", "\"task\":"] {
        if let Some(idx) = context.rfind(needle) {
            let after = &context[idx + needle.len()..];
            if let Some(name) = first_quoted(after) {
                return Some(name);
            }
        }
    }
    None
}

// ----------------------------------------------------------------------------
// Java/Spring: `@Scheduled(cron = "0 4 * * * *")`
// ----------------------------------------------------------------------------

fn extract_scheduled_annotation(source: &str, guard: &mut LazyGuard<'_>) -> Vec<CronJob> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("@Scheduled(") {
        let pos = search_from + rel;
        let after_paren = pos + "@Scheduled(".len();
        // CJ.1c: an annotation in a Rust / Python literal or comment is no
        // job; the scan resumes inside it, so a real one after is still read.
        if !guard.admits(pos) {
            search_from = after_paren;
            continue;
        }
        // Find the closing paren bounds for this annotation.
        let mut j = after_paren;
        let mut depth = 1i32;
        let bytes = source.as_bytes();
        while j < bytes.len() && depth > 0 {
            match bytes[j] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                b'"' => {
                    j += 1;
                    while j < bytes.len() && bytes[j] != b'"' {
                        if bytes[j] == b'\\' && j + 1 < bytes.len() {
                            j += 2;
                        } else {
                            j += 1;
                        }
                    }
                }
                _ => {}
            }
            if depth > 0 {
                j += 1;
            }
        }
        let body = &source[after_paren..j.min(source.len())];
        if let Some(idx) = body.find("cron") {
            let tail = &body[idx + "cron".len()..];
            // Permit `cron = "..."` and `cron="..."` alike.
            if let Some(eq) = tail.find('=') {
                let after_eq = &tail[eq + 1..];
                if let Some(schedule) = first_quoted(after_eq) {
                    if looks_like_cron_expr(&schedule) {
                        // `j` sits on the annotation's `)`.
                        let method =
                            method_name_after_annotation(source.get(j + 1..).unwrap_or_default());
                        // LA.19a: the annotated method, scoped by the class
                        // the annotation sits in, is the job's handler.
                        let handler = method.as_ref().and_then(|m| {
                            let class = enclosing_class_name(source.get(..pos)?)?;
                            Some(CallQualifier::Attribute {
                                base: class,
                                name: m.clone(),
                            })
                        });
                        out.push(CronJob {
                            schedule,
                            target: method.unwrap_or_else(|| "anon".to_string()),
                            source: "scheduled_annot",
                            handler: at_row(handler, source, pos),
                        });
                    }
                }
            }
        }
        search_from = (j + 1).min(source.len());
    }
    out
}

/// After the closing `)` of a scheduling annotation / decorator (Spring
/// `@Scheduled(..)`, NestJS `@Cron(..)` / `@Interval(..)`), find the next
/// method declaration's name — best-effort identifier scan. Annotations /
/// decorators stacked between the two (`@SchedulerLock(name = "x")`,
/// `@UseGuards(G)`, `@Timed`) are skipped whole, their arguments included, so
/// their name never stands in for the method's (CL.9).
fn method_name_after_annotation(after_close: &str) -> Option<String> {
    let mut rest = after_close.trim_start();
    while let Some(tail) = rest.strip_prefix('@') {
        let name_len = tail
            .bytes()
            .take_while(|b| is_ident_byte(*b) || *b == b'.')
            .count();
        if name_len == 0 {
            return None;
        }
        rest = if tail.as_bytes().get(name_len) == Some(&b'(') {
            let (_, end) = call_args(tail, name_len + 1)?;
            tail.get(end..)?.trim_start()
        } else {
            tail.get(name_len..)?.trim_start()
        };
    }
    // Then tokens until we see `(`. Take the identifier immediately preceding
    // it.
    let after_close = rest;
    let bytes = after_close.as_bytes();
    let mut last_ident_start: Option<usize> = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'(' {
            if let Some(start) = last_ident_start {
                // Walk back to ident start.
                let mut s = start;
                while s > 0
                    && (bytes[s - 1].is_ascii_alphanumeric() || bytes[s - 1] == b'_')
                {
                    s -= 1;
                }
                let end = start + 1;
                let mut e = end;
                while e < bytes.len() && (bytes[e].is_ascii_alphanumeric() || bytes[e] == b'_') {
                    e += 1;
                }
                return Some(after_close[s..e].to_string());
            }
            return None;
        }
        if c.is_ascii_alphanumeric() || c == b'_' {
            if last_ident_start.is_none() || {
                let prev = if i == 0 { 0 } else { bytes[i - 1] };
                !(prev.is_ascii_alphanumeric() || prev == b'_')
            } {
                last_ident_start = Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Nearest Java / Kotlin / C# / TypeScript `class <Name>` declaration in
/// `before` (the source up to an annotation or decorator). A `class ` hit
/// counts only when everything between its line start and the keyword is
/// modifiers or annotations, so prose (`// this class handles ..`) and
/// `subclass ` never match. `export` / `default` are the TS / JS spellings
/// (CL.9), which Java never writes before `class`.
fn enclosing_class_name(before: &str) -> Option<String> {
    const MODIFIERS: &[&str] = &[
        "export",
        "default",
        "public",
        "private",
        "protected",
        "internal",
        "static",
        "final",
        "abstract",
        "sealed",
        "open",
        "data",
        "strictfp",
        "non-sealed",
        "partial",
    ];
    let mut end = before.len();
    for _ in 0..64 {
        let idx = before.get(..end)?.rfind("class ")?;
        end = idx;
        let line_start = before.get(..idx)?.rfind('\n').map_or(0, |n| n + 1);
        let prefix = before.get(line_start..idx)?;
        if !prefix
            .split_whitespace()
            .all(|t| MODIFIERS.contains(&t) || t.starts_with('@'))
        {
            continue;
        }
        let name = leading_ident(before.get(idx + "class ".len()..)?.trim_start())?;
        return Some(name.to_string());
    }
    None
}

// ----------------------------------------------------------------------------
// LA.19a framework schedulers. Every scan is a byte-safe `match_indices` over
// the source with a bounded argument reader ([`call_args`]); all slicing goes
// through `get(..)` at ASCII delimiters.
// ----------------------------------------------------------------------------

/// The shared schedule normaliser for code sources: cron descriptors expand to
/// their 5-field form (robfig / Cronos / Vixie agree on these), `@every <dur>`
/// and `@reboot` stay verbatim as rate / event markers, anything else must
/// already look like a cron expression and is kept verbatim.
fn normalise_schedule(raw: &str) -> Option<String> {
    let s = raw.trim();
    let Some(descriptor) = s.strip_prefix('@') else {
        return looks_like_cron_expr(s).then(|| s.to_string());
    };
    let fixed = match descriptor {
        "yearly" | "annually" => "0 0 1 1 *",
        "monthly" => "0 0 1 * *",
        "weekly" => "0 0 * * 0",
        "daily" | "midnight" => "0 0 * * *",
        "hourly" => "0 * * * *",
        "reboot" => "@reboot",
        _ => {
            let dur = descriptor.strip_prefix("every ")?.trim();
            let valid =
                !dur.is_empty() && dur.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.');
            return valid.then(|| format!("@every {dur}"));
        }
    };
    Some(fixed.to_string())
}

/// Skip a string literal opening at `at`; the index just past its close,
/// which must come before `limit`.
fn skip_string(bytes: &[u8], at: usize, limit: usize) -> Option<usize> {
    let quote = *bytes.get(at)?;
    let mut j = at + 1;
    while j < limit.min(bytes.len()) {
        match bytes[j] {
            b'\\' => j += 2,
            b if b == quote => return Some(j + 1),
            _ => j += 1,
        }
    }
    None
}

/// The top-level arguments of the call whose `(` ends just before `open`,
/// trimmed, plus the index just past its `)`. Nested brackets and string
/// literals are skipped whole; `None` when the call does not close within
/// 4 KiB (a truncated or non-call needle).
fn call_args(source: &str, open: usize) -> Option<(Vec<&str>, usize)> {
    bracket_items(source, open, b')')
}

/// [`call_args`] for any bracket: the top-level items of the list whose
/// opener ends just before `open` and which closes with `close` (`)` / `]` /
/// `}`), plus the index just past the closer. A mismatched closer, or no
/// close within 4 KiB, is `None`.
fn bracket_items(source: &str, open: usize, close: u8) -> Option<(Vec<&str>, usize)> {
    const MAX_CALL_BYTES: usize = 4096;
    let bytes = source.as_bytes();
    let limit = open.saturating_add(MAX_CALL_BYTES).min(bytes.len());
    let mut args = Vec::new();
    let (mut depth, mut start, mut i) = (0usize, open, open);
    while i < limit {
        match bytes[i] {
            b'"' | b'\'' | b'`' => {
                i = skip_string(bytes, i, limit)?;
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b if b == close => {
                let last = source.get(start..i)?.trim();
                if !last.is_empty() {
                    args.push(last);
                }
                return Some((args, i + 1));
            }
            b')' | b']' | b'}' => return None,
            b',' if depth == 0 => {
                args.push(source.get(start..i)?.trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// `"x"` / `'x'` / `` `x` `` — the text of an argument that is exactly one
/// string literal, else `None` (a variable, a concatenation, an interpolation).
fn string_literal(arg: &str) -> Option<String> {
    let bytes = arg.as_bytes();
    let first = *bytes.first()?;
    if !matches!(first, b'"' | b'\'' | b'`') {
        return None;
    }
    (skip_string(bytes, 0, bytes.len())? == bytes.len())
        .then(|| arg.get(1..arg.len() - 1).map(str::to_string))?
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// The identifier `s` starts with, if any.
fn leading_ident(s: &str) -> Option<&str> {
    let n = s.bytes().take_while(|b| is_ident_byte(*b)).count();
    let id = s.get(..n)?;
    (n > 0 && !id.as_bytes()[0].is_ascii_digit()).then_some(id)
}

/// `a` / `a.b` / `a.b.c` — every dot-separated segment an identifier.
fn ident_path(s: &str) -> Option<Vec<&str>> {
    let segs: Vec<&str> = s.trim().split('.').collect();
    segs.iter()
        .all(|seg| leading_ident(seg).is_some_and(|id| id.len() == seg.len()))
        .then_some(segs)
}

/// The identifier ending right before byte `end`, if any.
fn ident_before(source: &str, end: usize) -> Option<&str> {
    let head = source.get(..end)?;
    let n = head.bytes().rev().take_while(|b| is_ident_byte(*b)).count();
    head.get(end - n..).filter(|id| !id.is_empty())
}

/// `(target, handler)` for a handler written as an identifier path: `f` is
/// `Bare(f)`, `a.b.f` is `Attribute { base: b, name: f }`.
fn handler_from_path(segs: &[&str]) -> Option<(String, CallQualifier)> {
    let name = segs.last()?.to_string();
    let q = match segs {
        [_] => CallQualifier::Bare(name.clone()),
        [.., base, _] => CallQualifier::Attribute {
            base: base.to_string(),
            name: name.clone(),
        },
        [] => return None,
    };
    Some((name, q))
}

/// A function-valued argument (Go `AddFunc`, gocron `Do`, APScheduler
/// `add_job`): a named function or method reference binds, an inline
/// `func() {..}` / `lambda: ..` is `anon` with no handler.
fn func_arg_handler(arg: &str) -> (String, Option<CallQualifier>) {
    match ident_path(arg).as_deref().and_then(handler_from_path) {
        Some((target, q)) => (target, Some(q)),
        None => ("anon".to_string(), None),
    }
}

// --- NestJS ------------------------------------------------------------------

/// NestJS `@nestjs/schedule` (gate: that import, in a TS / JS file; CL.9).
/// `@Cron(<expr>)` on a provider method: a quoted expression (5 fields, or
/// NestJS's 6 with seconds first) goes through [`normalise_schedule`]; a
/// `CronExpression.<MEMBER>` keeps the member path verbatim as the schedule
/// identity (the enum's values are not tabled, so it pairs with no equal
/// literal elsewhere); any other first argument (a variable, a `Date`) is no
/// job. `@Interval(ms)` / `@Interval('name', ms)` with an integer literal is
/// the rate `@every <ms>ms`; `@Timeout` is a one-shot, not a schedule. The
/// decorated method, scoped by its class, is the handler — the Spring shape.
/// A decorator counts only when it leads its line ([`decorator_leads_line`]),
/// so a commented-out `// @Cron(..)` or a JSDoc `* @Cron(..)` schedules
/// nothing.
fn extract_nest_schedule(source: &str) -> Vec<CronJob> {
    let mut sites: Vec<(usize, &str)> = source
        .match_indices("@Cron(")
        .chain(source.match_indices("@Interval("))
        .collect();
    sites.sort_unstable();
    let mut out = Vec::new();
    for (pos, needle) in sites {
        if !decorator_leads_line(source, pos) {
            continue;
        }
        let Some((args, end)) = call_args(source, pos + needle.len()) else {
            continue;
        };
        let schedule = if needle == "@Cron(" {
            args.first().and_then(|a| nest_cron_schedule(a))
        } else {
            nest_interval(&args)
        };
        let Some(schedule) = schedule else {
            continue;
        };
        let method = method_name_after_annotation(source.get(end..).unwrap_or_default());
        let handler = method.as_ref().and_then(|m| {
            Some(CallQualifier::Attribute {
                base: enclosing_class_name(source.get(..pos)?)?,
                name: m.clone(),
            })
        });
        out.push(CronJob {
            schedule,
            target: method.unwrap_or_else(|| "anon".to_string()),
            source: "nestjs",
            handler: at_row(handler, source, pos),
        });
    }
    out
}

/// True when only whitespace, or other decorators, precede the `@` at `pos`
/// on its line: a `//` / `/*` / `*` comment line, or prose, does not.
fn decorator_leads_line(source: &str, pos: usize) -> bool {
    let Some(head) = source.get(..pos) else {
        return false;
    };
    let line_start = head.rfind('\n').map_or(0, |n| n + 1);
    let prefix = head.get(line_start..).unwrap_or_default().trim();
    prefix.is_empty()
        || (prefix.starts_with('@') && !prefix.contains("//") && !prefix.contains("/*"))
}

/// A NestJS `@Cron` first argument: a cron literal, or `CronExpression.X`
/// verbatim.
fn nest_cron_schedule(arg: &str) -> Option<String> {
    if let Some(lit) = string_literal(arg) {
        return normalise_schedule(&lit);
    }
    match ident_path(arg)?.as_slice() {
        ["CronExpression", member] => Some(format!("CronExpression.{member}")),
        _ => None,
    }
}

/// `@Interval(ms)` / `@Interval('name', ms)`: an integer literal (`_`
/// separators allowed) is `@every <ms>ms`; an expression is not read.
fn nest_interval(args: &[&str]) -> Option<String> {
    let ms = match args {
        [ms] => *ms,
        [name, ms] if string_literal(name).is_some() => *ms,
        _ => return None,
    };
    let digits: String = ms.chars().filter(|c| *c != '_').collect();
    let literal = ms.as_bytes().first().is_some_and(u8::is_ascii_digit)
        && digits.bytes().all(|b| b.is_ascii_digit());
    literal.then(|| format!("@every {digits}ms"))
}

// --- Quartz ------------------------------------------------------------------

/// Quartz (gate `org.quartz`): `cronSchedule("..")` (static import or
/// `CronScheduleBuilder.cronSchedule`) and `new CronExpression("..")`, the
/// 6/7-field expression verbatim. A file building exactly one job class
/// (`newJob(X.class)`) runs X; otherwise the trigger statement's
/// `withIdentity("name")` names the job and there is no handler.
fn extract_quartz(source: &str) -> Vec<CronJob> {
    let mut classes: Vec<&str> = Vec::new();
    for (pos, needle) in source.match_indices("newJob(") {
        if ident_before(source, pos).is_some() {
            continue;
        }
        let class = call_args(source, pos + needle.len())
            .and_then(|(args, _)| args.first()?.strip_suffix(".class").map(str::trim))
            .and_then(ident_path)
            .and_then(|segs| segs.last().copied());
        if let Some(c) = class
            && !classes.contains(&c)
        {
            classes.push(c);
        }
    }
    let job_class = match classes.as_slice() {
        [only] => Some(only.to_string()),
        _ => None,
    };
    let mut out = Vec::new();
    for needle in ["cronSchedule(", "new CronExpression("] {
        for (pos, _) in source.match_indices(needle) {
            if ident_before(source, pos).is_some() {
                continue;
            }
            let Some((args, _)) = call_args(source, pos + needle.len()) else {
                continue;
            };
            let Some(schedule) = args
                .first()
                .and_then(|a| string_literal(a))
                .and_then(|raw| normalise_schedule(&raw))
            else {
                continue;
            };
            let (target, handler) = match &job_class {
                Some(x) => (x.clone(), Some(CallQualifier::Bare(x.clone()))),
                None => (
                    statement_identity(source, pos).unwrap_or_else(|| "anon".to_string()),
                    None,
                ),
            };
            out.push(CronJob {
                schedule,
                target,
                source: "quartz",
                handler: at_row(handler, source, pos),
            });
        }
    }
    out
}

/// The `withIdentity("name")` literal of the statement holding byte `pos`
/// (bounded by the nearest `;` / `{` / `}` either side).
fn statement_identity(source: &str, pos: usize) -> Option<String> {
    let start = source
        .get(..pos)?
        .rfind([';', '{', '}'])
        .map_or(0, |i| i + 1);
    let end = source
        .get(pos..)?
        .find(';')
        .map_or(source.len(), |i| pos + i);
    let stmt = source.get(start..end)?;
    let (at, needle) = stmt.match_indices("withIdentity(").next()?;
    let (args, _) = call_args(source, start + at + needle.len())?;
    string_literal(args.first()?)
}

// --- Hangfire ----------------------------------------------------------------

/// Hangfire (gate `Hangfire`): `RecurringJob.AddOrUpdate[<T>](..)` or the
/// same call on an `IRecurringJobManager` receiver. The arguments are an
/// optional job id, the job lambda, then the schedule: a string literal or a
/// `Cron.X` helper. The job is named by the lambda's method.
fn extract_hangfire(source: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    for (pos, needle) in source.match_indices(".AddOrUpdate") {
        let Some(recv) = ident_before(source, pos) else {
            continue;
        };
        let manager = recv == "RecurringJob"
            || recv.to_ascii_lowercase().contains("recurringjob")
            || source.contains(&format!("IRecurringJobManager {recv}"));
        if !manager {
            continue;
        }
        let mut i = pos + needle.len();
        let mut generic: Option<&str> = None;
        if source.get(i..).is_some_and(|r| r.starts_with('<')) {
            let Some(close) = matching_angle(source.as_bytes(), i) else {
                continue;
            };
            generic = source.get(i + 1..close).map(str::trim);
            i = close + 1;
        }
        if !source.get(i..).is_some_and(|r| r.starts_with('(')) {
            continue;
        }
        let Some((args, _)) = call_args(source, i + 1) else {
            continue;
        };
        let Some(li) = args.iter().position(|a| a.contains("=>")) else {
            continue;
        };
        let Some(schedule) = args.get(li + 1).and_then(|a| hangfire_schedule(a)) else {
            continue;
        };
        let (method, handler) = hangfire_lambda(args[li], generic);
        let target = method
            .or_else(|| args[..li].iter().find_map(|a| string_literal(a)))
            .unwrap_or_else(|| "anon".to_string());
        out.push(CronJob {
            schedule,
            target,
            source: "hangfire",
            handler: at_row(handler, source, pos),
        });
    }
    out
}

/// The `>` closing the generic argument list whose `<` is at `open`, within
/// 256 bytes; nested `<..>` (`Repo<Order>`) count.
fn matching_angle(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, &b) in bytes.get(open..)?.iter().take(256).enumerate() {
        match b {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + k);
                }
            }
            b'(' | b')' | b';' | b'{' | b'}' => return None,
            _ => {}
        }
    }
    None
}

/// A Hangfire schedule argument: a cron string, or `Cron.X` / `Cron.X(..)`.
fn hangfire_schedule(arg: &str) -> Option<String> {
    if let Some(raw) = string_literal(arg) {
        return normalise_schedule(&raw);
    }
    let arg = arg.strip_prefix("Hangfire.").unwrap_or(arg);
    let rest = arg.strip_prefix("Cron.")?;
    let member = leading_ident(rest)?;
    let tail = rest.get(member.len()..)?.trim_start();
    if tail.is_empty() {
        return hangfire_cron(member, &[]);
    }
    let offset = arg.len() - tail.len();
    if !tail.starts_with('(') {
        return None;
    }
    let (args, end) = call_args(arg, offset + 1)?;
    if !arg.get(end..)?.trim().is_empty() {
        return None;
    }
    hangfire_cron(member, &args)
}

/// Hangfire's own `Cron` helpers (Hangfire.Core `Cron.cs`), positional or
/// named arguments. Unknown members, and any argument that is not a literal,
/// are skipped — never guessed.
fn hangfire_cron(member: &str, args: &[&str]) -> Option<String> {
    let params: &[&str] = match member {
        "Minutely" => &[],
        "Hourly" => &["minute"],
        "Daily" => &["hour", "minute"],
        "Weekly" => &["dayOfWeek", "hour", "minute"],
        "Monthly" => &["day", "hour", "minute"],
        "Yearly" => &["month", "day", "hour", "minute"],
        _ => return None,
    };
    if args.len() > params.len() {
        return None;
    }
    let mut got: Vec<Option<u32>> = vec![None; params.len()];
    for (i, arg) in args.iter().enumerate() {
        let (slot, value) = match arg.split_once(':') {
            Some((name, v)) => (params.iter().position(|p| *p == name.trim())?, v.trim()),
            None => (i, arg.trim()),
        };
        got[slot] = Some(if params[slot] == "dayOfWeek" {
            day_of_week_index(value)?
        } else {
            value.parse().ok()?
        });
    }
    let get = |p: &str, default: u32| {
        params
            .iter()
            .position(|x| *x == p)
            .and_then(|i| got[i])
            .unwrap_or(default)
    };
    let (m, h) = (get("minute", 0), get("hour", 0));
    Some(match member {
        "Minutely" => "* * * * *".to_string(),
        "Hourly" => format!("{m} * * * *"),
        "Daily" => format!("{m} {h} * * *"),
        // `Cron.Weekly()` is Monday in Hangfire, not cron's Sunday.
        "Weekly" => format!("{m} {h} * * {}", get("dayOfWeek", 1)),
        "Monthly" => format!("{m} {h} {} * *", get("day", 1)),
        _ => format!("{m} {h} {} {} *", get("day", 1), get("month", 1)),
    })
}

/// `DayOfWeek.Monday` → 1 (.NET's `DayOfWeek` numbering, Sunday = 0).
fn day_of_week_index(v: &str) -> Option<u32> {
    const DAYS: [&str; 7] = [
        "Sunday",
        "Monday",
        "Tuesday",
        "Wednesday",
        "Thursday",
        "Friday",
        "Saturday",
    ];
    let day = v.strip_prefix("DayOfWeek.")?;
    DAYS.iter()
        .position(|d| *d == day)
        .and_then(|i| u32::try_from(i).ok())
}

/// The method a Hangfire job lambda calls, and its handler ref:
/// `() => Cleaner.Run()` → `Attribute { Cleaner, Run }`; `x => x.Send()` on
/// `AddOrUpdate<IFoo>` → `Attribute { IFoo, Send }`. A receiver that is the
/// lambda parameter with no generic type, or a bare call, names the job but
/// binds nothing.
fn hangfire_lambda(arg: &str, generic: Option<&str>) -> (Option<String>, Option<CallQualifier>) {
    let Some((params, body)) = arg.split_once("=>") else {
        return (None, None);
    };
    let params = params.trim();
    let params = params.strip_prefix("async").map_or(params, str::trim_start);
    let param = params
        .trim_start_matches('(')
        .trim_end_matches(')')
        .split(',')
        .next()
        .and_then(|p| p.split_whitespace().last());
    let mut body = body.trim();
    if let Some(block) = body.strip_prefix('{') {
        body = block.trim_start();
    }
    if let Some(awaited) = body.strip_prefix("await ") {
        body = awaited.trim_start();
    }
    let Some(segs) = body.split_once('(').and_then(|(path, _)| ident_path(path)) else {
        return (None, None);
    };
    let Some(name) = segs.last().map(|s| s.to_string()) else {
        return (None, None);
    };
    let handler = match segs.as_slice() {
        [.., base, _] if Some(*base) == param => generic
            .and_then(|t| t.split('<').next())
            .and_then(|t| t.trim().rsplit('.').next())
            .filter(|t| leading_ident(t).is_some_and(|id| id.len() == t.len()))
            .map(|t| CallQualifier::Attribute {
                base: t.to_string(),
                name: name.clone(),
            }),
        [.., base, _] => Some(CallQualifier::Attribute {
            base: base.to_string(),
            name: name.clone(),
        }),
        _ => None,
    };
    (Some(name), handler)
}

// --- Go: robfig/cron and gocron ------------------------------------------------

/// Go schedulers. robfig (gate `robfig/cron`): `c.AddFunc("spec", fn)` and
/// `c.AddJob("spec", job)`. gocron (gate `go-co-op/gocron`): v1
/// `s.Cron("spec")..Do(fn)` / `CronWithSeconds`, v2
/// `gocron.CronJob("spec", secs), gocron.NewTask(fn, ..)`.
fn extract_go_cron(source: &str, robfig: bool, gocron: bool) -> Vec<CronJob> {
    let mut out = Vec::new();
    let mut push = |schedule: Option<String>, (target, handler), src: &'static str, at: usize| {
        if let Some(schedule) = schedule {
            out.push(CronJob {
                schedule,
                target,
                source: src,
                handler: at_row(handler, source, at),
            });
        }
    };
    let spec = |args: &[&str]| {
        args.first()
            .and_then(|a| string_literal(a))
            .and_then(|raw| normalise_schedule(&raw))
    };
    if robfig {
        for needle in [".AddFunc(", ".AddJob("] {
            for (pos, _) in source.match_indices(needle) {
                let Some((args, _)) = call_args(source, pos + needle.len()) else {
                    continue;
                };
                let Some(job) = args.get(1) else {
                    continue;
                };
                let handler = if needle == ".AddFunc(" {
                    func_arg_handler(job)
                } else {
                    go_job_handler(job)
                };
                push(spec(&args), handler, "robfig", pos);
            }
        }
    }
    if gocron {
        for needle in [".Cron(", ".CronWithSeconds("] {
            for (pos, _) in source.match_indices(needle) {
                let Some((args, end)) = call_args(source, pos + needle.len()) else {
                    continue;
                };
                let handler = chained_call_arg(source, end, "Do")
                    .map_or_else(|| ("anon".to_string(), None), func_arg_handler);
                push(spec(&args), handler, "gocron", pos);
            }
        }
        for (pos, needle) in source.match_indices("gocron.CronJob(") {
            let Some((args, end)) = call_args(source, pos + needle.len()) else {
                continue;
            };
            let task = source.get(end..).and_then(|rest| {
                let rest = rest.trim_start().strip_prefix(',')?.trim_start();
                let rest = rest.strip_prefix("gocron.NewTask(")?;
                let open = source.len() - rest.len();
                call_args(source, open).and_then(|(a, _)| a.first().copied())
            });
            let handler = task.map_or_else(|| ("anon".to_string(), None), func_arg_handler);
            push(spec(&args), handler, "gocron", pos);
        }
    }
    out
}

/// robfig `AddJob`'s job value: `&T{..}` / `T{..}` runs `T.Run`,
/// `cron.FuncJob(f)` runs `f`; a plain variable names the job but binds
/// nothing (its type is not visible to a text scan).
fn go_job_handler(arg: &str) -> (String, Option<CallQualifier>) {
    if let Some(inner) = arg
        .strip_prefix("cron.FuncJob(")
        .and_then(|r| r.strip_suffix(')'))
    {
        return func_arg_handler(inner);
    }
    let lit = arg.strip_prefix('&').unwrap_or(arg);
    if let Some((ty, _)) = lit.split_once('{')
        && let Some(t) = ident_path(ty).and_then(|segs| segs.last().copied())
    {
        let q = CallQualifier::Attribute {
            base: t.to_string(),
            name: "Run".to_string(),
        };
        return (t.to_string(), Some(q));
    }
    match ident_path(arg).as_deref() {
        Some([var]) => (var.to_string(), None),
        _ => ("anon".to_string(), None),
    }
}

/// Walk a method chain from byte `i` (just past a call's `)`): the first
/// argument of the `.method(..)` link, if the chain reaches one.
fn chained_call_arg<'s>(source: &'s str, mut i: usize, method: &str) -> Option<&'s str> {
    for _ in 0..8 {
        let rest = source.get(i..)?;
        let link = rest.trim_start().strip_prefix('.')?;
        let name = leading_ident(link)?;
        if !link.get(name.len()..)?.starts_with('(') {
            return None;
        }
        let open = source.len() - link.len() + name.len() + 1;
        let (args, end) = call_args(source, open)?;
        if name == method {
            return args.first().copied();
        }
        i = end;
    }
    None
}

// --- APScheduler ---------------------------------------------------------------

/// APScheduler (gate `apscheduler`): the `@s.scheduled_job(trigger, **kw)`
/// decorator (handler: the decorated `def`) and `s.add_job(f, trigger, **kw)`
/// / 4.x `s.add_schedule(..)` (handler: `f`). The trigger is `"cron"` /
/// `"interval"` with field kwargs, `CronTrigger(**kw)`,
/// `CronTrigger.from_crontab("..")` or `IntervalTrigger(**kw)`.
fn extract_apscheduler(source: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    for (pos, needle) in source.match_indices(".scheduled_job(") {
        let line_start = source
            .get(..pos)
            .and_then(|h| h.rfind('\n'))
            .map_or(0, |n| n + 1);
        if !source
            .get(line_start..pos)
            .is_some_and(|l| l.trim_start().starts_with('@'))
        {
            continue;
        }
        let Some((args, end)) = call_args(source, pos + needle.len()) else {
            continue;
        };
        let (positional, kw) = split_kwargs(&args);
        let trigger = positional
            .first()
            .copied()
            .or_else(|| kw_get(&kw, "trigger"));
        let Some(schedule) = aps_schedule(trigger, &kw) else {
            continue;
        };
        let func = decorated_def(source, end);
        out.push(CronJob {
            schedule,
            target: func.clone().unwrap_or_else(|| "anon".to_string()),
            source: "apscheduler",
            handler: at_row(func.map(CallQualifier::Bare), source, pos),
        });
    }
    for needle in [".add_job(", ".add_schedule("] {
        for (pos, _) in source.match_indices(needle) {
            let Some((args, _)) = call_args(source, pos + needle.len()) else {
                continue;
            };
            let (positional, kw) = split_kwargs(&args);
            let (func, rest) = match kw_get(&kw, "func") {
                Some(f) => (Some(f), positional.as_slice()),
                None => (
                    positional.first().copied(),
                    positional.get(1..).unwrap_or(&[]),
                ),
            };
            let trigger = rest.first().copied().or_else(|| kw_get(&kw, "trigger"));
            let Some(schedule) = aps_schedule(trigger, &kw) else {
                continue;
            };
            let (target, handler) =
                func.map_or_else(|| ("anon".to_string(), None), py_func_handler);
            out.push(CronJob {
                schedule,
                target,
                source: "apscheduler",
                handler: at_row(handler, source, pos),
            });
        }
    }
    out
}

/// Split call arguments into positionals and `name=value` keywords.
fn split_kwargs<'a>(args: &[&'a str]) -> (Vec<&'a str>, Vec<(&'a str, &'a str)>) {
    let mut positional = Vec::new();
    let mut kw = Vec::new();
    for arg in args {
        let named = arg.split_once('=').filter(|(name, value)| {
            let name = name.trim();
            leading_ident(name).is_some_and(|id| id.len() == name.len()) && !value.starts_with('=')
        });
        match named {
            Some((name, value)) => kw.push((name.trim(), value.trim())),
            None => positional.push(*arg),
        }
    }
    (positional, kw)
}

fn kw_get<'a>(kw: &[(&str, &'a str)], name: &str) -> Option<&'a str> {
    kw.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

/// A job function reference: `f`, `mod.f`, or APScheduler's textual
/// `"pkg.mod:f"` reference.
fn py_func_handler(arg: &str) -> (String, Option<CallQualifier>) {
    if let Some(text) = string_literal(arg) {
        if let Some((_, func)) = text.split_once(':') {
            return func_arg_handler(func);
        }
        return ("anon".to_string(), None);
    }
    func_arg_handler(arg)
}

/// The name of the `def` a decorator ending at byte `end` decorates, skipping
/// any further stacked decorators.
fn decorated_def(source: &str, end: usize) -> Option<String> {
    let mut rest = source.get(end..)?;
    // The rest of the decorator's own line.
    rest = rest.split_once('\n')?.1;
    for _ in 0..8 {
        let t = rest.trim_start();
        if t.starts_with('@') {
            rest = t.split_once('\n')?.1;
            continue;
        }
        let t = t.strip_prefix("async ").map_or(t, str::trim_start);
        let name = leading_ident(t.strip_prefix("def ")?.trim_start())?;
        return Some(name.to_string());
    }
    None
}

/// The schedule an APScheduler trigger argument (plus the call's kwargs)
/// describes.
fn aps_schedule(trigger: Option<&str>, kw: &[(&str, &str)]) -> Option<String> {
    let trigger = trigger?;
    if let Some(alias) = string_literal(trigger) {
        return match alias.as_str() {
            "cron" => aps_cron(kw),
            "interval" => aps_interval(kw),
            _ => None,
        };
    }
    let (head, _) = trigger.split_once('(')?;
    let (args, end) = call_args(trigger, head.len() + 1)?;
    if !trigger.get(end..)?.trim().is_empty() {
        return None;
    }
    let head = head.trim();
    if head == "CronTrigger.from_crontab" || head.ends_with(".CronTrigger.from_crontab") {
        return normalise_schedule(&string_literal(args.first()?)?);
    }
    let (positional, inner) = split_kwargs(&args);
    if !positional.is_empty() {
        return None;
    }
    match head.rsplit('.').next()? {
        "CronTrigger" => aps_cron(&inner),
        "IntervalTrigger" => aps_interval(&inner),
        _ => None,
    }
}

/// A Python kwarg value that is a literal: `3` or `'mon'` / `"*/5"`.
fn py_literal(v: &str) -> Option<String> {
    if let Some(s) = string_literal(v) {
        return Some(s);
    }
    (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())).then(|| v.to_string())
}

/// APScheduler cron-trigger kwargs → a cron expression, by APScheduler's own
/// rule: fields coarser than the least-significant explicitly given field
/// default to `*`, finer ones to their minimum (`hour=3` → `0 3 * * *`).
/// `second` is dropped when 0 and otherwise leads a 6-field expression.
/// `year` / `week` have no 5-field slot, so a trigger using them is skipped;
/// so is any field whose value is not a literal.
fn aps_cron(kw: &[(&str, &str)]) -> Option<String> {
    // (name, APScheduler's DEFAULT_VALUES entry), coarse to fine.
    const FIELDS: [(&str, &str); 8] = [
        ("year", "*"),
        ("month", "1"),
        ("day", "1"),
        ("week", "*"),
        ("day_of_week", "*"),
        ("hour", "0"),
        ("minute", "0"),
        ("second", "0"),
    ];
    let mut given: Vec<Option<String>> = Vec::with_capacity(FIELDS.len());
    for (name, _) in FIELDS {
        given.push(match kw_get(kw, name) {
            Some(v) => Some(py_literal(v)?),
            None => None,
        });
    }
    if given[0].is_some() || given[3].is_some() {
        return None;
    }
    let last = given.iter().rposition(Option::is_some);
    let vals: Vec<String> = given
        .into_iter()
        .enumerate()
        .map(|(i, g)| match g {
            Some(v) => v,
            None if last.is_some_and(|l| i > l) => FIELDS[i].1.to_string(),
            None => "*".to_string(),
        })
        .collect();
    let dow = aps_day_of_week(&vals[4])?;
    let five = format!("{} {} {} {} {dow}", vals[6], vals[5], vals[2], vals[1]);
    let expr = if vals[7] == "0" {
        five
    } else {
        format!("{} {five}", vals[7])
    };
    looks_like_cron_expr(&expr).then_some(expr)
}

/// APScheduler numbers weekdays from Monday = 0; cron from Sunday = 0. Every
/// number that is a weekday (not a `/step`) becomes its name, so the
/// expression means the same day in both.
fn aps_day_of_week(v: &str) -> Option<String> {
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let bytes = v.as_bytes();
    let mut out = String::with_capacity(v.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() && (i == 0 || bytes[i - 1] != b'/') {
            let n = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
            let day: usize = v.get(i..i + n)?.parse().ok()?;
            out.push_str(DAYS.get(day)?);
            i += n;
            continue;
        }
        out.push(char::from(bytes[i]));
        i += 1;
    }
    Some(out)
}

/// APScheduler interval kwargs → an `@every` rate marker (robfig's family):
/// `minutes=5` → `@every 5m`.
fn aps_interval(kw: &[(&str, &str)]) -> Option<String> {
    const UNITS: [(&str, &str); 5] = [
        ("weeks", "w"),
        ("days", "d"),
        ("hours", "h"),
        ("minutes", "m"),
        ("seconds", "s"),
    ];
    let mut dur = String::new();
    for (name, unit) in UNITS {
        if let Some(v) = kw_get(kw, name) {
            if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            dur.push_str(v);
            dur.push_str(unit);
        }
    }
    (!dur.is_empty()).then(|| format!("@every {dur}"))
}

// ----------------------------------------------------------------------------
// LA.19b script-language schedulers: whenever, sidekiq-cron, Laravel, Oban.
// Ruby / PHP / Elixir sources are scanned through [`blank_comments`] first, so
// a commented-out job never fires and an apostrophe in a comment cannot
// derail the bracket reader. Offsets are preserved, so every helper above
// applies unchanged.
// ----------------------------------------------------------------------------

/// Dispatch by extension, then by the library's own spelling: whenever (path
/// `config/schedule.rb`, or `every ` with a `runner ` / `rake ` / `command `
/// job), sidekiq-cron (`Sidekiq::Cron::Job` in Ruby; `class:` beside `cron:`
/// / `every:` in YAML), Laravel (`$schedule->` / `Schedule::`), Oban
/// (`Oban.Plugins.Cron`).
fn extract_script_schedulers(source: &str, path: &str, ext: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    match ext {
        "rb" => {
            let whenever = path.replace('\\', "/").ends_with("config/schedule.rb")
                || (source.contains("every ")
                    && ["runner ", "rake ", "command "]
                        .iter()
                        .any(|k| source.contains(k)));
            let sidekiq = source.contains("Sidekiq::Cron::Job");
            if whenever || sidekiq {
                let clean = blank_comments(source, CommentStyle::Hash);
                if whenever {
                    out.extend(extract_whenever(&clean));
                }
                if sidekiq {
                    out.extend(extract_sidekiq_cron(&clean, false));
                }
            }
        }
        "yml" | "yaml"
            if source.contains("class:")
                && (source.contains("cron:") || source.contains("every:")) =>
        {
            out.extend(extract_sidekiq_cron(source, true));
        }
        "php" if source.contains("$schedule->") || source.contains("Schedule::") => {
            out.extend(extract_laravel_schedule(&blank_comments(
                source,
                CommentStyle::Php,
            )));
        }
        "ex" | "exs" if source.contains("Oban.Plugins.Cron") => {
            out.extend(extract_oban_crontab(&blank_comments(
                source,
                CommentStyle::Hash,
            )));
        }
        _ => {}
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum CommentStyle {
    /// Ruby / Elixir: `#` to end of line.
    Hash,
    /// PHP: `#` (not a `#[..]` attribute), `//` and `/* .. */`.
    Php,
}

/// `source` with every comment byte replaced by a space (newlines kept), so
/// byte offsets and line numbers are unchanged. String literals — Elixir's
/// `"""` heredocs included — are skipped whole; an unterminated one ends the
/// pass with the rest of the file untouched.
fn blank_comments(source: &str, style: CommentStyle) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut blank = |from: usize, to: usize| {
        for b in out.iter_mut().take(to).skip(from) {
            if *b != b'\n' {
                *b = b' ';
            }
        }
    };
    let eol = |from: usize| {
        bytes
            .get(from..)
            .and_then(|r| r.iter().position(|b| *b == b'\n'))
            .map_or(bytes.len(), |n| from + n)
    };
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            q @ (b'"' | b'\'') => {
                let triple = [q, q, q];
                let next = if bytes.get(i..i + 3) == Some(&triple[..]) {
                    bytes
                        .get(i + 3..)
                        .and_then(|r| r.windows(3).position(|w| w == triple))
                        .map(|n| i + 3 + n + 3)
                } else {
                    skip_string(bytes, i, bytes.len())
                };
                match next {
                    Some(n) => i = n,
                    None => break,
                }
                continue;
            }
            b'#' if !(style == CommentStyle::Php && bytes.get(i + 1) == Some(&b'[')) => {
                let end = eol(i);
                blank(i, end);
                i = end;
                continue;
            }
            b'/' if style == CommentStyle::Php && bytes.get(i + 1) == Some(&b'/') => {
                let end = eol(i);
                blank(i, end);
                i = end;
                continue;
            }
            b'/' if style == CommentStyle::Php && bytes.get(i + 1) == Some(&b'*') => {
                let end = bytes
                    .get(i + 2..)
                    .and_then(|r| r.windows(2).position(|w| w == b"*/"))
                    .map_or(bytes.len(), |n| i + 2 + n + 2);
                blank(i, end);
                i = end;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| source.to_string())
}

/// Split `s` on its top-level commas (brackets and string literals skipped
/// whole), trimmed; `None` when a bracket or string does not balance.
fn split_top_level(s: &str) -> Option<Vec<&str>> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let (mut depth, mut start, mut i) = (0usize, 0usize, 0usize);
    while i < bytes.len() {
        match bytes[i] {
            b'"' | b'\'' | b'`' => {
                i = skip_string(bytes, i, bytes.len())?;
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.checked_sub(1)?,
            b',' if depth == 0 => {
                out.push(s.get(start..i)?.trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 {
        return None;
    }
    let last = s.get(start..)?.trim();
    if !last.is_empty() {
        out.push(last);
    }
    Some(out)
}

/// A time of day: `4:30 am`, `4pm`, `12am`, `noon`, `midnight` (with
/// `meridiem`), and 24-hour `16:00` / `9` always. `(hour, minute)`, or `None`
/// for anything else — a computed or natural-language time is never guessed.
fn clock_time(s: &str, meridiem: bool) -> Option<(u32, u32)> {
    let t = s.trim().to_ascii_lowercase();
    if meridiem {
        match t.as_str() {
            "noon" => return Some((12, 0)),
            "midnight" => return Some((0, 0)),
            _ => {}
        }
    }
    let (body, pm) = match (t.strip_suffix("am"), t.strip_suffix("pm")) {
        (Some(b), _) if meridiem => (b.trim_end(), Some(false)),
        (_, Some(b)) if meridiem => (b.trim_end(), Some(true)),
        _ => (t.as_str(), None),
    };
    let (h, m) = body.split_once(':').unwrap_or((body, "0"));
    let num = |v: &str| {
        (!v.is_empty() && v.len() <= 2 && v.bytes().all(|b| b.is_ascii_digit()))
            .then(|| v.parse::<u32>().ok())
            .flatten()
    };
    let (h, m) = (num(h)?, num(m)?);
    if m >= 60 {
        return None;
    }
    let h = match pm {
        None if h < 24 => h,
        None => return None,
        Some(pm) if (1..=12).contains(&h) => h % 12 + if pm { 12 } else { 0 },
        Some(_) => return None,
    };
    Some((h, m))
}

/// A Ruby hash pair or keyword argument: `key: v`, `:key => v`, `'key' => v`,
/// `"key": v` — `(key, v)`.
fn ruby_pair(item: &str) -> Option<(&str, &str)> {
    let item = item.trim();
    let bytes = item.as_bytes();
    if matches!(bytes.first(), Some(b'"' | b'\'')) {
        let end = skip_string(bytes, 0, bytes.len())?;
        let key = item.get(1..end - 1)?;
        let rest = item.get(end..)?.trim_start();
        let value = rest.strip_prefix("=>").or_else(|| rest.strip_prefix(':'))?;
        return Some((key, value.trim()));
    }
    if let Some(sym) = item.strip_prefix(':') {
        let key = leading_ident(sym)?;
        let value = sym.get(key.len()..)?.trim_start().strip_prefix("=>")?;
        return Some((key, value.trim()));
    }
    let key = leading_ident(item)?;
    let rest = item.get(key.len()..)?;
    let value = rest.strip_prefix(':').filter(|v| !v.starts_with(':'))?;
    Some((key, value.trim()))
}

/// A Ruby constant path (`Report`, `Reports::Digest`) — its segments.
fn ruby_const_path(s: &str) -> Option<Vec<&str>> {
    let segs: Vec<&str> = s.trim().trim_start_matches("::").split("::").collect();
    segs.iter()
        .all(|seg| {
            leading_ident(seg).is_some_and(|id| id.len() == seg.len())
                && seg.starts_with(|c: char| c.is_ascii_uppercase())
        })
        .then_some(segs)
}

// --- whenever ------------------------------------------------------------------

/// whenever's `config/schedule.rb`: each `every <freq>[, at: '<time>'] do .. end`
/// block, run through [`whenever_every`]. The job is the block's first
/// `runner` / `rake` / `command` / `script` line: `runner "Report.generate"`
/// names `Report.generate` and binds `Attribute { Report, generate }`; a rake
/// task, shell command or script is named by its first word and binds
/// nothing.
fn extract_whenever(src: &str) -> Vec<CronJob> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let Some(rest) = lines[i]
            .trim_start()
            .strip_prefix("every")
            .filter(|r| r.starts_with([' ', '(']))
        else {
            i += 1;
            continue;
        };
        // A header continued over lines that end in `,`.
        let mut header = rest.trim().to_string();
        let mut j = i;
        while header.ends_with(',') && j + 1 < lines.len() && j - i < 4 {
            j += 1;
            header.push(' ');
            header.push_str(lines[j].trim());
        }
        i = j + 1;
        let Some(parts) = block_header_args(&header).and_then(split_top_level) else {
            continue;
        };
        let Some((freq, kw)) = parts.split_first() else {
            continue;
        };
        let at = kw
            .iter()
            .filter_map(|p| ruby_pair(p))
            .find_map(|(k, v)| (k == "at").then_some(v));
        let Some(schedule) = whenever_every(freq, at) else {
            continue;
        };
        let job = lines
            .get(i..)
            .unwrap_or(&[])
            .iter()
            .enumerate()
            .map(|(k, l)| (i + k, l.trim()))
            .take_while(|(_, l)| *l != "end" && !l.starts_with("every "))
            .find_map(|(row, l)| whenever_job_line(l).map(|job| (row, job)));
        let (target, handler) = job
            .and_then(|(row, (kind, arg))| {
                let (target, handler) = whenever_job(kind, arg)?;
                let row = u32::try_from(row).unwrap_or(u32::MAX);
                Some((target, handler.map(|h| (h, row))))
            })
            .unwrap_or_else(|| ("anon".to_string(), None));
        out.push(CronJob {
            schedule,
            target,
            source: "whenever",
            handler,
        });
    }
    out
}

/// The argument text of `<args> do` / `(<args>) do |..|` — `None` when the
/// header does not open a `do` block.
fn block_header_args(header: &str) -> Option<&str> {
    let mut h = header.trim_end();
    if let Some(body) = h.strip_suffix('|') {
        h = body.rfind('|').and_then(|p| body.get(..p))?.trim_end();
    }
    let args = h.strip_suffix("do")?;
    if !args.ends_with([' ', ')']) {
        return None;
    }
    let args = args.trim();
    Some(
        args.strip_prefix('(')
            .and_then(|a| a.strip_suffix(')'))
            .unwrap_or(args),
    )
}

/// A whenever job line: its job type and argument text.
fn whenever_job_line(line: &str) -> Option<(&'static str, &str)> {
    ["runner", "rake", "command", "script"]
        .into_iter()
        .find_map(|kind| {
            let rest = line.strip_prefix(kind)?;
            rest.starts_with([' ', '(']).then(|| (kind, rest.trim()))
        })
}

fn whenever_job(kind: &str, arg: &str) -> Option<(String, Option<CallQualifier>)> {
    let arg = arg
        .strip_prefix('(')
        .and_then(|a| a.strip_suffix(')'))
        .unwrap_or(arg);
    let text = string_literal(split_top_level(arg)?.first()?)?;
    let text = text.trim();
    if kind == "runner"
        && let Some(bound) = ruby_runner_handler(text)
    {
        return Some(bound);
    }
    Some((text.split_whitespace().next()?.to_string(), None))
}

/// `Report.generate` / `Reports::Digest.build!(1)` — a `runner` string that
/// calls a class method: `(path, Attribute { class, method })`.
fn ruby_runner_handler(text: &str) -> Option<(String, Option<CallQualifier>)> {
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '!' | '?')))
        .unwrap_or(text.len());
    let path = text.get(..end)?;
    let (recv, method) = path.rsplit_once('.')?;
    let class = *ruby_const_path(recv)?.last()?;
    let bare = method.strip_suffix(['!', '?']).unwrap_or(method);
    if leading_ident(bare).is_none_or(|id| id.len() != bare.len()) {
        return None;
    }
    let q = CallQualifier::Attribute {
        base: class.to_string(),
        name: method.to_string(),
    };
    Some((path.to_string(), Some(q)))
}

/// whenever's `every` argument → a cron expression. A quoted cron string is
/// verbatim; `N.minutes` → `*/N * * * *`; `N.hours` → `M */N * * *` (`M` an
/// integer `at:`, else 0); `1.day` / `:day` → `M H * * *` from `at:`, else
/// midnight; `N.days` → `M H */N * *`; `:monday`..`:sunday` / `:weekday` /
/// `:weekend` → day of week `1`..`0` / `1-5` / `0,6`; `1.week` → `M H * * 0`;
/// `1.month` → `M H 1 * *`; `1.year` → `M H 1 1 *`; `:reboot` and the
/// `:daily`-style keywords → their `@` descriptor. Anything else, and any
/// `at:` that is not a literal time, is skipped.
fn whenever_every(freq: &str, at: Option<&str>) -> Option<String> {
    let freq = freq.trim();
    if let Some(raw) = string_literal(freq) {
        return normalise_schedule(&raw);
    }
    let clock = || match at {
        None => Some((0, 0)),
        Some(a) => clock_time(&string_literal(a)?, true),
    };
    let (n, unit): (u32, &str) = match freq.strip_prefix(':') {
        Some(sym) => match sym {
            "reboot" | "yearly" | "annually" | "monthly" | "weekly" | "daily" | "midnight"
            | "hourly" => {
                return at
                    .is_none()
                    .then(|| normalise_schedule(&format!("@{sym}")))?;
            }
            "minute" | "hour" | "day" | "week" | "month" | "year" => (1, sym),
            _ => {
                let dow = match sym {
                    "sunday" => "0",
                    "monday" => "1",
                    "tuesday" => "2",
                    "wednesday" => "3",
                    "thursday" => "4",
                    "friday" => "5",
                    "saturday" => "6",
                    "weekday" => "1-5",
                    "weekend" => "0,6",
                    _ => return None,
                };
                let (h, m) = clock()?;
                return Some(format!("{m} {h} * * {dow}"));
            }
        },
        None => {
            let (n, unit) = freq.split_once('.')?;
            if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (n.parse().ok().filter(|n| *n > 0)?, unit)
        }
    };
    let unit = unit.strip_suffix('s').unwrap_or(unit);
    Some(match (unit, n) {
        ("minute", 1) if at.is_none() => "* * * * *".to_string(),
        ("minute", 2..=59) if at.is_none() => format!("*/{n} * * * *"),
        ("hour", 1..=23) => {
            let m: u32 = match at {
                None => 0,
                Some(a) => a.parse().ok().filter(|m| *m < 60)?,
            };
            if n == 1 {
                format!("{m} * * * *")
            } else {
                format!("{m} */{n} * * *")
            }
        }
        ("day", 1) => {
            let (h, m) = clock()?;
            format!("{m} {h} * * *")
        }
        ("day", 2..=31) => {
            let (h, m) = clock()?;
            format!("{m} {h} */{n} * *")
        }
        ("week", 1) => {
            let (h, m) = clock()?;
            format!("{m} {h} * * 0")
        }
        ("month", 1) => {
            let (h, m) = clock()?;
            format!("{m} {h} 1 * *")
        }
        ("year", 1) => {
            let (h, m) = clock()?;
            format!("{m} {h} 1 1 *")
        }
        _ => return None,
    })
}

// --- sidekiq-cron / sidekiq-scheduler ---------------------------------------------

/// sidekiq-cron and sidekiq-scheduler. A job needs BOTH a schedule (`cron:`,
/// or sidekiq-scheduler's `every:`) and a `class:` — the precision gate that
/// keeps arbitrary `cron:` keys out. The target is the class; in Ruby source
/// the handler is `Bare(<class>)`, a YAML job binds nothing (other graph).
///
/// YAML (`is_yaml`): the entries of the top-level mapping (sidekiq-cron's
/// `schedule.yml`), the items of a top-level list (its array form), and the
/// entries under a `schedule:` / `:schedule:` key (sidekiq-scheduler).
/// Ruby: `Sidekiq::Cron::Job.create(..)` / `.new(..)` with keyword or hash
/// arguments, `load_from_hash({name => {..}})` and `load_from_array([{..}])`
/// with literal arguments.
fn extract_sidekiq_cron(source: &str, is_yaml: bool) -> Vec<CronJob> {
    if is_yaml {
        return extract_sidekiq_yaml(source);
    }
    let mut out = Vec::new();
    for (pos, needle) in source.match_indices("Sidekiq::Cron::Job.") {
        let at = pos + needle.len();
        let Some(method) = source.get(at..).and_then(leading_ident) else {
            continue;
        };
        let mut open = at + method.len();
        if source.get(open..).is_some_and(|r| r.starts_with('!')) {
            open += 1;
        }
        if !source.get(open..).is_some_and(|r| r.starts_with('(')) {
            continue;
        }
        let Some((args, _)) = call_args(source, open + 1) else {
            continue;
        };
        let hashes: Vec<Vec<&str>> = match method {
            "create" | "new" => match args.as_slice() {
                [one] if one.starts_with('{') => ruby_hash_items(one).into_iter().collect(),
                _ => vec![args],
            },
            "load_from_hash" => args
                .first()
                .and_then(|a| ruby_hash_items(a))
                .unwrap_or_default()
                .iter()
                .filter_map(|item| ruby_hash_items(ruby_pair(item)?.1))
                .collect(),
            "load_from_array" => args
                .first()
                .and_then(|a| ruby_or_php_list(a))
                .unwrap_or_default()
                .iter()
                .filter_map(|item| ruby_hash_items(item))
                .collect(),
            _ => continue,
        };
        for items in hashes {
            let pairs: Vec<(&str, &str)> = items.iter().filter_map(|i| ruby_pair(i)).collect();
            let get = |k: &str| pairs.iter().find_map(|(key, v)| (*key == k).then_some(*v));
            let Some(cron) = get("cron").and_then(string_literal) else {
                continue;
            };
            let Some(class) = get("class").and_then(ruby_class_value) else {
                continue;
            };
            if let Some(job) = sidekiq_job(Some(&cron), None, &class, Some(line_of(source, pos))) {
                out.push(job);
            }
        }
    }
    out
}

/// The items of a Ruby hash literal / Elixir tuple `{ .. }`.
fn ruby_hash_items(s: &str) -> Option<Vec<&str>> {
    let s = s.trim();
    let inner = s.strip_prefix('{')?;
    let (items, end) = bracket_items(s, s.len() - inner.len(), b'}')?;
    s.get(end..)?.trim().is_empty().then_some(items)
}

/// The items of a `[..]` list literal (Ruby / PHP / Elixir).
fn ruby_or_php_list(s: &str) -> Option<Vec<&str>> {
    let s = s.trim();
    let inner = s.strip_prefix('[')?;
    let (items, end) = bracket_items(s, s.len() - inner.len(), b']')?;
    s.get(end..)?.trim().is_empty().then_some(items)
}

/// A sidekiq `class:` value in Ruby: `'Worker'`, `Worker`, `A::Worker`, or
/// `Worker.name` / `Worker.to_s`.
fn ruby_class_value(v: &str) -> Option<String> {
    if let Some(s) = string_literal(v) {
        return Some(s);
    }
    let v = v
        .strip_suffix(".name")
        .or_else(|| v.strip_suffix(".to_s"))
        .unwrap_or(v);
    ruby_const_path(v).map(|_| v.trim().to_string())
}

/// One sidekiq job from its unquoted `cron` / `every` and `class` values.
/// `handler_line` is `Some(row of the Ruby call)` for the code form, which
/// binds the class as the handler; the YAML form binds none.
fn sidekiq_job(
    cron: Option<&str>,
    every: Option<&str>,
    class: &str,
    handler_line: Option<u32>,
) -> Option<CronJob> {
    let schedule = match (cron, every) {
        (Some(c), _) => normalise_schedule(c)?,
        (None, Some(e)) => normalise_schedule(&format!("@every {}", e.trim()))?,
        (None, None) => return None,
    };
    let class = class.trim();
    if class.is_empty() {
        return None;
    }
    let handler = handler_line.and_then(|line| {
        ruby_const_path(class)
            .and_then(|segs| segs.last().map(|s| (CallQualifier::Bare(s.to_string()), line)))
    });
    Some(CronJob {
        schedule,
        target: class.to_string(),
        source: "sidekiq_cron",
        handler,
    })
}

/// One logical line of a YAML block mapping: a `- ` list item opener, or a
/// `key: value` line (a list item's inline first key is its own line, one
/// level deeper than its dash).
struct YamlLine<'a> {
    indent: usize,
    dash: bool,
    key: Option<&'a str>,
    value: &'a str,
}

fn yaml_lines(source: &str) -> Vec<YamlLine<'_>> {
    let mut lines = Vec::new();
    for raw in source.lines() {
        let t = raw.trim_start();
        let mut indent = raw.len() - t.len();
        let t = t.trim_end();
        if t.is_empty() || t.starts_with('#') || t.starts_with("---") || t == "..." {
            continue;
        }
        let mut body = t;
        if let Some(rest) = t
            .strip_prefix('-')
            .filter(|r| r.is_empty() || r.starts_with(' '))
        {
            lines.push(YamlLine {
                indent,
                dash: true,
                key: None,
                value: "",
            });
            let r = rest.trim_start();
            if r.is_empty() {
                continue;
            }
            indent += t.len() - r.len();
            body = r;
        }
        let (key, value) = yaml_key_value(body);
        lines.push(YamlLine {
            indent,
            dash: false,
            key,
            value,
        });
    }
    lines
}

/// `key: value` → `(Some(key), value)`, the key unquoted and a Ruby symbol's
/// leading `:` dropped (`:schedule:` is sidekiq-scheduler's spelling).
fn yaml_key_value(body: &str) -> (Option<&str>, &str) {
    let bytes = body.as_bytes();
    let from = match bytes.first() {
        Some(b'"' | b'\'') => skip_string(bytes, 0, bytes.len()).unwrap_or(0),
        Some(b':') => 1,
        _ => 0,
    };
    let colon = (from..bytes.len())
        .find(|&i| bytes[i] == b':' && bytes.get(i + 1).is_none_or(|b| *b == b' '));
    match colon {
        Some(c) => {
            let key = body.get(..c).unwrap_or("").trim();
            let key = key.strip_prefix(':').unwrap_or(key);
            let key = key.trim_matches(|c| c == '"' || c == '\'');
            (Some(key), body.get(c + 1..).unwrap_or("").trim())
        }
        None => (None, body),
    }
}

fn extract_sidekiq_yaml(source: &str) -> Vec<CronJob> {
    let lines = yaml_lines(source);
    let mut out = Vec::new();
    for (h, head) in lines.iter().enumerate() {
        let opens_block = head.dash || (head.key.is_some() && head.value.is_empty());
        if !opens_block {
            continue;
        }
        // Where the block sits: top level, or under a `schedule` key.
        let parent = lines
            .get(..h)
            .unwrap_or(&[])
            .iter()
            .rev()
            .find(|l| l.indent < head.indent);
        let placed = match parent {
            None => true,
            Some(p) => !p.dash && p.key == Some("schedule"),
        };
        if !placed {
            continue;
        }
        let body: Vec<&YamlLine> = lines
            .get(h + 1..)
            .unwrap_or(&[])
            .iter()
            .take_while(|l| l.indent > head.indent)
            .collect();
        let Some(child_indent) = body.first().map(|l| l.indent) else {
            continue;
        };
        let get = |k: &str| {
            body.iter()
                .find(|l| l.indent == child_indent && !l.dash && l.key == Some(k))
                .map(|l| l.value)
        };
        let Some(class) = get("class").and_then(first_yaml_string) else {
            continue;
        };
        let cron = get("cron").and_then(first_yaml_string);
        let every = get("every").and_then(|v| {
            if v.starts_with('[') {
                first_list_element(v)
            } else {
                first_yaml_string(v)
            }
        });
        if let Some(job) = sidekiq_job(cron.as_deref(), every.as_deref(), &class, None) {
            out.push(job);
        }
    }
    out
}

// --- Laravel -------------------------------------------------------------------

/// Laravel's scheduler (gate `$schedule->` / `Schedule::`): per statement, the
/// head `->command('name')` (target: the command name; no handler, since no
/// resolver indexes CLI_COMMANDs by command string — `command(X::class)` binds
/// `Bare(X)`), `->job(new X)` / `->job(X::class)` (`Bare(X)`), `->call(..)`
/// (an invokable `new X` / `X::class` or `[X::class, 'm']` binds, a closure
/// is `anon`) or `->exec('..')` (its first word), then the frequency chain
/// through [`frequency_to_cron`]. A chain with no frequency method, or with a
/// method the table does not know, is skipped — never guessed.
fn extract_laravel_schedule(src: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    for needle in ["$schedule->", "Schedule::"] {
        for (pos, _) in src.match_indices(needle) {
            if needle == "Schedule::" && ident_before(src, pos).is_some() {
                continue;
            }
            let head_at = pos + needle.len();
            let Some(head) = src.get(head_at..).and_then(leading_ident) else {
                continue;
            };
            if !matches!(head, "command" | "job" | "call" | "exec") {
                continue;
            }
            let open = head_at + head.len();
            if !src.get(open..).is_some_and(|r| r.starts_with('(')) {
                continue;
            }
            let Some((args, end)) = call_args(src, open + 1) else {
                continue;
            };
            let Some(schedule) = laravel_chain_schedule(src, end) else {
                continue;
            };
            let (target, handler) = laravel_head(head, &args);
            out.push(CronJob {
                schedule,
                target,
                source: "laravel",
                handler: at_row(handler, src, pos),
            });
        }
    }
    out
}

/// Walk the `->method(..)` chain from byte `i` (just past the head call) and
/// fold every link into the expression, starting from Laravel's default
/// `* * * * *`.
fn laravel_chain_schedule(src: &str, mut i: usize) -> Option<String> {
    let mut fields: Vec<String> = vec!["*".to_string(); 5];
    let mut framed = false;
    for _ in 0..32 {
        let rest = src.get(i..)?.trim_start();
        let Some(link) = rest.strip_prefix("->").or_else(|| rest.strip_prefix("?->")) else {
            break;
        };
        let link = link.trim_start();
        let name = leading_ident(link)?;
        let paren = link.get(name.len()..)?.trim_start().strip_prefix('(')?;
        let (args, end) = call_args(src, src.len() - paren.len())?;
        framed |= frequency_to_cron(&mut fields, name, &args)?;
        i = end;
    }
    if !framed {
        return None;
    }
    normalise_schedule(&fields.join(" "))
}

/// The job a Laravel head call schedules: `(target, handler)`.
fn laravel_head(head: &str, args: &[&str]) -> (String, Option<CallQualifier>) {
    let first = args.first().copied().unwrap_or("");
    let anon = || ("anon".to_string(), None);
    let class = |a: &str| php_class_ref(a).map(|c| (c.clone(), Some(CallQualifier::Bare(c))));
    match head {
        "command" | "exec" => match string_literal(first) {
            Some(text) => text
                .split_whitespace()
                .next()
                .map_or_else(anon, |w| (w.to_string(), None)),
            None if head == "command" => class(first).unwrap_or_else(anon),
            None => anon(),
        },
        "call" => class(first)
            .or_else(|| {
                let items = ruby_or_php_list(first)?;
                let [cls, method] = items.as_slice() else {
                    return None;
                };
                let base = php_class_ref(cls)?;
                let name = string_literal(method)?;
                leading_ident(&name).filter(|id| id.len() == name.len())?;
                let q = CallQualifier::Attribute {
                    base,
                    name: name.clone(),
                };
                Some((name, Some(q)))
            })
            .unwrap_or_else(anon),
        _ => class(first).unwrap_or_else(anon),
    }
}

/// `new X` / `new \App\Jobs\X(..)` / `X::class` → `X`.
fn php_class_ref(arg: &str) -> Option<String> {
    let a = arg.trim();
    let path = match a.strip_prefix("new ") {
        Some(n) => n.split('(').next()?.trim(),
        None => a.strip_suffix("::class")?.trim(),
    };
    let path = path.trim_start_matches('\\');
    let segs: Vec<&str> = path.split('\\').collect();
    let all_idents = segs
        .iter()
        .all(|s| leading_ident(s).is_some_and(|id| id.len() == s.len()));
    all_idents.then(|| segs.last().map(|s| s.to_string()))?
}

/// One link of a Laravel schedule chain, folded into the running 5-field
/// expression exactly as Laravel's `ManagesFrequencies` splices it (so
/// `->weekly()->mondays()->at('9:30')` is `30 9 * * 1`). `Some(true)` for a
/// frequency method, `Some(false)` for a known non-frequency modifier
/// (`->withoutOverlapping()`, `->timezone(..)`, ..), `None` for an unknown
/// method or an argument that is not a literal.
fn frequency_to_cron(fields: &mut Vec<String>, method: &str, args: &[&str]) -> Option<bool> {
    const MODIFIERS: &[&str] = &[
        "after",
        "appendOutputTo",
        "before",
        "between",
        "description",
        "emailOutputOnFailure",
        "emailOutputTo",
        "emailWrittenOutputTo",
        "environments",
        "evenInMaintenanceMode",
        "name",
        "onFailure",
        "onFailureWithOutput",
        "onOneServer",
        "onSuccess",
        "onSuccessWithOutput",
        "pingBefore",
        "pingBeforeIf",
        "pingOnFailure",
        "pingOnFailureIf",
        "pingOnSuccess",
        "pingOnSuccessIf",
        "runInBackground",
        "sendOutputTo",
        "skip",
        "storeOutput",
        "then",
        "thenPing",
        "thenPingIf",
        "timezone",
        "unlessBetween",
        "user",
        "when",
        "withoutOverlapping",
    ];
    if MODIFIERS.contains(&method) {
        return Some(false);
    }
    let int = |i: usize, default: u32| -> Option<u32> {
        match args.get(i) {
            None => Some(default),
            Some(a) if !a.is_empty() && a.bytes().all(|b| b.is_ascii_digit()) => a.parse().ok(),
            Some(_) => None,
        }
    };
    let at = |f: &mut Vec<String>, i: usize| -> Option<()> {
        let (h, m) = match args.get(i) {
            None => (0, 0),
            Some(a) => clock_time(&string_literal(a)?, false)?,
        };
        splice(f, 1, m.to_string())?;
        splice(f, 2, h.to_string())
    };
    let fixed = |f: &mut Vec<String>, set: &[(usize, &str)]| -> Option<()> {
        if !args.is_empty() {
            return None;
        }
        set.iter()
            .try_for_each(|(p, v)| splice(f, *p, v.to_string()))
    };
    match method {
        "cron" => {
            let [expr] = args else {
                return None;
            };
            let expr = normalise_schedule(&string_literal(expr)?)?;
            *fields = expr.split_whitespace().map(str::to_string).collect();
        }
        "everyMinute" => fixed(fields, &[(1, "*")])?,
        "everyTwoMinutes" => fixed(fields, &[(1, "*/2")])?,
        "everyThreeMinutes" => fixed(fields, &[(1, "*/3")])?,
        "everyFourMinutes" => fixed(fields, &[(1, "*/4")])?,
        "everyFiveMinutes" => fixed(fields, &[(1, "*/5")])?,
        "everyTenMinutes" => fixed(fields, &[(1, "*/10")])?,
        "everyFifteenMinutes" => fixed(fields, &[(1, "*/15")])?,
        "everyThirtyMinutes" => fixed(fields, &[(1, "0,30")])?,
        "hourly" => fixed(fields, &[(1, "0")])?,
        "hourlyAt" => {
            let offset = match args {
                [one] if one.starts_with('[') => ruby_or_php_list(one)?
                    .iter()
                    .map(|m| m.parse::<u32>().ok().filter(|m| *m < 60))
                    .collect::<Option<Vec<u32>>>()?
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                [_] => int(0, 0).filter(|m| *m < 60)?.to_string(),
                _ => return None,
            };
            splice(fields, 1, offset)?;
        }
        "everyOddHour" | "everyTwoHours" | "everyThreeHours" | "everyFourHours"
        | "everySixHours" => {
            if args.len() > 1 {
                return None;
            }
            let hours = match method {
                "everyOddHour" => "1-23/2",
                "everyTwoHours" => "*/2",
                "everyThreeHours" => "*/3",
                "everyFourHours" => "*/4",
                _ => "*/6",
            };
            splice(fields, 1, int(0, 0).filter(|m| *m < 60)?.to_string())?;
            splice(fields, 2, hours.to_string())?;
        }
        "daily" => fixed(fields, &[(1, "0"), (2, "0")])?,
        "dailyAt" | "at" => {
            if args.len() != 1 {
                return None;
            }
            at(fields, 0)?;
        }
        "twiceDaily" | "twiceDailyAt" => {
            let max = if method == "twiceDaily" { 2 } else { 3 };
            if args.len() > max {
                return None;
            }
            let (first, second) = (int(0, 1)?, int(1, 13)?);
            let offset = if max == 3 { int(2, 0)? } else { 0 };
            if first > 23 || second > 23 || offset > 59 {
                return None;
            }
            splice(fields, 1, offset.to_string())?;
            splice(fields, 2, format!("{first},{second}"))?;
        }
        "weekly" => fixed(fields, &[(1, "0"), (2, "0"), (5, "0")])?,
        "weeklyOn" => {
            if args.is_empty() || args.len() > 2 {
                return None;
            }
            let day = laravel_days(args.get(..1)?)?;
            at(fields, 1)?;
            splice(fields, 5, day)?;
        }
        "monthly" => fixed(fields, &[(1, "0"), (2, "0"), (3, "1")])?,
        "monthlyOn" => {
            if args.len() > 2 {
                return None;
            }
            let day = int(0, 1).filter(|d| (1..=31).contains(d))?;
            at(fields, 1)?;
            splice(fields, 3, day.to_string())?;
        }
        "twiceMonthly" => {
            if args.len() > 3 {
                return None;
            }
            let (first, second) = (int(0, 1)?, int(1, 16)?);
            at(fields, 2)?;
            splice(fields, 3, format!("{first},{second}"))?;
        }
        "quarterly" => fixed(fields, &[(1, "0"), (2, "0"), (3, "1"), (4, "1-12/3")])?,
        "quarterlyOn" => {
            if args.len() > 2 {
                return None;
            }
            let day = int(0, 1)?;
            at(fields, 1)?;
            splice(fields, 3, day.to_string())?;
            splice(fields, 4, "1-12/3".to_string())?;
        }
        "yearly" => fixed(fields, &[(1, "0"), (2, "0"), (3, "1"), (4, "1")])?,
        "yearlyOn" => {
            if args.len() > 3 {
                return None;
            }
            let (month, day) = (int(0, 1)?, int(1, 1)?);
            at(fields, 2)?;
            splice(fields, 3, day.to_string())?;
            splice(fields, 4, month.to_string())?;
        }
        "weekdays" => fixed(fields, &[(5, "1-5")])?,
        "weekends" => fixed(fields, &[(5, "6,0")])?,
        "sundays" => fixed(fields, &[(5, "0")])?,
        "mondays" => fixed(fields, &[(5, "1")])?,
        "tuesdays" => fixed(fields, &[(5, "2")])?,
        "wednesdays" => fixed(fields, &[(5, "3")])?,
        "thursdays" => fixed(fields, &[(5, "4")])?,
        "fridays" => fixed(fields, &[(5, "5")])?,
        "saturdays" => fixed(fields, &[(5, "6")])?,
        "days" => {
            let days = laravel_days(args)?;
            splice(fields, 5, days)?;
        }
        _ => return None,
    }
    Some(true)
}

/// Set 1-based cron field `pos`.
fn splice(fields: &mut [String], pos: usize, value: String) -> Option<()> {
    *fields.get_mut(pos.checked_sub(1)?)? = value;
    Some(())
}

/// Laravel day-of-week arguments (`->days(..)`, `->weeklyOn(d, ..)`): ints
/// 0–6, `Schedule::MONDAY`-style constants (any class), a literal like
/// `'1-5'`, or an array of those; joined with `,` as Laravel implodes them.
fn laravel_days(args: &[&str]) -> Option<String> {
    const DAYS: [&str; 7] = [
        "SUNDAY",
        "MONDAY",
        "TUESDAY",
        "WEDNESDAY",
        "THURSDAY",
        "FRIDAY",
        "SATURDAY",
    ];
    let mut out: Vec<String> = Vec::new();
    for arg in args {
        if arg.starts_with('[') {
            out.push(laravel_days(&ruby_or_php_list(arg)?)?);
            continue;
        }
        let v = if let Some(s) = string_literal(arg) {
            let ok = !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b',' | b'-' | b'*' | b'/'));
            ok.then_some(s)?
        } else if let Some((_, name)) = arg.rsplit_once("::") {
            DAYS.iter().position(|d| *d == name)?.to_string()
        } else {
            let n: u32 = arg.parse().ok().filter(|n| *n < 7)?;
            n.to_string()
        };
        out.push(v);
    }
    (!out.is_empty()).then(|| out.join(","))
}

// --- Oban ----------------------------------------------------------------------

/// Oban's cron plugin (gate `Oban.Plugins.Cron`): every `crontab: [..]` list,
/// each `{"expr", Mod}` / `{"expr", Mod, opts}` tuple. The expression runs
/// through [`normalise_schedule`] (Oban accepts `@daily` and friends); the
/// target is the full module name and the handler `Bare(<last segment>)` —
/// Elixir `defmodule` PACKAGE nodes are recorded by that short name.
fn extract_oban_crontab(src: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    for (pos, needle) in src.match_indices("crontab:") {
        if ident_before(src, pos).is_some() {
            continue;
        }
        let Some(list) = src
            .get(pos + needle.len()..)
            .and_then(|r| r.trim_start().strip_prefix('['))
        else {
            continue;
        };
        let Some((items, _)) = bracket_items(src, src.len() - list.len(), b']') else {
            continue;
        };
        for item in items {
            let Some(elems) = ruby_hash_items(item) else {
                continue;
            };
            let Some(schedule) = elems
                .first()
                .and_then(|e| string_literal(e))
                .and_then(|raw| normalise_schedule(&raw))
            else {
                continue;
            };
            let Some(module) = elems.get(1).copied() else {
                continue;
            };
            let Some(segs) = ident_path(module) else {
                continue;
            };
            let aliased = segs
                .iter()
                .all(|s| s.starts_with(|c: char| c.is_ascii_uppercase()));
            let Some(short) = segs.last().filter(|_| aliased) else {
                continue;
            };
            out.push(CronJob {
                schedule,
                target: module.to_string(),
                source: "oban",
                handler: at_row(
                    Some(CallQualifier::Bare(short.to_string())),
                    src,
                    offset_in(src, item, pos),
                ),
            });
        }
    }
    out
}

// ----------------------------------------------------------------------------
// Common helpers
// ----------------------------------------------------------------------------

/// `'foo'` / `"foo"` / `foo` — read the first token as a YAML scalar value.
fn first_yaml_string(s: &str) -> Option<String> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();
    let first = bytes[0];
    if first == b'\'' || first == b'"' {
        let delim = first;
        let mut j = 1;
        while j < bytes.len() && bytes[j] != delim {
            j += 1;
        }
        if j < bytes.len() {
            return Some(s[1..j].to_string());
        }
        return None;
    }
    // Bare scalar — take to end of line / comment.
    let end = s
        .find(|c: char| c == '#' || c == '\n')
        .unwrap_or(s.len());
    let v = s[..end].trim().to_string();
    if v.is_empty() { None } else { Some(v) }
}

fn first_list_element(s: &str) -> Option<String> {
    let s = s.trim_start();
    if let Some(rest) = s.strip_prefix('[') {
        let close = rest.find(']')?;
        let first = rest[..close].split(',').next()?;
        let trimmed = first.trim().trim_matches(|c| c == '"' || c == '\'');
        if trimmed.is_empty() {
            return None;
        }
        return Some(trimmed.to_string());
    }
    None
}

/// First quoted string literal in `s` (single, double, or backtick).
fn first_quoted(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' || c == b'"' || c == b'`' {
            let delim = c;
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != delim {
                if bytes[j] == b'\\' && j + 1 < bytes.len() {
                    j += 2;
                } else {
                    j += 1;
                }
            }
            if j < bytes.len() {
                return Some(s[start..j].to_string());
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Locate the second positional arg of `cron.schedule(schedule, handler)` and
/// return a stringified handler name if extractable.
fn handler_after_first_arg(s: &str) -> Option<String> {
    // Skip the schedule string literal; then find the comma that follows it.
    let after_schedule = first_quoted(s)?;
    let _ = after_schedule;
    // Re-scan to find the position past the closing quote.
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b'\'' && bytes[i] != b'"' && bytes[i] != b'`' {
        i += 1;
    }
    if i == bytes.len() {
        return None;
    }
    let delim = bytes[i];
    i += 1;
    while i < bytes.len() && bytes[i] != delim {
        i += 1;
    }
    if i == bytes.len() {
        return None;
    }
    // Past closing quote — find `,` then read identifier.
    let tail = &s[i + 1..];
    let comma = tail.find(',')?;
    let after_comma = tail[comma + 1..].trim_start();
    handler_identifier(after_comma)
}

/// Extract a JS-style handler identifier from `s` — bare name, `obj.method`,
/// or arrow function which we collapse to `anon`. Returns None for `() => ...`.
fn handler_identifier(s: &str) -> Option<String> {
    let s = s.trim_start();
    let bytes = s.as_bytes();
    if bytes.is_empty() {
        return None;
    }
    if bytes[0] == b'(' || bytes[0] == b'{' {
        return Some("anon".to_string());
    }
    let mut j = 0;
    while j < bytes.len()
        && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'.')
    {
        j += 1;
    }
    if j == 0 {
        None
    } else {
        Some(s[..j].to_string())
    }
}

/// Loose check that `s` looks like a 5- or 6-field cron expression. Permits
/// `*`, `*/N`, `N-N`, `N,N`, `?`, `L`, `W` — gates against arbitrary strings
/// matching `cron:` / `cronTime:` keys that aren't actually schedules.
fn looks_like_cron_expr(s: &str) -> bool {
    let parts: Vec<&str> = s.split_whitespace().collect();
    if !(5..=7).contains(&parts.len()) {
        return false;
    }
    parts.iter().all(|p| {
        p.chars().all(|c| {
            c.is_ascii_digit()
                || c == '*'
                || c == '/'
                || c == ','
                || c == '-'
                || c == '?'
                || c == 'L'
                || c == 'W'
                || c == '#'
                || c.is_ascii_alphabetic() // SUN, MON, JAN, etc.
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module_id(repo: RepoId) -> NodeId {
        NodeId::from_parts(GRAPH_TYPE, repo, node_kind::MODULE, "test")
    }

    fn cron_qnames(out: &CronNodes) -> Vec<String> {
        out.nav.qname_by_id.values().cloned().collect()
    }

    fn sorted_qnames(out: &CronNodes) -> Vec<String> {
        let mut q = cron_qnames(out);
        q.sort();
        q
    }

    /// `(job qname, handler)` per HANDLED_BY ref, sorted by qname.
    fn handlers(out: &CronNodes) -> Vec<(String, CallQualifier)> {
        let mut v: Vec<(String, CallQualifier)> = out
            .refs
            .iter()
            .map(|r| (out.nav.qname_by_id[&r.from].clone(), r.qualifier.clone()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    fn bare(name: &str) -> CallQualifier {
        CallQualifier::Bare(name.to_string())
    }

    fn attr(base: &str, name: &str) -> CallQualifier {
        CallQualifier::Attribute {
            base: base.to_string(),
            name: name.to_string(),
        }
    }

    fn run(src: &str, path: &str) -> CronNodes {
        let repo = RepoId(1);
        extract_cron_nodes(src, path, module_id(repo), repo)
    }

    #[test]
    fn github_actions_workflow_cron() {
        let repo = RepoId(1);
        let src = r#"
name: Nightly
on:
  schedule:
    - cron: '0 4 * * *'
  push:
    branches: [main]
jobs:
  build:
    runs-on: ubuntu-latest
"#;
        let out = extract_cron_nodes(
            src,
            ".github/workflows/nightly.yml",
            module_id(repo),
            repo,
        );
        let qnames = cron_qnames(&out);
        assert!(qnames.contains(&"cron:0 4 * * *:nightly".to_string()));
    }

    #[test]
    fn github_actions_only_when_in_workflows_path() {
        // Same content, wrong path → no emit.
        let repo = RepoId(1);
        let src = "schedule:\n  - cron: '0 4 * * *'";
        let out = extract_cron_nodes(src, "docs/example.yml", module_id(repo), repo);
        assert!(out.nodes.is_empty(), "GHA cron only inside .github/workflows/");
    }

    #[test]
    fn k8s_cronjob_yaml() {
        let repo = RepoId(1);
        let src = r#"
apiVersion: batch/v1
kind: CronJob
metadata:
  name: nightly-cleanup
spec:
  schedule: "0 2 * * *"
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: cleanup
            image: registry.example.com/ops/cleanup:1.4
            command: ["/usr/local/bin/cleanup", "--all"]
"#;
        let out = extract_cron_nodes(src, "k8s/cronjobs.yaml", module_id(repo), repo);
        let qnames = cron_qnames(&out);
        // Target preference: command basename (`cleanup`) over image basename.
        assert!(
            qnames.contains(&"cron:0 2 * * *:cleanup".to_string()),
            "qnames = {:?}",
            qnames
        );
    }

    #[test]
    fn k8s_falls_back_to_image_basename_when_no_command() {
        let repo = RepoId(1);
        let src = r#"
kind: CronJob
spec:
  schedule: "*/15 * * * *"
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - image: ghcr.io/example/poller:latest
"#;
        let out = extract_cron_nodes(src, "k8s/poller.yaml", module_id(repo), repo);
        let qnames = cron_qnames(&out);
        assert!(qnames.contains(&"cron:*/15 * * * *:poller".to_string()));
    }

    #[test]
    fn k8s_multidoc_emits_one_job_per_document() {
        let repo = RepoId(1);
        // Three documents. The trailing Deployment is the trap: before the
        // per-document split its `command:` overwrote the last CronJob's and
        // the file yielded ONE node `cron:0 6 * * 1:server`.
        let src = r#"
apiVersion: batch/v1
kind: CronJob
metadata:
  name: nightly-cleanup
spec:
  schedule: "0 2 * * *"
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: cleanup
            image: ghcr.io/example/cleanup:1.4
            command: ["/usr/local/bin/cleanup", "--all"]
---
apiVersion: batch/v1
kind: CronJob
metadata:
  name: weekly-report
spec:
  schedule: "0 6 * * 1"
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - name: report
            image: ghcr.io/example/report:2.1
            command: ["/app/report"]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
spec:
  template:
    spec:
      containers:
      - name: api
        image: ghcr.io/example/api:1.0
        command: ["/app/server"]
"#;
        let out = extract_cron_nodes(src, "k8s/cronjobs.yaml", module_id(repo), repo);
        let mut qnames = cron_qnames(&out);
        qnames.sort();
        assert_eq!(
            qnames,
            vec![
                "cron:0 2 * * *:cleanup".to_string(),
                "cron:0 6 * * 1:report".to_string(),
            ],
            "one job per CronJob document, no cross-document bleed"
        );
        assert!(
            !qnames.iter().any(|q| q.ends_with(":server")),
            "the Deployment's command must never become a CronJob target: {qnames:?}"
        );
        // Module -> job edge for each.
        assert_eq!(out.edges.len(), 2, "one SCHEDULES edge per job");
    }

    #[test]
    fn k8s_cronjob_kind_tolerates_quotes_and_extra_space() {
        let repo = RepoId(1);
        let src = r#"
kind:   "CronJob"
spec:
  schedule: "30 3 * * *"
  jobTemplate:
    spec:
      template:
        spec:
          containers:
          - image: ghcr.io/example/vacuum:latest
"#;
        let out = extract_cron_nodes(src, "k8s/vacuum.yaml", module_id(repo), repo);
        let qnames = cron_qnames(&out);
        assert!(
            qnames.contains(&"cron:30 3 * * *:vacuum".to_string()),
            "quoted / extra-spaced `kind:` must still gate in: {qnames:?}"
        );
    }

    #[test]
    fn node_cron_method_form() {
        let repo = RepoId(1);
        let src = r#"
import cron from 'node-cron';
cron.schedule('*/5 * * * *', cleanupSessions);
cron.schedule('0 0 * * 0', () => weeklyDigest());
"#;
        let out = extract_cron_nodes(src, "src/jobs.ts", module_id(repo), repo);
        let qnames = cron_qnames(&out);
        assert!(qnames.contains(&"cron:*/5 * * * *:cleanupSessions".to_string()));
        assert!(qnames.contains(&"cron:0 0 * * 0:anon".to_string()));
    }

    #[test]
    fn node_cron_class_form() {
        let repo = RepoId(1);
        let src = r#"
import { CronJob } from 'cron';
const job = new CronJob({
    cronTime: '0 12 * * *',
    onTick: dailyReport,
});
"#;
        let out = extract_cron_nodes(src, "src/jobs.ts", module_id(repo), repo);
        let qnames = cron_qnames(&out);
        assert!(qnames.contains(&"cron:0 12 * * *:dailyReport".to_string()));
    }

    #[test]
    fn celery_beat_schedule_with_task() {
        let repo = RepoId(1);
        let src = r#"
app.conf.beat_schedule = {
    'cleanup-sessions': {
        'task': 'tasks.cleanup_sessions',
        'schedule': crontab(minute=0, hour=4),
    },
    'rebuild-cache': {
        'task': 'tasks.rebuild_cache',
        'schedule': 30.0,
    },
}
"#;
        let out = extract_cron_nodes(src, "app/celery_config.py", module_id(repo), repo);
        let qnames = cron_qnames(&out);
        assert!(
            qnames
                .iter()
                .any(|q| q.starts_with("cron:crontab(minute=0, hour=4):")
                    && q.ends_with("tasks.cleanup_sessions"))
        );
        assert!(
            qnames
                .iter()
                .any(|q| q.starts_with("cron:30.0:") && q.ends_with("tasks.rebuild_cache"))
        );
    }

    #[test]
    fn java_scheduled_cron_attribute() {
        let repo = RepoId(1);
        let src = r#"
@Component
public class NightlyJob {
    @Scheduled(cron = "0 0 4 * * *")
    public void runCleanup() {}

    @Scheduled(cron="*/15 * * * *", zone = "UTC")
    public void poll() {}
}
"#;
        let out = extract_cron_nodes(src, "src/main/java/NightlyJob.java", module_id(repo), repo);
        let qnames = cron_qnames(&out);
        assert!(qnames.contains(&"cron:0 0 4 * * *:runCleanup".to_string()));
        assert!(qnames.contains(&"cron:*/15 * * * *:poll".to_string()));
        // LA.19a: each annotated method is its job's handler, scoped by class.
        assert_eq!(
            handlers(&out),
            vec![
                (
                    "cron:*/15 * * * *:poll".to_string(),
                    attr("NightlyJob", "poll")
                ),
                (
                    "cron:0 0 4 * * *:runCleanup".to_string(),
                    attr("NightlyJob", "runCleanup")
                ),
            ]
        );
        assert!(
            out.refs
                .iter()
                .all(|r| r.category == edge_category::HANDLED_BY)
        );
    }

    #[test]
    fn rejects_strings_that_arent_cron_exprs() {
        // `cron:` key with a non-schedule value (e.g. "false") shouldn't emit.
        let repo = RepoId(1);
        let src = r#"
on:
  schedule:
    - cron: 'false'
"#;
        let out = extract_cron_nodes(
            src,
            ".github/workflows/x.yml",
            module_id(repo),
            repo,
        );
        assert!(out.nodes.is_empty(), "non-schedule string must not emit");
    }

    #[test]
    fn dedupes_within_file() {
        let repo = RepoId(1);
        let src = r#"
cron.schedule('*/5 * * * *', cleanupSessions);
cron.schedule('*/5 * * * *', cleanupSessions);
"#;
        let out = extract_cron_nodes(src, "src/jobs.ts", module_id(repo), repo);
        assert_eq!(out.nodes.len(), 1, "duplicate (schedule, target) collapses");
    }

    #[test]
    fn node_cron_window_cut_inside_a_multibyte_char() {
        // U+1F600 starts at `cronTime:` + 255, so the 256-byte `onTick:`
        // window ends on its 2nd byte (panicked before LA.25a).
        let repo = RepoId(1);
        let head = "const job = new CronJob({ cronTime: '*/5 * * * *', onTick: tick, /* ";
        let pos = head.find("cronTime:").unwrap();
        let at = pos + 255;
        let src = format!("{head}{}\u{1F600} */ }});\n", "x".repeat(at - head.len()));
        assert!(!src.is_char_boundary(pos + 256));
        let out = extract_cron_nodes(&src, "src/jobs.ts", module_id(repo), repo);
        let qnames = cron_qnames(&out);
        assert!(
            qnames.contains(&"cron:*/5 * * * *:tick".to_string()),
            "{qnames:?}"
        );
    }

    #[test]
    fn celery_lookback_cut_inside_a_multibyte_char() {
        // U+1F600 starts at `'schedule':` - 257, so the 256-byte look-back
        // starts on its 2nd byte (panicked before LA.25a). The `'task':`
        // sibling sits inside the look-back and must still be read.
        let repo = RepoId(1);
        for (task, schedule) in [("'task':", "'schedule':"), ("\"task\":", "\"schedule\":")] {
            let head = "app.conf.beat_schedule = {\n    'cleanup': {  # \u{1F600}";
            let body = format!(
                "\n        {task} 'tasks.cleanup',\n        {schedule} crontab(minute=0),\n    }},\n}}\n"
            );
            let char_at = head.len() - 4;
            let pad = (char_at + 257)
                .checked_sub(head.len() + body.find(schedule).unwrap())
                .unwrap();
            let src = format!("{head}{}{body}", "x".repeat(pad));
            let pos = src.find(schedule).unwrap();
            assert_eq!(pos - 257, char_at);
            assert!(!src.is_char_boundary(pos - 256));
            let out = extract_cron_nodes(&src, "app/celery_config.py", module_id(repo), repo);
            let qnames = cron_qnames(&out);
            assert!(
                qnames.contains(&"cron:crontab(minute=0):tasks.cleanup".to_string()),
                "{schedule}: {qnames:?}"
            );
        }
    }
    // ---- CJ.1c: literal / comment guard -----------------------------------

    #[test]
    fn literal_schedules_mint_nothing_in_rust() {
        let src = "fn t() {\n    let s = \"cron.schedule('*/5 * * * *', tick);\";\n    let c = \"new CronJob({ cronTime: '0 12 * * *', onTick: report })\";\n}\n// @Scheduled(cron = \"0 4 * * * *\")\nfn job() {}\n";
        assert!(run(src, "src/x.rs").nodes.is_empty(), "{:?}", sorted_qnames(&run(src, "src/x.rs")));
        // The same text in a JS / Java file reads as at HEAD.
        let js = sorted_qnames(&run(src, "src/x.js"));
        assert!(js.contains(&"cron:*/5 * * * *:tick".to_string()), "{js:?}");
        assert!(js.contains(&"cron:0 12 * * *:report".to_string()), "{js:?}");
        let java = sorted_qnames(&run(src, "src/X.java"));
        assert!(java.contains(&"cron:0 4 * * * *:job".to_string()), "{java:?}");
        // A real call in Rust code after a refused one is still read.
        let mixed = "// cron.schedule('0 0 * * 0', old);\ncron.schedule('*/5 * * * *', tick);\n";
        assert_eq!(sorted_qnames(&run(mixed, "src/x.rs")), ["cron:*/5 * * * *:tick"]);
    }

    #[test]
    fn celery_beat_key_must_open_its_literal() {
        // A real Python dict: the key opens its own literal.
        let single = "BEAT = {\n    'c': {'task': 'tasks.cleanup', 'schedule': crontab(minute=0)},\n}\n";
        assert_eq!(
            sorted_qnames(&run(single, "app/x.py")),
            ["cron:crontab(minute=0):tasks.cleanup"]
        );
        let double = "BEAT = {\n    \"t\": {\"task\": \"tasks.tick\", \"schedule\": EVERY_MINUTE},\n}\n";
        assert_eq!(sorted_qnames(&run(double, "app/x.py")), ["cron:EVERY_MINUTE:tasks.tick"]);
        // A key inside a larger Python string or a `#` comment is no entry.
        for src in [
            "S = \"{'task': 'tasks.cleanup', 'schedule': crontab()}\"\n",
            "# 'task': 'tasks.cleanup', 'schedule': crontab()\n",
            "S = '{\"task\": \"tasks.cleanup\", \"schedule\": crontab()}'\n",
        ] {
            assert!(run(src, "app/x.py").nodes.is_empty(), "{src:?}");
        }
        // Rust: a key inside a string is held by it; the same text in a TS
        // file reads as at HEAD.
        let rs = "let s = \"{'task': 'tasks.cleanup', 'schedule': crontab()}\";\nlet r = r#\"{\"schedule\": \"* * * * *\"}\"#;\n";
        assert!(run(rs, "src/x.rs").nodes.is_empty(), "{:?}", sorted_qnames(&run(rs, "src/x.rs")));
        let ts = sorted_qnames(&run(rs, "src/x.ts"));
        assert!(ts.contains(&"cron:crontab():tasks.cleanup".to_string()), "{ts:?}");
    }

    // ---- LA.19a: framework schedulers ------------------------------------

    #[test]
    fn normalise_schedule_descriptors_and_verbatim() {
        for (raw, want) in [
            ("@yearly", Some("0 0 1 1 *")),
            ("@annually", Some("0 0 1 1 *")),
            ("@monthly", Some("0 0 1 * *")),
            ("@weekly", Some("0 0 * * 0")),
            ("@daily", Some("0 0 * * *")),
            ("@midnight", Some("0 0 * * *")),
            ("@hourly", Some("0 * * * *")),
            ("@every 1h30m", Some("@every 1h30m")),
            ("@reboot", Some("@reboot")),
            ("0 0/15 * * * ?", Some("0 0/15 * * * ?")),
            (" 15 3 * * * ", Some("15 3 * * *")),
            ("@every ", None),
            ("@sometimes", None),
            ("not a schedule", None),
        ] {
            assert_eq!(normalise_schedule(raw).as_deref(), want, "{raw:?}");
        }
    }

    #[test]
    fn quartz_single_job_class_is_the_handler() {
        let src = r#"
import static org.quartz.CronScheduleBuilder.cronSchedule;
import org.quartz.*;
public class Jobs {
    void schedule(Scheduler s) {
        JobDetail job = JobBuilder.newJob(ReportJob.class).withIdentity("report").build();
        Trigger t = TriggerBuilder.newTrigger().withSchedule(cronSchedule("0 0/15 * * * ?")).build();
        Trigger u = newTrigger().withSchedule(CronScheduleBuilder.cronSchedule("0 0 12 ? * MON-FRI")).build();
    }
}
"#;
        let out = run(src, "src/Jobs.java");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:0 0 12 ? * MON-FRI:ReportJob",
                "cron:0 0/15 * * * ?:ReportJob"
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                (
                    "cron:0 0 12 ? * MON-FRI:ReportJob".to_string(),
                    bare("ReportJob")
                ),
                (
                    "cron:0 0/15 * * * ?:ReportJob".to_string(),
                    bare("ReportJob")
                ),
            ]
        );
    }

    #[test]
    fn quartz_many_job_classes_fall_back_to_trigger_identity() {
        let src = r#"
import org.quartz.*;
class Jobs {
    void schedule(Scheduler s) {
        JobDetail a = newJob(ReportJob.class).build();
        JobDetail b = newJob(PurgeJob.class).build();
        Trigger t = newTrigger().withIdentity("nightly-report").withSchedule(cronSchedule("0 0 2 * * ?")).build();
        CronExpression e = new CronExpression("0 30 6 * * ?");
    }
}
"#;
        let out = run(src, "src/Jobs.java");
        assert_eq!(
            sorted_qnames(&out),
            vec!["cron:0 0 2 * * ?:nightly-report", "cron:0 30 6 * * ?:anon"]
        );
        assert!(out.refs.is_empty(), "no single job class, no handler");
    }

    #[test]
    fn quartz_needs_its_import_and_a_java_file() {
        let src = "Trigger t = newTrigger().withSchedule(cronSchedule(\"0 0 2 * * ?\")).build();";
        assert!(
            run(src, "src/Jobs.java").nodes.is_empty(),
            "no org.quartz import"
        );
        let gated = format!("import org.quartz.*;\n{src}");
        assert!(
            run(&gated, "src/jobs.ts").nodes.is_empty(),
            "not a JVM file"
        );
        assert_eq!(run(&gated, "src/Jobs.java").nodes.len(), 1);
    }

    #[test]
    fn hangfire_recurring_jobs_and_handlers() {
        let src = r#"
using Hangfire;
public class Startup {
    public void Configure(IRecurringJobManager manager) {
        RecurringJob.AddOrUpdate<IInvoiceService>("invoices", x => x.SendReminders(), Cron.Daily);
        RecurringJob.AddOrUpdate("cleanup", () => Cleaner.Run(), "*/10 * * * *");
        RecurringJob.AddOrUpdate(() => Reports.Build(), Cron.Daily(3, 15));
        manager.AddOrUpdate("sync", () => Sync.Pull(), Cron.Hourly());
        _recurringJobManager.AddOrUpdate<Repo<Order>>("orders", r => r.Flush(), Cron.Weekly());
        RecurringJob.AddOrUpdate("async", async () => await Mailer.SendAsync(), Cron.Daily(hour: 4));
        RecurringJob.AddOrUpdate("never", () => Cleaner.Run(), Cron.Never());
        cache.AddOrUpdate("key", k => k.Value(), "0 0 * * *");
    }
}
"#;
        let out = run(src, "Startup.cs");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:*/10 * * * *:Run",
                "cron:0 * * * *:Pull",
                "cron:0 0 * * *:SendReminders",
                "cron:0 0 * * 1:Flush",
                "cron:0 4 * * *:SendAsync",
                "cron:15 3 * * *:Build",
            ],
            "Cron.Never() is skipped; a non-Hangfire receiver never fires"
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cron:*/10 * * * *:Run".to_string(), attr("Cleaner", "Run")),
                ("cron:0 * * * *:Pull".to_string(), attr("Sync", "Pull")),
                (
                    "cron:0 0 * * *:SendReminders".to_string(),
                    attr("IInvoiceService", "SendReminders")
                ),
                ("cron:0 0 * * 1:Flush".to_string(), attr("Repo", "Flush")),
                (
                    "cron:0 4 * * *:SendAsync".to_string(),
                    attr("Mailer", "SendAsync")
                ),
                (
                    "cron:15 3 * * *:Build".to_string(),
                    attr("Reports", "Build")
                ),
            ]
        );
    }

    #[test]
    fn hangfire_cron_helpers_follow_hangfire() {
        for (member, args, want) in [
            ("Minutely", &[][..], Some("* * * * *")),
            ("Hourly", &[][..], Some("0 * * * *")),
            ("Hourly", &["5"][..], Some("5 * * * *")),
            ("Daily", &[][..], Some("0 0 * * *")),
            ("Daily", &["3"][..], Some("0 3 * * *")),
            ("Daily", &["3", "15"][..], Some("15 3 * * *")),
            ("Daily", &["minute: 30"][..], Some("30 0 * * *")),
            ("Weekly", &[][..], Some("0 0 * * 1")),
            (
                "Weekly",
                &["DayOfWeek.Friday", "18"][..],
                Some("0 18 * * 5"),
            ),
            ("Monthly", &[][..], Some("0 0 1 * *")),
            ("Monthly", &["15", "6"][..], Some("0 6 15 * *")),
            ("Yearly", &[][..], Some("0 0 1 1 *")),
            ("Yearly", &["6", "2", "9", "45"][..], Some("45 9 2 6 *")),
            ("Daily", &["hourOfDay"][..], None),
            ("Daily", &["1", "2", "3"][..], None),
            ("MinuteInterval", &["5"][..], None),
        ] {
            assert_eq!(
                hangfire_cron(member, args).as_deref(),
                want,
                "{member}{args:?}"
            );
        }
    }

    #[test]
    fn hangfire_needs_its_namespace() {
        let src = "RecurringJob.AddOrUpdate(\"c\", () => Cleaner.Run(), \"*/10 * * * *\");";
        assert!(run(src, "Startup.cs").nodes.is_empty());
        let gated = format!("using Hangfire;\n{src}");
        assert_eq!(run(&gated, "Startup.cs").nodes.len(), 1);
    }

    #[test]
    fn robfig_add_func_and_add_job() {
        let src = r#"
package main

import "github.com/robfig/cron/v3"

func main() {
	c := cron.New()
	c.AddFunc("15 3 * * *", sweep)
	c.AddFunc("0 * * * *", jobs.Cleanup)
	c.AddFunc("@hourly", h.Refresh)
	c.AddFunc("@every 5m", func() { log.Println("tick") })
	c.AddJob("0 6 * * 1", &Reporter{})
	c.AddJob("30 6 * * *", cron.FuncJob(rotate))
	c.AddJob("45 6 * * *", job)
}
"#;
        let out = run(src, "cmd/worker.go");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:0 * * * *:Cleanup",
                "cron:0 * * * *:Refresh",
                "cron:0 6 * * 1:Reporter",
                "cron:15 3 * * *:sweep",
                "cron:30 6 * * *:rotate",
                "cron:45 6 * * *:job",
                "cron:@every 5m:anon",
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                (
                    "cron:0 * * * *:Cleanup".to_string(),
                    attr("jobs", "Cleanup")
                ),
                ("cron:0 * * * *:Refresh".to_string(), attr("h", "Refresh")),
                (
                    "cron:0 6 * * 1:Reporter".to_string(),
                    attr("Reporter", "Run")
                ),
                ("cron:15 3 * * *:sweep".to_string(), bare("sweep")),
                ("cron:30 6 * * *:rotate".to_string(), bare("rotate")),
            ],
            "a func literal and a plain job variable bind nothing"
        );
    }

    #[test]
    fn gocron_v1_chain_and_v2_job() {
        let src = r#"
package main

import "github.com/go-co-op/gocron/v2"

func main() {
	s.Cron("*/1 * * * *").Tag("t").Do(task)
	s.CronWithSeconds("0 */5 * * * *").Do(svc.Poll)
	s.NewJob(gocron.CronJob("0 2 * * *", false), gocron.NewTask(nightly, 1))
}
"#;
        let out = run(src, "main.go");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:*/1 * * * *:task",
                "cron:0 */5 * * * *:Poll",
                "cron:0 2 * * *:nightly"
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cron:*/1 * * * *:task".to_string(), bare("task")),
                ("cron:0 */5 * * * *:Poll".to_string(), attr("svc", "Poll")),
                ("cron:0 2 * * *:nightly".to_string(), bare("nightly")),
            ]
        );
    }

    #[test]
    fn go_add_func_without_robfig_emits_nothing() {
        let src = "package main\n\nfunc main() {\n\tc.AddFunc(\"15 3 * * *\", sweep)\n}\n";
        let out = run(src, "main.go");
        assert!(out.nodes.is_empty() && out.refs.is_empty());
    }

    #[test]
    fn apscheduler_decorator_and_add_job() {
        let src = r#"
from apscheduler.schedulers.blocking import BlockingScheduler
from apscheduler.triggers.cron import CronTrigger

sched = BlockingScheduler()


@sched.scheduled_job("cron", hour=3, minute=15)
def nightly():
    pass


@sched.scheduled_job('interval', minutes=5)
@traced
async def poll():
    pass


sched.add_job(rollup, CronTrigger.from_crontab("0 6 * * mon"))
sched.add_job(tasks.purge, "cron", day_of_week="0-4", hour=1)
sched.add_job("app.jobs:digest", trigger=CronTrigger(month=6, timezone="UTC"))
sched.add_job(func=rebuild, trigger="cron", hour=cfg.hour)
sched.add_job(lambda: None, "cron", second=30, minute="*/2")
"#;
        let out = run(src, "app/sched.py");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:0 0 1 6 *:digest",
                "cron:0 1 * * mon-fri:purge",
                "cron:0 6 * * mon:rollup",
                "cron:15 3 * * *:nightly",
                "cron:30 */2 * * * *:anon",
                "cron:@every 5m:poll",
            ],
            "a non-literal field value skips the job"
        );
        assert_eq!(
            handlers(&out),
            vec![
                ("cron:0 0 1 6 *:digest".to_string(), bare("digest")),
                (
                    "cron:0 1 * * mon-fri:purge".to_string(),
                    attr("tasks", "purge")
                ),
                ("cron:0 6 * * mon:rollup".to_string(), bare("rollup")),
                ("cron:15 3 * * *:nightly".to_string(), bare("nightly")),
                ("cron:@every 5m:poll".to_string(), bare("poll")),
            ]
        );
    }

    #[test]
    fn apscheduler_kwargs_follow_the_default_rule() {
        let kw = |pairs: &[(&'static str, &'static str)]| pairs.to_vec();
        for (pairs, want) in [
            (kw(&[("hour", "3")]), Some("0 3 * * *")),
            (kw(&[("hour", "3"), ("minute", "15")]), Some("15 3 * * *")),
            (kw(&[("day_of_week", "'mon'")]), Some("0 0 * * mon")),
            (kw(&[("day_of_week", "6")]), Some("0 0 * * sun")),
            (kw(&[("minute", "'*/10'")]), Some("*/10 * * * *")),
            (kw(&[("second", "30")]), Some("30 * * * * *")),
            // `minute` is coarser than the last given field (`second`): `*`.
            (kw(&[("hour", "2"), ("second", "0")]), Some("* 2 * * *")),
            (kw(&[("day", "1"), ("id", "'x'")]), Some("0 0 1 * *")),
            (kw(&[("year", "2027")]), None),
            (kw(&[("week", "2")]), None),
            (kw(&[("hour", "h")]), None),
        ] {
            assert_eq!(aps_cron(&pairs).as_deref(), want, "{pairs:?}");
        }
        assert_eq!(
            aps_interval(&[("hours", "1"), ("minutes", "30")]).as_deref(),
            Some("@every 1h30m")
        );
        assert_eq!(aps_interval(&[("minutes", "n")]), None);
    }

    #[test]
    fn apscheduler_needs_its_import() {
        let src = "@sched.scheduled_job(\"cron\", hour=3)\ndef nightly():\n    pass\n";
        assert!(run(src, "sched.py").nodes.is_empty());
        let gated = format!("from apscheduler.schedulers.blocking import BlockingScheduler\n{src}");
        assert_eq!(run(&gated, "sched.py").nodes.len(), 1);
    }

    #[test]
    fn yaml_jobs_carry_no_handler_refs() {
        let src = "kind: CronJob\nspec:\n  schedule: \"0 * * * *\"\n  jobTemplate:\n    spec:\n      template:\n        spec:\n          containers:\n          - image: example/cleanup:1\n";
        let out = run(src, "k8s/cleanup.yaml");
        assert_eq!(out.nodes.len(), 1);
        assert!(out.refs.is_empty());
    }

    #[test]
    fn code_marker_counts_code_sources_only() {
        let mut c = CodeCounts::default();
        assert_eq!(
            code_marker(&c, 0, "a.ts"),
            None,
            "no code-sourced job, no marker"
        );
        c.bump("node_cron");
        c.bump("k8s_cronjob");
        assert_eq!(code_marker(&c, 0, "a.ts"), None);
        c.bump("quartz");
        assert_eq!(
            code_marker(&c, 1, "Jobs.java").as_deref(),
            Some(
                "[cron] code jobs=1 quartz=1 hangfire=0 go=0 apscheduler=0 spring=0 nestjs=0 handler_refs=1 path=Jobs.java"
            )
        );
        c.bump("robfig");
        c.bump("gocron");
        c.bump("scheduled_annot");
        assert_eq!(
            code_marker(&c, 3, "w.go").as_deref(),
            Some(
                "[cron] code jobs=4 quartz=1 hangfire=0 go=2 apscheduler=0 spring=1 nestjs=0 handler_refs=3 path=w.go"
            )
        );
    }

    #[test]
    fn enclosing_class_skips_prose_and_subclass() {
        let before =
            "@Component\npublic class Cleanup {\n    // this class handles the subclass case\n    ";
        assert_eq!(enclosing_class_name(before).as_deref(), Some("Cleanup"));
        assert_eq!(enclosing_class_name("// no class here\n"), None);
    }

    /// The matrix/typescript/cron probe file (CL.9).
    const NEST_TASKS: &str = "import { Injectable } from \"@nestjs/common\";\nimport { Cron } from \"@nestjs/schedule\";\n\n@Injectable()\nexport class TasksService {\n  @Cron(\"0 3 * * *\")\n  purge() {\n    return 0;\n  }\n}\n";

    #[test]
    fn nest_cron_literal_names_its_handler() {
        let out = run(NEST_TASKS, "src/tasks.service.ts");
        assert_eq!(sorted_qnames(&out), vec!["cron:0 3 * * *:purge"]);
        assert_eq!(
            handlers(&out),
            vec![(
                "cron:0 3 * * *:purge".to_string(),
                attr("TasksService", "purge")
            )]
        );
        // The ref sits at the decorator's row (0-based), the LC.3b site.
        assert_eq!(out.refs[0].line, 5);
        assert_eq!(out.edges.len(), 1, "one SCHEDULES edge module -> job");
        assert_eq!(out.edges[0].category, edge_category::SCHEDULES);
        let mut c = CodeCounts::default();
        c.bump("nestjs");
        assert_eq!(
            code_marker(&c, 1, "src/tasks.service.ts").as_deref(),
            Some(
                "[cron] code jobs=1 quartz=0 hangfire=0 go=0 apscheduler=0 spring=0 nestjs=1 handler_refs=1 path=src/tasks.service.ts"
            )
        );
        // A 6-field NestJS expression (seconds first), options, stacked
        // decorators and an `async` method with a return type.
        let src = "import { Cron } from '@nestjs/schedule';\nexport default class Reports {\n  @Cron('45 */5 * * * *', {\n    name: 'digest',\n    timeZone: 'Europe/Paris',\n  })\n  @UseGuards(LockGuard)\n  @Timed()\n  async sendDigest(): Promise<void> {}\n}\n";
        let out = run(src, "reports.ts");
        assert_eq!(
            handlers(&out),
            vec![(
                "cron:45 */5 * * * *:sendDigest".to_string(),
                attr("Reports", "sendDigest")
            )]
        );
    }

    #[test]
    fn nest_cron_expression_member_verbatim() {
        let src = "import { Cron, CronExpression } from '@nestjs/schedule';\n@Injectable()\nexport class Cleanup {\n  @Cron(CronExpression.EVERY_DAY_AT_3AM)\n  sweep() {}\n\n  @Cron(SCHEDULE)\n  computed() {}\n\n  @Cron(new Date(Date.now() + 1000))\n  once() {}\n\n  @Cron('not a schedule')\n  prose() {}\n}\n";
        let out = run(src, "cleanup.service.ts");
        assert_eq!(
            handlers(&out),
            vec![(
                "cron:CronExpression.EVERY_DAY_AT_3AM:sweep".to_string(),
                attr("Cleanup", "sweep")
            )],
            "only the enum member is a schedule identity; a variable, a Date and prose are not"
        );
    }

    #[test]
    fn nest_interval_is_a_rate() {
        let src = "import { Interval, Timeout } from '@nestjs/schedule';\nexport class Poller {\n  @Interval(5000)\n  poll() {}\n\n  @Interval('heartbeat', 10_000)\n  beat() {}\n\n  @Interval(60 * 1000)\n  computed() {}\n\n  @Timeout(3000)\n  warmUp() {}\n}\n";
        let out = run(src, "poller.js");
        assert_eq!(
            handlers(&out),
            vec![
                (
                    "cron:@every 10000ms:beat".to_string(),
                    attr("Poller", "beat")
                ),
                (
                    "cron:@every 5000ms:poll".to_string(),
                    attr("Poller", "poll")
                ),
            ],
            "a literal interval is a rate; an expression and a one-shot @Timeout are not"
        );
    }

    #[test]
    fn nest_cron_needs_the_schedule_import() {
        let body = "export class TasksService {\n  @Cron(\"0 3 * * *\")\n  purge() {}\n}\n";
        assert!(
            run(body, "src/tasks.service.ts").nodes.is_empty(),
            "no @nestjs/schedule import"
        );
        let gated = format!("import {{ Cron }} from '@nestjs/schedule';\n{body}");
        assert!(
            run(&gated, "tasks.py").nodes.is_empty(),
            "not a TS / JS file"
        );
        assert_eq!(run(&gated, "src/tasks.service.mts").nodes.len(), 1);
        // A commented-out decorator schedules nothing.
        let commented = "import { Cron } from '@nestjs/schedule';\nexport class TasksService {\n  // @Cron('0 3 * * *')\n  /** @Cron('0 4 * * *') */\n  /*\n   * @Cron('0 5 * * *')\n   */\n  purge() {}\n}\n";
        assert!(run(commented, "src/tasks.service.ts").nodes.is_empty());
    }

    #[test]
    fn export_class_is_an_enclosing_class() {
        assert_eq!(
            enclosing_class_name("export default class Reports {\n  ").as_deref(),
            Some("Reports")
        );
        assert_eq!(
            enclosing_class_name("@Injectable()\nexport class TasksService {\n  ").as_deref(),
            Some("TasksService")
        );
        assert_eq!(
            enclosing_class_name("@Injectable() export abstract class Base {\n  ").as_deref(),
            Some("Base")
        );
        assert_eq!(enclosing_class_name("// export the class below\n"), None);
    }

    #[test]
    fn stacked_annotations_are_skipped_before_the_method() {
        // Spring with ShedLock: the method, not the second annotation, runs.
        let src = "@Component\npublic class Jobs {\n    @Scheduled(cron = \"0 0 4 * * *\")\n    @SchedulerLock(name = \"purge\",\n        lockAtMostFor = \"10m\")\n    public void purge() {}\n}\n";
        let out = run(src, "Jobs.java");
        assert_eq!(
            handlers(&out),
            vec![("cron:0 0 4 * * *:purge".to_string(), attr("Jobs", "purge"))]
        );
        assert_eq!(
            method_name_after_annotation("\n  @A() @B.c(1, (2))\n  @D\n  run(): void {}")
                .as_deref(),
            Some("run")
        );
    }

    #[test]
    fn framework_scans_survive_multibyte_text() {
        // Every reader slices through `get(..)` at ASCII delimiters, so a
        // 4-byte char anywhere near a needle is inert.
        let wide = "\u{1F600}";
        for (src, path) in [
            (
                format!(
                    "import org.quartz.*; // {wide}\nx(cronSchedule(\"{wide}\")); newJob({wide}.class); cronSchedule(\"0 0 2 * * ?\"{wide}"
                ),
                "J.java",
            ),
            (
                format!(
                    "using Hangfire; RecurringJob.AddOrUpdate<{wide}>(\"{wide}\", () => {wide}.Run(), Cron.Daily({wide})); RecurringJob.AddOrUpdate(\"x\", () => A.B(), \"{wide}"
                ),
                "S.cs",
            ),
            (
                format!(
                    "import \"github.com/robfig/cron/v3\"\nc.AddFunc(\"{wide}\", {wide})\nc.AddJob(\"0 * * * *\", &{wide}{{}})\nc.AddFunc(\"0 * * * *\""
                ),
                "w.go",
            ),
            (
                format!(
                    "import apscheduler\n@s.scheduled_job(\"cron\", hour='{wide}')\ndef {wide}(): pass\ns.add_job({wide}, CronTrigger.from_crontab(\"{wide}\"))\ns.add_job(f, \"cron\", hour=3"
                ),
                "s.py",
            ),
        ] {
            let _ = run(&src, path);
        }
    }

    // ---- LA.19b: whenever / sidekiq-cron / Laravel / Oban -------------------

    #[test]
    fn whenever_every_macro_table() {
        for (freq, at, want) in [
            ("'0 0 27-31 * *'", None, Some("0 0 27-31 * *")),
            ("\"@reboot\"", None, Some("@reboot")),
            ("1.minute", None, Some("* * * * *")),
            ("5.minutes", None, Some("*/5 * * * *")),
            ("1.hour", None, Some("0 * * * *")),
            ("3.hours", None, Some("0 */3 * * *")),
            ("2.hours", Some("15"), Some("15 */2 * * *")),
            ("1.day", None, Some("0 0 * * *")),
            ("1.day", Some("'4:30 am'"), Some("30 4 * * *")),
            (":day", Some("'16:00'"), Some("0 16 * * *")),
            ("2.days", Some("\"4pm\""), Some("0 16 */2 * *")),
            (":monday", Some("'12pm'"), Some("0 12 * * 1")),
            (":sunday", None, Some("0 0 * * 0")),
            (":weekday", Some("'9:15 am'"), Some("15 9 * * 1-5")),
            (":weekend", None, Some("0 0 * * 0,6")),
            ("1.week", None, Some("0 0 * * 0")),
            ("1.month", None, Some("0 0 1 * *")),
            ("1.year", None, Some("0 0 1 1 *")),
            (":hour", None, Some("0 * * * *")),
            (":reboot", None, Some("@reboot")),
            (":daily", None, Some("0 0 * * *")),
            (":hourly", Some("'4:30 am'"), None),
            ("1.day", Some("['4:30 am', '6:00 pm']"), None),
            ("1.day", Some("Time.now"), None),
            ("5.minutes", Some("'1:00'"), None),
            ("60.minutes", None, None),
            ("2.weeks", None, None),
            ("3.months", None, None),
            ("0.days", None, None),
            (":fortnight", None, None),
            ("interval", None, None),
            ("'every day'", None, None),
        ] {
            assert_eq!(
                whenever_every(freq, at).as_deref(),
                want,
                "{freq} at {at:?}"
            );
        }
    }

    #[test]
    fn clock_time_parses_meridiem_and_24h() {
        for (s, meridiem, want) in [
            ("4:30 am", true, Some((4, 30))),
            ("4:30 AM", true, Some((4, 30))),
            ("4:30pm", true, Some((16, 30))),
            ("4pm", true, Some((16, 0))),
            ("12am", true, Some((0, 0))),
            ("12:15 pm", true, Some((12, 15))),
            ("noon", true, Some((12, 0))),
            ("midnight", true, Some((0, 0))),
            ("16:00", true, Some((16, 0))),
            ("16:00", false, Some((16, 0))),
            ("9", false, Some((9, 0))),
            ("4pm", false, None),
            ("13pm", true, None),
            ("24:00", false, None),
            ("4:60", false, None),
            ("4:30 a.m.", true, None),
            ("", false, None),
        ] {
            assert_eq!(clock_time(s, meridiem), want, "{s:?} meridiem={meridiem}");
        }
    }

    #[test]
    fn whenever_schedule_rb_jobs_and_handlers() {
        let src = r#"
set :output, "log/cron.log"

every 1.day, at: '4:30 am' do
  runner "Report.generate"
end

every '0 0 27-31 * *' do
  command "echo month-end"
end

every :sunday, at: '12pm' do # it's the weekly one
  rake "db:backup"
  runner "Other.ignored"
end

every 3.hours,
      roles: [:app] do
  runner "Billing::Invoice.sweep!"
end

# every 1.day do
#   runner "Commented.out"
# end

every 2.weeks do
  runner "Skipped.biweekly"
end

every :reboot do
  script "boot_warmup"
end
"#;
        let out = run(src, "config/schedule.rb");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:0 */3 * * *:Billing::Invoice.sweep!",
                "cron:0 0 27-31 * *:echo",
                "cron:0 12 * * 0:db:backup",
                "cron:30 4 * * *:Report.generate",
                "cron:@reboot:boot_warmup",
            ],
            "a commented-out block and an unsupported frequency never fire"
        );
        assert_eq!(
            handlers(&out),
            vec![
                (
                    "cron:0 */3 * * *:Billing::Invoice.sweep!".to_string(),
                    attr("Invoice", "sweep!")
                ),
                (
                    "cron:30 4 * * *:Report.generate".to_string(),
                    attr("Report", "generate")
                ),
            ]
        );
    }

    #[test]
    fn whenever_needs_its_path_or_its_dsl() {
        let src = "every 1.day do\n  runner \"Report.generate\"\nend\n";
        assert_eq!(run(src, "lib/tasks/cron.rb").nodes.len(), 1, "DSL gate");
        let no_job = "every 1.day do\n  Report.generate\nend\n";
        assert!(
            run(no_job, "lib/jobs.rb").nodes.is_empty(),
            "no job verb, no gate"
        );
        assert_eq!(
            sorted_qnames(&run(no_job, "config/schedule.rb")),
            vec!["cron:0 0 * * *:anon"],
            "the path gate admits a block with no recognised job line"
        );
        assert!(
            run(src, "config/schedule.py").nodes.is_empty(),
            "not a Ruby file"
        );
    }

    #[test]
    fn sidekiq_cron_yaml_needs_cron_and_class() {
        let src = r#"
nightly_digest:
  cron: "30 2 * * *"
  class: "DigestWorker"
  queue: default

no_class:
  cron: "0 5 * * *"
  queue: default

hourly_sync:
  cron: '0 * * * *'
  class: Sync::PullWorker
  args:
    cron: "0 1 * * *"
    class: "Nested"

natural_language:
  cron: "every 5 minutes"
  class: "FugitWorker"
"#;
        let out = run(src, "config/sidekiq_schedule.yml");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:0 * * * *:Sync::PullWorker",
                "cron:30 2 * * *:DigestWorker",
            ],
            "no class, a nested pair and a fugit phrase are all skipped"
        );
        assert!(out.refs.is_empty(), "YAML jobs bind nothing");
    }

    #[test]
    fn sidekiq_cron_yaml_array_and_scheduler_forms() {
        let array = r#"
- name: digest
  cron: "30 2 * * *"
  class: DigestWorker
- name: queue_only
  cron: "0 3 * * *"
"#;
        assert_eq!(
            sorted_qnames(&run(array, "config/schedule.yml")),
            vec!["cron:30 2 * * *:DigestWorker"]
        );
        let scheduler = r#"
:concurrency: 5
:scheduler:
  :schedule:
    hello_world:
      every: '45m'
      class: HelloWorld
    cleanup:
      cron: '0 0 * * * *'
      class: "CleanupWorker"
    backoff:
      every: ['1h', first_in: '10s']
      class: BackoffWorker
"#;
        assert_eq!(
            sorted_qnames(&run(scheduler, "config/sidekiq.yml")),
            vec![
                "cron:0 0 * * * *:CleanupWorker",
                "cron:@every 1h:BackoffWorker",
                "cron:@every 45m:HelloWorld",
            ]
        );
        let workflow = "on:\n  schedule:\n    - cron: '0 4 * * *'\njobs:\n  build:\n    runs-on: ubuntu-latest\n";
        let out = run(workflow, "ci/nightly.yml");
        assert!(
            out.nodes.is_empty(),
            "a bare `cron:` with no class is not sidekiq"
        );
    }

    #[test]
    fn sidekiq_cron_ruby_create_and_load() {
        let src = r#"
require "sidekiq/cron/job"

Sidekiq::Cron::Job.create(name: 'Digest - nightly', cron: '30 2 * * *', class: 'DigestWorker')
Sidekiq::Cron::Job.new(name: 'sync', cron: '0 * * * *', class: Sync::PullWorker).save
Sidekiq::Cron::Job.create({ 'name' => 'purge', 'cron' => '0 4 * * *', 'class' => 'PurgeWorker' })
Sidekiq::Cron::Job.load_from_hash({
  'report' => { 'cron' => '0 6 * * 1', 'class' => 'ReportWorker' },
  'broken' => { 'class' => 'NoCron' }
})
Sidekiq::Cron::Job.load_from_array([
  { 'name' => 'a', 'cron' => '15 1 * * *', 'class' => 'AWorker' }
])
Sidekiq::Cron::Job.load_from_hash(YAML.load_file(path))
"#;
        let out = run(src, "config/initializers/sidekiq.rb");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:0 * * * *:Sync::PullWorker",
                "cron:0 4 * * *:PurgeWorker",
                "cron:0 6 * * 1:ReportWorker",
                "cron:15 1 * * *:AWorker",
                "cron:30 2 * * *:DigestWorker",
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                (
                    "cron:0 * * * *:Sync::PullWorker".to_string(),
                    bare("PullWorker")
                ),
                (
                    "cron:0 4 * * *:PurgeWorker".to_string(),
                    bare("PurgeWorker")
                ),
                (
                    "cron:0 6 * * 1:ReportWorker".to_string(),
                    bare("ReportWorker")
                ),
                ("cron:15 1 * * *:AWorker".to_string(), bare("AWorker")),
                (
                    "cron:30 2 * * *:DigestWorker".to_string(),
                    bare("DigestWorker")
                ),
            ]
        );
    }

    /// One Laravel frequency method on the default `* * * * *`.
    fn laravel(method: &str, args: &[&str]) -> Option<String> {
        let mut fields = vec!["*".to_string(); 5];
        frequency_to_cron(&mut fields, method, args)?;
        Some(fields.join(" "))
    }

    #[test]
    fn laravel_frequency_table() {
        for (method, args, want) in [
            ("cron", &["'15 1 * * 1'"][..], Some("15 1 * * 1")),
            ("everyMinute", &[][..], Some("* * * * *")),
            ("everyTwoMinutes", &[][..], Some("*/2 * * * *")),
            ("everyFiveMinutes", &[][..], Some("*/5 * * * *")),
            ("everyTenMinutes", &[][..], Some("*/10 * * * *")),
            ("everyFifteenMinutes", &[][..], Some("*/15 * * * *")),
            ("everyThirtyMinutes", &[][..], Some("0,30 * * * *")),
            ("hourly", &[][..], Some("0 * * * *")),
            ("hourlyAt", &["17"][..], Some("17 * * * *")),
            ("hourlyAt", &["[0, 30]"][..], Some("0,30 * * * *")),
            ("everyOddHour", &[][..], Some("0 1-23/2 * * *")),
            ("everyTwoHours", &["15"][..], Some("15 */2 * * *")),
            ("everySixHours", &[][..], Some("0 */6 * * *")),
            ("daily", &[][..], Some("0 0 * * *")),
            ("dailyAt", &["'13:00'"][..], Some("0 13 * * *")),
            ("dailyAt", &["'9:05'"][..], Some("5 9 * * *")),
            ("at", &["'7'"][..], Some("0 7 * * *")),
            ("twiceDaily", &[][..], Some("0 1,13 * * *")),
            ("twiceDaily", &["1", "13"][..], Some("0 1,13 * * *")),
            (
                "twiceDailyAt",
                &["1", "13", "15"][..],
                Some("15 1,13 * * *"),
            ),
            ("weekly", &[][..], Some("0 0 * * 0")),
            ("weeklyOn", &["1", "'8:00'"][..], Some("0 8 * * 1")),
            ("weeklyOn", &["Schedule::FRIDAY"][..], Some("0 0 * * 5")),
            ("monthly", &[][..], Some("0 0 1 * *")),
            ("monthlyOn", &["4", "'15:00'"][..], Some("0 15 4 * *")),
            (
                "twiceMonthly",
                &["1", "16", "'13:00'"][..],
                Some("0 13 1,16 * *"),
            ),
            ("quarterly", &[][..], Some("0 0 1 1-12/3 *")),
            (
                "quarterlyOn",
                &["4", "'14:00'"][..],
                Some("0 14 4 1-12/3 *"),
            ),
            ("yearly", &[][..], Some("0 0 1 1 *")),
            ("yearlyOn", &["6", "1", "'17:00'"][..], Some("0 17 1 6 *")),
            ("weekdays", &[][..], Some("* * * * 1-5")),
            ("weekends", &[][..], Some("* * * * 6,0")),
            ("mondays", &[][..], Some("* * * * 1")),
            (
                "days",
                &["[Schedule::SUNDAY, Schedule::WEDNESDAY]"][..],
                Some("* * * * 0,3"),
            ),
            ("days", &["1", "3"][..], Some("* * * * 1,3")),
            ("daily", &["1"][..], None),
            ("dailyAt", &["$time"][..], None),
            ("dailyAt", &["'4pm'"][..], None),
            ("hourlyAt", &["60"][..], None),
            ("days", &["9"][..], None),
            ("lastDayOfMonth", &[][..], None),
            ("everySecond", &[][..], None),
            ("sometimes", &[][..], None),
        ] {
            assert_eq!(laravel(method, args).as_deref(), want, "{method}{args:?}");
        }
        let mut fields = vec!["*".to_string(); 5];
        assert_eq!(
            frequency_to_cron(&mut fields, "withoutOverlapping", &[]),
            Some(false),
            "a modifier leaves the expression alone"
        );
        assert_eq!(fields.join(" "), "* * * * *");
    }

    #[test]
    fn laravel_kernel_chains_and_heads() {
        let src = r#"<?php

namespace App\Console;

use App\Jobs\Heartbeat;
use Illuminate\Console\Scheduling\Schedule;
use Illuminate\Foundation\Console\Kernel as ConsoleKernel;

class Kernel extends ConsoleKernel
{
    protected function schedule(Schedule $schedule)
    {
        $schedule->command('emails:send')->daily();
        $schedule->job(new Heartbeat)->everyFiveMinutes();
        $schedule->command('reports:build')->cron('15 1 * * 1');
        // $schedule->command('commented:out')->hourly();
        $schedule->command('backup:run --only-db')
            ->weekly()->mondays()->at('9:30') // don't run twice
            ->withoutOverlapping()
            ->onOneServer();
        $schedule->call(function () {
            DB::table('recent_users')->delete();
        })->weekdays()->hourly()->timezone('America/Chicago');
        $schedule->call([Cleaner::class, 'purge'])->twiceDaily(1, 13);
        $schedule->exec('node /home/forge/script.js')->dailyAt('3:15');
        $schedule->job(\App\Jobs\Prune::class)->monthlyOn(4, '15:00');
        $schedule->command(SendReminders::class, ['--force'])->everyTenMinutes();
        $schedule->command('unknown:freq')->daily()->sometimesMaybe();
        $schedule->command('no:frequency')->withoutOverlapping();
        $event = $schedule->command('assigned:later');
    }
}
"#;
        let out = run(src, "app/Console/Kernel.php");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:*/10 * * * *:SendReminders",
                "cron:*/5 * * * *:Heartbeat",
                "cron:0 * * * 1-5:anon",
                "cron:0 0 * * *:emails:send",
                "cron:0 1,13 * * *:purge",
                "cron:0 15 4 * *:Prune",
                "cron:15 1 * * 1:reports:build",
                "cron:15 3 * * *:node",
                "cron:30 9 * * 1:backup:run",
            ],
            "an unknown frequency, a chain with none and a commented line are skipped"
        );
        assert_eq!(
            handlers(&out),
            vec![
                (
                    "cron:*/10 * * * *:SendReminders".to_string(),
                    bare("SendReminders")
                ),
                ("cron:*/5 * * * *:Heartbeat".to_string(), bare("Heartbeat")),
                (
                    "cron:0 1,13 * * *:purge".to_string(),
                    attr("Cleaner", "purge")
                ),
                ("cron:0 15 4 * *:Prune".to_string(), bare("Prune")),
            ],
            "a `command('name')` job carries no handler ref"
        );
    }

    #[test]
    fn laravel_schedule_facade_and_gate() {
        let src = "<?php\nuse Illuminate\\Support\\Facades\\Schedule;\n\nSchedule::command('inspire')->hourly();\nSchedule::job(new Heartbeat)->everyMinute();\n\\Illuminate\\Support\\Facades\\Schedule::exec('php artisan queue:work')->everyTwoMinutes();\nweeklyOn(Schedule::MONDAY);\n";
        let out = run(src, "routes/console.php");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:* * * * *:Heartbeat",
                "cron:*/2 * * * *:php",
                "cron:0 * * * *:inspire",
            ]
        );
        let not_php = "Schedule::command('inspire')->hourly();";
        assert!(run(not_php, "routes/console.ts").nodes.is_empty());
    }

    #[test]
    fn oban_crontab_tuples() {
        let src = r#"
import Config

config :my_app, Oban,
  plugins: [
    {Oban.Plugins.Cron,
     crontab: [
       {"0 * * * *", MyApp.Workers.HourlyWorker},
       # it's daily
       {"@daily", MyApp.Workers.DailyWorker, args: %{kind: "full"}},
       {"*/15 9-17 * * MON-FRI", Reports},
       {"@every 5m", MyApp.Workers.Nope},
       {bad_expr, MyApp.Workers.Var},
       {"0 3 * * *", :"Elixir.Atom"}
     ]}
  ]
"#;
        let out = run(src, "config/config.exs");
        assert_eq!(
            sorted_qnames(&out),
            vec![
                "cron:*/15 9-17 * * MON-FRI:Reports",
                "cron:0 * * * *:MyApp.Workers.HourlyWorker",
                "cron:0 0 * * *:MyApp.Workers.DailyWorker",
                "cron:@every 5m:MyApp.Workers.Nope",
            ]
        );
        assert_eq!(
            handlers(&out),
            vec![
                (
                    "cron:*/15 9-17 * * MON-FRI:Reports".to_string(),
                    bare("Reports")
                ),
                (
                    "cron:0 * * * *:MyApp.Workers.HourlyWorker".to_string(),
                    bare("HourlyWorker")
                ),
                (
                    "cron:0 0 * * *:MyApp.Workers.DailyWorker".to_string(),
                    bare("DailyWorker")
                ),
                (
                    "cron:@every 5m:MyApp.Workers.Nope".to_string(),
                    bare("Nope")
                ),
            ]
        );
        let ungated = "config :my_app, Scheduler, crontab: [{\"0 * * * *\", MyApp.Job}]";
        assert!(
            run(ungated, "config/config.exs").nodes.is_empty(),
            "no Oban.Plugins.Cron"
        );
    }

    #[test]
    fn script_marker_counts_script_sources_only() {
        let mut c = CodeCounts::default();
        c.bump("quartz");
        assert_eq!(
            script_marker(&c, 0, "Jobs.java"),
            None,
            "a code job is not a script job"
        );
        for s in ["laravel", "laravel", "laravel"] {
            c.bump(s);
        }
        assert_eq!(
            script_marker(&c, 1, "app/Console/Kernel.php").as_deref(),
            Some(
                "[cron] script jobs=3 whenever=0 sidekiq=0 laravel=3 oban=0 handler_refs=1 path=app/Console/Kernel.php"
            )
        );
        c.bump("whenever");
        c.bump("sidekiq_cron");
        c.bump("oban");
        assert_eq!(
            script_marker(&c, 2, "x").as_deref(),
            Some(
                "[cron] script jobs=6 whenever=1 sidekiq=1 laravel=3 oban=1 handler_refs=2 path=x"
            )
        );
        assert_eq!(
            code_marker(&c, 0, "x").as_deref(),
            Some(
                "[cron] code jobs=1 quartz=1 hangfire=0 go=0 apscheduler=0 spring=0 nestjs=0 handler_refs=0 path=x"
            ),
            "the LA.19a marker is unchanged by script jobs"
        );
    }

    #[test]
    fn blank_comments_keeps_offsets_and_strings() {
        let src = "a = \"# not a comment\" # gone \u{1F600}\nb = 'x' # it's gone\n\"\"\"\n# heredoc text\n\"\"\"\n";
        let out = blank_comments(src, CommentStyle::Hash);
        assert_eq!(out.len(), src.len());
        assert_eq!(out.lines().count(), src.lines().count());
        assert!(out.contains("\"# not a comment\""));
        assert!(!out.contains("gone"));
        assert!(
            out.contains("# heredoc text"),
            "a heredoc is a string, not a comment"
        );
        let php = "<?php\n#[Attr]\n$a = 'http://x'; // c1\n/* c2\n c3 */ $b = 1; # c4\n";
        let out = blank_comments(php, CommentStyle::Php);
        assert_eq!(out.len(), php.len());
        assert!(out.contains("#[Attr]") && out.contains("'http://x'") && out.contains("$b = 1;"));
        assert!(
            !out.contains("c1")
                && !out.contains("c2")
                && !out.contains("c3")
                && !out.contains("c4")
        );
    }

    #[test]
    fn script_scans_survive_multibyte_text() {
        let wide = "\u{1F600}";
        for (src, path) in [
            (
                format!(
                    "every 1.day, at: '{wide}' do\n  runner \"{wide}.x\"\nend\nevery {wide} do # {wide}\nend\nevery 1.day do\n  runner \"R.{wide}"
                ),
                "config/schedule.rb",
            ),
            (
                format!(
                    "{wide}:\n  cron: \"{wide}\"\n  class: {wide}\n- cron: '0 * * * *'\n  class: \"{wide}"
                ),
                "config/sidekiq_schedule.yml",
            ),
            (
                format!(
                    "Sidekiq::Cron::Job.create(cron: '{wide}', class: {wide})\nSidekiq::Cron::Job.load_from_hash({{'{wide}' => {{'cron' => '0 * * * *'"
                ),
                "s.rb",
            ),
            (
                format!(
                    "<?php $schedule->job(new {wide})->dailyAt('{wide}'); $schedule->call([{wide}::class, '{wide}'])->days({wide}); /* {wide}"
                ),
                "k.php",
            ),
            (
                format!(
                    "Oban.Plugins.Cron crontab: [{{\"{wide}\", {wide}}}, {{\"0 * * * *\", M.{wide}}}, {{\"0 * * * *\""
                ),
                "c.exs",
            ),
        ] {
            let _ = run(&src, path);
        }
    }
}
