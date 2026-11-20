//! A site file with targets `cpu` and `gpu`, stub `eb`, `eb-keep-logs` and
//! `eb-stack` binaries, and a fake EESSI tree, for the tests of the run
//! summary, `watch` and what reads them. Each test file uses a part of it.
#![allow(dead_code)]
#![allow(clippy::disallowed_methods)]
#[path = "exec_stub.rs"]
mod exec_stub_shared;

use serde_json::Value;
use std::io::BufRead;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Output, Stdio};
use std::time::Duration;

pub const STAMP: &str = "20260101_000000";

/// A standard-output handle whose pipeline consumer has already closed.
pub fn closed_stdout() -> Stdio {
    let mut fds = [0; 2];
    // SAFETY: both descriptors are checked, and ownership of the write
    // descriptor moves into `Stdio`.
    unsafe {
        assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
        assert_eq!(libc::close(fds[0]), 0);
        Stdio::from_raw_fd(fds[1])
    }
}

/// A `flock(2)` lock on a file, held by a separate process until released.
///
/// A descriptor for the lock file in this multithreaded process can be copied
/// by another test thread's fork and kept until that child execs. The lock
/// then outlives the test's release and the next lock attempt fails. The
/// holder is one process that owns the only descriptor, and `release` returns
/// only after it is dead, so the lock is free from then on. It is stopped with
/// a signal, not by closing a pipe, because a forked copy of a pipe end would
/// keep the holder alive in the same way.
pub struct LockHolder {
    child: Child,
    _stdin: Option<ChildStdin>,
}

impl LockHolder {
    pub fn shared(path: &Path) -> Self {
        Self::hold("--shared", path)
    }

    pub fn exclusive(path: &Path) -> Self {
        Self::hold("--exclusive", path)
    }

    fn hold(mode: &str, path: &Path) -> Self {
        let script = r#"exec 9>>"$1" || exit 98; flock "$2" --timeout 20 9 || exit 99; echo held; exec cat >/dev/null"#;
        let mut child = Command::new("sh")
            .args(["-c", script, "lock-holder"])
            .arg(path)
            .arg(mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("cannot start the lock holder");
        let stdin = child.stdin.take();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "held\n", "cannot lock {}", path.display());
        Self {
            child,
            _stdin: stdin,
        }
    }

    pub fn release(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for LockHolder {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Take an advisory lock in this process, as another writer would.
pub fn hold_lock(path: &Path, posix: bool) -> std::fs::File {
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

/// Fails the easyconfig stems listed in STUB_EB_FAIL. With STUB_EB_INSTALL
/// set it installs `Name-version.eb` as EasyBuild does, a module file and
/// an `easybuild` directory with a new log, unless the module is there and
/// the line does not ask for a rebuild.
const EB: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
ec="$1"; stem="${ec##*/}"; stem="${stem%.eb}"
echo "$*" >> "$D/eb.calls"
case ",${STUB_EB_FAIL:-}," in *",$stem,"*) echo "ERROR: $stem failed"; exit 1;; esac
[ -n "${STUB_EB_INSTALL:-}" ] || exit 0
name="${stem%%-*}"; ver="${stem#*-}"
root="$EASYBUILD_INSTALLPATH/software/$name/$ver"
mod="$EASYBUILD_INSTALLPATH/modules/all/$name/$ver.lua"
case " $* " in
  *" --rebuild "*) rm -rf "$root";;
  *) if [ -f "$mod" ]; then echo "$stem is already installed"; exit 0; fi;;
esac
mkdir -p "$root/easybuild" "${mod%/*}"
echo "name = '$name'" > "$root/easybuild/$stem.eb"
: > "$root/easybuild/easybuild-$name-$ver-$(date +%s%N).log"
printf 'help([==[%s]==])\nlocal root = "%s"\n' "$name" "$root" > "$mod"
exit 0
"#;

/// `rebuild-set` answers from `$STUB_DIR/deps.tsv`, one `dependent changed
/// class edge_kind` row per line: the rows whose changed easyconfig is in
/// `--changed`. `stack shard` writes the build list in the order of the
/// easyconfigs in `$STUB_DIR/order.txt`, then the rest as given, and in
/// `plan.json` each line's closure: the modules `$STUB_DIR/missing.txt`
/// gives it, one `easyconfig module` pair per line, then its own.
const EB_STACK: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
case "$1" in --version) echo "eb-stack 0.7.4"; exit 0;; esac
echo "$*" >> "$D/ebstack.calls"
cmd="$1"; shift
if [ "$cmd" = stack ]; then cmd="stack-$1"; shift; fi
while [ $# -gt 0 ]; do
  case "$1" in
    --out-dir) out="$2"; shift;;
    --out) outfile="$2"; shift;;
    --lock) lock="$2"; shift;;
    --changed) changed="$2"; shift;;
    --build-list) list="$2"; shift;;
  esac
  shift
done
[ -n "${STUB_EB_STACK_FAIL:-}" ] && { echo "error: stub failure"; exit "$STUB_EB_STACK_FAIL"; }
[ "$cmd" = stack-sbom ] && [ -n "${STUB_SBOM_DELAY:-}" ] && sleep "$STUB_SBOM_DELAY"
[ "$cmd" = stack-sbom ] && [ -n "${STUB_SBOM_SIGNAL:-}" ] && kill "-$STUB_SBOM_SIGNAL" "$$"
[ "$cmd" = stack-sbom ] && [ -n "${STUB_SBOM_FAIL:-}" ] && { echo "error: sbom"; exit "$STUB_SBOM_FAIL"; }
[ "$cmd" = stack-sbom ] && [ -n "${STUB_SBOM_SKIP_WRITE:-}" ] && exit 0
mkdir -p "$out"
touch "$D/deps.tsv" "$D/order.txt" "$D/missing.txt"
case "$cmd" in
  rebuild-set)
    rows=$(awk -F'\t' 'NR == FNR { c[$1] = 1; next }
      ($2 in c) { printf "%s{\"changed\":\"%s\",\"dependent\":\"%s\",\"class\":\"%s\",\"edge_kind\":\"%s\",\"reason\":\"stub\"}", (n++ ? "," : ""), $2, $1, $3, $4 }' "$changed" "$D/deps.tsv")
    if [ -n "${STUB_REBUILD_SET_JSON+x}" ]; then
      printf '%s' "$STUB_REBUILD_SET_JSON" > "$out/rebuild-set.json"
    else
      printf '{"schema_version":5,"rows":[%s]}\n' "$rows" > "$out/rebuild-set.json"
    fi;;
  stack-sbom)
    printf '%s\n' '{"bomFormat":"CycloneDX","specVersion":"1.5"}' > "$outfile";;
  stack-shard)
    awk 'NR == FNR { o[$1] = NR; next } { print (($1 in o) ? o[$1] : 100000 + FNR) "\t" $0 }' "$D/order.txt" "$list" \
      | sort -n | cut -f2- > "$out/shard-01.list"
    lines=$(awk 'FILENAME == ARGV[1] { need[$1] = need[$1] "\"" $2 "\","; next }
      { m = $1; sub(/\.eb$/, "", m); sub(/-/, "/", m)
        printf "%s{\"easyconfig\":\"%s\",\"module\":\"%s\",\"closure\":[%s\"%s\"]}", (n++ ? "," : ""), $1, m, need[$1], m }' \
      "$D/missing.txt" "$out/shard-01.list")
    if [ -n "${STUB_SHARD_PLAN_JSON+x}" ]; then
      printf '%s' "$STUB_SHARD_PLAN_JSON" > "$out/plan.json"
    else
      printf '{"prelude":null,"shards":[{"name":"shard-01","file":"shard-01.list"}],"lines":[%s]}\n' "$lines" > "$out/plan.json"
    fi;;
  stack-from-commit)
    printf '{}\n';;
esac
exit 0
"#;

/// `rotate` exits STUB_ROTATE_EXIT, saying what eb-keep-logs says when it
/// exits 75 because another rotate holds its lock.
const KEEP_LOGS: &str = r#"#!/bin/sh
case "$1" in
  --version) echo "eb-keep-logs 0.2.0"; exit 0;;
  -h|--help) echo "  eb-keep-logs rotate ROOT --compress-after DAYS [--delete-after DAYS] [--lock-timeout SECONDS]"; exit 0;;
esac
echo "$*" >> "$STUB_DIR/keeplogs.calls"
if [ "$1" = rotate ] && [ "${STUB_ROTATE_EXIT:-0}" = 75 ]; then
  echo "eb-keep-logs: another rotate holds the lock $2/.eb-keep-logs-rotate.lock; gave up after 0 s (--lock-timeout)" >&2
  exit 75
fi
exit 0
"#;

pub struct T {
    pub dir: tempfile::TempDir,
    pub site: PathBuf,
    pub stubs: PathBuf,
}

impl T {
    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    pub fn run_dir(&self, stamp: &str) -> PathBuf {
        self.root().join("logs").join(stamp)
    }

    pub fn summary(&self, stamp: &str) -> Value {
        let p = self.run_dir(stamp).join("summary.json");
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        serde_json::from_str(&text).unwrap()
    }

    /// The fake EESSI module tree.
    pub fn eessi_modules(&self) -> PathBuf {
        self.root().join("eessi/modules/all")
    }

    /// Install `name/version` in the fake EESSI tree, or rebuild it in
    /// place, which leaves another log in its `easybuild` directory.
    pub fn eessi_module(&self, name: &str, version: &str) {
        let root = self.root().join("eessi/software").join(name).join(version);
        let eb = root.join("easybuild");
        std::fs::create_dir_all(&eb).unwrap();
        std::fs::write(eb.join(format!("{name}-{version}.eb")), "").unwrap();
        let logs = std::fs::read_dir(&eb).unwrap().count();
        std::fs::write(
            eb.join(format!("easybuild-{name}-{version}-{logs}.log")),
            "",
        )
        .unwrap();
        let dir = self.eessi_modules().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{version}.lua")),
            format!(
                "help([==[{name}]==])\nlocal root = \"{}\"\n",
                root.display()
            ),
        )
        .unwrap();
    }

    pub fn remove_eessi_module(&self, name: &str, version: &str) {
        std::fs::remove_file(
            self.eessi_modules()
                .join(name)
                .join(format!("{version}.lua")),
        )
        .unwrap();
    }

    /// What `eb-stack rebuild-set` answers: `(dependent, changed easyconfig,
    /// class, edge kind)` rows.
    pub fn deps(&self, rows: &[(&str, &str, &str, &str)]) {
        let text: String = rows
            .iter()
            .map(|(d, c, k, e)| format!("{d}\t{c}\t{k}\t{e}\n"))
            .collect();
        std::fs::write(self.root().join("deps.tsv"), text).unwrap();
    }

    /// The watch state of `target`.
    pub fn watch_state(&self, target: &str) -> Value {
        let p = self
            .root()
            .join("install/.eb-pipeline-watch")
            .join(format!("{target}.json"));
        let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        serde_json::from_str(&text).unwrap()
    }

    pub fn calls(&self, tool: &str) -> String {
        std::fs::read_to_string(self.root().join(format!("{tool}.calls"))).unwrap_or_default()
    }
}

/// The variables EESSI's init sets, for the fake tree.
pub fn eessi_vars(t: &T, version: &'static str) -> Vec<(&'static str, String)> {
    vec![
        ("EESSI_VERSION", version.to_string()),
        ("EESSI_SOFTWARE_SUBDIR", "x86_64/generic".to_string()),
        ("EESSI_MODULEPATH", t.eessi_modules().display().to_string()),
        ("STUB_EB_INSTALL", "1".to_string()),
    ]
}

/// Give `dir` the CVMFS revision attribute that a repository carries.
pub fn set_revision(dir: &Path, rev: u64) {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).unwrap();
    let value = rev.to_string();
    // SAFETY: both names are NUL-terminated and the value outlives the call.
    let r = unsafe {
        libc::setxattr(
            path.as_ptr(),
            c"user.revision".as_ptr(),
            value.as_ptr().cast(),
            value.len(),
            0,
        )
    };
    assert_eq!(r, 0, "{}", std::io::Error::last_os_error());
}

pub fn write_exec(path: &Path, text: &str) {
    exec_stub_shared::write_exec(path, text);
}

/// A stand-in for srun, and for the prolog and epilog of its node: each job
/// is the next job of alice, in a directory under `local/` that goes when
/// the job ends. A job of the phase that STUB_NO_JOB_DIR names gets none;
/// the jobs of `eb-stack rebuild-set` and `eb-stack stack shard` count as
/// phases `rebuild-set` and `shard`.
pub const JOB_DIR_SRUN: &str = r#"#!/bin/sh
D="${STUB_DIR:?}"
job=$(( $(cat "$D/srun.jobs" 2>/dev/null || echo 0) + 1 ))
echo "$job" > "$D/srun.jobs"
phase=
while [ $# -gt 0 ]; do
  case $1 in
    --job-name=*) phase=${1##*-} ;;
    -*) ;;
    *) break ;;
  esac
  shift
done
for a; do case $a in rebuild-set|shard) phase=$a ;; esac; done
dir="$D/local/alice.$job"
[ -n "${STUB_NO_JOB_DIR:-}" ] && [ "$phase" = "$STUB_NO_JOB_DIR" ] || mkdir -p "$dir"
SLURM_JOB_ID=$job SLURM_JOB_USER=alice "$@"
code=$?
rm -rf "$dir"
exit "$code"
"#;

/// Start each command of the site through `srun`, a stand-in for srun, with
/// `build_path` in the job's directory under `local/`, which is returned.
pub fn with_job_dirs(t: &T, srun: &str) -> PathBuf {
    let bin = t.stubs.join("srun");
    write_exec(&bin, srun);
    let local = t.root().join("local");
    let text = std::fs::read_to_string(&t.site)
        .unwrap()
        .replace(
            "kind = \"local\"",
            &format!("kind = \"slurm\"\nlaunch = \"{}\"", bin.display()),
        )
        .replace(
            &format!("build_path = \"{}/build\"", t.root().display()),
            &format!(
                "job_dir = \"{}/{{user}}.{{job}}\"\nbuild_path = \"{{job_dir}}/build\"",
                local.display()
            ),
        );
    std::fs::write(&t.site, text).unwrap();
    local
}

/// A site file whose tables end with `extra`, in which `@ROOT@` is the
/// test's directory.
pub fn setup(extra: &str) -> T {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let stubs = root.join("stubs");
    std::fs::create_dir(&stubs).unwrap();
    write_exec(&stubs.join("eb"), EB);
    write_exec(&stubs.join("eb-keep-logs"), KEEP_LOGS);
    write_exec(&stubs.join("eb-stack"), EB_STACK);
    let r = root.display();
    let s = stubs.display();
    let extra = extra.replace("@ROOT@", &r.to_string());
    let site = format!(
        r#"
schema_version = 1
kind = "site"
[site]
name = "t"
[site.tools.eb_keep_logs]
bin = "{s}/eb-keep-logs"
min_version = "0.2"
[scheduler]
kind = "local"
[[targets]]
name = "cpu"
[[targets]]
name = "gpu"
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
[logs.rotate]
compress_after_days = 3
{extra}
"#
    );
    let site_path = root.join("site.toml");
    std::fs::write(&site_path, site).unwrap();
    T {
        dir,
        site: site_path,
        stubs,
    }
}

/// Drop the caller's `EESSI_*` variables from a command's environment. The stub
/// module environment prints the environment it inherits, so a test started
/// inside a loaded EESSI stack would probe that stack and not its fixture.
pub fn without_host_eessi(c: &mut Command) -> &mut Command {
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("EESSI_") {
            c.env_remove(key);
        }
    }
    c
}

/// `eb-pipeline ARGS`, with the stubs first on PATH.
pub fn eb_pipeline(t: &T, args: &[&str], vars: &[(&str, &str)]) -> Output {
    let path = format!(
        "{}:{}",
        t.stubs.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let mut c = Command::new(env!("CARGO_BIN_EXE_eb-pipeline"));
    without_host_eessi(&mut c)
        .args(args)
        .env("PATH", path)
        .env("STUB_DIR", t.root())
        .env_remove("EB_PIPELINE_LOG_STAMP")
        .env_remove("EB_PIPELINE_RUN_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in vars {
        c.env(k, v);
    }
    c.output().unwrap()
}

/// `run` of `list` for `target` in the run directory `stamp`.
pub fn run(t: &T, target: &str, stamp: &str, list: &str, vars: &[(&str, &str)]) -> Output {
    run_extra(t, target, stamp, list, &[], vars)
}

/// `run` with additional command-line arguments.
pub fn run_extra(
    t: &T,
    target: &str,
    stamp: &str,
    list: &str,
    extra: &[&str],
    vars: &[(&str, &str)],
) -> Output {
    let list_path = t.root().join(format!("list-{target}.txt"));
    std::fs::write(&list_path, list).unwrap();
    let site = t.site.display().to_string();
    let list_arg = list_path.display().to_string();
    let result = t.root().join(format!("result-{target}.json"));
    let result_arg = result.display().to_string();
    let mut args = vec![
        "run",
        "--site",
        &site,
        "--list",
        &list_arg,
        "--target",
        target,
        "--build-type",
        "production",
        "--log-stamp",
        stamp,
        "--result",
        &result_arg,
    ];
    args.extend_from_slice(extra);
    eb_pipeline(t, &args, vars)
}

pub fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

pub fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

pub fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

/// Create an empty FIFO at `path`.
pub fn make_fifo(path: &Path) {
    use std::os::unix::ffi::OsStrExt;
    let raw = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
}

/// Spawn `cmd` and collect its output. A child still running after `limit`
/// is killed so the caller does not wait on a blocked read.
pub fn output_within(cmd: &mut Command, limit: Duration) -> Output {
    let mut child = cmd.spawn().unwrap();
    let pid = libc::pid_t::try_from(child.id()).unwrap();
    // SAFETY: `pid` is the live child just spawned, and `pidfd` is closed below.
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    assert!(
        pidfd >= 0,
        "pidfd_open: {}",
        std::io::Error::last_os_error()
    );
    let pidfd = libc::c_int::try_from(pidfd).unwrap();
    let mut pollfd = libc::pollfd {
        fd: pidfd,
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout = i32::try_from(limit.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: `pollfd` is a valid one-element list and `pidfd` stays open.
    let waited = unsafe { libc::poll(&mut pollfd, 1, timeout) };
    // SAFETY: `pidfd` is the descriptor opened above and is not used again.
    unsafe { libc::close(pidfd) };
    assert!(waited >= 0, "poll: {}", std::io::Error::last_os_error());
    if waited == 0 || child.try_wait().unwrap().is_none() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("command still running after {limit:?}");
    }
    child.wait_with_output().unwrap()
}
