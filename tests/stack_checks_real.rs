//! End-to-end stack check against an unmodified eb-stack build.

use std::path::Path;
use std::process::Command;

#[test]
fn real_eb_stack_generation_check_passes_before_a_run() {
    let Some(eb_stack) = std::env::var_os("EB_STACK_REAL") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let baseline = root.join("baseline.lock.json");
    std::fs::write(
        &baseline,
        r#"{
  "schema_version": 1,
  "toolchain": {"name": "foss", "version": "2026.1"},
  "generation_label": "foss-2026.1",
  "packages": [],
  "solver": {
    "engine": "resolvo",
    "engine_version": "test",
    "timestamp": "2026-11-01T00:00:00Z",
    "criteria": []
  }
}
"#,
    )
    .unwrap();
    let site = root.join("site.toml");
    std::fs::write(
        &site,
        format!(
            r#"schema_version = 1
kind = "site"
[site]
name = "test"
[site.tools.eb_stack]
bin = {bin:?}
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
[filesystems]
scratch = "{root}/scratch"
tmp = "{root}/tmp"
build_path = "{root}/build"
install_root = "{root}/install"
log_root = "{root}/logs"
[modules]
init = []
[sbom]
enabled = false
lock = {baseline:?}
"#,
            root = root.display(),
            bin = Path::new(&eb_stack).display().to_string(),
            baseline = baseline.display().to_string()
        ),
    )
    .unwrap();
    let list = root.join("eessi-2026.06-eb-5.4.1-2026.1.yml");
    std::fs::write(&list, "easyconfigs:\n  - Autoconf-2.72-GCCcore-15.2.0.eb\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["check", "--site"])
        .arg(&site)
        .args(["--list"])
        .arg(&list)
        .args(["--stack-checks", "generation"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let build_list = root.join("site.list");
    std::fs::write(&build_list, "zlib-1.3.1-GCCcore-15.2.0.eb\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["check", "--site"])
        .arg(&site)
        .args(["--list"])
        .arg(&build_list)
        .args(["--stack-checks", "provided"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
