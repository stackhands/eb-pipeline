//! `eb-pipeline metrics` against a result file on disk.
#![allow(clippy::disallowed_methods)]

use serde_json::Value;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
}

fn result() -> &'static str {
    r#"{
  "kind": "result",
  "site": "small-lab",
  "target": "cpu",
  "build_type": "production",
  "run_id": "demo",
  "sharded": false,
  "exit_code": 1,
  "ended_at": "2026-11-17T20:00:00Z",
  "duration_seconds": 12.5,
  "counts": {"success": 2, "failed": 1},
  "phases": [{"name": "build", "status": "failed"}],
  "lines": [{"log": "/secret/path/Foo.log"}]
}
"#
}

#[test]
fn metrics_prints_prometheus_for_a_failed_run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    std::fs::write(&path, result()).unwrap();
    let o = bin()
        .args(["metrics", "--result"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(text.contains("eb_pipeline_exit_code{"), "{text}");
    assert!(text.contains("} 1\n"), "{text}");
    assert!(text.contains("status=\"failed\"} 1\n"), "{text}");
    assert!(text.contains("status=\"success\"} 2\n"), "{text}");
    assert!(text.contains("status=\"pending\"} 0\n"), "{text}");
    assert!(
        text.contains("eb_pipeline_last_run_timestamp_seconds{"),
        "{text}"
    );
    assert!(
        text.contains("eb_pipeline_last_run_duration_seconds{"),
        "{text}"
    );
    assert!(text.contains("eb_pipeline_items_built{"), "{text}");
    assert!(text.contains("eb_pipeline_items_failed{"), "{text}");
    assert!(!text.contains("retried"), "{text}");
    assert!(!text.contains("/secret/path"), "{text}");
    assert!(
        o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn metrics_json_is_a_status_and_a_plan_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    std::fs::write(&path, result()).unwrap();
    let o = bin()
        .args(["metrics", "--result"])
        .arg(&path)
        .args(["--format", "json"])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["kind"], "status");
    assert_eq!(v["ok"], false);
    assert_eq!(v["counts"]["failed"], 1);
    assert!(v.get("lines").is_none());

    std::fs::write(&path, "{\"kind\":\"plan\"}\n").unwrap();
    let o = bin()
        .args(["metrics", "--result"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(o.stdout.is_empty());
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.contains("kind `plan`"), "{err}");
}

#[test]
fn metrics_refuses_an_unknown_result_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    let text = result().replacen('{', r#"{"schema_version": 99,"#, 1);
    std::fs::write(&path, text).unwrap();

    let output = bin()
        .args(["metrics", "--result"])
        .arg(&path)
        .args(["--format", "json"])
        .output()
        .unwrap();

    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("result.schema_version must be 1"), "{error}");
}

fn with_schema_version(value: &str) -> String {
    result().replacen('{', &format!(r#"{{"schema_version": {value},"#), 1)
}

fn metrics_of(text: &str) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    std::fs::write(&path, text).unwrap();
    bin()
        .args(["metrics", "--result"])
        .arg(&path)
        .args(["--format", "json"])
        .output()
        .unwrap()
}

#[test]
fn metrics_accepts_an_explicit_result_schema_version_1() {
    let explicit = metrics_of(&with_schema_version("1"));
    assert_eq!(
        explicit.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&explicit.stderr)
    );
    let omitted = metrics_of(result());
    assert_eq!(omitted.status.code(), Some(0));
    assert_eq!(explicit.stdout, omitted.stdout);
    let status: Value = serde_json::from_slice(&explicit.stdout).unwrap();
    assert_eq!(status["kind"], "status");
}

#[test]
fn metrics_refuses_a_result_schema_version_that_is_not_the_integer_1() {
    for value in [r#""1""#, "null", "1.0", "true", "[1]", "-1", "0"] {
        let output = metrics_of(&with_schema_version(value));
        let error = String::from_utf8_lossy(&output.stderr);
        assert_eq!(output.status.code(), Some(2), "{value}: {error}");
        assert!(output.stdout.is_empty(), "{value}");
        assert!(
            error.contains("result.schema_version must be 1"),
            "{value}: {error}"
        );
    }
}
