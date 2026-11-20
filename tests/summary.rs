//! Every `run` merges its target into `summary.json` and `summary.html` in
//! its run directory.

mod common;
use common::{code, run, setup, stderr, STAMP};
use eb_pipeline::summary::{record, TargetSummary, JSON};
use serde_json::Value;
use std::collections::BTreeMap;
use std::os::unix::fs::symlink;

const LIST: &str = "Foo-1.0-foss-2025a.eb\nBar-2.0-foss-2025a.eb\n";

fn named<'a>(v: &'a Value, key: &str, field: &str, name: &str) -> &'a Value {
    v[key]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x[field] == name)
        .unwrap_or_else(|| panic!("no {name} in {key}: {v}"))
}

#[test]
fn a_run_writes_its_summary_as_json_and_html() {
    let t = setup("");
    let o = run(
        &t,
        "cpu",
        STAMP,
        LIST,
        &[
            ("STUB_EB_FAIL", "Bar-2.0-foss-2025a"),
            ("STUB_ROTATE_EXIT", "75"),
        ],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    let run_dir = t.run_dir(STAMP);
    assert!(
        stderr(&o).contains(&format!(
            "INFO [summary] cpu: summary.json and summary.html written in {}\n",
            run_dir.display()
        )),
        "{}",
        stderr(&o)
    );
    let s = t.summary(STAMP);
    assert_eq!(s["schema_version"], 1);
    assert_eq!(s["kind"], "summary");
    assert_eq!(s["site"], "t");
    assert_eq!(s["run"], STAMP);
    assert_eq!(s["exit_code"], 1);
    assert_eq!(s["status"], "failed");
    let cpu = named(&s, "targets", "name", "cpu");
    assert_eq!(cpu["command"], "run");
    assert_eq!(cpu["status"], "failed");
    assert_eq!(cpu["counts"]["success"], 1);
    assert_eq!(cpu["counts"]["failed"], 1);
    assert!(cpu["duration_seconds"].as_f64().is_some(), "{cpu}");

    let foo = named(cpu, "packages", "easyconfig", "Foo-1.0-foss-2025a.eb");
    assert_eq!(foo["status"], "success");
    assert_eq!(foo["module"], "Foo/1.0-foss-2025a");
    assert!(foo["duration_seconds"].as_f64().is_some(), "{foo}");
    let foo_log = run_dir.join("cpu/Foo-1.0-foss-2025a.stdout.log");
    assert_eq!(foo["log"], foo_log.display().to_string());
    assert!(foo_log.is_file());
    let bar = named(cpu, "packages", "easyconfig", "Bar-2.0-foss-2025a.eb");
    assert_eq!(bar["status"], "failed");
    assert_eq!(bar["exit_code"], 1);

    let logs = named(cpu, "phases", "name", "logs");
    assert_eq!(logs["status"], "skipped");
    assert_eq!(logs["exit_code"], 75);
    let why = logs["reason"].as_str().unwrap();
    assert!(why.contains(".eb-keep-logs-rotate.lock"), "{why}");
    assert_eq!(
        named(cpu, "phases", "name", "admission")["status"],
        "not configured"
    );

    let html = std::fs::read_to_string(run_dir.join("summary.html")).unwrap();
    assert!(html.starts_with("<!DOCTYPE html>"), "{html}");
    assert!(html.contains("<h2 id=\"target-cpu\">cpu: <span class=\"failed\">failed</span></h2>"));
    assert!(
        html.contains("<a href=\"cpu/Foo-1.0-foss-2025a.stdout.log\">"),
        "{html}"
    );
    assert!(html.contains("eb-keep-logs rotate exited 75"), "{html}");
    assert!(
        html.contains("<td class=\"skipped\">skipped</td>"),
        "{html}"
    );
    for remote in ["<script", "http:", "https:", "src="] {
        assert!(!html.contains(remote), "{remote} in {html}");
    }
}

#[test]
fn targets_that_share_a_log_stamp_share_one_summary() {
    let t = setup("");
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let o = run(
        &t,
        "gpu",
        STAMP,
        LIST,
        &[("STUB_EB_FAIL", "Foo-1.0-foss-2025a")],
    );
    assert_eq!(code(&o), 1, "{}", stderr(&o));
    let s = t.summary(STAMP);
    let names: Vec<&str> = s["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["cpu", "gpu"]);
    assert_eq!(named(&s, "targets", "name", "cpu")["status"], "success");
    assert_eq!(s["exit_code"], 1);

    // a later run of the same target in the same directory replaces its entry
    let o = run(&t, "gpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let s = t.summary(STAMP);
    assert_eq!(s["targets"].as_array().unwrap().len(), 2);
    assert_eq!(s["exit_code"], 0);
    assert_eq!(s["status"], "success");
    let html = std::fs::read_to_string(t.run_dir(STAMP).join("summary.html")).unwrap();
    assert!(html.contains("id=\"target-gpu\""), "{html}");
    assert!(html.contains("id=\"target-cpu\""), "{html}");
}

#[test]
fn a_corrupt_summary_is_not_replaced_by_one_target() {
    let t = setup("");
    let cpu = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&cpu), 0, "{}", stderr(&cpu));
    let gpu = run(&t, "gpu", STAMP, LIST, &[]);
    assert_eq!(code(&gpu), 0, "{}", stderr(&gpu));
    let path = t.run_dir(STAMP).join("summary.json");
    let corrupt = b"{not json\n";
    std::fs::write(&path, corrupt).unwrap();

    let repeated = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&repeated), 0, "{}", stderr(&repeated));
    assert_eq!(std::fs::read(&path).unwrap(), corrupt);
    assert!(
        stderr(&repeated).contains("no summary written: cannot parse"),
        "{}",
        stderr(&repeated)
    );
}

#[test]
fn a_dry_run_writes_no_summary() {
    let t = setup("");
    let list = t.root().join("list.txt");
    std::fs::write(&list, LIST).unwrap();
    let o = common::eb_pipeline(
        &t,
        &[
            "run",
            "--site",
            &t.site.display().to_string(),
            "--list",
            &list.display().to_string(),
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--log-stamp",
            STAMP,
            "--dry-run",
        ],
        &[],
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(!t.run_dir(STAMP).join("summary.json").exists());

    // the same run without --dry-run writes one
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(t.run_dir(STAMP).join("summary.json").is_file());
}

#[test]
fn a_summary_that_cannot_be_written_leaves_the_exit_code() {
    let t = setup("");
    std::fs::create_dir_all(t.run_dir(STAMP).join("summary.json")).unwrap();
    let o = run(&t, "cpu", STAMP, LIST, &[]);
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(
        stderr(&o).contains("WARN [summary] cpu: no summary written: "),
        "{}",
        stderr(&o)
    );
}

fn target(name: &str, code: i32) -> TargetSummary {
    TargetSummary {
        name: name.into(),
        command: "run".into(),
        build_type: "production".into(),
        run_id: "r".into(),
        list: "/srv/list.txt".into(),
        log_dir: format!("/srv/logs/20260101_000000/{name}"),
        result: None,
        started_at: "2026-01-01T00:00:00+00:00".into(),
        ended_at: "2026-01-01T00:00:01+00:00".into(),
        duration_seconds: 1.0,
        exit_code: code,
        status: "failed".into(),
        message: None,
        interrupted_by: None,
        counts: BTreeMap::new(),
        phases: vec![],
        packages: vec![],
        deselected: vec![],
        watch: None,
    }
}

#[test]
fn summary_record_refuses_a_symbolic_link_output_path() {
    let dir = tempfile::tempdir().unwrap();
    let run_dir = dir.path().join("20260101_000000");
    std::fs::create_dir_all(&run_dir).unwrap();
    let dangling = dir.path().join("outside-summary.json");
    symlink(&dangling, run_dir.join(JSON)).unwrap();

    let error = record("t", target("cpu", 1), &run_dir).unwrap_err();
    assert!(error.contains("output path is a symbolic link"), "{error}");
    assert!(run_dir.join(JSON).is_symlink());
}
