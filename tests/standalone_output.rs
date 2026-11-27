//! Standalone command output follows the CLI broken-pipe contract.
#![allow(clippy::disallowed_methods)]

mod common;

use common::closed_stdout;
use std::process::{Command, Stdio};

const GAP_EB_STACK: &str = r#"#!/bin/sh
echo "$*" >> "$STUB_DIR/gap.calls"
case "$1 $2" in
  "stack gap")
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "--out-dir" ]; then out=$2; shift; fi
      shift
    done
    mkdir -p "$out"
    printf 'route\tname\nrecipe\tLocal\n' > "$out/gap.tsv"
    printf 'Local-1.0-GCCcore-15.2.0.eb\n' > "$out/roots.txt"
    echo "gap warning" >&2
    if [ -n "${STUB_GAP_LARGE_OUTPUT:-}" ]; then
      dd if=/dev/zero bs=65536 count=2 2>/dev/null | tr '\0' x
    fi
    echo "site=1 in-stack=0 filtered=0 open-layer=0 recipe=1 open-easyconfigs=0 no-recipe=0 excluded=0 out=$out"
    exit "${STUB_GAP_EXIT:-0}"
    ;;
esac
exit 2
"#;

fn help_and_version() {
    for binary in [
        env!("CARGO_BIN_EXE_eb-cvmfs"),
        env!("CARGO_BIN_EXE_eb-junit"),
        env!("CARGO_BIN_EXE_eb-retry-list"),
        env!("CARGO_BIN_EXE_eb-shard-weights"),
        env!("CARGO_BIN_EXE_eb-test-map"),
    ] {
        for option in ["--help", "--version"] {
            let output = Command::new(binary)
                .arg(option)
                .stdout(closed_stdout())
                .stderr(Stdio::piped())
                .output()
                .unwrap();
            assert_eq!(
                output.status.code(),
                Some(0),
                "{binary} {option}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

fn shard_weights_closed() {
    let dir = tempfile::tempdir().unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        r#"{"kind":"result","target":"cpu","lines":[{"easyconfig":"A-1.0.eb","phase":"build","status":"success","duration_seconds":1.0}]}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-shard-weights"))
        .args(["--result"])
        .arg(&result)
        .env_remove("EB_PIPELINE_RESULT")
        .env_remove("EB_PIPELINE_TARGET")
        .stdout(closed_stdout())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "eb-shard-weights: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn test_map_closed(format: &str) {
    let dir = tempfile::tempdir().unwrap();
    let map = dir.path().join("map.tsv");
    std::fs::write(&map, "G g\n").unwrap();
    let list = dir.path().join("l.txt");
    std::fs::write(&list, "G-1.0.eb\n").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_eb-test-map"));
    cmd.args(["--list"]).arg(&list).args(["--format", format]);
    if format != "eessi-modules" {
        cmd.args(["--map"]).arg(&map);
    }
    let output = cmd
        .stdout(closed_stdout())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "eb-test-map {format}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn test_map_text() {
    test_map_closed("text");
}

fn test_map_eessi() {
    test_map_closed("eessi-modules");
}

fn junit_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");
    std::fs::write(
        &path,
        r#"{"kind":"result","schema_version":1,"site":"s","target":"t","build_type":"b","run_id":"r","exit_code":0,"lines":[]}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-junit"))
        .args(["--result"])
        .arg(&path)
        .env_remove("EB_PIPELINE_RESULT")
        .stdout(closed_stdout())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "eb-junit: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn retry_list_closed() {
    let dir = tempfile::tempdir().unwrap();
    let list = dir.path().join("list.txt");
    std::fs::write(&list, "Want.eb\n").unwrap();
    let result = dir.path().join("result.json");
    std::fs::write(
        &result,
        r#"{"kind":"result","exit_code":1,"lines":[{"easyconfig":"Want.eb","status":"failed"}]}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-retry-list"))
        .args(["--result"])
        .arg(&result)
        .args(["--list"])
        .arg(&list)
        .env_remove("EB_PIPELINE_RESULT")
        .env_remove("EB_PIPELINE_LIST")
        .env_remove("EB_PIPELINE_TARGET")
        .stdout(closed_stdout())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "eb-retry-list: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cvmfs_dry_run_closed() {
    let output = Command::new(env!("CARGO_BIN_EXE_eb-cvmfs"))
        .args(["--dry-run", "repo.example", "--", "sync"])
        .stdout(closed_stdout())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "eb-cvmfs: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn gap_closed() {
    let t = common::setup(
        r#"
[shards]
enabled = false
easyconfigs = ["/recipes/local"]
installed_modules_dirs = ["/base/cpu"]
installed_modules_dirs_from_env = ["GAP_BASE"]
[watch]
enabled = false
modules_dirs = ["/base/cpu", "/base/gpu"]
easyconfigs = ["/recipes/local", "/recipes/extra"]
"#,
    );
    common::write_exec(&t.stubs.join("eb-stack"), GAP_EB_STACK);
    let text = std::fs::read_to_string(&t.site)
        .unwrap()
        .replacen(
            "name = \"cpu\"",
            "name = \"cpu\"\nmodules_root = \"/local/cpu/modules\"",
            1,
        )
        .replacen(
            "name = \"gpu\"",
            "name = \"gpu\"\nmodules_root = \"/local/gpu/modules\"",
            1,
        );
    std::fs::write(&t.site, text).unwrap();
    let out_dir = t.root().join("gap-report");
    let path = format!(
        "{}:{}",
        t.stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let site = t.site.display().to_string();
    let out = out_dir.display().to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args([
            "check",
            "--site",
            &site,
            "--gap-out",
            &out,
            "--generation",
            "2026.1",
        ])
        .env("PATH", path)
        .env("STUB_DIR", t.root())
        .env("GAP_BASE", "/base/accel")
        .env("STUB_GAP_LARGE_OUTPUT", "1")
        .stdin(Stdio::null())
        .stdout(closed_stdout())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "check --gap-out: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(out_dir.join("gap.stdout.log").is_file());
}

#[test]
fn standalone_help_and_version_tolerate_a_closed_consumer() {
    for row in [
        help_and_version as fn(),
        shard_weights_closed,
        test_map_text,
        test_map_eessi,
        junit_closed,
        retry_list_closed,
        cvmfs_dry_run_closed,
        gap_closed,
    ] {
        row();
    }
}
