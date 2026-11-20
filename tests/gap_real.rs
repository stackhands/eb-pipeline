//! Real `check --gap-out` run against an unmodified eb-stack binary.
#![allow(clippy::disallowed_methods)]

use std::path::Path;
use std::process::Command;

#[test]
fn real_eb_stack_writes_gap_and_shadow_reports() {
    let Some(eb_stack) = std::env::var_os("EB_STACK_REAL") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let local_modules = root.join("local/modules/all/Local");
    let base_modules = root.join("base/modules/all/Base");
    let easyconfigs = root.join("easyconfigs/l/Local");
    std::fs::create_dir_all(&local_modules).unwrap();
    std::fs::create_dir_all(&base_modules).unwrap();
    std::fs::create_dir_all(&easyconfigs).unwrap();
    std::fs::write(
        local_modules.join("1.0-GCCcore-15.2.0.lua"),
        "help([==[Local]==])\n",
    )
    .unwrap();
    std::fs::write(
        base_modules.join("2.0-GCCcore-15.2.0.lua"),
        "help([==[Base]==])\n",
    )
    .unwrap();
    std::fs::write(
        easyconfigs.join("Local-1.0-GCCcore-15.2.0.eb"),
        "\
easyblock = 'ConfigureMake'
name = 'Local'
version = '1.0'
homepage = 'https://example.invalid'
description = 'fixture'
toolchain = {'name': 'GCCcore', 'version': '15.2.0'}
",
    )
    .unwrap();

    let site = root.join("site.toml");
    std::fs::write(
        &site,
        format!(
            r#"schema_version = 1
kind = "site"
[site]
name = "gap-real"
[site.tools.eb_stack]
bin = {bin:?}
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
modules_root = {local_root:?}
[filesystems]
scratch = {scratch:?}
tmp = {tmp:?}
build_path = {build:?}
install_root = {install:?}
log_root = {logs:?}
[modules]
init = []
[shards]
enabled = false
easyconfigs = [{easyconfigs:?}]
installed_modules_dirs = [{base:?}]
"#,
            bin = Path::new(&eb_stack).display().to_string(),
            local_root = root.join("local/modules").display().to_string(),
            scratch = root.join("scratch").display().to_string(),
            tmp = root.join("tmp").display().to_string(),
            build = root.join("build").display().to_string(),
            install = root.join("install").display().to_string(),
            logs = root.join("logs").display().to_string(),
            easyconfigs = root.join("easyconfigs").display().to_string(),
            base = root.join("base/modules/all").display().to_string(),
        ),
    )
    .unwrap();

    let report = root.join("report");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["check", "--site"])
        .arg(&site)
        .args(["--gap-out"])
        .arg(&report)
        .args(["--generation", "2026.1", "--shadow"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).starts_with("site=1 "),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    for file in ["gap.tsv", "roots.txt", "version-change.tsv", "shadow.json"] {
        assert!(report.join(file).is_file(), "missing {file}");
    }
    let shadow: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(report.join("shadow.json")).unwrap())
            .unwrap();
    assert_eq!(shadow["hits"], serde_json::json!([]));
}
