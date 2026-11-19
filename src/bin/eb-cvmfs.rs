//! Publish one configured cache synchronization in a CVMFS transaction.
//!
//! `--archive-manifest` checks one ingest archive before any `cvmfs_server`
//! command. `--metrics-file` records the invocation after it finishes.

#[path = "../cvmfs_archive.rs"]
mod archive_manifest;

use std::env;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::time::{Duration, Instant};

const TIMEOUT_EXIT: u8 = 124;
const MAX_TIMEOUT_SECONDS: u64 = i32::MAX as u64;
const TERMINATION_GRACE: Duration = Duration::from_secs(1);

fn write_stdout(bytes: &[u8]) -> Result<(), String> {
    match io::stdout().lock().write_all(bytes) {
        Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(format!("cannot write stdout: {e}")),
        Ok(()) => Ok(()),
    }
}

struct Args {
    check: bool,
    dry_run: bool,
    metrics_file: Option<PathBuf>,
    archive_manifest: Option<PathBuf>,
    repository: String,
    tag: Option<String>,
    tag_description: Option<String>,
    verify_cmd: Option<String>,
    sync_timeout: Option<Duration>,
    verify_timeout: Option<Duration>,
    publish_timeout: Option<Duration>,
    check_timeout: Option<Duration>,
    sync: Vec<String>,
}

enum VerifyCommand {
    Direct(Vec<String>),
    Shell(String),
}

fn main() -> ExitCode {
    let args = match parse(env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("eb-cvmfs: {message}");
            return ExitCode::from(2);
        }
    };
    let metrics = match args
        .metrics_file
        .as_deref()
        .map(MetricsGuard::new)
        .transpose()
    {
        Ok(metrics) => metrics,
        Err(error) => {
            eprintln!(
                "eb-cvmfs: cannot prepare metrics {}: {error}",
                args.metrics_file.as_deref().unwrap().display()
            );
            return ExitCode::from(1);
        }
    };
    let mut facts = Facts::default();
    let code = match run(&args, &mut facts) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("eb-cvmfs: {message}");
            2
        }
    };
    facts.exit_code = code;
    if let Some(metrics) = metrics {
        if let Err(error) = metrics.write(&args.repository, &facts) {
            eprintln!(
                "eb-cvmfs: cannot write metrics {}: {error}",
                metrics.path.display()
            );
            if code == 0 {
                return ExitCode::from(1);
            }
        }
    }
    ExitCode::from(code)
}

fn parse(words: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut check = false;
    let mut dry_run = false;
    let mut metrics_file = None;
    let mut archive_manifest = None;
    let mut repository = None;
    let mut tag = None;
    let mut tag_description = None;
    let mut verify_cmd = None;
    let mut sync_timeout = None;
    let mut verify_timeout = None;
    let mut publish_timeout = None;
    let mut check_timeout = None;
    let mut sync = Vec::new();
    let mut after_separator = false;
    let mut words = words.peekable();

    while let Some(word) = words.next() {
        if after_separator {
            sync.push(word);
            continue;
        }
        match word.as_str() {
            "-h" | "--help" => {
                write_stdout(include_str!("eb-cvmfs-help.txt").as_bytes())?;
                std::process::exit(0);
            }
            "-V" | "--version" => {
                write_stdout(format!("eb-cvmfs {}\n", env!("CARGO_PKG_VERSION")).as_bytes())?;
                std::process::exit(0);
            }
            "--check" => check = true,
            "--dry-run" => dry_run = true,
            "--metrics-file" => {
                if metrics_file.is_some() {
                    return Err("--metrics-file may be given only once".into());
                }
                let path =
                    eb_pipeline::util::separate_value(&mut words, "--metrics-file requires PATH")?;
                if path.is_empty() {
                    return Err("--metrics-file PATH must not be empty".into());
                }
                metrics_file = Some(PathBuf::from(path));
            }
            "--tag" => {
                if tag.is_some() {
                    return Err("`--tag` may be given only once".into());
                }
                let name = eb_pipeline::util::separate_value(&mut words, "`--tag` requires NAME")?;
                if name.is_empty() {
                    return Err("`--tag` must not be empty".into());
                }
                tag = Some(name);
            }
            "--tag-description" => {
                if tag_description.is_some() {
                    return Err("`--tag-description` may be given only once".into());
                }
                let text = eb_pipeline::util::separate_value(
                    &mut words,
                    "`--tag-description` requires TEXT",
                )?;
                if text.is_empty() {
                    return Err("`--tag-description` must not be empty".into());
                }
                tag_description = Some(text);
            }
            "--verify-cmd" => {
                if verify_cmd.is_some() {
                    return Err("`--verify-cmd` may be given only once".into());
                }
                verify_cmd = Some(eb_pipeline::util::separate_value(
                    &mut words,
                    "`--verify-cmd` requires CMD",
                )?);
            }
            "--sync-timeout" => {
                let value =
                    eb_pipeline::util::separate_value(&mut words, "`--sync-timeout` requires SEC")?;
                set_timeout(&mut sync_timeout, "--sync-timeout", Some(value))?;
            }
            "--verify-timeout" => {
                let value = eb_pipeline::util::separate_value(
                    &mut words,
                    "`--verify-timeout` requires SEC",
                )?;
                set_timeout(&mut verify_timeout, "--verify-timeout", Some(value))?;
            }
            "--publish-timeout" => {
                let value = eb_pipeline::util::separate_value(
                    &mut words,
                    "`--publish-timeout` requires SEC",
                )?;
                set_timeout(&mut publish_timeout, "--publish-timeout", Some(value))?;
            }
            "--check-timeout" => {
                let value = eb_pipeline::util::separate_value(
                    &mut words,
                    "`--check-timeout` requires SEC",
                )?;
                set_timeout(&mut check_timeout, "--check-timeout", Some(value))?;
            }
            "--archive-manifest" => {
                if archive_manifest.is_some() {
                    return Err("`--archive-manifest` was given twice".into());
                }
                let path = eb_pipeline::util::separate_value(
                    &mut words,
                    "`--archive-manifest` needs a path",
                )?;
                if path.is_empty() || path == "--" {
                    return Err("`--archive-manifest` needs a path".into());
                }
                archive_manifest = Some(PathBuf::from(path));
            }
            "--" => after_separator = true,
            _ if word.starts_with("--metrics-file=") => {
                if metrics_file.is_some() {
                    return Err("--metrics-file may be given only once".into());
                }
                let path = word.trim_start_matches("--metrics-file=");
                if path.is_empty() {
                    return Err("--metrics-file PATH must not be empty".into());
                }
                if path == "--" {
                    return Err("--metrics-file requires PATH".into());
                }
                metrics_file = Some(PathBuf::from(path));
            }
            _ if word.starts_with("--archive-manifest=") => {
                if archive_manifest.is_some() {
                    return Err("`--archive-manifest` was given twice".into());
                }
                let path = &word["--archive-manifest=".len()..];
                if path.is_empty() || path == "--" {
                    return Err("`--archive-manifest` needs a path".into());
                }
                archive_manifest = Some(PathBuf::from(path));
            }
            _ if word.starts_with("--tag=") => {
                if tag.is_some() {
                    return Err("`--tag` may be given only once".into());
                }
                let name = word["--tag=".len()..].to_string();
                if name.is_empty() {
                    return Err("`--tag` must not be empty".into());
                }
                tag = Some(name);
            }
            _ if word.starts_with("--tag-description=") => {
                if tag_description.is_some() {
                    return Err("`--tag-description` may be given only once".into());
                }
                let text = word["--tag-description=".len()..].to_string();
                if text.is_empty() {
                    return Err("`--tag-description` must not be empty".into());
                }
                tag_description = Some(text);
            }
            _ if word.starts_with("--verify-cmd=") => {
                if verify_cmd.is_some() {
                    return Err("`--verify-cmd` may be given only once".into());
                }
                verify_cmd = Some(word["--verify-cmd=".len()..].to_string());
            }
            _ if word.starts_with("--sync-timeout=") => {
                set_timeout(
                    &mut sync_timeout,
                    "--sync-timeout",
                    Some(word["--sync-timeout=".len()..].to_string()),
                )?;
            }
            _ if word.starts_with("--verify-timeout=") => {
                set_timeout(
                    &mut verify_timeout,
                    "--verify-timeout",
                    Some(word["--verify-timeout=".len()..].to_string()),
                )?;
            }
            _ if word.starts_with("--publish-timeout=") => {
                set_timeout(
                    &mut publish_timeout,
                    "--publish-timeout",
                    Some(word["--publish-timeout=".len()..].to_string()),
                )?;
            }
            _ if word.starts_with("--check-timeout=") => {
                set_timeout(
                    &mut check_timeout,
                    "--check-timeout",
                    Some(word["--check-timeout=".len()..].to_string()),
                )?;
            }
            _ if word.starts_with('-') => return Err(format!("does not know `{word}`")),
            _ if repository.is_none() => repository = Some(word),
            _ => return Err("put `--` between REPOSITORY and the sync command".into()),
        }
    }

    let repository = repository.ok_or_else(|| "REPOSITORY is required".to_string())?;
    if repository.is_empty() {
        return Err("REPOSITORY must not be empty".into());
    }
    if sync.is_empty() {
        return Err("a sync command is required after `--`".into());
    }
    if tag_description.is_some() && tag.is_none() {
        return Err("`--tag-description` requires `--tag`".into());
    }
    if verify_timeout.is_some() && verify_cmd.is_none() {
        return Err("`--verify-timeout` requires `--verify-cmd`".into());
    }
    if check_timeout.is_some() && !check {
        return Err("`--check-timeout` requires `--check`".into());
    }
    Ok(Args {
        check,
        dry_run,
        metrics_file,
        archive_manifest,
        repository,
        tag,
        tag_description,
        verify_cmd,
        sync_timeout,
        verify_timeout,
        publish_timeout,
        check_timeout,
        sync,
    })
}

fn set_timeout(
    timeout: &mut Option<Duration>,
    option: &str,
    value: Option<String>,
) -> Result<(), String> {
    if timeout.is_some() {
        return Err(format!("`{option}` may be given only once"));
    }
    let value = value.ok_or_else(|| format!("`{option}` requires SEC"))?;
    let seconds: u64 = value
        .parse()
        .map_err(|_| format!("`{option}` SEC must be a positive integer"))?;
    if seconds == 0 {
        return Err(format!("`{option}` SEC must be a positive integer"));
    }
    if seconds > MAX_TIMEOUT_SECONDS {
        return Err(format!("`{option}` SEC is too large"));
    }
    *timeout = Some(Duration::from_secs(seconds));
    Ok(())
}

#[derive(Default)]
struct Facts {
    exit_code: u8,
    published_at_seconds: Option<f64>,
    transaction_duration_seconds: Option<f64>,
    aborts: u8,
    timed_out: Option<&'static str>,
}

struct MetricsGuard {
    path: PathBuf,
    _lock: File,
}

impl MetricsGuard {
    fn new(path: &Path) -> io::Result<Self> {
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let name = path.file_name().unwrap_or_else(|| "metrics".as_ref());
        let mut lock_name = std::ffi::OsString::from(".");
        lock_name.push(name);
        lock_name.push(".lock");
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(parent.join(lock_name))?;
        // SAFETY: `flock` receives a live file descriptor and does not retain it.
        let status = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
        if status != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            path: path.to_path_buf(),
            _lock: lock,
        })
    }

    fn write(&self, repository: &str, facts: &Facts) -> io::Result<()> {
        let previous = previous_publish(&self.path, repository);
        let published = facts.published_at_seconds.or(previous);
        eb_pipeline::metrics::write_atomic(&self.path, &prometheus(repository, facts, published))
    }
}

fn previous_publish(path: &Path, repository: &str) -> Option<f64> {
    let text = std::fs::read_to_string(path).ok()?;
    let prefix = format!(
        "eb_cvmfs_last_publish_timestamp_seconds{{repository=\"{}\"}} ",
        escape_label(repository)
    );
    text.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .and_then(|value| value.parse().ok())
}

fn prometheus(repository: &str, facts: &Facts, published: Option<f64>) -> String {
    let label = format!("repository=\"{}\"", escape_label(repository));
    let mut out = String::new();
    metric(
        &mut out,
        "eb_cvmfs_last_result",
        "Whether the latest invocation succeeded: 1 for success and 0 otherwise.",
        &label,
        if facts.exit_code == 0 { "1" } else { "0" },
    );
    metric(
        &mut out,
        "eb_cvmfs_exit_code",
        "Exit code of the latest invocation.",
        &label,
        &facts.exit_code.to_string(),
    );
    if let Some(timestamp) = published {
        metric(
            &mut out,
            "eb_cvmfs_last_publish_timestamp_seconds",
            "Unix timestamp of the latest successful publish known in this file.",
            &label,
            &timestamp.to_string(),
        );
    }
    if let Some(duration) = facts.transaction_duration_seconds {
        metric(
            &mut out,
            "eb_cvmfs_last_transaction_duration_seconds",
            "Duration from transaction start through completed publish or abort.",
            &label,
            &duration.to_string(),
        );
    }
    metric(
        &mut out,
        "eb_cvmfs_aborts",
        "Abort commands started by the latest invocation.",
        &label,
        &facts.aborts.to_string(),
    );
    if let Some(phase) = facts.timed_out {
        metric(
            &mut out,
            "eb_cvmfs_timeout",
            "Whether the latest invocation timed out, labelled by phase.",
            &format!("{label},phase=\"{phase}\""),
            "1",
        );
    }
    out
}

fn metric(out: &mut String, name: &str, help: &str, label: &str, value: &str) {
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&format!(
        "# HELP {name} {help}\n# TYPE {name} gauge\n{name}{{{label}}} {value}\n"
    ));
}

fn escape_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
}

fn run(args: &Args, facts: &mut Facts) -> Result<u8, String> {
    if let Some(path) = &args.archive_manifest {
        archive_manifest::check(path)?;
    }
    let verify = args
        .verify_cmd
        .as_deref()
        .map(parse_verify_command)
        .transpose()?;
    if args.dry_run {
        let mut plan = String::new();
        plan.push_str("cvmfs_server list\n");
        plan.push_str(&display_command(&[
            "cvmfs_server",
            "transaction",
            &args.repository,
        ]));
        plan.push('\n');
        plan.push_str(&display_command(
            &args.sync.iter().map(String::as_str).collect::<Vec<_>>(),
        ));
        plan.push('\n');
        if let Some(command) = &verify {
            plan.push_str(&display_verify_command(command));
            plan.push('\n');
        }
        plan.push_str(&display_publish_command(args));
        plan.push('\n');
        if args.check {
            plan.push_str(&display_command(&[
                "cvmfs_server",
                "check",
                &args.repository,
            ]));
            plan.push('\n');
        }
        let failure = if verify.is_some() {
            "sync or verification failure"
        } else {
            "sync failure"
        };
        plan.push_str(&format!(
            "# on {failure}: {}\n",
            display_command(&["cvmfs_server", "abort", "-f", &args.repository])
        ));
        write_stdout(plan.as_bytes())?;
        return Ok(0);
    }

    let listed = Command::new("cvmfs_server")
        .arg("list")
        .output()
        .map_err(|error| start_error("cvmfs_server list", error))?;
    if !listed.status.success() {
        return Err(command_error(
            "cvmfs_server list",
            listed.status,
            &listed.stderr,
        ));
    }
    let repositories = String::from_utf8_lossy(&listed.stdout);
    if repositories.lines().any(|line| {
        line.split_whitespace()
            .any(|field| field.trim_matches(|c: char| !is_repository_char(c)) == args.repository)
            && line.contains("in transaction")
    }) {
        return Err(format!(
            "repository `{}` is already in a transaction",
            args.repository
        ));
    }

    let transaction_started = Instant::now();
    let transaction = status(
        Command::new("cvmfs_server")
            .arg("transaction")
            .arg(&args.repository),
        "cvmfs_server transaction",
    )?;
    if !transaction.success() {
        return Ok(exit_code(transaction));
    }

    let sync_name = display_command(&args.sync.iter().map(String::as_str).collect::<Vec<_>>());
    let sync_status = match grouped_status(
        Command::new(&args.sync[0]).args(&args.sync[1..]),
        args.sync_timeout,
    ) {
        Ok(GroupStatus::Exited(status)) => status,
        Ok(GroupStatus::TimedOut) => {
            timeout_message("sync", args.sync_timeout.unwrap());
            facts.timed_out = Some("sync");
            let completed = abort(&args.repository, facts);
            if completed {
                facts.transaction_duration_seconds =
                    Some(eb_pipeline::run::seconds(transaction_started.elapsed()));
            }
            return Ok(TIMEOUT_EXIT);
        }
        Err(error) => {
            eprintln!("eb-cvmfs: cannot start sync command `{sync_name}`: {error}");
            let completed = abort(&args.repository, facts);
            if completed {
                facts.transaction_duration_seconds =
                    Some(eb_pipeline::run::seconds(transaction_started.elapsed()));
            }
            return Ok(command_start_exit(&error));
        }
    };
    if !sync_status.success() {
        let completed = abort(&args.repository, facts);
        if completed {
            facts.transaction_duration_seconds =
                Some(eb_pipeline::run::seconds(transaction_started.elapsed()));
        }
        return Ok(exit_code(sync_status));
    }

    if let Some(command) = &verify {
        let verify_name = display_verify_command(command);
        let verify_status = match command {
            VerifyCommand::Direct(words) => grouped_status(
                Command::new(&words[0]).args(&words[1..]),
                args.verify_timeout,
            ),
            VerifyCommand::Shell(script) => {
                grouped_status(Command::new("sh").args(["-c", script]), args.verify_timeout)
            }
        };
        let verify_status = match verify_status {
            Ok(GroupStatus::Exited(status)) => status,
            Ok(GroupStatus::TimedOut) => {
                timeout_message("verification", args.verify_timeout.unwrap());
                facts.timed_out = Some("verify");
                let completed = abort(&args.repository, facts);
                if completed {
                    facts.transaction_duration_seconds =
                        Some(eb_pipeline::run::seconds(transaction_started.elapsed()));
                }
                return Ok(TIMEOUT_EXIT);
            }
            Err(error) => {
                eprintln!("eb-cvmfs: cannot start verification command `{verify_name}`: {error}");
                let completed = abort(&args.repository, facts);
                if completed {
                    facts.transaction_duration_seconds =
                        Some(eb_pipeline::run::seconds(transaction_started.elapsed()));
                }
                return Ok(command_start_exit(&error));
            }
        };
        if !verify_status.success() {
            let completed = abort(&args.repository, facts);
            if completed {
                facts.transaction_duration_seconds =
                    Some(eb_pipeline::run::seconds(transaction_started.elapsed()));
            }
            return Ok(exit_code(verify_status));
        }
    }

    let mut publish = configure_publish(args);
    let published = match grouped_status(&mut publish, args.publish_timeout) {
        Ok(GroupStatus::Exited(status)) => status,
        Ok(GroupStatus::TimedOut) => {
            timeout_message("publish", args.publish_timeout.unwrap());
            facts.timed_out = Some("publish");
            let completed = abort(&args.repository, facts);
            if completed {
                facts.transaction_duration_seconds =
                    Some(eb_pipeline::run::seconds(transaction_started.elapsed()));
            }
            return Ok(TIMEOUT_EXIT);
        }
        Err(error) => return Err(start_error("cvmfs_server publish", error)),
    };
    facts.transaction_duration_seconds =
        Some(eb_pipeline::run::seconds(transaction_started.elapsed()));
    if !published.success() {
        return Ok(exit_code(published));
    }
    facts.published_at_seconds = Some(chrono::Utc::now().timestamp_millis() as f64 / 1000.0);
    if !args.check {
        return Ok(0);
    }

    let checked = match grouped_status(
        Command::new("cvmfs_server")
            .arg("check")
            .arg(&args.repository),
        args.check_timeout,
    ) {
        Ok(GroupStatus::Exited(status)) => status,
        Ok(GroupStatus::TimedOut) => {
            timeout_message("check", args.check_timeout.unwrap());
            facts.timed_out = Some("check");
            return Ok(TIMEOUT_EXIT);
        }
        Err(error) => return Err(start_error("cvmfs_server check", error)),
    };
    Ok(exit_code(checked))
}

fn configure_publish(args: &Args) -> Command {
    let mut command = Command::new("cvmfs_server");
    command.arg("publish");
    if let Some(name) = &args.tag {
        command.arg("-a").arg(name);
    }
    if let Some(text) = &args.tag_description {
        command.arg("-m").arg(text);
    }
    command.arg(&args.repository);
    command
}

fn display_publish_command(args: &Args) -> String {
    let mut words = vec!["cvmfs_server", "publish"];
    if let Some(name) = &args.tag {
        words.push("-a");
        words.push(name);
    }
    if let Some(text) = &args.tag_description {
        words.push("-m");
        words.push(text);
    }
    words.push(&args.repository);
    display_command(&words)
}

fn is_repository_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn status(command: &mut Command, name: &str) -> Result<ExitStatus, String> {
    command.status().map_err(|error| start_error(name, error))
}

enum GroupStatus {
    Exited(ExitStatus),
    TimedOut,
}

fn grouped_status(command: &mut Command, timeout: Option<Duration>) -> io::Result<GroupStatus> {
    let Some(timeout) = timeout else {
        return command.status().map(GroupStatus::Exited);
    };
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "timeout is too large"))?;
    // SAFETY: `setpgid` is async-signal-safe, and the closure captures nothing.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
    }
    let mut child = command.spawn()?;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(GroupStatus::Exited(status));
        }
        if Instant::now() >= deadline {
            terminate_group(&mut child);
            return Ok(GroupStatus::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn terminate_group(child: &mut Child) {
    signal_group(child.id(), libc::SIGTERM);
    std::thread::sleep(TERMINATION_GRACE);
    signal_group(child.id(), libc::SIGKILL);
    let _ = child.wait();
}

fn signal_group(id: u32, signal: libc::c_int) {
    // SAFETY: the negative pid addresses the process group created for this
    // child. The unreaped group leader prevents its pid from being reused.
    unsafe {
        libc::kill(-(id as libc::pid_t), signal);
    }
}

fn timeout_message(phase: &str, timeout: Duration) {
    let timeout_unit = if timeout.as_secs() == 1 {
        "second"
    } else {
        "seconds"
    };
    let grace_unit = if TERMINATION_GRACE.as_secs() == 1 {
        "second"
    } else {
        "seconds"
    };
    eprintln!(
        "eb-cvmfs: {phase} command timed out after {} {timeout_unit}; sent TERM, waited {} {grace_unit}, then sent KILL",
        timeout.as_secs(),
        TERMINATION_GRACE.as_secs()
    );
}

fn abort(repository: &str, facts: &mut Facts) -> bool {
    match Command::new("cvmfs_server")
        .args(["abort", "-f", repository])
        .status()
    {
        Ok(status) => {
            facts.aborts = 1;
            if !status.success() {
                eprintln!("eb-cvmfs: cvmfs_server abort failed with {status}");
            }
            true
        }
        Err(error) => {
            eprintln!("eb-cvmfs: cannot start cvmfs_server abort: {error}");
            false
        }
    }
}

fn start_error(name: &str, error: io::Error) -> String {
    format!("cannot start {name}: {error}")
}

fn command_error(name: &str, status: ExitStatus, stderr: &[u8]) -> String {
    let detail = String::from_utf8_lossy(stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        format!("{name} failed with {status}")
    } else {
        format!("{name} failed with {status}: {detail}")
    }
}

fn command_start_exit(error: &io::Error) -> u8 {
    if error.kind() == io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

fn exit_code(status: ExitStatus) -> u8 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .and_then(|code| u8::try_from(code).ok())
        .unwrap_or(1)
}

fn parse_verify_command(command: &str) -> Result<VerifyCommand, String> {
    if command.trim().is_empty() {
        return Err("`--verify-cmd` must not be empty".into());
    }
    if requires_shell(command) {
        return Ok(VerifyCommand::Shell(command.to_string()));
    }
    let words = shell_words::split(command)
        .map_err(|error| format!("cannot parse `--verify-cmd`: {error}"))?;
    if words.is_empty() {
        return Err("`--verify-cmd` must not be empty".into());
    }
    if is_assignment(&words[0]) || is_shell_word(&words[0]) {
        Ok(VerifyCommand::Shell(command.to_string()))
    } else {
        Ok(VerifyCommand::Direct(words))
    }
}

fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
        })
}

fn is_shell_word(word: &str) -> bool {
    matches!(
        word,
        "." | ":"
            | "alias"
            | "bg"
            | "break"
            | "case"
            | "cd"
            | "command"
            | "continue"
            | "do"
            | "done"
            | "elif"
            | "else"
            | "esac"
            | "eval"
            | "exec"
            | "exit"
            | "export"
            | "fc"
            | "fg"
            | "fi"
            | "for"
            | "getopts"
            | "hash"
            | "if"
            | "in"
            | "jobs"
            | "read"
            | "readonly"
            | "return"
            | "set"
            | "shift"
            | "source"
            | "then"
            | "times"
            | "trap"
            | "type"
            | "ulimit"
            | "umask"
            | "unalias"
            | "unset"
            | "until"
            | "wait"
            | "while"
    )
}

fn requires_shell(command: &str) -> bool {
    #[derive(Clone, Copy)]
    enum Quote {
        Unquoted,
        Single,
        Double,
    }

    let mut quote = Quote::Unquoted;
    let mut escaped = false;
    let mut word_start = true;
    for character in command.chars() {
        if escaped {
            escaped = false;
            word_start = false;
            continue;
        }
        match quote {
            Quote::Single => {
                if character == '\'' {
                    quote = Quote::Unquoted;
                }
            }
            Quote::Double => match character {
                '"' => quote = Quote::Unquoted,
                '\\' => escaped = true,
                '$' | '`' => return true,
                _ => {}
            },
            Quote::Unquoted => match character {
                '\'' => {
                    quote = Quote::Single;
                    word_start = false;
                }
                '"' => {
                    quote = Quote::Double;
                    word_start = false;
                }
                '\\' => {
                    escaped = true;
                    word_start = false;
                }
                '|' | '&' | ';' | '<' | '>' | '(' | ')' | '$' | '`' | '\n' => return true,
                '*' | '?' | '[' => return true,
                '~' | '#' | '!' if word_start => return true,
                character if character.is_whitespace() => word_start = true,
                _ => word_start = false,
            },
        }
    }
    false
}

fn display_verify_command(command: &VerifyCommand) -> String {
    match command {
        VerifyCommand::Direct(words) => {
            display_command(&words.iter().map(String::as_str).collect::<Vec<_>>())
        }
        VerifyCommand::Shell(script) => display_command(&["sh", "-c", script]),
    }
}

fn display_command(words: &[&str]) -> String {
    words
        .iter()
        .map(|word| shell_word(word))
        .collect::<Vec<_>>()
        .join(" ")
}

fn shell_word(word: &str) -> String {
    if !word.is_empty()
        && word
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_+-./:=,@%".contains(&byte))
    {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_display_quotes_only_words_that_need_it() {
        assert_eq!(
            display_command(&["sync", "--root=/cvmfs/repo", "two words", "it's"]),
            "sync --root=/cvmfs/repo 'two words' 'it'\\''s'"
        );
    }

    #[test]
    fn prometheus_escapes_the_repository_label() {
        let text = prometheus(
            "repo\"with\\newline\n",
            &Facts {
                exit_code: 1,
                aborts: 1,
                ..Facts::default()
            },
            None,
        );
        assert!(
            text.contains(r#"repository="repo\"with\\newline\n""#),
            "{text}"
        );
        assert!(text
            .lines()
            .all(|line| !line.starts_with("eb_") || { line.starts_with("eb_cvmfs_") }));
    }

    #[test]
    fn verification_uses_a_shell_only_for_shell_syntax() {
        let direct = parse_verify_command("check --path 'two words'").unwrap();
        assert_eq!(display_verify_command(&direct), "check --path 'two words'");

        let shell = parse_verify_command("test -s \"$CACHE\" && check").unwrap();
        assert_eq!(
            display_verify_command(&shell),
            "sh -c 'test -s \"$CACHE\" && check'"
        );

        let builtin = parse_verify_command("command -v check").unwrap();
        assert_eq!(display_verify_command(&builtin), "sh -c 'command -v check'");

        let heredoc = parse_verify_command("cat <<'EOF'\n'\nEOF").unwrap();
        assert_eq!(
            display_verify_command(&heredoc),
            "sh -c 'cat <<'\\''EOF'\\''\n'\\''\nEOF'"
        );
    }
}
