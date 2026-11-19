//! Shared wrappers around `eb-stack stack bot-classify --json`.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};

/// One `stack bot-classify` verdict for one log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobClass {
    /// Top-level class.
    pub class: String,
    /// Sub-class.
    pub sub_class: String,
    /// Evidence lines from the classifier output.
    pub evidence: Vec<String>,
}

/// Classify one log with `eb-stack stack bot-classify --json`.
pub fn classify_log(log: &Path) -> Result<JobClass, String> {
    classify_log_with("eb-stack", log)
}

/// Classify one log with an explicit `eb-stack` binary.
pub fn classify_log_with(bin: &str, log: &Path) -> Result<JobClass, String> {
    let output = Command::new(bin)
        .args(["stack", "bot-classify", "--json"])
        .arg(log)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("cannot run {bin} for {}: {e}", log.display()))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        };
        return Err(format!(
            "{bin} failed for {} with {}{detail}",
            log.display(),
            output.status
        ));
    }
    let report: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("{bin} returned invalid JSON for {}: {e}", log.display()))?;
    let jobs = report
        .get("jobs")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{bin} JSON for {} has no `jobs` array", log.display()))?;
    if jobs.len() != 1 {
        return Err(format!(
            "{bin} JSON for {} classified {} jobs, expected 1",
            log.display(),
            jobs.len()
        ));
    }
    let job = jobs[0]
        .as_object()
        .ok_or_else(|| format!("{bin} JSON job for {} must be an object", log.display()))?;
    let class = job
        .get("class")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    let sub_class = job
        .get("sub_class")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_default();
    if class.is_empty() && sub_class.is_empty() {
        return Err(format!(
            "{bin} JSON job for {} lacks string `class` and `sub_class`",
            log.display()
        ));
    }
    let evidence = job
        .get("evidence")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(JobClass {
        class,
        sub_class,
        evidence,
    })
}

/// Infrastructure failure classes used by `eb-retry-list` safeguards.
pub fn infrastructure_class(job: &JobClass, log_text: &str) -> Option<&'static str> {
    let lower = log_text.to_ascii_lowercase();
    if lower.contains("command not found")
        || lower.contains("permission denied")
        || lower.contains("exit code 126")
        || lower.contains("exit code 127")
    {
        return Some("command-path");
    }
    if lower.contains("no space left on device") {
        return Some("disk-full");
    }
    if lower.contains("cannot read the eessi module tree")
        || (lower.contains("module tree") && lower.contains("no such file or directory"))
    {
        return Some("missing-module-tree");
    }
    let fetch_marker = lower.contains("download failed")
        || lower.contains("timed out")
        || lower.contains("could not resolve host")
        || lower.contains("temporary failure in name resolution")
        || lower.contains("connection refused")
        || lower.contains("connection reset");
    if job.sub_class == "source" && fetch_marker && !lower.contains("couldn't find file") {
        return Some("fetch-network");
    }
    None
}
