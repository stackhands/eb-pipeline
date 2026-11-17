//! End-to-end OOM preflight against an unmodified eb-stack build.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

#[test]
fn real_eb_stack_refuses_a_known_oom_before_easybuild() {
    let Some(eb_stack) = std::env::var_os("EB_STACK_REAL") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
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
"#,
            root = root.display(),
            bin = Path::new(&eb_stack).display().to_string()
        ),
    )
    .unwrap();
    let list = root.join("known-oom.yml");
    std::fs::write(&list, "easyconfigs:\n  - libxc-7.0.0-GCC-15.2.0.eb\n").unwrap();
    let result = root.join("result.json");
    let out = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["run", "--site"])
        .arg(&site)
        .args(["--list"])
        .arg(&list)
        .args([
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--run-id",
            "real-eb-stack",
            "--log-stamp",
            "20260101_000000",
            "--oom-preflight",
            "--node",
            "8:32768:x86_64/amd/zen4",
            "--result",
        ])
        .arg(&result)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(78),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: Value = serde_json::from_str(&std::fs::read_to_string(result).unwrap()).unwrap();
    assert_eq!(value["counts"]["skipped"], 1);
    assert!(
        value["message"].as_str().unwrap().contains("build skipped"),
        "{value}"
    );
}
