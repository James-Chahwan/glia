//! Cron extraction (v0.4.x — task #7).
//!
//! Emits `CRON_JOB` nodes for every scheduled invocation we can see in the
//! repo. Five high-leverage in-repo sources (path/content gated):
//!
//!   1. GitHub Actions — `.github/workflows/*.yml` with `on.schedule.cron`
//!   2. k8s CronJob YAML — file containing `kind: CronJob` + `schedule:`
//!   3. node-cron / cron — `cron.schedule('* * * * *', handler)`
//!   4. Celery beat — entries inside an `app.conf.beat_schedule = { ... }`
//!      dict, looking for `'schedule': crontab(...)` or `'schedule': N.0`
//!   5. Java/Spring/Quartz — `@Scheduled(cron = "...")`
//!
//! Out of scope (deliberate v1 cut, see ship-plan memory):
//!   - Server-side `crontab -e` entries that aren't committed
//!   - UI-configured cloud schedulers (GCP Scheduler / EventBridge)
//!   - systemd `*.timer`, Sidekiq-cron, Hangfire, APScheduler, Rails whenever
//!   - Dockerfile CMD bridging to external scheduler — IaC resolver (#9) closes
//!     this gap by linking image → k8s CronJob via Resource nodes.
//!
//! Qname shape: `cron:<schedule>:<target_id>`. `<schedule>` is the verbatim
//! cron expression (or a normalised rate marker). `<target_id>` is the script
//! basename / handler symbol when extractable, else `anon`. The full qname is
//! the join key for `CronResolver` — drift detection rather than schedule
//! overlap.

use repo_graph_code_domain::{CodeNav, GRAPH_TYPE, edge_category, node_kind};
use repo_graph_core::{Cell, CellPayload, Confidence, Edge, Node, NodeId, RepoId};

pub struct CronNodes {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub nav: CodeNav,
}

#[derive(Debug, Clone)]
struct CronJob {
    schedule: String,
    target: String,
    source: &'static str, // workflow / k8s / node-cron / celery / scheduled-annot
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
    let mut seen = std::collections::HashSet::new();

    let mut jobs: Vec<CronJob> = Vec::new();

    if is_github_actions_workflow(path) {
        jobs.extend(extract_github_actions(source, path));
    }
    if looks_like_k8s_cronjob(source) {
        jobs.extend(extract_k8s_cronjob(source));
    }
    jobs.extend(extract_node_cron(source));
    jobs.extend(extract_celery_beat(source));
    jobs.extend(extract_scheduled_annotation(source));

    for job in jobs {
        let qname = format!("cron:{}:{}", job.schedule, job.target);
        if !seen.insert(qname.clone()) {
            continue;
        }
        let id = NodeId::from_parts(GRAPH_TYPE, repo, node_kind::CRON_JOB, &qname);
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
                kind: repo_graph_code_domain::cell_type::CODE,
                payload: CellPayload::Json(payload),
            }],
        });
        nav.record(id, &job.schedule, &qname, node_kind::CRON_JOB, Some(module_id));
        // Edge from the file's module → the cron job — lets a graph query find
        // every job a service registers without a separate index.
        edges.push(Edge {
            from: module_id,
            to: id,
            category: edge_category::SCHEDULES,
            confidence: Confidence::Medium,
        });
    }

    CronNodes { nodes, edges, nav }
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
// k8s CronJob: any YAML containing `kind: CronJob` (case-sensitive — k8s
// kinds are PascalCase). Pull `schedule:` and a target hint from the first
// container's `command:` / `args:` / `image:` if present.
// ----------------------------------------------------------------------------

fn looks_like_k8s_cronjob(source: &str) -> bool {
    source.contains("kind: CronJob")
}

fn extract_k8s_cronjob(source: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    let mut current_schedule: Option<String> = None;
    let mut current_image: Option<String> = None;
    let mut current_command: Option<String> = None;
    for line in source.lines() {
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
    if let Some(schedule) = current_schedule {
        let target = current_command
            .or(current_image)
            .unwrap_or_else(|| "anon".to_string());
        out.push(CronJob {
            schedule,
            target,
            source: "k8s_cronjob",
        });
    }
    out
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

fn extract_node_cron(source: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    // Method form: `cron.schedule('...', handler)`. Handler may be an
    // identifier, an arrow function, or a method reference.
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("cron.schedule(") {
        let pos = search_from + rel;
        let after = &source[pos + "cron.schedule(".len()..];
        if let Some(schedule) = first_quoted(after) {
            if looks_like_cron_expr(&schedule) {
                let target = handler_after_first_arg(after).unwrap_or_else(|| "anon".to_string());
                out.push(CronJob {
                    schedule,
                    target,
                    source: "node_cron",
                });
            }
        }
        search_from = pos + "cron.schedule(".len();
    }
    // Class form: `new CronJob({ cronTime: '...', onTick: handler })`
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("cronTime:") {
        let pos = search_from + rel;
        let after = &source[pos + "cronTime:".len()..];
        if let Some(schedule) = first_quoted(after) {
            if looks_like_cron_expr(&schedule) {
                // Look for `onTick:` within the next ~256 bytes for the target.
                let win_end = (pos + 256).min(source.len());
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
                });
            }
        }
        search_from = pos + "cronTime:".len();
    }
    out
}

// ----------------------------------------------------------------------------
// Celery beat: `'schedule': crontab(minute=..., hour=...)` or
// `'schedule': 30.0` / `'schedule': timedelta(...)`. We capture the schedule
// expression verbatim from the source (post-colon, pre-comma) and try to
// pull a sibling `'task':` for the target.
// ----------------------------------------------------------------------------

fn extract_celery_beat(source: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("'schedule':") {
        let pos = search_from + rel;
        let after = &source[pos + "'schedule':".len()..];
        let schedule = celery_schedule_expr(after);
        if !schedule.is_empty() && schedule.len() < 256 {
            // Walk back ~256 bytes to find the sibling `'task':` value.
            let look_back_start = pos.saturating_sub(256);
            let context = &source[look_back_start..pos];
            let target = celery_task_in(context).unwrap_or_else(|| "anon".to_string());
            out.push(CronJob {
                schedule,
                target,
                source: "celery_beat",
            });
        }
        search_from = pos + "'schedule':".len();
    }
    // Double-quoted variant.
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("\"schedule\":") {
        let pos = search_from + rel;
        let after = &source[pos + "\"schedule\":".len()..];
        let schedule = celery_schedule_expr(after);
        if !schedule.is_empty() && schedule.len() < 256 {
            let look_back_start = pos.saturating_sub(256);
            let context = &source[look_back_start..pos];
            let target = celery_task_in(context).unwrap_or_else(|| "anon".to_string());
            out.push(CronJob {
                schedule,
                target,
                source: "celery_beat",
            });
        }
        search_from = pos + "\"schedule\":".len();
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
// Java/Spring/Quartz: `@Scheduled(cron = "0 4 * * * *")`
// ----------------------------------------------------------------------------

fn extract_scheduled_annotation(source: &str) -> Vec<CronJob> {
    let mut out = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = source[search_from..].find("@Scheduled(") {
        let pos = search_from + rel;
        let after_paren = pos + "@Scheduled(".len();
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
                        let target = method_name_after_annotation(&source[j..])
                            .unwrap_or_else(|| "anon".to_string());
                        out.push(CronJob {
                            schedule,
                            target,
                            source: "scheduled_annot",
                        });
                    }
                }
            }
        }
        search_from = (j + 1).min(source.len());
    }
    out
}

/// After the closing `)` of an `@Scheduled(...)` annotation, find the next
/// Java method declaration's name — best-effort identifier scan.
fn method_name_after_annotation(after_close: &str) -> Option<String> {
    // Skip whitespace/newlines, then tokens until we see `(`. Take the
    // identifier immediately preceding it.
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
}
