//! The run summary: `summary.json` and `summary.html` in a run directory,
//! `<log_root>/<log-stamp>/`, with one entry for each target that ran there.
//! Targets that share a log stamp merge their entries under
//! `<run directory>/.summary.lock`.

use crate::exit;
use crate::plan::{module_rel, PHASES};
use crate::run::RunResult;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// The version of the format of `summary.json`.
pub const SCHEMA_VERSION: u32 = 1;
/// The summary's file name in the run directory.
pub const JSON: &str = "summary.json";
/// The page's file name in the run directory.
pub const HTML: &str = "summary.html";
const LOCK: &str = ".summary.lock";

/// `summary.json`: what each target that ran in a run directory came to.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Summary {
    /// Version of this format, [`SCHEMA_VERSION`].
    pub schema_version: u32,
    /// Always `summary`.
    pub kind: String,
    /// `eb-pipeline VERSION` that last wrote the file.
    pub tool: String,
    /// `site.name`.
    pub site: String,
    /// The run directory's name, its log stamp.
    pub run: String,
    /// When the file was last written, in RFC 3339, in UTC.
    pub updated_at: String,
    /// The targets' exit codes combined as the run contract orders them.
    pub exit_code: i32,
    /// What `exit_code` stands for, as [`status_of`] names it.
    pub status: String,
    /// By target name.
    pub targets: Vec<TargetSummary>,
}

/// What one target's run came to, from its run result.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TargetSummary {
    /// The target's name.
    pub name: String,
    /// `run`, or `watch` for a targeted rebuild.
    pub command: String,
    /// The build type, `default` without one.
    pub build_type: String,
    /// The run id.
    pub run_id: String,
    /// The build list, as given.
    pub list: String,
    /// Where the run wrote its logs, `<run directory>/<target>`.
    pub log_dir: String,
    /// The `--result` file, when the run had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// When the run started, in RFC 3339, in UTC.
    pub started_at: String,
    /// When the run ended, in RFC 3339, in UTC.
    pub ended_at: String,
    /// Time from the start of the run to its result, to the millisecond.
    pub duration_seconds: f64,
    /// The run's exit code.
    pub exit_code: i32,
    /// What `exit_code` stands for, as [`status_of`] names it.
    pub status: String,
    /// The error that stopped the run, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// `SIGINT` or `SIGTERM`, when a signal stopped the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_by: Option<String>,
    /// How many packages came to each status.
    pub counts: BTreeMap<String, usize>,
    /// In pipeline order, with the phases the site does not configure.
    pub phases: Vec<PhaseSummary>,
    /// Every line the run reached, in that order.
    pub packages: Vec<PackageSummary>,
    /// The easyconfigs the target's selector left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deselected: Vec<String>,
    /// The plan of the watch that ran this rebuild.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<serde_json::Value>,
}

/// What one phase of a target's run came to.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PhaseSummary {
    /// The phase, one of [`PHASES`].
    pub name: String,
    /// The result's phase status, or `not configured`.
    pub status: String,
    /// The exit code behind the status, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Time the phase's commands ran; absent for a phase that ran none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    /// The result's note for the phase, such as why it was skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What one build list line of a target's run came to.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PackageSummary {
    /// The line's easyconfig.
    pub easyconfig: String,
    /// `Name/version`, the module the easyconfig's name gives, when it gives
    /// one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// The unit it belongs to: `all`, `cpu-prerequisites`, `prelude`, or a
    /// shard's name.
    pub unit: String,
    /// `build`, or `sources` when its source fetch failed.
    pub phase: String,
    /// `success`, `failed`, `refused`, `pending` or `skipped`.
    pub status: String,
    /// The exit code the line counts as; absent for a line that did not start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Time the line's command ran; absent for a line that did not start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    /// The log file of the line's command.
    pub log: String,
    /// The result's note for the line, such as why it was skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What an exit code of the run contract stands for.
pub fn status_of(code: i32) -> &'static str {
    match code {
        exit::OK => "success",
        exit::USAGE => "usage",
        exit::TEMPFAIL => "pending",
        exit::REFUSED => "refused",
        exit::SIGINT | exit::SIGTERM => "interrupted",
        _ => "failed",
    }
}

/// Several targets' exit codes as one: a signal, then 2, 1, 78, 75, 0.
pub fn combined_code(codes: &[i32]) -> i32 {
    [
        exit::SIGINT,
        exit::SIGTERM,
        exit::USAGE,
        exit::FAILED,
        exit::REFUSED,
        exit::TEMPFAIL,
    ]
    .into_iter()
    .find(|c| codes.contains(c))
    .or_else(|| codes.iter().copied().find(|c| *c != exit::OK))
    .unwrap_or(exit::OK)
}

/// A target's entry, from its run result.
pub fn entry(
    res: &RunResult,
    command: &str,
    result: Option<&Path>,
    watch: Option<serde_json::Value>,
) -> TargetSummary {
    let mut phases: Vec<PhaseSummary> = res
        .phases
        .iter()
        .map(|p| PhaseSummary {
            name: p.name.clone(),
            status: p.status.clone(),
            exit_code: p.exit_code,
            duration_seconds: p.duration_seconds,
            reason: p.note.clone(),
        })
        .collect();
    for n in &res.not_configured {
        if !phases.iter().any(|p| &p.name == n) {
            phases.push(PhaseSummary {
                name: n.clone(),
                status: "not configured".into(),
                exit_code: None,
                duration_seconds: None,
                reason: None,
            });
        }
    }
    phases.sort_by_key(|p| {
        PHASES
            .iter()
            .position(|n| *n == p.name)
            .unwrap_or(PHASES.len())
    });
    TargetSummary {
        name: res.target.clone(),
        command: command.into(),
        build_type: res.build_type.clone(),
        run_id: res.run_id.clone(),
        list: res.list.clone(),
        log_dir: res.log_dir.clone(),
        result: result.map(|p| p.display().to_string()),
        started_at: res.started_at.clone(),
        ended_at: res.ended_at.clone(),
        duration_seconds: res.duration_seconds,
        exit_code: res.exit_code,
        status: status_of(res.exit_code).into(),
        message: res.message.clone(),
        interrupted_by: res.interrupted_by.clone(),
        counts: res.counts.clone(),
        phases,
        packages: res
            .lines
            .iter()
            .map(|l| PackageSummary {
                easyconfig: l.easyconfig.clone(),
                module: module_rel(&l.easyconfig),
                unit: l.unit.clone(),
                phase: l.phase.clone(),
                status: l.status.clone(),
                exit_code: l.exit_code,
                duration_seconds: l.duration_seconds,
                log: l.log.clone(),
                reason: l.note.clone(),
            })
            .collect(),
        deselected: res
            .deselected
            .iter()
            .map(|d| d.easyconfig.clone())
            .collect(),
        watch,
    }
}

/// The run directory a result's log directory, `<run dir>/<target>`, is in.
pub fn run_dir(res: &RunResult) -> PathBuf {
    let d = Path::new(&res.log_dir);
    d.parent().unwrap_or(d).to_path_buf()
}

/// Merge `entry` into `run_dir`'s summary, replacing that target's earlier
/// entry, and write `summary.json` and `summary.html`. Another writer's
/// targets are kept; a file of another schema version is replaced.
pub fn record(site: &str, entry: TargetSummary, run_dir: &Path) -> Result<Summary, String> {
    let shown = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    std::fs::create_dir_all(run_dir).map_err(|e| shown(run_dir, e))?;
    let lock_path = run_dir.join(LOCK);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&lock_path)
        .map_err(|e| shown(&lock_path, e))?;
    lock.lock().map_err(|e| shown(&lock_path, e))?;
    let path = run_dir.join(JSON);
    let earlier = match std::fs::read_to_string(&path) {
        Ok(t) => serde_json::from_str::<Summary>(&t)
            .ok()
            .filter(|s| s.schema_version == SCHEMA_VERSION),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(shown(&path, e)),
    };
    let mut targets = earlier.map(|s| s.targets).unwrap_or_default();
    targets.retain(|t| t.name != entry.name);
    targets.push(entry);
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    let codes: Vec<i32> = targets.iter().map(|t| t.exit_code).collect();
    let code = combined_code(&codes);
    let s = Summary {
        schema_version: SCHEMA_VERSION,
        kind: "summary".into(),
        tool: format!("eb-pipeline {}", env!("CARGO_PKG_VERSION")),
        site: site.into(),
        run: run_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        updated_at: chrono::Utc::now().to_rfc3339(),
        exit_code: code,
        status: status_of(code).into(),
        targets,
    };
    let json = serde_json::to_value(&s).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())? + "\n";
    write_new(&path, &text)?;
    write_new(&run_dir.join(HTML), &html(&s, run_dir))?;
    Ok(s)
}

/// Write through a temporary file and a rename, so a reader never sees half.
fn write_new(path: &Path, text: &str) -> Result<(), String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    std::fs::write(&tmp, text)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Read a run directory's summary.
pub fn read(run_dir: &Path) -> Result<Summary, String> {
    let path = run_dir.join(JSON);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let s: Summary = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if s.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{}: schema_version {} is not {SCHEMA_VERSION}",
            path.display(),
            s.schema_version
        ));
    }
    Ok(s)
}

/// A run directory and its summary, or why it has none to show.
pub struct RunEntry {
    /// The run directory's name, its log stamp.
    pub run: String,
    /// The run directory, `<log_root>/<log-stamp>`.
    pub dir: PathBuf,
    /// Its summary, or why it has none to show.
    pub summary: Result<Summary, String>,
}

/// The newest `last` run directories under `log_root`, newest first. With
/// `target`, only the runs that have a directory for it, with only its entry.
pub fn runs(log_root: &Path, target: Option<&str>, last: usize) -> Result<Vec<RunEntry>, String> {
    selected_runs(log_root, target, last, false)
}

/// The newest `last` problematic run directories under `log_root`.
///
/// A run is problematic when its selected summary has a nonzero exit code,
/// or when its summary is absent or unreadable.
pub fn problem_runs(
    log_root: &Path,
    target: Option<&str>,
    last: usize,
) -> Result<Vec<RunEntry>, String> {
    selected_runs(log_root, target, last, true)
}

fn selected_runs(
    log_root: &Path,
    target: Option<&str>,
    last: usize,
    problems_only: bool,
) -> Result<Vec<RunEntry>, String> {
    let entries = match std::fs::read_dir(log_root) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("{}: {e}", log_root.display())),
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| crate::util::is_log_stamp(n))
        .collect();
    names.sort_unstable_by(|a, b| b.cmp(a));
    let mut out = Vec::new();
    for run in names {
        if out.len() >= last {
            break;
        }
        let dir = log_root.join(&run);
        if target.is_some_and(|t| !dir.join(t).is_dir()) {
            continue;
        }
        let summary = if dir.join(JSON).exists() {
            read(&dir).map(|mut s| {
                if let Some(t) = target {
                    s.targets.retain(|x| x.name == t);
                }
                s
            })
        } else {
            Err(format!(
                "no {JSON}: the run is still going, or ended before writing one"
            ))
        };
        let entry = RunEntry { run, dir, summary };
        if !problems_only || is_problem(&entry, target) {
            out.push(entry);
        }
    }
    Ok(out)
}

fn is_problem(entry: &RunEntry, target: Option<&str>) -> bool {
    match &entry.summary {
        Err(_) => true,
        Ok(summary) if target.is_some() => summary
            .targets
            .first()
            .is_none_or(|selected| selected.exit_code != exit::OK),
        Ok(summary) => summary.exit_code != exit::OK,
    }
}

/// `status` as text: a line per run, then a line per target, in columns
/// that line up across the runs.
pub fn status_text(runs: &[RunEntry]) -> String {
    let targets = || {
        runs.iter()
            .filter_map(|r| r.summary.as_ref().ok())
            .flat_map(|s| &s.targets)
    };
    let name_w = targets().map(|t| t.name.len()).max().unwrap_or(0);
    let command_w = targets().map(|t| t.command.len()).max().unwrap_or(0);
    let status_w = targets().map(|t| t.status.len()).max().unwrap_or(0);
    let mut o = String::new();
    for r in runs {
        let s = match &r.summary {
            Ok(s) => s,
            Err(e) => {
                let _ = writeln!(o, "{}  {e}", r.run);
                continue;
            }
        };
        let _ = writeln!(
            o,
            "{}  {} (exit {})  {}",
            r.run,
            s.status,
            s.exit_code,
            r.dir.join(HTML).display()
        );
        for t in &s.targets {
            let mut what: Vec<String> = t.counts.iter().map(|(k, v)| format!("{v} {k}")).collect();
            if what.is_empty() {
                what.push("no packages".into());
            }
            let phases: Vec<String> = t
                .phases
                .iter()
                .filter(|p| p.status != "ok" && p.status != "not configured")
                .map(|p| match p.exit_code {
                    Some(c) => format!("{} {} (exit {c})", p.name, p.status),
                    None => format!("{} {}", p.name, p.status),
                })
                .collect();
            let phases = if phases.is_empty() {
                String::new()
            } else {
                format!("; {}", phases.join(", "))
            };
            let _ = writeln!(
                o,
                "  {:<name_w$}  {:<command_w$}  {:<status_w$}  exit {:<3}  {:>11}  {}{phases}",
                t.name,
                t.command,
                t.status,
                t.exit_code,
                duration_text(t.duration_seconds),
                what.join(", "),
            );
        }
    }
    o
}

/// `status` as one JSON document.
pub fn status_json(log_root: &Path, runs: &[RunEntry]) -> serde_json::Value {
    let runs: Vec<serde_json::Value> = runs
        .iter()
        .map(|r| {
            let mut v = serde_json::json!({
                "run": r.run,
                "dir": r.dir.display().to_string(),
            });
            match &r.summary {
                Ok(s) => v["summary"] = serde_json::to_value(s).unwrap_or_default(),
                Err(e) => v["error"] = e.clone().into(),
            }
            v
        })
        .collect();
    serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "status",
        "log_root": log_root.display().to_string(),
        "runs": runs,
    })
}

/// `12.3 s`, `4 min 05 s`, `2 h 03 min`.
pub fn duration_text(secs: f64) -> String {
    if secs < 60.0 {
        return format!("{secs:.1} s");
    }
    let t = secs.round() as u64;
    if t < 3600 {
        format!("{} min {:02} s", t / 60, t % 60)
    } else {
        format!("{} h {:02} min", t / 3600, (t % 3600) / 60)
    }
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&#39;"),
            _ => o.push(c),
        }
    }
    o
}

/// A log path as a link relative to the run directory; `None` outside it.
fn href(log: &str, run_dir: &Path) -> Option<String> {
    let rel = Path::new(log).strip_prefix(run_dir).ok()?;
    let mut o = String::new();
    for b in rel.to_string_lossy().bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/+,=@".contains(&b) {
            o.push(b as char);
        } else {
            let _ = write!(o, "%{b:02X}");
        }
    }
    Some(o)
}

const STYLE: &str = "\
body{font-family:system-ui,sans-serif;margin:1.5rem;color:#1f2328;background:#fff;line-height:1.4}
h1{font-size:1.4rem;margin:0 0 .3rem}
h2{font-size:1.15rem;margin:2rem 0 .3rem}
h3{font-size:1rem;margin:1rem 0 .3rem}
table{border-collapse:collapse;margin:.4rem 0 1rem;font-size:.9rem}
th,td{border-bottom:1px solid #d0d7de;padding:.25rem .6rem;text-align:left;vertical-align:top}
th{background:#f6f8fa}
td.n{text-align:right;font-variant-numeric:tabular-nums}
code{font-family:ui-monospace,monospace;font-size:.85rem}
.meta{color:#59636e}
.success,.ok{color:#1a7f37;font-weight:600}
.failed,.usage{color:#cf222e;font-weight:600}
.refused,.pending,.skipped,.interrupted,.unsharded{color:#9a6700;font-weight:600}
@media (prefers-color-scheme:dark){body{background:#0d1117;color:#e6edf3}th{background:#161b22}th,td{border-color:#30363d}.meta{color:#9198a1}}
";

fn status_cell(status: &str) -> String {
    let class: String = status
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    format!("<td class=\"{class}\">{}</td>", esc(status))
}

fn opt_cell(v: Option<String>) -> String {
    match v {
        Some(v) => format!("<td>{}</td>", esc(&v)),
        None => "<td></td>".into(),
    }
}

fn num_cell(v: Option<String>) -> String {
    format!("<td class=\"n\">{}</td>", esc(&v.unwrap_or_default()))
}

/// The summary as one static page: no script, nothing fetched.
pub fn html(s: &Summary, run_dir: &Path) -> String {
    let mut h = String::new();
    let _ = write!(
        h,
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>eb-pipeline run {run}: {status}</title>\n<style>\n{STYLE}</style>\n</head>\n<body>\n\
         <h1>Run {run}: <span class=\"{status}\">{status}</span></h1>\n\
         <p class=\"meta\">Site <code>{site}</code>, exit code {code}, written {at} by {tool}. \
         The same summary as JSON: <a href=\"{JSON}\">{JSON}</a>.</p>\n",
        run = esc(&s.run),
        status = esc(&s.status),
        site = esc(&s.site),
        code = s.exit_code,
        at = esc(&s.updated_at),
        tool = esc(&s.tool),
    );
    h.push_str(
        "<table>\n<tr><th>Target</th><th>Command</th><th>Status</th><th>Exit</th>\
         <th>Duration</th><th>Packages</th></tr>\n",
    );
    for t in &s.targets {
        let counts: Vec<String> = t.counts.iter().map(|(k, v)| format!("{v} {k}")).collect();
        let _ = writeln!(
            h,
            "<tr><td><a href=\"#target-{id}\">{name}</a></td><td>{cmd}</td>{status}\
             <td class=\"n\">{code}</td><td class=\"n\">{dur}</td><td>{counts}</td></tr>",
            id = esc(&t.name),
            name = esc(&t.name),
            cmd = esc(&t.command),
            status = status_cell(&t.status),
            code = t.exit_code,
            dur = duration_text(t.duration_seconds),
            counts = esc(&counts.join(", ")),
        );
    }
    h.push_str("</table>\n");
    for t in &s.targets {
        let _ = writeln!(
            h,
            "<h2 id=\"target-{id}\">{name}: <span class=\"{class}\">{status}</span></h2>\n\
             <p class=\"meta\"><code>{cmd}</code> of the {bt} build type, run id <code>{rid}</code>, \
             list <code>{list}</code>, from {from} to {to} ({dur}). Logs in <code>{logs}</code>.</p>",
            id = esc(&t.name),
            name = esc(&t.name),
            class = esc(&t.status),
            status = esc(&t.status),
            cmd = esc(&t.command),
            bt = esc(&t.build_type),
            rid = esc(&t.run_id),
            list = esc(&t.list),
            from = esc(&t.started_at),
            to = esc(&t.ended_at),
            dur = duration_text(t.duration_seconds),
            logs = esc(&t.log_dir),
        );
        if let Some(m) = &t.message {
            let _ = writeln!(h, "<p>{}</p>", esc(m));
        }
        h.push_str(
            "<h3>Phases</h3>\n<table>\n<tr><th>Phase</th><th>Status</th><th>Exit</th>\
             <th>Duration</th><th>Reason</th></tr>\n",
        );
        for p in &t.phases {
            let _ = writeln!(
                h,
                "<tr><td>{}</td>{}{}{}{}</tr>",
                esc(&p.name),
                status_cell(&p.status),
                num_cell(p.exit_code.map(|c| c.to_string())),
                num_cell(p.duration_seconds.map(duration_text)),
                opt_cell(p.reason.clone()),
            );
        }
        h.push_str("</table>\n");
        h.push_str(
            "<h3>Packages</h3>\n<table>\n<tr><th>Easyconfig</th><th>Module</th><th>Unit</th>\
             <th>Status</th><th>Exit</th><th>Duration</th><th>Log</th><th>Reason</th></tr>\n",
        );
        for p in &t.packages {
            let log = match href(&p.log, run_dir) {
                Some(rel) => format!(
                    "<td><a href=\"{}\">{}</a></td>",
                    esc(&rel),
                    esc(rel.rsplit('/').next().unwrap_or(&rel))
                ),
                None => format!("<td><code>{}</code></td>", esc(&p.log)),
            };
            let _ = writeln!(
                h,
                "<tr><td><code>{}</code></td>{}<td>{}</td>{}{}{}{log}{}</tr>",
                esc(&p.easyconfig),
                opt_cell(p.module.clone()),
                esc(&p.unit),
                status_cell(&p.status),
                num_cell(p.exit_code.map(|c| c.to_string())),
                num_cell(p.duration_seconds.map(duration_text)),
                opt_cell(p.reason.clone()),
            );
        }
        h.push_str("</table>\n");
        if !t.deselected.is_empty() {
            let _ = writeln!(
                h,
                "<p class=\"meta\">Left out by the target's selector: {}.</p>",
                t.deselected
                    .iter()
                    .map(|d| format!("<code>{}</code>", esc(d)))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        if let Some(w) = &t.watch {
            let text = serde_json::to_string_pretty(w).unwrap_or_default();
            let _ = writeln!(
                h,
                "<details><summary>The plan of <code>watch</code></summary>\n<pre>{}</pre>\n</details>",
                esc(&text)
            );
        }
    }
    h.push_str("</body>\n</html>\n");
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(easyconfig: &str, reason: &str) -> Summary {
        Summary {
            schema_version: SCHEMA_VERSION,
            kind: "summary".into(),
            tool: "eb-pipeline 0".into(),
            site: "t".into(),
            run: "20260101_000000".into(),
            updated_at: "2026-01-01T00:00:00+00:00".into(),
            exit_code: 1,
            status: "failed".into(),
            targets: vec![TargetSummary {
                name: "cpu".into(),
                command: "run".into(),
                build_type: "production".into(),
                run_id: "r1".into(),
                list: "/srv/list.txt".into(),
                log_dir: "/srv/logs/20260101_000000/cpu".into(),
                result: None,
                started_at: String::new(),
                ended_at: String::new(),
                duration_seconds: 75.4,
                exit_code: 1,
                status: "failed".into(),
                message: None,
                interrupted_by: None,
                counts: BTreeMap::new(),
                phases: vec![],
                packages: vec![PackageSummary {
                    easyconfig: easyconfig.into(),
                    module: None,
                    unit: "list".into(),
                    phase: "build".into(),
                    status: "failed".into(),
                    exit_code: Some(1),
                    duration_seconds: Some(3.0),
                    log: format!("/srv/logs/20260101_000000/cpu/{easyconfig}.stdout.log"),
                    reason: Some(reason.into()),
                }],
                deselected: vec![],
                watch: None,
            }],
        }
    }

    #[test]
    fn the_page_escapes_what_a_run_recorded() {
        let s = summary("A<b>&\"c\".eb", "<script>alert(1)</script>");
        let h = html(&s, Path::new("/srv/logs/20260101_000000"));
        assert!(!h.contains("<script"), "{h}");
        assert!(!h.contains("A<b>"), "{h}");
        assert!(h.contains("A&lt;b&gt;&amp;&quot;c&quot;.eb"), "{h}");
        assert!(
            h.contains("href=\"cpu/A%3Cb%3E%26%22c%22.eb.stdout.log\""),
            "{h}"
        );
    }

    #[test]
    fn a_log_outside_the_run_directory_is_named_but_not_linked() {
        let s = summary("Foo-1.0.eb", "x");
        let h = html(&s, Path::new("/elsewhere"));
        assert!(!h.contains("href=\"/srv"), "{h}");
        assert!(h.contains("<code>/srv/logs/20260101_000000/cpu/Foo-1.0.eb.stdout.log</code>"));
    }

    #[test]
    fn targets_combine_as_the_run_contract_orders_codes() {
        assert_eq!(combined_code(&[]), 0);
        assert_eq!(combined_code(&[0, 75, 78]), 78);
        assert_eq!(combined_code(&[78, 1, 0]), 1);
        assert_eq!(combined_code(&[1, 143]), 143);
        assert_eq!(combined_code(&[0, 75]), 75);
    }

    #[test]
    fn durations_read_as_seconds_minutes_or_hours() {
        assert_eq!(duration_text(4.04), "4.0 s");
        assert_eq!(duration_text(245.0), "4 min 05 s");
        assert_eq!(duration_text(7380.0), "2 h 03 min");
    }
}
