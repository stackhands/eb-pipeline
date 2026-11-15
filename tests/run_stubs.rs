//! `run` against stub tool binaries written into a temporary directory.

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static SPAWN: Mutex<()> = Mutex::new(());

const EB_STACK: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
case "$1" in --version) echo "eb-stack 0.7.4"; exit 0;; esac
[ "$1" = stack ] || exit 64
sub="$2"; shift 2
echo "$sub $*" >> "$D/eb-stack.calls"
case "$sub" in
  admit)
    n=$(cat "$D/admit.count" 2>/dev/null || echo 0); n=$((n+1)); echo $n > "$D/admit.count"
    ok="${STUB_ADMIT_OK_AT:-1}"
    if [ "$ok" -gt 0 ] && [ "$n" -ge "$ok" ]; then echo "{\"admitted\":true,\"attempt\":$n}"; exit 0; fi
    echo "{\"admitted\":false,\"attempt\":$n}"; exit 78;;
  shard)
    case "${STUB_SHARD:-ok}" in unsharded) exit 3;; fail) exit 1;; esac
    out=""; while [ $# -gt 0 ]; do [ "$1" = --out-dir ] && out="$2"; shift; done
    mkdir -p "$out"
    printf 'Foo-1.0-foss-2025a.eb\n' > "$out/shard-01.list"
    printf 'Bar-2.0-foss-2025a.eb\n' > "$out/shard-02.list"
    echo '{"prelude":{"file":null},"shards":[{"name":"shard-01","file":"shard-01.list","inode_footprint":111},{"name":"shard-02","file":"shard-02.list","inode_footprint":222}]}' > "$out/plan.json"
    exit 0;;
esac
exit 64
"#;

const EB: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
ec="$1"; stem="${ec%.eb}"
echo "$*" >> "$D/eb.calls"
if [ -n "${STUB_EB_SLEEP:-}" ]; then
  echo started > "$D/started"
  trap 'echo term > "$D/termed"; kill $pid 2>/dev/null; exit 0' TERM
  sleep 30 &
  pid=$!
  wait $pid
  exit 0
fi
case ",${STUB_EB_FAIL:-}," in *",$stem,"*) echo "ERROR: $stem failed"; exit 1;; esac
exit 0
"#;

const KEEP_LOGS: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
case "$1" in --version) echo "eb-keep-logs 0.2.0"; exit 0;; esac
echo "$*" >> "$D/keeplogs.calls"
exit 0
"#;

const STAGED: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
case "$1" in --version) echo "eb-staged-rebuild 1.4.0"; exit 0;; esac
echo "$*" >> "$D/staged.calls"
exit "${STUB_STAGED_CODE:-0}"
"#;

struct Env {
    dir: tempfile::TempDir,
    site: PathBuf,
    stubs: PathBuf,
}

fn write_exec(path: &Path, text: &str) {
    let _g = SPAWN.lock().unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn setup(extra_site: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let stubs = root.join("stubs");
    std::fs::create_dir(&stubs).unwrap();
    for (n, t) in [
        ("eb-stack", EB_STACK),
        ("eb", EB),
        ("eb-keep-logs", KEEP_LOGS),
        ("eb-staged-rebuild", STAGED),
    ] {
        write_exec(&stubs.join(n), t);
    }
    let r = root.display();
    let s = stubs.display();
    let site = format!(
        r#"
schema_version = 1
kind = "site"
[site]
name = "t"
[site.tools.eb_stack]
bin = "{s}/eb-stack"
min_version = "0.7.0"
[site.tools.eb_keep_logs]
bin = "{s}/eb-keep-logs"
min_version = "0.2"
[site.tools.eb_staged_rebuild]
bin = "{s}/eb-staged-rebuild"
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
[filesystems]
scratch = "{r}/scratch"
tmp = "{r}/tmp"
build_path = "{r}/build"
install_root = "{r}/install"
log_root = "{r}/logs"
[modules]
init = []
[logs]
keep_failed = true
{extra_site}
"#
    );
    let site_path = root.join("site.toml");
    std::fs::write(&site_path, site).unwrap();
    Env {
        dir,
        site: site_path,
        stubs,
    }
}

struct Out {
    code: i32,
    stderr: String,
    result: Option<Value>,
}

fn driver(env: &Env, list: &str, vars: &[(&str, &str)], extra: &[&str]) -> std::process::Child {
    let root = env.dir.path();
    let list_path = root.join("list.txt");
    std::fs::write(&list_path, list).unwrap();
    let path = format!(
        "{}:{}",
        env.stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut c = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
    c.args(["run", "--site"])
        .arg(&env.site)
        .arg("--list")
        .arg(&list_path)
        .args(["--target", "cpu", "--build-type", "production", "--run-id", "r1"])
        .arg("--result")
        .arg(root.join("result.json"))
        .args(extra)
        .env("PATH", path)
        .env("STUB_DIR", root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in vars {
        c.env(k, v);
    }
    let _g = SPAWN.lock().unwrap();
    c.spawn().unwrap()
}

fn finish(env: &Env, child: std::process::Child) -> Out {
    let o = child.wait_with_output().unwrap();
    let result = std::fs::read_to_string(env.dir.path().join("result.json"))
        .ok()
        .map(|t| serde_json::from_str(&t).unwrap());
    Out {
        code: o.status.code().unwrap_or(-1),
        stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        result,
    }
}

fn go(env: &Env, list: &str, vars: &[(&str, &str)]) -> Out {
    let c = driver(env, list, vars, &[]);
    finish(env, c)
}

fn calls(env: &Env, name: &str) -> Vec<String> {
    std::fs::read_to_string(env.dir.path().join(name))
        .map(|t| t.lines().map(str::to_string).collect())
        .unwrap_or_default()
}

const LIST: &str = "Foo-1.0-foss-2025a.eb\nBar-2.0-foss-2025a.eb\n";

#[test]
fn success_exits_zero_and_records_lines() {
    let env = setup("");
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let r = o.result.unwrap();
    assert_eq!(r["exit_code"], 0);
    assert_eq!(r["counts"]["success"], 2);
    assert_eq!(r["lines"].as_array().unwrap().len(), 2);
    assert!(r["not_configured"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p == "admission"));
    assert!(calls(&env, "keeplogs.calls").is_empty());
    assert!(o.stderr.contains("INFO [build] cpu: Foo-1.0-foss-2025a: ok"), "{}", o.stderr);
}

#[test]
fn one_failed_line_exits_one_never_a_sum() {
    let env = setup("");
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb\nBar-2.0-foss-2025a.eb\nBaz-3.0-foss-2025a.eb\n",
        &[("STUB_EB_FAIL", "Foo-1.0-foss-2025a,Baz-3.0-foss-2025a")],
    );
    assert_eq!(o.code, 1, "{}", o.stderr);
    let r = o.result.unwrap();
    assert_eq!(r["counts"]["failed"], 2);
    assert_eq!(r["counts"]["success"], 1);
    let k = calls(&env, "keeplogs.calls");
    assert_eq!(k.len(), 2);
    assert!(k[0].starts_with("failed 1 Foo-1.0-foss-2025a "), "{k:?}");
}

const ADMISSION: &str = r#"
[admission]
quota_cmd = "/opt/q"
reservations = "/scratch/res"
attempts = 3
wait_seconds = 0
max_wait_seconds = 0
"#;

#[test]
fn admission_refused_after_n_attempts_exits_78() {
    let env = setup(ADMISSION);
    let o = go(&env, LIST, &[("STUB_ADMIT_OK_AT", "0")]);
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(o.stderr.contains("admission refused after 3 attempts"), "{}", o.stderr);
    assert!(o.stderr.contains(r#"{"admitted":false,"attempt":3}"#), "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty(), "no build step may start");
    let r = o.result.unwrap();
    assert_eq!(r["admission"][0]["attempts"], 3);
    assert_eq!(r["admission"][0]["outcome"], "refused");
    assert_eq!(r["counts"]["skipped"], 2);
}

#[test]
fn admission_admitted_on_second_attempt() {
    let env = setup(ADMISSION);
    let o = go(&env, LIST, &[("STUB_ADMIT_OK_AT", "2")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let r = o.result.unwrap();
    assert_eq!(r["admission"][0]["attempts"], 2);
    assert_eq!(r["admission"][0]["outcome"], "admitted");
    assert_eq!(calls(&env, "eb.calls").len(), 2);
}

#[test]
fn admission_required_without_config_refuses() {
    let env = setup("[admission]\nrequired = true\n");
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
}

#[test]
fn staged_rebuild_refusal_is_translated() {
    let env = setup("[rebuild]\nstaged = true\n");
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --rebuild\n",
        &[("STUB_STAGED_CODE", "2")],
    );
    assert_eq!(o.code, 78, "{}", o.stderr);
    let r = o.result.unwrap();
    let l = &r["lines"][0];
    assert_eq!(l["status"], "refused");
    assert_eq!(l["translated_from"], 2);
    assert!(l["note"].as_str().unwrap().contains("refusal"));
    assert_eq!(calls(&env, "staged.calls").len(), 1);
    assert!(calls(&env, "eb.calls").is_empty());
}

#[test]
fn staged_rebuild_failure_is_a_failure() {
    let env = setup("[rebuild]\nstaged = true\n");
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --rebuild\n",
        &[("STUB_STAGED_CODE", "1")],
    );
    assert_eq!(o.code, 1, "{}", o.stderr);
}

#[test]
fn unsharded_plan_builds_the_list_and_exits_zero() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    let o = go(&env, LIST, &[("STUB_SHARD", "unsharded")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let r = o.result.unwrap();
    assert_eq!(r["sharded"], false);
    let shard = r["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "shard")
        .unwrap();
    assert_eq!(shard["status"], "unsharded");
    assert_eq!(calls(&env, "eb.calls").len(), 2);
}

#[test]
fn sharded_plan_admits_each_unit_with_its_footprint() {
    let env = setup(&format!(
        "[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n{ADMISSION}"
    ));
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let r = o.result.unwrap();
    assert_eq!(r["sharded"], true);
    assert_eq!(r["lines"][0]["unit"], "shard-01");
    let c = calls(&env, "eb-stack.calls");
    let admits: Vec<&String> = c.iter().filter(|l| l.starts_with("admit")).collect();
    assert_eq!(admits.len(), 2);
    assert!(admits[0].contains("--footprint 111"));
    assert!(admits[1].contains("--footprint 222"));
}

#[test]
fn shard_failure_exits_one() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    let o = go(&env, LIST, &[("STUB_SHARD", "fail")]);
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
}

#[test]
fn tool_below_min_version_exits_two_before_any_work() {
    let env = setup("");
    let text = std::fs::read_to_string(&env.site)
        .unwrap()
        .replace("min_version = \"0.2\"", "min_version = \"9.0\"");
    std::fs::write(&env.site, text).unwrap();
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert!(o.stderr.contains("below min_version"), "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
}

#[test]
fn sigterm_is_forwarded_and_exit_is_143() {
    let env = setup("");
    let mut child = driver(&env, LIST, &[("STUB_EB_SLEEP", "1")], &[]);
    let started = env.dir.path().join("started");
    let t0 = Instant::now();
    while !started.exists() {
        assert!(t0.elapsed() < Duration::from_secs(20), "stub never started");
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: the pid is the driver child this test started.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let t0 = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(t0.elapsed() < Duration::from_secs(20), "driver did not exit");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(143));
    assert!(env.dir.path().join("termed").exists(), "child saw SIGTERM");
    assert_eq!(calls(&env, "eb.calls").len(), 1, "no further line started");
    let r: Value =
        serde_json::from_str(&std::fs::read_to_string(env.dir.path().join("result.json")).unwrap())
            .unwrap();
    assert_eq!(r["exit_code"], 143);
    assert_eq!(r["interrupted_by"], "SIGTERM");
}

#[test]
fn dry_run_starts_nothing() {
    let env = setup("");
    let c = driver(&env, LIST, &[], &["--dry-run"]);
    let o = c.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("[build] eb Foo-1.0-foss-2025a.eb"), "{out}");
    assert!(calls(&env, "eb.calls").is_empty());
}
