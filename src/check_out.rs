//! JSON document for `eb-pipeline check --format json`.
//!
//! Warnings stay on stderr as today; the document on stdout is for scripts.

use crate::exit::{self, Fail};
use crate::schema::Site;
use crate::site;
use crate::tools;
use serde_json::{json, Value};
use std::path::Path;

/// Version of [`document`]'s format.
pub const SCHEMA_VERSION: u32 = 1;

/// Parse and validate a site file, optionally probe tools, and build one JSON
/// value with exit code `code`.
pub fn document(
    site_path: &Path,
    lists_dir: Option<&Path>,
    probe: bool,
) -> Result<(Value, i32), Fail> {
    let mut warnings: Vec<String> = Vec::new();
    let s = match site::load(site_path) {
        Ok(s) => s,
        Err(f) => {
            return Ok((
                envelope(false, None, vec![f.msg.clone()], &warnings, probe),
                f.code,
            ));
        }
    };
    let mut problems = site::validate(&s, lists_dir);
    if !problems.is_empty() {
        return Ok((
            envelope(
                false,
                Some(s.site.name.as_str()),
                std::mem::take(&mut problems),
                &warnings,
                probe,
            ),
            exit::USAGE,
        ));
    }
    collect_warnings(&s, &mut warnings);
    if probe {
        if let Err(f) = tools::check_versions(&s, true) {
            problems.push(f.msg);
            return Ok((
                envelope(false, Some(s.site.name.as_str()), problems, &warnings, true),
                f.code,
            ));
        }
    }
    Ok((
        envelope(
            true,
            Some(s.site.name.as_str()),
            Vec::new(),
            &warnings,
            probe,
        ),
        exit::OK,
    ))
}

fn envelope(
    valid: bool,
    site: Option<&str>,
    problems: Vec<String>,
    warnings: &[String],
    probe_tools: bool,
) -> Value {
    let warn: Vec<Value> = warnings.iter().map(|m| json!({"message": m})).collect();
    json!({
        "schema_version": SCHEMA_VERSION,
        "kind": "check",
        "valid": valid,
        "site": site,
        "probe_tools": probe_tools,
        "problems": problems,
        "warnings": warn,
    })
}

fn collect_warnings(s: &Site, out: &mut Vec<String>) {
    let m = &s.modules;
    if let Some(first) = m
        .init
        .first()
        .filter(|f| m.unuse_path && !f.starts_with('/'))
    {
        out.push(format!(
            "modules.unuse_path clears MODULEPATH after shell_init, so `{first}` has no path to load from; load it by absolute path or drop unuse_path"
        ));
    }
    if let Some(a) = s.admission_on() {
        if a.required && (a.quota_cmd.is_none() || a.reservations.is_none()) {
            out.push(
                "admission.required is set without quota_cmd and reservations; `run` will refuse with exit 78"
                    .into(),
            );
        }
        if a.live_jobs_cmd.is_none() && a.quota_cmd.is_some() && a.reservations.is_some() {
            out.push(
                "admission.live_jobs_cmd is unset; stack admit then counts every other job's reservation, including those of jobs that ended without removing theirs"
                    .into(),
            );
        }
    }
}
