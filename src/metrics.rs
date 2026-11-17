//! Prometheus text and a short JSON status from one pipeline result file.
//!
//! Rendering does not start a build and does not call another tool. A failed
//! run is still a successful rendering: the exit code in the file is data.

use crate::exit::Fail;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Line statuses a result records, in label order.
pub const LINE_STATUSES: [&str; 5] = ["failed", "pending", "refused", "skipped", "success"];

/// The fields a status export keeps. Log paths stay in the result file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    /// Site name recorded by the pipeline.
    pub site: String,
    /// Build target recorded by the pipeline.
    pub target: String,
    /// Build type recorded by the pipeline.
    pub build_type: String,
    /// Run identifier recorded by the pipeline.
    pub run_id: String,
    /// Whether this result belongs to a sharded run.
    pub sharded: bool,
    /// Pipeline process exit code.
    pub exit_code: i32,
    /// Number of lines in each terminal status.
    pub counts: BTreeMap<String, u64>,
    /// Phase names and their recorded statuses.
    pub phases: Vec<(String, String)>,
}

impl Status {
    /// Return whether the pipeline process exited successfully.
    pub fn ok(&self) -> bool {
        self.exit_code == 0
    }
}

/// Read the status fields of a result object.
pub fn from_result(v: &Value) -> Result<Status, Fail> {
    let obj = v
        .as_object()
        .ok_or_else(|| Fail::usage("result must be a JSON object"))?;
    let kind = need_str(obj, "kind")?;
    if kind != "result" {
        return Err(Fail::usage(format!(
            "metrics reads a result file, found kind `{kind}`"
        )));
    }
    let exit_code = obj
        .get("exit_code")
        .and_then(Value::as_i64)
        .ok_or_else(|| Fail::usage("result.exit_code must be an integer"))?;
    let exit_code =
        i32::try_from(exit_code).map_err(|_| Fail::usage("result.exit_code is out of range"))?;
    let sharded = obj
        .get("sharded")
        .and_then(Value::as_bool)
        .ok_or_else(|| Fail::usage("result.sharded must be a boolean"))?;

    let mut counts = BTreeMap::new();
    for name in LINE_STATUSES {
        counts.insert(name.to_string(), 0);
    }
    if let Some(raw) = obj.get("counts") {
        let raw = raw
            .as_object()
            .ok_or_else(|| Fail::usage("result.counts must be an object"))?;
        for (key, value) in raw {
            let n = value.as_u64().ok_or_else(|| {
                Fail::usage(format!(
                    "result.counts.{key} must be a non-negative integer"
                ))
            })?;
            counts.insert(key.clone(), n);
        }
    }

    let mut phases = Vec::new();
    if let Some(raw) = obj.get("phases") {
        let raw = raw
            .as_array()
            .ok_or_else(|| Fail::usage("result.phases must be an array"))?;
        for (i, item) in raw.iter().enumerate() {
            let item = item
                .as_object()
                .ok_or_else(|| Fail::usage(format!("result.phases[{i}] must be an object")))?;
            let name = need_str(item, "name")
                .map_err(|_| Fail::usage(format!("result.phases[{i}].name must be a string")))?;
            let status = need_str(item, "status")
                .map_err(|_| Fail::usage(format!("result.phases[{i}].status must be a string")))?;
            phases.push((name.to_string(), status.to_string()));
        }
    }

    Ok(Status {
        site: need_str(obj, "site")?.to_string(),
        target: need_str(obj, "target")?.to_string(),
        build_type: need_str(obj, "build_type")?.to_string(),
        run_id: need_str(obj, "run_id")?.to_string(),
        sharded,
        exit_code,
        counts,
        phases,
    })
}

/// Prometheus text exposition, one sample set for this run.
pub fn prometheus(status: &Status) -> String {
    let sharded = if status.sharded { "true" } else { "false" };
    let id = [
        ("site", status.site.as_str()),
        ("target", status.target.as_str()),
        ("build_type", status.build_type.as_str()),
        ("run_id", status.run_id.as_str()),
        ("sharded", sharded),
    ];
    let mut out = String::new();
    family(
        &mut out,
        "eb_pipeline_info",
        "One pipeline run.",
        &[sample("eb_pipeline_info", &id, "1")],
    );
    family(
        &mut out,
        "eb_pipeline_exit_code",
        "Process exit code of the run.",
        &[sample(
            "eb_pipeline_exit_code",
            &id,
            &status.exit_code.to_string(),
        )],
    );
    let mut lines = Vec::new();
    for (name, n) in &status.counts {
        let mut labels = id.to_vec();
        labels.push(("status", name.as_str()));
        lines.push(sample("eb_pipeline_lines", &labels, &n.to_string()));
    }
    family(
        &mut out,
        "eb_pipeline_lines",
        "Build lines by status. A status the run did not use is 0.",
        &lines,
    );
    let mut phases = Vec::new();
    for (name, phase_status) in &status.phases {
        let mut labels = id.to_vec();
        labels.push(("phase", name.as_str()));
        labels.push(("status", phase_status.as_str()));
        phases.push(sample("eb_pipeline_phase", &labels, "1"));
    }
    family(
        &mut out,
        "eb_pipeline_phase",
        "Recorded outcome of one phase.",
        &phases,
    );
    out
}

/// Short JSON status. Log paths and command lines are left out.
pub fn status_value(status: &Status) -> Value {
    let mut counts = Map::new();
    for (k, n) in &status.counts {
        counts.insert(k.clone(), Value::from(*n));
    }
    let phases: Vec<Value> = status
        .phases
        .iter()
        .map(|(name, phase_status)| {
            let mut m = Map::new();
            m.insert("name".into(), Value::String(name.clone()));
            m.insert("status".into(), Value::String(phase_status.clone()));
            Value::Object(m)
        })
        .collect();
    let mut m = Map::new();
    m.insert(
        "build_type".into(),
        Value::String(status.build_type.clone()),
    );
    m.insert("counts".into(), Value::Object(counts));
    m.insert("exit_code".into(), Value::from(status.exit_code));
    m.insert("kind".into(), Value::String("status".into()));
    m.insert("ok".into(), Value::Bool(status.ok()));
    m.insert("phases".into(), Value::Array(phases));
    m.insert("run_id".into(), Value::String(status.run_id.clone()));
    m.insert("schema_version".into(), Value::from(1));
    m.insert("sharded".into(), Value::Bool(status.sharded));
    m.insert("site".into(), Value::String(status.site.clone()));
    m.insert("target".into(), Value::String(status.target.clone()));
    Value::Object(m)
}

/// Pretty JSON status, ending in one newline.
pub fn status_json(status: &Status) -> String {
    serde_json::to_string_pretty(&status_value(status)).expect("status is json") + "\n"
}

fn need_str<'a>(obj: &'a Map<String, Value>, key: &str) -> Result<&'a str, Fail> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Fail::usage(format!("result.{key} must be a string")))
}

/// Escape a label value the way the Prometheus text format requires.
fn escape_label(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

fn sample(name: &str, labels: &[(&str, &str)], value: &str) -> String {
    let mut inner = String::new();
    for (i, (key, label)) in labels.iter().enumerate() {
        if i > 0 {
            inner.push(',');
        }
        inner.push_str(key);
        inner.push_str("=\"");
        inner.push_str(&escape_label(label));
        inner.push('"');
    }
    format!("{name}{{{inner}}} {value}\n")
}

fn family(out: &mut String, name: &str, help: &str, samples: &[String]) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str("# HELP ");
    out.push_str(name);
    out.push(' ');
    out.push_str(help);
    out.push('\n');
    out.push_str("# TYPE ");
    out.push_str(name);
    out.push_str(" gauge\n");
    for sample in samples {
        out.push_str(sample);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result() -> Value {
        serde_json::json!({
            "kind": "result",
            "site": "small-lab",
            "target": "cpu",
            "build_type": "production",
            "run_id": "demo",
            "sharded": false,
            "exit_code": 1,
            "counts": {"failed": 1, "success": 2},
            "phases": [
                {"name": "build", "status": "failed"},
                {"name": "checks", "status": "skipped"}
            ],
            "lines": [{"log": "/secret/path/Foo.log", "status": "failed"}]
        })
    }

    const ID: &str = "site=\"small-lab\",target=\"cpu\",build_type=\"production\",run_id=\"demo\",sharded=\"false\"";

    #[test]
    fn a_failed_run_is_prometheus_text_without_log_paths() {
        let status = from_result(&result()).unwrap();
        let text = prometheus(&status);
        let expected = format!(
            "\
# HELP eb_pipeline_info One pipeline run.
# TYPE eb_pipeline_info gauge
eb_pipeline_info{{{id}}} 1

# HELP eb_pipeline_exit_code Process exit code of the run.
# TYPE eb_pipeline_exit_code gauge
eb_pipeline_exit_code{{{id}}} 1

# HELP eb_pipeline_lines Build lines by status. A status the run did not use is 0.
# TYPE eb_pipeline_lines gauge
eb_pipeline_lines{{{id},status=\"failed\"}} 1
eb_pipeline_lines{{{id},status=\"pending\"}} 0
eb_pipeline_lines{{{id},status=\"refused\"}} 0
eb_pipeline_lines{{{id},status=\"skipped\"}} 0
eb_pipeline_lines{{{id},status=\"success\"}} 2

# HELP eb_pipeline_phase Recorded outcome of one phase.
# TYPE eb_pipeline_phase gauge
eb_pipeline_phase{{{id},phase=\"build\",status=\"failed\"}} 1
eb_pipeline_phase{{{id},phase=\"checks\",status=\"skipped\"}} 1
",
            id = ID
        );
        assert_eq!(text, expected);
        assert!(!text.contains("/secret/path"));
        assert!(!status.ok());
    }

    #[test]
    fn label_values_escape_quotes_backslashes_and_newlines() {
        let mut v = result();
        v["site"] = Value::String("a\"b\\c\nd".into());
        let text = prometheus(&from_result(&v).unwrap());
        assert!(text.contains(r#"site="a\"b\\c\nd""#), "{text}");
    }

    #[test]
    fn json_status_fills_missing_line_counts_and_drops_logs() {
        let text = status_json(&from_result(&result()).unwrap());
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["kind"], "status");
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["ok"], false);
        assert_eq!(v["exit_code"], 1);
        assert_eq!(v["counts"]["failed"], 1);
        assert_eq!(v["counts"]["pending"], 0);
        assert_eq!(v["counts"]["success"], 2);
        assert_eq!(v["phases"][0]["name"], "build");
        assert_eq!(v["phases"][1]["status"], "skipped");
        assert!(v.get("lines").is_none());
        assert!(!text.contains("/secret/path"));
    }

    #[test]
    fn a_plan_file_is_not_a_result() {
        let v = serde_json::json!({"kind": "plan", "exit_code": 0});
        let err = from_result(&v).unwrap_err();
        assert_eq!(err.code, 2);
        assert!(err.msg.contains("kind `plan`"), "{}", err.msg);
    }

    #[test]
    fn a_fractional_count_is_refused() {
        let mut v = result();
        v["counts"]["failed"] = serde_json::json!(1.5);
        let err = from_result(&v).unwrap_err();
        assert!(err.msg.contains("result.counts.failed"), "{}", err.msg);
    }

    #[test]
    fn a_missing_exit_code_is_refused() {
        let mut v = result();
        v.as_object_mut().unwrap().remove("exit_code");
        let err = from_result(&v).unwrap_err();
        assert!(err.msg.contains("exit_code"), "{}", err.msg);
    }
}
