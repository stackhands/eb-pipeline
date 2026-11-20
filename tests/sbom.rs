//! After a clean build, `eb-stack stack sbom --lock` writes CycloneDX beside the summary.

mod common;
use common::{code, run, run_extra, setup, stderr, write_exec, STAMP};

const LIST: &str = "Foo-1.0-foss-2025a.eb\n";
const EMPTY_SBOM: &str = r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--out" ]; then out=$2; shift; fi
  shift
done
: > "$out"
"#;
const TRUNCATED_SBOM: &str = r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--out" ]; then out=$2; shift; fi
  shift
done
printf '{' > "$out"
"#;

#[test]
fn a_clean_build_stores_the_cyclonedx_file_beside_the_summary() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    std::fs::write(t.root().join("stack.lock.json"), "{}\n").unwrap();
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let doc = t.run_dir(STAMP).join("sbom.cdx.json");
    let text = std::fs::read_to_string(&doc).unwrap_or_else(|e| panic!("{e}: {}", stderr(&o)));
    assert!(text.contains("CycloneDX"), "{text}");
    let calls = t.calls("ebstack");
    assert!(calls.contains("stack sbom"), "{calls}");
    assert!(calls.contains("--lock"), "{calls}");
    assert!(
        calls.contains(doc.to_str().unwrap()),
        "{calls} does not name {}",
        doc.display()
    );
    let summary = t.summary(STAMP);
    assert_eq!(summary["exit_code"], 0);
}

#[test]
fn a_relative_lock_is_refused_before_any_build() {
    let t = setup("[sbom]\nlock = \"stack.lock.json\"\n");
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 2, "{}", stderr(&o));
    assert!(
        stderr(&o).contains("not an absolute path"),
        "{}",
        stderr(&o)
    );
    assert!(!t.calls("ebstack").contains("stack sbom"));
}

#[test]
fn a_failed_build_does_not_ask_for_an_sbom() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    let before = t.calls("ebstack");
    let o = run(
        &t,
        "cpu",
        STAMP,
        LIST,
        &[("STUB_EB_FAIL", "Foo-1.0-foss-2025a")],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert_eq!(t.calls("ebstack"), before);
    assert!(!t.run_dir(STAMP).join("sbom.cdx.json").exists());
}

#[test]
fn an_sbom_command_that_fails_fails_the_run_and_leaves_no_document() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    let o = run(&t, "cpu", STAMP, LIST, &[("STUB_SBOM_FAIL", "1")]);
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    assert!(stderr(&o).contains("stack sbom"), "{}", stderr(&o));
    assert!(!t.run_dir(STAMP).join("sbom.cdx.json").exists());
    assert_eq!(t.summary(STAMP)["exit_code"], 1);
}

#[test]
fn an_sbom_command_status_outside_the_contract_fails_the_run_with_1() {
    for status in ["2", "3", "75", "78"] {
        let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
        let o = run(&t, "cpu", STAMP, LIST, &[("STUB_SBOM_FAIL", status)]);
        assert_eq!(code(&o), 1, "sbom exit {status}: {}", stderr(&o));
        assert!(
            stderr(&o).contains(&format!("exited {status}")),
            "sbom exit {status}: {}",
            stderr(&o)
        );
        assert!(!t.run_dir(STAMP).join("sbom.cdx.json").exists());
        assert_eq!(t.summary(STAMP)["exit_code"], 1, "sbom exit {status}");
    }
}

#[test]
fn a_success_without_a_new_document_does_not_accept_a_stale_one() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    let document = t.run_dir(STAMP).join("sbom.cdx.json");
    std::fs::create_dir_all(document.parent().unwrap()).unwrap();
    std::fs::write(&document, "stale\n").unwrap();
    let output = run(&t, "cpu", STAMP, LIST, &[("STUB_SBOM_SKIP_WRITE", "1")]);
    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(stderr(&output).contains("wrote no"), "{}", stderr(&output));
    assert!(!document.exists());
    assert_eq!(t.summary(STAMP)["exit_code"], 1);
}

#[test]
fn a_success_with_an_empty_document_fails_the_run() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    write_exec(&t.stubs.join("eb-stack"), EMPTY_SBOM);
    let output = run(&t, "cpu", STAMP, LIST, &[]);
    let document = t.run_dir(STAMP).join("sbom.cdx.json");

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("wrote no non-empty"),
        "{}",
        stderr(&output)
    );
    assert!(!document.exists());
    assert_eq!(t.summary(STAMP)["exit_code"], 1);
}

#[test]
fn a_success_with_truncated_json_fails_the_run() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    write_exec(&t.stubs.join("eb-stack"), TRUNCATED_SBOM);
    let output = run(&t, "cpu", STAMP, LIST, &[]);
    let document = t.run_dir(STAMP).join("sbom.cdx.json");

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("wrote invalid JSON"),
        "{}",
        stderr(&output)
    );
    assert!(!document.exists());
    assert_eq!(t.summary(STAMP)["exit_code"], 1);
}

#[test]
fn a_signalled_sbom_command_preserves_the_signal_exit_code() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    std::fs::write(t.root().join("stack.lock.json"), "{}\n").unwrap();
    let metrics = t.root().join("pipeline.prom");
    let o = run_extra(
        &t,
        "cpu",
        STAMP,
        LIST,
        &["--metrics-file", metrics.to_str().unwrap()],
        &[("STUB_SBOM_SIGNAL", "TERM")],
    );

    assert_eq!(code(&o), 143, "{}", stderr(&o));
    assert!(stderr(&o).contains("exited 143"), "{}", stderr(&o));
    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(t.root().join("result-cpu.json")).unwrap())
            .unwrap();
    assert_eq!(result["exit_code"], 143);
    assert_eq!(t.summary(STAMP)["exit_code"], 143);
    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_pipeline_exit_code"), 143.0, "{text}");
}

#[test]
fn final_times_and_metrics_include_an_sbom_failure() {
    let t = setup("[sbom]\nlock = \"@ROOT@/stack.lock.json\"\n");
    std::fs::write(t.root().join("stack.lock.json"), "{}\n").unwrap();
    let metrics = t.root().join("pipeline.prom");
    let metrics_arg = metrics.to_str().unwrap();
    let o = run_extra(
        &t,
        "cpu",
        STAMP,
        LIST,
        &["--metrics-file", metrics_arg],
        &[("STUB_SBOM_DELAY", "0.2"), ("STUB_SBOM_FAIL", "1")],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));

    let result: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(t.root().join("result-cpu.json")).unwrap())
            .unwrap();
    assert_eq!(result["exit_code"], 1);
    let duration = result["duration_seconds"].as_f64().unwrap();
    assert!(duration >= 0.2, "{result}");
    let started =
        chrono::DateTime::parse_from_rfc3339(result["started_at"].as_str().unwrap()).unwrap();
    let ended = chrono::DateTime::parse_from_rfc3339(result["ended_at"].as_str().unwrap()).unwrap();
    let wall_seconds = (ended - started).num_milliseconds() as f64 / 1000.0;
    assert!(wall_seconds >= 0.2, "{result}");
    assert!((wall_seconds - duration).abs() < 0.1, "{result}");

    let text = std::fs::read_to_string(metrics).unwrap();
    assert_eq!(sample(&text, "eb_pipeline_exit_code"), 1.0, "{text}");
    assert_eq!(sample(&text, "eb_pipeline_last_run_result"), 0.0, "{text}");
    assert_eq!(
        sample(&text, "eb_pipeline_last_run_duration_seconds"),
        duration,
        "{text}"
    );
    let ended_seconds =
        ended.timestamp() as f64 + f64::from(ended.timestamp_subsec_nanos()) / 1_000_000_000.0;
    assert_eq!(
        sample(&text, "eb_pipeline_last_run_timestamp_seconds"),
        ended_seconds,
        "{text}"
    );
}

fn sample(text: &str, family: &str) -> f64 {
    text.lines()
        .find(|line| line.starts_with(&format!("{family}{{")))
        .and_then(|line| line.split_whitespace().last())
        .unwrap()
        .parse()
        .unwrap()
}
