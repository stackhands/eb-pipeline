//! Execution of a plan as subprocesses.

use crate::exit::{self, Fail};
use crate::plan::{
    build_plan, read_units, shell_line, valid_job_id, Cmd, Deselected, Plan, Planner, StemSeq,
    Unit, PHASES,
};
use crate::resolve::Role;
use crate::resume::{self, Prior};
use crate::tools::{help_lists, map_rotate, map_staged, RotateOutcome, StagedOutcome, Tool};
use crate::util::{self, ListLine};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::File;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::{Duration, Instant};

static SIGNAL: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(sig: libc::c_int) {
    SIGNAL.store(sig, Ordering::SeqCst);
}

/// Install SIGTERM and SIGINT handlers that record the signal.
pub fn install_handlers() {
    for s in [libc::SIGTERM, libc::SIGINT] {
        // SAFETY: the handler only stores into an atomic.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = 0;
            libc::sigaction(s, &sa, std::ptr::null_mut());
        }
    }
}

fn interrupted() -> Option<i32> {
    match SIGNAL.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

/// The exit code of a SIGINT or SIGTERM received since `install_handlers`.
pub fn signal_exit() -> Option<i32> {
    interrupted().map(|s| {
        if s == libc::SIGINT {
            exit::SIGINT
        } else {
            exit::SIGTERM
        }
    })
}

/// The wait before the next admission attempt. It starts at `wait_seconds`
/// and is never longer than `max_wait_seconds`, including when that maximum
/// is the smaller of the two.
fn capped_wait(wait_seconds: u64, max_wait_seconds: u64) -> u64 {
    wait_seconds.min(max_wait_seconds)
}

/// The wait after a refused attempt: double the previous one, still capped
/// at `max_wait_seconds`.
fn doubled_wait(wait: u64, max_wait_seconds: u64) -> u64 {
    wait.saturating_mul(2).min(max_wait_seconds)
}

/// Sleep in short slices; returns true when a signal arrived.
fn sleep_interruptible(secs: u64) -> bool {
    let mut left = Duration::from_secs(secs);
    let slice = Duration::from_millis(50);
    while !left.is_zero() {
        if interrupted().is_some() {
            return true;
        }
        let d = left.min(slice);
        std::thread::sleep(d);
        left -= d;
    }
    interrupted().is_some()
}

/// Print a log line on stderr: `<level> [<phase>] <target>: <msg>`.
pub fn log(level: &str, phase: &str, target: &str, msg: &str) {
    eprintln!("{level} [{phase}] {target}: {msg}");
}

/// Seconds, to the millisecond.
pub fn seconds(d: Duration) -> f64 {
    (d.as_secs_f64() * 1000.0).round() / 1000.0
}

/// What one phase came to.
#[derive(Debug, Serialize, Clone)]
pub struct PhaseResult {
    /// The phase, one of [`PHASES`].
    pub name: String,
    /// `ok`, `failed`, `refused`, `skipped`, `unsharded`, `interrupted` or
    /// `not configured`.
    pub status: String,
    /// The exit code behind the status, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Why the phase came to its status, when that needs saying.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Time the phase's commands ran; absent for a phase that ran none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
}

/// What one build list line came to.
#[derive(Debug, Serialize, Clone)]
pub struct LineResult {
    /// The line's easyconfig.
    pub easyconfig: String,
    /// The unit it belongs to: `all`, `cpu-prerequisites`, `prelude`, or a
    /// shard's name.
    pub unit: String,
    /// `build`, or `sources` when its source fetch failed.
    pub phase: String,
    /// `success`, `failed`, `refused`, `pending`, `skipped`.
    pub status: String,
    /// The exit code the line counts as.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// The command's own exit code, when `exit_code` translates it, as 78
    /// does eb-staged-rebuild's 2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translated_from: Option<i32>,
    /// The log file of the line's command.
    pub log: String,
    /// Why the line came to its status, when that needs saying.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Time the line's command ran; absent for a line that did not start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    /// Module files a successful line left, when `[resume]` recorded them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
}

/// What one unit's admission came to.
#[derive(Debug, Serialize, Clone)]
pub struct AdmissionResult {
    /// The unit, as in [`LineResult::unit`].
    pub unit: String,
    /// How many times `stack admit` ran.
    pub attempts: u32,
    /// `admitted`, `refused` or `failed`.
    pub outcome: String,
    /// The last JSON line `stack admit` printed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_json: Option<String>,
}

/// The result file `run` writes: what every phase and line came to.
#[derive(Debug, Serialize, Clone)]
pub struct RunResult {
    /// Version of this format, 1.
    pub schema_version: u32,
    /// `result`.
    pub kind: String,
    /// `site.name`.
    pub site: String,
    /// The target's name.
    pub target: String,
    /// The build type, `default` without one.
    pub build_type: String,
    /// The run id.
    pub run_id: String,
    /// The build list, as given.
    pub list: String,
    /// Where the run writes its logs.
    pub log_dir: String,
    /// When the run started, in RFC 3339, in UTC.
    pub started_at: String,
    /// When the run ended, in RFC 3339, in UTC.
    pub ended_at: String,
    /// Time from the start of the run to its result, to the millisecond.
    pub duration_seconds: f64,
    /// The process exit code.
    pub exit_code: i32,
    /// Whether the lines were built in the units of a shard plan.
    pub sharded: bool,
    /// SHA-256 of the `stack order --hashes-out` file, when `[resume]` recorded it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_digest: Option<String>,
    /// SHA-256 of the eb-stack `package.plan.json`, for a package-plan input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_plan_digest: Option<String>,
    /// Checkpoint and inclusion proof of the input record, when `[attest]` is
    /// enabled. Absent, the result JSON is unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_attestation: Option<crate::attest::InputAttestation>,
    /// How many lines came to each status.
    pub counts: BTreeMap<String, usize>,
    /// The phases the site file leaves out.
    pub not_configured: Vec<String>,
    /// The admission of each unit that went through it, in order.
    pub admission: Vec<AdmissionResult>,
    /// Every phase that was reached, in [`PHASES`] order.
    pub phases: Vec<PhaseResult>,
    /// Every line, in the order the run reached them.
    pub lines: Vec<LineResult>,
    /// Lines the target's selector left out; not counted as results.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deselected: Vec<Deselected>,
    /// The error that stopped the run, when one did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// `SIGINT` or `SIGTERM`, when a signal stopped the run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupted_by: Option<String>,
}

struct Exec {
    code: i32,
    log: PathBuf,
    elapsed: Duration,
}

/// Run one command to completion with output in a log file. A signal that
/// arrives is forwarded to the child once.
fn exec(cmd: &Cmd, base_env: &BTreeMap<String, String>, log: &Path) -> Result<Exec, Fail> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot create {}: {e}", dir.display()),
            )
        })?;
    }
    let out = File::create(log).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", log.display()),
        )
    })?;
    let err = out
        .try_clone()
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot duplicate log handle: {e}")))?;
    let mut c = Command::new(&cmd.argv[0]);
    c.args(&cmd.argv[1..])
        .envs(base_env)
        .envs(&cmd.env)
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    // SAFETY: setpgid is async-signal-safe and runs before the child execs.
    unsafe {
        c.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    let started = Instant::now();
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            let _ = std::fs::write(log, format!("cannot start {}: {e}\n", cmd.argv[0]));
            return Ok(Exec {
                code: 127,
                log: log.to_path_buf(),
                elapsed: started.elapsed(),
            });
        }
    };
    let mut forwarded = 0;
    let status = loop {
        if let Some(sig) = interrupted() {
            if forwarded != sig {
                // SAFETY: the negative pid names the process group created for this child.
                unsafe {
                    libc::kill(-(child.id() as libc::pid_t), sig);
                }
                forwarded = sig;
            }
        }
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => {
                return Err(Fail::new(exit::FAILED, format!("wait failed: {e}")));
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    use std::os::unix::process::ExitStatusExt;
    let code = status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0));
    Ok(Exec {
        code,
        log: log.to_path_buf(),
        elapsed: started.elapsed(),
    })
}

/// Run `cmd` as a phase runs its commands, output in `log`, and return its
/// exit code: 127 when it cannot start.
pub fn exec_logged(
    cmd: &Cmd,
    base_env: &BTreeMap<String, String>,
    log: &Path,
) -> Result<i32, Fail> {
    exec(cmd, base_env, log).map(|e| e.code)
}

fn last_json_line(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            serde_json::from_str::<serde_json::Value>(line).is_ok_and(|value| value.is_object())
        })
        .map(str::to_string)
}

/// The lock eb-staged-rebuild names when it gives up waiting for another
/// writer.
fn held_lock(path: &Path) -> Option<String> {
    const HELD: &str = "another eb-staged-rebuild holds the lock at ";
    let bytes = std::fs::read(path).ok()?;
    String::from_utf8_lossy(&bytes)
        .lines()
        .rev()
        .find_map(|l| l.split_once(HELD).map(|(_, lock)| lock.trim().to_string()))
}

/// Why a staged line was refused, when eb-staged-rebuild found a lock held:
/// the lock, and whether `rebuild.lock_timeout` ran out waiting for it.
/// Without `rebuild.state_root`, a module the line loads can set the state
/// root in EB_STAGED_REBUILD_STATE_ROOT, out of eb-pipeline's sight, so a
/// lock away from the expected state root may still be the state root's.
fn lock_refusal(
    p: &Planner,
    cmd: &Cmd,
    env: &BTreeMap<String, String>,
    log: &Path,
) -> Option<String> {
    const ROOT: &str = "EB_STAGED_REBUILD_STATE_ROOT";
    let lock = held_lock(log)?;
    let env_root = cmd
        .env
        .get(ROOT)
        .or(env.get(ROOT))
        .cloned()
        .or_else(|| std::env::var(ROOT).ok());
    let ours = p.state_root_lock(env_root.as_deref());
    let n = p.scope.site.rebuild_on().map_or(0, |r| r.lock_timeout);
    if Path::new(&lock).components().eq(ours.components()) {
        return Some(format!(
            "another eb-staged-rebuild holds the lock at {lock}, and rebuild.lock_timeout ({n} s) ran out"
        ));
    }
    if p.scope.state_root().is_some() {
        return Some(format!(
            "another eb-staged-rebuild holds the lock at {lock}"
        ));
    }
    Some(format!(
        "another eb-staged-rebuild holds the lock at {lock}, and rebuild.lock_timeout ({n} s) ran out if that is the state root's lock: rebuild.state_root is unset, and eb-pipeline cannot see a state root that a module sets in {ROOT}"
    ))
}

/// The reservation `stack admit` wrote for this job, `<reservations>/<job>.json`.
struct Reservation {
    path: PathBuf,
    job: String,
    lock: Option<PathBuf>,
    posix: bool,
}

/// Take the admission lock the way `stack admit` takes it: `flock`, or a
/// POSIX record lock over the whole file. Dropping the file releases it.
fn admission_lock(path: &Path, posix: bool) -> std::io::Result<File> {
    use std::os::fd::AsRawFd;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    let fd = f.as_raw_fd();
    // SAFETY: `fd` is open for the whole call, and a zeroed `libc::flock` is
    // a valid value of that plain C struct.
    let rc = unsafe {
        if posix {
            let mut l: libc::flock = std::mem::zeroed();
            l.l_type = libc::F_WRLCK as _;
            l.l_whence = libc::SEEK_SET as _;
            libc::fcntl(fd, libc::F_SETLKW, &raw const l)
        } else {
            libc::flock(fd, libc::LOCK_EX)
        }
    };
    if rc == 0 {
        Ok(f)
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Hold the rotation lock shared for the whole run, so that another run's
/// rotation does not compress this run's logs while they are written: a run
/// that began on an earlier day in UTC is old enough. Waits while another
/// run rotates; `Ok(None)` when the lock cannot be had, and the rotation then
/// goes ahead without it, as it did before there was one.
fn hold_rotate_lock(st: &mut State, path: &Path) -> Result<Option<File>, i32> {
    let target = st.res.target.clone();
    let without = |e: std::io::Error| {
        log(
            "WARN",
            "logs",
            &target,
            &format!(
                "cannot lock {}: {e}; the rotation does not wait for other runs",
                path.display()
            ),
        );
        Ok(None)
    };
    let opened = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| {
            std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(path)
        });
    let f = match opened {
        Ok(f) => f,
        Err(e) => return without(e),
    };
    let mut waiting = false;
    loop {
        match f.try_lock_shared() {
            Ok(()) => return Ok(Some(f)),
            Err(std::fs::TryLockError::WouldBlock) => {
                if !waiting {
                    let msg = format!("waiting: another run rotates and holds {}", path.display());
                    log("INFO", "logs", &target, &msg);
                    waiting = true;
                }
                if sleep_interruptible(1) {
                    if let Some(c) = stop_if_signalled(st, "logs") {
                        return Err(c);
                    }
                }
            }
            Err(std::fs::TryLockError::Error(e)) => return without(e),
        }
    }
}

/// The rotation lock taken alone in place of this run's shared hold;
/// `Ok(None)` while another run holds it, live or rotating.
fn rotate_alone(f: File) -> std::io::Result<Option<File>> {
    f.unlock()?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

/// Remove this job's reservation under the admission lock. A record that is
/// no longer the one `stack admit` wrote for this job is kept: a forecast or
/// waiting record belongs to another writer.
fn release(r: Reservation, target: &str) {
    let kept = |why: String| {
        log(
            "WARN",
            "admission",
            target,
            &format!("reservation {} kept: {why}", r.path.display()),
        )
    };
    let _lock = match r.lock.as_deref().map(|l| admission_lock(l, r.posix)) {
        Some(Err(e)) => return kept(format!("cannot take the lock: {e}")),
        held => held,
    };
    let text = match std::fs::read_to_string(&r.path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => return kept(e.to_string()),
    };
    let ours = serde_json::from_str::<serde_json::Value>(&text).is_ok_and(|v| {
        v["job"] == r.job.as_str() && v.get("phase").is_none_or(|p| p == "admitted")
    });
    if !ours {
        return kept("it is no longer this job's admitted reservation".into());
    }
    match std::fs::remove_file(&r.path) {
        Ok(()) => log(
            "INFO",
            "admission",
            target,
            &format!("released {}", r.path.display()),
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => kept(e.to_string()),
    }
}

/// How [`run`] goes through the plan.
pub struct Options<'a> {
    /// Print each planned command, run none, and return exit 0.
    pub dry_run: bool,
    /// Units read from a saved `stack shard` plan, when the caller supplied one.
    pub shard_units: Option<&'a [Unit]>,
    /// Node passed to `eb-stack stack oom`; absent leaves the run unchanged.
    pub oom_node: Option<&'a str>,
    /// Prior result to resume from, when `--resume-result` named one.
    pub resume: Option<&'a Prior>,
    /// Why `--resume-result` is refused before any command, when it is.
    pub resume_refused: Option<&'a str>,
    /// File that receives the plan before any phase command starts.
    pub plan_out: Option<&'a Path>,
}

#[derive(Serialize)]
struct OomPlan<'a> {
    easyconfigs: Vec<&'a str>,
}

fn write_plan(path: &Path, plan: &Plan) -> Result<(), Fail> {
    let value = serde_json::to_value(plan).expect("plan is serializable");
    let text = serde_json::to_string_pretty(&value).expect("plan is serializable") + "\n";
    util::write_atomic(path, text.as_bytes()).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write plan {}: {e}", path.display()),
        )
    })
}

fn parse_oom_count(stdout: &str) -> Result<usize, Fail> {
    let mut counts = stdout.lines().filter_map(|line| {
        line.split_whitespace()
            .next()
            .and_then(|word| word.strip_prefix("oom="))
    });
    let Some(raw) = counts.next() else {
        return Err(Fail::new(
            exit::FAILED,
            "eb-stack stack oom printed no `oom=N` line",
        ));
    };
    if counts.next().is_some() {
        return Err(Fail::new(
            exit::FAILED,
            "eb-stack stack oom printed more than one `oom=N` line",
        ));
    }
    raw.parse().map_err(|_| {
        Fail::new(
            exit::FAILED,
            format!("eb-stack stack oom printed an invalid count `oom={raw}`"),
        )
    })
}

fn oom_preflight(p: &Planner, units: &[Unit], node: &str, log_dir: &Path) -> Result<usize, Fail> {
    let easyconfigs = units
        .iter()
        .flat_map(|unit| unit.lines.iter())
        .map(|line| line.easyconfig.as_str())
        .collect();
    let text = serde_yaml::to_string(&OomPlan { easyconfigs })
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot make OOM plan: {e}")))?;
    std::fs::create_dir_all(log_dir).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot create {}: {e}", log_dir.display()),
        )
    })?;
    let plan = log_dir.join("oom-preflight.yml");
    std::fs::write(&plan, text).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot write {}: {e}", plan.display()),
        )
    })?;
    let bin = Tool::EbStack.bin(p.scope.site);
    let out = Command::new(&bin)
        .args(["stack", "oom", "--easystack"])
        .arg(&plan)
        .args(["--node", node])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Fail::new(exit::FAILED, format!("cannot run `{bin} stack oom`: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    std::fs::write(log_dir.join("oom-preflight.log"), &out.stdout).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!(
                "cannot write {}: {e}",
                log_dir.join("oom-preflight.log").display()
            ),
        )
    })?;
    if !out.status.success() {
        let code = out
            .status
            .code()
            .map_or_else(|| out.status.to_string(), |code| code.to_string());
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Fail::new(
            exit::FAILED,
            format!("eb-stack stack oom exited {code}: {}", stderr.trim()),
        ));
    }
    parse_oom_count(&stdout)
}

struct State {
    res: RunResult,
    refused: bool,
    pending: bool,
    failed: bool,
    forced_code: Option<i32>,
    /// Released when the admitted unit ends, or by `finish`.
    reservation: Option<Reservation>,
    began: Instant,
    /// Time each phase's commands ran.
    spent: BTreeMap<String, Duration>,
}

impl State {
    fn phase(&mut self, name: &str, status: &str, code: Option<i32>, note: Option<String>) {
        self.res.phases.push(PhaseResult {
            name: name.into(),
            status: status.into(),
            exit_code: code,
            note,
            duration_seconds: self.spent.get(name).copied().map(seconds),
        });
    }

    /// `exec`, with the command's time counted towards `phase`.
    fn exec(
        &mut self,
        phase: &str,
        cmd: &Cmd,
        env: &BTreeMap<String, String>,
        log: &Path,
    ) -> Result<Exec, Fail> {
        let ex = exec(cmd, env, log)?;
        *self.spent.entry(phase.to_string()).or_default() += ex.elapsed;
        Ok(ex)
    }
}

/// Execute the plan. Returns the result; its `exit_code` is the process
/// exit code.
pub fn run(planner: &Planner, lines: &[ListLine], opts: &Options<'_>) -> RunResult {
    let mut owned = planner.clone();
    let p = &mut owned;
    let site = p.scope.site;
    let target = p.scope.target.name.clone();
    let env = p.scope.env();
    let log_dir = PathBuf::from(p.scope.log_dir());
    let mut st = State {
        res: RunResult {
            schema_version: 1,
            kind: "result".into(),
            site: site.site.name.clone(),
            target: target.clone(),
            build_type: p.scope.build_name().to_string(),
            run_id: p.scope.run_id.clone(),
            list: p.list_path.clone(),
            log_dir: p.scope.log_dir(),
            started_at: chrono::Utc::now().to_rfc3339(),
            ended_at: String::new(),
            duration_seconds: 0.0,
            exit_code: 0,
            sharded: false,
            input_digest: None,
            package_plan_digest: p.package_plan.as_ref().map(|input| input.digest.clone()),
            input_attestation: None,
            counts: BTreeMap::new(),
            not_configured: vec![],
            admission: vec![],
            phases: vec![],
            lines: vec![],
            deselected: vec![],
            message: None,
            interrupted_by: None,
        },
        refused: false,
        pending: false,
        failed: false,
        forced_code: None,
        reservation: None,
        began: Instant::now(),
        spent: BTreeMap::new(),
    };

    if let Some(msg) = opts.resume_refused {
        log("ERROR", "shard", &target, msg);
        st.phase(
            "shard",
            "refused",
            Some(exit::REFUSED),
            Some(msg.to_string()),
        );
        st.res.message = Some(msg.to_string());
        return finish(st, Some(exit::REFUSED));
    }

    let mut plan = match build_plan(p, lines, None) {
        Ok(pl) => pl,
        Err(f) => {
            st.res.message = Some(f.msg.clone());
            return finish(st, Some(f.code));
        }
    };

    if opts.dry_run {
        if let Some(path) = opts.plan_out {
            if let Err(f) = write_plan(path, &plan) {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
        }
        let mut output = String::new();
        for ph in &plan.phases {
            if ph.status != "planned" {
                writeln!(output, "# [{}] {}", ph.name, ph.status).expect("writing to a string");
                continue;
            }
            for c in &ph.commands {
                writeln!(output, "[{}] {}", ph.name, shell_line(c)).expect("writing to a string");
            }
        }
        if let Err(e) = util::write_stdout(output.as_bytes()) {
            st.res.message = Some(format!("cannot write stdout: {e}"));
            return finish(st, Some(exit::FAILED));
        }
        log("INFO", "run", &target, "dry run: nothing executed");
        return finish(st, None);
    }

    if let Some(conf) = site.attest_on() {
        if p.package_plan.is_some() {
            st.res.message = Some("attest needs --list".into());
            return finish(st, Some(exit::USAGE));
        }
        let Some(site_path) = p.scope.site_file.as_deref() else {
            st.res.message = Some("attest needs the site file path".into());
            return finish(st, Some(exit::USAGE));
        };
        match crate::attest::append(&crate::attest::Request {
            site,
            attest: conf,
            site_path: Path::new(site_path),
            list_path: Path::new(&p.list_path),
            lines,
            target: &target,
        }) {
            Ok(att) => {
                plan.input_attestation = Some(att.clone());
                st.res.input_attestation = Some(att);
            }
            Err(f) => {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
        }
    }
    if let Some(path) = opts.plan_out {
        if let Err(f) = write_plan(path, &plan) {
            st.res.message = Some(f.msg);
            return finish(st, Some(f.code));
        }
    }

    // Phases that are not configured are recorded, not failed.
    for ph in &plan.phases {
        match ph.status.as_str() {
            "not configured" => {
                log("INFO", &ph.name, &target, "not configured");
                st.res.not_configured.push(ph.name.clone());
                st.phase(&ph.name, "not configured", None, None);
            }
            "refused" => {
                let msg = ph.reason.clone().unwrap_or_default();
                log("ERROR", &ph.name, &target, &msg);
                st.phase(&ph.name, "refused", Some(exit::REFUSED), Some(msg.clone()));
                st.res.message = Some(msg);
                return finish(st, Some(exit::REFUSED));
            }
            _ => {}
        }
    }
    if let Some(a) = site
        .admission_on()
        .filter(|a| a.quota_cmd.is_some() && a.reservations.is_some())
    {
        match std::env::var(&a.job_id_env) {
            Ok(v) if valid_job_id(&v) => p.job_id = Some(v),
            _ => {
                let msg = format!(
                    "admission: {} is unset or not a positive integer without a leading zero",
                    a.job_id_env
                );
                log("ERROR", "preflight", &target, &msg);
                st.phase(
                    "preflight",
                    "refused",
                    Some(exit::REFUSED),
                    Some(msg.clone()),
                );
                st.res.message = Some(msg);
                return finish(st, Some(exit::REFUSED));
            }
        }
    }
    if let Some(sh) = site.shards_on().filter(|_| opts.shard_units.is_none()) {
        let mut vars = BTreeMap::new();
        for n in &sh.installed_modules_dirs_from_env {
            match std::env::var(n) {
                Ok(v) => {
                    vars.insert(n.clone(), v);
                }
                Err(_) => {
                    let msg = format!("shards.installed_modules_dirs_from_env: {n} is unset");
                    log("ERROR", "preflight", &target, &msg);
                    st.phase(
                        "preflight",
                        "refused",
                        Some(exit::REFUSED),
                        Some(msg.clone()),
                    );
                    st.res.message = Some(msg);
                    return finish(st, Some(exit::REFUSED));
                }
            }
        }
        p.runtime_env = Some(vars);
    }
    if let Some(node) = opts.oom_node {
        let units = match opts.shard_units {
            Some(units) => units.to_vec(),
            None => match p.select_lines(lines) {
                Ok((kept, _)) => vec![Unit {
                    name: "all".into(),
                    role: Role::Build,
                    lines: kept,
                    footprint: None,
                }],
                Err(f) => {
                    st.res.message = Some(f.msg);
                    return finish(st, Some(f.code));
                }
            },
        };
        if units.iter().any(|unit| !unit.lines.is_empty()) {
            match oom_preflight(p, &units, node, &log_dir) {
                Ok(0) => log(
                    "INFO",
                    "preflight",
                    &target,
                    &format!("eb-stack found no OOM risks for node {node}"),
                ),
                Ok(count) => {
                    let msg = format!(
                        "OOM preflight found {count} risk(s) for node {node}; build skipped"
                    );
                    log("ERROR", "preflight", &target, &msg);
                    st.phase(
                        "preflight",
                        "refused",
                        Some(exit::REFUSED),
                        Some(msg.clone()),
                    );
                    let mut stems = StemSeq::default();
                    for unit in &units {
                        for line in &unit.lines {
                            let stem = stems.next(&line.easyconfig);
                            st.res.lines.push(LineResult {
                                easyconfig: line.easyconfig.clone(),
                                unit: unit.name.clone(),
                                phase: "build".into(),
                                status: "skipped".into(),
                                exit_code: None,
                                translated_from: None,
                                log: p.log_stdout(&stem),
                                note: Some(msg.clone()),
                                duration_seconds: None,
                                outputs: Vec::new(),
                            });
                        }
                    }
                    st.phase("build", "skipped", None, Some(msg.clone()));
                    st.res.message = Some(msg);
                    return finish(st, Some(exit::REFUSED));
                }
                Err(f) => {
                    log("ERROR", "preflight", &target, &f.msg);
                    st.phase(
                        "preflight",
                        "failed",
                        Some(exit::FAILED),
                        Some(f.msg.clone()),
                    );
                    st.res.message = Some(f.msg);
                    return finish(st, Some(exit::FAILED));
                }
            }
        }
    }
    let p: &Planner = p;
    st.phase("preflight", "ok", None, None);
    if site.licences_on().is_some() {
        st.phase("licences", "ok", None, None);
    }
    let rotate_lock = match p.rotate_lock().map(|l| hold_rotate_lock(&mut st, &l)) {
        Some(Err(c)) => return finish(st, Some(c)),
        Some(Ok(f)) => f,
        None => None,
    };

    // selection: lines the target's selector leaves out are recorded, not built;
    // the kept lines, or the lines read from an easystack, go to a file that
    // the shard planner reads
    let (kept, deselected) = match p.select_lines(lines) {
        Ok(v) => v,
        Err(f) => {
            st.res.message = Some(f.msg);
            return finish(st, Some(f.code));
        }
    };
    let has_select = p.selected_list_path().is_some();
    if let Some(path) = p.selected_list_path().or_else(|| p.easystack_list_path()) {
        let mut text = kept
            .iter()
            .map(|l| l.raw.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if !kept.is_empty() {
            text.push('\n');
        }
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| util::write_atomic(&path, text.as_bytes()));
        if let Err(e) = written {
            let msg = format!("cannot write {}: {e}", path.display());
            log("ERROR", "preflight", &target, &msg);
            st.res.message = Some(msg);
            return finish(st, Some(exit::FAILED));
        }
    }
    if !deselected.is_empty() {
        log(
            "INFO",
            "build",
            &target,
            &format!("{} line(s) deselected for this target", deselected.len()),
        );
    }
    st.res.deselected = deselected;

    if site.resume_on().is_some() && !kept.is_empty() {
        match p.order_cmd(&kept) {
            Err(f) => {
                log("ERROR", "shard", &target, &f.msg);
                st.phase("shard", "failed", Some(f.code), Some(f.msg.clone()));
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
            Ok(cmd) => {
                let hashes = resume::hashes_path(&p.scope.log_dir());
                let ex = match st.exec("shard", &cmd, &env, &log_dir.join("order.log")) {
                    Ok(e) => e,
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                };
                if let Some(c) = stop_if_signalled(&mut st, "shard") {
                    return finish(st, Some(c));
                }
                if ex.code != 0 {
                    let code = if ex.code == 2 {
                        exit::USAGE
                    } else {
                        exit::FAILED
                    };
                    let msg = format!("stack order exited {} (see {})", ex.code, ex.log.display());
                    log("ERROR", "shard", &target, &msg);
                    st.phase("shard", "failed", Some(ex.code), Some(msg.clone()));
                    st.res.message = Some(msg);
                    return finish(st, Some(code));
                }
                match resume::digest_from_log(&ex.log, &hashes) {
                    Ok(dig) => {
                        if let Some(prior) = opts.resume {
                            if prior.digest != dig {
                                let msg = format!(
                                    "input digest mismatch: prior result has {}, this plan has {dig}",
                                    prior.digest
                                );
                                log("ERROR", "shard", &target, &msg);
                                st.phase(
                                    "shard",
                                    "refused",
                                    Some(exit::REFUSED),
                                    Some(msg.clone()),
                                );
                                st.res.message = Some(msg);
                                return finish(st, Some(exit::REFUSED));
                            }
                        }
                        log("INFO", "shard", &target, "recorded the input identity");
                        st.res.input_digest = Some(dig);
                    }
                    Err(f) => {
                        let status = if f.code == exit::REFUSED {
                            "refused"
                        } else {
                            "failed"
                        };
                        log("ERROR", "shard", &target, &f.msg);
                        st.phase("shard", status, Some(f.code), Some(f.msg.clone()));
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                }
            }
        }
    }

    // shard
    let mut units: Vec<Unit> = if let Some(saved) = opts.shard_units {
        st.res.sharded = true;
        st.phase(
            "shard",
            "ok",
            Some(0),
            Some("reused the supplied shard plan".into()),
        );
        saved.to_vec()
    } else if has_select && kept.is_empty() {
        vec![]
    } else {
        vec![Unit {
            name: "all".into(),
            role: Role::Build,
            lines: kept,
            footprint: None,
        }]
    };
    if let Some(sh) = site
        .shards_on()
        .filter(|_| opts.shard_units.is_none() && !units.is_empty())
    {
        match p.shard_cmd() {
            Err(f) => {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
            Ok(cmd) => {
                for d in p.missing_installed_modules_dirs() {
                    log(
                        "WARN",
                        "shard",
                        &target,
                        &format!("{d} does not exist; no module counts as installed there"),
                    );
                }
                log(
                    "INFO",
                    "shard",
                    &target,
                    &format!("planning {} shards", sh.count),
                );
                let ex = match st.exec("shard", &cmd, &env, &log_dir.join("shard.log")) {
                    Ok(e) => e,
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                };
                if let Some(c) = stop_if_signalled(&mut st, "shard") {
                    return finish(st, Some(c));
                }
                if ex.code == 0 {
                    match read_units(Path::new(&p.shard_dir())) {
                        Ok(u) if !u.is_empty() => {
                            units = u;
                            st.res.sharded = true;
                            st.phase("shard", "ok", Some(0), None);
                        }
                        Ok(_) => {
                            st.phase(
                                "shard",
                                "ok",
                                Some(0),
                                Some("plan holds no lines; building the list as one unit".into()),
                            );
                        }
                        Err(f) => {
                            log("ERROR", "shard", &target, &f.msg);
                            st.phase("shard", "failed", Some(0), Some(f.msg.clone()));
                            st.res.message = Some(f.msg);
                            return finish(st, Some(exit::FAILED));
                        }
                    }
                } else if ex.code == sh.planner_exit_unsharded {
                    log(
                        "INFO",
                        "shard",
                        &target,
                        &format!(
                            "planner declined to shard (exit {}); building unsharded",
                            ex.code
                        ),
                    );
                    st.phase(
                        "shard",
                        "unsharded",
                        Some(ex.code),
                        Some(format!(
                            "exit {} is shards.planner_exit_unsharded: planner declined to shard; plan built unsharded",
                            ex.code
                        )),
                    );
                } else {
                    let code = if ex.code == 2 {
                        exit::USAGE
                    } else {
                        exit::FAILED
                    };
                    let msg = format!("stack shard exited {} (see {})", ex.code, ex.log.display());
                    log("ERROR", "shard", &target, &msg);
                    st.phase("shard", "failed", Some(ex.code), Some(msg.clone()));
                    st.res.message = Some(msg);
                    return finish(st, Some(code));
                }
            }
        }
    } else if site.resume_on().is_some_and(|r| r.shards >= 2)
        && opts.shard_units.is_none()
        && !units.is_empty()
    {
        let count = site.resume_on().map(|r| r.shards).unwrap_or(1);
        let unsharded = site
            .shards
            .as_ref()
            .map(|s| s.planner_exit_unsharded)
            .unwrap_or(3);
        match p.boundary_shard_cmd() {
            Err(f) => {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
            Ok(cmd) => {
                log(
                    "INFO",
                    "shard",
                    &target,
                    &format!("planning {count} shards"),
                );
                let ex = match st.exec("shard", &cmd, &env, &log_dir.join("shard.log")) {
                    Ok(e) => e,
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                };
                if let Some(c) = stop_if_signalled(&mut st, "shard") {
                    return finish(st, Some(c));
                }
                if ex.code == 0 {
                    match read_units(Path::new(&p.shard_dir())) {
                        Ok(u) if !u.is_empty() => {
                            units = u;
                            st.res.sharded = true;
                            st.phase("shard", "ok", Some(0), None);
                        }
                        Ok(_) => {
                            st.phase(
                                "shard",
                                "ok",
                                Some(0),
                                Some("plan holds no lines; building the list as one unit".into()),
                            );
                        }
                        Err(f) => {
                            log("ERROR", "shard", &target, &f.msg);
                            st.phase("shard", "failed", Some(0), Some(f.msg.clone()));
                            st.res.message = Some(f.msg);
                            return finish(st, Some(exit::FAILED));
                        }
                    }
                } else if ex.code == unsharded {
                    log(
                        "INFO",
                        "shard",
                        &target,
                        &format!(
                            "planner declined to shard (exit {}); building unsharded",
                            ex.code
                        ),
                    );
                    st.phase(
                        "shard",
                        "unsharded",
                        Some(ex.code),
                        Some(format!(
                            "exit {code} means the plan was built unsharded; plan built unsharded",
                            code = ex.code
                        )),
                    );
                } else {
                    let code = if ex.code == 2 {
                        exit::USAGE
                    } else {
                        exit::FAILED
                    };
                    let msg = format!("stack shard exited {} (see {})", ex.code, ex.log.display());
                    log("ERROR", "shard", &target, &msg);
                    st.phase("shard", "failed", Some(ex.code), Some(msg.clone()));
                    st.res.message = Some(msg);
                    return finish(st, Some(code));
                }
            }
        }
    }
    if site.resume_on().is_some()
        && st.res.input_digest.is_some()
        && !st.res.phases.iter().any(|ph| ph.name == "shard")
    {
        st.phase(
            "shard",
            "ok",
            Some(0),
            Some("recorded the input identity".into()),
        );
    }

    let skip: BTreeSet<String> = match opts.resume {
        Some(prior) => resume::units_to_skip(
            units.iter().map(|u| (u.name.as_str(), u.lines.as_slice())),
            prior,
        ),
        None => BTreeSet::new(),
    };

    // the route's receipt digest is read once a CPU-role unit needs it
    let mut routed = p.clone();
    if units.iter().any(|u| u.role == Role::Cpu) {
        if let Some(n) = p
            .scope
            .target
            .route
            .as_ref()
            .filter(|r| r.kind == "command")
            .and_then(|r| r.receipt_digest_env.as_ref())
        {
            match std::env::var(n) {
                Ok(v) if !v.is_empty() => routed.route_digest = Some(v),
                _ => {
                    let msg = format!("route.receipt_digest_env: {n} is unset or empty");
                    log("ERROR", "build", &target, &msg);
                    st.phase("build", "refused", Some(exit::REFUSED), Some(msg.clone()));
                    st.res.message = Some(msg);
                    return finish(st, Some(exit::REFUSED));
                }
            }
        }
    }
    let p = &routed;

    // sources: prefetch every line once; a failed fetch fails the line.
    let mut stems = StemSeq::default();
    let mut unit_stems: Vec<Vec<String>> = Vec::new();
    for u in &units {
        unit_stems.push(u.lines.iter().map(|l| stems.next(&l.easyconfig)).collect());
    }
    let mut fetch_failed: Vec<String> = Vec::new();
    if site.sources_on().is_some() {
        let mut any_failed = false;
        let bundle_lines: Vec<ListLine> = units
            .iter()
            .flat_map(|unit| unit.lines.iter().cloned())
            .collect();
        let planner_cmds = match p.offline_planner_cmds(&bundle_lines) {
            Ok(commands) => commands,
            Err(f) => {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
        };
        if let Some(bundle) = &p.offline_bundle {
            if let Err(f) = crate::offline::prepare(bundle) {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
            if let Err(e) = std::fs::create_dir(bundle.join("planner")) {
                let msg = format!(
                    "cannot create offline planner directory {}: {e}",
                    bundle.join("planner").display()
                );
                st.res.message = Some(msg);
                return finish(st, Some(exit::FAILED));
            }
        }
        for (i, cmd) in planner_cmds.iter().enumerate() {
            let ex = match st.exec(
                "sources",
                cmd,
                &env,
                &log_dir.join(format!("offline-planner-{:02}.log", i + 1)),
            ) {
                Ok(ex) => ex,
                Err(f) => {
                    st.res.message = Some(f.msg);
                    return finish(st, Some(f.code));
                }
            };
            if let Some(c) = stop_if_signalled(&mut st, "sources") {
                return finish(st, Some(c));
            }
            if ex.code != 0 {
                let msg = format!("{} exited {} (see {})", cmd.name, ex.code, ex.log.display());
                log("ERROR", "sources", &target, &msg);
                st.phase("sources", "failed", Some(ex.code), Some(msg.clone()));
                st.res.message = Some(msg);
                return finish(st, Some(exit::FAILED));
            }
            if let Some(bundle) = &p.offline_bundle {
                let copy = bundle.join("planner").join(format!(
                    "call-{:02}-{}.log",
                    i + 1,
                    cmd.name.replace(':', "-")
                ));
                if let Err(e) = std::fs::copy(&ex.log, &copy) {
                    let msg = format!("cannot copy planner log to {}: {e}", copy.display());
                    st.res.message = Some(msg);
                    return finish(st, Some(exit::FAILED));
                }
            }
        }
        for (u, ss) in units.iter().zip(&unit_stems) {
            if skip.contains(&u.name) && p.offline_bundle.is_none() {
                continue;
            }
            for (l, s) in u.lines.iter().zip(ss) {
                make_route_dirs(p, u.role, "sources");
                let cmd = match p.prefetch_cmd(l, s, u.role) {
                    Ok(c) => c,
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                };
                let ex = match st.exec(
                    "sources",
                    &cmd,
                    &env,
                    &log_dir.join(format!("{s}.fetch.log")),
                ) {
                    Ok(e) => e,
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                };
                if let Some(c) = stop_if_signalled(&mut st, "sources") {
                    st.res.lines.push(LineResult {
                        easyconfig: l.easyconfig.clone(),
                        unit: u.name.clone(),
                        phase: "sources".into(),
                        status: "failed".into(),
                        exit_code: Some(c),
                        translated_from: None,
                        log: ex.log.display().to_string(),
                        note: Some("source fetch interrupted".into()),
                        duration_seconds: Some(seconds(ex.elapsed)),
                        outputs: Vec::new(),
                    });
                    st.failed = true;
                    fetch_failed.push(s.clone());
                    note_skipped(&mut st, &units, &unit_stems, &fetch_failed, 0, 0, p);
                    return finish(st, Some(c));
                }
                if ex.code != 0 {
                    any_failed = true;
                    fetch_failed.push(s.clone());
                    log(
                        "ERROR",
                        "sources",
                        &target,
                        &format!("{s}: fetch exited {}", ex.code),
                    );
                    st.res.lines.push(LineResult {
                        easyconfig: l.easyconfig.clone(),
                        unit: u.name.clone(),
                        phase: "sources".into(),
                        status: "failed".into(),
                        exit_code: Some(ex.code),
                        translated_from: None,
                        log: ex.log.display().to_string(),
                        note: Some("source fetch failed; line not built".into()),
                        duration_seconds: Some(seconds(ex.elapsed)),
                        outputs: Vec::new(),
                    });
                    st.failed = true;
                }
            }
        }
        if !any_failed {
            if let Some(bundle) = &p.offline_bundle {
                let assets = &site.sources_on().expect("sources active").assets;
                match crate::offline::write(bundle, &p.scope, assets) {
                    Ok(manifest) => log(
                        "INFO",
                        "sources",
                        &target,
                        &format!(
                            "wrote offline bundle {} with {} file(s)",
                            bundle.display(),
                            manifest.files.len()
                        ),
                    ),
                    Err(f) => {
                        log("ERROR", "sources", &target, &f.msg);
                        st.phase("sources", "failed", Some(exit::FAILED), Some(f.msg.clone()));
                        st.res.message = Some(f.msg);
                        return finish(st, Some(exit::FAILED));
                    }
                }
            }
        }
        st.phase(
            "sources",
            if any_failed { "failed" } else { "ok" },
            None,
            None,
        );
        if any_failed && p.offline_bundle.is_some() {
            st.res.message = Some("source prefetch failed; no offline bundle was published".into());
            return finish(st, Some(exit::FAILED));
        }
    }

    // admission and builds, unit by unit
    let adm = site
        .admission_on()
        .filter(|a| a.quota_cmd.is_some() && a.reservations.is_some());
    let keep_failed = site.logs_on().is_some_and(|l| l.keep_failed);
    let mut stopped = false;
    for (ui, (u, ss)) in units.iter().zip(&unit_stems).enumerate() {
        if stopped {
            for (l, s) in u.lines.iter().zip(ss) {
                st.res.lines.push(LineResult {
                    easyconfig: l.easyconfig.clone(),
                    unit: u.name.clone(),
                    phase: "build".into(),
                    status: "skipped".into(),
                    exit_code: None,
                    translated_from: None,
                    log: p.log_stdout(s),
                    note: Some("an earlier unit was refused".into()),
                    duration_seconds: None,
                    outputs: Vec::new(),
                });
            }
            continue;
        }
        if skip.contains(&u.name) {
            for (l, s) in u.lines.iter().zip(ss) {
                let outputs = opts
                    .resume
                    .map(|pr| resume::outputs_for(pr, &l.easyconfig))
                    .unwrap_or_default();
                log(
                    "INFO",
                    "build",
                    &target,
                    &format!("{s}: resumed; outputs still present"),
                );
                st.res.lines.push(LineResult {
                    easyconfig: l.easyconfig.clone(),
                    unit: u.name.clone(),
                    phase: "build".into(),
                    status: "skipped".into(),
                    exit_code: None,
                    translated_from: None,
                    log: p.log_stdout(s),
                    note: Some(
                        "resumed: the unit succeeded and its outputs are still present".into(),
                    ),
                    duration_seconds: None,
                    outputs,
                });
            }
            continue;
        }
        if let Some(a) = adm {
            let fp = u.footprint.or(a.default_footprint).unwrap_or(0);
            let cmd = match p.admission_cmd(&u.name, fp) {
                Ok(c) => c,
                Err(f) => {
                    st.res.message = Some(f.msg);
                    return finish(st, Some(f.code));
                }
            };
            let mut attempt = 0u32;
            let mut wait = capped_wait(a.wait_seconds, a.max_wait_seconds);
            let outcome = loop {
                attempt += 1;
                let ex = match st.exec(
                    "admission",
                    &cmd,
                    &env,
                    &log_dir.join(format!("admission-{}.{attempt}.log", u.name)),
                ) {
                    Ok(e) => e,
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                };
                if let Some(c) = stop_if_signalled(&mut st, "admission") {
                    return finish(st, Some(c));
                }
                let last = last_json_line(&ex.log);
                match ex.code {
                    0 => {
                        log(
                            "INFO",
                            "admission",
                            &target,
                            &format!("{} admitted on attempt {attempt}", u.name),
                        );
                        break ("admitted", None, last, attempt, 0);
                    }
                    78 => {
                        if attempt >= a.attempts {
                            break ("refused", None, last, attempt, 78);
                        }
                        log(
                            "WARN",
                            "admission",
                            &target,
                            &format!(
                                "{} refused on attempt {attempt} of {}; waiting {wait}s",
                                u.name, a.attempts
                            ),
                        );
                        if sleep_interruptible(wait) {
                            if let Some(c) = stop_if_signalled(&mut st, "admission") {
                                return finish(st, Some(c));
                            }
                        }
                        wait = doubled_wait(wait, a.max_wait_seconds);
                    }
                    c => {
                        let code = if c == 2 { exit::USAGE } else { exit::FAILED };
                        break (
                            "failed",
                            Some(format!("stack admit exited {c}")),
                            last,
                            attempt,
                            code,
                        );
                    }
                }
            };
            let (kind, note, last, attempts, code) = outcome;
            st.res.admission.push(AdmissionResult {
                unit: u.name.clone(),
                attempts,
                outcome: kind.into(),
                last_json: last.clone(),
            });
            if kind == "admitted" {
                st.reservation = p.job_id.as_ref().map(|job| Reservation {
                    path: Path::new(a.reservations.as_deref().unwrap_or_default())
                        .join(format!("{job}.json")),
                    job: job.clone(),
                    lock: a.lock.as_ref().map(PathBuf::from),
                    posix: a.lock_kind == "posix",
                });
            }
            if kind != "admitted" {
                let msg = if kind == "refused" {
                    format!(
                        "admission refused after {attempts} attempts: {}",
                        last.clone().unwrap_or_else(|| "(no JSON line)".into())
                    )
                } else {
                    format!("admission failed: {}", note.unwrap_or_default())
                };
                log("ERROR", "admission", &target, &msg);
                st.phase("admission", kind, Some(code), Some(msg.clone()));
                st.res.message = Some(msg);
                if kind == "refused" {
                    st.refused = true;
                    stopped = true;
                    for (l, s) in u.lines.iter().zip(ss) {
                        st.res.lines.push(LineResult {
                            easyconfig: l.easyconfig.clone(),
                            unit: u.name.clone(),
                            phase: "build".into(),
                            status: "skipped".into(),
                            exit_code: None,
                            translated_from: None,
                            log: p.log_stdout(s),
                            note: Some("admission refused".into()),
                            duration_seconds: None,
                            outputs: Vec::new(),
                        });
                    }
                    continue;
                }
                return finish(st, Some(code));
            }
        }

        for (li, (l, s)) in u.lines.iter().zip(ss).enumerate() {
            if fetch_failed.contains(s) {
                continue;
            }
            if interrupted().is_some() {
                note_skipped(&mut st, &units, &unit_stems, &fetch_failed, ui, li, p);
                break;
            }
            make_route_dirs(p, u.role, "build");
            // Before the build, which may install the module file it looks for.
            let staged = p.is_staged(l);
            let cmd = match p.build_cmd(u.role, l, s) {
                Ok(c) => c,
                Err(f) => {
                    st.res.message = Some(f.msg);
                    return finish(st, Some(f.code));
                }
            };
            if let Some(w) = p.in_place_reason(l) {
                log("WARN", "build", &target, &format!("{s}: {w}"));
            }
            log("INFO", "build", &target, &format!("{s}: start"));
            let ex = match st.exec("build", &cmd, &env, Path::new(&p.log_stdout(s))) {
                Ok(e) => e,
                Err(f) => {
                    st.res.message = Some(f.msg);
                    return finish(st, Some(f.code));
                }
            };
            let mut lr = LineResult {
                easyconfig: l.easyconfig.clone(),
                unit: u.name.clone(),
                phase: "build".into(),
                status: "success".into(),
                exit_code: Some(ex.code),
                translated_from: None,
                log: ex.log.display().to_string(),
                note: staged.then(|| "ran through eb-staged-rebuild".into()),
                duration_seconds: Some(seconds(ex.elapsed)),
                outputs: Vec::new(),
            };
            if ex.code != 0 {
                let outcome = if staged {
                    map_staged(ex.code)
                } else {
                    StagedOutcome::Failed
                };
                let mut detail = None;
                match outcome {
                    StagedOutcome::Refused => {
                        lr.status = "refused".into();
                        lr.translated_from = Some(ex.code);
                        lr.exit_code = Some(exit::REFUSED);
                        detail = lock_refusal(p, &cmd, &env, &ex.log);
                        lr.note = Some(match &detail {
                            Some(d) => {
                                format!("eb-staged-rebuild exit 2: {d}; translated to refused (78)")
                            }
                            None => {
                                "eb-staged-rebuild exit 2 is a refusal; translated to refused (78)"
                                    .into()
                            }
                        });
                        st.refused = true;
                    }
                    StagedOutcome::Pending => {
                        lr.status = "pending".into();
                        lr.note = Some(
                            "eb-staged-rebuild exit 75: a required ledger event is pending".into(),
                        );
                        st.pending = true;
                    }
                    _ => {
                        lr.status = "failed".into();
                        st.failed = true;
                    }
                }
                let detail = detail.map(|d| format!(": {d}")).unwrap_or_default();
                log(
                    "ERROR",
                    "build",
                    &target,
                    &format!("{s}: {} (exit {}){detail}", lr.status, ex.code),
                );
                if interrupted().is_none() && keep_failed && lr.status == "failed" {
                    if let Ok(kc) = p.keep_failed_cmd(s, &ex.code.to_string()) {
                        let kx = st.exec(
                            "logs",
                            &kc,
                            &env,
                            &log_dir.join(format!("{s}.keep-logs.log")),
                        );
                        match kx {
                            Ok(k) if k.code == 0 => {}
                            Ok(k) => log(
                                "WARN",
                                "logs",
                                &target,
                                &format!("{s}: eb-keep-logs failed exited {}", k.code),
                            ),
                            Err(f) => log("WARN", "logs", &target, &f.msg),
                        }
                    }
                }
            } else {
                if site.resume_on().is_some() {
                    lr.outputs = p.declared_outputs(l);
                }
                log("INFO", "build", &target, &format!("{s}: ok"));
            }
            st.res.lines.push(lr);
            if interrupted().is_some() {
                note_skipped(&mut st, &units, &unit_stems, &fetch_failed, ui, li + 1, p);
                break;
            }
        }
        if let Some(r) = st.reservation.take() {
            release(r, &target);
        }
        if interrupted().is_some() {
            break;
        }
    }
    if let Some(c) = stop_if_signalled(&mut st, "build") {
        return finish(st, Some(c));
    }
    if site.admission_on().is_some() && !st.res.admission.is_empty() && !st.refused {
        st.phase("admission", "ok", Some(0), None);
    }
    let build: Vec<&str> = st
        .res
        .lines
        .iter()
        .filter(|l| l.phase == "build")
        .map(|l| l.status.as_str())
        .collect();
    let (status, note) = if build.contains(&"failed") {
        ("failed", None)
    } else if st.refused && build.iter().all(|s| *s == "skipped") {
        (
            "skipped",
            Some("admission refused before any build line started".to_string()),
        )
    } else if st.refused && !build.contains(&"success") {
        ("refused", None)
    } else {
        ("ok", None)
    };
    st.phase("build", status, None, note);

    // A site test command sees the tree the build left, including a staged swap.
    let any_staged = match p.any_staged(lines) {
        Ok(v) => v,
        Err(f) => {
            st.res.message = Some(f.msg);
            return finish(st, Some(f.code));
        }
    };
    match p.check_cmd(any_staged) {
        Ok(Some(cmd)) => {
            if !st.failed && !st.refused && !st.pending {
                log("INFO", "checks", &target, "start");
                match st.exec("checks", &cmd, &env, &log_dir.join("checks.log")) {
                    Ok(ex) => {
                        if ex.code == 0 {
                            st.phase("checks", "ok", Some(0), None);
                        } else {
                            let msg = format!("check command exited {}", ex.code);
                            log("ERROR", "checks", &target, &msg);
                            st.phase("checks", "failed", Some(ex.code), Some(msg));
                            st.failed = true;
                        }
                    }
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                }
                if let Some(c) = stop_if_signalled(&mut st, "checks") {
                    return finish(st, Some(c));
                }
            } else {
                st.phase(
                    "checks",
                    "skipped",
                    None,
                    Some("the build did not finish cleanly".into()),
                );
            }
        }
        Ok(None) => {}
        Err(f) => {
            st.res.message = Some(f.msg);
            return finish(st, Some(f.code));
        }
    }

    if p.package_plan.is_some() && p.verify_package_plan {
        match p.verification_cmds() {
            Ok(commands) if !st.failed && !st.refused && !st.pending => {
                let mut failed = None;
                for (index, cmd) in commands.iter().enumerate() {
                    log(
                        "INFO",
                        "verification",
                        &target,
                        &format!("{}: start", cmd.name),
                    );
                    match st.exec(
                        "verification",
                        cmd,
                        &env,
                        &log_dir.join(format!("verification-{}.log", index + 1)),
                    ) {
                        Ok(ex) if ex.code == 0 => {
                            log(
                                "INFO",
                                "verification",
                                &target,
                                &format!("{}: ok", cmd.name),
                            );
                        }
                        Ok(ex) => {
                            let message = format!("{} exited {}", cmd.name, ex.code);
                            log("ERROR", "verification", &target, &message);
                            failed.get_or_insert((ex.code, message));
                        }
                        Err(f) => {
                            st.res.message = Some(f.msg);
                            return finish(st, Some(f.code));
                        }
                    }
                    if let Some(c) = stop_if_signalled(&mut st, "verification") {
                        return finish(st, Some(c));
                    }
                }
                match failed {
                    Some((code, message)) => {
                        st.phase("verification", "failed", Some(code), Some(message));
                        st.failed = true;
                    }
                    None => st.phase("verification", "ok", Some(0), None),
                }
            }
            Ok(_) => st.phase(
                "verification",
                "skipped",
                None,
                Some("the build did not finish cleanly".into()),
            ),
            Err(f) => {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
        }
    }

    // caches only after a clean build
    if !st.failed && !st.refused && !st.pending {
        match p.cache_cmd() {
            Ok(Some(cmd)) => {
                log("INFO", "caches", &target, "start");
                match st.exec("caches", &cmd, &env, &log_dir.join("caches.log")) {
                    Ok(ex) => {
                        if ex.code == 0 {
                            st.phase("caches", "ok", Some(0), None);
                        } else {
                            let msg = format!("cache command exited {}", ex.code);
                            log("ERROR", "caches", &target, &msg);
                            st.phase("caches", "failed", Some(ex.code), Some(msg));
                            st.failed = true;
                        }
                    }
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                }
                if let Some(c) = stop_if_signalled(&mut st, "caches") {
                    return finish(st, Some(c));
                }
            }
            Ok(None) => {}
            Err(f) => {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
        }
    } else if p.cache_cmd().ok().flatten().is_some() {
        st.phase(
            "caches",
            "skipped",
            None,
            Some("the build did not finish cleanly".into()),
        );
    }

    // rotation, by a run that holds the rotation lock alone
    if let Ok(Some(cmd)) = p.rotate_cmd(true) {
        let lock = p.rotate_lock().unwrap_or_default();
        match rotate_lock.map(rotate_alone) {
            Some(Ok(None)) => {
                let msg = format!(
                    "another run under {} holds {}, live or rotating; the rotation is left to a run that ends alone",
                    site.filesystems.log_root,
                    lock.display()
                );
                log("INFO", "logs", &target, &format!("rotation skipped: {msg}"));
                st.phase("logs", "skipped", None, Some(msg));
            }
            alone => {
                if let Some(Err(e)) = &alone {
                    let msg = format!(
                        "cannot take {} alone: {e}; rotating without it",
                        lock.display()
                    );
                    log("WARN", "logs", &target, &msg);
                }
                let bin = Tool::EbKeepLogs.bin(site);
                let mut probe_env = env.clone();
                probe_env.extend(cmd.env.clone());
                let note = match help_lists(&bin, &probe_env, "--lock-timeout") {
                    Ok(true) => None,
                    Ok(false) => Some(format!(
                        "`{bin} --help` does not list it, as in eb-keep-logs 0.2.1, which takes no lock of its own"
                    )),
                    Err(e) => Some(e),
                }
                .map(|why| format!("no --lock-timeout for eb-keep-logs rotate: {why}"));
                let cmd = match &note {
                    None => cmd,
                    Some(n) => {
                        log("INFO", "logs", &target, n);
                        p.rotate_cmd(false).ok().flatten().unwrap_or(cmd)
                    }
                };
                match st.exec("logs", &cmd, &env, &log_dir.join("rotate.log")) {
                    Ok(ex) => match map_rotate(ex.code) {
                        RotateOutcome::Ok => st.phase("logs", "ok", Some(0), note),
                        RotateOutcome::Locked => {
                            let msg = format!(
                                "eb-keep-logs rotate exited 75: another rotate holds {}, and this one changed nothing; the rotation is left to it",
                                p.keep_logs_lock().display()
                            );
                            log("INFO", "logs", &target, &format!("rotation skipped: {msg}"));
                            st.phase("logs", "skipped", Some(ex.code), Some(msg));
                        }
                        RotateOutcome::Failed => {
                            let msg = format!("eb-keep-logs rotate exited {}", ex.code);
                            log("ERROR", "logs", &target, &msg);
                            st.phase("logs", "failed", Some(ex.code), Some(msg));
                            st.failed = true;
                        }
                    },
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                }
                if let Some(c) = stop_if_signalled(&mut st, "logs") {
                    return finish(st, Some(c));
                }
            }
        }
    }

    finish(st, None)
}

/// Lines from `(from_unit, from_line)` onward that have not started, recorded
/// as skipped. A fetch that already failed stays the fetch result.
fn note_skipped(
    st: &mut State,
    units: &[Unit],
    stems: &[Vec<String>],
    fetch_failed: &[String],
    from_unit: usize,
    from_line: usize,
    p: &Planner,
) {
    for (ui, (u, ss)) in units.iter().zip(stems).enumerate().skip(from_unit) {
        let start = if ui == from_unit { from_line } else { 0 };
        for (l, s) in u.lines.iter().zip(ss).skip(start) {
            if fetch_failed.iter().any(|f| f == s) {
                continue;
            }
            st.res.lines.push(LineResult {
                easyconfig: l.easyconfig.clone(),
                unit: u.name.clone(),
                phase: "build".into(),
                status: "skipped".into(),
                exit_code: None,
                translated_from: None,
                log: p.log_stdout(s),
                note: Some("interrupted before this line started".into()),
                duration_seconds: None,
                outputs: Vec::new(),
            });
        }
    }
}

/// Create the directories a routed command expects; a failure surfaces when
/// the command itself fails.
fn make_route_dirs(p: &Planner, role: Role, phase: &str) {
    if role == Role::Cpu {
        for d in p.scope.route_dirs(phase) {
            let _ = std::fs::create_dir_all(d);
        }
    }
}

fn stop_if_signalled(st: &mut State, phase: &str) -> Option<i32> {
    let sig = interrupted()?;
    let (name, code) = if sig == libc::SIGINT {
        ("SIGINT", exit::SIGINT)
    } else {
        ("SIGTERM", exit::SIGTERM)
    };
    log(
        "WARN",
        phase,
        &st.res.target.clone(),
        &format!("{name} received; child ended, stopping"),
    );
    st.res.interrupted_by = Some(name.into());
    st.phase(phase, "interrupted", None, None);
    st.forced_code = Some(code);
    Some(code)
}

fn finish(mut st: State, forced: Option<i32>) -> RunResult {
    if let Some(r) = st.reservation.take() {
        release(r, &st.res.target);
    }
    st.res.ended_at = chrono::Utc::now().to_rfc3339();
    st.res.duration_seconds = seconds(st.began.elapsed());
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for l in &st.res.lines {
        *counts.entry(l.status.clone()).or_default() += 1;
    }
    st.res.counts = counts;
    st.res.phases.sort_by_key(|ph| {
        PHASES
            .iter()
            .position(|n| *n == ph.name)
            .unwrap_or(PHASES.len())
    });
    st.res.exit_code = match forced.or(st.forced_code) {
        Some(c) => c,
        None if st.failed => exit::FAILED,
        None if st.refused => exit::REFUSED,
        None if st.pending => exit::TEMPFAIL,
        None => exit::OK,
    };
    st.res
}

#[cfg(test)]
mod tests {
    use super::{capped_wait, doubled_wait};

    #[test]
    fn admission_wait_never_exceeds_the_configured_maximum() {
        assert_eq!(capped_wait(60, 600), 60);
        assert_eq!(doubled_wait(60, 600), 120);
        assert_eq!(doubled_wait(480, 600), 600);
        assert_eq!(doubled_wait(600, 600), 600);
        // A maximum below the initial wait is still the maximum.
        assert_eq!(capped_wait(120, 50), 50);
        assert_eq!(doubled_wait(50, 50), 50);
        assert_eq!(doubled_wait(120, 50), 50);
        assert_eq!(capped_wait(5, 0), 0);
    }
}
