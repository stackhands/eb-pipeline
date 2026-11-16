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
    if [ "$ok" -gt 0 ] && [ "$n" -ge "$ok" ]; then
      res=""; job=""
      while [ $# -gt 0 ]; do
        case "$1" in --reservations) res="$2";; --job) job="$2";; esac
        shift
      done
      if [ -d "$res" ]; then
        if [ -e "$res/$job.json" ]; then echo held >> "$D/admit.saw"; else echo free >> "$D/admit.saw"; fi
        ph=""; [ -n "${STUB_ADMIT_PHASE:-}" ] && ph=",\"phase\":\"$STUB_ADMIT_PHASE\""
        echo "{\"job\":\"$job\",\"footprint\":1,\"ts\":\"2026-01-01T00:00:00Z\"$ph}" > "$res/$job.json"
      fi
      echo "{\"admitted\":true,\"attempt\":$n}"; exit 0
    fi
    echo "{\"admitted\":false,\"attempt\":$n}"; exit 78;;
  shard)
    echo "${EASYBUILD_FILTER_DEPS:-}" > "$D/shard.filterdeps"
    case "${STUB_SHARD:-ok}" in unsharded) exit 3;; fail) exit 1;; esac
    out=""; bl=""
    while [ $# -gt 0 ]; do
      [ "$1" = --out-dir ] && out="$2"
      [ "$1" = --build-list ] && bl="$2"
      if [ "$1" = --installed-modules-dir ] && [ ! -d "$2" ]; then
        echo "error: --installed-modules-dir $2: No such file or directory (os error 2)" >&2; exit 2
      fi
      shift
    done
    mkdir -p "$out"
    if [ -n "$bl" ]; then echo "$bl" > "$D/shard.listpath"; cat "$bl" > "$D/shard.listtext" 2>/dev/null; fi
    if [ "${STUB_SHARD:-ok}" = fromlist ]; then
      : > "$out/shard-01.list"; : > "$out/shard-02.list"; i=0
      while IFS= read -r l; do
        i=$((i+1)); echo "$l" >> "$out/shard-0$(( (i-1) % 2 + 1 )).list"
      done < "$bl"
      echo '{"prelude":{"file":null},"shards":[{"name":"shard-01","file":"shard-01.list","inode_footprint":111},{"name":"shard-02","file":"shard-02.list","inode_footprint":222}]}' > "$out/plan.json"
      exit 0
    fi
    printf 'Foo-1.0-foss-2025a.eb\n' > "$out/shard-01.list"
    printf 'Bar-2.0-foss-2025a.eb\n' > "$out/shard-02.list"
    if [ "${STUB_SHARD:-ok}" = cpu ]; then
      printf 'Pre-1.0-foss-2025a.eb\n' > "$out/cpu.list"
      echo '{"cpu_prerequisites":{"file":"cpu.list","inode_footprint":5},"prelude":{"file":null},"shards":[{"name":"shard-01","file":"shard-01.list","inode_footprint":111},{"name":"shard-02","file":"shard-02.list","inode_footprint":222}]}' > "$out/plan.json"
      exit 0
    fi
    echo '{"prelude":{"file":null},"shards":[{"name":"shard-01","file":"shard-01.list","inode_footprint":111},{"name":"shard-02","file":"shard-02.list","inode_footprint":222}]}' > "$out/plan.json"
    exit 0;;
esac
exit 64
"#;

const EB: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
ec="$1"; stem="${ec%.eb}"
echo "$*" >> "$D/eb.calls"
case " $* " in *" --fetch "*) kind=fetch;; *) kind=build;; esac
env | grep '^EB_PIPELINE_\|^EASYBUILD_\|^ROUTE_' | sort > "$D/env.$kind.$stem"
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

const ROUTE: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
echo "$*" >> "$D/route.calls"
while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done
shift
exec "$@"
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
id=""; prev=""
for a in "$@"; do [ "$prev" = --run-id ] && id="$a"; prev="$a"; done
if grep -qxF -- "$id" "$D/staged.runs" 2>/dev/null; then
  echo "run $id already exists; resume it or choose another --run-id" >&2; exit 2
fi
echo "$id" >> "$D/staged.runs"
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
        ("route", ROUTE),
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
        .args([
            "--target",
            "cpu",
            "--build-type",
            "production",
            "--run-id",
            "r1",
        ]);
    if !extra.contains(&"--log-stamp") {
        c.args(["--log-stamp", "20260101_000000"]);
    }
    c.arg("--result")
        .arg(root.join("result.json"))
        .args(extra)
        .env("PATH", path)
        .env("STUB_DIR", root)
        .env("SLURM_JOB_ID", "12345")
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

/// Install the module file the staged-rebuild check looks for.
fn install_module(env: &Env, rel: &str) {
    let p = env.dir.path().join("install/modules/all").join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "-- module\n").unwrap();
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
    assert!(
        o.stderr
            .contains("INFO [build] cpu: Foo-1.0-foss-2025a: ok"),
        "{}",
        o.stderr
    );
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

/// Replace the site's `[modules]` table: source `init`, then load `modules`.
fn with_modules(env: &Env, init: &str, modules: &[&str]) {
    let path = env.dir.path().join("init.sh");
    std::fs::write(&path, init).unwrap();
    let list: Vec<String> = modules.iter().map(|m| format!("{m:?}")).collect();
    let text = std::fs::read_to_string(&env.site).unwrap().replace(
        "[modules]\ninit = []",
        &format!(
            "[modules]\nshell_init = {:?}\ninit = [{}]",
            path.display().to_string(),
            list.join(", ")
        ),
    );
    std::fs::write(&env.site, text).unwrap();
}

/// `module` for the stub inits: it records each call, and `Missing/1.0` does
/// not load.
const MODULE_FN: &str = r#"module() {
    echo "$*" >> "$STUB_DIR/module.calls"
    [ "$*" != "load Missing/1.0" ]
}
"#;

#[test]
fn the_module_wrapper_sources_an_init_that_reads_unset_variables() {
    // EESSI 2026.06's init/lmod/bash reads EESSI_NO_MODULE_PURGE_ON_INIT
    // without a default.
    let env = setup("");
    let init = format!("if [ -z \"$EESSI_NO_MODULE_PURGE_ON_INIT\" ]; then :; fi\n{MODULE_FN}");
    with_modules(
        &env,
        &init,
        &["EESSI/2026.06", "EESSI-extend/2026.06-easybuild"],
    );
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "eb.calls").len(), 2);
    assert_eq!(
        calls(&env, "module.calls")[..3],
        [
            "purge",
            "load EESSI/2026.06",
            "load EESSI-extend/2026.06-easybuild"
        ]
    );
}

#[test]
fn the_module_wrapper_sources_an_init_whose_last_test_is_false() {
    let env = setup("");
    let init = format!("{MODULE_FN}[ -n \"${{EESSI_MODULE_DEBUG_INIT:-}}\" ] && echo debug\n");
    with_modules(&env, &init, &["EESSI/2026.06"]);
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "eb.calls").len(), 2);
}

#[test]
fn a_module_that_does_not_load_fails_the_line() {
    let env = setup("");
    with_modules(&env, MODULE_FN, &["Missing/1.0"]);
    let o = go(&env, "Foo-1.0-foss-2025a.eb\n", &[]);
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
    assert_eq!(o.result.unwrap()["counts"]["failed"], 1);
    assert_eq!(
        calls(&env, "module.calls"),
        ["purge", "load Missing/1.0", "purge", "load Missing/1.0"],
        "the build and keep-failed steps each stop at the module"
    );
}

const ADMISSION: &str = r#"
[admission]
quota_cmd = "/opt/q"
reservations = "/scratch/res"
default_footprint = 500
attempts = 3
wait_seconds = 0
max_wait_seconds = 0
"#;

#[test]
fn admission_refused_after_n_attempts_exits_78() {
    let env = setup(ADMISSION);
    let o = go(&env, LIST, &[("STUB_ADMIT_OK_AT", "0")]);
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(
        o.stderr.contains("admission refused after 3 attempts"),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr.contains(r#"{"admitted":false,"attempt":3}"#),
        "{}",
        o.stderr
    );
    assert!(
        calls(&env, "eb.calls").is_empty(),
        "no build step may start"
    );
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

/// Put the reservations and the admission lock under the test directory;
/// returns the reservation file of the job the driver runs as.
fn reservations_here(env: &Env, lock_kind: &str) -> PathBuf {
    let root = env.dir.path();
    let res = root.join("res");
    std::fs::create_dir(&res).unwrap();
    let text = std::fs::read_to_string(&env.site).unwrap().replace(
        "reservations = \"/scratch/res\"",
        &format!(
            "reservations = \"{}\"\nlock = \"{}\"\nlock_kind = \"{lock_kind}\"",
            res.display(),
            root.join("admission.lock").display()
        ),
    );
    std::fs::write(&env.site, text).unwrap();
    res.join("12345.json")
}

fn result_phase<'a>(r: &'a Value, name: &str) -> &'a Value {
    r["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()
}

#[test]
fn each_unit_releases_its_reservation_when_it_ends() {
    let env = setup(&format!(
        "[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n{ADMISSION}"
    ));
    let held = reservations_here(&env, "flock");
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "admit.saw"), ["free", "free"]);
    assert!(!held.exists(), "{}", o.stderr);
    assert!(
        o.stderr.contains("INFO [admission] cpu: released"),
        "{}",
        o.stderr
    );
}

#[test]
fn a_reservation_another_writer_rewrote_is_kept() {
    for (phase, kept) in [("forecast", true), ("waiting", true), ("admitted", false)] {
        let env = setup(ADMISSION);
        let held = reservations_here(&env, "flock");
        let o = go(&env, LIST, &[("STUB_ADMIT_PHASE", phase)]);
        assert_eq!(o.code, 0, "{phase}: {}", o.stderr);
        assert_eq!(held.exists(), kept, "{phase}: {}", o.stderr);
        assert_eq!(
            o.stderr
                .contains("no longer this job's admitted reservation"),
            kept,
            "{phase}: {}",
            o.stderr
        );
    }
}

/// Take the admission lock in this process, as another controller would.
fn hold_lock(path: &Path, posix: bool) -> std::fs::File {
    use std::os::fd::AsRawFd;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    // SAFETY: the descriptor is open, and a zeroed `libc::flock` is a valid
    // value of that plain C struct.
    let rc = unsafe {
        if posix {
            let mut l: libc::flock = std::mem::zeroed();
            l.l_type = libc::F_WRLCK as _;
            l.l_whence = libc::SEEK_SET as _;
            libc::fcntl(f.as_raw_fd(), libc::F_SETLK, &raw const l)
        } else {
            libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
        }
    };
    assert_eq!(rc, 0, "{}", std::io::Error::last_os_error());
    f
}

#[test]
fn the_release_waits_for_the_admission_lock() {
    for kind in ["flock", "posix"] {
        let env = setup(ADMISSION);
        let held = reservations_here(&env, kind);
        let lock = hold_lock(&env.dir.path().join("admission.lock"), kind == "posix");
        let mut child = driver(&env, LIST, &[], &[]);
        std::thread::sleep(Duration::from_millis(700));
        assert!(
            child.try_wait().unwrap().is_none(),
            "{kind}: the run ended while the lock was held"
        );
        assert!(held.exists(), "{kind}");
        drop(lock);
        let o = finish(&env, child);
        assert_eq!(o.code, 0, "{kind}: {}", o.stderr);
        assert!(!held.exists(), "{kind}: {}", o.stderr);
    }
}

#[test]
fn build_phase_is_skipped_when_admission_refuses_before_any_line() {
    let env = setup(ADMISSION);
    let o = go(&env, LIST, &[("STUB_ADMIT_OK_AT", "0")]);
    assert_eq!(o.code, 78, "{}", o.stderr);
    let r = o.result.unwrap();
    assert_eq!(result_phase(&r, "admission")["status"], "refused");
    let b = result_phase(&r, "build");
    assert_eq!(b["status"], "skipped", "{b}");
    assert!(
        b["note"].as_str().unwrap().contains("admission refused"),
        "{b}"
    );
}

#[test]
fn build_phase_is_refused_when_every_line_that_ran_was_refused() {
    let env = setup("[rebuild]\nstaged = true\n");
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --rebuild\n",
        &[("STUB_STAGED_CODE", "2")],
    );
    assert_eq!(o.code, 78, "{}", o.stderr);
    let r = o.result.unwrap();
    assert_eq!(result_phase(&r, "build")["status"], "refused");
}

#[test]
fn admission_required_without_config_refuses() {
    let env = setup("[admission]\nrequired = true\ndefault_footprint = 500\n");
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
}

#[test]
fn staged_rebuild_refusal_is_translated() {
    let env = setup("[rebuild]\nstaged = true\n");
    install_module(&env, "Foo/1.0-foss-2025a.lua");
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
fn each_staged_line_passes_its_own_run_id() {
    let env = setup("[rebuild]\nstaged = true\n");
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    install_module(&env, "Bar/2.0-foss-2025a.lua");
    let list = "Foo-1.0-foss-2025a.eb --rebuild\nBar-2.0-foss-2025a.eb -f\nFoo-1.0-foss-2025a.eb --force\n";
    let o = go(&env, list, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.result.unwrap()["counts"]["success"], 3);
    assert_eq!(
        calls(&env, "staged.runs"),
        [
            "r1-Foo-1.0-foss-2025a",
            "r1-Bar-2.0-foss-2025a",
            "r1-Foo-1.0-foss-2025a.2"
        ]
    );
    assert!(calls(&env, "eb.calls").is_empty());

    let again = go(&env, list, &[]);
    assert_eq!(again.code, 78, "{}", again.stderr);
    let r = again.result.unwrap();
    assert_eq!(r["counts"]["refused"], 3);
    assert_eq!(r["lines"][0]["translated_from"], 2);
}

#[test]
fn rebuild_line_without_installed_module_warns_and_runs_plain_eb() {
    let env = setup("[rebuild]\nstaged = true\n");
    let o = go(&env, "Foo-1.0-foss-2025a.eb --rebuild\n", &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let warn = format!(
        "WARN [build] cpu: Foo-1.0-foss-2025a: no module file {}/install/modules/all/Foo/1.0-foss-2025a[.lua]; the rebuild runs as plain eb",
        env.dir.path().display()
    );
    assert!(o.stderr.contains(&warn), "{}", o.stderr);
    assert_eq!(calls(&env, "eb.calls").len(), 1);
    assert!(calls(&env, "staged.calls").is_empty());

    install_module(&env, "Foo/1.0-foss-2025a.lua");
    let o = go(&env, "Foo-1.0-foss-2025a.eb --rebuild\n", &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(!o.stderr.contains("no module file"), "{}", o.stderr);
    assert_eq!(calls(&env, "staged.calls").len(), 1);
}

#[test]
fn staged_rebuild_failure_is_a_failure() {
    let env = setup("[rebuild]\nstaged = true\n");
    install_module(&env, "Foo/1.0-foss-2025a.lua");
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

/// A `module` function: EESSI-extend sets EASYBUILD_FILTER_DEPS when it loads.
const MODULE_INIT: &str = r#"module() {
  case "$1" in
    purge) unset EASYBUILD_FILTER_DEPS ;;
    load)
      echo "$2" >> "$STUB_DIR/module.loads"
      if [ "$2" = EESSI-extend ]; then EASYBUILD_FILTER_DEPS=M4,Bison; export EASYBUILD_FILTER_DEPS; fi ;;
  esac
}
"#;

#[test]
fn shard_planner_runs_inside_the_module_wrapper() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    let init = env.dir.path().join("module-init.sh");
    std::fs::write(&init, MODULE_INIT).unwrap();
    let text = std::fs::read_to_string(&env.site).unwrap().replace(
        "init = []",
        &format!(
            "init = [\"EESSI/2026.06\", \"EESSI-extend\"]\nshell_init = \"{}\"",
            init.display()
        ),
    );
    std::fs::write(&env.site, text).unwrap();
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(file_text(&env, "shard.filterdeps").trim(), "M4,Bison");
    assert_eq!(
        calls(&env, "module.loads")[..2],
        ["EESSI/2026.06", "EESSI-extend"]
    );
}

#[test]
fn shard_failure_exits_one() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    let o = go(&env, LIST, &[("STUB_SHARD", "fail")]);
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
}

/// Rewrites the site file's `@ROOT@` to the test directory.
fn rooted(env: &Env) -> String {
    let r = env.dir.path().display().to_string();
    let text = std::fs::read_to_string(&env.site)
        .unwrap()
        .replace("@ROOT@", &r);
    std::fs::write(&env.site, text).unwrap();
    r
}

#[test]
fn shard_phase_leaves_out_installed_modules_dirs_not_created_yet() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\ninstalled_modules_dirs = [\"@ROOT@/eessi/modules/all\", \"@ROOT@/site/modules/all\"]\ninstalled_modules_dirs_from_env = [\"STUB_MODS\"]\n");
    let r = rooted(&env);
    std::fs::create_dir_all(format!("{r}/eessi/modules/all")).unwrap();
    let user = format!("{r}/user/modules/all");
    let o = go(&env, LIST, &[("STUB_MODS", user.as_str())]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let c = calls(&env, "eb-stack.calls");
    let dirs: Vec<&str> = c[0]
        .split(" --installed-modules-dir ")
        .skip(1)
        .map(|d| d.split(' ').next().unwrap())
        .collect();
    assert_eq!(dirs, [format!("{r}/eessi/modules/all")], "{c:?}");
    for d in ["site", "user"] {
        assert!(
            o.stderr.contains(&format!(
                "WARN [shard] cpu: {r}/{d}/modules/all does not exist; no module counts as installed there"
            )),
            "{}",
            o.stderr
        );
    }
    assert_eq!(calls(&env, "eb.calls").len(), 2);
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
fn keep_failed_runs_with_the_basic_output_style() {
    let env = setup("");
    write_exec(
        &env.stubs.join("eb-keep-logs"),
        r#"#!/bin/sh
case "$1" in --version) echo "eb-keep-logs 0.2.0"; exit 0;; esac
echo "$1 ${EASYBUILD_OUTPUT_STYLE:-unset}" >> "$STUB_DIR/keeplogs.style"
"#,
    );
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb\n",
        &[
            ("STUB_EB_FAIL", "Foo-1.0-foss-2025a"),
            ("EASYBUILD_OUTPUT_STYLE", "rich"),
        ],
    );
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert_eq!(calls(&env, "keeplogs.style"), ["failed basic"]);
    assert!(
        file_text(&env, "env.build.Foo-1.0-foss-2025a").contains("EASYBUILD_OUTPUT_STYLE=rich"),
        "the build keeps the style it was given"
    );
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
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "driver did not exit"
        );
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
    let lines = r["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["easyconfig"], "Foo-1.0-foss-2025a.eb");
    assert_eq!(lines[1]["status"], "skipped");
    assert_eq!(lines[1]["easyconfig"], "Bar-2.0-foss-2025a.eb");
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

#[test]
fn dry_runs_and_input_errors_leave_the_result_file_alone() {
    let env = setup("");
    let old = serde_json::json!({"kind": "an earlier run"});
    std::fs::write(env.dir.path().join("result.json"), old.to_string()).unwrap();
    let o = finish(&env, driver(&env, LIST, &[], &["--dry-run"]));
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.result.as_ref(), Some(&old));
    let o = go(&env, "\n", &[]);
    assert_eq!(o.code, 2, "{}", o.stderr);
    assert_eq!(o.result.as_ref(), Some(&old));
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(o.result.unwrap()["kind"], "result");
}

fn staged_or_plain(env: &Env, line: &str) -> (usize, usize) {
    let o = go(env, line, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    (
        calls(env, "staged.calls").len(),
        calls(env, "eb.calls").len(),
    )
}

#[test]
fn staged_rebuild_needs_all_three_conditions() {
    // all three: enabled, whole option, module installed
    for opt in ["--rebuild", "-f", "--force"] {
        let env = setup("[rebuild]\nstaged = true\n");
        install_module(&env, "Foo/1.0-foss-2025a.lua");
        assert_eq!(
            staged_or_plain(&env, &format!("Foo-1.0-foss-2025a.eb {opt}\n")),
            (1, 0),
            "{opt}"
        );
    }
    // a module file without the .lua extension counts
    let env = setup("[rebuild]\nstaged = true\n");
    install_module(&env, "Foo/1.0-foss-2025a");
    assert_eq!(
        staged_or_plain(&env, "Foo-1.0-foss-2025a.eb --rebuild\n"),
        (1, 0)
    );
    // module not installed
    let env = setup("[rebuild]\nstaged = true\n");
    assert_eq!(
        staged_or_plain(&env, "Foo-1.0-foss-2025a.eb --rebuild\n"),
        (0, 1)
    );
    // no rebuild option
    let env = setup("[rebuild]\nstaged = true\n");
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    assert_eq!(
        staged_or_plain(&env, "Foo-1.0-foss-2025a.eb --robot\n"),
        (0, 1)
    );
    // staged = false
    let env = setup("[rebuild]\nstaged = false\n");
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    assert_eq!(
        staged_or_plain(&env, "Foo-1.0-foss-2025a.eb --rebuild\n"),
        (0, 1)
    );
    // no [rebuild] section
    let env = setup("");
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    assert_eq!(
        staged_or_plain(&env, "Foo-1.0-foss-2025a.eb --rebuild\n"),
        (0, 1)
    );
}

#[test]
fn lookalike_options_do_not_trigger_staged_rebuild() {
    for opt in [
        "--force-download",
        "--rebuild-from-scratch",
        "--forcefoo",
        "-fx",
    ] {
        let env = setup("[rebuild]\nstaged = true\n");
        install_module(&env, "Foo/1.0-foss-2025a.lua");
        assert_eq!(
            staged_or_plain(&env, &format!("Foo-1.0-foss-2025a.eb {opt}\n")),
            (0, 1),
            "{opt}"
        );
    }
}

#[test]
fn target_can_override_staged() {
    let env = setup("[rebuild]\nstaged = true\n");
    let text = std::fs::read_to_string(&env.site)
        .unwrap()
        .replace("name = \"cpu\"", "name = \"cpu\"\nstaged = false");
    std::fs::write(&env.site, text).unwrap();
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    assert_eq!(
        staged_or_plain(&env, "Foo-1.0-foss-2025a.eb --rebuild\n"),
        (0, 1)
    );
}

#[test]
fn staged_rebuild_gets_the_target_roots() {
    // defaults: <install_root>/software and <install_root>/modules
    let env = setup("[rebuild]\nstaged = true\n");
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    let o = go(&env, "Foo-1.0-foss-2025a.eb --rebuild\n", &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let root = env.dir.path().display().to_string();
    let c = calls(&env, "staged.calls");
    assert_eq!(c.len(), 1);
    assert!(
        c[0].contains(&format!("--software {root}/install/software ")),
        "{c:?}"
    );
    assert!(
        c[0].contains(&format!("--modules {root}/install/modules ")),
        "{c:?}"
    );

    // overrides: both values reach the tool and the installed check reads
    // the overridden modules root
    let env = setup("[rebuild]\nstaged = true\n");
    let r = env.dir.path().display().to_string();
    let text = std::fs::read_to_string(&env.site).unwrap().replace(
        "name = \"cpu\"",
        &format!(
            "name = \"cpu\"\nsoftware_root = \"{r}/live/sw\"\nmodules_root = \"{r}/live/mod\""
        ),
    );
    std::fs::write(&env.site, text).unwrap();
    // a module under the default root does not count
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    let o = go(&env, "Foo-1.0-foss-2025a.eb --rebuild\n", &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(calls(&env, "staged.calls").is_empty());
    // a module under the overridden root does
    let m = env.dir.path().join("live/mod/all/Foo");
    std::fs::create_dir_all(&m).unwrap();
    std::fs::write(m.join("1.0-foss-2025a.lua"), "-- module\n").unwrap();
    let o = go(&env, "Foo-1.0-foss-2025a.eb --rebuild\n", &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let c = calls(&env, "staged.calls");
    assert_eq!(c.len(), 1);
    assert!(c[0].contains(&format!("--software {r}/live/sw ")), "{c:?}");
    assert!(c[0].contains(&format!("--modules {r}/live/mod ")), "{c:?}");
}

#[test]
fn job_id_must_be_a_positive_integer_without_leading_zero() {
    for bad in ["", "0123", "abc", "0", "12x"] {
        let env = setup(ADMISSION);
        let o = go(&env, LIST, &[("SLURM_JOB_ID", bad)]);
        assert_eq!(o.code, 78, "{bad:?}: {}", o.stderr);
        assert!(calls(&env, "eb.calls").is_empty());
        assert!(
            calls(&env, "eb-stack.calls").is_empty(),
            "no admit before the id is valid"
        );
    }
    let env = setup(ADMISSION);
    let o = go(&env, LIST, &[("SLURM_JOB_ID", "987")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let c = calls(&env, "eb-stack.calls");
    assert!(c[0].contains("--job 987"), "{c:?}");
    assert!(c[0].contains("--footprint 500"), "{c:?}");
}

#[test]
fn logs_go_under_stamp_and_target() {
    let env = setup("");
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let r = o.result.unwrap();
    assert_eq!(r["run_id"], "r1");
    let log = r["lines"][0]["log"].as_str().unwrap();
    assert!(log.contains("/logs/20260101_000000/cpu/"), "{log}");
}

#[test]
fn a_log_stamp_must_name_a_run_directory() {
    // `eb-keep-logs rotate` ages only directories named YYYYMMDD_HHMMSS
    // with a real date, and the stamp is a path component under log_root.
    for bad in [
        "",
        "2026-01-01",
        "20260101",
        "20261301_000000",
        "20260230_000000",
        "20260101_240000",
        "../20260101_000000",
        "20260101_000000/x",
    ] {
        let env = setup("");
        let o = finish(&env, driver(&env, LIST, &[], &["--log-stamp", bad]));
        assert_eq!(o.code, 2, "{bad:?}: {}", o.stderr);
        assert!(o.stderr.contains("--log-stamp"), "{bad:?}: {}", o.stderr);
        assert!(calls(&env, "eb.calls").is_empty(), "{bad:?}");
        assert!(!env.dir.path().join("20260101_000000").exists(), "{bad:?}");
    }
}

#[test]
fn rotate_counts_run_ages_in_utc() {
    let env = setup("[logs.rotate]\ncompress_after_days = 30\n");
    write_exec(
        &env.stubs.join("eb-keep-logs"),
        r#"#!/bin/sh
case "$1" in --version) echo "eb-keep-logs 0.2.0"; exit 0;; esac
echo "$1 ${TZ:-unset}" >> "$STUB_DIR/keeplogs.tz"
"#,
    );
    let o = go(&env, LIST, &[("TZ", "America/Los_Angeles")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "keeplogs.tz"), ["rotate UTC"]);
}

/// Put `frag` straight after the `cpu` target's name.
fn with_cpu_target(env: &Env, frag: &str) {
    let text = std::fs::read_to_string(&env.site)
        .unwrap()
        .replace("name = \"cpu\"", &format!("name = \"cpu\"\n{frag}"));
    std::fs::write(&env.site, text).unwrap();
}

fn file_text(env: &Env, name: &str) -> String {
    std::fs::read_to_string(env.dir.path().join(name)).unwrap_or_default()
}

#[test]
fn selection_filters_lines_and_records_them_apart_from_failures() {
    let env = setup("");
    with_cpu_target(
        &env,
        "[targets.select]\noption = \"--only-targets\"\nvalue = \"cpu\"\nstrip = true\nwhen_absent = \"none\"\n",
    );
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --only-targets=cpu,x --rebuild\nBar-2.0-foss-2025a.eb --only-targets=x\nBaz-3.0-foss-2025a.eb\n",
        &[],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    let c = calls(&env, "eb.calls");
    assert_eq!(c.len(), 1, "{c:?}");
    assert!(c[0].starts_with("Foo-1.0-foss-2025a.eb --rebuild"), "{c:?}");
    assert!(!c[0].contains("--only-targets"), "{c:?}");
    let r = o.result.unwrap();
    assert_eq!(r["counts"]["success"], 1);
    assert!(r["counts"].get("failed").is_none());
    let d = r["deselected"].as_array().unwrap();
    assert_eq!(d.len(), 2);
    assert_eq!(d[0]["easyconfig"], "Bar-2.0-foss-2025a.eb");
    assert_eq!(d[1]["easyconfig"], "Baz-3.0-foss-2025a.eb");
    assert_eq!(r["lines"].as_array().unwrap().len(), 1);
}

#[test]
fn selected_list_feeds_the_shard_planner() {
    let env = setup(&format!(
        "[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n{ADMISSION}"
    ));
    with_cpu_target(
        &env,
        "[targets.select]\noption = \"--only-targets\"\nvalue = \"cpu\"\nstrip = true\nwhen_absent = \"none\"\n",
    );
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --only-targets=cpu,x --rebuild\nBar-2.0-foss-2025a.eb --only-targets=x\nBaz-3.0-foss-2025a.eb --only-targets=cpu\nQux-1.0-foss-2025a.eb\n",
        &[("STUB_SHARD", "fromlist")],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    // the planner read the selected file, stripped, in original order
    assert_eq!(
        file_text(&env, "shard.listtext"),
        "Foo-1.0-foss-2025a.eb --rebuild\nBaz-3.0-foss-2025a.eb\n"
    );
    let path = file_text(&env, "shard.listpath");
    assert!(path.trim().ends_with("/selected-cpu.list"), "{path}");
    assert!(path.contains("/logs/"), "{path}");
    let on_disk = std::fs::read_to_string(path.trim()).unwrap();
    assert_eq!(on_disk, file_text(&env, "shard.listtext"));
    let built: Vec<String> = calls(&env, "eb.calls");
    assert_eq!(built.len(), 2, "{built:?}");
    let r = o.result.unwrap();
    assert_eq!(r["sharded"], true);
    assert_eq!(r["counts"]["success"], 2);
    let d = r["deselected"].as_array().unwrap();
    assert_eq!(d.len(), 2);
    assert_eq!(d[0]["easyconfig"], "Bar-2.0-foss-2025a.eb");
}

#[test]
fn selected_list_keeps_the_option_without_strip() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    with_cpu_target(
        &env,
        "[targets.select]\noption = \"--only-targets\"\nvalue = \"cpu\"\nwhen_absent = \"all\"\n",
    );
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --only-targets=cpu\nBar-2.0-foss-2025a.eb --only-targets=x\nBaz-3.0-foss-2025a.eb\n",
        &[("STUB_SHARD", "fromlist")],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(
        file_text(&env, "shard.listtext"),
        "Foo-1.0-foss-2025a.eb --only-targets=cpu\nBaz-3.0-foss-2025a.eb\n"
    );
}

#[test]
fn no_selection_shards_the_list_as_given() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let path = file_text(&env, "shard.listpath");
    assert!(path.trim().ends_with("/list.txt"), "{path}");
    assert!(!path.contains("selected-"), "{path}");
}

#[test]
fn empty_selection_skips_the_shard_planner() {
    let env = setup(&format!(
        "[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n{ADMISSION}"
    ));
    with_cpu_target(
        &env,
        "[targets.select]\noption = \"--only-targets\"\nvalue = \"cpu\"\nwhen_absent = \"none\"\n",
    );
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
    assert!(calls(&env, "eb-stack.calls")
        .iter()
        .all(|l| !l.starts_with("admit") && !l.starts_with("shard")));
    let r = o.result.unwrap();
    assert_eq!(r["deselected"].as_array().unwrap().len(), 2);
}

const ROUTE_SITE: &str = r#"[targets.route]
kind = "command"
command = "{R}/route --label x"
args = ["--runtime", "{runtime_root}", "--rw", "{shares_rw}", "--tag", "{target}-{phase}"]
runtime_root = "{R}/rt"
shares_rw = ["{phase_dir}"]
receipt_digest_env = "STUB_DIGEST"
{PHASES}
[targets.route.env]
ROUTE_MARK = "{phase_dir}"
"#;

fn route_env(env: &Env, phases: &str) {
    let r = env.stubs.display().to_string();
    let frag = ROUTE_SITE.replace("{R}", &r).replace("{PHASES}", phases);
    with_cpu_target(env, &frag);
    let text = std::fs::read_to_string(&env.site).unwrap()
        + "\n[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n[sources]\n";
    std::fs::write(&env.site, text).unwrap();
}

#[test]
fn route_wraps_cpu_units_with_args_env_and_digest() {
    let env = setup("");
    route_env(&env, "");
    let o = go(
        &env,
        LIST,
        &[("STUB_SHARD", "cpu"), ("STUB_DIGEST", "abc123")],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    // default phases: the CPU unit's build goes through the route; fetches do not
    let rc = calls(&env, "route.calls");
    assert_eq!(rc.len(), 1, "{rc:?}");
    let log = format!(
        "{}/logs/20260101_000000/cpu/build",
        env.dir.path().display()
    );
    assert!(
        rc[0].contains(&format!(
            "--label x --runtime {}/rt --rw {log} --tag cpu-build -- eb Pre-1.0-foss-2025a.eb",
            env.stubs.display()
        )),
        "{rc:?}"
    );
    let e = file_text(&env, "env.build.Pre-1.0-foss-2025a");
    assert!(e.contains("EB_PIPELINE_ROUTE_RECEIPT_DIGEST=abc123"), "{e}");
    assert!(e.contains(&format!("ROUTE_MARK={log}")), "{e}");
    // the directory named by {phase_dir} exists before the command starts
    assert!(std::path::Path::new(&log).is_dir());
    // other units are not routed
    let e = file_text(&env, "env.build.Foo-1.0-foss-2025a");
    assert!(!e.contains("ROUTE_MARK"), "{e}");
}

#[test]
fn route_phases_add_the_sources_phase() {
    let env = setup("");
    route_env(&env, "phases = [\"build\", \"sources\"]");
    let o = go(
        &env,
        LIST,
        &[("STUB_SHARD", "cpu"), ("STUB_DIGEST", "abc123")],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    let rc = calls(&env, "route.calls");
    assert_eq!(rc.len(), 2, "{rc:?}");
    assert!(
        rc[0].contains("--tag cpu-sources -- eb Pre-1.0-foss-2025a.eb --fetch"),
        "{rc:?}"
    );
    assert!(rc[1].contains("--tag cpu-build"), "{rc:?}");
}

#[test]
fn missing_receipt_digest_refuses_before_any_build() {
    for v in [None, Some("")] {
        let env = setup("");
        route_env(&env, "");
        let vars: Vec<(&str, &str)> = [("STUB_SHARD", "cpu")]
            .into_iter()
            .chain(v.map(|x| ("STUB_DIGEST", x)))
            .collect();
        let o = go(&env, LIST, &vars);
        assert_eq!(o.code, 78, "{}", o.stderr);
        assert!(calls(&env, "eb.calls").is_empty());
    }
    // a run without a CPU-role unit needs no digest
    let env = setup("");
    route_env(&env, "");
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(calls(&env, "route.calls").is_empty());
}

#[test]
fn hardware_class_and_fetch_profile_reach_the_commands() {
    let env = setup("");
    with_cpu_target(
        &env,
        "[targets.hardware]\nvendor_id = \"v\"\nmodel = 2\n[targets.cpu_hardware]\nvendor_id = \"w\"\nmodel = 4\n[targets.fetch_profile]\nmirror = \"m\"\n",
    );
    let text = std::fs::read_to_string(&env.site).unwrap()
        + "\n[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n[sources]\n";
    std::fs::write(&env.site, text).unwrap();
    let o = go(&env, LIST, &[("STUB_SHARD", "cpu")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let pre = file_text(&env, "env.fetch.Pre-1.0-foss-2025a");
    assert!(pre.contains("EB_PIPELINE_HW_MODEL=4"), "{pre}");
    assert!(
        pre.contains("EB_PIPELINE_FETCH_PROFILE={\"mirror\":\"m\"}"),
        "{pre}"
    );
    let foo = file_text(&env, "env.build.Foo-1.0-foss-2025a");
    assert!(foo.contains("EB_PIPELINE_HW_MODEL=2"), "{foo}");
    assert!(foo.contains("EB_PIPELINE_HW_VENDOR_ID=v"), "{foo}");
    assert!(!foo.contains("FETCH_PROFILE"), "{foo}");
}

#[test]
fn split_install_vars_reach_the_build() {
    let env = setup("");
    let text = std::fs::read_to_string(&env.site).unwrap().replace(
        "install_root =",
        "export_install_vars = \"split\"\nread_only_installdir = true\nread_only_installdir_value = \"True\"\ninstall_root =",
    );
    std::fs::write(&env.site, text).unwrap();
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let e = file_text(&env, "env.build.Foo-1.0-foss-2025a");
    let r = env.dir.path().display();
    assert!(
        e.contains(&format!(
            "EASYBUILD_INSTALLPATH_SOFTWARE={r}/install/software"
        )),
        "{e}"
    );
    assert!(
        e.contains(&format!(
            "EASYBUILD_INSTALLPATH_MODULES={r}/install/modules"
        )),
        "{e}"
    );
    assert!(!e.contains("EASYBUILD_INSTALLPATH="), "{e}");
    assert!(e.contains("EASYBUILD_READ_ONLY_INSTALLDIR=True"), "{e}");
}

#[test]
fn installed_modules_dirs_come_from_the_environment_at_run_time() {
    let site = "[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\ninstalled_modules_dirs = [\"@ROOT@/m\"]\ninstalled_modules_dirs_from_env = [\"STUB_MODS\", \"STUB_MODS2\"]\n";
    let env = setup(site);
    let r = rooted(&env);
    for d in ["m", "a/x", "a/y"] {
        std::fs::create_dir_all(format!("{r}/{d}")).unwrap();
    }
    let mods = format!("{r}/a/x:{r}/a/y");
    let o = go(
        &env,
        LIST,
        &[("STUB_MODS", mods.as_str()), ("STUB_MODS2", "")],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    let c = calls(&env, "eb-stack.calls");
    assert!(
        c[0].contains(&format!(
            "--installed-modules-dir {r}/m --installed-modules-dir {r}/a/x --installed-modules-dir {r}/a/y"
        )),
        "{c:?}"
    );
    assert!(!c[0].contains("STUB_MODS"), "{c:?}");
    // an unset variable refuses before any work
    let env = setup(site);
    rooted(&env);
    let o = go(&env, LIST, &[("STUB_MODS", "/a/x")]);
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(calls(&env, "eb-stack.calls").is_empty());
    assert!(calls(&env, "eb.calls").is_empty());
}
