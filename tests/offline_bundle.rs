//! Observable behavior of opt-in offline bundles.

#[path = "common/exec_stub.rs"]
mod exec_stub;

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const EB_STACK: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
case "$1" in --version) echo "eb-stack 0.8.1"; exit 0;; esac
echo "$*" >> "$D/eb-stack.calls"
case "$1 $2" in
  "stack order")
    shift 2; out=; easystack=; hashes=
    while [ $# -gt 0 ]; do
      case "$1" in
        --out) out="$2"; shift;;
        --easystack-out) easystack="$2"; shift;;
        --hashes-out) hashes="$2"; shift;;
      esac
      shift
    done
    if [ -n "$hashes" ]; then
      mkdir -p "$(dirname "$hashes")"
      printf '%s\n' 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  Foo/1.0' > "$hashes"
      echo "hashes=$hashes modules=1 incomplete=0"
      echo "order=$hashes builds=1"
      exit 0
    fi
    printf '%s\n' 'Foo-1.0.eb' > "$out"
    printf '%s\n' 'easyconfigs:' '  - Foo-1.0.eb' > "$easystack";;
  "stack files")
    [ -f "$4" ] || exit 9;;
  "package inspect")
    shift 2; out=
    while [ $# -gt 0 ]; do
      [ "$1" = --out-dir ] && { out="$2"; shift; }
      shift
    done
    mkdir -p "$out"
    printf '%s\n' '{"kind":"package-plan"}' > "$out/package.plan.json"
    printf '%s\n' '{"bomFormat":"CycloneDX"}' > "$out/package.sbom.cdx.json";;
  *) exit 8;;
esac
"#;

const EB: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
echo "$*" >> "$D/eb.calls"
case " $* " in
  *" --fetch "*)
    mkdir -p "$EASYBUILD_SOURCEPATH/extensions"
    printf source > "$EASYBUILD_SOURCEPATH/Foo-1.0.tar.gz"
    printf patch > "$EASYBUILD_SOURCEPATH/Foo-fix.patch"
    printf extension > "$EASYBUILD_SOURCEPATH/extensions/helper.whl";;
  *)
    echo "$*" >> "$D/eb.build.calls"
    mkdir -p "$EASYBUILD_INSTALLPATH/modules/all/Foo"
    printf 'help([[Foo]])\n' > "$EASYBUILD_INSTALLPATH/modules/all/Foo/1.0.lua";;
esac
"#;

const WRAPPER: &str = r#"#!/bin/sh
echo "$*" >> "${STUB_DIR:?}/wrapper.calls"
exec "$@"
"#;

const CURL: &str = r#"#!/bin/sh
cat >/dev/null
"#;

struct Fixture {
    dir: tempfile::TempDir,
    site: PathBuf,
    list: PathBuf,
    bundle: PathBuf,
}

fn executable(path: &Path, text: &str) {
    exec_stub::write_exec(path, text);
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn fixture(wrapper: bool, asset_digest: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let stubs = root.join("stubs");
    let sources = root.join("sources");
    let robot = root.join("easyconfigs");
    std::fs::create_dir_all(&stubs).unwrap();
    std::fs::create_dir_all(&robot).unwrap();
    executable(&stubs.join("eb-stack"), EB_STACK);
    executable(&stubs.join("eb"), EB);
    executable(&stubs.join("offline"), WRAPPER);
    executable(&stubs.join("curl"), CURL);
    let asset = root.join("mnist.bin");
    std::fs::write(&asset, b"runtime data").unwrap();
    let foreign = root.join("pyproject.toml");
    std::fs::write(&foreign, "[project]\nname='demo'\nversion='1'\n").unwrap();
    let wrapper_line = if wrapper {
        format!(
            "offline_wrapper = [{:?}]\n",
            stubs.join("offline").display().to_string()
        )
    } else {
        String::new()
    };
    let site_text = format!(
        r#"schema_version = 1
kind = "site"
[site]
name = "test"
[site.tools.eb_stack]
bin = {stack:?}
min_version = "0.8"
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
[filesystems]
scratch = {scratch:?}
tmp = {tmp:?}
build_path = {build:?}
source_paths = [{sources:?}]
install_root = {install:?}
log_root = {logs:?}
[modules]
init = []
[sources]
easyconfigs = [{robot:?}]
{wrapper_line}[[sources.assets]]
path = {asset:?}
url = "https://assets.invalid/mnist.bin"
sha256 = {asset_digest:?}
consumer = "image-test"
local_name = "mnist.bin"
[[sources.foreign]]
source = {foreign:?}
format = "pypi"
toolchain = "foss/2026.1"
[notify]
when = "always"
webhook = "https://notify.invalid/hook"
curl = {curl:?}
"#,
        stack = stubs.join("eb-stack").display().to_string(),
        scratch = root.join("scratch").display().to_string(),
        tmp = root.join("tmp").display().to_string(),
        build = root.join("build").display().to_string(),
        sources = sources.display().to_string(),
        install = root.join("install").display().to_string(),
        logs = root.join("logs").display().to_string(),
        robot = robot.display().to_string(),
        asset = asset.display().to_string(),
        foreign = foreign.display().to_string(),
        curl = stubs.join("curl").display().to_string(),
    );
    let site = root.join("site.toml");
    std::fs::write(&site, site_text).unwrap();
    let list = root.join("build.list");
    std::fs::write(&list, "Foo-1.0.eb\n").unwrap();
    let bundle = root.join("bundle");
    Fixture {
        dir,
        site,
        list,
        bundle,
    }
}

fn run(f: &Fixture, extra: &[&str]) -> Output {
    let root = f.dir.path();
    let path = format!(
        "{}:{}",
        root.join("stubs").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["run", "--site"])
        .arg(&f.site)
        .arg("--list")
        .arg(&f.list)
        .args([
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--run-id",
            "offline-test",
            "--log-stamp",
            "20261117_220000",
        ])
        .args(extra)
        .current_dir(root)
        .env("PATH", path)
        .env("STUB_DIR", root)
        .output()
        .unwrap()
}

fn plan(f: &Fixture) -> Output {
    Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["plan", "--site"])
        .arg(&f.site)
        .arg("--list")
        .arg(&f.list)
        .args([
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--run-id",
            "offline-test",
            "--log-stamp",
            "20261117_220000",
            "--out",
            "-",
        ])
        .current_dir(f.dir.path())
        .output()
        .unwrap()
}

fn calls(f: &Fixture, name: &str) -> String {
    std::fs::read_to_string(f.dir.path().join(name)).unwrap_or_default()
}

#[test]
fn absent_and_disabled_offline_bundles_keep_default_plan_bytes_identical() {
    let f = fixture(true, &digest(b"runtime data"));
    let configured = std::fs::read_to_string(&f.site).unwrap();
    let asset_start = configured.find("[[sources.assets]]").unwrap();
    let notify_start = configured.find("[notify]").unwrap();
    let mut absent = format!(
        "{}{}",
        &configured[..asset_start],
        &configured[notify_start..]
    );
    absent = absent
        .lines()
        .filter(|line| {
            !line.starts_with("easyconfigs = ") && !line.starts_with("offline_wrapper = ")
        })
        .collect::<Vec<_>>()
        .join("\n");
    absent.push('\n');

    std::fs::write(&f.site, absent).unwrap();
    let without_offline_config = plan(&f);
    assert_eq!(
        without_offline_config.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&without_offline_config.stderr)
    );

    std::fs::write(&f.site, configured).unwrap();
    let disabled_offline_bundle = plan(&f);
    assert_eq!(
        disabled_offline_bundle.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&disabled_offline_bundle.stderr)
    );
    assert_eq!(
        disabled_offline_bundle.stdout, without_offline_config.stdout,
        "default plan output changed when offline bundle configuration was present but unused"
    );
}

#[test]
fn bundle_indexes_prefetched_and_runtime_assets_and_isolates_later_commands() {
    let f = fixture(true, &digest(b"runtime data"));
    let bundle = f.bundle.display().to_string();
    let output = run(&f, &["--offline-bundle", &bundle, "--require-offline"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let planner = calls(&f, "eb-stack.calls");
    assert!(planner.contains("stack order "), "{planner}");
    assert!(planner.contains("--root Foo==1.0"), "{planner}");
    assert!(planner.contains("stack files "), "{planner}");
    assert!(planner.contains("package inspect "), "{planner}");
    assert!(f.bundle.join("planner/build-order.txt").is_file());
    assert!(f
        .bundle
        .join("planner/foreign-01/package.plan.json")
        .is_file());

    let text = std::fs::read_to_string(f.bundle.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(manifest["kind"], "offline-bundle");
    let files = manifest["files"].as_array().unwrap();
    assert_eq!(
        files
            .iter()
            .filter(|entry| entry["consumer"] == "easybuild")
            .count(),
        3,
        "{text}"
    );
    assert!(
        files.iter().any(|entry| entry["consumer"] == "eb-stack"),
        "{text}"
    );
    let runtime = files
        .iter()
        .find(|entry| entry["consumer"] == "image-test")
        .unwrap();
    assert_eq!(runtime["url"], "https://assets.invalid/mnist.bin");
    assert_eq!(runtime["sha256"], digest(b"runtime data"));
    for entry in files {
        let object = entry["object"].as_str().unwrap();
        assert!(f.bundle.join(object).is_file(), "{entry}");
    }
    let wrapped = calls(&f, "wrapper.calls");
    assert!(wrapped.contains("eb Foo-1.0.eb"), "{wrapped}");
    assert!(wrapped.contains("curl --silent"), "{wrapped}");
    assert!(calls(&f, "eb.build.calls").contains("Foo-1.0.eb"));

    let before = std::fs::read_to_string(f.bundle.join("manifest.json")).unwrap();
    let again = run(&f, &["--offline-bundle", &bundle, "--require-offline"]);
    assert_eq!(again.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("must name an absent path"),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(f.bundle.join("manifest.json")).unwrap(),
        before
    );
}

#[test]
fn a_bundle_manifest_is_not_taken_for_a_run_summary() {
    let f = fixture(true, &digest(b"runtime data"));
    let bundle = f.bundle.display().to_string();
    let output = run(&f, &["--offline-bundle", &bundle, "--require-offline"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = std::fs::read_to_string(f.bundle.join("manifest.json")).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&manifest).unwrap()["kind"],
        "offline-bundle"
    );
    let summary = f.dir.path().join("logs/20261117_220000/summary.json");
    std::fs::write(&summary, &manifest).unwrap();
    let why = "kind `offline-bundle` is not `summary`";

    let again = run(&f, &[]);
    let stderr = String::from_utf8_lossy(&again.stderr);
    assert_eq!(again.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("no summary written: ") && stderr.contains(why),
        "{stderr}"
    );
    assert_eq!(std::fs::read_to_string(&summary).unwrap(), manifest);

    let status = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["status", "--site"])
        .arg(&f.site)
        .output()
        .unwrap();
    let shown = String::from_utf8_lossy(&status.stdout);
    assert_eq!(status.status.code(), Some(0), "{shown}");
    assert!(shown.contains(why), "{shown}");
}

#[test]
fn default_run_does_not_plan_or_write_a_bundle() {
    let f = fixture(true, &digest(b"runtime data"));
    let output = run(&f, &[]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(calls(&f, "eb-stack.calls").is_empty());
    assert!(calls(&f, "wrapper.calls").is_empty());
    assert!(!f.bundle.exists());
}

#[test]
fn require_offline_needs_a_wrapper_and_asset_hashes_fail_closed() {
    let f = fixture(false, &digest(b"runtime data"));
    let bundle = f.bundle.display().to_string();
    let output = run(&f, &["--offline-bundle", &bundle, "--require-offline"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(calls(&f, "eb-stack.calls").is_empty());
    assert!(calls(&f, "eb.build.calls").is_empty());

    let f = fixture(true, &"0".repeat(64));
    let bundle = f.bundle.display().to_string();
    let output = run(&f, &["--offline-bundle", &bundle]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("has SHA-256"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(calls(&f, "eb.build.calls").is_empty());
    assert!(!f.bundle.join("manifest.json").exists());
}

#[test]
fn a_prefetch_failure_refuses_to_build_or_publish_a_partial_bundle() {
    let f = fixture(true, &digest(b"runtime data"));
    executable(
        &f.dir.path().join("stubs/eb"),
        "#!/bin/sh\ncase \" $* \" in *\" --fetch \"*) exit 7;; *) echo build >> \"${STUB_DIR:?}/eb.build.calls\";; esac\n",
    );
    let bundle = f.bundle.display().to_string();
    let output = run(&f, &["--offline-bundle", &bundle, "--require-offline"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no offline bundle was published"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(calls(&f, "eb.build.calls").is_empty());
    assert!(!f.bundle.join("manifest.json").exists());
}

#[test]
fn resumed_units_are_prefetched_again_when_writing_a_bundle() {
    let f = fixture(true, &digest(b"runtime data"));
    let mut site = std::fs::read_to_string(&f.site).unwrap();
    site.push_str(&format!(
        "\n[resume]\nenabled = true\neasyconfigs = [{:?}]\nshards = 1\n",
        f.dir.path().join("easyconfigs").display().to_string()
    ));
    std::fs::write(&f.site, site).unwrap();

    let first = f.dir.path().join("first-result.json");
    let first_name = first.display().to_string();
    let output = run(&f, &["--result", &first_name]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    std::fs::remove_dir_all(f.dir.path().join("sources")).unwrap();
    std::fs::create_dir(f.dir.path().join("sources")).unwrap();
    for name in ["eb.calls", "eb.build.calls", "eb-stack.calls"] {
        let _ = std::fs::remove_file(f.dir.path().join(name));
    }

    let second = f.dir.path().join("second-result.json");
    let second_name = second.display().to_string();
    let bundle = f.bundle.display().to_string();
    let output = run(
        &f,
        &[
            "--resume-result",
            &first_name,
            "--result",
            &second_name,
            "--offline-bundle",
            &bundle,
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(calls(&f, "eb.calls").contains("--fetch"));
    assert!(
        calls(&f, "eb.build.calls").is_empty(),
        "the completed unit should still skip its build"
    );
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(f.bundle.join("manifest.json")).unwrap())
            .unwrap();
    assert!(
        manifest["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["consumer"] == "easybuild"),
        "{manifest}"
    );
}

#[test]
fn a_relative_bundle_below_a_source_root_does_not_index_itself() {
    let f = fixture(true, &digest(b"runtime data"));
    let root = f.dir.path().display().to_string();
    let text = std::fs::read_to_string(&f.site)
        .unwrap()
        .replace(&format!("{root}/sources"), &root);
    std::fs::write(&f.site, text).unwrap();
    let output = run(&f, &["--offline-bundle", "bundle"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = std::fs::read_to_string(f.bundle.join("manifest.json")).unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    assert!(
        manifest["files"].as_array().unwrap().iter().all(|entry| {
            entry["consumer"] != "easybuild"
                || !entry["url"]
                    .as_str()
                    .unwrap_or_default()
                    .starts_with("easybuild-source:bundle/")
        }),
        "{text}"
    );
}

#[test]
fn configured_assets_must_be_regular_files() {
    let f = fixture(true, &digest(b"runtime data"));
    let asset = f.dir.path().join("mnist.bin");
    std::fs::remove_file(&asset).unwrap();
    std::os::unix::fs::symlink(f.dir.path().join("pyproject.toml"), &asset).unwrap();
    let bundle = f.bundle.display().to_string();
    let output = run(&f, &["--offline-bundle", &bundle]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("is not a regular file"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!f.bundle.join("manifest.json").exists());
}

#[test]
fn foreign_inputs_are_inspected_when_selection_keeps_no_build_lines() {
    let f = fixture(true, &digest(b"runtime data"));
    let text = std::fs::read_to_string(&f.site).unwrap().replace(
        "name = \"cpu\"\n[filesystems]",
        "name = \"cpu\"\n[targets.select]\noption = \"--only\"\nvalue = \"cpu\"\nwhen_absent = \"none\"\n[filesystems]",
    );
    std::fs::write(&f.site, text).unwrap();
    let bundle = f.bundle.display().to_string();
    let output = run(&f, &["--offline-bundle", &bundle]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let planner = calls(&f, "eb-stack.calls");
    assert!(!planner.contains("stack order"), "{planner}");
    assert!(!planner.contains("stack files"), "{planner}");
    assert!(planner.contains("package inspect"), "{planner}");
    assert!(f.bundle.join("manifest.json").is_file());
}
