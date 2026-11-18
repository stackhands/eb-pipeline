//! Publish one configured cache synchronization in a CVMFS transaction.
//!
//! `--archive-manifest` checks one ingest archive before any `cvmfs_server`
//! command. `--metrics-file` records the invocation after it finishes.

#[path = "../cvmfs_archive.rs"]
mod archive_manifest;

use std::env;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus};
use std::time::Instant;

struct Args {
    dry_run: bool,
    metrics_file: Option<PathBuf>,
    archive_manifest: Option<PathBuf>,
    repository: String,
    verify_cmd: Option<String>,
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
    let mut dry_run = false;
    let mut metrics_file = None;
    let mut archive_manifest = None;
    let mut repository = None;
    let mut verify_cmd = None;
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
                print!("{}", include_str!("eb-cvmfs-help.txt"));
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("eb-cvmfs {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--dry-run" => dry_run = true,
            "--metrics-file" => {
                if metrics_file.is_some() {
                    return Err("--metrics-file may be given only once".into());
                }
                let path = words
                    .next()
                    .ok_or_else(|| "--metrics-file requires PATH".to_string())?;
                if path.is_empty() {
                    return Err("--metrics-file PATH must not be empty".into());
                }
                metrics_file = Some(PathBuf::from(path));
            }
            "--verify-cmd" => {
                if verify_cmd.is_some() {
                    return Err("`--verify-cmd` may be given only once".into());
                }
                verify_cmd = Some(
                    words
                        .next()
                        .ok_or_else(|| "`--verify-cmd` requires CMD".to_string())?,
                );
            }
            "--archive-manifest" => {
                if archive_manifest.is_some() {
                    return Err("`--archive-manifest` was given twice".into());
                }
                let path = words
                    .next()
                    .ok_or_else(|| "`--archive-manifest` needs a path".to_string())?;
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
                metrics_file = Some(PathBuf::from(path));
            }
            _ if word.starts_with("--verify-cmd=") => {
                if verify_cmd.is_some() {
                    return Err("`--verify-cmd` may be given only once".into());
                }
                verify_cmd = Some(word["--verify-cmd=".len()..].to_string());
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
    Ok(Args {
        dry_run,
        metrics_file,
        archive_manifest,
        repository,
        verify_cmd,
        sync,
    })
}

#[derive(Default)]
struct Facts {
    exit_code: u8,
    published_at_seconds: Option<f64>,
    transaction_duration_seconds: Option<f64>,
    aborts: u8,
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
        println!("cvmfs_server list");
        println!(
            "{}",
            display_command(&["cvmfs_server", "transaction", &args.repository])
        );
        println!(
            "{}",
            display_command(&args.sync.iter().map(String::as_str).collect::<Vec<_>>())
        );
        if let Some(command) = &verify {
            println!("{}", display_verify_command(command));
        }
        println!(
            "{}",
            display_command(&["cvmfs_server", "publish", &args.repository])
        );
        let failure = if verify.is_some() {
            "sync or verification failure"
        } else {
            "sync failure"
        };
        println!(
            "# on {failure}: {}",
            display_command(&["cvmfs_server", "abort", "-f", &args.repository])
        );
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
    let sync_status = match Command::new(&args.sync[0]).args(&args.sync[1..]).status() {
        Ok(status) => status,
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
            VerifyCommand::Direct(words) => Command::new(&words[0]).args(&words[1..]).status(),
            VerifyCommand::Shell(script) => Command::new("sh").args(["-c", script]).status(),
        };
        let verify_status = match verify_status {
            Ok(status) => status,
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

    let published = status(
        Command::new("cvmfs_server")
            .arg("publish")
            .arg(&args.repository),
        "cvmfs_server publish",
    )?;
    facts.transaction_duration_seconds =
        Some(eb_pipeline::run::seconds(transaction_started.elapsed()));
    if published.success() {
        facts.published_at_seconds = Some(chrono::Utc::now().timestamp_millis() as f64 / 1000.0);
    }
    Ok(exit_code(published))
}

fn is_repository_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn status(command: &mut Command, name: &str) -> Result<ExitStatus, String> {
    command.status().map_err(|error| start_error(name, error))
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
