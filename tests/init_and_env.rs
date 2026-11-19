//! `init` templates and configuration from the environment.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
    // Only what a test sets reaches the binary, whatever the caller exported.
    c.env_clear();
    c
}

fn template(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("templates")
        .join(format!("{name}.toml"))
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn init_lists_every_template() {
    let o = bin().args(["init", "--list"]).output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let out = String::from_utf8_lossy(&o.stdout);
    for t in eb_pipeline::templates::TEMPLATES {
        assert!(out.contains(t.name), "{out}");
    }
}

#[test]
fn init_writes_a_valid_site_file_and_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    for t in eb_pipeline::templates::TEMPLATES {
        let p = dir.path().join(format!("{}.toml", t.name));
        let o = bin()
            .args(["init", "--template", t.name, "--out"])
            .arg(&p)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(0), "{}", text(&o));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), t.text);
        assert_eq!(t.text, std::fs::read_to_string(template(t.name)).unwrap());

        let o = bin().args(["check", "--site"]).arg(&p).output().unwrap();
        assert_eq!(o.status.code(), Some(0), "{}", text(&o));

        std::fs::write(&p, "kept").unwrap();
        let o = bin()
            .args(["init", "--template", t.name, "--out"])
            .arg(&p)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{}", text(&o));
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "kept");
    }
}

#[test]
fn init_reports_an_output_filesystem_error_as_a_runtime_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("absent").join("site.toml");
    let output = bin()
        .args(["init", "--template", "small-cluster", "--out"])
        .arg(&path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert_eq!(output.stdout, b"");
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("cannot create"), "{error}");
    assert!(error.contains(path.to_str().unwrap()), "{error}");
}

#[test]
fn init_hints_print_next_steps_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("site.toml");
    let o = bin()
        .arg("init")
        .args(["--template", "eessi-site", "--hints"])
        .arg("--out")
        .arg(&p)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("hint: check the site file"), "{err}");
    assert!(err.contains("hint: list target names"), "{err}");
    assert!(
        err.contains("release 0.3.0 has no targets command"),
        "{err}"
    );
}

#[test]
fn init_prints_on_stdout_and_refuses_an_unknown_name() {
    let o = bin()
        .args(["init", "--template", "small-cluster"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        eb_pipeline::templates::get("small-cluster").unwrap().text
    );
    let o = bin().args(["init", "--template", "nope"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    let o = bin().args(["init"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
}

#[test]
fn help_lists_dry_run_output_among_what_stdout_carries() {
    let o = bin().arg("--help").output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let help = String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let carried = help
        .split_once("Output streams: stdout carries ")
        .and_then(|(_, rest)| rest.split_once(" output only."))
        .map(|(carried, _)| carried)
        .unwrap_or_default();
    for name in [
        "plan",
        "explain",
        "schema",
        "init",
        "targets",
        "run --dry-run",
        "watch --dry-run",
    ] {
        assert!(carried.contains(name), "{name} missing from {carried:?}");
    }
}

#[test]
fn help_names_the_eb_stack_stack_subcommands() {
    let o = bin().arg("--help").output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let help = String::from_utf8_lossy(&o.stdout);
    assert!(help.contains("eb-stack stack admit"), "{help}");
    assert!(help.contains("eb-stack stack shard"), "{help}");
    assert!(!help.contains("eb-stack admit    "), "{help}");
    assert!(!help.contains("eb-stack shard    "), "{help}");
}

#[test]
fn help_explains_every_eb_stack_shard_exit_mapping() {
    let o = bin().arg("--help").output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let help = String::from_utf8_lossy(&o.stdout);
    assert!(
        help.contains("2 usage, any other non-zero failed"),
        "{help}"
    );
}

#[test]
fn flags_fall_back_to_the_environment() {
    let o = bin()
        .arg("check")
        .env("EB_PIPELINE_SITE", template("small-cluster"))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    // A flag on the command line wins over the variable.
    let o = bin()
        .args(["check", "--site"])
        .arg(template("small-cluster"))
        .env("EB_PIPELINE_SITE", "/nonexistent/site.toml")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let o = bin().arg("check").output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
}

#[test]
fn site_values_come_from_the_environment() {
    let o = bin()
        .args(["explain", "--format", "json", "--site"])
        .arg(template("small-cluster"))
        .env("EB_PIPELINE__FILESYSTEMS__INSTALL_ROOT", "/opt/eb/from-env")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(
        String::from_utf8_lossy(&o.stdout).contains("/opt/eb/from-env"),
        "{}",
        text(&o)
    );
    assert!(
        String::from_utf8_lossy(&o.stderr)
            .contains("filesystems.install_root set from EB_PIPELINE__FILESYSTEMS__INSTALL_ROOT"),
        "{}",
        text(&o)
    );

    let o = bin()
        .args(["check", "--site"])
        .arg(template("small-cluster"))
        .env("EB_PIPELINE__FILESYSTEMS__NO_SUCH_KEY", "x")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
}

#[test]
fn help_and_the_exit_status_reference_list_the_codes_run_returns() {
    let o = bin().arg("--help").output().unwrap();
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let help = String::from_utf8_lossy(&o.stdout).into_owned();
    let help_codes: Vec<u32> = help
        .lines()
        .skip_while(|l| *l != "Exit codes:")
        .skip(1)
        .take_while(|l| !l.is_empty())
        .filter_map(|l| l.split_whitespace().next()?.parse().ok())
        .collect();
    let page = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/orgmode/reference/exit-status.org"),
    )
    .unwrap();
    let page_codes: Vec<u32> = page
        .lines()
        .skip_while(|l| *l != "** Statuses")
        .skip_while(|l| !l.starts_with('|'))
        .take_while(|l| l.starts_with('|'))
        .filter_map(|l| l.trim_matches('|').split('|').next()?.trim().parse().ok())
        .collect();
    assert_eq!(help_codes, [0, 1, 2, 75, 78, 130, 143], "{help}");
    assert_eq!(page_codes, help_codes);
}
