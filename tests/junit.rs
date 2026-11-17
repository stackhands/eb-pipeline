//! `eb-junit` turns `eb-pipeline run --result` files into a JUnit report.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-junit"))
}

fn result(exit_code: i32, lines: &str) -> String {
    format!(
        r#"{{
  "schema_version": 1,
  "kind": "result",
  "site": "example",
  "target": "cpu",
  "build_type": "production",
  "run_id": "nightly",
  "exit_code": {exit_code},
  "lines": [{lines}]
}}
"#
    )
}

#[test]
fn help_and_version_exit_zero() {
    let help = bin().arg("--help").output().unwrap();
    assert_eq!(help.status.code(), Some(0));
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.contains("eb-junit [--result FILE]... [--reframe-report FILE]... [--out FILE]"));
    assert!(text.contains("--reframe-report FILE"));
    assert!(text.contains("Exit 0 means the report was"));
    assert!(help.stderr.is_empty());

    let version = bin().arg("--version").output().unwrap();
    assert_eq!(version.status.code(), Some(0));
    let text = String::from_utf8(version.stdout).unwrap();
    assert!(text.starts_with("eb-junit "), "{text}");
}

#[test]
fn a_result_file_is_reported_and_left_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    let raw = result(
        1,
        r#"{"easyconfig":"Ok-1.0.eb","unit":"all","phase":"build","status":"success","log":"logs/ok.log","duration_seconds":0.5},
           {"easyconfig":"Bad.eb","unit":"all","phase":"build","status":"failed","exit_code":1,"log":"logs/bad.log"}"#,
    );
    std::fs::write(&path, &raw).unwrap();
    let out = dir.path().join("junit.xml");
    let o = bin()
        .arg("--result")
        .arg(&path)
        .arg("--out")
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(o.stdout.is_empty());
    assert!(o.stderr.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    let xml = std::fs::read_to_string(&out).unwrap();
    assert!(xml.contains("tests=\"2\" failures=\"1\" errors=\"0\" skipped=\"0\""));
    assert!(xml.contains(
        "<testcase name=\"cpu/Ok-1.0.eb\" classname=\"example/cpu\" file=\"Ok-1.0.eb\" time=\"0.500\"/>"
    ));
    assert!(!xml.contains("logs/ok.log"), "{xml}");
    assert!(xml.contains("<failure type=\"failed\" message=\"failed\">"));
    assert!(xml.contains("log: logs/bad.log"));
}

#[test]
fn a_plan_and_a_missing_file_are_usage_errors() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plan.json");
    std::fs::write(&path, "{\"kind\":\"plan\"}\n").unwrap();
    let o = bin().arg("--result").arg(&path).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(o.stdout.is_empty());
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.starts_with("eb-junit: "), "{err}");
    assert!(
        err.contains("expected kind `result`, found `plan`"),
        "{err}"
    );

    let missing = dir.path().join("missing.json");
    let o = bin().arg("--result").arg(&missing).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let err = String::from_utf8(o.stderr).unwrap();
    assert!(err.contains("cannot read"), "{err}");
}

#[test]
fn a_reframe_report_adds_suites() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("reframe.json");
    std::fs::write(
        &report,
        r#"{
  "report_version": "4.8",
  "runs": [
    {
      "system": "cluster",
      "partition": "cpu",
      "environ": "foss",
      "testcases": [
        {"name": "checks/ok", "result": "success", "time_total": 0.5},
        {"name": "checks/wait", "result": "skipped", "fail_reason": "maint"},
        {"name": "checks/fail", "result": "failure", "fail_reason": "wrong answer"}
      ]
    }
  ]
}"#,
    )
    .unwrap();
    let o = bin().arg("--reframe-report").arg(&report).output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let xml = String::from_utf8(o.stdout).unwrap();
    assert!(xml.contains("name=\"reframe/cluster/cpu/foss\""), "{xml}");
    assert!(xml.contains("name=\"checks/ok\""), "{xml}");
    assert!(xml.contains("<skipped message=\"skipped\">"), "{xml}");
    assert!(
        xml.contains("<failure type=\"failure\" message=\"failure\">"),
        "{xml}"
    );
}

#[test]
fn the_pages_name_eb_junit() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode");
    for rel in [
        "howto/publish-a-junit-report.org",
        "reference/junit.org",
        "reference/cli.org",
    ] {
        let page = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(page.contains("eb-junit"), "{rel}");
        assert!(page.contains("--reframe-report"), "{rel}");
    }
    let howto = std::fs::read_to_string(root.join("howto/index.org")).unwrap();
    assert!(howto.contains("publish-a-junit-report"), "{howto}");
    let reference = std::fs::read_to_string(root.join("reference/index.org")).unwrap();
    assert!(reference.contains("\n   junit\n"), "{reference}");
}
