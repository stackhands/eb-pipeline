//! `eb-shard-weights` builds an `eb-stack stack shard --weights` TSV from run durations.

use std::process::Command;

fn bin() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-shard-weights"));
    command.env_remove("EB_PIPELINE_RESULT");
    command.env_remove("EB_PIPELINE_TARGET");
    command
}

#[test]
fn successful_build_durations_become_sorted_tab_separated_weights() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        r#"{
  "kind": "result",
  "target": "zen4",
  "ended_at": "2026-11-17T09:00:00Z",
  "lines": [
    {"easyconfig": "GROMACS-2024.1-foss-2023b.eb", "phase": "build", "status": "success", "duration_seconds": 124.2},
    {"easyconfig": "pigz-2.8-GCCcore-15.2.0.eb", "phase": "build", "status": "success", "duration_seconds": 0.2},
    {"easyconfig": "Broken-1.0-foss-2023b.eb", "phase": "build", "status": "failed", "duration_seconds": 3.0},
    {"easyconfig": "Skip-1.0-foss-2023b.eb", "phase": "build", "status": "skipped"},
    {"easyconfig": "FetchOnly-1.0-foss-2023b.eb", "phase": "sources", "status": "failed", "duration_seconds": 9.0}
  ]
}
"#,
    )
    .unwrap();
    let o = bin().args(["--result"]).arg(&result).output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "GROMACS/2024.1-foss-2023b\t125\npigz/2.8-GCCcore-15.2.0\t1\n"
    );
}

#[test]
fn common_environment_falls_back_and_flags_win() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        r#"{"kind":"result","target":"cpu","lines":[{"easyconfig":"A-1.0.eb","phase":"build","status":"success","duration_seconds":2.0}]}"#,
    )
    .unwrap();
    let from_env = bin()
        .env("EB_PIPELINE_RESULT", &result)
        .env("EB_PIPELINE_TARGET", "cpu")
        .output()
        .unwrap();
    let from_flags = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--target", "cpu"])
        .env("EB_PIPELINE_RESULT", dir.path().join("wrong.json"))
        .env("EB_PIPELINE_TARGET", "wrong")
        .output()
        .unwrap();
    assert_eq!(from_env.status.code(), Some(0));
    assert_eq!(from_flags.status.code(), Some(0));
    assert_eq!(from_env.stdout, b"A/1.0\t2\n");
    assert_eq!(from_flags.stdout, from_env.stdout);
    assert!(from_env.stderr.is_empty());
    assert!(from_flags.stderr.is_empty());
}

#[test]
fn minimum_samples_is_opt_in_and_default_bytes_stay_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    let existing = dir.path().join("existing.tsv");
    std::fs::write(
        &result,
        r#"{
  "kind": "result",
  "target": "cpu",
  "ended_at": "2026-11-18T07:00:00Z",
  "lines": [
    {"easyconfig": "A-1.0.eb", "phase": "build", "status": "success", "duration_seconds": 10.0},
    {"easyconfig": "B-1.0.eb", "phase": "build", "status": "success", "duration_seconds": 12.0},
    {"easyconfig": "B-1.0.eb", "phase": "build", "status": "success", "duration_seconds": 20.0}
  ]
}"#,
    )
    .unwrap();
    std::fs::write(&existing, "A/1.0\t99\nKeep/1.0\t7\n").unwrap();

    let o = bin().args(["--result"]).arg(&result).output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(o.stdout, b"A/1.0\t10\nB/1.0\t20\n");
    assert_eq!(o.stderr, b"");

    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--existing"])
        .arg(&existing)
        .args(["--min-samples", "2"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(o.stdout, b"A/1.0\t99\nB/1.0\t20\nKeep/1.0\t7\n");
    assert_eq!(o.stderr, b"");
}

#[test]
fn duplicate_existing_modules_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    let existing = dir.path().join("existing.tsv");
    std::fs::write(&result, r#"{"kind":"result","target":"cpu","lines":[]}"#).unwrap();
    std::fs::write(&existing, "A/1.0\t5\nA/1.0\t50\n").unwrap();

    let output = bin()
        .arg("--result")
        .arg(&result)
        .arg("--existing")
        .arg(&existing)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(output.stdout, b"");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("line 2: module `A/1.0` was already set on line 1"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn output_aliases_are_refused_before_weight_inputs_are_changed() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    let result_bytes = r#"{
  "kind": "result",
  "target": "cpu",
  "lines": []
}"#;
    std::fs::write(&result, result_bytes).unwrap();
    let output = dir.path().join("weights.tsv");
    std::os::unix::fs::symlink(&result, &output).unwrap();

    let command = bin()
        .arg("--result")
        .arg(&result)
        .arg("--out")
        .arg(&output)
        .output()
        .unwrap();
    assert_eq!(command.status.code(), Some(2));
    assert_eq!(command.stdout, b"");
    assert!(
        String::from_utf8_lossy(&command.stderr)
            .contains("--result and --out must name different files"),
        "{}",
        String::from_utf8_lossy(&command.stderr)
    );
    assert_eq!(std::fs::read_to_string(&result).unwrap(), result_bytes);

    let existing = dir.path().join("existing.tsv");
    std::fs::write(&existing, "Keep/1.0\t4\n").unwrap();
    let existing_alias = dir.path().join("existing.out");
    std::fs::hard_link(&existing, &existing_alias).unwrap();
    let command = bin()
        .arg("--result")
        .arg(&result)
        .arg("--existing")
        .arg(&existing)
        .arg("--out")
        .arg(&existing_alias)
        .output()
        .unwrap();
    assert_eq!(command.status.code(), Some(2));
    assert_eq!(command.stdout, b"");
    assert!(
        String::from_utf8_lossy(&command.stderr)
            .contains("--existing and --out must name different files"),
        "{}",
        String::from_utf8_lossy(&command.stderr)
    );
    assert_eq!(std::fs::read_to_string(&existing).unwrap(), "Keep/1.0\t4\n");
}

#[test]
fn log_root_and_existing_weights_aggregate_with_mode_and_target_filter() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let run1 = logs.join("20261116_010000");
    let run2 = logs.join("20261117_010000");
    std::fs::create_dir_all(run1.join("zen4")).unwrap();
    std::fs::create_dir_all(run2.join("zen4")).unwrap();
    std::fs::create_dir_all(run2.join("zen2")).unwrap();
    // In-progress run without summary.json is skipped cleanly.
    std::fs::create_dir_all(logs.join("20261117_020000/zen4")).unwrap();

    std::fs::write(
        run1.join("summary.json"),
        r#"{
  "schema_version": 1,
  "kind": "summary",
  "tool": "eb-pipeline 0.3.0",
  "site": "example",
  "run": "20261116_010000",
  "updated_at": "2026-11-16T01:10:00Z",
  "exit_code": 0,
  "status": "success",
  "targets": [
    {
      "name": "zen4",
      "command": "run",
      "build_type": "production",
      "run_id": "r1",
      "list": "b.list",
      "log_dir": "/tmp/logs/20261116_010000/zen4",
      "started_at": "2026-11-16T01:00:00Z",
      "ended_at": "2026-11-16T01:10:00Z",
      "duration_seconds": 50.0,
      "exit_code": 0,
      "status": "success",
      "counts": {"success": 1},
      "phases": [],
      "packages": [
        {"easyconfig": "FFTW-3.3.10-GCC-13.2.0.eb", "module": "FFTW/3.3.10-GCC-13.2.0", "unit": "all", "phase": "build", "status": "success", "duration_seconds": 40.0, "log": "/tmp/a.log"}
      ]
    }
  ]
}
"#,
    )
    .unwrap();
    std::fs::write(
        run2.join("summary.json"),
        r#"{
  "schema_version": 1,
  "kind": "summary",
  "tool": "eb-pipeline 0.3.0",
  "site": "example",
  "run": "20261117_010000",
  "updated_at": "2026-11-17T01:10:00Z",
  "exit_code": 0,
  "status": "success",
  "targets": [
    {
      "name": "zen2",
      "command": "run",
      "build_type": "production",
      "run_id": "r2a",
      "list": "b.list",
      "log_dir": "/tmp/logs/20261117_010000/zen2",
      "started_at": "2026-11-17T01:00:00Z",
      "ended_at": "2026-11-17T01:05:00Z",
      "duration_seconds": 99.0,
      "exit_code": 0,
      "status": "success",
      "counts": {"success": 1},
      "phases": [],
      "packages": [
        {"easyconfig": "FFTW-3.3.10-GCC-13.2.0.eb", "module": "FFTW/3.3.10-GCC-13.2.0", "unit": "all", "phase": "build", "status": "success", "duration_seconds": 99.0, "log": "/tmp/b.log"}
      ]
    },
    {
      "name": "zen4",
      "command": "run",
      "build_type": "production",
      "run_id": "r2b",
      "list": "b.list",
      "log_dir": "/tmp/logs/20261117_010000/zen4",
      "started_at": "2026-11-17T01:00:00Z",
      "ended_at": "2026-11-17T01:10:00Z",
      "duration_seconds": 20.0,
      "exit_code": 0,
      "status": "success",
      "counts": {"success": 1},
      "phases": [],
      "packages": [
        {"easyconfig": "FFTW-3.3.10-GCC-13.2.0.eb", "module": "FFTW/3.3.10-GCC-13.2.0", "unit": "all", "phase": "build", "status": "success", "duration_seconds": 20.0, "log": "/tmp/c.log"}
      ]
    }
  ]
}
"#,
    )
    .unwrap();

    let existing = dir.path().join("existing.tsv");
    std::fs::write(
        &existing,
        "# seed\nFFTW/3.3.10-GCC-13.2.0\t10\nOldMod/1.0\t77\n",
    )
    .unwrap();
    let out = dir.path().join("weights.tsv");

    // `max` keeps 40 for zen4 (ignoring zen2's 99) and preserves OldMod/1.0.
    let o = bin()
        .args(["--log-root"])
        .arg(&logs)
        .args(["--target", "zen4", "--existing"])
        .arg(&existing)
        .args(["--mode", "max", "--out"])
        .arg(&out)
        .env("EB_PIPELINE_RESULT", dir.path().join("wrong.json"))
        .env("EB_PIPELINE_TARGET", "wrong")
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "FFTW/3.3.10-GCC-13.2.0\t40\nOldMod/1.0\t77\n"
    );

    // `latest` uses the newest run (20) for zen4 and preserves OldMod/1.0.
    let o = bin()
        .args(["--log-root"])
        .arg(&logs)
        .args(["--target", "zen4", "--existing"])
        .arg(&existing)
        .args(["--mode", "latest"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "FFTW/3.3.10-GCC-13.2.0\t20\nOldMod/1.0\t77\n"
    );

    // `mean` averages 40 and 20 -> 30.
    let o = bin()
        .args(["--log-root"])
        .arg(&logs)
        .args(["--target", "zen4", "--mode", "mean"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "FFTW/3.3.10-GCC-13.2.0\t30\n"
    );
}

#[test]
fn invalid_inputs_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let plan = dir.path().join("plan.json");
    let bad_tsv = dir.path().join("bad.tsv");
    std::fs::write(&plan, "{\"kind\":\"plan\"}\n").unwrap();
    std::fs::write(&bad_tsv, "OnlyOneField\n").unwrap();

    for args in [
        vec![],
        vec!["--result", plan.to_str().unwrap()],
        vec!["--result", plan.to_str().unwrap(), "--last", "5"],
        vec!["--log-root", dir.path().to_str().unwrap(), "--last", "0"],
        vec![
            "--log-root",
            dir.path().to_str().unwrap(),
            "--min-samples",
            "0",
        ],
        vec![
            "--log-root",
            dir.path().to_str().unwrap(),
            "--mode",
            "median",
        ],
        vec![
            "--log-root",
            dir.path().to_str().unwrap(),
            "--existing",
            bad_tsv.to_str().unwrap(),
        ],
    ] {
        let o = bin().args(&args).output().unwrap();
        assert_eq!(
            o.status.code(),
            Some(2),
            "args={args:?} stderr={}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(o.stdout.is_empty(), "args={args:?}");
    }
}

#[test]
fn a_corrupt_log_root_summary_prevents_partial_weights() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("logs");
    let run = logs.join("20261118_230000");
    let existing = dir.path().join("existing.tsv");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::write(run.join("summary.json"), "not json\n").unwrap();
    std::fs::write(&existing, "Keep/1.0\t7\n").unwrap();

    let output = bin()
        .args(["--log-root"])
        .arg(&logs)
        .args(["--existing"])
        .arg(&existing)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("summary.json"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn docs_name_eb_shard_weights() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode");
    for rel in ["howto/build-from-ci.org", "reference/cli.org"] {
        let page = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(page.contains("eb-shard-weights"), "{rel}");
        assert!(page.contains("--weights"), "{rel}");
        assert!(page.contains("--min-samples"), "{rel}");
    }
}

#[test]
fn duplicate_single_value_flags_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        r#"{"kind":"result","target":"cpu","lines":[{"easyconfig":"A-1.0.eb","phase":"build","status":"success","duration_seconds":1.0}]}"#,
    )
    .unwrap();
    let existing = dir.path().join("existing.tsv");
    std::fs::write(&existing, "A\t1\n").unwrap();

    for (flag, args) in [
        (
            "--out",
            vec![
                "--result".to_string(),
                result.display().to_string(),
                "--out".to_string(),
                dir.path().join("a.tsv").display().to_string(),
                "--out".to_string(),
                dir.path().join("b.tsv").display().to_string(),
            ],
        ),
        (
            "--target",
            vec![
                "--result".to_string(),
                result.display().to_string(),
                "--target".to_string(),
                "cpu".to_string(),
                "--target=gpu".to_string(),
            ],
        ),
        (
            "--mode",
            vec![
                "--result".to_string(),
                result.display().to_string(),
                "--mode".to_string(),
                "max".to_string(),
                "--mode".to_string(),
                "mean".to_string(),
            ],
        ),
        (
            "--min-samples",
            vec![
                "--result".to_string(),
                result.display().to_string(),
                "--min-samples".to_string(),
                "1".to_string(),
                "--min-samples=2".to_string(),
            ],
        ),
        (
            "--existing",
            vec![
                "--result".to_string(),
                result.display().to_string(),
                "--existing".to_string(),
                existing.display().to_string(),
                "--existing".to_string(),
                existing.display().to_string(),
            ],
        ),
        (
            "--log-root",
            vec![
                "--log-root".to_string(),
                dir.path().display().to_string(),
                "--log-root".to_string(),
                dir.path().display().to_string(),
            ],
        ),
        (
            "--last",
            vec![
                "--log-root".to_string(),
                dir.path().display().to_string(),
                "--last".to_string(),
                "1".to_string(),
                "--last=2".to_string(),
            ],
        ),
    ] {
        let o = bin().args(&args).output().unwrap();
        assert_eq!(
            o.status.code(),
            Some(2),
            "{flag}: stderr={}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("was given twice"),
            "{flag}: stderr={}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn shard_weights_succeeds_when_stdout_consumer_closes() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        r#"{"kind":"result","target":"cpu","lines":[{"easyconfig":"A-1.0.eb","phase":"build","status":"success","duration_seconds":1.0}]}"#,
    )
    .unwrap();
    let mut fds = [0; 2];
    unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        assert_eq!(libc::close(fds[0]), 0);
    }
    let closed = unsafe { <std::process::Stdio as std::os::fd::FromRawFd>::from_raw_fd(fds[1]) };
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .stdout(closed)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn a_target_filter_must_match_an_input_target() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        r#"{"kind":"result","target":"cpu","lines":[{"easyconfig":"A-1.0.eb","phase":"build","status":"success","duration_seconds":2.0}]}"#,
    )
    .unwrap();

    let mismatch = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--target", "gpu"])
        .output()
        .unwrap();
    assert_eq!(mismatch.status.code(), Some(2));
    assert_eq!(mismatch.stdout, b"");
    assert!(
        String::from_utf8_lossy(&mismatch.stderr).contains("no input data matched target `gpu`"),
        "{}",
        String::from_utf8_lossy(&mismatch.stderr)
    );

    let matched = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--target", "cpu"])
        .output()
        .unwrap();
    assert_eq!(matched.status.code(), Some(0));
    assert_eq!(matched.stdout, b"A/1.0\t2\n");
    assert_eq!(matched.stderr, b"");
}

#[test]
fn a_successful_result_line_requires_a_numeric_duration() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        r#"{"kind":"result","target":"cpu","lines":[{"easyconfig":"A-1.0.eb","phase":"build","status":"success","duration_seconds":"2.0"}]}"#,
    )
    .unwrap();

    let malformed = bin().args(["--result"]).arg(&result).output().unwrap();
    assert_eq!(malformed.status.code(), Some(2));
    assert_eq!(malformed.stdout, b"");
    assert!(
        String::from_utf8_lossy(&malformed.stderr)
            .contains("lines[0].duration_seconds must be a non-negative number"),
        "{}",
        String::from_utf8_lossy(&malformed.stderr)
    );

    std::fs::write(
        &result,
        r#"{"kind":"result","target":"cpu","lines":[{"easyconfig":"A-1.0.eb","phase":"build","status":"success","duration_seconds":2.0}]}"#,
    )
    .unwrap();
    let valid = bin().args(["--result"]).arg(&result).output().unwrap();
    assert_eq!(valid.status.code(), Some(0));
    assert_eq!(valid.stdout, b"A/1.0\t2\n");
    assert_eq!(valid.stderr, b"");
}

#[test]
fn a_successful_summary_package_requires_a_duration() {
    let dir = tempfile::tempdir().unwrap();
    let summary = dir.path().join("summary.json");
    std::fs::write(
        &summary,
        r#"{
  "schema_version": 1,
  "kind": "summary",
  "tool": "eb-pipeline 0.3.1",
  "site": "example",
  "run": "20261118_230000",
  "updated_at": "2026-11-18T23:10:00Z",
  "exit_code": 0,
  "status": "success",
  "targets": [{
    "name": "cpu",
    "command": "run",
    "build_type": "production",
    "run_id": "r1",
    "list": "build.list",
    "log_dir": "/tmp/logs/cpu",
    "started_at": "2026-11-18T23:00:00Z",
    "ended_at": "2026-11-18T23:10:00Z",
    "duration_seconds": 600.0,
    "exit_code": 0,
    "status": "success",
    "counts": {"success": 2},
    "phases": [],
    "packages": [
      {"easyconfig": "A-1.0.eb", "unit": "all", "phase": "build", "status": "success", "log": "/tmp/a.log"},
      {"easyconfig": "B-1.0.eb", "unit": "all", "phase": "build", "status": "success", "duration_seconds": 3.0, "log": "/tmp/b.log"}
    ]
  }]
}"#,
    )
    .unwrap();

    let output = bin().args(["--summary"]).arg(&summary).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("targets[0].packages[0].duration_seconds must be a non-negative number"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn value_options_do_not_consume_the_next_long_option() {
    let output = bin()
        .args(["--result", "--target", "cpu"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(output.stdout, b"");
    assert_eq!(output.stderr, b"eb-shard-weights: --result needs a value\n");
}
