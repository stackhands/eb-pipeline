//! `eb-pipeline metrics` against a result file on disk.

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
