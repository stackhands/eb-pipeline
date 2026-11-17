//! Publish one configured cache synchronization in a CVMFS transaction.

use std::env;
use std::io;
use std::process::{Command, ExitCode, ExitStatus};

struct Args {
    dry_run: bool,
    repository: String,
    sync: Vec<String>,
}

fn main() -> ExitCode {
    match parse(env::args().skip(1)).and_then(run) {
        Ok(code) => ExitCode::from(code),
        Err(message) => {
            eprintln!("eb-cvmfs: {message}");
            ExitCode::from(2)
        }
    }
}

fn parse(words: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut dry_run = false;
    let mut repository = None;
    let mut sync = Vec::new();
    let mut after_separator = false;

    for word in words {
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
            "--" => after_separator = true,
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
        repository,
        sync,
    })
}

fn run(args: Args) -> Result<u8, String> {
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
        println!(
            "{}",
            display_command(&["cvmfs_server", "publish", &args.repository])
        );
        println!(
            "# on sync failure: {}",
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
            abort(&args.repository);
            return Ok(command_start_exit(&error));
        }
    };
    if !sync_status.success() {
        abort(&args.repository);
        return Ok(exit_code(sync_status));
    }

    let published = status(
        Command::new("cvmfs_server")
            .arg("publish")
            .arg(&args.repository),
        "cvmfs_server publish",
    )?;
    Ok(exit_code(published))
}

fn is_repository_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn status(command: &mut Command, name: &str) -> Result<ExitStatus, String> {
    command.status().map_err(|error| start_error(name, error))
}

fn abort(repository: &str) {
    match Command::new("cvmfs_server")
        .args(["abort", "-f", repository])
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("eb-cvmfs: cvmfs_server abort failed with {status}"),
        Err(error) => eprintln!("eb-cvmfs: cannot start cvmfs_server abort: {error}"),
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
}
