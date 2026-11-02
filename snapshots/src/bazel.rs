//! `targets sync` (0.5.7, HT.3): the build-target snapshot step.
//!
//! Runs `bazel query '<expr>' --output=streamed_jsonproto --keep_going
//! --noshow_progress` in one bazel workspace of a repo (or reads a saved
//! query output), normalises each target to a
//! [`TargetRow`](glia_build_domain::snapshot::TargetRow) and writes
//! `<repo>/.glia/targets-snapshot/` through
//! [`write_targets`](glia_build_domain::snapshot::write_targets). A static
//! BUILD-file parse cannot see what macros and `select()` produce, so the
//! build system itself is asked, once, here; a glia build never runs it.
//!
//! The query output is build.proto's `Target` message printed by protobuf's
//! JsonFormat, one per line: `{"type":"RULE","rule":{"name","ruleClass",
//! "location":"/abs/BUILD:L:C","attribute":[{"name","type",
//! "explicitlySpecified","stringValue"|"stringListValue"|"booleanValue"|
//! "selectorList"}]}}`, `{"type":"SOURCE_FILE","sourceFile":{"name",
//! "location","visibilityLabel"}}` and `{"type":"GENERATED_FILE",
//! "generatedFile":{"name","generatingRule","location"}}`. A rule row keeps
//! every LABEL / LABEL_LIST attribute its BUILD file set (`explicitlySpecified`),
//! by name: the rule class's defaults and its own wiring (implicit `$x`,
//! late-bound `:x` and Starlark-private `_x` attributes, a toolchain on every
//! target) are not edges the repo declared.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

use glia_build_domain::snapshot::{
    KIND_GENERATED_FILE, KIND_RULE, KIND_SOURCE_FILE, TargetRow, TargetsMeta, WorkspaceMeta,
    read_targets, targets_dir, write_targets,
};
use serde::Serialize;
use serde_json::Value;

/// The generator `meta.json` names.
pub(crate) const GENERATOR: &str = "glia targets sync";

/// The query expression a sync runs by default: every target of every
/// package, source files included (`//...` alone lists rule targets only).
pub const DEFAULT_EXPR: &str = "//...:*";

/// [`TargetsSummary::bazel_version`] of a sync that read a saved query output.
pub const FROM_FILE: &str = "from-file";

/// Largest query output read: past it the query is stopped and the sync fails.
pub const MAX_QUERY_BYTES: u64 = 512 << 20;

/// `bazel query`'s exit code for a partial result under `--keep_going`.
const EXIT_PARTIAL: i32 = 3;

/// Options for [`targets_sync`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TargetsOptions {
    /// The bazel (or bazelisk) binary to run; default `bazel`, found on PATH.
    pub bazel: String,
    /// The bazel workspace, repo-relative; default `""`, the repo root. It
    /// must hold `MODULE.bazel`, `WORKSPACE.bazel` or `WORKSPACE` (unless
    /// [`TargetsOptions::from`] is set).
    pub workspace: PathBuf,
    /// The query expression; default [`DEFAULT_EXPR`].
    pub expr: String,
    /// Read this saved `--output=streamed_jsonproto` output instead of running
    /// bazel.
    pub from: Option<PathBuf>,
}

impl Default for TargetsOptions {
    fn default() -> Self {
        Self {
            bazel: "bazel".to_string(),
            workspace: PathBuf::new(),
            expr: DEFAULT_EXPR.to_string(),
            from: None,
        }
    }
}

impl TargetsOptions {
    /// Query workspace `workspace` (repo-relative) with the defaults.
    pub fn in_workspace(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            ..Self::default()
        }
    }
}

/// What one sync wrote for its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct TargetsSummary {
    /// The workspace, repo-relative and `/`-separated (`""` the repo root).
    pub workspace: String,
    /// Target rows written for the workspace (rules, source and generated
    /// files).
    pub targets: usize,
    /// Rule rows.
    pub rules: usize,
    /// Source-file rows.
    pub source_files: usize,
    /// Distinct labels outside the main repository (`@repo//..`) the rows'
    /// attributes name.
    pub external_labels: usize,
    /// Query lines not read as a target (package or environment groups,
    /// unparseable or nameless lines, repeated labels) plus, for a partial
    /// `--keep_going` result, the `ERROR:` lines bazel printed.
    pub skipped: usize,
    /// `bazel version`'s build label, `unknown` when it printed none, or
    /// [`FROM_FILE`].
    pub bazel_version: String,
}

/// Query the bazel workspace `opts.workspace` of the repo at `repo_root` (or
/// read the saved query output `opts.from`) and write its targets to
/// `<repo_root>/.glia/targets-snapshot/`, replacing only that workspace's
/// rows (`targets.jsonl`, then `meta.json` LAST). Creates `.glia/.gitignore`
/// when absent, never overwriting one. Never run inside a build.
///
/// Errors come back before anything is written: a workspace outside the repo
/// or holding no `MODULE.bazel` / `WORKSPACE.bazel` / `WORKSPACE`, a bazel
/// that cannot run (the message names the binary), a bazel with no
/// `--output=streamed_jsonproto`, a failed query (exit 3, a `--keep_going`
/// partial result, is accepted and its `ERROR:` lines counted `skipped`), or
/// an output past [`MAX_QUERY_BYTES`]. On success prints, on stderr,
/// `[targets] sync workspace=<w> rows=<n> rules=<r> source_files=<s>
/// external=<x> skipped=<k> bazel=<version|from-file> -> <repo>/.glia/targets-snapshot/`.
pub fn targets_sync(repo_root: &Path, opts: &TargetsOptions) -> Result<TargetsSummary, String> {
    let workspace = workspace_key(&opts.workspace)?;
    let ws_dir = repo_root.join(&workspace);
    let (output, bazel_version, error_lines) = match &opts.from {
        Some(file) => {
            let bytes = read_capped(file)?;
            (bytes, FROM_FILE.to_string(), 0)
        }
        None => {
            if !is_workspace(&ws_dir) {
                return Err(format!(
                    "no bazel workspace at {} (no MODULE.bazel / WORKSPACE)",
                    ws_dir.display()
                ));
            }
            let version = bazel_version(&opts.bazel, &ws_dir)?;
            let (bytes, errors) = run_query(&opts.bazel, &ws_dir, &opts.expr, &version)?;
            (bytes, version, errors)
        }
    };

    let paths = Paths::new(repo_root, &ws_dir);
    let parsed = parse_query(&output, &workspace, &paths);
    let summary = TargetsSummary {
        workspace: workspace.clone(),
        targets: parsed.rows.len(),
        rules: parsed.rows.iter().filter(|r| r.kind == KIND_RULE).count(),
        source_files: parsed
            .rows
            .iter()
            .filter(|r| r.kind == KIND_SOURCE_FILE)
            .count(),
        external_labels: external_labels(&parsed.rows),
        skipped: parsed.skipped + error_lines,
        bazel_version: bazel_version.clone(),
    };

    // Replace only this workspace's rows; an incomplete snapshot (reported by
    // read_targets) is started over.
    let (mut workspaces, mut rows) = match read_targets(repo_root) {
        Some(snap) => (
            snap.meta
                .workspaces
                .into_iter()
                .filter(|w| w.workspace != workspace)
                .collect::<Vec<_>>(),
            snap.rows
                .into_iter()
                .filter(|r| r.workspace != workspace)
                .collect::<Vec<_>>(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    workspaces.push(WorkspaceMeta::new(
        workspace.clone(),
        bazel_version,
        opts.expr.clone(),
    ));
    workspaces.sort_by(|a, b| a.workspace.cmp(&b.workspace));
    rows.extend(parsed.rows);
    rows.sort_by(|a, b| (&a.workspace, &a.label).cmp(&(&b.workspace, &b.label)));
    write_targets(repo_root, TargetsMeta::new(GENERATOR, workspaces), &rows)?;
    crate::ensure_glia_gitignore(repo_root)?;
    eprintln!("{}", marker(&summary, &targets_dir(repo_root)));
    Ok(summary)
}

/// `[targets] sync workspace=<w> rows=<n> rules=<r> source_files=<s>
/// external=<x> skipped=<k> bazel=<version|from-file> -> <dir>/` (the repo
/// root's workspace prints `.`).
pub(crate) fn marker(s: &TargetsSummary, dir: &Path) -> String {
    let ws = if s.workspace.is_empty() {
        "."
    } else {
        s.workspace.as_str()
    };
    format!(
        "[targets] sync workspace={ws} rows={} rules={} source_files={} external={} skipped={} bazel={} -> {}/",
        s.targets,
        s.rules,
        s.source_files,
        s.external_labels,
        s.skipped,
        s.bazel_version,
        dir.display()
    )
}

/// The workspace as a repo-relative `/`-separated key, refusing an absolute
/// path or one that leaves the repo.
fn workspace_key(ws: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for c in ws.components() {
        match c {
            Component::Normal(p) => parts.push(
                p.to_str()
                    .ok_or_else(|| format!("workspace {} is not UTF-8", ws.display()))?
                    .to_string(),
            ),
            Component::CurDir => {}
            _ => {
                return Err(format!(
                    "workspace {} must be a repo-relative directory inside the repo",
                    ws.display()
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

fn is_workspace(dir: &Path) -> bool {
    ["MODULE.bazel", "WORKSPACE.bazel", "WORKSPACE"]
        .iter()
        .any(|f| dir.join(f).is_file())
}

fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_QUERY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read {}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_QUERY_BYTES {
        return Err(format!(
            "{} is larger than {} MB; narrow the query",
            path.display(),
            MAX_QUERY_BYTES >> 20
        ));
    }
    Ok(bytes)
}

/// `bazel version`'s `Build label:`, or `unknown`.
fn bazel_version(bazel: &str, ws_dir: &Path) -> Result<String, String> {
    let out = Command::new(bazel)
        .arg("version")
        .current_dir(ws_dir)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run bazel `{bazel}` in {}: {e}", ws_dir.display()))?;
    if !out.status.success() {
        return Err(format!(
            "`{bazel} version` failed ({}) in {}: {}",
            out.status,
            ws_dir.display(),
            tail(&out.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.strip_prefix("Build label:"))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "unknown".to_string()))
}

/// Run the query; returns its stdout and, for a partial result, the number
/// of `ERROR:` lines it printed (at least 1).
fn run_query(
    bazel: &str,
    ws_dir: &Path,
    expr: &str,
    version: &str,
) -> Result<(Vec<u8>, usize), String> {
    let mut child = Command::new(bazel)
        .arg("query")
        .arg(expr)
        .args([
            "--output=streamed_jsonproto",
            "--keep_going",
            "--noshow_progress",
        ])
        .current_dir(ws_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run bazel `{bazel}` in {}: {e}", ws_dir.display()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| "bazel query: no stderr pipe".to_string())?;
    let errs = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let mut stdout = Vec::new();
    let read = match child.stdout.take() {
        Some(out) => out
            .take(MAX_QUERY_BYTES + 1)
            .read_to_end(&mut stdout)
            .map(|_| ()),
        None => Ok(()),
    };
    let over = stdout.len() as u64 > MAX_QUERY_BYTES;
    if over {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|e| format!("bazel query: {e}"))?;
    let stderr = errs.join().unwrap_or_default();
    read.map_err(|e| format!("bazel query: reading its output: {e}"))?;
    if over {
        return Err(format!(
            "bazel query output in {} is larger than {} MB; narrow it with --expr",
            ws_dir.display(),
            MAX_QUERY_BYTES >> 20
        ));
    }
    let text = String::from_utf8_lossy(&stderr);
    match status.code() {
        Some(0) => Ok((stdout, 0)),
        Some(EXIT_PARTIAL) => {
            let errors = text.lines().filter(|l| l.starts_with("ERROR:")).count();
            Ok((stdout, errors.max(1)))
        }
        _ if text.contains("streamed_jsonproto")
            && (text.contains("Invalid output format") || text.contains("not a valid")) =>
        {
            Err(format!(
                "bazel {version} has no --output=streamed_jsonproto; run it with a newer bazel and pass --from <file>"
            ))
        }
        code => Err(format!(
            "bazel query failed (exit {}) in {}: {}",
            code.map_or_else(|| "signal".to_string(), |c| c.to_string()),
            ws_dir.display(),
            tail(&stderr)
        )),
    }
}

/// The last few non-empty lines of a process's stderr.
fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(5)..].join(" | ")
}

/// The roots a `location` path is made repo-relative against.
struct Paths {
    /// The repo root, as given and canonical.
    repo: Vec<PathBuf>,
    /// The workspace dir, canonical when it exists.
    workspace: PathBuf,
}

impl Paths {
    fn new(repo_root: &Path, ws_dir: &Path) -> Self {
        let mut repo = vec![repo_root.to_path_buf()];
        if let Ok(c) = std::fs::canonicalize(repo_root)
            && c != repo_root
        {
            repo.insert(0, c);
        }
        let workspace = std::fs::canonicalize(ws_dir).unwrap_or_else(|_| ws_dir.to_path_buf());
        Self { repo, workspace }
    }

    /// `path` (absolute, or relative to the workspace as `--relative_locations`
    /// prints it) as a repo-relative `/`-separated path, or `None` outside
    /// the repo.
    fn relative(&self, path: &str) -> Option<String> {
        let p = Path::new(path);
        let abs = if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.workspace.join(p)
        };
        self.repo.iter().find_map(|root| {
            let rel = abs.strip_prefix(root).ok()?;
            let mut parts = Vec::new();
            for c in rel.components() {
                match c {
                    Component::Normal(s) => parts.push(s.to_str()?.to_string()),
                    Component::CurDir => {}
                    _ => return None,
                }
            }
            (!parts.is_empty()).then(|| parts.join("/"))
        })
    }

    /// `"<path>:<line>:<col>"` -> (repo-relative file, 0-based line).
    fn location(&self, loc: &str) -> (Option<String>, Option<u32>) {
        let mut it = loc.rsplitn(3, ':');
        let (col, line, path) = (it.next(), it.next(), it.next());
        match (path, line.and_then(|l| l.parse::<u32>().ok()), col) {
            (Some(path), Some(line), Some(c)) if c.parse::<u32>().is_ok() => {
                (self.relative(path), Some(line.saturating_sub(1)))
            }
            _ => match loc.rsplit_once(':') {
                Some((path, line)) if line.parse::<u32>().is_ok() => (
                    self.relative(path),
                    line.parse::<u32>().ok().map(|l| l.saturating_sub(1)),
                ),
                _ => (self.relative(loc), None),
            },
        }
    }
}

struct Parsed {
    rows: Vec<TargetRow>,
    skipped: usize,
}

/// Every line of `output` read as a target of `workspace`, sorted by label;
/// a repeated label keeps its first row.
fn parse_query(output: &[u8], workspace: &str, paths: &Paths) -> Parsed {
    let mut rows: BTreeMap<String, TargetRow> = BTreeMap::new();
    let mut skipped = 0;
    for line in output.split(|b| *b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let row = serde_json::from_slice::<Value>(line)
            .ok()
            .and_then(|v| target_row(&v, workspace, paths));
        match row {
            Some(row) if !rows.contains_key(&row.label) => {
                rows.insert(row.label.clone(), row);
            }
            _ => skipped += 1,
        }
    }
    Parsed {
        rows: rows.into_values().collect(),
        skipped,
    }
}

/// One `Target` JSON object as a row; `None` for a package or environment
/// group, an unknown type or a nameless target.
fn target_row(v: &Value, workspace: &str, paths: &Paths) -> Option<TargetRow> {
    let (kind, body) = match v.get("type")?.as_str()? {
        "RULE" => (KIND_RULE, v.get("rule")?),
        "SOURCE_FILE" => (KIND_SOURCE_FILE, v.get("sourceFile")?),
        "GENERATED_FILE" => (KIND_GENERATED_FILE, v.get("generatedFile")?),
        _ => return None,
    };
    let name = body.get("name")?.as_str()?;
    if name.is_empty() {
        return None;
    }
    let mut row = TargetRow::new(workspace, main_repo(name), kind);
    if let Some(loc) = body.get("location").and_then(Value::as_str) {
        let (file, line) = paths.location(loc);
        row.file = file;
        row.line = line;
    }
    match kind {
        KIND_RULE => {
            row.rule_class = body
                .get("ruleClass")
                .and_then(Value::as_str)
                .map(str::to_string);
            for attr in body
                .get("attribute")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                read_attribute(attr, &mut row);
            }
        }
        KIND_SOURCE_FILE => {
            row.visibility = sorted(strings(body.get("visibilityLabel")));
        }
        _ => {}
    }
    Some(row)
}

/// One rule attribute into `row`.
fn read_attribute(attr: &Value, row: &mut TargetRow) {
    let Some(name) = attr.get("name").and_then(Value::as_str) else {
        return;
    };
    let ty = attr.get("type").and_then(Value::as_str).unwrap_or("");
    match name {
        "tags" => row.tags = sorted(values(attr)),
        "visibility" => row.visibility = sorted(values(attr)),
        "testonly" => row.testonly = bool_value(attr),
        _ => {
            let label_typed = matches!(ty, "LABEL" | "LABEL_LIST");
            // An absent explicitlySpecified (an older bazel) counts as set.
            let explicit = attr
                .get("explicitlySpecified")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let wiring = name.starts_with(['$', ':', '_']);
            if !label_typed || !explicit || wiring {
                return;
            }
            let labels = sorted(values(attr).into_iter().map(|l| main_repo(&l)).collect());
            if !labels.is_empty() {
                row.labels.insert(name.to_string(), labels);
            }
        }
    }
}

/// An attribute's string values: `stringValue`, `stringListValue`, and every
/// branch of a `selectorList`.
fn values(attr: &Value) -> Vec<String> {
    let mut out = strings(attr.get("stringListValue"));
    if let Some(s) = attr.get("stringValue").and_then(Value::as_str) {
        out.push(s.to_string());
    }
    for element in attr
        .get("selectorList")
        .and_then(|s| s.get("elements"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        for entry in element
            .get("entries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            out.extend(strings(entry.get("stringListValue")));
            if let Some(s) = entry.get("stringValue").and_then(Value::as_str) {
                out.push(s.to_string());
            }
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn bool_value(attr: &Value) -> bool {
    attr.get("booleanValue")
        .and_then(Value::as_bool)
        .or_else(|| attr.get("intValue").and_then(Value::as_i64).map(|i| i != 0))
        .unwrap_or(false)
}

fn sorted(v: Vec<String>) -> Vec<String> {
    v.into_iter().collect::<BTreeSet<_>>().into_iter().collect()
}

/// `@@//x` and `@//x` name the main repository: `//x`.
fn main_repo(label: &str) -> String {
    label
        .strip_prefix("@@//")
        .or_else(|| label.strip_prefix("@//"))
        .map_or_else(|| label.to_string(), |rest| format!("//{rest}"))
}

/// Distinct attribute labels outside the main repository.
fn external_labels(rows: &[TargetRow]) -> usize {
    rows.iter()
        .flat_map(|r| r.named_labels().map(|(_, l)| l))
        .filter(|l| l.starts_with('@'))
        .collect::<BTreeSet<_>>()
        .len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_the_specified_line() {
        let s = TargetsSummary {
            workspace: "go-tutorial/stage3".into(),
            targets: 8,
            rules: 3,
            source_files: 5,
            external_labels: 0,
            skipped: 1,
            bazel_version: "7.4.0".into(),
        };
        assert_eq!(
            marker(&s, Path::new("/r/.glia/targets-snapshot")),
            "[targets] sync workspace=go-tutorial/stage3 rows=8 rules=3 source_files=5 external=0 \
             skipped=1 bazel=7.4.0 -> /r/.glia/targets-snapshot/"
        );
        let root = TargetsSummary {
            workspace: String::new(),
            bazel_version: FROM_FILE.into(),
            ..s
        };
        assert!(marker(&root, Path::new("d")).starts_with("[targets] sync workspace=. rows=8"));
        assert!(marker(&root, Path::new("d")).contains(" bazel=from-file -> d/"));
    }

    #[test]
    fn locations_read_path_line_and_column() {
        let paths = Paths {
            repo: vec![PathBuf::from("/r")],
            workspace: PathBuf::from("/r/ws"),
        };
        assert_eq!(
            paths.location("/r/ws/main/BUILD:10:10"),
            (Some("ws/main/BUILD".into()), Some(9))
        );
        assert_eq!(
            paths.location("main/BUILD:4:11"),
            (Some("ws/main/BUILD".into()), Some(3))
        );
        assert_eq!(paths.location("/other/BUILD:1:1"), (None, Some(0)));
        assert_eq!(
            paths.location("/r/ws/a/BUILD:2"),
            (Some("ws/a/BUILD".into()), Some(1))
        );
        assert_eq!(paths.location("/r/../etc/x:1:1").0, None);
    }

    #[test]
    fn workspace_keys_stay_inside_the_repo() {
        assert_eq!(workspace_key(Path::new("")).unwrap(), "");
        assert_eq!(workspace_key(Path::new("./a/b/")).unwrap(), "a/b");
        assert!(workspace_key(Path::new("../x")).is_err());
        assert!(workspace_key(Path::new("/abs")).is_err());
    }

    #[test]
    fn main_repo_labels_drop_their_repo_prefix() {
        assert_eq!(main_repo("@@//a:b"), "//a:b");
        assert_eq!(main_repo("@//a:b"), "//a:b");
        assert_eq!(main_repo("@rules_cc//cc:x"), "@rules_cc//cc:x");
        assert_eq!(main_repo("//a"), "//a");
    }
}
