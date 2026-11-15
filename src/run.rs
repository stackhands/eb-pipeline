//! Execution of a plan as subprocesses.

use crate::exit::{self, Fail};
use crate::plan::{build_plan, shell_line, Cmd, Planner, StemSeq, Unit};
use crate::resolve::Role;
use crate::tools::{map_staged, StagedOutcome};
use crate::util::{parse_list, ListLine};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

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

pub fn log(level: &str, phase: &str, target: &str, msg: &str) {
    eprintln!("{level} [{phase}] {target}: {msg}");
}

#[derive(Debug, Serialize, Clone)]
pub struct PhaseResult {
    pub name: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct LineResult {
    pub easyconfig: String,
    pub unit: String,
    pub phase: String,
    /// `success`, `failed`, `refused`, `pending`, `skipped`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translated_from: Option<i32>,
    pub log: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct AdmissionResult {
    pub unit: String,
    pub attempts: u32,
    /// `admitted`, `refused` or `failed`.
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_json: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct RunResult {
    pub schema_version: u32,
    pub kind: String,
    pub site: String,
    pub target: String,
    pub build_type: String,
    pub run_id: String,
    pub list: String,
    pub started_at: String,
    pub ended_at: String,
    pub exit_code: i32,
    pub sharded: bool,
    pub counts: BTreeMap<String, usize>,
    pub not_configured: Vec<String>,
    pub admission: Vec<AdmissionResult>,
    pub phases: Vec<PhaseResult>,
    pub lines: Vec<LineResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interrupted_by: Option<String>,
}

struct Exec {
    code: i32,
    log: PathBuf,
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
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => {
            let _ = std::fs::write(log, format!("cannot start {}: {e}\n", cmd.argv[0]));
            return Ok(Exec {
                code: 127,
                log: log.to_path_buf(),
            });
        }
    };
    let mut forwarded = 0;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => {
                return Err(Fail::new(exit::FAILED, format!("wait failed: {e}")));
            }
        }
        if let Some(sig) = interrupted() {
            if forwarded != sig {
                // SAFETY: the pid belongs to a child this process started.
                unsafe {
                    libc::kill(child.id() as libc::pid_t, sig);
                }
                forwarded = sig;
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
    })
}

fn last_json_line(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with('{') && l.ends_with('}'))
        .map(str::to_string)
}

pub struct Options {
    pub dry_run: bool,
}

struct State {
    res: RunResult,
    refused: bool,
    pending: bool,
    failed: bool,
    forced_code: Option<i32>,
}

impl State {
    fn phase(&mut self, name: &str, status: &str, code: Option<i32>, note: Option<String>) {
        self.res.phases.push(PhaseResult {
            name: name.into(),
            status: status.into(),
            exit_code: code,
            note,
        });
    }
}

fn read_units(dir: &Path) -> Result<Vec<Unit>, Fail> {
    let plan_path = dir.join("plan.json");
    let text = std::fs::read_to_string(&plan_path).map_err(|e| {
        Fail::new(
            exit::FAILED,
            format!("cannot read {}: {e}", plan_path.display()),
        )
    })?;
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| Fail::new(exit::FAILED, format!("{}: {e}", plan_path.display())))?;
    let mut units = Vec::new();
    let mut add = |name: String, role: Role, entry: &serde_json::Value| -> Result<(), Fail> {
        let Some(file) = entry["file"].as_str() else {
            return Ok(());
        };
        let list = std::fs::read_to_string(dir.join(file)).map_err(|e| {
            Fail::new(
                exit::FAILED,
                format!("cannot read {}: {e}", dir.join(file).display()),
            )
        })?;
        let lines = parse_list(&list)?;
        if lines.is_empty() {
            return Ok(());
        }
        units.push(Unit {
            name,
            role,
            lines,
            footprint: entry["inode_footprint"].as_u64(),
        });
        Ok(())
    };
    add(
        "cpu-prerequisites".into(),
        Role::Cpu,
        &v["cpu_prerequisites"],
    )?;
    add("prelude".into(), Role::Build, &v["prelude"])?;
    if let Some(shards) = v["shards"].as_array() {
        for (i, s) in shards.iter().enumerate() {
            let name = s["name"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| format!("shard-{:02}", i + 1));
            add(name, Role::Build, s)?;
        }
    }
    Ok(units)
}

/// Execute the plan. Returns the result; its `exit_code` is the process
/// exit code.
pub fn run(p: &Planner, lines: &[ListLine], opts: &Options) -> RunResult {
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
            started_at: chrono::Utc::now().to_rfc3339(),
            ended_at: String::new(),
            exit_code: 0,
            sharded: false,
            counts: BTreeMap::new(),
            not_configured: vec![],
            admission: vec![],
            phases: vec![],
            lines: vec![],
            message: None,
            interrupted_by: None,
        },
        refused: false,
        pending: false,
        failed: false,
        forced_code: None,
    };

    let plan = match build_plan(p, lines) {
        Ok(pl) => pl,
        Err(f) => {
            st.res.message = Some(f.msg.clone());
            return finish(st, Some(f.code));
        }
    };

    if opts.dry_run {
        for ph in &plan.phases {
            if ph.status != "planned" {
                println!("# [{}] {}", ph.name, ph.status);
                continue;
            }
            for c in &ph.commands {
                println!("[{}] {}", ph.name, shell_line(c));
            }
        }
        log("INFO", "run", &target, "dry run: nothing executed");
        return finish(st, None);
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
    st.phase("preflight", "ok", None, None);
    if site.licences_on().is_some() {
        st.phase("licences", "ok", None, None);
    }

    // shard
    let mut units: Vec<Unit> = vec![Unit {
        name: "all".into(),
        role: Role::Build,
        lines: lines.to_vec(),
        footprint: None,
    }];
    if let Some(sh) = site.shards_on() {
        match p.shard_cmd() {
            Err(f) => {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
            Ok(cmd) => {
                log(
                    "INFO",
                    "shard",
                    &target,
                    &format!("planning {} shards", sh.count),
                );
                let ex = match exec(&cmd, &env, &log_dir.join("shard.log")) {
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
                        Some(
                            "exit 3 contract: planner declined to shard; plan built unsharded"
                                .into(),
                        ),
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

    // sources: prefetch every line once; a failed fetch fails the line.
    let mut stems = StemSeq::default();
    let mut unit_stems: Vec<Vec<String>> = Vec::new();
    for u in &units {
        unit_stems.push(u.lines.iter().map(|l| stems.next(&l.easyconfig)).collect());
    }
    let mut fetch_failed: Vec<String> = Vec::new();
    if site.sources_on().is_some() {
        let mut any_failed = false;
        for (u, ss) in units.iter().zip(&unit_stems) {
            for (l, s) in u.lines.iter().zip(ss) {
                let cmd = match p.prefetch_cmd(l, s) {
                    Ok(c) => c,
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                };
                let ex = match exec(&cmd, &env, &log_dir.join(format!("{s}.fetch.log"))) {
                    Ok(e) => e,
                    Err(f) => {
                        st.res.message = Some(f.msg);
                        return finish(st, Some(f.code));
                    }
                };
                if let Some(c) = stop_if_signalled(&mut st, "sources") {
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
                    });
                    st.failed = true;
                }
            }
        }
        st.phase(
            "sources",
            if any_failed { "failed" } else { "ok" },
            None,
            None,
        );
    }

    // admission and builds, unit by unit
    let adm = site
        .admission_on()
        .filter(|a| a.quota_cmd.is_some() && a.reservations.is_some());
    let keep_failed = site.logs_on().is_some_and(|l| l.keep_failed);
    let mut stopped = false;
    for (u, ss) in units.iter().zip(&unit_stems) {
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
                });
            }
            continue;
        }
        if let Some(a) = adm {
            let fp = u.footprint.unwrap_or(a.unknown_reserve.unwrap_or(0));
            let cmd = match p.admission_cmd(&u.name, fp) {
                Ok(c) => c,
                Err(f) => {
                    st.res.message = Some(f.msg);
                    return finish(st, Some(f.code));
                }
            };
            let mut attempt = 0u32;
            let mut wait = a.wait_seconds;
            let outcome = loop {
                attempt += 1;
                let ex = match exec(
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
                        wait = (wait.saturating_mul(2)).min(a.max_wait_seconds.max(a.wait_seconds));
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
                        });
                    }
                    continue;
                }
                return finish(st, Some(code));
            }
        }

        for (l, s) in u.lines.iter().zip(ss) {
            if fetch_failed.contains(s) {
                continue;
            }
            if interrupted().is_some() {
                break;
            }
            let cmd = match p.build_cmd(u.role, l, s) {
                Ok(c) => c,
                Err(f) => {
                    st.res.message = Some(f.msg);
                    return finish(st, Some(f.code));
                }
            };
            log("INFO", "build", &target, &format!("{s}: start"));
            let ex = match exec(&cmd, &env, Path::new(&p.log_stdout(s))) {
                Ok(e) => e,
                Err(f) => {
                    st.res.message = Some(f.msg);
                    return finish(st, Some(f.code));
                }
            };
            let staged = p.is_staged(l);
            let mut lr = LineResult {
                easyconfig: l.easyconfig.clone(),
                unit: u.name.clone(),
                phase: "build".into(),
                status: "success".into(),
                exit_code: Some(ex.code),
                translated_from: None,
                log: ex.log.display().to_string(),
                note: None,
            };
            if ex.code != 0 {
                let outcome = if staged {
                    map_staged(ex.code)
                } else {
                    StagedOutcome::Failed
                };
                match outcome {
                    StagedOutcome::Refused => {
                        lr.status = "refused".into();
                        lr.translated_from = Some(ex.code);
                        lr.exit_code = Some(exit::REFUSED);
                        lr.note = Some(
                            "eb-staged-rebuild exit 2 is a refusal; translated to refused (78)"
                                .into(),
                        );
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
                log(
                    "ERROR",
                    "build",
                    &target,
                    &format!("{s}: {} (exit {})", lr.status, ex.code),
                );
                if interrupted().is_none() && keep_failed && lr.status == "failed" {
                    if let Ok(kc) = p.keep_failed_cmd(s, &ex.code.to_string()) {
                        let kx = exec(&kc, &env, &log_dir.join(format!("{s}.keep-logs.log")));
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
                log("INFO", "build", &target, &format!("{s}: ok"));
            }
            st.res.lines.push(lr);
            if interrupted().is_some() {
                break;
            }
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
    let build_failed = st
        .res
        .lines
        .iter()
        .any(|l| l.status == "failed" && l.phase == "build");
    st.phase(
        "build",
        if build_failed || st.refused && st.res.lines.iter().all(|l| l.status != "success") {
            "failed"
        } else {
            "ok"
        },
        None,
        None,
    );

    // caches only after a clean build
    if !st.failed && !st.refused && !st.pending {
        match p.cache_cmd() {
            Ok(Some(cmd)) => {
                log("INFO", "caches", &target, "start");
                match exec(&cmd, &env, &log_dir.join("caches.log")) {
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

    // rotation
    if let Ok(Some(cmd)) = p.rotate_cmd() {
        match exec(&cmd, &env, &log_dir.join("rotate.log")) {
            Ok(ex) if ex.code == 0 => st.phase("logs", "ok", Some(0), None),
            Ok(ex) => {
                let msg = format!("eb-keep-logs rotate exited {}", ex.code);
                log("ERROR", "logs", &target, &msg);
                st.phase("logs", "failed", Some(ex.code), Some(msg));
                st.failed = true;
            }
            Err(f) => {
                st.res.message = Some(f.msg);
                return finish(st, Some(f.code));
            }
        }
        if let Some(c) = stop_if_signalled(&mut st, "logs") {
            return finish(st, Some(c));
        }
    }

    finish(st, None)
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
    st.res.ended_at = chrono::Utc::now().to_rfc3339();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for l in &st.res.lines {
        *counts.entry(l.status.clone()).or_default() += 1;
    }
    st.res.counts = counts;
    st.res.exit_code = match forced.or(st.forced_code) {
        Some(c) => c,
        None if st.failed => exit::FAILED,
        None if st.refused => exit::REFUSED,
        None if st.pending => exit::TEMPFAIL,
        None => exit::OK,
    };
    if st.res.exit_code == exit::UNSHARDED {
        st.res.exit_code = exit::OK;
    }
    st.res
}
