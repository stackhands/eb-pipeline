//! `audit` shells out to eb-stack and copies its rebuild lists.

mod common;

use common::{code, eb_pipeline, eessi_vars, setup, stderr, write_exec};
use std::path::Path;
use std::process::Command;

const EB_STACK_AUDIT: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
case "$1" in --version) echo "eb-stack 0.7.4"; exit 0;; esac
[ -n "${STUB_AUDIT_SIGNAL:-}" ] && kill "-$STUB_AUDIT_SIGNAL" "$$"
echo "$*" >> "$D/ebstack.calls"
cmd="$1"; shift
if [ "$cmd" = stack ]; then cmd="stack-$1"; shift; fi
while [ $# -gt 0 ]; do
  case "$1" in
    --out-dir) out="$2"; shift;;
    --out) outfile="$2"; shift;;
    --backport-list) backport="$2"; shift;;
  esac
  shift
done
backport_count="${STUB_BACKPORT:-0}"
case "$cmd" in
  rebuild-set)
    mkdir -p "$out/rebuilds"
    printf 'easyconfigs:\n  - Foo-1.0-GCCcore-13.2.0.eb\n' > "$out/rebuilds/CVE-2026-0001.yml"
    if [ -n "${STUB_AUDIT_TRUNCATED_REBUILD_SET:-}" ]; then
      printf '{' > "$out/rebuild-set.json"
    else
      printf '{"schema_version":5,"rows":[]}\n' > "$out/rebuild-set.json"
    fi
    echo "changed=1 rows=1 must-rebuild=1 review=0 no-rebuild=0 missing=0 easystacks=1 skipped_easyconfigs=0 findings_modules=1 backport=$backport_count out=$out"
    ;;
  stack-sbom)
    if [ -n "${STUB_AUDIT_TRUNCATED_SBOM:-}" ]; then
      printf '{' > "$outfile"
    else
      printf '%s\n' '{"bomFormat":"CycloneDX","specVersion":"1.5"}' > "$outfile"
    fi
    : > "$backport"
    echo "sbom=$outfile components=1 recipes_read=1 recipes_unreadable=0 with_artifact_facts=0"
    echo "backport=$backport_count recorded=1 stack=1 out=$backport"
    ;;
  component-reach)
    mkdir -p "$out"
    if [ -n "${STUB_AUDIT_TRUNCATED_COMPONENT:-}" ]; then
      printf '{' > "$out/component-reach.json"
    else
      printf '{"kind":"ok"}\n' > "$out/component-reach.json"
    fi
    echo "cve=CVE-2026-0001 packages=1 generation=2026.06 carriers=1 changed=1 must-rebuild=1 review=0 no-rebuild=0 out=$out"
    ;;
esac
exit 0
"#;

#[test]
fn findings_mode_copies_rebuild_lists() {
    let t = setup("");
    write_exec(&t.stubs.join("eb-stack"), EB_STACK_AUDIT);
    t.eessi_module("Foo", "1.0-GCCcore-13.2.0");
    let findings = t.root().join("findings.csv");
    std::fs::write(
        &findings,
        "severity,vuln_id,package,used_in\nhigh,CVE-2026-0001,zlib,Foo/1.0-GCCcore-13.2.0\n",
    )
    .unwrap();
    let out_dir = t.root().join("audit");
    let vars: Vec<(&str, &str)> = eessi_vars(&t, "2026.06")
        .into_iter()
        .map(|(k, v)| (k, Box::leak(v.into_boxed_str()) as &str))
        .collect();
    let o = eb_pipeline(
        &t,
        &[
            "audit",
            "--site",
            &t.site.display().to_string(),
            "--target",
            "cpu",
            "--findings",
            &findings.display().to_string(),
            "--out-dir",
            &out_dir.display().to_string(),
        ],
        &vars,
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    assert!(out_dir.join("rebuilds/CVE-2026-0001.yml").is_file());
    let calls = t.calls("ebstack");
    assert!(calls.contains("rebuild-set"), "{calls}");
    assert!(!calls.contains("stack sbom"), "{calls}");
}

#[test]
fn strict_mode_fails_when_eb_stack_reports_backports() {
    let t = setup("");
    write_exec(&t.stubs.join("eb-stack"), EB_STACK_AUDIT);
    t.eessi_module("Foo", "1.0-GCCcore-13.2.0");
    let findings = t.root().join("findings.csv");
    std::fs::write(
        &findings,
        "severity,vuln_id,package,used_in\nhigh,CVE-2026-0001,zlib,Foo/1.0-GCCcore-13.2.0\n",
    )
    .unwrap();
    let out_dir = t.root().join("audit");
    let mut c = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
    c.env(
        "PATH",
        format!(
            "{}:{}",
            t.stubs.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    )
    .env("STUB_DIR", t.root())
    .env("STUB_BACKPORT", "2")
    .env("EESSI_VERSION", "2026.06")
    .env("EESSI_SOFTWARE_SUBDIR", "x86_64/generic")
    .env("EESSI_MODULEPATH", t.eessi_modules())
    .args([
        "audit",
        "--site",
        &t.site.display().to_string(),
        "--target",
        "cpu",
        "--findings",
        &findings.display().to_string(),
        "--strict",
        "--out-dir",
        &out_dir.display().to_string(),
    ]);
    let o = c.output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("strict mode"),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn cve_mode_runs_stack_sbom_and_component_reach() {
    let t = setup("[sbom]\nenabled = true\nlock = \"@ROOT@/stack.lock.json\"");
    write_exec(&t.stubs.join("eb-stack"), EB_STACK_AUDIT);
    t.eessi_module("Foo", "1.0-GCCcore-13.2.0");
    std::fs::write(t.root().join("stack.lock.json"), "{\"packages\":[]}").unwrap();
    let out_dir = t.root().join("audit");
    let vars: Vec<(&str, &str)> = eessi_vars(&t, "2026.06")
        .into_iter()
        .map(|(k, v)| (k, Box::leak(v.into_boxed_str()) as &str))
        .collect();
    let o = eb_pipeline(
        &t,
        &[
            "audit",
            "--site",
            &t.site.display().to_string(),
            "--target",
            "cpu",
            "--cve",
            "CVE-2026-0001",
            "--cve",
            "CVE-2026-0002",
            "--out-dir",
            &out_dir.display().to_string(),
        ],
        &vars,
    );
    assert_eq!(code(&o), 0, "{}", stderr(&o));
    let calls = t.calls("ebstack");
    assert!(calls.contains("stack sbom"), "{calls}");
    let component_calls = calls
        .lines()
        .filter(|line| line.starts_with("component-reach "))
        .count();
    assert_eq!(component_calls, 2, "{calls}");
}

#[test]
fn audit_refuses_truncated_rebuild_set_json() {
    let t = setup("");
    write_exec(&t.stubs.join("eb-stack"), EB_STACK_AUDIT);
    t.eessi_module("Foo", "1.0-GCCcore-13.2.0");
    let findings = t.root().join("findings.csv");
    std::fs::write(
        &findings,
        "severity,vuln_id,package,used_in\nhigh,CVE-2026-0001,zlib,Foo/1.0-GCCcore-13.2.0\n",
    )
    .unwrap();
    let out_dir = t.root().join("audit");
    let mut vars: Vec<(&str, &str)> = eessi_vars(&t, "2026.06")
        .into_iter()
        .map(|(key, value)| (key, Box::leak(value.into_boxed_str()) as &str))
        .collect();
    vars.push(("STUB_AUDIT_TRUNCATED_REBUILD_SET", "1"));

    let output = eb_pipeline(
        &t,
        &[
            "audit",
            "--site",
            &t.site.display().to_string(),
            "--target",
            "cpu",
            "--findings",
            &findings.display().to_string(),
            "--out-dir",
            &out_dir.display().to_string(),
        ],
        &vars,
    );

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("rebuild-set.json")
            && stderr(&output).contains("wrote invalid JSON"),
        "{}",
        stderr(&output)
    );
    assert!(!out_dir.join("rebuilds").exists());
}

#[test]
fn audit_refuses_truncated_sbom_json() {
    let t = setup("[sbom]\nenabled = true\nlock = \"@ROOT@/stack.lock.json\"");
    write_exec(&t.stubs.join("eb-stack"), EB_STACK_AUDIT);
    t.eessi_module("Foo", "1.0-GCCcore-13.2.0");
    std::fs::write(t.root().join("stack.lock.json"), "{\"packages\":[]}").unwrap();
    let out_dir = t.root().join("audit");
    let mut vars: Vec<(&str, &str)> = eessi_vars(&t, "2026.06")
        .into_iter()
        .map(|(key, value)| (key, Box::leak(value.into_boxed_str()) as &str))
        .collect();
    vars.push(("STUB_AUDIT_TRUNCATED_SBOM", "1"));

    let output = eb_pipeline(
        &t,
        &[
            "audit",
            "--site",
            &t.site.display().to_string(),
            "--target",
            "cpu",
            "--cve",
            "CVE-2026-0001",
            "--out-dir",
            &out_dir.display().to_string(),
        ],
        &vars,
    );

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("stack.cdx.json")
            && stderr(&output).contains("wrote invalid JSON"),
        "{}",
        stderr(&output)
    );
    assert!(!t.calls("ebstack").contains("component-reach"));
}

#[test]
fn audit_refuses_truncated_component_json() {
    let t = setup("[sbom]\nenabled = true\nlock = \"@ROOT@/stack.lock.json\"");
    write_exec(&t.stubs.join("eb-stack"), EB_STACK_AUDIT);
    t.eessi_module("Foo", "1.0-GCCcore-13.2.0");
    std::fs::write(t.root().join("stack.lock.json"), "{\"packages\":[]}").unwrap();
    let out_dir = t.root().join("audit");
    let mut vars: Vec<(&str, &str)> = eessi_vars(&t, "2026.06")
        .into_iter()
        .map(|(key, value)| (key, Box::leak(value.into_boxed_str()) as &str))
        .collect();
    vars.push(("STUB_AUDIT_TRUNCATED_COMPONENT", "1"));

    let output = eb_pipeline(
        &t,
        &[
            "audit",
            "--site",
            &t.site.display().to_string(),
            "--target",
            "cpu",
            "--cve",
            "CVE-2026-0001",
            "--out-dir",
            &out_dir.display().to_string(),
        ],
        &vars,
    );

    assert_eq!(code(&output), 1, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("component-reach.json")
            && stderr(&output).contains("wrote invalid JSON"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn empty_or_control_cves_fail_before_site_loading() {
    for cve in ["   ", "CVE-2026-0001\nCVE-2026-0002"] {
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
            .current_dir(dir.path())
            .args([
                "audit",
                "--site",
                "not-read.toml",
                "--target",
                "cpu",
                "--cve",
                cve,
                "--out-dir",
                "not-written",
            ])
            .output()
            .unwrap();
        assert_eq!(code(&output), 2, "{}", stderr(&output));
        assert!(
            stderr(&output).contains("--cve must be non-empty and contain no control characters"),
            "{}",
            stderr(&output)
        );
        assert!(!dir.path().join("not-written").exists());
    }
}

#[test]
fn an_audit_helper_signal_keeps_its_exit_code() {
    let t = setup("");
    write_exec(&t.stubs.join("eb-stack"), EB_STACK_AUDIT);
    t.eessi_module("Foo", "1.0-GCCcore-13.2.0");
    let findings = t.root().join("findings.csv");
    std::fs::write(
        &findings,
        "severity,vuln_id,package,used_in\nhigh,CVE-2026-0001,zlib,Foo/1.0-GCCcore-13.2.0\n",
    )
    .unwrap();
    let out_dir = t.root().join("audit");
    let mut vars: Vec<(&str, &str)> = eessi_vars(&t, "2026.06")
        .into_iter()
        .map(|(key, value)| (key, Box::leak(value.into_boxed_str()) as &str))
        .collect();
    vars.push(("STUB_AUDIT_SIGNAL", "TERM"));
    let output = eb_pipeline(
        &t,
        &[
            "audit",
            "--site",
            &t.site.display().to_string(),
            "--target",
            "cpu",
            "--findings",
            &findings.display().to_string(),
            "--out-dir",
            &out_dir.display().to_string(),
        ],
        &vars,
    );
    assert_eq!(code(&output), 143, "{}", stderr(&output));
    assert!(
        stderr(&output).contains("exited 143"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn generated_cli_assets_include_the_audit_subcommand() {
    let dir = tempfile::tempdir().unwrap();
    let completions = dir.path().join("completions");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["completions", "--shell", "bash", "--out-dir"])
        .arg(&completions)
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let bash = std::fs::read_to_string(completions.join("eb-pipeline.bash")).unwrap();
    assert!(bash.contains("audit"), "{bash}");

    let man = dir.path().join("man");
    let output = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["man", "--out-dir"])
        .arg(&man)
        .output()
        .unwrap();
    assert_eq!(code(&output), 0, "{}", stderr(&output));
    let page = std::fs::read_to_string(man.join("eb-pipeline.1")).unwrap();
    assert!(page.contains("audit"), "{page}");
}

#[test]
fn real_eb_stack_e2e_runs_when_enabled() {
    let Some(eb_stack) = std::env::var_os("EB_STACK_REAL") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let modules = root.join("eessi/modules/all/Foo");
    let install = root.join("eessi/software/Foo/1.0-GCCcore-13.2.0/easybuild");
    std::fs::create_dir_all(&modules).unwrap();
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(
        install.join("Foo-1.0-GCCcore-13.2.0.eb"),
        "easyblock = 'ConfigureMake'\nname = 'Foo'\nversion = '1.0'\ntoolchain = {'name': 'GCCcore', 'version': '13.2.0'}\nhomepage = 'https://example.invalid'\ndescription = 'fixture'\n",
    )
    .unwrap();
    std::fs::write(
        modules.join("1.0-GCCcore-13.2.0.lua"),
        format!(
            "help([==[Foo]==])\nlocal root = \"{}\"\n",
            root.join("eessi/software/Foo/1.0-GCCcore-13.2.0").display()
        ),
    )
    .unwrap();
    let site = root.join("site.toml");
    std::fs::write(
        &site,
        format!(
            r#"schema_version = 1
kind = "site"
[site]
name = "audit-e2e"
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
    let findings = root.join("findings.csv");
    std::fs::write(
        &findings,
        "severity,vuln_id,package,used_in\nhigh,CVE-2026-0001,foo,Foo/1.0-GCCcore-13.2.0\n",
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .env("EESSI_VERSION", "2026.06")
        .env("EESSI_SOFTWARE_SUBDIR", "x86_64/generic")
        .env("EESSI_MODULEPATH", root.join("eessi/modules/all"))
        .args(["audit", "--site"])
        .arg(&site)
        .args(["--target", "cpu", "--findings"])
        .arg(&findings)
        .args(["--out-dir"])
        .arg(root.join("audit"))
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
