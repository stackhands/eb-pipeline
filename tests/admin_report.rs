//! Integration tests for `eb-admin-report`.

mod common;

use common::{output_within, write_exec};
use std::process::{Command, Stdio};
use std::time::Duration;

const LIMIT: Duration = Duration::from_secs(30);

fn bin() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-admin-report"));
    command
        .env_remove("EB_PIPELINE_RESULT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn classifier_stub(bin_dir: &std::path::Path) {
    write_exec(
        &bin_dir.join("eb-stack"),
        r#"#!/bin/sh
log="$4"
if grep -q "command not found" "$log"; then
  printf '{"jobs":[{"class":"infra","sub_class":"source"}]}\n'
elif grep -q "Checksum verification failed" "$log"; then
  printf '{"jobs":[{"class":"real-failure","sub_class":"checksum"}]}\n'
elif grep -q "Couldn't find file" "$log"; then
  printf '{"jobs":[{"class":"real-failure","sub_class":"source"}]}\n'
elif grep -q "Sanity check FAILED" "$log"; then
  printf '{"jobs":[{"class":"real-failure","sub_class":"sanity"}]}\n'
elif grep -q "Killed process" "$log"; then
  printf '{"jobs":[{"class":"real-failure","sub_class":"install"}]}\n'
else
  printf '{"jobs":[{"class":"real-failure","sub_class":"unknown"}]}\n'
fi
"#,
    );
}

fn write_real_failure_fixture(dir: &std::path::Path) -> std::path::PathBuf {
    let logs = [
        (
            "missing-source.log",
            "== ERROR: Couldn't find file zlib-9.9.9.tar.gz anywhere, and downloading it didn't work either.\n",
            "missing-source.eb",
        ),
        (
            "bad-checksum.log",
            "== ERROR: Checksum verification failed for source file zlib-1.3.1.tar.gz\n",
            "bad-checksum.eb",
        ),
        (
            "bad-sanity.log",
            "== FAILED: Sanity check FAILED for easyblock 'ConfigureMake': no files in /tmp/install/lib\n",
            "bad-sanity.eb",
        ),
        (
            "oom-kill.log",
            "kernel: Killed process 12345 (python3) total-vm:999999kB\n",
            "oom-kill.eb",
        ),
    ];
    for (name, text, _) in logs {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let result = dir.join("result.json");
    std::fs::write(
        &result,
        format!(
            r#"{{
  "kind": "result",
  "site": "eessi",
  "target": "zen4",
  "build_type": "production",
  "run_id": "run-20261119",
  "exit_code": 1,
  "started_at": "2026-11-19T00:00:00Z",
  "ended_at": "2026-11-19T00:10:00Z",
  "counts": {{"failed": 4}},
  "lines": [
    {{"easyconfig":"missing-source.eb","phase":"sources","unit":"all","status":"failed","exit_code":1,"log":"{}"}},
    {{"easyconfig":"bad-checksum.eb","phase":"sources","unit":"all","status":"failed","exit_code":1,"log":"{}"}},
    {{"easyconfig":"bad-sanity.eb","phase":"build","unit":"all","status":"failed","exit_code":1,"log":"{}"}},
    {{"easyconfig":"oom-kill.eb","phase":"build","unit":"all","status":"failed","exit_code":1,"log":"{}"}}
  ]
}}"#,
            dir.join("missing-source.log").display(),
            dir.join("bad-checksum.log").display(),
            dir.join("bad-sanity.log").display(),
            dir.join("oom-kill.log").display()
        ),
    )
    .unwrap();
    result
}

#[test]
fn report_uses_shared_classifier_signatures_for_real_failure_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    classifier_stub(&bin_dir);
    let result = write_real_failure_fixture(dir.path());
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = output_within(
        bin().args(["--result"]).arg(&result).env("PATH", path),
        LIMIT,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(
        report.contains("Failure signatures (eb-stack bot-classify)"),
        "{report}"
    );
    assert!(report.contains("| real-failure/source | 1 |"), "{report}");
    assert!(report.contains("| real-failure/checksum | 1 |"), "{report}");
    assert!(report.contains("| real-failure/sanity | 1 |"), "{report}");
    assert!(report.contains("| real-failure/install | 1 |"), "{report}");
    assert!(
        report.contains("Systemic infrastructure failure: `no`"),
        "{report}"
    );
}

#[test]
fn report_marks_shared_command_path_failures_as_systemic_infrastructure() {
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    classifier_stub(&bin_dir);
    let log = dir.path().join("missing-eb.log");
    std::fs::write(&log, "bash: eb: command not found\n").unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        format!(
            r#"{{
  "kind": "result",
  "site": "eessi",
  "target": "zen4",
  "build_type": "production",
  "run_id": "run",
  "exit_code": 1,
  "counts": {{"failed": 2}},
  "lines": [
    {{"easyconfig":"A.eb","phase":"build","unit":"all","status":"failed","exit_code":127,"log":"{}"}},
    {{"easyconfig":"B.eb","phase":"build","unit":"all","status":"failed","exit_code":127,"log":"{}"}}
  ]
}}"#,
            log.display(),
            log.display()
        ),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let output = output_within(
        bin().args(["--result"]).arg(&result).env("PATH", path),
        LIMIT,
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    assert!(
        report.contains("Systemic infrastructure failure: `yes`"),
        "{report}"
    );
    assert!(
        report.contains("Infrastructure class: `command-path`"),
        "{report}"
    );
}

/// A result file holding `lines`, a JSON array, at `path`.
fn write_result(path: &std::path::Path, lines: &str) {
    std::fs::write(
        path,
        format!(
            r#"{{"kind":"result","site":"eessi","target":"zen4","build_type":"production","run_id":"run","exit_code":1,"lines":{lines}}}"#
        ),
    )
    .unwrap();
}

#[test]
fn a_result_path_that_is_not_utf8_is_read() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let result = dir
        .path()
        .join(std::ffi::OsStr::from_bytes(b"res\xffult.json"));
    write_result(
        &result,
        r#"[{"easyconfig":"A.eb","phase":"build","unit":"all","status":"success"}]"#,
    );
    let mut joined = std::ffi::OsString::from("--result=");
    joined.push(&result);
    for output in [
        output_within(bin().arg("--result").arg(&result), LIMIT),
        output_within(bin().arg(&joined), LIMIT),
    ] {
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let report = String::from_utf8_lossy(&output.stdout);
        assert!(report.contains("All lines succeeded."), "{report}");
    }
}

#[test]
fn an_argument_that_is_not_utf8_is_refused_without_a_panic() {
    use std::os::unix::ffi::OsStrExt;
    let output = output_within(bin().arg(std::ffi::OsStr::from_bytes(b"--b\xffd")), LIMIT);
    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{err}");
    assert!(
        err.contains("eb-admin-report: argument is not valid UTF-8: `--b\u{fffd}d`"),
        "{err}"
    );
    assert!(!err.contains("panicked"), "{err}");
}

#[test]
fn a_line_log_that_is_a_fifo_is_noted_and_not_waited_on() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("line.log");
    common::make_fifo(&log);
    let result = dir.path().join("result.json");
    write_result(
        &result,
        &format!(
            r#"[{{"easyconfig":"A.eb","phase":"build","unit":"all","status":"failed","exit_code":1,"log":"{}"}}]"#,
            log.display()
        ),
    );
    let output = output_within(bin().arg("--result").arg(&result), Duration::from_secs(10));
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(
        report.contains(&format!(
            "- A.eb: cannot read `{}`: not a regular file",
            log.display()
        )),
        "{report}"
    );
    assert!(!report.contains("log tail"), "{report}");
}
