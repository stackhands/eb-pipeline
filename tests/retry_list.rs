//! `eb-retry-list` turns a run result or summary and its build list into the lines to retry.

use std::process::Command;
use std::{os::unix::fs::PermissionsExt, path::Path};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-retry-list"))
}

fn write_exec(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn explicit_result_list_keeps_default_stdout_stderr_and_status_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    std::fs::write(
        &list,
        "\
# nightly list
Foo-1.0-foss-2025a.eb --robot
GpuOnly-1.0-foss-2025a.eb --only-targets=gpu
Bar-1.0-foss-2025a.eb --robot
Baz-1.0-foss-2025a.eb --rebuild
Qux-1.0-foss-2025a.eb
Tail-1.0-foss-2025a.eb --from-pr=123
",
    )
    .unwrap();
    std::fs::write(
        &result,
        r#"{
  "kind": "result",
  "exit_code": 1,
  "deselected": [
    {"easyconfig": "GpuOnly-1.0-foss-2025a.eb", "unit": "all"}
  ],
  "lines": [
    {"easyconfig": "Foo-1.0-foss-2025a.eb", "phase": "build", "status": "success"},
    {"easyconfig": "Bar-1.0-foss-2025a.eb", "phase": "sources", "status": "failed"},
    {"easyconfig": "Baz-1.0-foss-2025a.eb", "phase": "build", "status": "refused"},
    {"easyconfig": "Qux-1.0-foss-2025a.eb", "phase": "build", "status": "pending"},
    {"easyconfig": "Tail-1.0-foss-2025a.eb", "phase": "build", "status": "skipped"}
  ]
}
"#,
    )
    .unwrap();
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .output()
        .unwrap();
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
        "\
Bar-1.0-foss-2025a.eb --robot
Baz-1.0-foss-2025a.eb --rebuild
Qux-1.0-foss-2025a.eb
Tail-1.0-foss-2025a.eb --from-pr=123
"
    );
}

#[test]
fn explicit_summary_list_keeps_default_output_file_and_stream_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let stack = dir.path().join("stack.yml");
    let summary = dir.path().join("summary.json");
    let out = dir.path().join("retry.list");
    std::fs::write(
        &stack,
        "\
easyconfigs:
  - FFTW-3.3.10-GCC-13.2.0.eb
  - GROMACS-2024.1-foss-2023b.eb:
      options:
        rebuild: true
",
    )
    .unwrap();
    std::fs::write(
        &summary,
        r#"{
  "schema_version": 1,
  "kind": "summary",
  "exit_code": 1,
  "targets": [
    {
      "name": "zen2",
      "exit_code": 1,
      "packages": [
        {"easyconfig": "FFTW-3.3.10-GCC-13.2.0.eb", "phase": "build", "status": "success"},
        {"easyconfig": "GROMACS-2024.1-foss-2023b.eb", "phase": "build", "status": "failed"}
      ]
    },
    {
      "name": "zen4",
      "exit_code": 0,
      "packages": [
        {"easyconfig": "FFTW-3.3.10-GCC-13.2.0.eb", "phase": "build", "status": "success"},
        {"easyconfig": "GROMACS-2024.1-foss-2023b.eb", "phase": "build", "status": "success"}
      ]
    }
  ]
}
"#,
    )
    .unwrap();
    let o = bin()
        .args(["--summary"])
        .arg(&summary)
        .args(["--target", "zen2", "--list"])
        .arg(&stack)
        .args(["--status", "failed", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(o.stderr, b"");
    assert!(o.stdout.is_empty());
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "GROMACS-2024.1-foss-2023b.eb --rebuild\n"
    );

    let o = bin()
        .args(["--summary"])
        .arg(&summary)
        .args(["--target", "zen4", "--list"])
        .arg(&stack)
        .args(["--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(o.stdout, b"");
    assert_eq!(o.stderr, b"");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "");
}

#[test]
fn recorded_list_reads_the_path_from_a_result_or_summary_target() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build list.txt");
    let result = dir.path().join("result.json");
    let summary = dir.path().join("summary.json");
    std::fs::write(&list, "Done.eb --robot\nRetry.eb --rebuild\n").unwrap();
    let list_path = list.display().to_string();
    let packages = serde_json::json!([
        {"easyconfig": "Done.eb", "status": "success"},
        {"easyconfig": "Retry.eb", "status": "failed"}
    ]);
    std::fs::write(
        &result,
        serde_json::to_vec(&serde_json::json!({
            "kind": "result",
            "exit_code": 1,
            "list": &list_path,
            "lines": &packages
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        &summary,
        serde_json::to_vec(&serde_json::json!({
            "kind": "summary",
            "targets": [{
                "name": "cpu",
                "exit_code": 1,
                "list": &list_path,
                "packages": &packages
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    for args in [
        vec!["--result", result.to_str().unwrap(), "--recorded-list"],
        vec![
            "--summary",
            summary.to_str().unwrap(),
            "--target",
            "cpu",
            "--recorded-list",
        ],
    ] {
        let o = bin().args(args).output().unwrap();
        assert_eq!(o.status.code(), Some(0));
        assert_eq!(o.stdout, b"Retry.eb --rebuild\n");
        assert_eq!(o.stderr, b"");
    }
}

#[test]
fn explicit_list_defaults_keep_their_error_bytes() {
    let o = bin().args(["--result", "result.json"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(o.stdout, b"");
    assert_eq!(o.stderr, b"eb-retry-list: --list is required\n");
}

#[test]
fn recorded_list_refuses_an_explicit_list_or_a_record_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    std::fs::write(&list, "Retry.eb\n").unwrap();
    std::fs::write(
        &result,
        r#"{"kind":"result","exit_code":1,"lines":[{"easyconfig":"Retry.eb","status":"failed"}]}"#,
    )
    .unwrap();

    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .arg("--recorded-list")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(
        o.stderr,
        b"eb-retry-list: pass either --list or --recorded-list, not both\n"
    );

    let o = bin()
        .args(["--result"])
        .arg(&result)
        .arg("--recorded-list")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert_eq!(o.stdout, b"");
    assert_eq!(
        o.stderr,
        b"eb-retry-list: the selected record has no string `list`\n"
    );
}

#[test]
fn sharded_order_duplicate_lines_and_early_abort_preserve_list_order() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    std::fs::write(
        &list,
        "\
Dup-1.0.eb
A-1.0.eb --robot
B-1.0.eb --robot
Dup-1.0.eb --rebuild
SkipMe-1.0.eb --only-targets=other
",
    )
    .unwrap();
    std::fs::write(
        &result,
        r#"{
  "kind": "result",
  "exit_code": 1,
  "deselected": [{"easyconfig": "SkipMe-1.0.eb", "unit": "all"}],
  "lines": [
    {"easyconfig": "zlib-1.3.1.eb", "unit": "prelude", "phase": "build", "status": "success"},
    {"easyconfig": "B-1.0.eb", "unit": "shard-01", "phase": "build", "status": "failed"},
    {"easyconfig": "A-1.0.eb", "unit": "shard-02", "phase": "build", "status": "success"},
    {"easyconfig": "Dup-1.0.eb", "unit": "shard-01", "phase": "build", "status": "success"},
    {"easyconfig": "Dup-1.0.eb", "unit": "shard-02", "phase": "build", "status": "failed"}
  ]
}
"#,
    )
    .unwrap();
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "B-1.0.eb --robot\nDup-1.0.eb --rebuild\n"
    );

    // An early abort before any line reached `build` still retries every non-deselected line.
    std::fs::write(
        &result,
        r#"{
  "kind": "result",
  "exit_code": 1,
  "deselected": [{"easyconfig": "SkipMe-1.0.eb", "unit": "all"}],
  "lines": []
}
"#,
    )
    .unwrap();
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "Dup-1.0.eb\nA-1.0.eb --robot\nB-1.0.eb --robot\nDup-1.0.eb --rebuild\n"
    );
}

#[test]
fn invalid_inputs_are_refused_with_exit_2() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let plan = dir.path().join("plan.json");
    let summary = dir.path().join("summary.json");
    std::fs::write(&list, "Foo-1.0.eb\n").unwrap();
    std::fs::write(&plan, "{\"kind\":\"plan\"}\n").unwrap();
    std::fs::write(&summary, "{\"kind\":\"summary\",\"targets\":[]}\n").unwrap();

    for args in [
        vec!["--list", list.to_str().unwrap()],
        vec![
            "--result",
            plan.to_str().unwrap(),
            "--list",
            list.to_str().unwrap(),
        ],
        vec![
            "--summary",
            summary.to_str().unwrap(),
            "--list",
            list.to_str().unwrap(),
        ],
        vec![
            "--summary",
            summary.to_str().unwrap(),
            "--target",
            "missing",
            "--list",
            list.to_str().unwrap(),
        ],
        vec![
            "--result",
            plan.to_str().unwrap(),
            "--summary",
            summary.to_str().unwrap(),
            "--list",
            list.to_str().unwrap(),
        ],
        vec![
            "--result",
            plan.to_str().unwrap(),
            "--target",
            "cpu",
            "--list",
            list.to_str().unwrap(),
        ],
        vec![
            "--result",
            plan.to_str().unwrap(),
            "--list",
            list.to_str().unwrap(),
            "--status",
            "bogus",
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
fn classify_keeps_only_retryable_eb_stack_sub_classes() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    let calls = dir.path().join("calls");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    std::fs::write(
        &list,
        "Interrupted.eb\nResource.eb\nTimeout.eb\nCompile.eb\n",
    )
    .unwrap();
    for name in ["interrupted", "resource", "timeout", "compile"] {
        std::fs::write(dir.path().join(format!("{name}.log")), name).unwrap();
    }
    std::fs::write(
        &result,
        format!(
            r#"{{
  "kind": "result",
  "exit_code": 1,
  "lines": [
    {{"easyconfig":"Interrupted.eb","status":"failed","log":"{}"}},
    {{"easyconfig":"Resource.eb","status":"failed","log":"{}"}},
    {{"easyconfig":"Timeout.eb","status":"failed","log":"{}"}},
    {{"easyconfig":"Compile.eb","status":"failed","log":"{}"}}
  ]
}}"#,
            dir.path().join("interrupted.log").display(),
            dir.path().join("resource.log").display(),
            dir.path().join("timeout.log").display(),
            dir.path().join("compile.log").display(),
        ),
    )
    .unwrap();
    write_exec(
        &bin_dir.join("eb-stack"),
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$CALLS"
class=$(cat "$4")
printf '{"jobs":[{"sub_class":"%s"}]}\n' "$class"
"#,
    );
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .arg("--classify")
        .env("PATH", path)
        .env("CALLS", &calls)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "Interrupted.eb\nResource.eb\nTimeout.eb\n"
    );
    let calls = std::fs::read_to_string(calls).unwrap();
    assert_eq!(calls.lines().count(), 4);
    assert!(calls
        .lines()
        .all(|line| line.starts_with("stack bot-classify --json ")));
}

#[test]
fn classify_reports_missing_logs_missing_eb_stack_and_eb_stack_failures() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    let log = dir.path().join("failed.log");
    std::fs::write(&list, "Failed.eb\n").unwrap();
    std::fs::write(&log, "failure").unwrap();

    std::fs::write(
        &result,
        r#"{"kind":"result","exit_code":1,"lines":[{"easyconfig":"Failed.eb","status":"failed"}]}"#,
    )
    .unwrap();
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .arg("--classify")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("no log was recorded"));
    assert!(o.stdout.is_empty());

    std::fs::write(
        &result,
        format!(
            r#"{{"kind":"result","exit_code":1,"lines":[{{"easyconfig":"Failed.eb","status":"failed","log":"{}"}}]}}"#,
            log.display()
        ),
    )
    .unwrap();
    let empty_path = dir.path().join("empty-bin");
    std::fs::create_dir(&empty_path).unwrap();
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .arg("--classify")
        .env("PATH", &empty_path)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("cannot run eb-stack"));
    assert!(o.stdout.is_empty());

    write_exec(
        &empty_path.join("eb-stack"),
        "#!/bin/sh\necho 'classifier unavailable' >&2\nexit 7\n",
    );
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .arg("--classify")
        .env("PATH", &empty_path)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("eb-stack failed"), "{stderr}");
    assert!(stderr.contains("classifier unavailable"), "{stderr}");
    assert!(o.stdout.is_empty());
}

#[test]
fn classify_refuses_malformed_eb_stack_output() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    let log = dir.path().join("failed.log");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    std::fs::write(&list, "Failed.eb\n").unwrap();
    std::fs::write(&log, "failure").unwrap();
    std::fs::write(
        &result,
        format!(
            r#"{{"kind":"result","exit_code":1,"lines":[{{"easyconfig":"Failed.eb","status":"failed","log":"{}"}}]}}"#,
            log.display()
        ),
    )
    .unwrap();
    for (script, message) in [
        ("#!/bin/sh\nprintf 'not JSON\\n'\n", "invalid JSON"),
        ("#!/bin/sh\nprintf '{}\\n'\n", "no `jobs` array"),
        (
            "#!/bin/sh\nprintf '{\"jobs\":[]}\\n'\n",
            "classified 0 jobs",
        ),
    ] {
        write_exec(&bin_dir.join("eb-stack"), script);
        let o = bin()
            .args(["--result"])
            .arg(&result)
            .args(["--list"])
            .arg(&list)
            .arg("--classify")
            .env("PATH", &bin_dir)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{message}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(o.stdout.is_empty());
    }
}

#[test]
fn docs_name_eb_retry_list() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode");
    for rel in ["howto/follow-runs.org", "reference/cli.org"] {
        let page = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(page.contains("eb-retry-list"), "{rel}");
        assert!(page.contains("--result"), "{rel}");
        assert!(page.contains("--summary"), "{rel}");
        assert!(page.contains("--list"), "{rel}");
        assert!(page.contains("--recorded-list"), "{rel}");
        assert!(page.contains("--classify"), "{rel}");
        assert!(page.contains("sub_class"), "{rel}");
        assert!(page.contains("--node"), "{rel}");
        assert!(page.contains("--drop-oom"), "{rel}");
    }
    let from_eb_stack = std::fs::read_to_string(root.join("howto/from-eb-stack.org")).unwrap();
    assert!(from_eb_stack.contains("stack bot-classify"));
    assert!(from_eb_stack.contains("stack oom"));
}

/// `eb-stack` stub that records every argv and answers both subcommands.
fn stub_both(path: &Path) {
    write_exec(
        path,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$CALLS"
case "$2" in
  bot-classify)
    class=$(cat "$4")
    printf '{"jobs":[{"class":"real-failure","sub_class":"%s"}]}\n' "$class"
    ;;
  oom)
    easystack=
    prev=
    for arg in "$@"; do
      if [ "$prev" = --easystack ]; then
        easystack=$arg
      fi
      prev=$arg
    done
    printf 'EASYSTACK\n' >> "$CALLS"
    cat "$easystack" >> "$CALLS"
    if grep -q 'Big.eb' "$easystack"; then
      printf '%s\n' 'Big.eb may run out of memory on x86_64/amd/zen4 (label): 10.0 GiB, the job has 1.0 GiB'
      printf '%s\n' 'oom=1 fits=0 builds=1 nodes=1'
    elif grep -q 'Parent.eb' "$easystack"; then
      printf '%s\n' 'Dep.eb may run out of memory on x86_64/amd/zen4 (label): 9.0 GiB, the job has 1.0 GiB (required by Parent.eb)'
      printf '%s\n' 'oom=1 fits=0 builds=1 nodes=1'
    else
      printf '%s\n' 'oom=0 fits=1 builds=1 nodes=1'
    fi
    ;;
  *)
    echo "unexpected $*" >&2
    exit 9
    ;;
esac
"#,
    );
}

fn class_fixture(dir: &Path, entries: &[(&str, &str)]) -> (std::path::PathBuf, std::path::PathBuf) {
    let list = dir.join("build.list");
    let result = dir.join("result.json");
    let mut list_text = String::new();
    let mut lines = String::from("[\n");
    for (i, (name, class)) in entries.iter().enumerate() {
        list_text.push_str(name);
        list_text.push_str(" --robot\n");
        let log = dir.join(format!("{class}.log"));
        std::fs::write(&log, class).unwrap();
        if i > 0 {
            lines.push_str(",\n");
        }
        lines.push_str(&format!(
            r#"  {{"easyconfig":"{name}","status":"failed","log":"{}"}}"#,
            log.display()
        ));
    }
    lines.push_str("\n]\n");
    std::fs::write(&list, list_text).unwrap();
    std::fs::write(
        &result,
        format!(
            r#"{{"kind":"result","exit_code":1,"lines":{lines}}}"#,
            lines = lines
        ),
    )
    .unwrap();
    (list, result)
}

#[test]
fn class_keeps_only_the_named_eb_stack_classes() {
    let dir = tempfile::tempdir().unwrap();
    let (list, result) = class_fixture(
        dir.path(),
        &[
            ("Fetch.eb", "source"),
            ("Interrupted.eb", "interrupted"),
            ("Compile.eb", "compile"),
        ],
    );
    let calls = dir.path().join("calls");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    stub_both(&bin_dir.join("eb-stack"));
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .args(["--classify", "--class", "source,checksum"])
        .env("PATH", &path)
        .env("CALLS", &calls)
        .output()
        .unwrap();
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
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "Fetch.eb --robot\n");
    let calls = std::fs::read_to_string(&calls).unwrap();
    assert_eq!(calls.lines().count(), 3);
    assert!(calls
        .lines()
        .all(|line| line.starts_with("stack bot-classify --json ")));
}

#[test]
fn class_matches_the_top_level_class_as_well_as_the_sub_class() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    let log = dir.path().join("infra.log");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    std::fs::write(&list, "Infra.eb\n").unwrap();
    std::fs::write(&log, "infra").unwrap();
    std::fs::write(
        &result,
        format!(
            r#"{{"kind":"result","exit_code":1,"lines":[{{"easyconfig":"Infra.eb","status":"failed","log":"{}"}}]}}"#,
            log.display()
        ),
    )
    .unwrap();
    write_exec(
        &bin_dir.join("eb-stack"),
        r#"#!/bin/sh
printf '{"jobs":[{"class":"infra","sub_class":"scheduler"}]}\n'
"#,
    );
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .args(["--classify", "--class", "infra"])
        .env("PATH", &bin_dir)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "Infra.eb\n");
}

#[test]
fn node_warns_without_dropping_unless_drop_oom_is_set() {
    let dir = tempfile::tempdir().unwrap();
    let (list, result) = class_fixture(
        dir.path(),
        &[("Big.eb", "timeout"), ("Small.eb", "timeout")],
    );
    let calls = dir.path().join("calls");
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    stub_both(&bin_dir.join("eb-stack"));
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let easyconfigs = dir.path().join("easyconfigs");
    std::fs::create_dir(&easyconfigs).unwrap();
    let run = |extra: &[&str]| {
        std::fs::write(&calls, "").unwrap();
        bin()
            .args(["--result"])
            .arg(&result)
            .args(["--list"])
            .arg(&list)
            .args(["--classify", "--node", "8:1024:x86_64/amd/zen4"])
            .args(extra)
            .env("PATH", &path)
            .env("CALLS", &calls)
            .output()
            .unwrap()
    };
    let o = run(&[
        "--easyconfigs",
        easyconfigs.to_str().unwrap(),
        "--arch",
        "x86_64/amd/zen4",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        String::from_utf8(o.stdout.clone()).unwrap(),
        "Big.eb --robot\nSmall.eb --robot\n"
    );
    let stderr = String::from_utf8(o.stderr).unwrap();
    assert!(
        stderr.contains("Big.eb may run out of memory on x86_64/amd/zen4"),
        "{stderr}"
    );
    assert!(!stderr.contains("Small.eb"), "{stderr}");
    let calls_text = std::fs::read_to_string(&calls).unwrap();
    let oom = calls_text
        .lines()
        .find(|line| line.starts_with("stack oom "))
        .expect("oom call");
    let easystack = oom
        .split_whitespace()
        .skip_while(|word| *word != "--easystack")
        .nth(1)
        .expect("easystack path");
    assert_eq!(
        oom,
        format!(
            "stack oom --node 8:1024:x86_64/amd/zen4 --easystack {easystack} --easyconfigs {} --arch x86_64/amd/zen4",
            easyconfigs.display()
        )
    );
    assert!(
        calls_text.contains("Big.eb") && calls_text.contains("Small.eb"),
        "{calls_text}"
    );
    assert!(
        !std::path::Path::new(easystack).exists(),
        "temporary easystack was left behind"
    );

    let o = run(&["--drop-oom"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "Small.eb --robot\n");
    assert!(String::from_utf8_lossy(&o.stderr).contains("may run out of memory"));
}

#[test]
fn drop_oom_removes_a_line_that_requires_a_build_eb_stack_warns_about() {
    let dir = tempfile::tempdir().unwrap();
    let (list, result) = class_fixture(dir.path(), &[("Parent.eb", "resource")]);
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    stub_both(&bin_dir.join("eb-stack"));
    let path = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let o = bin()
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .args(["--classify", "--node", "4:2048", "--drop-oom"])
        .env("PATH", &path)
        .env("CALLS", dir.path().join("calls"))
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(String::from_utf8(o.stdout).unwrap(), "");
    assert!(String::from_utf8_lossy(&o.stderr).contains("required by Parent.eb"));
}

#[test]
fn node_options_without_classify_or_node_are_usage_errors() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    std::fs::write(&list, "Foo.eb\n").unwrap();
    std::fs::write(
        &result,
        r#"{"kind":"result","exit_code":1,"lines":[{"easyconfig":"Foo.eb","status":"failed"}]}"#,
    )
    .unwrap();
    let list = list.to_str().unwrap();
    let result = result.to_str().unwrap();
    let cases = [
        (
            vec!["--result", result, "--list", list, "--class", "timeout"],
            "--class requires --classify",
        ),
        (
            vec!["--result", result, "--list", list, "--node", "8:1024"],
            "--node requires --classify",
        ),
        (
            vec!["--result", result, "--list", list, "--drop-oom"],
            "--drop-oom requires --classify",
        ),
        (
            vec![
                "--result",
                result,
                "--list",
                list,
                "--classify",
                "--drop-oom",
            ],
            "--drop-oom requires --node",
        ),
        (
            vec![
                "--result",
                result,
                "--list",
                list,
                "--classify",
                "--easyconfigs",
                "/tmp/ecs",
            ],
            "--easyconfigs requires --node",
        ),
        (
            vec![
                "--result",
                result,
                "--list",
                list,
                "--arch",
                "x86_64/amd/zen4",
            ],
            "--arch requires --classify",
        ),
        (
            vec![
                "--result",
                result,
                "--list",
                list,
                "--classify",
                "--node",
                "0:1024",
            ],
            "needs positive cores and memory",
        ),
        (
            vec![
                "--result",
                result,
                "--list",
                list,
                "--classify",
                "--class",
                "timeout,",
            ],
            "--class needs a class name",
        ),
    ];
    for (args, message) in cases {
        let o = bin().args(&args).output().unwrap();
        assert_eq!(
            o.status.code(),
            Some(2),
            "args={args:?} stderr={}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(message),
            "args={args:?} stderr={}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(o.stdout.is_empty(), "args={args:?}");
    }
}

#[test]
fn oom_failures_and_a_count_mismatch_write_no_retry_list() {
    let dir = tempfile::tempdir().unwrap();
    let (list, result) = class_fixture(dir.path(), &[("Big.eb", "timeout")]);
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    for (script, message) in [
        (
            r#"#!/bin/sh
if [ "$2" = bot-classify ]; then
  printf '{"jobs":[{"sub_class":"timeout"}]}\n'
else
  echo 'oom table unreadable' >&2
  exit 4
fi
"#,
            "oom table unreadable",
        ),
        (
            r#"#!/bin/sh
if [ "$2" = bot-classify ]; then
  printf '{"jobs":[{"sub_class":"timeout"}]}\n'
else
  printf 'fits=1\n'
fi
"#,
            "no `oom=N` line",
        ),
        (
            r#"#!/bin/sh
if [ "$2" = bot-classify ]; then
  printf '{"jobs":[{"sub_class":"timeout"}]}\n'
else
  printf '%s\n' 'Other.eb may run out of memory on cpu (label): 2.0 GiB, the job has 1.0 GiB'
  printf '%s\n' 'oom=1 fits=0 builds=1 nodes=1'
fi
"#,
            "which is not a selected line",
        ),
    ] {
        write_exec(&bin_dir.join("eb-stack"), script);
        let o = bin()
            .args(["--result"])
            .arg(&result)
            .args(["--list"])
            .arg(&list)
            .args(["--classify", "--node", "8:1024", "--drop-oom"])
            .env("PATH", &bin_dir)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{message}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(message),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(o.stdout.is_empty());
    }
}

#[test]
fn help_names_class_and_node() {
    let o = bin().arg("--help").output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    let stdout = String::from_utf8(o.stdout).unwrap();
    assert!(stdout.contains("--class"));
    assert!(stdout.contains("--node"));
    assert!(stdout.contains("--drop-oom"));
    assert!(stdout.contains("--recorded-list"));
    assert!(stdout.contains("does not classify"));
}

#[test]
fn real_eb_stack_classifies_and_drops_a_predicted_oom() {
    let Some(eb_stack) = std::env::var_os("EB_STACK_REAL") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = dir.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    std::os::unix::fs::symlink(&eb_stack, bin_dir.join("eb-stack")).unwrap();
    let timeout = "\
== FAILED: the command timed out before the build finished
";
    let compile = "\
== FAILED: Installation ended unsuccessfully: build failed (first 300 chars): cmd \"make -j 16\" exited with exit code 2
src/fft.c:12:5: error: 'plan' undeclared (first use in this function)
";
    let logs = [
        ("libxc-7.0.0-GCC-15.2.0.eb", "slurm-11.out", timeout),
        ("NoEstimate-1.0.0-GCC-15.2.0.eb", "slurm-12.out", timeout),
        ("CompileMe-1.0.0-GCC-15.2.0.eb", "slurm-13.out", compile),
    ];
    let mut list_text = String::new();
    let mut lines = String::new();
    for (i, (name, file, text)) in logs.iter().enumerate() {
        let log = dir.path().join(file);
        std::fs::write(&log, text).unwrap();
        list_text.push_str(name);
        list_text.push_str(" --robot\n");
        if i > 0 {
            lines.push_str(",\n");
        }
        lines.push_str(&format!(
            r#"{{"easyconfig":"{name}","status":"failed","log":"{}"}}"#,
            log.display()
        ));
    }
    let list = dir.path().join("build.list");
    let result = dir.path().join("result.json");
    std::fs::write(&list, list_text).unwrap();
    std::fs::write(
        &result,
        format!(r#"{{"kind":"result","exit_code":1,"lines":[{lines}]}}"#),
    )
    .unwrap();

    let run = |extra: &[&str]| {
        bin()
            .args(["--result"])
            .arg(&result)
            .args(["--list"])
            .arg(&list)
            .args(extra)
            .env("PATH", &bin_dir)
            .output()
            .unwrap()
    };
    let o = run(&["--classify"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "libxc-7.0.0-GCC-15.2.0.eb --robot\nNoEstimate-1.0.0-GCC-15.2.0.eb --robot\n"
    );

    let o = run(&["--classify", "--class", "compile"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "CompileMe-1.0.0-GCC-15.2.0.eb --robot\n"
    );

    let o = run(&[
        "--classify",
        "--class",
        "timeout",
        "--node",
        "8:32768:x86_64/amd/zen4",
        "--arch",
        "x86_64/amd/zen4",
        "--drop-oom",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr.contains("libxc-7.0.0-GCC-15.2.0.eb may run out of memory"),
        "{stderr}"
    );
    assert_eq!(
        String::from_utf8(o.stdout).unwrap(),
        "NoEstimate-1.0.0-GCC-15.2.0.eb --robot\n"
    );
}
