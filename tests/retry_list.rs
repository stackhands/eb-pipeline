//! `eb-retry-list` turns a run result or summary and its build list into the lines to retry.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_eb-retry-list"))
}

#[test]
fn failed_refused_pending_and_skipped_lines_keep_their_options_in_list_order() {
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
fn summary_json_selects_one_target_and_easystack_preserves_options() {
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
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "");
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
fn docs_name_eb_retry_list() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode");
    for rel in ["howto/follow-runs.org", "reference/cli.org"] {
        let page = std::fs::read_to_string(root.join(rel)).unwrap();
        assert!(page.contains("eb-retry-list"), "{rel}");
        assert!(page.contains("--result"), "{rel}");
        assert!(page.contains("--summary"), "{rel}");
        assert!(page.contains("--list"), "{rel}");
    }
}
