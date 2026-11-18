#[path = "common/exec_stub.rs"]
mod exec_stub;

use exec_stub::CommandSyncExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture {
    root: tempfile::TempDir,
    site: PathBuf,
    bundle: PathBuf,
    manifest: String,
}

fn executable(path: &Path, text: &str) {
    exec_stub::write_exec(path, text);
}

fn fixture(origin: &str, package: &str, recipe: &str) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let stubs = root.path().join("stubs");
    let bundle = root.path().join("bundle");
    let recipe_dir = bundle.join("easyconfigs/d/demo");
    std::fs::create_dir_all(&stubs).unwrap();
    std::fs::create_dir_all(&recipe_dir).unwrap();
    executable(
        &stubs.join("eb"),
        "#!/bin/sh\necho \"$*\" >> \"$STUB_DIR/eb.calls\"\n",
    );
    executable(
        &stubs.join("pip"),
        "#!/bin/sh\necho \"$*\" >> \"$STUB_DIR/pip.calls\"\n",
    );
    executable(
        &stubs.join("Rscript"),
        "#!/bin/sh\necho \"$*\" >> \"$STUB_DIR/r.calls\"\n",
    );
    let shell_init = root.path().join("modules.sh");
    std::fs::write(
        &shell_init,
        "module() { printf '%s\\n' \"$*\" >> \"$STUB_DIR/module.calls\"; }\n",
    )
    .unwrap();
    let manifest =
        format!(r#"{{"schema_version":1,"origin":"{origin}","package":{{"name":"{package}"}}}}"#);
    std::fs::write(bundle.join("package.plan.json"), &manifest).unwrap();
    std::fs::write(recipe_dir.join(recipe), "# generated\n").unwrap();
    let site = root.path().join("site.toml");
    std::fs::write(
        &site,
        format!(
            r#"
schema_version = 1
kind = "site"
[site]
name = "package-plan-test"
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
shell_init = "{shell_init}"
"#,
            root = root.path().display(),
            shell_init = shell_init.display()
        ),
    )
    .unwrap();
    Fixture {
        root,
        site,
        bundle,
        manifest,
    }
}

fn command(fixture: &Fixture, subcommand: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
    command
        .arg(subcommand)
        .arg("--site")
        .arg(&fixture.site)
        .arg("--package-plan")
        .arg(&fixture.bundle)
        .args([
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--run-id",
            "package-plan",
            "--log-stamp",
            "20261117_120000",
        ])
        .env(
            "PATH",
            format!(
                "{}:{}",
                fixture.root.path().join("stubs").display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("STUB_DIR", fixture.root.path());
    command
}

#[test]
fn plan_builds_declared_recipes_and_records_the_manifest_digest() {
    let fixture = fixture("pypi", "demo", "demo-1.0-GCCcore-15.2.0.eb");
    let output = command(&fixture, "plan").sync_output();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        plan["package_plan_digest"],
        format!("{:x}", Sha256::digest(fixture.manifest.as_bytes()))
    );
    let build = plan["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["name"] == "build")
        .unwrap();
    let argv = build["commands"][0]["argv"].as_array().unwrap();
    assert!(argv.iter().any(|arg| {
        arg.as_str()
            .is_some_and(|arg| arg.ends_with("demo-1.0-GCCcore-15.2.0.eb"))
    }));
    assert!(argv.iter().any(|arg| {
        arg.as_str().is_some_and(|arg| {
            arg == format!("--robot={}", fixture.bundle.join("easyconfigs").display())
        })
    }));
    let verification = plan["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["name"] == "verification")
        .unwrap();
    assert_eq!(verification["status"], "not configured");
    assert_eq!(verification["reason"], "not requested");
}

#[test]
fn verification_is_opt_in_and_runs_pip_check_with_the_product_module() {
    let fixture = fixture("pypi", "demo", "demo-1.0-GCCcore-15.2.0.eb");
    let result = fixture.root.path().join("result.json");
    let output = command(&fixture, "run")
        .arg("--verify-package-plan")
        .arg("--result")
        .arg(&result)
        .sync_output();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("pip.calls")).unwrap(),
        "check\n"
    );
    let modules = std::fs::read_to_string(fixture.root.path().join("module.calls")).unwrap();
    assert!(
        modules
            .lines()
            .any(|line| line == "load demo/1.0-GCCcore-15.2.0"),
        "{modules}"
    );
    let value: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(result).unwrap()).unwrap();
    assert_eq!(value["exit_code"], 0);
    assert_eq!(
        value["phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|phase| { phase["name"] == "verification" })
            .unwrap()["status"],
        "ok"
    );
    assert!(value["package_plan_digest"].as_str().is_some());
}

#[test]
fn a_cran_plan_uses_the_r_description_dependency_check() {
    let fixture = fixture("cran", "DemoR", "DemoR-2.0-GCCcore-15.2.0.eb");
    let output = command(&fixture, "plan")
        .arg("--verify-package-plan")
        .sync_output();
    assert_eq!(output.status.code(), Some(0));
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let verification = plan["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["name"] == "verification")
        .unwrap();
    let argv = verification["commands"][0]["argv"].as_array().unwrap();
    assert!(argv.iter().any(|arg| arg == "Rscript"));
    assert!(argv.iter().any(|arg| arg == "DemoR"));
    assert!(argv.iter().any(|arg| {
        arg.as_str()
            .is_some_and(|arg| arg.contains("package_dependencies"))
    }));
}

#[test]
fn default_list_plan_is_byte_identical_to_main() {
    let root = tempfile::tempdir().unwrap();
    let site = root.path().join("site.toml");
    let list = root.path().join("list.txt");
    std::fs::copy(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates/small-cluster.toml"),
        &site,
    )
    .unwrap();
    std::fs::write(
        &list,
        "Foo-1.0-GCCcore-15.2.0.eb\nBar-2.0-GCCcore-15.2.0.eb --rebuild\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"))
        .args(["plan", "--site"])
        .arg(&site)
        .arg("--list")
        .arg(&list)
        .args([
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--run-id",
            "stable",
            "--log-stamp",
            "20261117_120000",
        ])
        .sync_output();
    assert_eq!(output.status.code(), Some(0));
    let normalized = String::from_utf8(output.stdout)
        .unwrap()
        .replace(root.path().to_str().unwrap(), "/tmp/eb-pipeline-default");
    assert_eq!(
        format!("{:x}", Sha256::digest(normalized.as_bytes())),
        "8d858f0a8fbd3c91b7959d155a36e8b1c61feb9808576bf70fa7bf43797e93ec",
        "ordinary --list plan bytes changed from main"
    );
}
