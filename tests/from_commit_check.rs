//! Opt-in `check --check-from-commit` delegation to `eb-stack`.

mod common;

use common::{code, eb_pipeline, setup, stderr, write_exec};
use std::fs::OpenOptions;
use std::process::{Command, Stdio};

#[test]
fn ordinary_check_matches_main_and_starts_no_from_commit_process() {
    let t = setup("");
    let site = t.site.display().to_string();
    let out = eb_pipeline(&t, &["check", "--site", &site], &[]);

    assert_eq!(code(&out), 0);
    assert_eq!(out.stdout, b"");
    assert_eq!(out.stderr, b"INFO [check] t: site file is valid\n");
    assert!(t.calls("ebstack").is_empty());
}

#[test]
fn from_commit_invokes_eb_stack_with_json_and_repo_flags() {
    let t = setup("");
    let list = t.root().join("20261011-eb-5.4.0-demo.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let repos = t.root().join("easyconfigs");
    std::fs::create_dir_all(&repos).unwrap();
    let list = list.display().to_string();
    let repos = repos.display().to_string();
    let out = eb_pipeline(
        &t,
        &[
            "check",
            "--list",
            &list,
            "--check-from-commit",
            "--easyconfigs-repo",
            &repos,
            "--easybuild-version",
            "5.4.0",
        ],
        &[],
    );
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        t.calls("ebstack"),
        format!(
            "stack from-commit --easystack {list} --json --easyconfigs-repo {repos} --easybuild-version 5.4.0\n"
        )
    );
}

#[test]
fn from_commit_without_site_uses_the_stub_on_path() {
    let t = setup("");
    let list = t.root().join("stack.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let list = list.display().to_string();
    let out = eb_pipeline(&t, &["check", "--list", &list, "--check-from-commit"], &[]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(
        t.calls("ebstack"),
        format!("stack from-commit --easystack {list} --json\n")
    );
}

#[test]
fn from_commit_propagates_eb_stack_failure() {
    let t = setup("");
    let list = t.root().join("stack.yaml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let list = list.display().to_string();
    let out = eb_pipeline(
        &t,
        &["check", "--list", &list, "--check-from-commit"],
        &[("STUB_EB_STACK_FAIL", "1")],
    );
    assert_eq!(code(&out), 1, "{}", stderr(&out));
    let err = stderr(&out);
    assert!(err.contains("from-commit failed with status 1"), "{err}");
}

#[test]
fn from_commit_preserves_an_eb_stack_signal_exit_code() {
    let t = setup("");
    write_exec(&t.stubs.join("eb-stack"), "#!/bin/sh\nkill -TERM $$\n");
    let list = t.root().join("stack.yaml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let list = list.display().to_string();
    let out = eb_pipeline(&t, &["check", "--list", &list, "--check-from-commit"], &[]);
    assert_eq!(code(&out), 143, "{}", stderr(&out));
    assert!(
        stderr(&out).contains("from-commit failed with status 143"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_missing_eb_stack_is_a_usage_error() {
    let t = setup(
        r#"
[site.tools.eb_stack]
bin = "@ROOT@/missing-eb-stack"
"#,
    );
    let list = t.root().join("stack.yaml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let output = eb_pipeline(
        &t,
        &[
            "check",
            "--site",
            &t.site.display().to_string(),
            "--list",
            &list.display().to_string(),
            "--check-from-commit",
        ],
        &[],
    );
    assert_eq!(code(&output), 2, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot run"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn check_from_commit_refuses_a_plain_build_list() {
    let t = setup("");
    let list = t.root().join("site.list");
    std::fs::write(&list, "Pkg-1.0.eb\n").unwrap();
    let list = list.display().to_string();
    let out = eb_pipeline(&t, &["check", "--list", &list, "--check-from-commit"], &[]);
    assert_eq!(code(&out), 2, "{}", stderr(&out));
    assert!(t.calls("ebstack").is_empty());
    let err = stderr(&out);
    assert!(err.contains("easystack .yml or .yaml"), "{err}");
}

#[test]
fn plain_check_still_requires_a_site() {
    let t = setup("");
    let out = eb_pipeline(&t, &["check"], &[]);
    assert_eq!(code(&out), 2);
    assert!(stderr(&out).contains("--site") || out.status.code() == Some(2));
}

#[test]
fn from_commit_reports_a_stdout_write_failure() {
    let t = setup("");
    write_exec(
        &t.stubs.join("eb-stack"),
        "#!/bin/sh\ndd if=/dev/zero bs=65536 count=2 2>/dev/null | tr '\\0' x\n",
    );
    let list = t.root().join("stack.yml");
    std::fs::write(&list, "easyconfigs: []\n").unwrap();
    let path = format!(
        "{}:{}",
        t.stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let full = OpenOptions::new().write(true).open("/dev/full").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args([
            "check",
            "--list",
            &list.display().to_string(),
            "--check-from-commit",
        ])
        .env("PATH", path)
        .env("STUB_DIR", t.root())
        .stdin(Stdio::null())
        .stdout(full)
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot print eb-stack output"),
        "{}",
        stderr(&output)
    );
}
