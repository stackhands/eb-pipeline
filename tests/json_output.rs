//! Command stdout is checked against the JSON Schemas in
//! `docs/orgmode/reference/json/`.

use jsonschema::Validator;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
}

fn schema_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("docs/orgmode/reference/json")
        .join(name)
}

fn parse_stdout(o: &std::process::Output) -> Value {
    let text = String::from_utf8(o.stdout.clone()).expect("utf-8 stdout");
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("stdout is not json: {e}\n{text}"))
}

fn assert_schema(value: &Value, schema_file: &str) {
    let text = std::fs::read_to_string(schema_path(schema_file))
        .unwrap_or_else(|e| panic!("{}: {e}", schema_path(schema_file).display()));
    let schema: Value = serde_json::from_str(&text).expect("schema json");
    let validator = Validator::new(&schema).expect("schema compiles");
    let errors: Vec<String> = validator
        .iter_errors(value)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{}: {}", schema_file, errors.join("; "));
}

fn example(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("templates")
        .join(name)
}

#[test]
fn check_json_validates_against_schema() {
    let o = bin()
        .args([
            "check",
            "--format",
            "json",
            "--site",
            &example("small-cluster.toml").display().to_string(),
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    let v = parse_stdout(&o);
    assert_schema(&v, "check.schema.json");
    assert_eq!(v["valid"], true);
}

#[test]
fn check_json_reports_invalid_site() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.toml");
    std::fs::write(&bad, "kind = \"not-site\"\n").unwrap();
    let o = bin()
        .args(["check", "--format", "json", "--site"])
        .arg(&bad)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    let v = parse_stdout(&o);
    assert_schema(&v, "check.schema.json");
    assert_eq!(v["valid"], false);
    assert!(!v["problems"].as_array().unwrap().is_empty());
}

#[test]
fn init_list_json_validates() {
    let o = bin()
        .args(["init", "--list", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_schema(&parse_stdout(&o), "templates.schema.json");
}

#[test]
fn targets_json_validates() {
    let o = bin()
        .args([
            "targets",
            "--format",
            "json",
            "--site",
            &example("small-cluster.toml").display().to_string(),
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    let v = parse_stdout(&o);
    assert_schema(&v, "targets.schema.json");
}

#[test]
fn status_json_validates() {
    let o = bin()
        .args([
            "status",
            "--format",
            "json",
            "--site",
            &example("small-cluster.toml").display().to_string(),
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_schema(&parse_stdout(&o), "status.schema.json");
}

#[test]
fn explain_json_validates() {
    let o = bin()
        .args([
            "explain",
            "--format",
            "json",
            "--site",
            &example("small-cluster.toml").display().to_string(),
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_schema(&parse_stdout(&o), "explain.schema.json");
}

#[test]
fn plan_json_validates() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("l.txt");
    std::fs::write(&list, "Foo-1.0-GCCcore-15.2.0.eb\n").unwrap();
    let o = bin()
        .args([
            "plan",
            "--site",
            &example("small-cluster.toml").display().to_string(),
            "--list",
            &list.display().to_string(),
            "--target",
            "cpu",
            "--build-type",
            "production",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_schema(&parse_stdout(&o), "plan.schema.json");
}

#[test]
fn metrics_json_validates_from_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        r#"{
  "kind": "result",
  "schema_version": 1,
  "site": "example",
  "target": "cpu",
  "build_type": "production",
  "run_id": "demo",
  "sharded": false,
  "exit_code": 0,
  "counts": {"success": 1, "failed": 0, "pending": 0, "refused": 0, "skipped": 0},
  "phases": [{"name": "build", "status": "ok"}],
  "lines": []
}"#,
    )
    .unwrap();
    let o = bin()
        .args([
            "metrics",
            "--format",
            "json",
            "--result",
            &result.display().to_string(),
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_schema(&parse_stdout(&o), "metrics-status.schema.json");
}

#[test]
fn check_json_default_stdout_stays_empty() {
    let o = bin()
        .args(["check", "--site"])
        .arg(example("small-cluster.toml"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert!(
        o.stdout.is_empty(),
        "stdout is reserved without --format json"
    );
}
