//! `run` against stub tool binaries written into a temporary directory.

mod common;

use common::CommandSyncExt;
use serde_json::Value;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
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
  oom)
    plan=""; node=""
    while [ $# -gt 0 ]; do
      [ "$1" = --easystack ] && plan="$2"
      [ "$1" = --node ] && node="$2"
      shift
    done
    cp "$plan" "$D/oom-plan.yml"
    echo "$node" > "$D/oom.node"
    [ -n "${STUB_OOM_EXIT:-}" ] && exit "$STUB_OOM_EXIT"
    [ -n "${STUB_OOM_OUTPUT:-}" ] && { printf '%s\n' "$STUB_OOM_OUTPUT"; exit 0; }
    echo "oom=${STUB_OOM_COUNT:-0} fits=0 builds=2 nodes=1"
    exit 0;;
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
    echo "{\"admitted\":false,\"attempt\":$n}"
    [ -n "${STUB_ADMIT_TRAILER:-}" ] && printf '%s\n' "$STUB_ADMIT_TRAILER"
    exit 78;;
  shard)
    echo "${EASYBUILD_FILTER_DEPS:-}" > "$D/shard.filterdeps"
    case "${STUB_SHARD:-ok}" in unsharded) exit 3;; fail) exit 1;; esac
    [ -n "${STUB_SHARD_EXIT:-}" ] && exit "$STUB_SHARD_EXIT"
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
if [ -n "${STUB_EB_PIPELINE:-}" ]; then
  (cd / && "$STUB_EB_PIPELINE" targets) > "$D/targets.$stem" 2>&1 || exit 1
fi
if [ -n "${STUB_EB_DESCENDANT:-}" ]; then
  "$D/descendant" &
  pid=$!
  echo "$pid" > "$D/descendant.pid"
  echo started > "$D/started"
  wait "$pid"
  exit 0
fi
if [ -n "${STUB_EB_DELAY:-}" ]; then sleep "$STUB_EB_DELAY"; fi
if [ -n "${STUB_EB_SLEEP:-}" ]; then
  echo started > "$D/started"
  trap 'echo term > "$D/termed"; kill $pid 2>/dev/null; exit 0' TERM
  sleep 30 &
  pid=$!
  wait $pid
  exit 0
fi
case ",${STUB_EB_FAIL:-}," in *",$stem,"*) echo "ERROR: $stem failed"; exit 1;; esac
# STUB_EB_INSTALLS names the module file a build installs, under
# install/modules/all; STUB_EB_CODE is the build's exit code.
if [ "$kind" = build ] && [ -n "${STUB_EB_INSTALLS:-}" ]; then
  mkdir -p "$(dirname "$D/install/modules/all/$STUB_EB_INSTALLS")"
  echo '-- module' > "$D/install/modules/all/$STUB_EB_INSTALLS"
fi
[ "$kind" = build ] && exit "${STUB_EB_CODE:-0}"
exit 0
"#;

const ROUTE: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
echo "$*" >> "$D/route.calls"
while [ $# -gt 0 ] && [ "$1" != -- ]; do shift; done
shift
exec "$@"
"#;

/// `--version` and `--help` of a stub eb-keep-logs, which records the TZ of
/// each `--help` in keeplogs.help. STUB_KEEP_LOGS_HELP `lock`, the default,
/// lists `--lock-timeout`; `none` does not, and refuses the option with
/// exit 2, as eb-keep-logs 0.2.1 does; `fail` exits 1.
const KEEP_LOGS_PROBES: &str = r#"case "$1" in
  --version) echo "eb-keep-logs 0.2.0"; exit 0;;
  -h|--help)
    echo "${TZ:-unset}" >> "$STUB_DIR/keeplogs.help"
    case "${STUB_KEEP_LOGS_HELP:-lock}" in
      lock) echo "  eb-keep-logs rotate ROOT --compress-after DAYS [--delete-after DAYS] [--lock-timeout SECONDS]";;
      none) echo "  eb-keep-logs rotate ROOT --compress-after DAYS [--delete-after DAYS]";;
      *) exit 1;;
    esac
    exit 0;;
esac
if [ "${STUB_KEEP_LOGS_HELP:-lock}" = none ]; then
  for a in "$@"; do
    case "$a" in --lock-timeout*) echo "eb-keep-logs: unknown option $a" >&2; exit 2;; esac
  done
fi
"#;

/// A stub eb-keep-logs that answers `--version` and `--help`, then runs `body`.
fn keep_logs(body: &str) -> String {
    format!("#!/bin/sh\n{KEEP_LOGS_PROBES}{body}")
}

const KEEP_LOGS: &str = r#"D="${STUB_DIR:?}"
echo "$*" >> "$D/keeplogs.calls"
exit 0
"#;

const STAGED: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
case "$1" in --version) echo "eb-staged-rebuild 1.4.0"; exit 0;; esac
echo "$*" >> "$D/staged.calls"
id=""; root=""; sw=""; wait=""; prev=""
for a in "$@"; do
  case "$prev" in
    --run-id) id="$a";; --state-root) root="$a";; --software) sw="$a";; --lock-timeout) wait="$a";;
  esac
  prev="$a"
done
# With STUB_STAGED_LOCKS set, take the state root's lock as the tool does,
# waiting --lock-timeout. STUB_STAGED_LOCK names another lock instead, such
# as a ledger's, which the tool takes without waiting.
if [ -n "${STUB_STAGED_LOCKS:-}" ]; then
  root="${root:-${EB_STAGED_REBUILD_STATE_ROOT:-$(dirname "$sw")/.eb-staged-rebuild}}"
  lock="${STUB_STAGED_LOCK:-$root/.lock}"
  [ -n "${STUB_STAGED_LOCK:-}" ] && wait=0
  mkdir -p "$(dirname "$lock")"
  exec 9>>"$lock"
  case "$wait" in
    "") flock -w 30 9 || { echo "stub: no --lock-timeout, gave up after 30 s" >&2; exit 99; } ;;
    0) flock -n 9 ;;
    *) flock -w "$wait" 9 ;;
  esac || { echo "eb-staged-rebuild: another eb-staged-rebuild holds the lock at $lock" >&2; exit 2; }
fi
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
    common::write_exec(path, text);
}

fn setup(extra_site: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let stubs = root.join("stubs");
    std::fs::create_dir(&stubs).unwrap();
    let keep_logs = keep_logs(KEEP_LOGS);
    for (n, t) in [
        ("eb-stack", EB_STACK),
        ("eb", EB),
        ("route", ROUTE),
        ("eb-keep-logs", &keep_logs),
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

fn use_eb_stack_from_path(env: &Env) {
    let configured = format!(
        "[site.tools.eb_stack]\nbin = \"{}/eb-stack\"\nmin_version = \"0.7.0\"\n",
        env.stubs.display()
    );
    let text = std::fs::read_to_string(&env.site)
        .unwrap()
        .replace(&configured, "");
    std::fs::write(&env.site, text).unwrap();
}

struct Out {
    code: i32,
    stdout: Vec<u8>,
    stderr: String,
    result: Option<Value>,
}

fn driver(env: &Env, list: &str, vars: &[(&str, &str)], extra: &[&str]) -> std::process::Child {
    driver_named(env, "list.txt", list, vars, extra)
}

/// `driver` with the list in the file `name`.
fn driver_named(
    env: &Env,
    name: &str,
    list: &str,
    vars: &[(&str, &str)],
    extra: &[&str],
) -> std::process::Child {
    let root = env.dir.path();
    let list_path = root.join(name);
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
    if vars.iter().any(|(name, _)| *name == "DRIVER_CLOSED_STDOUT") {
        c.stdout(common::closed_stdout());
    }
    if let Some((_, value)) = vars.iter().find(|(name, _)| *name == "DRIVER_UMASK") {
        let mode = u32::from_str_radix(value, 8).unwrap();
        // SAFETY: `umask` is async-signal-safe and the closure allocates nothing.
        unsafe {
            c.pre_exec(move || {
                libc::umask(mode);
                Ok(())
            });
        }
    }
    for (k, v) in vars {
        if *k != "DRIVER_UMASK" && *k != "DRIVER_CLOSED_STDOUT" {
            c.env(k, v);
        }
    }
    let _g = SPAWN.lock().unwrap();
    c.sync_spawn()
}

fn finish(env: &Env, child: std::process::Child) -> Out {
    let o = child.wait_with_output().unwrap();
    let result = std::fs::read_to_string(env.dir.path().join("result.json"))
        .ok()
        .map(|t| serde_json::from_str(&t).unwrap());
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: o.stdout,
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
fn successful_run_default_output_bytes_remain_unchanged() {
    let env = setup("");
    let o = go(&env, "Foo-1.0-foss-2025a.eb\n", &[]);
    assert_eq!(o.code, 0);
    assert_eq!(o.stdout, b"");
    let root = env.dir.path().display();
    let stderr = format!(
        concat!(
            "INFO [licences] cpu: not configured\n",
            "INFO [shard] cpu: not configured\n",
            "INFO [sources] cpu: not configured\n",
            "INFO [admission] cpu: not configured\n",
            "INFO [checks] cpu: not configured\n",
            "INFO [caches] cpu: not configured\n",
            "INFO [logs] cpu: not configured\n",
            "INFO [build] cpu: Foo-1.0-foss-2025a: start\n",
            "INFO [build] cpu: Foo-1.0-foss-2025a: ok\n",
            "INFO [summary] cpu: summary.json and summary.html written in ",
            "{root}/logs/20260101_000000\n",
        ),
        root = root,
    );
    assert_eq!(o.stderr.as_bytes(), stderr.as_bytes());
}

#[test]
fn metrics_file_is_opt_in_and_atomically_replaced_after_a_run() {
    let env = setup("");
    let metrics = env.dir.path().join("pipeline.prom");
    assert!(!metrics.exists());

    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(!metrics.exists());

    std::fs::write(&metrics, "stale\n").unwrap();
    std::fs::set_permissions(&metrics, std::fs::Permissions::from_mode(0o644)).unwrap();
    let metrics_arg = metrics.to_str().unwrap();
    let child = driver(
        &env,
        LIST,
        &[("DRIVER_UMASK", "077")],
        &["--metrics-file", metrics_arg],
    );
    let o = finish(&env, child);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let text = std::fs::read_to_string(&metrics).unwrap();
    assert!(text.contains("eb_pipeline_last_run_result{"), "{text}");
    assert!(text.contains("eb_pipeline_items_built{"), "{text}");
    assert!(text.contains("} 2\n"), "{text}");
    assert!(!text.contains("stale"), "{text}");
    assert_eq!(
        std::fs::metadata(&metrics).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert!(
        !std::fs::read_dir(env.dir.path()).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp"))
    );
}

#[test]
fn a_result_write_failure_does_not_leave_requested_metrics_stale() {
    let env = setup("");
    std::fs::create_dir(env.dir.path().join("result.json")).unwrap();
    let metrics = env.dir.path().join("pipeline.prom");
    std::fs::write(&metrics, "stale\n").unwrap();
    let metrics_arg = metrics.to_str().unwrap();

    let child = driver(&env, LIST, &[], &["--metrics-file", metrics_arg]);
    let o = finish(&env, child);

    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(o.stderr.contains("cannot write"), "{}", o.stderr);
    assert!(o.stderr.contains("result.json"), "{}", o.stderr);
    let text = std::fs::read_to_string(metrics).unwrap();
    assert!(text.contains("eb_pipeline_last_run_result{"), "{text}");
    assert!(!text.contains("stale"), "{text}");
}

#[test]
fn failures_of_both_requested_outputs_are_reported_in_option_order() {
    let env = setup("");
    std::fs::create_dir(env.dir.path().join("result.json")).unwrap();
    let metrics = env.dir.path().join("pipeline.prom");
    std::fs::create_dir(&metrics).unwrap();
    let metrics_arg = metrics.to_str().unwrap();

    let child = driver(&env, LIST, &[], &["--metrics-file", metrics_arg]);
    let o = finish(&env, child);

    assert_eq!(o.code, 1, "{}", o.stderr);
    let result_at = o.stderr.find("result.json").unwrap();
    let metrics_at = o.stderr.find("cannot write metrics").unwrap();
    assert!(result_at < metrics_at, "{}", o.stderr);
}

fn output_alias(original: &Path, kind: &str) -> PathBuf {
    match kind {
        "lexical" => original.to_path_buf(),
        "canonical" => {
            let alias = original.with_extension("symlink");
            std::os::unix::fs::symlink(original, &alias).unwrap();
            alias
        }
        "hard-link" => {
            let alias = original.with_extension("hardlink");
            std::fs::hard_link(original, &alias).unwrap();
            alias
        }
        _ => unreachable!(),
    }
}

fn assert_metrics_alias_refused(
    env: &Env,
    metrics: &Path,
    resume: Option<&Path>,
    json_option: &str,
) {
    let mut extra = vec!["--metrics-file", metrics.to_str().unwrap()];
    if let Some(resume) = resume {
        extra.extend(["--resume-result", resume.to_str().unwrap()]);
    }
    let child = driver(env, LIST, &[], &extra);
    let o = child.wait_with_output().unwrap();

    assert_eq!(o.status.code(), Some(2), "{:?}", o.status);
    let stderr = String::from_utf8(o.stderr).unwrap();
    assert!(
        stderr.contains(&format!(
            "{json_option} and --metrics-file must name different files"
        )),
        "{stderr}"
    );
    assert!(calls(env, "eb.calls").is_empty());
}

#[test]
fn metrics_cannot_alias_result_lexically_canonically_or_by_hard_link() {
    for kind in ["lexical", "canonical", "hard-link"] {
        let env = setup("");
        let result = env.dir.path().join("result.json");
        std::fs::write(&result, "keep result\n").unwrap();
        let metrics = output_alias(&result, kind);

        assert_metrics_alias_refused(&env, &metrics, None, "--result");

        assert_eq!(
            std::fs::read_to_string(&result).unwrap(),
            "keep result\n",
            "{kind}"
        );
    }
}

#[test]
fn metrics_cannot_alias_resume_result_lexically_canonically_or_by_hard_link() {
    for kind in ["lexical", "canonical", "hard-link"] {
        let env = setup("");
        let resume = env.dir.path().join("prior.json");
        std::fs::write(&resume, "keep prior\n").unwrap();
        let metrics = output_alias(&resume, kind);

        assert_metrics_alias_refused(&env, &metrics, Some(&resume), "--resume-result");

        assert_eq!(
            std::fs::read_to_string(&resume).unwrap(),
            "keep prior\n",
            "{kind}"
        );
        assert!(!env.dir.path().join("result.json").exists(), "{kind}");
    }
}

#[test]
fn metrics_cannot_share_one_file_with_both_json_roles() {
    let env = setup("");
    let shared = env.dir.path().join("result.json");
    std::fs::write(&shared, "keep shared\n").unwrap();

    assert_metrics_alias_refused(&env, &shared, Some(&shared), "--result");

    assert_eq!(std::fs::read_to_string(&shared).unwrap(), "keep shared\n");
}

#[test]
fn plan_out_cannot_alias_the_result_lexically_canonically_or_by_hard_link() {
    for kind in ["lexical", "canonical", "hard-link"] {
        let env = setup("");
        let result = env.dir.path().join("result.json");
        std::fs::write(&result, "keep result\n").unwrap();
        let plan = output_alias(&result, kind);
        let o = driver(&env, LIST, &[], &["--plan-out", plan.to_str().unwrap()])
            .wait_with_output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{kind}");
        let stderr = String::from_utf8(o.stderr).unwrap();
        assert!(
            stderr.contains("--result and --plan-out must name different files"),
            "{stderr}"
        );
        assert_eq!(
            std::fs::read_to_string(&result).unwrap(),
            "keep result\n",
            "{kind}"
        );
        assert!(calls(&env, "eb.calls").is_empty());
    }
}

#[test]
fn result_phases_follow_the_pipeline_order() {
    let env = setup("");
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let r = o.result.unwrap();
    let names: Vec<&str> = r["phases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    let in_order: Vec<&str> = eb_pipeline::plan::PHASES
        .into_iter()
        .filter(|n| names.contains(n))
        .collect();
    assert_eq!(names, in_order);
    assert_eq!(names.first(), Some(&"preflight"), "{names:?}");
    assert!(!r["not_configured"].as_array().unwrap().is_empty());
}

#[test]
fn the_result_records_how_long_each_line_phase_and_run_took() {
    let env = setup("");
    let o = go(&env, LIST, &[("STUB_EB_DELAY", "0.3")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let r = o.result.unwrap();
    for l in r["lines"].as_array().unwrap() {
        assert!(l["duration_seconds"].as_f64().unwrap() >= 0.3, "{l}");
    }
    let build = result_phase(&r, "build")["duration_seconds"]
        .as_f64()
        .unwrap();
    assert!(build >= 0.6, "{r}");
    assert!(
        result_phase(&r, "preflight")
            .get("duration_seconds")
            .is_none(),
        "a phase that ran no command has no duration: {r}"
    );
    assert!(r["duration_seconds"].as_f64().unwrap() >= build, "{r}");
}

#[test]
fn a_leading_dash_easystack_entry_uses_its_robot_path() {
    let env = setup("[sources]\n");
    let root = env.dir.path().join("robot");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("-Robot-1.0.eb"), "# robot-only easyconfig\n").unwrap();
    write_exec(
        &env.stubs.join("eb"),
        r#"#!/bin/sh
D="${STUB_DIR:?}"
operand=
robot=
separator=
for arg
do
  if [ -n "$separator" ]; then
    [ -z "$operand" ] || exit 91
    operand=$arg
  else
    case "$arg" in
      --) separator=1;;
      --robot=*) robot=${arg#--robot=};;
    esac
  fi
done
[ "$operand" = "-Robot-1.0.eb" ] || exit 92
[ -n "$robot" ] && [ -f "$robot/$operand" ] || exit 93
[ ! -e "$operand" ] || exit 94
printf '%s\n' "$*" >> "$D/eb.calls"
"#,
    );
    let easystack = format!(
        "easyconfigs:\n  - '-Robot-1.0.eb':\n      options:\n        robot: '{}'\n",
        root.display()
    );
    let plan_path = env.dir.path().join("plan.json");
    let child = driver_named(
        &env,
        "edge.yml",
        &easystack,
        &[],
        &["--plan-out", plan_path.to_str().unwrap()],
    );
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let called = calls(&env, "eb.calls");
    assert_eq!(called.len(), 2, "{called:?}");
    assert_eq!(
        called[0],
        format!(
            "--fetch --robot --robot={} -- -Robot-1.0.eb",
            root.display()
        )
    );
    assert!(
        called[1].starts_with(&format!("--robot={} --tmp-logdir=", root.display())),
        "{called:?}"
    );
    assert!(called[1].ends_with(" -- -Robot-1.0.eb"), "{called:?}");

    let result: Value =
        serde_json::from_slice(&std::fs::read(env.dir.path().join("result.json")).unwrap())
            .unwrap();
    assert_eq!(result["lines"][0]["easyconfig"], "-Robot-1.0.eb");
    let plan: Value = serde_json::from_slice(&std::fs::read(plan_path).unwrap()).unwrap();
    assert_eq!(
        plan["phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|phase| phase["name"] == "build")
            .unwrap()["commands"][0]["argv"]
            .as_array()
            .unwrap()
            .last()
            .unwrap(),
        "-Robot-1.0.eb"
    );
}

#[test]
fn easystack_special_names_stay_single_plan_arguments() {
    let env = setup("");
    let root = env.dir.path();
    let plan_path = root.join("plan with λ.json");
    let child = driver_named(
        &env,
        "stack with λ.yml",
        "easyconfigs:\n  - space name.eb\n  - λ-name.eb\n",
        &[],
        &["--dry-run", "--plan-out", plan_path.to_str().unwrap()],
    );
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        output.stderr,
        b"INFO [run] cpu: dry run: nothing executed\n"
    );
    let bytes = std::fs::read(&plan_path).unwrap();
    assert!(bytes.windows("λ".len()).any(|part| part == "λ".as_bytes()));
    let plan: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        plan["list"],
        root.join("stack with λ.yml").display().to_string()
    );
    let commands = plan["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["name"] == "build")
        .unwrap()["commands"]
        .as_array()
        .unwrap();
    let operands: Vec<&str> = commands
        .iter()
        .filter(|command| {
            command["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("build:"))
        })
        .map(|command| command["argv"][1].as_str().unwrap())
        .collect();
    assert_eq!(operands, ["space name.eb", "λ-name.eb"]);
    let stdout = output.stdout;
    assert!(stdout
        .windows(b"eb 'space name.eb'".len())
        .any(|part| part == b"eb 'space name.eb'"));
    assert!(stdout
        .windows("eb 'λ-name.eb'".len())
        .any(|part| part == "eb 'λ-name.eb'".as_bytes()));
    assert!(calls(&env, "eb.calls").is_empty());
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
fn eessi_commands_do_not_inherit_another_install_mode_or_cpu_override() {
    let env = setup("\n[modules.eessi]\nsite_install = true\n");
    let init = format!(
        r#"
[ -z "${{EESSI_CVMFS_INSTALL+x}}" ] || exit 17
[ -z "${{EESSI_SOFTWARE_SUBDIR_OVERRIDE+x}}" ] || exit 18
[ "${{EESSI_SITE_INSTALL:-}}" = 1 ] || exit 19
{MODULE_FN}"#
    );
    with_modules(
        &env,
        &init,
        &["EESSI/2026.06", "EESSI-extend/2026.06-easybuild"],
    );

    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb\n",
        &[
            ("EESSI_CVMFS_INSTALL", "1"),
            ("EESSI_SOFTWARE_SUBDIR_OVERRIDE", "x86_64/amd/zen4"),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "eb.calls").len(), 1);
}

#[test]
fn eessi_commands_do_not_inherit_default_or_custom_target_overrides() {
    let env = setup(
        "\n[modules.eessi]\nsite_install = true\ncpu_override_var = \"SITE_CPU_OVERRIDE\"\naccel_override_var = \"SITE_ACCEL_OVERRIDE\"\n",
    );
    let init = format!(
        r#"
[ -z "${{EESSI_SOFTWARE_SUBDIR_OVERRIDE+x}}" ] || exit 17
[ -z "${{EESSI_ACCELERATOR_TARGET_OVERRIDE+x}}" ] || exit 18
[ -z "${{SITE_CPU_OVERRIDE+x}}" ] || exit 19
[ -z "${{SITE_ACCEL_OVERRIDE+x}}" ] || exit 20
{MODULE_FN}"#
    );
    with_modules(&env, &init, &["EESSI/2026.06"]);

    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb\n",
        &[
            ("EESSI_SOFTWARE_SUBDIR_OVERRIDE", "x86_64/amd/zen4"),
            ("EESSI_ACCELERATOR_TARGET_OVERRIDE", "accel/nvidia/cc90"),
            ("SITE_CPU_OVERRIDE", "x86_64/amd/zen4"),
            ("SITE_ACCEL_OVERRIDE", "accel/nvidia/cc90"),
        ],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "eb.calls").len(), 1);
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
fn admission_retries_keep_the_last_valid_json_report() {
    let env = setup(ADMISSION);
    let o = go(
        &env,
        LIST,
        &[
            ("STUB_ADMIT_OK_AT", "0"),
            ("STUB_ADMIT_TRAILER", "{not json}"),
        ],
    );
    assert_eq!(o.code, 78, "{}", o.stderr);
    let admission = &o.result.unwrap()["admission"][0];
    assert_eq!(admission["attempts"], 3);
    assert_eq!(admission["last_json"], r#"{"admitted":false,"attempt":3}"#);
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
fn a_held_state_root_lock_refuses_the_line_when_lock_timeout_runs_out() {
    let env = setup("[rebuild]\nstaged = true\nlock_timeout = 1\n");
    let r = env.dir.path().display().to_string();
    let text = std::fs::read_to_string(&env.site).unwrap().replace(
        "staged = true",
        &format!("staged = true\nstate_root = \"{r}/state\""),
    );
    std::fs::write(&env.site, text).unwrap();
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    std::fs::create_dir(env.dir.path().join("state")).unwrap();
    let _held = hold_lock(&env.dir.path().join("state/.lock"), false);
    let t0 = Instant::now();
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --rebuild\n",
        &[("STUB_STAGED_LOCKS", "1")],
    );
    let took = t0.elapsed();
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(
        took >= Duration::from_secs(1) && took < Duration::from_secs(20),
        "{took:?}"
    );
    let c = calls(&env, "staged.calls");
    assert!(
        c[0].contains(&format!(" --state-root {r}/state --lock-timeout 1 ")),
        "{c:?}"
    );
    let why = format!(
        "another eb-staged-rebuild holds the lock at {r}/state/.lock, and rebuild.lock_timeout (1 s) ran out"
    );
    let l = &o.result.unwrap()["lines"][0];
    assert_eq!(l["status"], "refused");
    assert_eq!(l["exit_code"], 78);
    assert_eq!(l["translated_from"], 2);
    assert_eq!(
        l["note"],
        format!("eb-staged-rebuild exit 2: {why}; translated to refused (78)")
    );
    assert!(
        o.stderr.contains(&format!(
            "cpu: Foo-1.0-foss-2025a: refused (exit 2): {why}\n"
        )),
        "{}",
        o.stderr
    );
}

#[test]
fn lock_timeout_zero_refuses_at_once_at_the_tool_default_state_root() {
    let env = setup("[rebuild]\nstaged = true\nlock_timeout = 0\n");
    let r = env.dir.path().display().to_string();
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    // eb-staged-rebuild's default: EB_STAGED_REBUILD_STATE_ROOT, else
    // `.eb-staged-rebuild` next to the software tree.
    let from_env = format!("{r}/env-state");
    for (root, vars) in [
        (format!("{r}/install/.eb-staged-rebuild"), vec![]),
        (
            from_env.clone(),
            vec![("EB_STAGED_REBUILD_STATE_ROOT", from_env.as_str())],
        ),
    ] {
        std::fs::create_dir_all(&root).unwrap();
        let _held = hold_lock(&Path::new(&root).join(".lock"), false);
        let _ = std::fs::remove_file(env.dir.path().join("staged.calls"));
        let mut vars = vars;
        vars.push(("STUB_STAGED_LOCKS", "1"));
        let t0 = Instant::now();
        let o = go(&env, "Foo-1.0-foss-2025a.eb --rebuild\n", &vars);
        assert!(t0.elapsed() < Duration::from_secs(20), "{root}");
        assert_eq!(o.code, 78, "{root}: {}", o.stderr);
        assert!(calls(&env, "staged.calls")[0].contains(" --lock-timeout 0 "));
        assert_eq!(
            o.result.unwrap()["lines"][0]["note"],
            format!(
                "eb-staged-rebuild exit 2: another eb-staged-rebuild holds the lock at {root}/.lock, and rebuild.lock_timeout (0 s) ran out; translated to refused (78)"
            )
        );
    }
}

#[test]
fn a_held_lock_other_than_the_state_roots_is_named_alone() {
    let env = setup("[rebuild]\nstaged = true\n");
    let r = env.dir.path().display().to_string();
    let text = std::fs::read_to_string(&env.site).unwrap().replace(
        "staged = true",
        &format!("staged = true\nstate_root = \"{r}/state\""),
    );
    std::fs::write(&env.site, text).unwrap();
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    std::fs::create_dir(env.dir.path().join("ledger")).unwrap();
    let ledger = format!("{r}/ledger/.lock");
    let _held = hold_lock(Path::new(&ledger), false);
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --rebuild\n",
        &[("STUB_STAGED_LOCKS", "1"), ("STUB_STAGED_LOCK", &ledger)],
    );
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(calls(&env, "staged.calls")[0]
        .contains(&format!(" --state-root {r}/state --lock-timeout 600 ")));
    assert_eq!(
        o.result.unwrap()["lines"][0]["note"],
        format!(
            "eb-staged-rebuild exit 2: another eb-staged-rebuild holds the lock at {ledger}; translated to refused (78)"
        )
    );
}

/// Without `rebuild.state_root`, a module that the line loads can set
/// EB_STAGED_REBUILD_STATE_ROOT in the line's shell, where eb-pipeline does
/// not see it.
#[test]
fn a_held_lock_at_a_state_root_a_module_sets_names_lock_timeout() {
    let env = setup("[rebuild]\nstaged = true\nlock_timeout = 0\n");
    let r = env.dir.path().display().to_string();
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    let root = format!("{r}/module-state");
    let init = format!(
        "module() {{\n    echo \"$*\" >> \"$STUB_DIR/module.calls\"\n    \
         [ \"$*\" != \"load StateRoot/1.0\" ] || export EB_STAGED_REBUILD_STATE_ROOT=\"{root}\"\n}}\n"
    );
    with_modules(&env, &init, &["StateRoot/1.0"]);
    std::fs::create_dir(&root).unwrap();
    let lock = format!("{root}/.lock");
    let _held = hold_lock(Path::new(&lock), false);
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --rebuild\n",
        &[("STUB_STAGED_LOCKS", "1")],
    );
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(calls(&env, "module.calls").contains(&"load StateRoot/1.0".to_string()));
    let c = calls(&env, "staged.calls");
    assert!(
        !c[0].contains("--state-root") && c[0].contains(" --lock-timeout 0 "),
        "{c:?}"
    );
    let why = format!(
        "another eb-staged-rebuild holds the lock at {lock}, and rebuild.lock_timeout (0 s) ran out if that is the state root's lock: rebuild.state_root is unset, and eb-pipeline cannot see a state root that a module sets in EB_STAGED_REBUILD_STATE_ROOT"
    );
    let l = &o.result.unwrap()["lines"][0];
    assert_eq!(l["status"], "refused");
    assert_eq!(
        l["note"],
        format!("eb-staged-rebuild exit 2: {why}; translated to refused (78)")
    );
    assert!(
        o.stderr.contains(&format!(
            "cpu: Foo-1.0-foss-2025a: refused (exit 2): {why}\n"
        )),
        "{}",
        o.stderr
    );
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
fn unsharded_note_names_the_configured_planner_code() {
    let env =
        setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\nplanner_exit_unsharded = 5\n");
    let o = go(&env, LIST, &[("STUB_SHARD_EXIT", "5")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let r = o.result.unwrap();
    let shard = r["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "shard")
        .unwrap();
    assert_eq!(shard["status"], "unsharded");
    assert_eq!(shard["exit_code"], 5);
    let note = shard["note"].as_str().unwrap();
    assert!(note.contains("exit 5"), "{note}");
    assert!(!note.contains("exit 3"), "{note}");
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
fn sigterm_during_source_prefetch_records_every_line() {
    let env = setup("[sources]\n");
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
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(143));
    let result: Value =
        serde_json::from_str(&std::fs::read_to_string(env.dir.path().join("result.json")).unwrap())
            .unwrap();
    let lines = result["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["easyconfig"], "Foo-1.0-foss-2025a.eb");
    assert_eq!(lines[0]["phase"], "sources");
    assert_eq!(lines[0]["status"], "failed");
    assert_eq!(lines[0]["exit_code"], 143);
    assert_eq!(lines[1]["easyconfig"], "Bar-2.0-foss-2025a.eb");
    assert_eq!(lines[1]["status"], "skipped");
}

#[test]
fn sigterm_reaches_descendants_of_the_running_command() {
    let env = setup("");
    write_exec(
        &env.dir.path().join("descendant"),
        r#"#!/bin/sh
trap 'echo term > "$STUB_DIR/descendant.termed"; exit 0' TERM
echo ready > "$STUB_DIR/descendant.ready"
while :; do sleep 1; done
"#,
    );
    let mut child = driver(&env, LIST, &[("STUB_EB_DESCENDANT", "1")], &[]);
    let ready = env.dir.path().join("descendant.ready");
    let t0 = Instant::now();
    while !ready.exists() {
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "descendant never started"
        );
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

    let terminated = env.dir.path().join("descendant.termed");
    let t0 = Instant::now();
    let descendant_stopped = loop {
        if terminated.exists() {
            break true;
        }
        if t0.elapsed() >= Duration::from_secs(2) {
            break false;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    if !descendant_stopped {
        let pid: libc::pid_t = file_text(&env, "descendant.pid").trim().parse().unwrap();
        // SAFETY: this cleans up the descendant pid recorded by the test stub.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
    }
    assert!(
        descendant_stopped,
        "SIGTERM did not reach the running command's descendant"
    );
}

#[test]
fn dry_run_default_output_remains_byte_for_byte_unchanged() {
    let env = setup("");
    let c = driver(&env, LIST, &[], &["--dry-run"]);
    let o = c.wait_with_output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(o.stdout, historical_dry_run(&env).as_bytes());
    assert_eq!(o.stderr, b"INFO [run] cpu: dry run: nothing executed\n");
    assert!(calls(&env, "eb.calls").is_empty());
}

#[test]
fn dry_run_succeeds_when_its_output_consumer_closes() {
    let env = setup("");
    let c = driver(&env, LIST, &[("DRIVER_CLOSED_STDOUT", "1")], &["--dry-run"]);
    let o = c.wait_with_output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(o.stderr, b"INFO [run] cpu: dry run: nothing executed\n");
}

fn historical_dry_run(env: &Env) -> String {
    let root = env.dir.path().display();
    let keep_logs = env.stubs.join("eb-keep-logs");
    let keep_logs = keep_logs.display();
    format!(
        concat!(
            "[preflight] {keep_logs} --version\n",
            "# [licences] not configured\n",
            "# [shard] not configured\n",
            "# [sources] not configured\n",
            "# [admission] not configured\n",
            "[build] eb Foo-1.0-foss-2025a.eb --tmp-logdir={root}/logs/20260101_000000/cpu/easybuild-logs/Foo-1.0-foss-2025a\n",
            "[build] EASYBUILD_OUTPUT_STYLE=basic {keep_logs} failed '<exit-status>' Foo-1.0-foss-2025a {root}/logs/20260101_000000/cpu {root}/logs/20260101_000000/cpu/easybuild-logs/Foo-1.0-foss-2025a\n",
            "[build] eb Bar-2.0-foss-2025a.eb --tmp-logdir={root}/logs/20260101_000000/cpu/easybuild-logs/Bar-2.0-foss-2025a\n",
            "[build] EASYBUILD_OUTPUT_STYLE=basic {keep_logs} failed '<exit-status>' Bar-2.0-foss-2025a {root}/logs/20260101_000000/cpu {root}/logs/20260101_000000/cpu/easybuild-logs/Bar-2.0-foss-2025a\n",
            "# [checks] not configured\n",
            "# [caches] not configured\n",
            "# [logs] not configured\n",
        ),
        keep_logs = keep_logs,
        root = root,
    )
}

#[test]
fn plan_out_records_the_run_plan_without_changing_dry_run_output() {
    let env = setup("");
    let plan = env.dir.path().join("run-plan.json");
    std::fs::write(&plan, b"complete old plan\n").unwrap();
    let old_inode = std::fs::metadata(&plan).unwrap().ino();
    let o = driver(
        &env,
        LIST,
        &[],
        &["--dry-run", "--plan-out", plan.to_str().unwrap()],
    )
    .wait_with_output()
    .unwrap();
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(o.stdout, historical_dry_run(&env).as_bytes());
    assert_eq!(o.stderr, b"INFO [run] cpu: dry run: nothing executed\n");
    assert!(calls(&env, "eb.calls").is_empty());
    assert_ne!(std::fs::metadata(&plan).unwrap().ino(), old_inode);
    let bytes = std::fs::read(&plan).unwrap();
    assert!(bytes.ends_with(b"\n"));
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["kind"], "plan");
    assert_eq!(value["target"], "cpu");
    assert_eq!(value["run_id"], "r1");
    assert_eq!(value["lines"], 2);
    assert_eq!(
        value["log_dir"],
        env.dir
            .path()
            .join("logs/20260101_000000/cpu")
            .display()
            .to_string()
    );
}

#[test]
fn plan_out_failure_starts_no_phase_command() {
    let env = setup("");
    let plan = env.dir.path().join("absent/run-plan.json");
    let o = finish(
        &env,
        driver(&env, LIST, &[], &["--plan-out", plan.to_str().unwrap()]),
    );
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(o.stderr.contains("cannot write plan"), "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
}

#[test]
fn a_real_run_writes_its_plan() {
    let env = setup("");
    let plan = env.dir.path().join("run-plan.json");
    let o = finish(
        &env,
        driver(&env, LIST, &[], &["--plan-out", plan.to_str().unwrap()]),
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "eb.calls").len(), 2);
    let value: Value = serde_json::from_slice(&std::fs::read(plan).unwrap()).unwrap();
    assert_eq!(value["kind"], "plan");
    assert_eq!(value["target"], "cpu");
    assert_eq!(value["run_id"], "r1");
}

#[test]
fn oom_preflight_skips_every_build_when_eb_stack_reports_a_risk() {
    let env = setup("");
    use_eb_stack_from_path(&env);
    let o = finish(
        &env,
        driver(
            &env,
            LIST,
            &[("STUB_OOM_COUNT", "2")],
            &["--oom-preflight", "--node", "16:32768:x86_64/amd/zen4"],
        ),
    );
    assert_eq!(o.code, 78, "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
    assert_eq!(
        file_text(&env, "oom.node").trim(),
        "16:32768:x86_64/amd/zen4"
    );
    let plan = file_text(&env, "oom-plan.yml");
    assert!(plan.contains("Foo-1.0-foss-2025a.eb"), "{plan}");
    assert!(plan.contains("Bar-2.0-foss-2025a.eb"), "{plan}");
    let r = o.result.unwrap();
    assert_eq!(r["counts"]["skipped"], 2);
    assert_eq!(result_phase(&r, "preflight")["status"], "refused");
    assert_eq!(result_phase(&r, "build")["status"], "skipped");
    assert!(
        o.stderr.contains(
            "OOM preflight found 2 risk(s) for node 16:32768:x86_64/amd/zen4; build skipped"
        ),
        "{}",
        o.stderr
    );
}

#[test]
fn oom_preflight_continues_only_from_eb_stacks_zero_count() {
    let env = setup("");
    use_eb_stack_from_path(&env);
    let o = finish(
        &env,
        driver(&env, LIST, &[], &["--oom-preflight", "--node", "8:65536"]),
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "eb.calls").len(), 2);
    assert_eq!(
        calls(&env, "eb-stack.calls")[0],
        "oom --easystack ".to_string()
            + &env
                .dir
                .path()
                .join("logs/20260101_000000/cpu/oom-preflight.yml")
                .display()
                .to_string()
            + " --node 8:65536"
    );
}

#[test]
fn malformed_oom_output_fails_closed_before_a_build() {
    let env = setup("");
    use_eb_stack_from_path(&env);
    let o = finish(
        &env,
        driver(
            &env,
            LIST,
            &[("STUB_OOM_OUTPUT", "fits=2 builds=2 nodes=1")],
            &["--oom-preflight", "--node", "8:65536"],
        ),
    );
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty());
    assert!(o.stderr.contains("printed no `oom=N` line"), "{}", o.stderr);
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
fn a_staged_line_records_that_it_ran_through_eb_staged_rebuild() {
    let list = "Foo-1.0-foss-2025a.eb --rebuild\nBar-2.0-foss-2025a.eb\n";
    for (code, status) in [("0", "success"), ("1", "failed")] {
        // a fresh site: the staged stub refuses a run id it has already seen
        let env = setup("[rebuild]\nstaged = true\n");
        install_module(&env, "Foo/1.0-foss-2025a.lua");
        let o = go(&env, list, &[("STUB_STAGED_CODE", code)]);
        let r = o.result.unwrap();
        let staged = &r["lines"][0];
        assert_eq!(staged["status"], status, "{}", o.stderr);
        assert!(
            staged["note"]
                .as_str()
                .is_some_and(|n| n.contains("eb-staged-rebuild")),
            "{staged}"
        );
        assert_eq!(r["lines"][1]["status"], "success");
        assert!(r["lines"][1].get("note").is_none(), "{}", r["lines"][1]);
    }
}

#[test]
fn a_rebuild_that_installs_its_module_ran_as_plain_eb() {
    // No module file before the build, so plain eb runs; the module file it
    // installs does not make the line a staged rebuild after the fact.
    for (code, status, exit) in [("0", "success", 0), ("2", "failed", 1)] {
        let env = setup("[rebuild]\nstaged = true\n");
        let o = go(
            &env,
            "Foo-1.0-foss-2025a.eb --rebuild\n",
            &[
                ("STUB_EB_INSTALLS", "Foo/1.0-foss-2025a.lua"),
                ("STUB_EB_CODE", code),
            ],
        );
        assert_eq!(o.code, exit, "{}", o.stderr);
        let r = o.result.unwrap();
        let line = &r["lines"][0];
        assert_eq!(line["status"], status, "{line}");
        assert_eq!(line["exit_code"], code.parse::<i64>().unwrap(), "{line}");
        assert!(line.get("note").is_none(), "{line}");
        assert!(calls(&env, "staged.calls").is_empty());
        assert_eq!(calls(&env, "eb.calls").len(), 1);
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
fn staged_rebuild_takes_the_ledger_config_of_its_target() {
    let rebuild = "[rebuild]\nstaged = true\nledger_config = \"/etc/eb/ledger.json\"\n";
    let env = setup(rebuild);
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    assert_eq!(
        staged_or_plain(&env, "Foo-1.0-foss-2025a.eb --rebuild\n"),
        (1, 0)
    );
    let c = calls(&env, "staged.calls");
    assert!(
        c[0].contains(" --ledger-config /etc/eb/ledger.json -- "),
        "{c:?}"
    );
    // the target's own configuration wins
    let env = setup(rebuild);
    let text = std::fs::read_to_string(&env.site).unwrap().replace(
        "name = \"cpu\"",
        "name = \"cpu\"\nledger_config = \"/etc/eb/ledger-cpu.json\"",
    );
    std::fs::write(&env.site, text).unwrap();
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    assert_eq!(
        staged_or_plain(&env, "Foo-1.0-foss-2025a.eb --rebuild\n"),
        (1, 0)
    );
    let c = calls(&env, "staged.calls");
    assert!(
        c[0].contains(" --ledger-config /etc/eb/ledger-cpu.json -- "),
        "{c:?}"
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
fn target_paths_reach_eb_and_eb_staged_rebuild() {
    let env = setup("[rebuild]\nstaged = true\n");
    let r = env.dir.path().display().to_string();
    let text = std::fs::read_to_string(&env.site)
        .unwrap()
        .replace(
            &format!("build_path = \"{r}/build\""),
            &format!(
                "build_path = \"{r}/build/{{target}}\"\nsource_paths = [\"{r}/sources/{{target}}\", \"{r}/sources\"]"
            ),
        )
        .replace(
            "staged = true",
            &format!("staged = true\nstate_root = \"{r}/state/{{target}}\""),
        );
    std::fs::write(&env.site, text).unwrap();
    install_module(&env, "Foo/1.0-foss-2025a.lua");
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb --rebuild\nBar-2.0-foss-2025a.eb\n",
        &[],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    let eb_env = file_text(&env, "env.build.Bar-2.0-foss-2025a");
    assert!(
        eb_env.contains(&format!("EASYBUILD_BUILDPATH={r}/build/cpu\n")),
        "{eb_env}"
    );
    assert!(
        eb_env.contains(&format!(
            "EASYBUILD_SOURCEPATH={r}/sources/cpu:{r}/sources\n"
        )),
        "{eb_env}"
    );
    let c = calls(&env, "staged.calls");
    assert!(
        c[0].contains(&format!(" --state-root {r}/state/cpu ")),
        "{c:?}"
    );
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
        &keep_logs("echo \"$1 ${TZ:-unset}\" >> \"$STUB_DIR/keeplogs.tz\"\n"),
    );
    let o = go(&env, LIST, &[("TZ", "America/Los_Angeles")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "keeplogs.tz"), ["rotate UTC"]);
    assert_eq!(calls(&env, "keeplogs.help"), ["UTC"]);
}

const ROTATE: &str = "[logs.rotate]\ncompress_after_days = 3\n";

fn rotations(env: &Env) -> Vec<String> {
    calls(env, "keeplogs.calls")
        .into_iter()
        .filter(|c| c.starts_with("rotate "))
        .collect()
}

/// eb-keep-logs passes over at most the run that it writes into itself, and
/// a run that began on an earlier day in UTC is old enough to compress.
#[test]
fn a_run_skips_its_rotation_while_another_run_holds_the_log_root() {
    let env = setup(ROTATE);
    let r = env.dir.path().display().to_string();
    let lock = format!("{r}/logs/.eb-pipeline-rotate.lock");
    std::fs::create_dir(env.dir.path().join("logs")).unwrap();
    let live = std::fs::File::create(&lock).unwrap();
    live.try_lock_shared().unwrap();
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(rotations(&env).is_empty());
    let why = format!(
        "another run under {r}/logs holds {lock}, live or rotating; the rotation is left to a run that ends alone"
    );
    let logs = result_phase(o.result.as_ref().unwrap(), "logs");
    assert_eq!(logs["status"], "skipped");
    assert_eq!(logs["note"], why);
    assert!(
        o.stderr
            .contains(&format!("INFO [logs] cpu: rotation skipped: {why}\n")),
        "{}",
        o.stderr
    );
    assert!(calls(&env, "keeplogs.help").is_empty());

    drop(live);
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let logs = result_phase(o.result.as_ref().unwrap(), "logs");
    assert_eq!(logs["status"], "ok");
    assert!(logs.get("note").is_none(), "{logs}");
    assert_eq!(
        rotations(&env),
        [format!(
            "rotate {r}/logs --compress-after 3 --lock-timeout 0"
        )]
    );
    assert_eq!(calls(&env, "keeplogs.help"), ["UTC"]);
}

#[test]
fn a_run_waits_to_start_while_another_run_rotates() {
    let env = setup(ROTATE);
    let r = env.dir.path().display().to_string();
    let lock = format!("{r}/logs/.eb-pipeline-rotate.lock");
    std::fs::create_dir(env.dir.path().join("logs")).unwrap();
    let rotating = hold_lock(Path::new(&lock), false);
    let mut child = driver(&env, LIST, &[], &[]);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(child.try_wait().unwrap().is_none(), "the run ended");
    assert!(calls(&env, "eb.calls").is_empty(), "a line started");
    drop(rotating);
    let o = finish(&env, child);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "eb.calls").len(), 2);
    assert_eq!(result_phase(&o.result.unwrap(), "logs")["status"], "ok");
    assert!(
        o.stderr.contains(&format!(
            "INFO [logs] cpu: waiting: another run rotates and holds {lock}\n"
        )),
        "{}",
        o.stderr
    );
}

#[test]
fn a_signal_stops_a_run_that_waits_for_a_rotation() {
    let env = setup(ROTATE);
    std::fs::create_dir(env.dir.path().join("logs")).unwrap();
    let lock = env.dir.path().join("logs/.eb-pipeline-rotate.lock");
    let _rotating = hold_lock(&lock, false);
    let lock = std::fs::canonicalize(&lock).unwrap();
    let mut child = driver(&env, LIST, &[], &[]);
    let fds = format!("/proc/{}/fd", child.id());
    let opened = || {
        std::fs::read_dir(&fds)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| std::fs::read_link(e.path()).is_ok_and(|l| l == lock))
    };
    let t0 = Instant::now();
    while !opened() {
        assert!(child.try_wait().unwrap().is_none(), "the run ended");
        assert!(
            t0.elapsed() < Duration::from_secs(20),
            "the lock never opened"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // SAFETY: the pid is the driver child this test started.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let o = finish(&env, child);
    assert_eq!(o.code, 143, "{}", o.stderr);
    assert!(calls(&env, "eb.calls").is_empty(), "a line started");
    let r = o.result.unwrap();
    assert_eq!(r["interrupted_by"], "SIGTERM");
    assert_eq!(result_phase(&r, "logs")["status"], "interrupted");
}

/// eb-keep-logs rewrites a `.licensed-logs` from what it read before, so
/// two rotations at once lose each other's renames.
#[test]
fn two_targets_that_end_together_rotate_once() {
    let env = setup(ROTATE);
    let text = std::fs::read_to_string(&env.site).unwrap().replace(
        "[[targets]]\nname = \"cpu\"",
        "[[targets]]\nname = \"cpu\"\n[[targets]]\nname = \"gpu\"",
    );
    std::fs::write(&env.site, text).unwrap();
    // Each target's first line waits until both targets build, so both runs
    // are live before either ends.
    write_exec(
        &env.stubs.join("eb"),
        r#"#!/bin/sh
D="${STUB_DIR:?}"
echo "$*" >> "$D/eb.calls"
echo "$EB_PIPELINE_TARGET" >> "$D/eb.targets"
i=0
while [ "$(sort -u "$D/eb.targets" | wc -l)" -lt 2 ] && [ $i -lt 200 ]; do sleep 0.1; i=$((i + 1)); done
exit 0
"#,
    );
    write_exec(
        &env.stubs.join("eb-keep-logs"),
        &keep_logs(
            r#"D="${STUB_DIR:?}"
[ -e "$D/rotating" ] && echo "$EB_PIPELINE_TARGET" >> "$D/keeplogs.overlap"
: > "$D/rotating"
echo "$*" >> "$D/keeplogs.calls"
sleep 0.5
rm -f "$D/rotating"
"#,
        ),
    );
    let root = env.dir.path();
    std::fs::write(root.join("list.txt"), LIST).unwrap();
    let path = format!(
        "{}:{}",
        env.stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let kids: Vec<_> = ["cpu", "gpu"]
        .into_iter()
        .map(|t| {
            let mut c = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
            c.args(["run", "--site"])
                .arg(&env.site)
                .args(["--target", t, "--build-type", "production"])
                .args([
                    "--run-id",
                    &format!("r-{t}"),
                    "--log-stamp",
                    "20260101_000000",
                ])
                .arg("--list")
                .arg(root.join("list.txt"))
                .arg("--result")
                .arg(root.join(format!("result-{t}.json")))
                .env("PATH", &path)
                .env("STUB_DIR", root)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let _g = SPAWN.lock().unwrap();
            (t, c.sync_spawn())
        })
        .collect();
    let mut status = Vec::new();
    for (t, kid) in kids {
        let o = kid.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(0), "{t}: {stderr}");
        let text = std::fs::read_to_string(root.join(format!("result-{t}.json"))).unwrap();
        let r: Value = serde_json::from_str(&text).unwrap();
        status.push(
            result_phase(&r, "logs")["status"]
                .as_str()
                .unwrap()
                .to_string(),
        );
    }
    status.sort();
    assert_eq!(status, ["ok", "skipped"]);
    assert_eq!(rotations(&env).len(), 1, "{:?}", rotations(&env));
    assert!(calls(&env, "keeplogs.overlap").is_empty());
}

/// An eb-keep-logs `rotate` that exits with `STUB_ROTATE_EXIT`, saying what
/// eb-keep-logs says when another rotate holds its lock.
const ROTATE_EXITS: &str = r#"echo "$*" >> "$STUB_DIR/keeplogs.calls"
[ "$1" = rotate ] || exit 0
echo "eb-keep-logs: another rotate holds the lock $2/.eb-keep-logs-rotate.lock; gave up after 0 s (--lock-timeout)" >&2
exit "${STUB_ROTATE_EXIT:?}"
"#;

/// An eb-keep-logs `rotate` that takes `ROOT/.eb-keep-logs-rotate.lock` as
/// eb-keep-logs does: it waits `--lock-timeout` seconds for it, 600 without
/// the option, and then exits 75.
const ROTATE_LOCKS: &str = r#"echo "$*" >> "$STUB_DIR/keeplogs.calls"
[ "$1" = rotate ] || exit 0
root="$2"; wait=600; prev=""
for a in "$@"; do [ "$prev" = --lock-timeout ] && wait="$a"; prev="$a"; done
exec 9>>"$root/.eb-keep-logs-rotate.lock"
flock -w "$wait" -E 75 9 || {
  echo "eb-keep-logs: another rotate holds the lock $root/.eb-keep-logs-rotate.lock; gave up after $wait s (--lock-timeout)" >&2
  exit 75
}
"#;

/// eb-keep-logs exits 75 when another rotate of the tree holds its lock past
/// `--lock-timeout`, and has then changed nothing.
#[test]
fn a_rotate_that_waits_out_the_lock_of_another_is_skipped() {
    let env = setup(ROTATE);
    write_exec(&env.stubs.join("eb-keep-logs"), &keep_logs(ROTATE_EXITS));
    let r = env.dir.path().display().to_string();
    let o = go(&env, LIST, &[("STUB_ROTATE_EXIT", "75")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let why = format!(
        "eb-keep-logs rotate exited 75: another rotate holds {r}/logs/.eb-keep-logs-rotate.lock, and this one changed nothing; the rotation is left to it"
    );
    let res = o.result.unwrap();
    let logs = result_phase(&res, "logs");
    assert_eq!(logs["status"], "skipped");
    assert_eq!(logs["exit_code"], 75);
    assert_eq!(logs["note"], why);
    assert_eq!(res["exit_code"], 0);
    assert!(
        o.stderr
            .contains(&format!("INFO [logs] cpu: rotation skipped: {why}\n")),
        "{}",
        o.stderr
    );
    assert!(!o.stderr.contains("ERROR"), "{}", o.stderr);
}

#[test]
fn a_rotate_that_exits_otherwise_fails_the_run() {
    for code in [1, 2] {
        let env = setup(ROTATE);
        write_exec(&env.stubs.join("eb-keep-logs"), &keep_logs(ROTATE_EXITS));
        let o = go(&env, LIST, &[("STUB_ROTATE_EXIT", &code.to_string())]);
        assert_eq!(o.code, 1, "{code}: {}", o.stderr);
        let msg = format!("eb-keep-logs rotate exited {code}");
        let logs = result_phase(o.result.as_ref().unwrap(), "logs").clone();
        assert_eq!(logs["status"], "failed", "{code}");
        assert_eq!(logs["exit_code"], code, "{code}");
        assert_eq!(logs["note"], msg, "{code}");
        assert!(
            o.stderr.contains(&format!("ERROR [logs] cpu: {msg}\n")),
            "{code}: {}",
            o.stderr
        );
    }
}

/// The run holds .eb-pipeline-rotate.lock alone while its rotate runs, and
/// every run under the log root that starts meanwhile waits for it, so the
/// rotate gives up at once on another rotate's lock.
#[test]
fn a_run_leaves_its_rotation_at_once_to_a_rotate_that_holds_the_tree() {
    let env = setup(ROTATE);
    write_exec(&env.stubs.join("eb-keep-logs"), &keep_logs(ROTATE_LOCKS));
    std::fs::create_dir(env.dir.path().join("logs")).unwrap();
    let r = env.dir.path().display().to_string();
    let rotating = hold_lock(
        &env.dir.path().join("logs/.eb-keep-logs-rotate.lock"),
        false,
    );
    let mut child = driver(&env, LIST, &[], &[]);
    let t0 = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if t0.elapsed() > Duration::from_secs(30) {
            child.kill().unwrap();
            panic!("the run still waits for {r}/logs/.eb-keep-logs-rotate.lock");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let o = finish(&env, child);
    drop(rotating);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(
        rotations(&env),
        [format!(
            "rotate {r}/logs --compress-after 3 --lock-timeout 0"
        )]
    );
    let logs = result_phase(o.result.as_ref().unwrap(), "logs");
    assert_eq!(logs["status"], "skipped");
    assert_eq!(logs["exit_code"], 75);
}

/// eb-keep-logs 0.2.1 refuses `--lock-timeout` with exit 2, and the builds
/// that take the option print the same version: `--help` tells them apart.
#[test]
fn an_eb_keep_logs_whose_help_lacks_lock_timeout_rotates_without_it() {
    let env = setup(ROTATE);
    let r = env.dir.path().display().to_string();
    let s = env.stubs.display().to_string();
    let o = go(&env, LIST, &[("STUB_KEEP_LOGS_HELP", "none")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(
        rotations(&env),
        [format!("rotate {r}/logs --compress-after 3")]
    );
    let why = format!(
        "no --lock-timeout for eb-keep-logs rotate: `{s}/eb-keep-logs --help` does not list it, as in eb-keep-logs 0.2.1, which takes no lock of its own"
    );
    let logs = result_phase(o.result.as_ref().unwrap(), "logs");
    assert_eq!(logs["status"], "ok");
    assert_eq!(logs["note"], why);
    assert!(
        o.stderr.contains(&format!("INFO [logs] cpu: {why}\n")),
        "{}",
        o.stderr
    );
    assert_eq!(calls(&env, "keeplogs.help"), ["UTC"]);
}

#[test]
fn a_help_that_fails_leaves_lock_timeout_out() {
    let env = setup(ROTATE);
    let r = env.dir.path().display().to_string();
    let s = env.stubs.display().to_string();
    let o = go(&env, LIST, &[("STUB_KEEP_LOGS_HELP", "fail")]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(
        rotations(&env),
        [format!("rotate {r}/logs --compress-after 3")]
    );
    let logs = result_phase(o.result.as_ref().unwrap(), "logs");
    assert_eq!(logs["status"], "ok");
    assert_eq!(
        logs["note"],
        format!("no --lock-timeout for eb-keep-logs rotate: `{s}/eb-keep-logs --help` exited 1")
    );
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
fn selected_list_atomically_replaces_prior_materialization() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    with_cpu_target(
        &env,
        "[targets.select]\noption = \"--only-targets\"\nvalue = \"cpu\"\nstrip = true\nwhen_absent = \"none\"\n",
    );
    let selected = env
        .dir
        .path()
        .join("logs/20260101_000000/cpu/selected-cpu.list");
    std::fs::create_dir_all(selected.parent().unwrap()).unwrap();
    std::fs::write(&selected, "stale partial bytes\n").unwrap();
    let old_inode = std::fs::metadata(&selected).unwrap().ino();

    let output = go(
        &env,
        "Foo-1.0-foss-2025a.eb --only-targets=cpu --rebuild\n",
        &[("STUB_SHARD", "fromlist")],
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    assert_eq!(
        std::fs::read_to_string(&selected).unwrap(),
        "Foo-1.0-foss-2025a.eb --rebuild\n"
    );
    assert_ne!(std::fs::metadata(&selected).unwrap().ino(), old_inode);
}

#[test]
fn run_can_reuse_a_saved_shard_plan() {
    let env = setup(&format!(
        "[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n{ADMISSION}"
    ));
    let plan = env.dir.path().join("saved-shards");
    std::fs::create_dir(&plan).unwrap();
    std::fs::write(
        plan.join("plan.json"),
        r#"{"prelude":{"file":null},"shards":[{"name":"saved-01","file":"saved-01.list","inode_footprint":111},{"name":"saved-02","file":"saved-02.list","inode_footprint":222}]}"#,
    )
    .unwrap();
    std::fs::write(plan.join("saved-01.list"), "Foo-1.0-foss-2025a.eb\n").unwrap();
    std::fs::write(plan.join("saved-02.list"), "Bar-2.0-foss-2025a.eb\n").unwrap();

    let o = finish(
        &env,
        driver(&env, LIST, &[], &["--shard-plan", plan.to_str().unwrap()]),
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    let stack = calls(&env, "eb-stack.calls");
    assert!(
        stack.iter().all(|line| !line.starts_with("shard ")),
        "{stack:?}"
    );
    assert_eq!(
        stack
            .iter()
            .filter(|line| line.starts_with("admit "))
            .count(),
        2
    );
    assert_eq!(
        calls(&env, "eb.calls"),
        [
            "Foo-1.0-foss-2025a.eb --tmp-logdir=".to_string()
                + &env
                    .dir
                    .path()
                    .join("logs/20260101_000000/cpu/easybuild-logs/Foo-1.0-foss-2025a")
                    .display()
                    .to_string(),
            "Bar-2.0-foss-2025a.eb --tmp-logdir=".to_string()
                + &env
                    .dir
                    .path()
                    .join("logs/20260101_000000/cpu/easybuild-logs/Bar-2.0-foss-2025a")
                    .display()
                    .to_string(),
        ]
    );
    assert_eq!(o.result.unwrap()["sharded"], true);
}

#[test]
fn dry_run_and_plan_out_use_the_saved_shard_units() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    let saved = env.dir.path().join("saved-shards");
    std::fs::create_dir(&saved).unwrap();
    std::fs::write(
        saved.join("plan.json"),
        r#"{"shards":[{"name":"saved-01","file":"saved-01.list","inode_footprint":111}]}"#,
    )
    .unwrap();
    std::fs::write(saved.join("saved-01.list"), "Foo-1.0-foss-2025a.eb\n").unwrap();
    let plan_out = env.dir.path().join("run-plan.json");

    let output = finish(
        &env,
        driver(
            &env,
            LIST,
            &[],
            &[
                "--shard-plan",
                saved.to_str().unwrap(),
                "--plan-out",
                plan_out.to_str().unwrap(),
                "--dry-run",
            ],
        ),
    );
    assert_eq!(output.code, 0, "{}", output.stderr);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Foo-1.0-foss-2025a.eb"), "{stdout}");
    assert!(!stdout.contains("Bar-2.0-foss-2025a.eb"), "{stdout}");
    assert!(!stdout.contains("stack shard"), "{stdout}");
    let plan: Value = serde_json::from_str(&std::fs::read_to_string(plan_out).unwrap()).unwrap();
    let build = plan["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|phase| phase["name"] == "build")
        .unwrap();
    let builds = build["commands"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|command| {
            command["name"]
                .as_str()
                .is_some_and(|name| name.starts_with("build:"))
        })
        .count();
    assert_eq!(builds, 1, "{build}");
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
fn an_easystack_reaches_the_shard_planner_as_build_list_lines() {
    let env = setup("[shards]\ncount = 2\neasyconfigs = [\"/opt/ec\"]\n");
    let easystack = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/eb-stack/stack-easystack.yml"),
    )
    .unwrap();
    let c = driver_named(
        &env,
        "stack.yml",
        &easystack,
        &[("STUB_SHARD", "fromlist")],
        &[],
    );
    let o = finish(&env, c);
    assert_eq!(o.code, 0, "{}", o.stderr);
    let lines = "Autoconf-2.72-GCCcore-15.2.0.eb\nM4-1.4.20-GCCcore-15.2.0.eb\nPerl-5.42.0-GCCcore-15.2.0.eb\npigz-2.8-GCCcore-15.2.0.eb --rebuild\n";
    assert_eq!(file_text(&env, "shard.listtext"), lines);
    let path = file_text(&env, "shard.listpath");
    assert!(
        path.trim()
            .ends_with("/logs/20260101_000000/cpu/easystack-cpu.list"),
        "{path}"
    );
    let mut built: Vec<String> = calls(&env, "eb.calls")
        .iter()
        .map(|c| c.split(" --tmp-logdir=").next().unwrap().to_string())
        .collect();
    built.sort();
    assert_eq!(
        built,
        [
            "Autoconf-2.72-GCCcore-15.2.0.eb",
            "M4-1.4.20-GCCcore-15.2.0.eb",
            "Perl-5.42.0-GCCcore-15.2.0.eb",
            "pigz-2.8-GCCcore-15.2.0.eb --rebuild",
        ]
    );
}

/// A stand-in for srun that drops its options and runs the command as the
/// job STUB_JOB_ID, 4242 unless set, of STUB_JOB_USER, alice unless set.
const SRUN: &str = r#"#!/bin/sh
while [ $# -gt 0 ]; do case $1 in -*) shift ;; *) break ;; esac; done
SLURM_JOB_ID=${STUB_JOB_ID:-4242} SLURM_JOB_USER=${STUB_JOB_USER-alice} exec "$@"
"#;

/// Make the site of `env` start each command through the stand-in srun,
/// with `tmp` and `build_path` in the job's directory under `local/`.
fn with_job_dirs(env: &Env) -> PathBuf {
    let srun = env.stubs.join("srun");
    write_exec(&srun, SRUN);
    let r = env.dir.path().display();
    let text = std::fs::read_to_string(&env.site)
        .unwrap()
        .replace(
            "kind = \"local\"",
            &format!("kind = \"slurm\"\nlaunch = \"{}\"", srun.display()),
        )
        .replace(
            &format!("tmp = \"{r}/tmp\""),
            &format!("job_dir = \"{r}/local/{{user}}.{{job}}\"\ntmp = \"{{job_dir}}/tmp\""),
        )
        .replace(
            &format!("build_path = \"{r}/build\""),
            "build_path = \"{job_dir}/build\"",
        );
    std::fs::write(&env.site, text).unwrap();
    env.dir.path().join("local")
}

#[test]
fn each_job_builds_in_its_own_directory() {
    let env = setup("");
    let local = with_job_dirs(&env);
    let user = Command::new("id").arg("-un").sync_output().stdout;
    let user = String::from_utf8(user).unwrap();
    // The job's user, else the user the job runs as.
    for (stem, job_user, owner) in [
        ("Foo-1.0-foss-2025a", None, "alice"),
        ("Baz-3.0-foss-2025a", Some(""), user.trim()),
    ] {
        let dir = local.join(format!("{owner}.4242"));
        std::fs::create_dir_all(&dir).unwrap();
        let vars: Vec<(&str, &str)> = job_user.map(|u| ("STUB_JOB_USER", u)).into_iter().collect();
        let o = go(&env, &format!("{stem}.eb\n"), &vars);
        assert_eq!(o.code, 0, "{}", o.stderr);
        let e = file_text(&env, &format!("env.build.{stem}"));
        let d = dir.display();
        for want in [
            format!("EASYBUILD_BUILDPATH={d}/build"),
            format!("EASYBUILD_TMPDIR={d}/tmp"),
            format!("EB_PIPELINE_JOB_DIR={d}"),
        ] {
            assert!(e.lines().any(|l| l == want), "{want} not in\n{e}");
        }
    }

    // A job without its directory starts no command.
    let o = go(&env, "Bar-2.0-foss-2025a.eb\n", &[("STUB_JOB_ID", "4343")]);
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(file_text(&env, "env.build.Bar-2.0-foss-2025a").is_empty());
    let log = file_text(
        &env,
        "logs/20260101_000000/cpu/Bar-2.0-foss-2025a.stdout.log",
    );
    let want = format!(
        "eb-pipeline: job 4343 has no directory {}/alice.4343 that it can write to, which filesystems.job_dir names",
        local.display()
    );
    assert!(log.lines().any(|l| l == want), "{want} not in\n{log}");
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

/// A command of a run that runs eb-pipeline without `--site`, from another
/// directory, reads the site file of the run: a relative `--site` reaches it
/// as an absolute EB_PIPELINE_SITE, and the site name as EB_PIPELINE_SITE_NAME.
#[test]
fn a_command_that_runs_eb_pipeline_reads_the_site_file_of_the_run() {
    let env = setup("");
    let root = env.dir.path();
    std::fs::write(root.join("list.txt"), "Foo-1.0-foss-2025a.eb\n").unwrap();
    let path = format!(
        "{}:{}",
        env.stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut c = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
    c.current_dir(root)
        .args(["run", "--site", "site.toml", "--list", "list.txt"])
        .args(["--target", "cpu", "--build-type", "production"])
        .args(["--log-stamp", "20260101_000000"])
        .env("PATH", path)
        .env("STUB_DIR", root)
        .env("STUB_EB_PIPELINE", env!("CARGO_BIN_EXE_eb-pipeline"))
        .env_remove("EB_PIPELINE_SITE")
        .env_remove("EB_PIPELINE_SITE_NAME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = {
        let _g = SPAWN.lock().unwrap();
        c.sync_spawn()
    };
    let o = child.wait_with_output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let e = file_text(&env, "env.build.Foo-1.0-foss-2025a");
    let site_file = root.canonicalize().unwrap().join("site.toml");
    assert!(
        e.lines()
            .any(|l| l == format!("EB_PIPELINE_SITE={}", site_file.display())),
        "{e}"
    );
    assert!(e.lines().any(|l| l == "EB_PIPELINE_SITE_NAME=t"), "{e}");
    assert_eq!(file_text(&env, "targets.Foo-1.0-foss-2025a"), "cpu\n");
}

/// The module wrapper gives each name of `modules.init` to `module load` as
/// written, in bash and in sh: a name with a `*` is not matched against the
/// files it would name.
#[test]
fn the_module_wrapper_loads_each_name_as_written() {
    // Each shell `modules.shell` may name, of those installed.
    let path = std::env::var_os("PATH").unwrap_or_default();
    let shells: Vec<&str> = eb_pipeline::resolve::MODULE_SHELLS
        .iter()
        .copied()
        .filter(|s| std::env::split_paths(&path).any(|d| d.join(s).is_file()))
        .collect();
    assert!(
        shells.contains(&"bash") && shells.contains(&"sh"),
        "{shells:?}"
    );
    for shell in shells {
        let env = setup("");
        let root = env.dir.path();
        let r = root.display().to_string();
        std::fs::create_dir(root.join("mods")).unwrap();
        std::fs::write(root.join("mods/Example-1.0.lua"), "").unwrap();
        std::fs::write(
            root.join("module.sh"),
            "module() { echo \"$*\" >> \"$STUB_DIR/module.calls\"; }\n",
        )
        .unwrap();
        let text = std::fs::read_to_string(&env.site).unwrap().replace(
            "[modules]\ninit = []",
            &format!(
                "[modules]\ninit = [\"{r}/mods/Example*\", \"Other/2.0\"]\nshell = \"{shell}\"\nshell_init = \"{r}/module.sh\""
            ),
        );
        std::fs::write(&env.site, text).unwrap();
        let o = go(&env, "Foo-1.0-foss-2025a.eb\n", &[]);
        assert_eq!(o.code, 0, "{shell}: {}", o.stderr);
        assert_eq!(
            calls(&env, "module.calls"),
            [
                "purge".to_string(),
                format!("load {r}/mods/Example*"),
                "load Other/2.0".to_string()
            ],
            "{shell}"
        );
        assert_eq!(calls(&env, "eb.calls").len(), 1, "{shell}");
    }
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

const CHECK: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
echo "$*" >> "$D/check.calls"
env | grep '^EB_PIPELINE_CHECK_' | sort > "$D/check.env"
exit "${STUB_CHECK_CODE:-0}"
"#;

const CACHE_STUB: &str = "#!/bin/sh\necho called >> \"${STUB_DIR:?}/cache.calls\"\nexit 0\n";

/// A site whose check command is the stub, with `CHECKBIN` replaced.
fn with_check(extra: &str) -> Env {
    let env = setup(extra);
    let bin = env.stubs.join("check");
    write_exec(&bin, CHECK);
    let text = std::fs::read_to_string(&env.site)
        .unwrap()
        .replace("CHECKBIN", &bin.display().to_string());
    std::fs::write(&env.site, text).unwrap();
    env
}

fn phase_status(r: &Value, name: &str) -> String {
    r["phases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == name)
        .unwrap()["status"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn a_clean_build_runs_the_check_command() {
    let env = with_check("[checks]\ncommand = \"CHECKBIN -c /opt/checks\"\n");
    let o = go(&env, LIST, &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(
        calls(&env, "check.calls"),
        vec!["-c /opt/checks".to_string()]
    );
    let r = o.result.unwrap();
    assert_eq!(phase_status(&r, "checks"), "ok");
    let recorded = std::fs::read_to_string(env.dir.path().join("check.env")).unwrap();
    assert!(
        recorded.contains("EB_PIPELINE_CHECK_STAGED=0\n"),
        "{recorded}"
    );
    assert!(
        recorded.contains("EB_PIPELINE_CHECK_RUN_ID=r1\n"),
        "{recorded}"
    );
    assert!(
        recorded.contains("EB_PIPELINE_CHECK_SOFTWARE="),
        "{recorded}"
    );
    assert!(
        recorded.contains("EB_PIPELINE_CHECK_MODULES="),
        "{recorded}"
    );
}

#[test]
fn a_failed_check_fails_the_run_and_skips_caches() {
    let env = with_check(
        "[checks]\ncommand = \"CHECKBIN\"\n[caches]\ncommand = \"CACHEBIN\"\n[[caches.targets]]\ntarget = \"cpu\"\nconfig = \"/opt/c\"\n",
    );
    let cache = env.stubs.join("cache");
    write_exec(&cache, CACHE_STUB);
    let text = std::fs::read_to_string(&env.site)
        .unwrap()
        .replace("CACHEBIN", &cache.display().to_string());
    std::fs::write(&env.site, text).unwrap();
    let o = go(&env, LIST, &[("STUB_CHECK_CODE", "7")]);
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(calls(&env, "cache.calls").is_empty());
    let r = o.result.unwrap();
    assert_eq!(phase_status(&r, "checks"), "failed");
    assert_eq!(
        r["phases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["name"] == "checks")
            .unwrap()["exit_code"],
        7
    );
    assert_eq!(phase_status(&r, "caches"), "skipped");
}

#[test]
fn a_failed_build_does_not_run_the_check() {
    let env = with_check("[checks]\ncommand = \"CHECKBIN\"\n");
    let o = go(
        &env,
        "Foo-1.0-foss-2025a.eb\n",
        &[("STUB_EB_FAIL", "Foo-1.0-foss-2025a")],
    );
    assert_eq!(o.code, 1, "{}", o.stderr);
    assert!(calls(&env, "check.calls").is_empty());
    assert_eq!(phase_status(&o.result.unwrap(), "checks"), "skipped");
}

#[test]
fn staged_checks_run_only_after_a_staged_rebuild() {
    let env = with_check(
        "[rebuild]\nstaged = true\n[checks]\ncommand = \"CHECKBIN\"\nwhen = \"staged\"\n",
    );
    let o = go(&env, "Foo-1.0-foss-2025a.eb\n", &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(calls(&env, "check.calls").is_empty());
    assert_eq!(phase_status(&o.result.unwrap(), "checks"), "not configured");

    install_module(&env, "Foo/1.0-foss-2025a.lua");
    let o = go(&env, "Foo-1.0-foss-2025a.eb --rebuild\n", &[]);
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert_eq!(calls(&env, "staged.calls").len(), 1);
    assert_eq!(calls(&env, "check.calls").len(), 1);
    let recorded = std::fs::read_to_string(env.dir.path().join("check.env")).unwrap();
    assert!(
        recorded.contains("EB_PIPELINE_CHECK_STAGED=1\n"),
        "{recorded}"
    );
}
